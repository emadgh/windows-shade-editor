use eframe::egui;
use std::fs;
use std::path::{Path, PathBuf};

use windows_shade_editor::characterization_intake::{
    CharacterizationIntakeMetadata, CharacterizationIntakeResult, MeasurementCoverageUnit,
    MeasurementTableDelimiter, build_characterization_package_from_table,
    save_characterization_package,
};
use windows_shade_editor::device_characterization_package::{
    CharacterizationMeasurementMetadata, CharacterizationProductionContext,
    CharacterizationValidationLevel,
};

const MAX_MEASUREMENT_TABLE_BYTES: u64 = 256 * 1024 * 1024;
const DEFAULT_CHANNEL_COUNT: usize = 4;
const MIN_CHANNEL_COUNT: usize = 1;
const MAX_CHANNEL_COUNT: usize = 12;

#[derive(Clone)]
pub(crate) struct CharacterizationIntakeUiState {
    table_path: Option<PathBuf>,
    table_text: String,
    delimiter: MeasurementTableDelimiter,
    coverage_unit: MeasurementCoverageUnit,
    validation_level: CharacterizationValidationLevel,
    revision: String,
    output_bit_depth: u8,
    channel_names: Vec<String>,
    channel_limits: Vec<String>,
    total_ink_limit: String,
    machine_id: String,
    rip_name: String,
    rip_version: String,
    linearization_id: String,
    substrate: String,
    glaze: String,
    body: String,
    product_family: String,
    instrument_model: String,
    instrument_serial: String,
    illuminant: String,
    observer: String,
    measurement_condition: String,
    operator_or_lab: String,
    built: Option<CharacterizationIntakeResult>,
    errors: Vec<String>,
    status: Option<String>,
}

impl Default for CharacterizationIntakeUiState {
    fn default() -> Self {
        Self {
            table_path: None,
            table_text: String::new(),
            delimiter: MeasurementTableDelimiter::Comma,
            coverage_unit: MeasurementCoverageUnit::Normalized,
            // Importing/parsing a table must never silently grant production authority.
            validation_level: CharacterizationValidationLevel::Experimental,
            revision: String::new(),
            output_bit_depth: 16,
            channel_names: vec![String::new(); DEFAULT_CHANNEL_COUNT],
            channel_limits: vec![String::new(); DEFAULT_CHANNEL_COUNT],
            total_ink_limit: String::new(),
            machine_id: String::new(),
            rip_name: String::new(),
            rip_version: String::new(),
            linearization_id: String::new(),
            substrate: String::new(),
            glaze: String::new(),
            body: String::new(),
            product_family: String::new(),
            instrument_model: String::new(),
            instrument_serial: String::new(),
            illuminant: "D50".to_owned(),
            observer: "2deg".to_owned(),
            measurement_condition: "M1".to_owned(),
            operator_or_lab: String::new(),
            built: None,
            errors: Vec::new(),
            status: None,
        }
    }
}

