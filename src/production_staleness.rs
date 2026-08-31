use std::path::{Path, PathBuf};

use crate::color_conversion::ProductionProvenance;
use crate::icc_conversion_worker::sha256_file;
use crate::production_project_compat::ProductionCompatibilityKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceConversionStatus {
    NoPriorConversion,
    UpToDate,
    SourceChanged,
    ProductionOutputMissing,
    ProductionOutputChanged,
    ProductionLineageAmbiguous,
    TargetNoLongerCompatible,
}

impl SourceConversionStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::NoPriorConversion => "Not converted yet",
            Self::UpToDate => "Up to date",
            Self::SourceChanged => "Source changed since conversion",
            Self::ProductionOutputMissing => "Production output missing",
            Self::ProductionOutputChanged => "Production output changed or unreadable",
            Self::ProductionLineageAmbiguous => "Production lineage requires selection",
            Self::TargetNoLongerCompatible => "Target no longer compatible",
        }
    }
}

#[derive(Clone, Debug)]
pub struct CurrentSourceIdentity<'a> {
    pub source_project_path: &'a Path,
    pub source_face_path: &'a Path,
    pub source_snapshot_id: Option<u64>,
    pub source_file_sha256: &'a str,
}

#[derive(Clone, Debug)]
pub struct SourceConversionAssessment<'a> {
    pub status: SourceConversionStatus,
    pub matching_provenance: Vec<&'a ProductionProvenance>,
    pub selected_provenance: Option<&'a ProductionProvenance>,
    pub diagnostic: String,
}

/// Assess saved Source identity against persisted Production provenance without
/// using timestamps or filenames alone. Multiple prior conversions are kept
/// explicit so the operator can choose which lineage/version to compare or
/// replace later.
pub fn assess_source_conversion<'a>(
    current: &CurrentSourceIdentity<'_>,
    provenances: &'a [ProductionProvenance],
    requested_target: Option<&ProductionCompatibilityKey>,
) -> SourceConversionAssessment<'a> {
    let matches = provenances
        .iter()
        .filter(|provenance| {
            paths_match(
                Path::new(&provenance.source.source_project_path),
                current.source_project_path,
            ) && paths_match(
                Path::new(&provenance.source.source_face_path),
                current.source_face_path,
            )
        })
        .collect::<Vec<_>>();

    if matches.is_empty() {
        return SourceConversionAssessment {
            status: SourceConversionStatus::NoPriorConversion,
            matching_provenance: matches,
            selected_provenance: None,
            diagnostic: "No Production provenance references this saved Source Face.".to_owned(),
        };
    }

    if matches.len() > 1 {
        return SourceConversionAssessment {
            status: SourceConversionStatus::ProductionLineageAmbiguous,
            matching_provenance: matches,
            selected_provenance: None,
            diagnostic: "Multiple Production conversions reference this Source Face; select the intended prior output/version before re-conversion or replacement."
                .to_owned(),
        };
    }

    let provenance = matches[0];
    if let Some(requested_target) = requested_target {
        match ProductionCompatibilityKey::from_provenance(provenance) {
            Ok(converted_target) if &converted_target == requested_target => {}
            Ok(_) => {
                return SourceConversionAssessment {
                    status: SourceConversionStatus::TargetNoLongerCompatible,
                    matching_provenance: matches,
                    selected_provenance: Some(provenance),
                    diagnostic: "The requested Production target differs from the target recorded by the prior conversion. Append/replacement is blocked; create a new Production conversion instead."
                        .to_owned(),
                };
            }
            Err(error) => {
                return SourceConversionAssessment {
                    status: SourceConversionStatus::TargetNoLongerCompatible,
                    matching_provenance: matches,
                    selected_provenance: Some(provenance),
                    diagnostic: format!(
                        "Prior Production target provenance is no longer valid: {error}"
                    ),
                };
            }
        }
    }

    if !provenance
        .source
        .source_file_sha256
        .eq_ignore_ascii_case(current.source_file_sha256.trim())
        || provenance.source.source_snapshot_id != current.source_snapshot_id
    {
        return SourceConversionAssessment {
            status: SourceConversionStatus::SourceChanged,
            matching_provenance: matches,
            selected_provenance: Some(provenance),
            diagnostic: describe_source_change(current, provenance),
        };
    }

    let output_path = PathBuf::from(&provenance.output_path);
    if !output_path.exists() {
        return SourceConversionAssessment {
            status: SourceConversionStatus::ProductionOutputMissing,
            matching_provenance: matches,
            selected_provenance: Some(provenance),
            diagnostic: format!(
                "Recorded Production output is missing: {}",
                output_path.display()
            ),
        };
    }

    match sha256_file(&output_path) {
        Ok(actual) if actual.eq_ignore_ascii_case(provenance.output_sha256.trim()) => {
            SourceConversionAssessment {
                status: SourceConversionStatus::UpToDate,
                matching_provenance: matches,
                selected_provenance: Some(provenance),
                diagnostic: "Saved Source identity and recorded Production output match the immutable conversion provenance."
                    .to_owned(),
            }
        }
        Ok(_) => SourceConversionAssessment {
            status: SourceConversionStatus::ProductionOutputChanged,
            matching_provenance: matches,
            selected_provenance: Some(provenance),
            diagnostic: "Recorded Production TIFF bytes no longer match their provenance SHA-256."
                .to_owned(),
        },
        Err(error) => SourceConversionAssessment {
            status: SourceConversionStatus::ProductionOutputChanged,
            matching_provenance: matches,
            selected_provenance: Some(provenance),
            diagnostic: format!("Cannot verify recorded Production TIFF: {error}"),
        },
    }
}

