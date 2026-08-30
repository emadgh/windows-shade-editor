use crate::*;
use eframe::egui;
use std::io::Read;
use std::path::{Path, PathBuf};
use windows_shade_editor::color_conversion::{ProductionProvenance, ProjectRole};
use windows_shade_editor::conversion_audit::ConversionAuditRecord;
use windows_shade_editor::external_validation_evidence::{
    ExternalValidationPacket, ExternalValidationStatus,
};

const MAX_EXTERNAL_VALIDATION_PACKET_BYTES: usize = 2 * 1024 * 1024;

impl ShadeApp {
    /// Export/import bridge from durable Production conversion audits to the
    /// manual Photoshop/RIP evidence contract. Generated packets always start
    /// Pending; returned packets are accepted only after exact audit binding.
    pub(crate) fn ui_external_validation_packet_menu(&mut self, ui: &mut egui::Ui) {
        if self.project.project_role != ProjectRole::Production
            || self.project.conversion_audits.is_empty()
        {
            return;
        }

        let audits = self.project.conversion_audits.clone();
        let provenances = self.project.production_provenance.clone();
        let mut export_request: Option<(String, String)> = None;
        let mut validate_request: Option<ConversionAuditRecord> = None;
        let mut export_error: Option<String> = None;

        ui.menu_button("External validation", |ui| {
            ui.set_min_width(560.0);
            ui.strong("Photoshop / ceramic RIP validation packets");
            ui.small(
                "Export starts Pending. Re-import validates manual evidence against the exact persisted Production conversion audit.",
            );
            ui.separator();

            for (index, audit) in audits.iter().enumerate() {
                let packet = packet_for_bound_audit(audit, &provenances);
                ui.group(|ui| {
                    ui.strong(format!(
                        "{} · {}-bit · {} channel(s)",
                        audit.target.target_name,
                        audit.target.bit_depth,
                        audit.target.channel_names.len()
                    ));
                    ui.small(format!("Output SHA-256: {}", short_hash(&audit.output.sha256)));
                    ui.small(format!(
                        "Channel order: {}",
                        audit.target.channel_names.join(" · ")
                    ));

                    match packet {
                        Ok(packet) => {
                            ui.small(
                                "Photoshop: Pending · Ceramic RIP: Pending · manual evidence required",
                            );
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("Export validation packet...").clicked() {
                                    match packet.to_pretty_json() {
                                        Ok(json) => {
                                            export_request = Some((
                                                format!(
                                                    "{}-external-validation.json",
                                                    safe_filename_component(
                                                        &packet.fixture.output_file
                                                    )
                                                ),
                                                json,
                                            ));
                                        }
                                        Err(error) => export_error = Some(error),
                                    }
                                }
                                if ui.button("Validate completed packet...").clicked() {
                                    validate_request = Some(audit.clone());
                                }
                            });
                        }
                        Err(error) => {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Cannot use external validation: audit is not bound to exactly one Production provenance record ({error})"
                                ))
                                .color(egui::Color32::LIGHT_RED),
                            );
                        }
                    }
                });
                if index + 1 < audits.len() {
                    ui.add_space(3.0);
                }
            }
        });

        if let Some(error) = export_error {
            self.report_error(format!("Cannot export external validation packet: {error}"));
        }
        if let Some((default_name, json)) = export_request {
            match save_validation_packet_json(&default_name, &json) {
                Ok(Some(path)) => self.report_info(format!(
                    "External validation packet exported: {}",
                    path.display()
                )),
                Ok(None) => {}
                Err(error) => self.report_error(format!(
                    "Cannot export external validation packet: {error}"
                )),
            }
        }
        if let Some(audit) = validate_request {
            match select_and_validate_completed_packet(&audit) {
                Ok(Some(packet)) => {
                    let photoshop = status_label(packet.photoshop.status);
                    let rip = status_label(packet.ceramic_rip.status);
                    if packet.externally_accepted() {
                        self.report_info(format!(
                            "External validation packet is exactly audit-bound and complete: Photoshop {photoshop}, Ceramic RIP {rip}."
                        ));
                    } else {
                        self.report_info(format!(
                            "External validation packet is exactly audit-bound but not fully accepted: Photoshop {photoshop}, Ceramic RIP {rip}."
                        ));
                    }
                }
                Ok(None) => {}
                Err(error) => self.report_error(format!(
                    "External validation packet rejected: {error}"
                )),
            }
        }
    }
}

