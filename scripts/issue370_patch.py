from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected exactly one match, found {count}\n--- OLD ---\n{old}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")
    print(f"patched {path}")


fallback = r'''use lcms2::Profile;
use sha2::{Digest, Sha256};

use crate::model::IccProfileIdentity;

/// Explicit production interpretation used only for otherwise-untagged RGB artwork.
///
/// The generated LittleCMS sRGB payload is hashed and stored in the conversion recipe,
/// so using the fallback remains deterministic and auditable rather than an unmanaged guess.
pub const SRGB_FALLBACK_DESCRIPTION: &str = "sRGB fallback (untagged RGB)";

pub fn srgb_fallback_icc() -> Result<Vec<u8>, String> {
    Profile::new_srgb()
        .icc()
        .map_err(|error| format!("Cannot materialize built-in sRGB fallback ICC: {error}"))
}

pub fn srgb_fallback_identity() -> Result<IccProfileIdentity, String> {
    let bytes = srgb_fallback_icc()?;
    Ok(IccProfileIdentity {
        description: SRGB_FALLBACK_DESCRIPTION.to_owned(),
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    })
}

pub fn is_srgb_fallback_identity(identity: &IccProfileIdentity) -> bool {
    is_srgb_fallback_sha256(&identity.sha256)
        && identity.description.eq_ignore_ascii_case(SRGB_FALLBACK_DESCRIPTION)
}

pub fn is_srgb_fallback_sha256(sha256: &str) -> bool {
    srgb_fallback_identity().is_ok_and(|fallback| {
        fallback.sha256.eq_ignore_ascii_case(sha256.trim())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_identity_is_stable_and_self_verifying() {
        let first = srgb_fallback_identity().unwrap();
        let second = srgb_fallback_identity().unwrap();
        assert_eq!(first, second);
        assert_eq!(first.sha256.len(), 64);
        assert!(is_srgb_fallback_identity(&first));
        assert!(is_srgb_fallback_sha256(&first.sha256));
    }
}
'''
Path("src/source_profile_fallback.rs").write_text(fallback, encoding="utf-8", newline="\n")
print("created src/source_profile_fallback.rs")

replace_once(
    "src/lib.rs",
    "pub mod source_tiff_writer;\npub mod source_transparency;",
    "pub mod source_tiff_writer;\npub mod source_profile_fallback;\npub mod source_transparency;",
)

# Keep the existing embedded-profile inspector truthful; add a production resolver that
# supplies an explicit sRGB identity only for untagged RGB sources.
replace_once(
    "src/color_management.rs",
    '''pub fn inspect_production_source_profile_runtime(
    path: &Path,
    expected_identity: &IccProfileIdentity,
    source_model: RuntimeColorModel,
) -> Result<InstalledIccProfile, String> {''',
    '''pub fn production_source_profile_identity_or_rgb_fallback_for_runtime(
    model: RuntimeColorModel,
    embedded_icc: Option<&[u8]>,
) -> Result<Option<IccProfileIdentity>, String> {
    let embedded = production_embedded_profile_identity_for_runtime(model, embedded_icc)?;
    if embedded.is_some() || model != RuntimeColorModel::Rgb {
        return Ok(embedded);
    }
    let fallback = windows_shade_editor::source_profile_fallback::srgb_fallback_identity()?;
    Ok(Some(IccProfileIdentity {
        description: fallback.description,
        sha256: fallback.sha256,
    }))
}

pub fn inspect_production_source_profile_runtime(
    path: &Path,
    expected_identity: &IccProfileIdentity,
    source_model: RuntimeColorModel,
) -> Result<InstalledIccProfile, String> {''',
)

# Route all three production entry points through the same RGB fallback resolver.
for path in [
    "src/ui/color_conversion.rs",
    "src/ui/conversion_batch.rs",
    "src/ui/conversion_candidate_preview.rs",
]:
    replace_once(
        path,
        "color_management::production_embedded_profile_identity_for_runtime(",
        "color_management::production_source_profile_identity_or_rgb_fallback_for_runtime(",
    )

