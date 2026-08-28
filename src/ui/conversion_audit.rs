use crate::*;
use chrono::{Local, TimeZone};
use eframe::egui;
use std::path::{Path, PathBuf};
use windows_shade_editor::color_conversion::{ProductionProvenance, ProjectRole};
use windows_shade_editor::conversion_audit::ConversionAuditRecord;
use windows_shade_editor::conversion_analytics::ConversionUsageReport;

#[derive(Clone)]
struct AuditUiRow {
    audit: ConversionAuditRecord,
    face_index: Option<usize>,
    face_label: String,
    binding_error: Option<String>,
}

impl ShadeApp {
    /// Read-only operator surface for the durable audit records embedded in a
    /// Production `.shade`. Nothing here reconstructs conversion state from the
    /// current UI; the menu only validates, displays and exports persisted data.
    pub(crate) fn ui_conversion_audit_menu(&mut self, ui: &mut egui::Ui) {
        if self.project.project_role != ProjectRole::Production {
            return;
        }

        let project_name = self.project.name.clone();
        let project_file_name = self
            .project_path
            .as_deref()
            .and_then(Path::file_name)
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "production.shade".to_owned());
        let provenances = self.project.production_provenance.clone();
        let rows = build_audit_rows(
            &self.project.conversion_audits,
            &provenances,
            &self
                .project
                .faces
                .iter()
                .map(|face| face.label.clone())
                .collect::<Vec<_>>(),
        );
        let audit_count = rows.len();
        let invalid_count = rows
            .iter()
            .filter(|row| row.binding_error.is_some())
            .count();
        let current_face = self.current_face;

        let mut export_request: Option<(String, String)> = None;
        let mut export_error: Option<String> = None;

        let label = if audit_count == 0 {
            "Conversion audit · legacy".to_owned()
        } else if invalid_count > 0 {
            format!("Conversion audit ({audit_count}, {invalid_count} invalid)")
        } else {
            format!("Conversion audit ({audit_count})")
        };

