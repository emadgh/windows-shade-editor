use crate::*;
use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use windows_shade_editor::color_conversion::ConversionRecipe;
use windows_shade_editor::conversion_analytics::ConversionUsageReport;
use windows_shade_editor::conversion_candidate_comparison::{
    CandidateComparison, CandidateComparisonSnapshot, compare_candidate_snapshots,
};
use windows_shade_editor::conversion_candidate_preview::{
    CandidatePreviewInput, CandidatePreviewResult, render_candidate_preview,
};
use windows_shade_editor::conversion_candidate_promotion::CandidatePromotionSnapshot;
use windows_shade_editor::conversion_recipe::recipe_sha256;
use windows_shade_editor::conversion_transaction::{
    CapturedSourceProfile, ConversionCancellation,
};
use windows_shade_editor::icc_conversion::IccSourceModel;

use super::conversion_plan::{ConversionFaceInspection, target_channel_rgb};

const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(220);

struct PendingCandidate {
    key: String,
    source_state_id: String,
    source_path: PathBuf,
    recipe: ConversionRecipe,
    generation: u64,
    cancellation: ConversionCancellation,
    rx: mpsc::Receiver<Result<CandidatePreviewResult, String>>,
}

struct ActiveCandidate {
    key: String,
    source_state_id: String,
    source_path: PathBuf,
    recipe: ConversionRecipe,
    face_index: usize,
    project_revision: u64,
    result: CandidatePreviewResult,
    solo_channel: Option<usize>,
    texture: egui::TextureHandle,
}

pub(crate) struct CandidatePreviewController {
    desired_key: Option<String>,
    desired_recipe: Option<ConversionRecipe>,
    debounce_started: Option<Instant>,
    generation: u64,
    pending: Option<PendingCandidate>,
    active: Option<ActiveCandidate>,
    pinned: Option<CandidatePromotionSnapshot>,
    error: Option<String>,
    show_converted: bool,
}

impl Default for CandidatePreviewController {
    fn default() -> Self {
        Self {
            desired_key: None,
            desired_recipe: None,
            debounce_started: None,
            generation: 0,
            pending: None,
            active: None,
            pinned: None,
            error: None,
            show_converted: true,
        }
    }
}

#[derive(Clone)]
struct CandidateRuntimeSource {
    face_index: usize,
    source_path: PathBuf,
    source_model: IccSourceModel,
    captured_profile: CapturedSourceProfile,
    embedded_source_icc: Option<Vec<u8>>,
    width: usize,
    height: usize,
}

#[derive(Clone)]
pub(crate) struct CandidateStatusSnapshot {
    pub(crate) active: bool,
    pub(crate) pending: bool,
    pub(crate) show_converted: bool,
    pub(crate) recipe_sha256: Option<String>,
    pub(crate) pinned_recipe_sha256: Option<String>,
    pub(crate) channel_count: usize,
    pub(crate) error: Option<String>,
}

#[derive(Clone)]
pub(crate) struct CandidatePromotionSelection {
    pub(crate) snapshot: CandidatePromotionSnapshot,
    pub(crate) face_index: usize,
    pub(crate) project_revision: u64,
    pub(crate) source_path: PathBuf,
}

impl ShadeApp {
    /// Poll only the render/cache runtime. Target selection and conversion intent live in the
    /// unified Production Color Conversion window; this controller owns no independent config.
    pub(crate) fn poll_conversion_candidate_runtime(&mut self, ctx: &egui::Context) {
        self.poll_candidate_result(ctx);
        self.apply_candidate_texture();
    }

    pub(crate) fn sync_conversion_candidate(
        &mut self,
        inspection: &ConversionFaceInspection,
        recipe: &ConversionRecipe,
        force: bool,
        ctx: &egui::Context,
    ) {
        if inspection.index != self.current_face || !inspection.ready() {
            self.invalidate_conversion_candidate();
            return;
        }
        if inspection.transparency
            == windows_shade_editor::design_source::TransparencyState::PresentUnresolved
        {
            self.conversion_candidate.error = Some(
                "Candidate preview waits until Source transparency is resolved. Final conversion keeps the explicit per-Face flatten policy."
                    .to_owned(),
            );
            self.invalidate_conversion_candidate_render_only();
            return;
        }
        let source_state_id = candidate_source_state_id(
            inspection.index,
            self.project_revision,
            &inspection.source_path,
        );
        if self
            .conversion_candidate
            .pinned
            .as_ref()
            .is_some_and(|pinned| pinned.source_state_id() != source_state_id)
        {
            self.conversion_candidate.pinned = None;
        }
        let key = candidate_key(
            inspection.index,
            self.project_revision,
            &inspection.source_path,
            recipe,
        );
        self.observe_candidate(key, recipe.clone(), force, ctx);
    }