# Single-convert label should describe fallback accurately rather than claiming it was embedded.
replace_once(
    "src/ui/color_conversion.rs",
    '''        Ok(Some(identity)) => (
            SourceProfileState::Embedded(conversion_profile_identity(&identity)),
            format!("Embedded: {}", identity.description),
            None,
            false,
        ),''',
    '''        Ok(Some(identity)) => {
            let converted = conversion_profile_identity(&identity);
            let label = if windows_shade_editor::source_profile_fallback::is_srgb_fallback_identity(&converted) {
                format!("No Source ICC · fallback: {}", identity.description)
            } else {
                format!("Embedded: {}", identity.description)
            };
            (
                SourceProfileState::Embedded(converted),
                label,
                None,
                false,
            )
        },''',
)

# Batch per-Face labels follow the same terminology.
replace_once(
    "src/ui/conversion_batch.rs",
    '''        Ok(Some(identity)) => {
            let identity = ConversionIccProfileIdentity {
                description: identity.description,
                sha256: identity.sha256,
            };
            (
                SourceProfileState::Embedded(identity.clone()),
                CapturedSourceProfile::Embedded,
                format!("Embedded: {}", identity.description),
            )
        }''',
    '''        Ok(Some(identity)) => {
            let identity = ConversionIccProfileIdentity {
                description: identity.description,
                sha256: identity.sha256,
            };
            let profile_label = if windows_shade_editor::source_profile_fallback::is_srgb_fallback_identity(&identity) {
                format!("No Source ICC · fallback: {}", identity.description)
            } else {
                format!("Embedded: {}", identity.description)
            };
            (
                SourceProfileState::Embedded(identity),
                CapturedSourceProfile::Embedded,
                profile_label,
            )
        }''',
)