        ui.menu_button(label, |ui| {
            ui.set_min_width(680.0);
            ui.strong("Production conversion audit");
            ui.small(
                "Persisted conversion evidence only. Exported JSON is portable and redacts absolute Source/Production paths.",
            );
            ui.separator();

            if rows.is_empty() {
                ui.label("This Production project predates durable conversion audit records.");
                ui.small(
                    "Existing provenance remains usable, but no audit record can be reconstructed safely after the fact.",
                );
                return;
            }

            ui.horizontal_wrapped(|ui| {
                ui.small(format!("{audit_count} audit record(s)"));
                if invalid_count > 0 {
                    ui.label(
                        egui::RichText::new(format!("{invalid_count} invalid binding(s)"))
                            .color(egui::Color32::LIGHT_RED),
                    );
                }
                if ui.button("Export all portable JSON...").clicked() {
                    match portable_audit_bundle(&project_name, &project_file_name, &rows) {
                        Ok(json) => {
                            export_request = Some((
                                format!(
                                    "{}-conversion-audits.json",
                                    safe_filename_component(&project_name)
                                ),
                                json,
                            ));
                        }
                        Err(error) => export_error = Some(error),
                    }
                }
            });
            ui.separator();

            for (ordinal, row) in rows.iter().enumerate() {
                let active = row.face_index == Some(current_face);
                let timestamp = format_timestamp(row.audit.output.converted_at_unix_ms);
                let heading = format!(
                    "{}{} · {} · {}",
                    if active { "▶ " } else { "" },
                    row.face_label,
                    row.audit.target.target_name,
                    timestamp
                );
                egui::CollapsingHeader::new(heading)
                    .id_salt(("conversion-audit-row", ordinal, &row.audit.output.sha256))
                    .default_open(active || audit_count == 1)
                    .show(ui, |ui| {
                        if let Some(error) = row.binding_error.as_deref() {
                            ui.label(
                                egui::RichText::new(format!("Invalid persisted audit: {error}"))
                                    .color(egui::Color32::LIGHT_RED),
                            );
                        } else {
                            ui.label(
                                egui::RichText::new("Validated against Production provenance")
                                    .color(egui::Color32::LIGHT_GREEN),
                            );
                        }

                        ui.separator();
                        ui.strong("Conversion");
                        ui.small(format!("Producer: Shade Editor {}", row.audit.app_version));
                        ui.small(format!("Engine: {:?}", row.audit.target.engine_mode));
                        ui.small(format!(
                            "Target: {} · {}-bit · {} channel(s)",
                            row.audit.target.target_name,
                            row.audit.target.bit_depth,
                            row.audit.target.channel_names.len()
                        ));
                        ui.small(format!(
                            "Channel order: {}",
                            row.audit.target.channel_names.join(" · ")
                        ));
                        if let Some(hash) = row.audit.target.output_profile_sha256.as_deref() {
                            ui.small(format!("Output ICC SHA-256: {hash}"));
                        }
                        if let Some(hash) = row.audit.target.device_link_sha256.as_deref() {
                            ui.small(format!("DeviceLink SHA-256: {hash}"));
                        }
                        if let Some(id) = row.audit.target.characterization_id.as_deref() {
                            ui.small(format!("Characterization: {id}"));
                        }
                        ui.small(format!("Recipe SHA-256: {}", row.audit.recipe_sha256));

                        if let Some(provenance) = row
                            .face_index
                            .and_then(|index| provenances.get(index))
                        {
                            let recipe = &provenance.recipe;
                            ui.small(format!(
                                "Rendering intent: {:?} · Black point compensation: {}",
                                recipe.rendering_intent,
                                if recipe.black_point_compensation { "on" } else { "off" }
                            ));
                            if let Some(policy) = recipe.source_transparency_policy.as_ref() {
                                ui.small(format!("Source transparency policy: {policy:?}"));
                            }

                            ui.add_space(4.0);
                            ui.strong("Separation strategy / limits");
                            ui.small(format!("Preset: {}", recipe.strategy.preset_name));
                            ui.small(format!(
                                "Black channel: {} · strength {:.3} · start {:.3} · max {:.3}",
                                recipe.strategy.black_channel.as_deref().unwrap_or("None"),
                                recipe.strategy.black_generation_strength,
                                recipe.strategy.black_start,
                                recipe.strategy.black_max
                            ));
                            ui.small(format!(
                                "Neutral chroma threshold: {:.3}",
                                recipe.strategy.neutral_chroma_threshold
                            ));
                            if let Some(max_delta_e00) = recipe.strategy.max_delta_e00 {
                                ui.small(format!("Strategy max ΔE00: {max_delta_e00:.3}"));
                            }
                            let effective_total = effective_total_ink_limit(
                                recipe.target.total_ink_limit,
                                recipe.strategy.total_ink_limit,
                            );
                            ui.small(format!(
                                "Target total-ink limit: {} · strategy total-ink limit: {} · effective: {}",
                                format_optional_float(recipe.target.total_ink_limit),
                                format_optional_float(recipe.strategy.total_ink_limit),
                                format_optional_float(effective_total),
                            ));
                            let channel_limits = recipe
                                .target
                                .channels
                                .iter()
                                .filter_map(|channel| {
                                    channel.max_coverage.map(|limit| {
                                        format!("{} {:.1}%", channel.name, limit * 100.0)
                                    })
                                })
                                .collect::<Vec<_>>();
                            ui.small(if channel_limits.is_empty() {
                                "Per-channel limits: none".to_owned()
                            } else {
                                format!("Per-channel limits: {}", channel_limits.join(" · "))
                            });
                            let biases = recipe
                                .strategy
                                .per_ink_bias
                                .iter()
                                .filter(|(_, value)| value.abs() > f32::EPSILON)
                                .map(|(name, value)| format!("{name} {value:+.3}"))
                                .collect::<Vec<_>>();
                            if !biases.is_empty() {
                                ui.small(format!("Ink preference bias: {}", biases.join(" · ")));
                            }
                        }

                        ui.add_space(4.0);
                        ui.strong("Source");
                        ui.small(format!("Project: {}", row.audit.source.project_path));
                        ui.small(format!(
                            "Project SHA-256: {}",
                            row.audit.source.project_file_sha256
                        ));
                        ui.small(format!("Face: {}", row.audit.source.face_path));
                        if let Some(raster) = row.audit.source.raster {
                            ui.small(format!(
                                "Raster: {} · {} · {}-bit · {} channel(s)",
                                raster.format.label(),
                                raster.color_model.label(),
                                raster.bit_depth,
                                raster.channel_count
                            ));
                        } else {
                            ui.small("Raster: unavailable in this legacy audit record");
                        }
                        ui.small(format!(
                            "Source file SHA-256: {}",
                            row.audit.source.source_file_sha256
                        ));
                        ui.small(format!(
                            "Source ICC SHA-256: {}",
                            row.audit.source.source_profile_sha256
                        ));
                        if let Some(snapshot_id) = row.audit.source.snapshot_id {
                            ui.small(format!("Source snapshot: #{snapshot_id}"));
                        }

                        ui.add_space(4.0);
                        ui.strong("Committed output");
                        ui.small(format!("Path: {}", row.audit.output.path));
                        ui.small(format!("SHA-256: {}", row.audit.output.sha256));
                        ui.small(format!("Converted: {timestamp}"));

                        ui.add_space(4.0);
                        ui.strong("Committed ink usage / constraints");
                        if let Some(usage) = row.audit.usage.as_ref() {
                            draw_persisted_usage(ui, usage);
                        } else {
                            ui.small(
                                "Committed usage analytics are unavailable in this legacy audit record.",
                            );
                        }

                        if let Some(custom) = row.audit.custom_optimizer.as_ref() {
                            ui.add_space(4.0);
                            ui.strong("Custom Optimizer authority evidence");
                            ui.small(format!(
                                "LUT identity: {}",
                                custom.lut_identity_content_id
                            ));
                            ui.small(format!("LUT payload SHA-256: {}", custom.lut_payload_sha256));
                            ui.small(format!(
                                "Validation report: {}",
                                custom.validation_report_content_id
                            ));
                            ui.small(format!("Characterization: {}", custom.characterization_id));
                            ui.small(format!("Threshold set: {}", custom.threshold_set_content_id));
                            ui.small(format!(
                                "Calibration manifest: {}",
                                custom.calibration_manifest_content_id
                            ));
                            ui.small(format!(
                                "Calibration approval: {}",
                                custom.calibration_approval_content_id
                            ));
                            ui.small(format!(
                                "PCS compatibility: {:?} · {}",
                                custom.pcs_compatibility_method,
                                custom.pcs_compatibility_content_id
                            ));
                        }
                        ui.small(
                            "Measured ΔE00: unavailable until approved measured PCS/characterization evidence is present.",
                        );

                        ui.add_space(4.0);
                        ui.strong(format!("Findings ({})", row.audit.findings.len()));
                        if row.audit.findings.is_empty() {
                            ui.small("No non-blocking preflight findings were captured.");
                        } else {
                            for finding in &row.audit.findings {
                                ui.group(|ui| {
                                    ui.small(format!(
                                        "{} · {}",
                                        finding.code,
                                        if finding.acknowledged {
                                            "acknowledged"
                                        } else {
                                            "not explicitly acknowledged"
                                        }
                                    ));
                                    ui.label(&finding.message);
                                });
                            }
                        }

                        ui.add_space(6.0);
                        if ui.button("Export this portable JSON...").clicked() {
                            match row.audit.to_portable_pretty_json() {
                                Ok(json) => {
                                    export_request = Some((
                                        format!(
                                            "{}-conversion-audit.json",
                                            safe_filename_component(&row.face_label)
                                        ),
                                        json,
                                    ));
                                }
                                Err(error) => export_error = Some(error),
                            }
                        }
                    });
                ui.add_space(2.0);
            }
        });

