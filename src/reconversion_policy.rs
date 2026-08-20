use std::path::Path;

use crate::color_conversion::ProductionProvenance;
use crate::model::{ChannelAdjustment, ShadeProject, MASTER_ADJUSTMENT_KEY};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReconversionMode {
    #[default]
    NewVersion,
    TransactionalReplace,
}

impl ReconversionMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::NewVersion => "Create new versioned output",
            Self::TransactionalReplace => "Replace prior Production output transactionally",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplacementRisk {
    pub face_index: usize,
    pub production_adjustments_modified: bool,
    pub production_snapshot_count: usize,
    pub requires_explicit_confirmation: bool,
    pub warning: Option<String>,
}

/// Analyze whether replacing one prior Production output would discard or
/// reinterpret production-side work. Replacement is never the default action;
/// even a clean Face must be explicitly selected by the operator.
pub fn analyze_replacement_risk(
    production_project: &ShadeProject,
    production_project_path: &Path,
    provenance: &ProductionProvenance,
) -> Result<ReplacementRisk, String> {
    let resolved_faces = production_project.resolve_face_paths(production_project_path);
    let face_index = resolved_faces
        .iter()
        .position(|path| paths_match(path, Path::new(&provenance.output_path)))
        .ok_or_else(|| {
            "Selected Production provenance does not match any Face in the Production project."
                .to_owned()
        })?;
    if production_project
        .production_provenance
        .get(face_index)
        .is_none_or(|stored| !same_output_identity(stored, provenance))
    {
        return Err(
            "Selected Production Face/provenance pairing is inconsistent; replacement is blocked."
                .to_owned(),
        );
    }

    let production_adjustments_modified =
        production_adjustments_are_modified(production_project, provenance);
    let production_snapshot_count = production_project.snapshots.len();
    let requires_explicit_confirmation =
        production_adjustments_modified || production_snapshot_count > 0;
    let warning = requires_explicit_confirmation.then(|| {
        let mut parts = Vec::new();
        if production_adjustments_modified {
            parts.push("Production Levels/Curves/Mixer adjustments are non-default".to_owned());
        }
        if production_snapshot_count > 0 {
            parts.push(format!(
                "the Production project contains {production_snapshot_count} Snapshot(s)"
            ));
        }
        format!(
            "Transactional replacement can invalidate production-side work: {}. Create a new version unless replacement is intentional.",
            parts.join("; ")
        )
    });

    Ok(ReplacementRisk {
        face_index,
        production_adjustments_modified,
        production_snapshot_count,
        requires_explicit_confirmation,
        warning,
    })
}

pub fn safe_default_reconversion_mode() -> ReconversionMode {
    ReconversionMode::NewVersion
}

/// Production projects seed each target channel's mixer with an explicit
/// identity matrix. Comparing those rows with `ChannelAdjustment::default()`
/// would therefore classify a freshly-created, untouched Production project as
/// modified. Compare the persisted adjustment state with the canonical target
/// identity instead, while treating unknown rows/coefficients as production
/// work that replacement must not silently discard.
fn production_adjustments_are_modified(
    project: &ShadeProject,
    provenance: &ProductionProvenance,
) -> bool {
    let channels = provenance
        .recipe
        .target
        .channels
        .iter()
        .map(|channel| channel.name.as_str())
        .collect::<Vec<_>>();

    project.adjustments.iter().any(|(output, adjustment)| {
        if output == MASTER_ADJUSTMENT_KEY {
            return adjustment != &ChannelAdjustment::default();
        }
        if !channels.iter().any(|channel| *channel == output) {
            return true;
        }
        if !adjustment.enabled
            || adjustment.levels != Default::default()
            || adjustment.curve != Default::default()
            || adjustment.mixer.constant != 0.0
            || adjustment.mixer.coefficients.len() != channels.len()
        {
            return true;
        }

        channels.iter().any(|input| {
            let expected = if *input == output { 1.0 } else { 0.0 };
            adjustment
                .mixer
                .coefficients
                .get(*input)
                .is_none_or(|value| *value != expected)
        })
    })
}