# Shared preflight: untagged RGB becomes a visible warning; CMYK/unsupported missing profiles remain blocking.
replace_once(
    "src/conversion_preflight.rs",
    '''use crate::source_transparency::SourceTransparencyPolicy;
use crate::tiff_io::ColorModel;''',
    '''use crate::source_profile_fallback::is_srgb_fallback_identity;
use crate::source_transparency::SourceTransparencyPolicy;
use crate::tiff_io::ColorModel;''',
)
replace_once(
    "src/conversion_preflight.rs",
    '''    match &input.profile {
        SourceProfileState::Missing => findings.push(PreflightFinding {
            code: PreflightCode::MissingSourceProfile,
            severity: PreflightSeverity::Blocking,
            title: "Source ICC profile required",
            detail: "Assign or confirm the source profile before conversion. Shade Editor must not silently guess production source colorimetry.".to_owned(),
            action: Some("Assign Source Profile..."),
        }),
        SourceProfileState::Invalid(reason) => findings.push(PreflightFinding {
            code: PreflightCode::InvalidSourceProfile,
            severity: PreflightSeverity::Blocking,
            title: "Invalid source ICC profile",
            detail: reason.clone(),
            action: Some("Assign Source Profile..."),
        }),
        SourceProfileState::Embedded(identity) | SourceProfileState::Assigned(identity)
            if identity.sha256.trim().is_empty() =>
        {
            findings.push(PreflightFinding {
                code: PreflightCode::InvalidSourceProfile,
                severity: PreflightSeverity::Blocking,
                title: "Source ICC identity is incomplete",
                detail: "The selected source ICC profile has no stable SHA-256 identity and cannot be captured in a reproducible conversion recipe.".to_owned(),
                action: Some("Reassign Source Profile..."),
            });
        }
        SourceProfileState::Embedded(_) | SourceProfileState::Assigned(_) => {}
    }''',
    '''    match &input.profile {
        SourceProfileState::Missing if input.color_model == ColorModel::Rgb => findings.push(PreflightFinding {
            code: PreflightCode::MissingSourceProfile,
            severity: PreflightSeverity::Warning,
            title: "No Source ICC — sRGB fallback will be used",
            detail: "This RGB artwork has no assigned or embedded Source ICC. Conversion is allowed and will interpret its RGB samples as sRGB. Assign a Source ICC if the artwork uses another RGB encoding.".to_owned(),
            action: Some("Assign Source Profile..."),
        }),
        SourceProfileState::Missing => findings.push(PreflightFinding {
            code: PreflightCode::MissingSourceProfile,
            severity: PreflightSeverity::Blocking,
            title: "Source ICC profile required",
            detail: "This source color model has no safe default production interpretation. Assign or confirm its Source ICC before conversion.".to_owned(),
            action: Some("Assign Source Profile..."),
        }),
        SourceProfileState::Invalid(reason) => findings.push(PreflightFinding {
            code: PreflightCode::InvalidSourceProfile,
            severity: PreflightSeverity::Blocking,
            title: "Invalid source ICC profile",
            detail: reason.clone(),
            action: Some("Assign Source Profile..."),
        }),
        SourceProfileState::Embedded(identity) if is_srgb_fallback_identity(identity) => findings.push(PreflightFinding {
            code: PreflightCode::MissingSourceProfile,
            severity: PreflightSeverity::Warning,
            title: "No Source ICC — using reproducible sRGB fallback",
            detail: "This RGB artwork is untagged. Shade Editor will interpret it as sRGB for this conversion and records the exact fallback ICC identity in the recipe. Assign a Source ICC if another RGB encoding is intended.".to_owned(),
            action: Some("Assign Source Profile..."),
        }),
        SourceProfileState::Embedded(identity) | SourceProfileState::Assigned(identity)
            if identity.sha256.trim().is_empty() =>
        {
            findings.push(PreflightFinding {
                code: PreflightCode::InvalidSourceProfile,
                severity: PreflightSeverity::Blocking,
                title: "Source ICC identity is incomplete",
                detail: "The selected source ICC profile has no stable SHA-256 identity and cannot be captured in a reproducible conversion recipe.".to_owned(),
                action: Some("Reassign Source Profile..."),
            });
        }
        SourceProfileState::Embedded(_) | SourceProfileState::Assigned(_) => {}
    }''',
)
replace_once(
    "src/conversion_preflight.rs",
    '''    #[test]
    fn missing_profile_blocks_production_conversion() {
        let mut input = ready_rgb();
        input.profile = SourceProfileState::Missing;
        let report = build_conversion_preflight(&input);
        assert!(report.contains(PreflightCode::MissingSourceProfile));
        assert!(!report.can_convert());
    }
''',
    '''    #[test]
    fn missing_rgb_profile_warns_but_does_not_block_conversion() {
        let mut input = ready_rgb();
        input.profile = SourceProfileState::Missing;
        let report = build_conversion_preflight(&input);
        assert!(report.contains(PreflightCode::MissingSourceProfile));
        assert!(report.can_convert());
        assert_eq!(report.highest_severity(), Some(PreflightSeverity::Warning));
    }

    #[test]
    fn missing_cmyk_profile_remains_blocking() {
        let mut input = ready_rgb();
        input.color_model = ColorModel::Cmyk;
        input.profile = SourceProfileState::Missing;
        let report = build_conversion_preflight(&input);
        assert!(report.contains(PreflightCode::MissingSourceProfile));
        assert!(!report.can_convert());
    }

    #[test]
    fn explicit_rgb_fallback_identity_is_reported_as_warning() {
        let mut input = ready_rgb();
        input.profile = SourceProfileState::Embedded(
            crate::source_profile_fallback::srgb_fallback_identity().unwrap(),
        );
        let report = build_conversion_preflight(&input);
        assert!(report.contains(PreflightCode::MissingSourceProfile));
        assert!(report.can_convert());
    }
''',
)