    pub(crate) fn clear_conversion_candidate(&mut self) {
        if let Some(pending) = self.conversion_candidate.pending.take() {
            pending.cancellation.request();
        }
        let removed = self.conversion_candidate.active.take().is_some();
        self.conversion_candidate.desired_key = None;
        self.conversion_candidate.desired_recipe = None;
        self.conversion_candidate.debounce_started = None;
        self.conversion_candidate.error = None;
        self.conversion_candidate.show_converted = true;
        if removed {
            self.force_source_preview_refresh();
        }
    }

    pub(crate) fn set_conversion_candidate_visible(
        &mut self,
        visible: bool,
        ctx: &egui::Context,
    ) {
        if self.conversion_candidate.show_converted == visible {
            return;
        }
        self.conversion_candidate.show_converted = visible;
        if visible {
            self.apply_candidate_texture();
        } else {
            self.force_source_preview_refresh();
        }
        ctx.request_repaint();
    }

    pub(crate) fn conversion_candidate_status(&self) -> CandidateStatusSnapshot {
        let active = self
            .conversion_candidate
            .active
            .as_ref()
            .filter(|active| {
                active.face_index == self.current_face
                    && active.project_revision == self.project_revision
            });
        CandidateStatusSnapshot {
            active: active.is_some(),
            pending: self.conversion_candidate.pending.is_some(),
            show_converted: self.conversion_candidate.show_converted,
            recipe_sha256: active.map(|active| active.result.recipe_sha256.clone()),
            pinned_recipe_sha256: self
                .conversion_candidate
                .pinned
                .as_ref()
                .map(|pinned| pinned.recipe_sha256().to_owned()),
            channel_count: active.map(|active| active.result.channel_count()).unwrap_or(0),
            error: self.conversion_candidate.error.clone(),
        }
    }

    pub(crate) fn conversion_candidate_matches_current(
        &self,
        recipe: &ConversionRecipe,
    ) -> bool {
        let expected = recipe_sha256(recipe).unwrap_or_default();
        self.conversion_candidate
            .active
            .as_ref()
            .is_some_and(|active| {
                active.face_index == self.current_face
                    && active.project_revision == self.project_revision
                    && active.result.recipe_sha256 == expected
            })
    }

    pub(crate) fn conversion_candidate_a_promotion_selection(
        &self,
    ) -> Result<CandidatePromotionSelection, String> {
        let pinned = self
            .conversion_candidate
            .pinned
            .as_ref()
            .ok_or_else(|| "Pin a Candidate as A before promoting A to final production.".to_owned())?;
        let active = self
            .conversion_candidate
            .active
            .as_ref()
            .filter(|active| {
                active.face_index == self.current_face
                    && active.project_revision == self.project_revision
            })
            .ok_or_else(|| {
                "A current Production Candidate is required to revalidate Candidate A Source state."
                    .to_owned()
            })?;
        if pinned.source_state_id() != active.source_state_id {
            return Err(
                "Candidate A no longer belongs to the current exact Source state; pin A again."
                    .to_owned(),
            );
        }
        Ok(CandidatePromotionSelection {
            snapshot: pinned.clone(),
            face_index: active.face_index,
            project_revision: active.project_revision,
            source_path: active.source_path.clone(),
        })
    }

    pub(crate) fn conversion_candidate_b_promotion_selection(
        &self,
    ) -> Result<CandidatePromotionSelection, String> {
        let active = self
            .conversion_candidate
            .active
            .as_ref()
            .filter(|active| {
                active.face_index == self.current_face
                    && active.project_revision == self.project_revision
            })
            .ok_or_else(|| "No current Production Candidate B is ready to promote.".to_owned())?;
        let snapshot = CandidatePromotionSnapshot::from_preview(
            active.source_state_id.clone(),
            &active.recipe,
            &active.result,
        )?;
        Ok(CandidatePromotionSelection {
            snapshot,
            face_index: active.face_index,
            project_revision: active.project_revision,
            source_path: active.source_path.clone(),
        })
    }