/// Assess one explicitly selected prior conversion when the same Source Face has
/// multiple historical Production outputs. This is the primitive used by the
/// future version/replacement UI after the operator resolves ambiguity.
pub fn assess_selected_provenance<'a>(
    current: &CurrentSourceIdentity<'_>,
    provenance: &'a ProductionProvenance,
    requested_target: Option<&ProductionCompatibilityKey>,
) -> SourceConversionAssessment<'a> {
    if !paths_match(
        Path::new(&provenance.source.source_project_path),
        current.source_project_path,
    ) || !paths_match(
        Path::new(&provenance.source.source_face_path),
        current.source_face_path,
    ) {
        return SourceConversionAssessment {
            status: SourceConversionStatus::NoPriorConversion,
            matching_provenance: Vec::new(),
            selected_provenance: None,
            diagnostic: "Selected Production provenance does not belong to this Source Face."
                .to_owned(),
        };
    }
    assess_source_conversion(current, std::slice::from_ref(provenance), requested_target)
}

fn describe_source_change(
    current: &CurrentSourceIdentity<'_>,
    provenance: &ProductionProvenance,
) -> String {
    let artwork_changed = !provenance
        .source
        .source_file_sha256
        .eq_ignore_ascii_case(current.source_file_sha256.trim());
    let snapshot_changed = provenance.source.source_snapshot_id != current.source_snapshot_id;
    match (artwork_changed, snapshot_changed) {
        (true, true) => "Source artwork bytes and saved Snapshot/state differ from the prior conversion."
            .to_owned(),
        (true, false) => "Source artwork bytes differ from the prior conversion.".to_owned(),
        (false, true) => "Saved Source Snapshot/state differs from the prior conversion.".to_owned(),
        (false, false) => "Source identity matches the prior conversion.".to_owned(),
    }
}