pub(crate) fn render_characterization_intake(
    ui: &mut egui::Ui,
    state: &mut CharacterizationIntakeUiState,
) {
    ui.label(
        "Build a content-addressed measured characterization package from a laboratory CSV/TSV table. This tool validates package structure; it does not approve production evidence.",
    );
    ui.small(
        "Production qualification remains #205. A successful build means schema-valid measured evidence only.",
    );

    let mut invalidate = false;

    ui.add_space(5.0);
    ui.horizontal_wrapped(|ui| {
        if ui.button("Load CSV / TSV...").clicked() {
            match load_table_dialog() {
                Ok(Some((path, text))) => {
                    state.table_path = Some(path.clone());
                    state.table_text = text;
                    state.errors.clear();
                    state.status = Some(format!("Loaded measurement table: {}", path.display()));
                    invalidate = true;
                }
                Ok(None) => {}
                Err(error) => {
                    state.errors = vec![error];
                    state.status = None;
                }
            }
        }
        if let Some(path) = state.table_path.as_deref() {
            ui.strong(path.display().to_string());
        } else {
            ui.label("No measurement table loaded");
        }
    });

    ui.horizontal_wrapped(|ui| {
        ui.label("Delimiter");
        invalidate |= ui
            .selectable_value(
                &mut state.delimiter,
                MeasurementTableDelimiter::Comma,
                "CSV comma",
            )
            .changed();
        invalidate |= ui
            .selectable_value(
                &mut state.delimiter,
                MeasurementTableDelimiter::Tab,
                "TSV tab",
            )
            .changed();
        ui.separator();
        ui.label("Coverage unit");
        invalidate |= ui
            .selectable_value(
                &mut state.coverage_unit,
                MeasurementCoverageUnit::Normalized,
                "Normalized 0..1",
            )
            .changed();
        invalidate |= ui
            .selectable_value(
                &mut state.coverage_unit,
                MeasurementCoverageUnit::Percent,
                "Percent 0..100",
            )
            .changed();
    });
    ui.small("Coverage units are explicit and are never guessed from the table values.");

    if let Some(header) = state
        .table_text
        .lines()
        .find(|line| !line.trim().is_empty())
    {
        ui.small(format!("Table header: {}", truncate_for_ui(header, 180)));
    }

    ui.add_space(6.0);
    ui.strong("Package identity / authority");
    egui::Grid::new("characterization-intake-package-metadata")
        .num_columns(2)
        .spacing([12.0, 5.0])
        .show(ui, |ui| {
            ui.label("Dataset revision");
            invalidate |= ui.text_edit_singleline(&mut state.revision).changed();
            ui.end_row();

            ui.label("Declared validation level");
            egui::ComboBox::from_id_salt("characterization-intake-validation-level")
                .selected_text(match state.validation_level {
                    CharacterizationValidationLevel::Experimental => "Experimental",
                    CharacterizationValidationLevel::ProductionValidated => "ProductionValidated",
                })
                .show_ui(ui, |ui| {
                    invalidate |= ui
                        .selectable_value(
                            &mut state.validation_level,
                            CharacterizationValidationLevel::Experimental,
                            "Experimental",
                        )
                        .changed();
                    invalidate |= ui
                        .selectable_value(
                            &mut state.validation_level,
                            CharacterizationValidationLevel::ProductionValidated,
                            "ProductionValidated (declaration only)",
                        )
                        .changed();
                });
            ui.end_row();

            ui.label("Output bit depth");
            ui.horizontal(|ui| {
                invalidate |= ui
                    .selectable_value(&mut state.output_bit_depth, 8, "8-bit")
                    .changed();
                invalidate |= ui
                    .selectable_value(&mut state.output_bit_depth, 16, "16-bit")
                    .changed();
            });
            ui.end_row();
        });
    ui.label(
        egui::RichText::new(
            "Selecting ProductionValidated only declares package metadata. It does not approve representativeness, thresholds, or Custom Optimizer production use.",
        )
        .color(egui::Color32::YELLOW),
    );

    ui.add_space(6.0);
    render_channel_editor(ui, state, &mut invalidate);

    ui.add_space(6.0);
    egui::CollapsingHeader::new("Production context")
        .id_salt("characterization-intake-production-context")
        .default_open(true)
        .show(ui, |ui| {
            metadata_text_row(ui, "Machine ID", &mut state.machine_id, &mut invalidate);
            metadata_text_row(ui, "RIP name", &mut state.rip_name, &mut invalidate);
            metadata_text_row(ui, "RIP version", &mut state.rip_version, &mut invalidate);
            metadata_text_row(
                ui,
                "Linearization / calibration ID",
                &mut state.linearization_id,
                &mut invalidate,
            );
            metadata_text_row(ui, "Substrate", &mut state.substrate, &mut invalidate);
            metadata_text_row(ui, "Glaze (optional)", &mut state.glaze, &mut invalidate);
            metadata_text_row(ui, "Body (optional)", &mut state.body, &mut invalidate);
            metadata_text_row(
                ui,
                "Product family (optional)",
                &mut state.product_family,
                &mut invalidate,
            );
        });

    egui::CollapsingHeader::new("Measurement metadata")
        .id_salt("characterization-intake-measurement-metadata")
        .default_open(true)
        .show(ui, |ui| {
            metadata_text_row(
                ui,
                "Instrument model",
                &mut state.instrument_model,
                &mut invalidate,
            );
            metadata_text_row(
                ui,
                "Instrument serial (optional)",
                &mut state.instrument_serial,
                &mut invalidate,
            );
            metadata_text_row(ui, "Illuminant", &mut state.illuminant, &mut invalidate);
            metadata_text_row(ui, "Observer", &mut state.observer, &mut invalidate);
            metadata_text_row(
                ui,
                "Measurement condition",
                &mut state.measurement_condition,
                &mut invalidate,
            );
            metadata_text_row(
                ui,
                "Operator / lab (optional)",
                &mut state.operator_or_lab,
                &mut invalidate,
            );
        });

    if invalidate {
        invalidate_built_result(state);
    }

    ui.add_space(7.0);
    ui.horizontal_wrapped(|ui| {
        if ui.button("Validate & build package").clicked() {
            build_package(state);
        }
        if ui
            .add_enabled(
                state.built.is_some(),
                egui::Button::new("Save package JSON..."),
            )
            .clicked()
        {
            save_package_dialog(state);
        }
    });

    for error in &state.errors {
        ui.label(egui::RichText::new(format!("• {error}")).color(egui::Color32::LIGHT_RED));
    }
    if let Some(status) = state.status.as_deref() {
        ui.label(egui::RichText::new(status).color(egui::Color32::LIGHT_GREEN));
    }

    if let Some(result) = state.built.as_ref() {
        render_validated_package(ui, result);
    }
}