    /// Replace the ordinary Source channel/histogram panel while the converted candidate is
    /// visible. These rows are inspection-only: clicking an ink changes candidate solo view and
    /// reuses cached converted samples instead of mutating Source adjustments or rerunning ICC.
    pub(crate) fn ui_conversion_candidate_channels_histogram(
        &mut self,
        ui: &mut egui::Ui,
    ) -> bool {
        if !self.conversion_candidate.show_converted {
            return false;
        }
        let Some(active) = self.conversion_candidate.active.as_ref() else {
            return false;
        };
        if active.face_index != self.current_face || active.project_revision != self.project_revision {
            return false;
        }

        let channels = active.result.channels.clone();
        let histograms = active.result.histograms.clone();
        let usage = active.result.usage.clone();
        let solo = active.solo_channel;
        let recipe_sha = active.result.recipe_sha256.clone();
        let pinned_recipe_sha = self
            .conversion_candidate
            .pinned
            .as_ref()
            .map(|pinned| pinned.recipe_sha256().to_owned());
        let comparison = self.current_candidate_comparison();
        let mut requested_solo: Option<Option<usize>> = None;
        let mut pin_current = false;
        let mut clear_pin = false;

        ui.horizontal(|ui| {
            ui.heading("Production Candidate");
            ui.label(
                egui::RichText::new(format!("{} inks", channels.len()))
                    .color(egui::Color32::LIGHT_GREEN),
            );
        });
        ui.small(format!("Converted target samples · recipe {}", short_hash(&recipe_sha)));

        ui.group(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("A/B comparison");
                if ui
                    .button(if pinned_recipe_sha.is_some() {
                        "Replace A with current"
                    } else {
                        "Pin current as A"
                    })
                    .on_hover_text(
                        "Pins the exact rendered recipe plus bounded Candidate analytics/identity. Candidate raster planes are not duplicated.",
                    )
                    .clicked()
                {
                    pin_current = true;
                }
                if pinned_recipe_sha.is_some() && ui.small_button("Clear A").clicked() {
                    clear_pin = true;
                }
            });
            if let Some(hash) = pinned_recipe_sha.as_deref() {
                ui.small(format!("A · recipe {}", short_hash(hash)));
                match comparison.as_ref() {
                    Ok(Some(comparison)) => draw_candidate_comparison(ui, comparison),
                    Ok(None) => {
                        ui.small(
                            "Current Candidate B matches A. Change conversion recipe/preset to compare a distinct Candidate.",
                        );
                    }
                    Err(error) => {
                        ui.label(egui::RichText::new(error).color(egui::Color32::YELLOW));
                    }
                }
            } else {
                ui.small(
                    "Pin the current converted Candidate as A, then change recipe/preset to compare the next real Candidate as B.",
                );
            }
        });

        if ui
            .selectable_label(solo.is_none(), "Composite converted preview")
            .clicked()
        {
            requested_solo = Some(None);
        }
        for (index, channel) in channels.iter().enumerate() {
            let rgb = channel
                .display_rgb
                .unwrap_or_else(|| target_channel_rgb(&channel.name, index));
            ui.horizontal(|ui| {
                ui.colored_label(egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]), "■");
                if ui
                    .selectable_label(solo == Some(index), format!("{}  {}", index + 1, channel.name))
                    .on_hover_text("Inspect this converted ink directly; no production transform is rerun.")
                    .clicked()
                {
                    requested_solo = Some(if solo == Some(index) { None } else { Some(index) });
                }
            });
        }
        if solo.is_some() && ui.small_button("Return to converted composite").clicked() {
            requested_solo = Some(None);
        }

        ui.separator();
        draw_candidate_usage(ui, &usage);

        ui.separator();
        ui.horizontal(|ui| {
            ui.strong("Converted histogram");
            let label = if self.settings.show_all_histograms {
                "All inks"
            } else {
                "Selected ink"
            };
            if ui.small_button(label).clicked() {
                self.settings.show_all_histograms = !self.settings.show_all_histograms;
                self.save_settings_quietly();
            }
        });
        if self.settings.show_all_histograms {
            for (index, channel) in channels.iter().enumerate() {
                let rgb = channel
                    .display_rgb
                    .unwrap_or_else(|| target_channel_rgb(&channel.name, index));
                ui.colored_label(
                    egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
                    &channel.name,
                );
                if let Some(histogram) = histograms.get(index) {
                    draw_candidate_histogram(
                        ui,
                        histogram,
                        egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
                    );
                }
            }
        } else if !channels.is_empty() {
            let index = solo.unwrap_or(0).min(channels.len() - 1);
            let channel = &channels[index];
            let rgb = channel
                .display_rgb
                .unwrap_or_else(|| target_channel_rgb(&channel.name, index));
            ui.strong(format!("Histogram - {}", channel.name));
            if let Some(histogram) = histograms.get(index) {
                draw_candidate_histogram(
                    ui,
                    histogram,
                    egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
                );
            }
        }

        if clear_pin {
            self.conversion_candidate.pinned = None;
            self.report_info("Cleared Candidate A comparison baseline");
        }
        if pin_current {
            match self.pin_current_conversion_candidate() {
                Ok(hash) => self.report_info(format!(
                    "Pinned Candidate A from recipe {}",
                    short_hash(&hash)
                )),
                Err(error) => self.report_error(error),
            }
        }
        if let Some(solo) = requested_solo {
            self.set_candidate_solo(solo, ui.ctx());
        }
        true
    }

    fn pin_current_conversion_candidate(&mut self) -> Result<String, String> {
        let active = self
            .conversion_candidate
            .active
            .as_ref()
            .filter(|active| {
                active.face_index == self.current_face
                    && active.project_revision == self.project_revision
            })
            .ok_or_else(|| "No current Production Candidate is ready to pin as A.".to_owned())?;
        let snapshot = CandidatePromotionSnapshot::from_preview(
            active.source_state_id.clone(),
            &active.recipe,
            &active.result,
        )?;
        let recipe_sha256 = snapshot.recipe_sha256().to_owned();
        self.conversion_candidate.pinned = Some(snapshot);
        Ok(recipe_sha256)
    }

    fn current_candidate_comparison(&self) -> Result<Option<CandidateComparison>, String> {
        let Some(baseline) = self.conversion_candidate.pinned.as_ref() else {
            return Ok(None);
        };
        let active = self
            .conversion_candidate
            .active
            .as_ref()
            .filter(|active| {
                active.face_index == self.current_face
                    && active.project_revision == self.project_revision
            })
            .ok_or_else(|| "Candidate B is not ready for comparison.".to_owned())?;
        let candidate = CandidateComparisonSnapshot::from_preview(
            active.source_state_id.clone(),
            &active.result,
        )?;
        if baseline.recipe_sha256() == candidate.recipe_sha256 {
            return Ok(None);
        }
        compare_candidate_snapshots(baseline.comparison(), &candidate).map(Some)
    }

    fn observe_candidate(
        &mut self,
        key: String,
        recipe: ConversionRecipe,
        force: bool,
        ctx: &egui::Context,
    ) {
        let changed = self.conversion_candidate.desired_key.as_deref() != Some(key.as_str());
        if changed || force {
            if let Some(pending) = self.conversion_candidate.pending.take() {
                pending.cancellation.request();
            }
            let removed = self.conversion_candidate.active.take().is_some();
            self.conversion_candidate.desired_key = Some(key.clone());
            self.conversion_candidate.desired_recipe = Some(recipe.clone());
            self.conversion_candidate.debounce_started = Some(if force {
                Instant::now() - PREVIEW_DEBOUNCE
            } else {
                Instant::now()
            });
            self.conversion_candidate.error = None;
            self.conversion_candidate.show_converted = true;
            if removed {
                self.force_source_preview_refresh();
            }
        }

        if self
            .conversion_candidate
            .active
            .as_ref()
            .is_some_and(|active| active.key == key)
        {
            self.conversion_candidate.debounce_started = None;
            self.apply_candidate_texture();
            return;
        }

        let start = self.conversion_candidate.pending.is_none()
            && self
                .conversion_candidate
                .debounce_started
                .is_some_and(|started| started.elapsed() >= PREVIEW_DEBOUNCE);
        if start {
            self.conversion_candidate.debounce_started = None;
            self.start_candidate_preview(key, recipe, ctx);
        } else if self.conversion_candidate.pending.is_some()
            || self.conversion_candidate.debounce_started.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }

    fn start_candidate_preview(
        &mut self,
        key: String,
        recipe: ConversionRecipe,
        ctx: &egui::Context,
    ) {
        let inspection = super::conversion_plan::inspect_conversion_face(
            self,
            self.current_face,
            recipe.source_transparency_policy.as_ref(),
        );
        let source = match self.candidate_runtime_source(&inspection) {
            Ok(source) => source,
            Err(error) => {
                self.conversion_candidate.error = Some(error);
                return;
            }
        };
        let source_state_id = candidate_source_state_id(
            source.face_index,
            self.project_revision,
            &source.source_path,
        );
        let source_path = source.source_path.clone();
        let rendered_recipe = recipe.clone();
        let adjusted_planes = match self.faces.get(source.face_index) {
            Some(face) => render::adjusted_planes(face.preview.as_ref(), &self.project),
            None => {
                self.conversion_candidate.error =
                    Some("Candidate Source Face disappeared before rendering.".to_owned());
                return;
            }
        };
        let cancellation = ConversionCancellation::default();
        let worker_cancel = cancellation.clone();
        let input = CandidatePreviewInput {
            width: source.width,
            height: source.height,
            source_model: source.source_model,
            source_planes: adjusted_planes,
            source_profile: source.captured_profile,
            embedded_source_icc: source.embedded_source_icc,
            recipe,
        };
        let (tx, rx) = mpsc::channel();
        self.conversion_candidate.generation =
            self.conversion_candidate.generation.wrapping_add(1).max(1);
        let generation = self.conversion_candidate.generation;
        thread::spawn(move || {
            let _ = tx.send(render_candidate_preview(input, &worker_cancel));
        });
        self.conversion_candidate.pending = Some(PendingCandidate {
            key,
            source_state_id,
            source_path,
            recipe: rendered_recipe,
            generation,
            cancellation,
            rx,
        });
        ctx.request_repaint_after(Duration::from_millis(30));
    }

    fn candidate_runtime_source(
        &self,
        inspection: &ConversionFaceInspection,
    ) -> Result<CandidateRuntimeSource, String> {
        let face = self
            .faces
            .get(inspection.index)
            .ok_or_else(|| "No active Source Face.".to_owned())?;
        if !face.available {
            return Err("Active Source Face is missing. Relink it first.".to_owned());
        }
        let source_model = match inspection.source_model {
            RuntimeColorModel::Rgb => IccSourceModel::Rgb,
            RuntimeColorModel::Cmyk => IccSourceModel::Cmyk,
            model => {
                return Err(format!(
                    "Candidate conversion requires RGB or CMYK Source data; found {}.",
                    model.title()
                ));
            }
        };
        Ok(CandidateRuntimeSource {
            face_index: inspection.index,
            source_path: inspection.source_path.clone(),
            source_model,
            captured_profile: inspection.captured_profile.clone(),
            embedded_source_icc: face.preview.embedded_icc().map(ToOwned::to_owned),
            width: face.preview.width(),
            height: face.preview.height(),
        })
    }

    fn poll_candidate_result(&mut self, ctx: &egui::Context) {
        enum PollResult {
            Empty,
            Disconnected,
            Ready(
                String,
                String,
                PathBuf,
                ConversionRecipe,
                u64,
                Result<CandidatePreviewResult, String>,
            ),
        }
        let poll = match self.conversion_candidate.pending.as_ref() {
            None => PollResult::Empty,
            Some(pending) => match pending.rx.try_recv() {
                Ok(result) => PollResult::Ready(
                    pending.key.clone(),
                    pending.source_state_id.clone(),
                    pending.source_path.clone(),
                    pending.recipe.clone(),
                    pending.generation,
                    result,
                ),
                Err(mpsc::TryRecvError::Empty) => PollResult::Empty,
                Err(mpsc::TryRecvError::Disconnected) => PollResult::Disconnected,
            },
        };
        match poll {
            PollResult::Empty => {
                if self.conversion_candidate.pending.is_some() {
                    ctx.request_repaint_after(Duration::from_millis(30));
                }
            }
            PollResult::Disconnected => {
                self.conversion_candidate.pending = None;
                self.conversion_candidate.error =
                    Some("Candidate preview worker disconnected.".to_owned());
            }
            PollResult::Ready(key, source_state_id, source_path, recipe, generation, result) => {
                self.conversion_candidate.pending = None;
                if self.conversion_candidate.desired_key.as_deref() != Some(key.as_str()) {
                    return;
                }
                match result {
                    Ok(result) => {
                        let rgba = candidate_rgba(&result, None);
                        let texture = load_candidate_texture(ctx, generation, None, &result, &rgba);
                        self.conversion_candidate.error = None;
                        self.conversion_candidate.active = Some(ActiveCandidate {
                            key,
                            source_state_id,
                            source_path,
                            recipe,
                            face_index: self.current_face,
                            project_revision: self.project_revision,
                            result,
                            solo_channel: None,
                            texture,
                        });
                        self.apply_candidate_texture();
                        self.report_info("Production candidate preview ready");
                    }
                    Err(error) => self.conversion_candidate.error = Some(error),
                }
            }
        }
    }

    fn apply_candidate_texture(&mut self) {
        if !self.conversion_candidate.show_converted {
            return;
        }
        let active = self.conversion_candidate.active.as_ref().map(|active| {
            (
                active.face_index,
                active.project_revision,
                active.texture.clone(),
            )
        });
        let Some((face_index, revision, texture)) = active else {
            return;
        };
        if face_index != self.current_face || revision != self.project_revision {
            self.clear_conversion_candidate();
            return;
        }
        if let Some(face) = self.faces.get_mut(face_index) {
            face.texture = Some(texture);
        }
    }

    fn set_candidate_solo(&mut self, solo: Option<usize>, ctx: &egui::Context) {
        let generation = self.conversion_candidate.generation;
        if let Some(active) = self.conversion_candidate.active.as_mut() {
            let solo = solo.filter(|index| *index < active.result.channel_count());
            let rgba = candidate_rgba(&active.result, solo);
            active.texture = load_candidate_texture(ctx, generation, solo, &active.result, &rgba);
            active.solo_channel = solo;
        }
        self.conversion_candidate.show_converted = true;
        self.apply_candidate_texture();
    }

    fn invalidate_conversion_candidate(&mut self) {
        if let Some(pending) = self.conversion_candidate.pending.take() {
            pending.cancellation.request();
        }
        let removed = self.conversion_candidate.active.take().is_some();
        self.conversion_candidate.desired_key = None;
        self.conversion_candidate.desired_recipe = None;
        self.conversion_candidate.debounce_started = None;
        if removed {
            self.force_source_preview_refresh();
        }
    }

    fn invalidate_conversion_candidate_render_only(&mut self) {
        if let Some(pending) = self.conversion_candidate.pending.take() {
            pending.cancellation.request();
        }
        let removed = self.conversion_candidate.active.take().is_some();
        self.conversion_candidate.desired_key = None;
        self.conversion_candidate.desired_recipe = None;
        self.conversion_candidate.debounce_started = None;
        if removed {
            self.force_source_preview_refresh();
        }
    }

    fn force_source_preview_refresh(&mut self) {
        if let Some(face) = self.faces.get_mut(self.current_face) {
            face.texture = None;
            face.generation = face.generation.wrapping_add(1).max(1);
        }
    }
}

