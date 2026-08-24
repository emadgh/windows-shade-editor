from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected exactly one match, found {count}\n--- OLD ---\n{old}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")
    print(f"patched {path}")


# One canonical deterministic filename core. The unified route intentionally does not use
# `_vN` allocation because scope must never change the Face destination.
replace_once(
    "src/conversion_output.rs",
    '''/// Return the first free `_vN` path when `preferred` already exists. This never
/// deletes or replaces anything and is therefore safe as the default reconversion policy.
pub fn next_versioned_output_path(preferred: &Path) -> Result<PathBuf, OutputPathError> {''',
    '''/// Build the canonical Production TIFF filename for one Source Face.
///
/// The name deliberately excludes target/profile labels so Current / Selected / All scopes map
/// the same Source Face to the same path. `face_disambiguator` is supplied only when the Source
/// project contains duplicate file stems, and must therefore come from stable Source Face identity.
pub fn deterministic_converted_filename(
    source: &Path,
    face_disambiguator: Option<usize>,
) -> Result<PathBuf, OutputPathError> {
    let stem = source
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or(OutputPathError::MissingFileName)?;
    let name = match face_disambiguator {
        Some(index) => format!("{stem}_F{index:02}.tif"),
        None => format!("{stem}.tif"),
    };
    Ok(PathBuf::from(name))
}

/// Return the first free `_vN` path when `preferred` already exists. This remains available for
/// legacy workflows. Unified Production Color Conversion intentionally does not call it because
/// versioned names break deterministic Source↔converted Face mapping.
pub fn next_versioned_output_path(preferred: &Path) -> Result<PathBuf, OutputPathError> {''',
)
replace_once(
    "src/conversion_output.rs",
    '''    #[test]
    fn default_filename_uses_sanitized_target_suffix() {''',
    '''    #[test]
    fn deterministic_name_is_scope_and_target_independent() {
        assert_eq!(
            deterministic_converted_filename(Path::new(r"C:\\Design\\Face01.png"), None).unwrap(),
            PathBuf::from("Face01.tif")
        );
        assert_eq!(
            deterministic_converted_filename(Path::new(r"C:\\A\\Face01.tif"), Some(3)).unwrap(),
            PathBuf::from("Face01_F03.tif")
        );
    }

    #[test]
    fn default_filename_uses_sanitized_target_suffix() {''',
)

# Converted candidate owns the ordinary Channels/Histogram surface only while Converted view is
# active. Returning to Source falls through to the existing source analysis unchanged.
replace_once(
    "src/ui/adjustments.rs",
    '''    pub(crate) fn ui_channels_histogram(&mut self, ui: &mut egui::Ui) {
        let Some(face) = self.faces.get(self.current_face) else {''',
    '''    pub(crate) fn ui_channels_histogram(&mut self, ui: &mut egui::Ui) {
        if self.ui_conversion_candidate_channels_histogram(ui) {
            return;
        }
        let Some(face) = self.faces.get(self.current_face) else {''',
)