fn same_output_identity(left: &ProductionProvenance, right: &ProductionProvenance) -> bool {
    left.output_path
        .trim()
        .eq_ignore_ascii_case(right.output_path.trim())
        && left
            .output_sha256
            .trim()
            .eq_ignore_ascii_case(right.output_sha256.trim())
        && left.converted_at_unix_ms == right.converted_at_unix_ms
}

fn paths_match(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
        ConversionRenderingIntent, ConversionSourceRef, ConversionTargetDefinition,
        SeparationStrategy, TargetChannelDefinition,
    };
    use crate::model::{AdjustmentSnapshot, IccProfileIdentity, Levels};
    use crate::production_project::{ProductionProjectSpec, build_production_project};

    fn hash(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn provenance(output: &Path) -> ProductionProvenance {
        ProductionProvenance {
            source: ConversionSourceRef {
                source_project_path: r"C:\Design\Source.shade".to_owned(),
                source_face_path: r"C:\Design\Face.tif".to_owned(),
                source_snapshot_id: Some(1),
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
                        sha256: hash('p'),
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
            output_path: output.to_string_lossy().into_owned(),
            output_sha256: hash('o'),
            converted_at_unix_ms: 10,
        }
    }

    fn project(output: &Path) -> ShadeProject {
        build_production_project(ProductionProjectSpec {
            project_name: "Production",
            source_project_path: Path::new(r"C:\Design\Source.shade"),
            output_tiff_path: output,
            output_face_label: "Face 1",
            provenance: provenance(output),
        })
        .unwrap()
    }

    #[test]
    fn new_version_is_always_the_safe_default() {
        assert_eq!(safe_default_reconversion_mode(), ReconversionMode::NewVersion);
    }

    #[test]
    fn clean_production_face_has_no_discard_warning() {
        let project_path = PathBuf::from(r"C:\Production\Job.shade");
        let output = PathBuf::from(r"C:\Production\Face.tif");
        let project = project(&output);
        let risk = analyze_replacement_risk(
            &project,
            &project_path,
            &project.production_provenance[0],
        )
        .unwrap();
        assert!(!risk.requires_explicit_confirmation);
        assert!(risk.warning.is_none());
    }

    #[test]
    fn production_adjustments_force_explicit_replacement_warning() {
        let project_path = PathBuf::from(r"C:\Production\Job.shade");
        let output = PathBuf::from(r"C:\Production\Face.tif");
        let mut project = project(&output);
        project.adjustments.get_mut("Black").unwrap().levels = Levels {
            gamma: 0.9,
            ..Levels::default()
        };
        let risk = analyze_replacement_risk(
            &project,
            &project_path,
            &project.production_provenance[0],
        )
        .unwrap();
        assert!(risk.production_adjustments_modified);
        assert!(risk.requires_explicit_confirmation);
        assert!(risk.warning.unwrap().contains("Levels/Curves/Mixer"));
    }

    #[test]
    fn production_snapshots_force_explicit_replacement_warning() {
        let project_path = PathBuf::from(r"C:\Production\Job.shade");
        let output = PathBuf::from(r"C:\Production\Face.tif");
        let mut project = project(&output);
        project.snapshots.push(AdjustmentSnapshot {
            id: 1,
            name: "Production tweak".to_owned(),
            created_at_unix_ms: 1,
            adjustments: project.adjustments.clone(),
            exports: Vec::new(),
            history: Default::default(),
        });
        let risk = analyze_replacement_risk(
            &project,
            &project_path,
            &project.production_provenance[0],
        )
        .unwrap();
        assert_eq!(risk.production_snapshot_count, 1);
        assert!(risk.requires_explicit_confirmation);
        assert!(risk.warning.unwrap().contains("Snapshot"));
    }
}