fn candidate_source_state_id(
    face_index: usize,
    project_revision: u64,
    source_path: &std::path::Path,
) -> String {
    format!(
        "{face_index}|{project_revision}|{}",
        source_path.to_string_lossy().to_ascii_lowercase()
    )
}

fn candidate_key(
    face_index: usize,
    project_revision: u64,
    source_path: &std::path::Path,
    recipe: &ConversionRecipe,
) -> String {
    format!(
        "{}|{}",
        candidate_source_state_id(face_index, project_revision, source_path),
        recipe_sha256(recipe).unwrap_or_default()
    )
}

fn candidate_rgba(result: &CandidatePreviewResult, solo: Option<usize>) -> Vec<u8> {
    let pixels = result.width.saturating_mul(result.height);
    let mut rgba = Vec::with_capacity(pixels.saturating_mul(4));
    if let Some(channel) = solo.filter(|index| *index < result.planes.len()) {
        for sample in &result.planes[channel] {
            let gray = 255u8.saturating_sub((*sample >> 8) as u8);
            rgba.extend_from_slice(&[gray, gray, gray, 255]);
        }
        return rgba;
    }
    for pixel in 0..pixels {
        let mut rgb = [1.0f32; 3];
        for (index, channel) in result.channels.iter().enumerate() {
            let coverage = result.planes[index][pixel] as f32 / u16::MAX as f32;
            let tint = channel
                .display_rgb
                .unwrap_or_else(|| target_channel_rgb(&channel.name, index));
            let tint = [
                tint[0] as f32 / 255.0,
                tint[1] as f32 / 255.0,
                tint[2] as f32 / 255.0,
            ];
            let strength = (coverage * channel.solidity).clamp(0.0, 1.0);
            for component in 0..3 {
                rgb[component] =
                    rgb[component] * (1.0 - strength) + tint[component] * strength;
            }
        }
        rgba.extend_from_slice(&[
            (rgb[0].clamp(0.0, 1.0) * 255.0).round() as u8,
            (rgb[1].clamp(0.0, 1.0) * 255.0).round() as u8,
            (rgb[2].clamp(0.0, 1.0) * 255.0).round() as u8,
            255,
        ]);
    }
    rgba
}