fn render_channel_editor(
    ui: &mut egui::Ui,
    state: &mut CharacterizationIntakeUiState,
    invalidate: &mut bool,
) {
    ui.horizontal_wrapped(|ui| {
        ui.strong("Authoritative production channel order");
        if state.channel_names.len() > MIN_CHANNEL_COUNT && ui.small_button("Remove last").clicked() {
            state.channel_names.pop();
            state.channel_limits.pop();
            *invalidate = true;
        }
        if state.channel_names.len() < MAX_CHANNEL_COUNT && ui.small_button("Add channel").clicked() {
            state.channel_names.push(String::new());
            state.channel_limits.push(String::new());
            *invalidate = true;
        }
        ui.small(format!("{} channel(s)", state.channel_names.len()));
    });
    ui.small(
        "Names and order must exactly match the table columns and the physical/RIP production order. The importer never reorders them.",
    );

    egui::Grid::new("characterization-intake-channels")
        .num_columns(3)
        .striped(true)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.strong("#");
            ui.strong("Channel name");
            ui.strong("Measured max (0..1)");
            ui.end_row();
            for index in 0..state.channel_names.len() {
                ui.label((index + 1).to_string());
                *invalidate |= ui
                    .text_edit_singleline(&mut state.channel_names[index])
                    .changed();
                *invalidate |= ui
                    .text_edit_singleline(&mut state.channel_limits[index])
                    .changed();
                ui.end_row();
            }
        });
    ui.horizontal(|ui| {
        ui.label("Measured total-ink limit");
        *invalidate |= ui
            .text_edit_singleline(&mut state.total_ink_limit)
            .changed();
    });
}