        if let Some(error) = export_error {
            self.report_error(format!("Cannot export conversion audit: {error}"));
        }
        if let Some((default_name, json)) = export_request {
            match save_portable_audit_json(&default_name, &json) {
                Ok(Some(path)) => self.report_info(format!(
                    "Conversion audit exported: {}",
                    path.display()
                )),
                Ok(None) => {}
                Err(error) => self.report_error(format!("Cannot export conversion audit: {error}")),
            }
        }
    }
}

fn draw_persisted_usage(ui: &mut egui::Ui, usage: &ConversionUsageReport) {
    ui.small(format!("Pixels analyzed: {}", usage.pixel_count));
    ui.small(format!(
        "Total ink · mean {:.1}% · p50 {:.1}% · p95 {:.1}% · p99 {:.1}% · peak {:.1}%",
        usage.mean_total_ink * 100.0,
        usage.total_ink_percentiles.p50 * 100.0,
        usage.total_ink_percentiles.p95 * 100.0,
        usage.total_ink_percentiles.p99 * 100.0,
        usage.peak_total_ink * 100.0,
    ));
    if let Some(hit_percent) = usage.total_ink_limit_hit_percent {
        ui.small(format!("Total-ink limit hits: {hit_percent:.2}%"));
    } else {
        ui.small("Total-ink limit hits: no total-ink limit configured");
    }
    for channel in &usage.channels {
        ui.small(format!(
            "{} · mean {:.1}% · p95 {:.1}% · p99 {:.1}% · peak {:.1}% · non-zero {:.2}%{}",
            channel.name,
            channel.mean_coverage * 100.0,
            channel.percentiles.p95 * 100.0,
            channel.percentiles.p99 * 100.0,
            channel.peak_coverage * 100.0,
            channel.nonzero_percent,
            channel
                .limit_hit_percent
                .map(|value| format!(" · limit hits {value:.2}%"))
                .unwrap_or_else(|| " · no channel limit".to_owned())
        ));
    }
    if let Some(share) = usage.neutral_black_share {
        ui.small(format!("Neutral Black share: {:.1}%", share * 100.0));
    } else {
        ui.small("Neutral Black share: unavailable without measured neutral classification");
    }
}