# Conversion worker: Embedded means real embedded bytes unless the immutable recipe explicitly
# carries the built-in sRGB fallback identity.
replace_once(
    "src/icc_conversion_worker.rs",
    '''use crate::source_transparency::{SourceTransparencyPolicy, composite_rgb_u16};
use crate::tiff_io::{self, ColorModel, StreamInfo};''',
    '''use crate::source_profile_fallback::{is_srgb_fallback_identity, srgb_fallback_icc};
use crate::source_transparency::{SourceTransparencyPolicy, composite_rgb_u16};
use crate::tiff_io::{self, ColorModel, StreamInfo};''',
)
replace_once(
    "src/icc_conversion_worker.rs",
    '''        CapturedSourceProfile::Embedded => embedded_icc
            .map(|bytes| bytes.to_vec())
            .ok_or_else(|| {
                "Captured source expects an embedded ICC, but the decoded source has none."
                    .to_owned()
            })?,''',
    '''        CapturedSourceProfile::Embedded => match embedded_icc {
            Some(bytes) => bytes.to_vec(),
            None if is_srgb_fallback_identity(&capture.conversion_recipe.source_profile_identity) => {
                srgb_fallback_icc()?
            }
            None => {
                return Err(
                    "Captured source expects an embedded ICC, but the decoded source has none."
                        .to_owned(),
                );
            }
        },''',
)

# Candidate domain runtime uses the same fallback bytes and still verifies the recipe hash.
replace_once(
    "src/conversion_candidate_preview.rs",
    '''use crate::nchannel_icc::ProductionNChannelTransform;''',
    '''use crate::nchannel_icc::ProductionNChannelTransform;
use crate::source_profile_fallback::srgb_fallback_icc;''',
)
replace_once(
    "src/conversion_candidate_preview.rs",
    '''        CapturedSourceProfile::Embedded => embedded
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                "Candidate expects an embedded Source ICC, but the preview source has none."
                    .to_owned()
            })?,''',
    '''        CapturedSourceProfile::Embedded => match embedded {
            Some(bytes) => bytes.to_vec(),
            None => srgb_fallback_icc()?,
        },''',
)

# Candidate UI: do not build adjusted raster on every egui repaint. The raster is built once
# only when the debounced immutable key actually starts a worker, then the result is cached.
replace_once(
    "src/ui/conversion_candidate_preview.rs",
    '''    width: usize,
    height: usize,
    adjusted_planes: Vec<Vec<u16>>,
}''',
    '''    width: usize,
    height: usize,
}''',
)
replace_once(
    "src/ui/conversion_candidate_preview.rs",
    '''        let adjusted_planes = render::adjusted_planes(face.preview.as_ref(), &self.project);
        let face_label = face_ref''',
    '''        let face_label = face_ref''',
)
replace_once(
    "src/ui/conversion_candidate_preview.rs",
    '''            embedded_source_icc,
            width: face.preview.width(),
            height: face.preview.height(),
            adjusted_planes,
        })''',
    '''            embedded_source_icc,
            width: face.preview.width(),
            height: face.preview.height(),
        })''',
)
replace_once(
    "src/ui/conversion_candidate_preview.rs",
    '''        let mut removed_stale_active = false;
        let mut start = false;''',
    '''        let mut removed_stale_active = false;
        let mut start = false;
        let mut keep_polling = false;''',
)
replace_once(
    "src/ui/conversion_candidate_preview.rs",
    '''                state.debounce_started = None;
                start = true;
            }
        });''',
    '''                state.debounce_started = None;
                start = true;
            }
            keep_polling = state.pending.is_some() || state.debounce_started.is_some();
        });''',
)
replace_once(
    "src/ui/conversion_candidate_preview.rs",
    '''        if start {
            self.start_candidate_preview(key, recipe, ctx);
        } else {
            ctx.request_repaint_after(Duration::from_millis(40));
        }''',
    '''        if start {
            self.start_candidate_preview(key, recipe, ctx);
        } else if keep_polling {
            ctx.request_repaint_after(Duration::from_millis(40));
        }''',
)
replace_once(
    "src/ui/conversion_candidate_preview.rs",
    '''        let cancellation = ConversionCancellation::default();
        let worker_cancel = cancellation.clone();
        let input = CandidatePreviewInput {
            width: source.width,
            height: source.height,
            source_model: source.source_model,
            source_planes: source.adjusted_planes,
            source_profile: source.captured_profile,
            embedded_source_icc: source.embedded_source_icc,
            recipe,
        };''',
    '''        let adjusted_planes = match self.faces.get(source.face_index) {
            Some(face) => render::adjusted_planes(face.preview.as_ref(), &self.project),
            None => {
                CANDIDATE.with(|cell| {
                    cell.borrow_mut().error = Some("Candidate Source Face disappeared before rendering.".to_owned())
                });
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
        };''',
)

