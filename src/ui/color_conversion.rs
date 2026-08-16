use super::actions::NavigationUiAction;
use crate::*;
use eframe::egui;
use sha2::{Digest, Sha256};
use windows_shade_editor::conversion_preflight::{
    ConversionPreflightInput, ConversionPreflightReport, PreflightCode, PreflightSeverity,
    SourceImageFormat, SourceProfileState, TransparencyState, build_conversion_preflight,
};
use windows_shade_editor::conversion_workflow::{
    ConversionSaveGate, ConversionSourceState, conversion_save_gate,
};
use windows_shade_editor::model::IccProfileIdentity as ConversionIccProfileIdentity;
use windows_shade_editor::tiff_io::ColorModel as ConversionColorModel;

const CONVERSION_WINDOW_ID: &str = "shade-editor-color-conversion-preflight-open";

#[derive(Clone, Debug)]
struct CurrentConversionSource {
    face_label: String,
    source_path: PathBuf,
    color_model_label: &'static str,
    bit_depth: u8,
    channel_count: usize,
    embedded_icc: bool,
    snapshot_id: Option<u64>,
    save_gate: ConversionSaveGate,
    report: ConversionPreflightReport,
}

impl ShadeApp {
    pub(crate) fn ui_color_conversion_status(&mut self, ui: &mut egui::Ui) {
        let Some(source) = self.current_conversion_source() else {
            return;
        };

        let is_rgb = source.report.contains(PreflightCode::RgbNotProductionSeparated);
        let supported_source = matches!(
            self.faces
                .get(self.current_face)
                .map(|face| face.preview.metadata.color_model),
            Some(tiff_io::ColorModel::Rgb | tiff_io::ColorModel::Cmyk)
        );

        if is_rgb {
            ui.separator();
            ui.label(
                egui::RichText::new("RGB source — not production separated")
                    .color(egui::Color32::YELLOW)
                    .small(),
            )
            .on_hover_text(
                "Convert this Source project to the target CMYK/Multichannel printing space before production-separated output.",
            );
        }

        if supported_source {
            if ui
                .small_button(app_features::COLOR_CONVERSION_LABEL)
                .on_hover_text("Inspect production color-conversion prerequisites. Source files remain unchanged.")
                .clicked()
            {
                set_conversion_window_open(ui.ctx(), true);
            }
        }
    }