fn load_candidate_texture(
    ctx: &egui::Context,
    generation: u64,
    solo: Option<usize>,
    result: &CandidatePreviewResult,
    rgba: &[u8],
) -> egui::TextureHandle {
    let image = egui::ColorImage::from_rgba_unmultiplied([result.width, result.height], rgba);
    ctx.load_texture(
        format!("production-candidate-{generation}-{solo:?}"),
        image,
        egui::TextureOptions::LINEAR,
    )
}

fn draw_candidate_comparison(ui: &mut egui::Ui, comparison: &CandidateComparison) {
    ui.label(
        egui::RichText::new(format!(
            "B − A · recipe {} → {}",
            short_hash(&comparison.baseline_recipe_sha256),
            short_hash(&comparison.candidate_recipe_sha256)
        ))
        .strong(),
    );
    ui.small(format!(
        "Total ink Δ · mean {:+.1} pp · p95 {:+.1} pp · p99 {:+.1} pp · peak {:+.1} pp",
        comparison.mean_total_ink * 100.0,
        comparison.p95_total_ink * 100.0,
        comparison.p99_total_ink * 100.0,
        comparison.peak_total_ink * 100.0,
    ));
    if let Some(delta) = comparison.total_ink_limit_hit_percent {
        ui.small(format!("Total-ink limit-hit Δ: {delta:+.2} percentage points"));
    }
    for channel in &comparison.channels {
        ui.small(format!(
            "{} · mean {:+.1} pp · p95 {:+.1} pp · peak {:+.1} pp · integrated {:+.3} relative ink units",
            channel.name,
            channel.mean_coverage * 100.0,
            channel.p95_coverage * 100.0,
            channel.peak_coverage * 100.0,
            channel.integrated_coverage,
        ));
    }
    if let Some(delta) = comparison.neutral_black_share {
        ui.small(format!("Measured Neutral Black-share Δ: {:+.1} pp", delta * 100.0));
    } else {
        ui.small(
            "Measured Neutral Black-share Δ: unavailable until both A and B carry valid measured neutral classification.",
        );
    }
    ui.small("ΔE00 A/B remains unavailable until approved measured PCS/characterization evidence exists.");
}