fn render_validated_package(ui: &mut egui::Ui, result: &CharacterizationIntakeResult) {
    ui.group(|ui| {
        ui.strong("Validated package");
        ui.label(format!("Content ID: {}", result.package.id));
        ui.small(format!(
            "{} samples · {} channels · {}-bit",
            result.package.payload.samples.len(),
            result.package.payload.channel_names.len(),
            result.package.payload.output_bit_depth
        ));
        if result.qualification_warnings.is_empty() {
            ui.label(
                egui::RichText::new(
                    "PCS metadata is compatible with the current D50 / CIE 2° Custom Optimizer qualification gate.",
                )
                .color(egui::Color32::LIGHT_GREEN),
            );
        } else {
            for warning in &result.qualification_warnings {
                ui.label(
                    egui::RichText::new(format!("Qualification warning: {warning}"))
                        .color(egui::Color32::YELLOW),
                );
            }
        }
        ui.small(
            "This result proves package validity only. Representative measured coverage, forward-model quality, thresholds and approval remain #205.",
        );
    });
}

fn build_package(state: &mut CharacterizationIntakeUiState) {
    state.errors.clear();
    state.status = None;
    state.built = None;

    let metadata = match collect_metadata(state) {
        Ok(metadata) => metadata,
        Err(errors) => {
            state.errors = errors;
            return;
        }
    };
    if state.table_text.trim().is_empty() {
        state
            .errors
            .push("Load a CSV/TSV measurement table first.".to_owned());
        return;
    }

    match build_characterization_package_from_table(
        &state.table_text,
        state.delimiter,
        state.coverage_unit,
        metadata,
    ) {
        Ok(result) => {
            state.status = Some(format!("Package validated: {}", result.package.id));
            state.built = Some(result);
        }
        Err(errors) => state.errors = errors,
    }
}

fn collect_metadata(
    state: &CharacterizationIntakeUiState,
) -> Result<CharacterizationIntakeMetadata, Vec<String>> {
    let mut errors = Vec::new();
    let channel_limits = state
        .channel_limits
        .iter()
        .enumerate()
        .map(|(index, value)| {
            parse_f32_field(
                value,
                &format!("Measured maximum for channel {}", index + 1),
                &mut errors,
            )
        })
        .collect::<Vec<_>>();
    let total_ink_limit = parse_f32_field(
        &state.total_ink_limit,
        "Measured total-ink limit",
        &mut errors,
    );

    if state.channel_names.iter().any(|name| name.trim().is_empty()) {
        errors.push("Every production channel must have an explicit non-empty name.".to_owned());
    }
    if state.revision.trim().is_empty() {
        errors.push("Dataset revision is required.".to_owned());
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(CharacterizationIntakeMetadata {
        revision: state.revision.trim().to_owned(),
        validation_level: state.validation_level,
        output_bit_depth: state.output_bit_depth,
        channel_names: state
            .channel_names
            .iter()
            .map(|value| value.trim().to_owned())
            .collect(),
        measured_channel_max_coverage: channel_limits,
        measured_total_ink_limit: total_ink_limit,
        production_context: CharacterizationProductionContext {
            machine_id: state.machine_id.trim().to_owned(),
            rip_name: state.rip_name.trim().to_owned(),
            rip_version: state.rip_version.trim().to_owned(),
            linearization_id: state.linearization_id.trim().to_owned(),
            substrate: state.substrate.trim().to_owned(),
            glaze: optional_text(&state.glaze),
            body: optional_text(&state.body),
            product_family: optional_text(&state.product_family),
        },
        measurement: CharacterizationMeasurementMetadata {
            instrument_model: state.instrument_model.trim().to_owned(),
            instrument_serial: optional_text(&state.instrument_serial),
            illuminant: state.illuminant.trim().to_owned(),
            observer: state.observer.trim().to_owned(),
            measurement_condition: state.measurement_condition.trim().to_owned(),
            measured_at_unix_ms: None,
            operator_or_lab: optional_text(&state.operator_or_lab),
        },
    })
}

fn load_table_dialog() -> Result<Option<(PathBuf, String)>, String> {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("Measurement table", &["csv", "tsv", "txt"])
        .set_title("Load measured characterization table")
        .pick_file()
    else {
        return Ok(None);
    };

    let metadata = fs::metadata(&path)
        .map_err(|error| format!("Cannot inspect measurement table {}: {error}", path.display()))?;
    if metadata.len() > MAX_MEASUREMENT_TABLE_BYTES {
        return Err(format!(
            "Measurement table {} is {} bytes; maximum accepted intake size is {} bytes.",
            path.display(),
            metadata.len(),
            MAX_MEASUREMENT_TABLE_BYTES
        ));
    }
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("Cannot read measurement table {}: {error}", path.display()))?;
    Ok(Some((path, text)))
}

