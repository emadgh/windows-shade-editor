use std::path::{Path, PathBuf};

use crate::color_conversion::{ProductionProvenance, ProjectRole};
use crate::model::ShadeProject;
use crate::production_project_compat::ProductionCompatibilityKey;
use crate::reconversion_policy::{ReplacementRisk, analyze_replacement_risk};

#[derive(Clone, Debug)]
pub struct ProductionReplacementPlan {
    pub production_project_path: PathBuf,
    pub face_index: usize,
    pub previous_provenance: ProductionProvenance,
    pub replacement_provenance: ProductionProvenance,
    pub target_compatibility: ProductionCompatibilityKey,
    pub risk: ReplacementRisk,
}

/// Prepare an immutable, auditable replacement plan without mutating the
/// Production project. The eventual transaction must persist both provenance
/// records before replacing the selected Face; this planner deliberately keeps
/// the previous record available so replacement cannot erase lineage by
/// construction.
pub fn prepare_production_replacement_plan(
    production_project: &ShadeProject,
    production_project_path: &Path,
    selected_previous: &ProductionProvenance,
    replacement: ProductionProvenance,
) -> Result<ProductionReplacementPlan, String> {
    if production_project.project_role != ProjectRole::Production {
        return Err("Replacement destination is not a Production project.".to_owned());
    }
    if production_project.faces.len() != production_project.production_provenance.len() {
        return Err(
            "Production Face/provenance pairing is inconsistent; replacement is blocked."
                .to_owned(),
        );
    }

    let risk = analyze_replacement_risk(
        production_project,
        production_project_path,
        selected_previous,
    )?;
    let previous_key = ProductionCompatibilityKey::from_provenance(selected_previous)?;
    let replacement_key = ProductionCompatibilityKey::from_provenance(&replacement)?;
    if replacement_key != previous_key {
        return Err(
            "Replacement conversion target differs from the selected Production target; create a new Production version instead."
                .to_owned(),
        );
    }

    if !paths_match_str(
        &selected_previous.source.source_project_path,
        &replacement.source.source_project_path,
    ) || !paths_match_str(
        &selected_previous.source.source_face_path,
        &replacement.source.source_face_path,
    ) {
        return Err(
            "Replacement provenance belongs to a different Source project or Face; replacement is blocked."
                .to_owned(),
        );
    }

    let replacement_output = Path::new(&replacement.output_path);
    let resolved_faces = production_project.resolve_face_paths(production_project_path);
    if resolved_faces
        .iter()
        .enumerate()
        .any(|(index, path)| index != risk.face_index && paths_match(path, replacement_output))
    {
        return Err(
            "Replacement output path is already owned by another Production Face.".to_owned(),
        );
    }

    Ok(ProductionReplacementPlan {
        production_project_path: production_project_path.to_path_buf(),
        face_index: risk.face_index,
        previous_provenance: selected_previous.clone(),
        replacement_provenance: replacement,
        target_compatibility: previous_key,
        risk,
    })
}

fn paths_match(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

fn paths_match_str(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
        ConversionRenderingIntent, ConversionSourceRef, ConversionTargetDefinition,
        SeparationStrategy, TargetChannelDefinition,
    };
    use crate::model::{IccProfileIdentity, Levels};
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
    fn same_target_and_source_prepare_an_auditable_plan() {
        let project_path = Path::new(r"C:\Production\Job.shade");
        let old_output = Path::new(r"C:\Production\Face.tif");
        let new_output = Path::new(r"C:\Production\Face-v2.tif");
        let project = project(old_output);
        let previous = project.production_provenance[0].clone();
        let mut replacement = provenance(new_output);
        replacement.source.source_file_sha256 = hash('n');
        replacement.converted_at_unix_ms = 20;

        let plan = prepare_production_replacement_plan(
            &project,
            project_path,
            &previous,
            replacement.clone(),
        )
        .unwrap();

        assert_eq!(plan.face_index, 0);
        assert_eq!(plan.previous_provenance.output_path, previous.output_path);
        assert_eq!(plan.replacement_provenance.output_path, replacement.output_path);
        assert!(!plan.risk.requires_explicit_confirmation);
    }

    #[test]
    fn target_drift_forces_new_version_instead_of_replacement() {
        let project_path = Path::new(r"C:\Production\Job.shade");
        let old_output = Path::new(r"C:\Production\Face.tif");
        let project = project(old_output);
        let previous = project.production_provenance[0].clone();
        let mut replacement = provenance(Path::new(r"C:\Production\Face-v2.tif"));
        replacement.recipe.target.bit_depth = 8;

        let error = prepare_production_replacement_plan(
            &project,
            project_path,
            &previous,
            replacement,
        )
        .expect_err("target drift must be blocked");
        assert!(error.contains("target differs"));
    }

    #[test]
    fn source_face_drift_is_not_a_replacement() {
        let project_path = Path::new(r"C:\Production\Job.shade");
        let old_output = Path::new(r"C:\Production\Face.tif");
        let project = project(old_output);
        let previous = project.production_provenance[0].clone();
        let mut replacement = provenance(Path::new(r"C:\Production\Other-v2.tif"));
        replacement.source.source_face_path = r"C:\Design\Other.tif".to_owned();

        let error = prepare_production_replacement_plan(
            &project,
            project_path,
            &previous,
            replacement,
        )
        .expect_err("different Source Face must be blocked");
        assert!(error.contains("different Source"));
    }

    #[test]
    fn production_adjustments_are_carried_into_plan_risk() {
        let project_path = Path::new(r"C:\Production\Job.shade");
        let old_output = Path::new(r"C:\Production\Face.tif");
        let mut project = project(old_output);
        project.adjustments.get_mut("Black").unwrap().levels = Levels {
            gamma: 0.9,
            ..Levels::default()
        };
        let previous = project.production_provenance[0].clone();
        let replacement = provenance(Path::new(r"C:\Production\Face-v2.tif"));

        let plan = prepare_production_replacement_plan(
            &project,
            project_path,
            &previous,
            replacement,
        )
        .unwrap();
        assert!(plan.risk.requires_explicit_confirmation);
        assert!(plan.risk.production_adjustments_modified);
    }
}