fn draw_candidate_usage(ui: &mut egui::Ui, usage: &ConversionUsageReport) {
    ui.strong("Ink usage / limits");
    ui.small(format!(
        "Total ink · mean {:.1}% · p50 {:.1}% · p95 {:.1}% · p99 {:.1}% · peak {:.1}%",
        usage.mean_total_ink * 100.0,
        usage.total_ink_percentiles.p50 * 100.0,
        usage.total_ink_percentiles.p95 * 100.0,
        usage.total_ink_percentiles.p99 * 100.0,
        usage.peak_total_ink * 100.0,
    ));
    if let Some(hit_percent) = usage.total_ink_limit_hit_percent {
        ui.small(format!("Total-ink limit hits: {hit_percent:.2}% of candidate pixels"));
    } else {
        ui.small("Total-ink limit hits: no total-ink limit configured");
    }

    for channel in &usage.channels {
        ui.group(|ui| {
            ui.strong(&channel.name);
            ui.small(format!(
                "Mean {:.1}% · p50 {:.1}% · p95 {:.1}% · p99 {:.1}% · peak {:.1}%",
                channel.mean_coverage * 100.0,
                channel.percentiles.p50 * 100.0,
                channel.percentiles.p95 * 100.0,
                channel.percentiles.p99 * 100.0,
                channel.peak_coverage * 100.0,
            ));
            ui.small(format!("Non-zero coverage: {:.2}%", channel.nonzero_percent));
            if let Some(hit_percent) = channel.limit_hit_percent {
                ui.small(format!("Channel-limit hits: {hit_percent:.2}%"));
            } else {
                ui.small("Channel-limit hits: no channel limit configured");
            }
        });
    }

    ui.small(
        "Neutral Black share: unavailable until PCS/characterization-based neutral classification exists.",
    );
    ui.small("ΔE00: unavailable until approved measured PCS/characterization evidence exists.");
}

