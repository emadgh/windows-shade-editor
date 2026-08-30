use crate::*;
use eframe::egui;
use std::path::PathBuf;
use windows_shade_editor::color_conversion::{ProductionProvenance, ProjectRole};
use windows_shade_editor::conversion_audit::ConversionAuditRecord;
use windows_shade_editor::external_validation_evidence::ExternalValidationPacket;

impl ShadeApp {
    /// Export-only bridge from durable Production conversion audits to the
    /// manual Photoshop/RIP evidence contract. This surface never fabricates
    /// observations and every generated consumer section starts Pending.
    pub(crate) fn ui_external_validation_packet_menu(&mut self, ui: &mut egui::Ui) {
        if self.project.project_role != ProjectRole::Production
            || self.project.conversion_audits.is_empty()
        {
            return;
        }

        let audits = self.project.conversion_audits.clone();
        let provenances = self.project.production_provenance.clone();
        let mut export_request: Option<(String, String)> = None;
        let mut export_error: Option<String> = None;

        ui.menu_button("External validation", |ui| {
            ui.set_min_width(520.0);
            ui.strong("Photoshop / ceramic RIP validation packets");
            ui.small(
                "Each packet is audit-bound and starts Pending. Exporting a packet is not external approval.",
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
                            if ui.button("Export validation packet...").clicked() {
                                match packet.to_pretty_json() {
                                    Ok(json) => {
                                        export_request = Some((
                                            format!(
                                                "{}-external-validation.json",
                                                safe_filename_component(&packet.fixture.output_file)
                                            ),
                                            json,
                                        ));
                                    }
                                    Err(error) => export_error = Some(error),
                                }
                            }
                        }
                        Err(error) => {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Cannot export: audit is not bound to exactly one Production provenance record ({error})"
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
    fn external_validation_surface_never_claims_generated_packet_is_approval() {
        let source = include_str!("external_validation.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(runtime.contains("starts Pending"));
        assert!(runtime.contains("is not external approval"));
        assert!(!runtime.contains("externally_accepted()"));
    }
}