# ShadeApp owns one operator state plus two narrow runtime controllers. The old single queue is
# retained only for jobs persisted by builds before #372.
replace_once(
    "src/main.rs",
    '''    color: ColorManagementController,
    color_conversion: ui::color_conversion::ColorConversionUiState,
    conversion_queue: windows_shade_editor::conversion_queue::ConversionQueue,''',
    '''    color: ColorManagementController,
    color_conversion: ui::color_conversion::ColorConversionUiState,
    conversion_candidate: ui::conversion_candidate_preview::CandidatePreviewController,
    conversion_batch: ui::conversion_batch::ConversionBatchController,
    conversion_queue: windows_shade_editor::conversion_queue::ConversionQueue,''',
)
replace_once(
    "src/main.rs",
    '''            color: ColorManagementController::default(),
            color_conversion: ui::color_conversion::ColorConversionUiState::default(),
            conversion_queue,''',
    '''            color: ColorManagementController::default(),
            color_conversion: ui::color_conversion::ColorConversionUiState::default(),
            conversion_candidate: ui::conversion_candidate_preview::CandidatePreviewController::default(),
            conversion_batch: ui::conversion_batch::ConversionBatchController::load(),
            conversion_queue,''',
)
replace_once(
    "src/main.rs",
    '''    fn request_project_transition(
        &mut self,
        transition: ProjectTransition,
        ctx: Option<&egui::Context>,
    ) {
        match self.lifecycle.request(
            transition,
            self.job.is_some(),
            self.export.queue.has_pending() || self.conversion_queue.has_pending(),''',
    '''    fn request_project_transition(
        &mut self,
        transition: ProjectTransition,
        ctx: Option<&egui::Context>,
    ) {
        let conversion_work_pending = self.export.queue.has_pending()
            || self.conversion_queue.has_pending()
            || self.conversion_batch_blocks_project_transition();
        match self.lifecycle.request(
            transition,
            self.job.is_some(),
            conversion_work_pending,''',
)
replace_once(
    "src/main.rs",
    '''    fn bump_project_session(&mut self) {
        self.lifecycle.bump_session();
        self.snapshot_preview_cache.clear();
    }''',
    '''    fn bump_project_session(&mut self) {
        self.lifecycle.bump_session();
        self.snapshot_preview_cache.clear();
        self.clear_conversion_candidate();
        self.color_conversion = ui::color_conversion::ColorConversionUiState::default();
    }''',
)
replace_once(
    "src/main.rs",
    '''        if let Some((value, text)) = self.conversion_queue.active_summary() {
            ui.add(
                egui::ProgressBar::new(value)
                    .desired_width(340.0)
                    .text(text),
            );
            return;
        }''',
    '''        if let Some((value, text)) = self.conversion_batch_active_summary() {
            ui.add(
                egui::ProgressBar::new(value)
                    .desired_width(340.0)
                    .text(text),
            );
            return;
        }
        if let Some((value, text)) = self.conversion_queue.active_summary() {
            ui.add(
                egui::ProgressBar::new(value)
                    .desired_width(340.0)
                    .text(text),
            );
            return;
        }''',
)
# The dynamic ICC/profile button is the viewport's Color Management control. Place the sole
# conversion entry immediately beside it in the same top viewport row.
replace_once(
    "src/main.rs",
    '''            if open_color_management {
                self.color.show = true;
                self.color.selected = self.project.preview_color.assigned_profile_path.clone();
            }
        });''',
    '''            if open_color_management {
                self.color.show = true;
                self.color.selected = self.project.preview_color.assigned_profile_path.clone();
            }
            if ui
                .small_button(app_features::COLOR_CONVERSION_LABEL)
                .on_hover_text("Production Color Conversion: exact candidate preview plus Current / Selected / All Face conversion.")
                .clicked()
            {
                self.open_color_conversion(ui.ctx());
            }
        });''',
)

# egui 0.35 replaced SelectableLabel with selectable Button construction.
replace_once(
    "src/ui/color_conversion.rs",
    '''                                                    let response = ui.add_enabled(
                                                        candidate.selectable(),
                                                        egui::SelectableLabel::new(selected, label),
                                                    );''',
    '''                                                    let response = ui.add_enabled(
                                                        candidate.selectable(),
                                                        egui::Button::selectable(selected, label),
                                                    );''',
)
replace_once(
    "src/ui/color_conversion.rs",
    '''                                        let response = ui.add_enabled(
                                            candidate.can_append(),
                                            egui::SelectableLabel::new(
                                                selected,
                                                format!(
                                                    "{} · {}",
                                                    candidate
                                                        .project_name
                                                        .as_deref()
                                                        .unwrap_or("Production"),
                                                    status
                                                ),
                                            ),
                                        );''',
    '''                                        let response = ui.add_enabled(
                                            candidate.can_append(),
                                            egui::Button::selectable(
                                                selected,
                                                format!(
                                                    "{} · {}",
                                                    candidate
                                                        .project_name
                                                        .as_deref()
                                                        .unwrap_or("Production"),
                                                    status
                                                ),
                                            ),
                                        );''',
)

# Keep the candidate runtime dependency surface narrow.
replace_once(
    "src/ui/conversion_candidate_preview.rs",
    '''use windows_shade_editor::color_conversion::{ConversionRecipe, TargetChannelDefinition};''',
    '''use windows_shade_editor::color_conversion::ConversionRecipe;''',
)

print("issue 372 shell wiring patch complete")
