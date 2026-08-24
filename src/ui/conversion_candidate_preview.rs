use crate::*;
use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use windows_shade_editor::color_conversion::ConversionRecipe;
use windows_shade_editor::conversion_candidate_preview::{
    CandidatePreviewInput, CandidatePreviewResult, render_candidate_preview,
};
use windows_shade_editor::conversion_recipe::recipe_sha256;
use windows_shade_editor::conversion_transaction::{
    CapturedSourceProfile, ConversionCancellation,
};
use windows_shade_editor::icc_conversion::IccSourceModel;

use super::conversion_plan::{ConversionFaceInspection, target_channel_rgb};

const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(220);

struct PendingCandidate {
    key: String,
    generation: u64,
    cancellation: ConversionCancellation,
    rx: mpsc::Receiver<Result<CandidatePreviewResult, String>>,
}

struct ActiveCandidate {
    key: String,
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
    pub(crate) channel_count: usize,
    pub(crate) error: Option<String>,
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
        let solo = active.solo_channel;
        let recipe_sha = active.result.recipe_sha256.clone();
        let mut requested_solo: Option<Option<usize>> = None;

        ui.horizontal(|ui| {
            ui.heading("Production Candidate");
            ui.label(
                egui::RichText::new(format!("{} inks", channels.len()))
                    .color(egui::Color32::LIGHT_GREEN),
            );
        });
        ui.small(format!("Converted target samples · recipe {}", short_hash(&recipe_sha)));
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

        if let Some(solo) = requested_solo {
            self.set_candidate_solo(solo, ui.ctx());
        }
        true
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
            Ready(String, u64, Result<CandidatePreviewResult, String>),
        }
        let poll = match self.conversion_candidate.pending.as_ref() {
            None => PollResult::Empty,
            Some(pending) => match pending.rx.try_recv() {
                Ok(result) => PollResult::Ready(pending.key.clone(), pending.generation, result),
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
            PollResult::Ready(key, generation, result) => {
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

fn candidate_key(
    face_index: usize,
    project_revision: u64,
    source_path: &std::path::Path,
    recipe: &ConversionRecipe,
) -> String {
    format!(
        "{face_index}|{project_revision}|{}|{}",
        source_path.to_string_lossy().to_ascii_lowercase(),
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
        };
        let rgba = candidate_rgba(&result, Some(0));
        assert_eq!(&rgba[0..4], &[255, 255, 255, 255]);
        assert_eq!(&rgba[4..8], &[0, 0, 0, 255]);
    }

    #[test]
    fn candidate_controller_contains_no_independent_target_config_or_window() {
        let source = include_str!("conversion_candidate_preview.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(!runtime.contains("struct CandidateConfig"));
        assert!(!runtime.contains("egui::Window::new"));
        assert!(!runtime.contains("Queue this exact conversion"));
        assert!(runtime.contains("sync_conversion_candidate"));
        assert!(runtime.contains("render_candidate_preview"));
    }
}