    pub(crate) fn ui_color_conversion_window(&mut self, ctx: &egui::Context) {
        if !conversion_window_open(ctx) {
            return;
        }

        let Some(source) = self.current_conversion_source() else {
            set_conversion_window_open(ctx, false);
            return;
        };

        let mut open = true;
        let mut navigation_action = None;
        let mut open_preview_color_management = false;

        egui::Window::new("Production Color Conversion")
            .id(egui::Id::new("production-color-conversion-window"))
            .open(&mut open)
            .resizable(true)
            .default_size([760.0, 650.0])
            .min_width(600.0)
            .show(ctx, |ui| {
                ui.heading("Production Color Conversion");
                ui.label(
                    "Preflight verifies the saved Source state before RGB/CMYK → CMYK/Multichannel conversion.",
                );
                ui.small(
                    "This workflow is separate from ICC Preview. Preview Color Management never changes production samples or satisfies a missing production Source ICC by itself.",
                );
                ui.add_space(8.0);

                egui::Grid::new("conversion-source-summary")
                    .num_columns(2)
                    .striped(true)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        ui.strong("Face");
                        ui.label(&source.face_label);
                        ui.end_row();

                        ui.strong("Source");
                        ui.label(source.source_path.display().to_string());
                        ui.end_row();

                        ui.strong("Color model");
                        ui.label(source.color_model_label);
                        ui.end_row();

                        ui.strong("Bit depth");
                        ui.label(format!("{}-bit", source.bit_depth));
                        ui.end_row();

                        ui.strong("Channels");
                        ui.label(source.channel_count.to_string());
                        ui.end_row();

                        ui.strong("Source ICC");
                        ui.label(if source.embedded_icc {
                            "Embedded ICC"
                        } else {
                            "Missing production Source ICC"
                        });
                        ui.end_row();

                        ui.strong("Saved state");
                        ui.label(save_gate_label(source.save_gate));
                        ui.end_row();

                        ui.strong("Snapshot");
                        ui.label(
                            source
                                .snapshot_id
                                .map(|id| format!("#{id}"))
                                .unwrap_or_else(|| "Current saved project state".to_owned()),
                        );
                        ui.end_row();
                    });

                ui.add_space(10.0);
                ui.separator();
                ui.strong("Preflight");
                ui.add_space(4.0);

                if source.report.findings.is_empty() {
                    ui.label(egui::RichText::new("Ready").color(egui::Color32::LIGHT_GREEN));
                } else {
                    for finding in &source.report.findings {
                        ui.group(|ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    egui::RichText::new(severity_label(finding.severity))
                                        .color(severity_color(finding.severity))
                                        .strong(),
                                );
                                ui.strong(finding.title);
                            });
                            ui.label(&finding.detail);

                            match finding.code {
                                PreflightCode::UnsavedSourceProject => match source.save_gate {
                                    ConversionSaveGate::SaveAsRequired => {
                                        if ui.button("Save Source Project As...").clicked() {
                                            navigation_action = Some(NavigationUiAction::SaveAs);
                                        }
                                    }
                                    ConversionSaveGate::SaveRequired => {
                                        if ui.button("Save & Continue").clicked() {
                                            navigation_action = Some(NavigationUiAction::Save);
                                        }
                                    }
                                    ConversionSaveGate::Ready | ConversionSaveGate::NoSourceFaces => {}
                                },
                                PreflightCode::MissingSourceProfile
                                | PreflightCode::InvalidSourceProfile => {
                                    ui.horizontal_wrapped(|ui| {
                                        if ui.button("Open Color Management / ICC Preview").clicked() {
                                            open_preview_color_management = true;
                                        }
                                        ui.small(
                                            "Preview assignment is display-only. Dedicated production Source ICC assignment is the next conversion slice.",
                                        );
                                    });
                                }
                                _ => {}
                            }
                        });
                        ui.add_space(4.0);
                    }
                }

                ui.add_space(8.0);
                ui.separator();

                let ready = source.report.can_convert();
                ui.horizontal_wrapped(|ui| {
                    let continue_button = ui.add_enabled(
                        false,
                        egui::Button::new("Continue to Target Setup"),
                    );
                    continue_button.on_hover_text(if ready {
                        "Preflight is ready. Target profile/DeviceLink and conversion destination UI will be enabled in the next implementation slice."
                    } else {
                        "Resolve all blocking preflight findings first."
                    });

                    if ready {
                        ui.label(
                            egui::RichText::new("Preflight ready")
                                .color(egui::Color32::LIGHT_GREEN),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new("Conversion blocked")
                                .color(egui::Color32::LIGHT_RED),
                        );
                    }
                });

                ui.small(
                    "No raster conversion is executed by this UI slice. The original Source TIFF remains byte-identical.",
                );
            });

        if !open {
            set_conversion_window_open(ctx, false);
        }
        if let Some(action) = navigation_action {
            self.dispatch_navigation_ui_action(action, ctx);
        }
        if open_preview_color_management {
            self.color.show = true;
        }
    }

    fn current_conversion_source(&self) -> Option<CurrentConversionSource> {
        let face = self.faces.get(self.current_face)?;
        let metadata = &face.preview.metadata;
        let source_model = conversion_color_model(metadata.color_model);
        let save_gate = conversion_save_gate(ConversionSourceState {
            has_faces: !self.faces.is_empty(),
            has_saved_project_path: self.project_path.is_some(),
            has_unsaved_changes: self.project_dirty,
        });
        let profile = embedded_profile_state(metadata);
        let embedded_icc = metadata.icc_profile.is_some();
        let report = build_conversion_preflight(&ConversionPreflightInput {
            format: SourceImageFormat::Tiff,
            color_model: source_model,
            bit_depth: metadata.bit_depth,
            profile,
            save_gate,
            transparency: TransparencyState::None,
        });

        let face_label = self
            .project
            .faces
            .get(self.current_face)
            .map(|item| item.label.clone())
            .filter(|label| !label.trim().is_empty())
            .or_else(|| {
                face.path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| format!("Face {}", self.current_face + 1));

        Some(CurrentConversionSource {
            face_label,
            source_path: face.path.clone(),
            color_model_label: metadata.color_model.title(),
            bit_depth: metadata.bit_depth,
            channel_count: metadata.samples_per_pixel,
            embedded_icc,
            snapshot_id: self.project.active_snapshot_id,
            save_gate,
            report,
        })
    }
}

fn conversion_color_model(model: tiff_io::ColorModel) -> ConversionColorModel {
    match model {
        tiff_io::ColorModel::Gray => ConversionColorModel::Gray,
        tiff_io::ColorModel::Rgb => ConversionColorModel::Rgb,
        tiff_io::ColorModel::Cmyk => ConversionColorModel::Cmyk,
        tiff_io::ColorModel::Other => ConversionColorModel::Other,
    }
}

fn embedded_profile_state(metadata: &tiff_io::TiffMetadata) -> SourceProfileState {
    let Some(bytes) = metadata.icc_profile.as_ref() else {
        return SourceProfileState::Missing;
    };
    if bytes.is_empty() {
        return SourceProfileState::Invalid("Embedded ICC payload is empty.".to_owned());
    }

    let digest = Sha256::digest(bytes);
    SourceProfileState::Embedded(ConversionIccProfileIdentity {
        description: "Embedded ICC profile".to_owned(),
        sha256: format!("{digest:x}"),
    })
}

fn severity_label(severity: PreflightSeverity) -> &'static str {
    match severity {
        PreflightSeverity::Info => "INFO",
        PreflightSeverity::Warning => "WARNING",
        PreflightSeverity::Blocking => "BLOCKING",
    }
}

fn severity_color(severity: PreflightSeverity) -> egui::Color32 {
    match severity {
        PreflightSeverity::Info => egui::Color32::LIGHT_BLUE,
        PreflightSeverity::Warning => egui::Color32::YELLOW,
        PreflightSeverity::Blocking => egui::Color32::LIGHT_RED,
    }
}

fn save_gate_label(gate: ConversionSaveGate) -> &'static str {
    match gate {
        ConversionSaveGate::Ready => "Saved / reproducible",
        ConversionSaveGate::NoSourceFaces => "No source Face",
        ConversionSaveGate::SaveAsRequired => "Save As required",
        ConversionSaveGate::SaveRequired => "Save required",
    }
}

fn conversion_window_open(ctx: &egui::Context) -> bool {
    ctx.data(|data| {
        data.get_temp::<bool>(egui::Id::new(CONVERSION_WINDOW_ID))
            .unwrap_or(false)
    })
}

fn set_conversion_window_open(ctx: &egui::Context, open: bool) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(CONVERSION_WINDOW_ID), open));
}