fn paths_match(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
        ConversionRenderingIntent, ConversionSourceRef, ConversionTargetDefinition,
        SeparationStrategy, TargetChannelDefinition,
    };
    use crate::model::IccProfileIdentity;

    fn hash(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn temp_output(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shade-staleness-{label}-{}-{}.tif",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn provenance(output: &Path, output_sha256: String) -> ProductionProvenance {
        ProductionProvenance {
            source: ConversionSourceRef {
                source_project_path: r"C:\Design\Source.shade".to_owned(),
                source_face_path: r"C:\Design\Face.tif".to_owned(),
                source_snapshot_id: Some(7),
                source_file_sha256: hash('s'),
            },
            recipe: ConversionRecipe {
                source_transparency_policy: None,
                schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
                engine_mode: ConversionEngineMode::Icc,
                source_profile_identity: IccProfileIdentity {
                    description: "Source".to_owned(),
                    sha256: hash('a'),
                },
                target: ConversionTargetDefinition {
                    name: "Press".to_owned(),
                    channels: ["Cyan", "Magenta", "Yellow", "Black"]
                        .into_iter()
                        .map(|name| TargetChannelDefinition {
                            name: name.to_owned(),
                            display_rgb: None,
                            solidity: 1.0,
                            max_coverage: None,
                        })
                        .collect(),
                    bit_depth: 16,
                    output_profile_identity: Some(IccProfileIdentity {
                        description: "Press".to_owned(),
                        sha256: hash('b'),
                    }),
                    output_profile_path: Some(r"C:\Color\Press.icc".to_owned()),
                    device_link_identity: None,
                    device_link_path: None,
                    characterization_id: None,
                    total_ink_limit: None,
                },
                rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
                black_point_compensation: true,
                strategy: SeparationStrategy::default(),
                custom_optimizer_solver: None,
            },
            custom_optimizer: None,
            profile_backed_optimizer: None,
            output_path: output.to_string_lossy().into_owned(),
            output_sha256,
            converted_at_unix_ms: 1,
        }
    }

    fn current<'a>(hash: &'a str, snapshot: Option<u64>) -> CurrentSourceIdentity<'a> {
        CurrentSourceIdentity {
            source_project_path: Path::new(r"C:\Design\Source.shade"),
            source_face_path: Path::new(r"C:\Design\Face.tif"),
            source_snapshot_id: snapshot,
            source_file_sha256: hash,
        }
    }

    #[test]
    fn unchanged_saved_source_and_output_are_up_to_date() {
        let output = temp_output("current");
        fs::write(&output, b"production bytes").unwrap();
        let output_sha = sha256_file(&output).unwrap();
        let provenance = provenance(&output, output_sha);
        let source_hash = hash('s');
        let assessment = assess_source_conversion(
            &current(&source_hash, Some(7)),
            std::slice::from_ref(&provenance),
            None,
        );
        assert_eq!(assessment.status, SourceConversionStatus::UpToDate);
        let _ = fs::remove_file(output);
    }

    #[test]
    fn artwork_or_snapshot_change_is_stale_before_output_checks() {
        let missing_output = temp_output("missing-but-source-changed");
        let provenance = provenance(&missing_output, hash('o'));
        let changed_hash = hash('x');
        let artwork = assess_source_conversion(
            &current(&changed_hash, Some(7)),
            std::slice::from_ref(&provenance),
            None,
        );
        assert_eq!(artwork.status, SourceConversionStatus::SourceChanged);
        let source_hash = hash('s');
        let snapshot = assess_source_conversion(
            &current(&source_hash, Some(8)),
            std::slice::from_ref(&provenance),
            None,
        );
        assert_eq!(snapshot.status, SourceConversionStatus::SourceChanged);
    }

    #[test]
    fn multiple_prior_outputs_require_explicit_lineage_selection() {
        let output_a = temp_output("a");
        let output_b = temp_output("b");
        let provenances = vec![
            provenance(&output_a, hash('a')),
            provenance(&output_b, hash('b')),
        ];
        let source_hash = hash('s');
        let assessment = assess_source_conversion(
            &current(&source_hash, Some(7)),
            &provenances,
            None,
        );
        assert_eq!(
            assessment.status,
            SourceConversionStatus::ProductionLineageAmbiguous
        );
        assert_eq!(assessment.matching_provenance.len(), 2);
    }

    #[test]
    fn missing_or_modified_production_output_is_reported() {
        let output = temp_output("missing");
        let provenance_missing = provenance(&output, hash('o'));
        let source_hash = hash('s');
        let missing = assess_source_conversion(
            &current(&source_hash, Some(7)),
            std::slice::from_ref(&provenance_missing),
            None,
        );
        assert_eq!(missing.status, SourceConversionStatus::ProductionOutputMissing);

        fs::write(&output, b"changed production bytes").unwrap();
        let changed = assess_source_conversion(
            &current(&source_hash, Some(7)),
            std::slice::from_ref(&provenance_missing),
            None,
        );
        assert_eq!(changed.status, SourceConversionStatus::ProductionOutputChanged);
        let _ = fs::remove_file(output);
    }

    #[test]
    fn requested_target_mismatch_blocks_existing_lineage() {
        let output = temp_output("target");
        fs::write(&output, b"production").unwrap();
        let output_sha = sha256_file(&output).unwrap();
        let provenance = provenance(&output, output_sha);
        let mut requested = ProductionCompatibilityKey::from_provenance(&provenance).unwrap();
        requested.bit_depth = 8;
        let source_hash = hash('s');
        let assessment = assess_source_conversion(
            &current(&source_hash, Some(7)),
            std::slice::from_ref(&provenance),
            Some(&requested),
        );
        assert_eq!(
            assessment.status,
            SourceConversionStatus::TargetNoLongerCompatible
        );
        let _ = fs::remove_file(output);
    }
}