fn build_audit_rows(
    audits: &[ConversionAuditRecord],
    provenances: &[ProductionProvenance],
    face_labels: &[String],
) -> Vec<AuditUiRow> {
    audits
        .iter()
        .cloned()
        .map(|audit| match audit_binding_index(&audit, provenances) {
            Ok(index) => AuditUiRow {
                face_label: face_labels
                    .get(index)
                    .map(|label| label.trim())
                    .filter(|label| !label.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("Production Face {}", index + 1)),
                face_index: Some(index),
                audit,
                binding_error: None,
            },
            Err(error) => AuditUiRow {
                face_label: Path::new(&audit.output.path)
                    .file_name()
                    .map(|value| value.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Unbound audit".to_owned()),
                face_index: None,
                audit,
                binding_error: Some(error),
            },
        })
        .collect()
}

fn audit_binding_index(
    audit: &ConversionAuditRecord,
    provenances: &[ProductionProvenance],
) -> Result<usize, String> {
    let candidates = provenances
        .iter()
        .enumerate()
        .filter(|(_, provenance)| {
            paths_match_text(&audit.output.path, &provenance.output_path)
                && audit
                    .output
                    .sha256
                    .trim()
                    .eq_ignore_ascii_case(provenance.output_sha256.trim())
        })
        .collect::<Vec<_>>();

    let (index, provenance) = match candidates.as_slice() {
        [(index, provenance)] => (*index, *provenance),
        [] => return Err("no matching Production provenance record".to_owned()),
        _ => return Err("multiple Production provenance records match this audit".to_owned()),
    };
    audit.validate_against_provenance(provenance)?;
    Ok(index)
}

fn portable_audit_bundle(
    project_name: &str,
    project_file_name: &str,
    rows: &[AuditUiRow],
) -> Result<String, String> {
    let mut audits = Vec::with_capacity(rows.len());
    for row in rows {
        let json = row.audit.to_portable_pretty_json()?;
        let value = serde_json::from_str::<serde_json::Value>(&json)
            .map_err(|error| format!("Cannot decode portable audit JSON: {error}"))?;
        audits.push(value);
    }
    serde_json::to_string_pretty(&serde_json::json!({
        "schema": "shade-editor-conversion-audit-bundle-v1",
        "project_name": project_name,
        "project_file": project_file_name,
        "audit_count": audits.len(),
        "audits": audits,
    }))
    .map_err(|error| format!("Cannot serialize portable audit bundle: {error}"))
}

fn save_portable_audit_json(default_name: &str, json: &str) -> Result<Option<PathBuf>, String> {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("JSON", &["json"])
        .set_file_name(default_name)
        .save_file()
    else {
        return Ok(None);
    };
    safe_fs::atomic_write(&path, json.as_bytes(), None)?;
    Ok(Some(path))
}

fn format_timestamp(unix_ms: i64) -> String {
    Local
        .timestamp_millis_opt(unix_ms)
        .single()
        .map(|value| value.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| unix_ms.to_string())
}

fn effective_total_ink_limit(target: Option<f32>, strategy: Option<f32>) -> Option<f32> {
    match (target, strategy) {
        (Some(target), Some(strategy)) => Some(target.min(strategy)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn format_optional_float(value: Option<f32>) -> String {
    value
        .map(|value| format!("{value:.3}"))
        .unwrap_or_else(|| "none".to_owned())
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
        "production".to_owned()
    } else {
        cleaned.to_owned()
    }
}

fn paths_match_text(left: &str, right: &str) -> bool {
    left.trim()
        .replace('/', "\\")
        .eq_ignore_ascii_case(&right.trim().replace('/', "\\"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_filename_component_never_leaks_path_separators() {
        assert_eq!(safe_filename_component(r"Face 01 / CMYK"), "Face-01---CMYK");
        assert_eq!(safe_filename_component("***"), "production");
    }

    #[test]
    fn path_matching_is_windows_separator_and_case_tolerant() {
        assert!(paths_match_text(
            r"C:\Production\Face.tif",
            "c:/production/face.tif"
        ));
    }

    #[test]
    fn effective_total_ink_limit_uses_stricter_policy() {
        assert_eq!(effective_total_ink_limit(Some(2.4), Some(2.0)), Some(2.0));
        assert_eq!(effective_total_ink_limit(Some(1.8), None), Some(1.8));
        assert_eq!(effective_total_ink_limit(None, Some(2.1)), Some(2.1));
        assert_eq!(effective_total_ink_limit(None, None), None);
    }

    #[test]
    fn audit_ui_uses_only_persisted_raster_and_usage_evidence() {
        let source = include_str!("conversion_audit.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(runtime.contains("row.audit.source.raster"));
        assert!(runtime.contains("row.audit.usage"));
        assert!(runtime.contains("draw_persisted_usage"));
        assert!(!runtime.contains("analyze_conversion_tiff"));
        assert!(!runtime.contains("ConversionUsageAccumulator"));
    }
}