fn draw_candidate_histogram(ui: &mut egui::Ui, histogram: &[u32; 256], color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(120.0), 68.0),
        egui::Sense::hover(),
    );
    let max = histogram.iter().copied().max().unwrap_or(1).max(1) as f32;
    let points = histogram
        .iter()
        .enumerate()
        .map(|(index, count)| {
            egui::pos2(
                rect.left() + index as f32 / 255.0 * rect.width(),
                rect.bottom() - (*count as f32 / max) * rect.height(),
            )
        })
        .collect::<Vec<_>>();
    ui.painter_at(rect)
        .add(egui::Shape::line(points, egui::Stroke::new(1.35, color)));
}

fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_shade_editor::color_conversion::TargetChannelDefinition;
    use windows_shade_editor::conversion_analytics::{
        ChannelUsageStats, ConversionUsageReport, CoveragePercentiles,
    };

    fn test_usage() -> ConversionUsageReport {
        ConversionUsageReport {
            pixel_count: 2,
            channels: vec![ChannelUsageStats {
                name: "Black".to_owned(),
                mean_coverage: 0.5,
                peak_coverage: 1.0,
                percentiles: CoveragePercentiles {
                    p50: 0.0,
                    p95: 1.0,
                    p99: 1.0,
                },
                nonzero_percent: 50.0,
                limit_hit_percent: None,
                integrated_coverage: 1.0,
            }],
            mean_total_ink: 0.5,
            peak_total_ink: 1.0,
            total_ink_percentiles: CoveragePercentiles {
                p50: 0.0,
                p95: 1.0,
                p99: 1.0,
            },
            total_ink_limit_hit_percent: None,
            neutral_black_share: None,
        }
    }

    #[test]
    fn solo_candidate_uses_direct_ink_coverage_polarity() {
        let result = CandidatePreviewResult {
            width: 2,
            height: 1,
            recipe_sha256: "a".repeat(64),
            channels: vec![TargetChannelDefinition {
                name: "Black".to_owned(),
                display_rgb: Some([0, 0, 0]),
                solidity: 1.0,
                max_coverage: None,
            }],
            planes: vec![vec![0, u16::MAX]],
            histograms: vec![[0; 256]],
            usage: test_usage(),
        };
        let rgba = candidate_rgba(&result, Some(0));
        assert_eq!(&rgba[0..4], &[255, 255, 255, 255]);
        assert_eq!(&rgba[4..8], &[0, 0, 0, 255]);
    }

    #[test]
    fn source_state_identity_changes_without_recipe_and_candidate_key_adds_recipe_identity() {
        let path = std::path::Path::new("C:/Designs/Face-01.tif");
        let source_a = candidate_source_state_id(0, 7, path);
        let source_b = candidate_source_state_id(0, 8, path);
        assert_ne!(source_a, source_b);
        assert_eq!(source_a, "0|7|c:/designs/face-01.tif");
    }

    #[test]
    fn candidate_controller_contains_no_independent_target_config_or_window() {
        let source = include_str!("conversion_candidate_preview.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(!runtime.contains("struct CandidateConfig"));
        assert!(!runtime.contains("egui::Window::new"));
        assert!(!runtime.contains("Queue this exact conversion"));
        assert!(!runtime.contains("ConversionUsageAccumulator"));
        assert!(!runtime.contains("analyze_conversion_tiff"));
        assert!(runtime.contains("CandidatePromotionSnapshot"));
        assert!(runtime.contains("pinned: Option<CandidatePromotionSnapshot>"));
        assert!(runtime.contains("conversion_candidate_a_promotion_selection"));
        assert!(runtime.contains("conversion_candidate_b_promotion_selection"));
        assert!(runtime.contains("recipe: ConversionRecipe"));
        assert!(runtime.contains("compare_candidate_snapshots"));
        assert!(runtime.contains("Pin current as A"));
        assert!(runtime.contains("active.result.usage"));
        assert!(runtime.contains("draw_candidate_usage"));
        assert!(runtime.contains("sync_conversion_candidate"));
        assert!(runtime.contains("render_candidate_preview"));
    }
}