fn packet_for_bound_audit(
    audit: &ConversionAuditRecord,
    provenances: &[ProductionProvenance],
) -> Result<ExternalValidationPacket, String> {
    audit.validate()?;
    let matches = provenances
        .iter()
        .filter(|provenance| audit.validate_against_provenance(provenance).is_ok())
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [provenance] => {
            audit.validate_against_provenance(provenance)?;
            ExternalValidationPacket::from_conversion_audit(audit)
        }
        [] => Err("no exact provenance match".to_owned()),
        _ => Err("multiple exact provenance matches".to_owned()),
    }
}

fn select_and_validate_completed_packet(
    audit: &ConversionAuditRecord,
) -> Result<Option<ExternalValidationPacket>, String> {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("External validation JSON", &["json"])
        .pick_file()
    else {
        return Ok(None);
    };
    let packet = load_validation_packet(&path)?;
    packet.validate_against_conversion_audit(audit)?;
    Ok(Some(packet))
}

fn load_validation_packet(path: &Path) -> Result<ExternalValidationPacket, String> {
    let file = std::fs::File::open(path).map_err(|error| {
        format!(
            "Cannot open external validation packet '{}': {error}",
            path.display()
        )
    })?;
    let mut bytes = Vec::new();
    file.take((MAX_EXTERNAL_VALIDATION_PACKET_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            format!(
                "Cannot read external validation packet '{}': {error}",
                path.display()
            )
        })?;
    validate_packet_size(bytes.len())?;
    let json = String::from_utf8(bytes)
        .map_err(|error| format!("External validation packet is not UTF-8 JSON: {error}"))?;
    ExternalValidationPacket::from_json(&json)
}

fn validate_packet_size(size: usize) -> Result<(), String> {
    if size > MAX_EXTERNAL_VALIDATION_PACKET_BYTES {
        return Err(format!(
            "External validation packet exceeds the {} byte safety limit.",
            MAX_EXTERNAL_VALIDATION_PACKET_BYTES
        ));
    }
    Ok(())
}

fn save_validation_packet_json(
    default_name: &str,
    json: &str,
) -> Result<Option<PathBuf>, String> {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("JSON", &["json"])
        .set_file_name(default_name)
        .save_file()
    else {
        return Ok(None);
    };
    windows_shade_editor::safe_fs::atomic_write(&path, json.as_bytes(), None)?;
    Ok(Some(path))
}

fn status_label(status: ExternalValidationStatus) -> &'static str {
    match status {
        ExternalValidationStatus::Pending => "Pending",
        ExternalValidationStatus::Passed => "Passed",
        ExternalValidationStatus::Failed => "Failed",
    }
}

fn short_hash(value: &str) -> String {
    value.chars().take(12).collect()
}

fn safe_filename_component(value: &str) -> String {
    let cleaned = value
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let cleaned = cleaned.trim_matches('-');
    if cleaned.is_empty() {
        "production-output".to_owned()
    } else {
        cleaned.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_validation_filename_is_portable() {
        assert_eq!(
            safe_filename_component("Face 01 / separated.tif"),
            "Face-01---separated.tif"
        );
        assert_eq!(safe_filename_component("***"), "production-output");
    }

    #[test]
    fn completed_packet_import_is_strictly_size_bounded() {
        assert!(validate_packet_size(MAX_EXTERNAL_VALIDATION_PACKET_BYTES).is_ok());
        assert!(validate_packet_size(MAX_EXTERNAL_VALIDATION_PACKET_BYTES + 1).is_err());
    }

    #[test]
    fn external_validation_surface_requires_exact_binding_before_reporting_returned_evidence() {
        let source = include_str!("external_validation.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(runtime.contains("starts Pending"));
        assert!(runtime.contains("Validate completed packet..."));
        assert!(runtime.contains("ExternalValidationPacket::from_json"));
        assert!(runtime.contains("packet.validate_against_conversion_audit(audit)?"));
        assert!(runtime.contains("file.take((MAX_EXTERNAL_VALIDATION_PACKET_BYTES + 1) as u64)"));
    }
}