# Candidate Preview must disclose the fallback directly in its own window.
replace_once(
    "src/ui/conversion_candidate_preview.rs",
    '''                        } else {
                            ui.label(
                                egui::RichText::new("Saved Source state ready")
                                    .color(egui::Color32::LIGHT_GREEN),
                            );
                        }
                    }''',
    '''                        } else {
                            ui.label(
                                egui::RichText::new("Saved Source state ready")
                                    .color(egui::Color32::LIGHT_GREEN),
                            );
                        }
                        if windows_shade_editor::source_profile_fallback::is_srgb_fallback_identity(&source.profile_identity) {
                            ui.label(
                                egui::RichText::new(
                                    "Warning: this RGB Face has no Source ICC. Candidate and final conversion use the reproducible sRGB fallback.",
                                )
                                .color(egui::Color32::YELLOW),
                            );
                        }
                    }''',
)

# Add a regression contract for the no-rerender loop.
replace_once(
    "src/ui/conversion_candidate_preview.rs",
    '''    #[test]
    fn solo_candidate_uses_direct_ink_coverage_polarity() {''',
    '''    #[test]
    fn candidate_raster_is_built_only_when_a_new_worker_starts() {
        let source = include_str!("conversion_candidate_preview.rs");
        let candidate_source = source
            .split("    fn candidate_source(&self)")
            .nth(1)
            .and_then(|section| section.split("    fn observe_candidate(").next())
            .unwrap();
        assert!(!candidate_source.contains("render::adjusted_planes"));
        let starter = source
            .split("    fn start_candidate_preview(")
            .nth(1)
            .and_then(|section| section.split("    fn poll_candidate_preview(").next())
            .unwrap();
        assert!(starter.contains("render::adjusted_planes"));
        assert!(source.contains("else if keep_polling"));
    }

    #[test]
    fn solo_candidate_uses_direct_ink_coverage_polarity() {''',
)

# Batch gets an explicit heterogeneous-profile warning but continues; recipes remain per Face.
replace_once(
    "src/ui/conversion_batch.rs",
    '''                for inspection in &inspections {
                    render_batch_face_preflight(ui, inspection, &mut config);
                }

                ui.add_space(8.0);''',
    '''                for inspection in &inspections {
                    render_batch_face_preflight(ui, inspection, &mut config);
                }
                render_batch_source_profile_consistency(ui, &inspections);

                ui.add_space(8.0);''',
)
replace_once(
    "src/ui/conversion_batch.rs",
    '''fn build_batch_plan_preview(
    app: &ShadeApp,''',
    '''fn render_batch_source_profile_consistency(
    ui: &mut egui::Ui,
    inspections: &[BatchFaceInspection],
) {
    let mut groups = BTreeMap::<String, (String, Vec<String>)>::new();
    for inspection in inspections {
        let Some(identity) = inspection.profile_identity.as_ref() else {
            continue;
        };
        let key = identity.sha256.trim().to_ascii_lowercase();
        let entry = groups
            .entry(key)
            .or_insert_with(|| (identity.description.clone(), Vec::new()));
        entry.1.push(format!("Face {} ({})", inspection.index + 1, inspection.label));
    }
    if groups.len() <= 1 {
        return;
    }
    ui.group(|ui| {
        ui.label(
            egui::RichText::new("Warning: selected Faces use different Source ICC interpretations")
                .color(egui::Color32::YELLOW)
                .strong(),
        );
        ui.small(
            "Batch conversion is allowed. Each Face keeps its own captured Source ICC or sRGB fallback; profiles are not forced to match the first Face.",
        );
        for (_hash, (description, faces)) in groups {
            ui.small(format!("{description}: {}", faces.join(", ")));
        }
    });
}

fn build_batch_plan_preview(
    app: &ShadeApp,''',
)

# Remove the one-shot patch machinery from the resulting commit.
Path("scripts/issue370_patch.py").unlink(missing_ok=True)
Path(".github/workflows/codex-issue-370-patch.yml").unlink(missing_ok=True)
print("removed temporary patch machinery")