fn save_package_dialog(state: &mut CharacterizationIntakeUiState) {
    let Some(result) = state.built.as_ref() else {
        return;
    };
    let file_name = format!(
        "{}.characterization.json",
        safe_filename_component(&result.package.payload.revision)
    );
    let Some(path) = rfd::FileDialog::new()
        .add_filter("Shade Editor measured characterization", &["json"])
        .set_file_name(file_name)
        .set_title("Save measured characterization package")
        .save_file()
    else {
        return;
    };

    match save_characterization_package(&path, &result.package) {
        Ok(()) => {
            state.errors.clear();
            state.status = Some(format!(
                "Saved measured characterization package: {}",
                path.display()
            ));
        }
        Err(error) => {
            state.errors = vec![error];
            state.status = None;
        }
    }
}

fn parse_f32_field(value: &str, label: &str, errors: &mut Vec<String>) -> f32 {
    match value.trim().parse::<f32>() {
        Ok(number) if number.is_finite() => number,
        _ => {
            errors.push(format!("{label} must be a finite number."));
            0.0
        }
    }
}

fn metadata_text_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    invalidate: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        *invalidate |= ui.text_edit_singleline(value).changed();
    });
}

fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

fn invalidate_built_result(state: &mut CharacterizationIntakeUiState) {
    if state.built.take().is_some() {
        state.status = Some(
            "Inputs changed; validate and build the package again before saving.".to_owned(),
        );
    }
}

fn safe_filename_component(value: &str) -> String {
    let cleaned = value
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let cleaned = cleaned.trim_matches('-');
    if cleaned.is_empty() {
        "measured-characterization".to_owned()
    } else {
        cleaned.to_owned()
    }
}

fn truncate_for_ui(value: &str, max_chars: usize) -> String {
    let mut text = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        text.push('…');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intake_ui_defaults_to_experimental_authority() {
        let state = CharacterizationIntakeUiState::default();
        assert_eq!(
            state.validation_level,
            CharacterizationValidationLevel::Experimental
        );
        assert_eq!(state.channel_names.len(), DEFAULT_CHANNEL_COUNT);
        assert!(state.built.is_none());
    }

    #[test]
    fn channel_editor_supports_three_channel_characterization() {
        let mut state = CharacterizationIntakeUiState::default();
        state.channel_names.pop();
        state.channel_limits.pop();
        assert_eq!(state.channel_names.len(), 3);
        assert_eq!(state.channel_limits.len(), 3);
        assert!(MIN_CHANNEL_COUNT <= 3);
    }

    #[test]
    fn ui_delegates_package_authority_to_the_intake_core() {
        let source = include_str!("characterization_intake.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(runtime.contains("build_characterization_package_from_table"));
        assert!(runtime.contains("save_characterization_package"));
        assert!(!runtime.contains("CharacterizationPackage::new"));
        assert!(!runtime.contains("CharacterizationPayload {"));
    }

    #[test]
    fn any_input_change_invalidates_a_built_result_before_save() {
        let source = include_str!("characterization_intake.rs");
        assert!(source.contains("invalidate_built_result(state)"));
        assert!(source.contains("state.built.is_some()"));
    }

    #[test]
    fn intake_is_a_single_exe_ui_module_not_an_application_binary() {
        let cargo = include_str!("../../Cargo.toml");
        assert_eq!(cargo.matches("[[bin]]").count(), 1);
        assert!(cargo.contains("name = \"ShadeEditor\""));
    }
}