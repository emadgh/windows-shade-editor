use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::color_conversion::{ProductionProvenance, ProjectRole};
use crate::model::ShadeProject;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkedProductionLineageStatus {
    Ready,
    Missing,
    Unreadable,
    InvalidRole,
    WrongSource,
    InvalidProvenance,
}

#[derive(Clone, Debug)]
pub struct LinkedProductionLineage {
    pub project_path: PathBuf,
    pub project_name: Option<String>,
    pub status: LinkedProductionLineageStatus,
    pub provenances: Vec<ProductionProvenance>,
    pub diagnostic: Option<String>,
}

pub fn collect_linked_production_lineage(
    source_project: &ShadeProject,
    source_project_path: &Path,
) -> Vec<LinkedProductionLineage> {
    let mut seen = BTreeSet::new();
    source_project
        .linked_projects
        .iter()
        .filter(|link| link.role == ProjectRole::Production)
        .filter_map(|link| {
            let path = resolve_linked_path(&link.path, source_project_path);
            let key = path.to_string_lossy().to_ascii_lowercase();
            seen.insert(key)
                .then(|| inspect_lineage(path, source_project_path))
        })
        .collect()
}

pub fn provenance_for_source_face(
    lineages: &[LinkedProductionLineage],
    source_project_path: &Path,
    source_face_path: &Path,
) -> Vec<ProductionProvenance> {
    lineages
        .iter()
        .filter(|lineage| lineage.status == LinkedProductionLineageStatus::Ready)
        .flat_map(|lineage| lineage.provenances.iter())
        .filter(|provenance| {
            paths_match(
                Path::new(&provenance.source.source_project_path),
                source_project_path,
            ) && paths_match(
                Path::new(&provenance.source.source_face_path),
                source_face_path,
            )
        })
        .cloned()
        .collect()
}

fn inspect_lineage(path: PathBuf, source_project_path: &Path) -> LinkedProductionLineage {
    if !path.exists() {
        return failed(
            path,
            LinkedProductionLineageStatus::Missing,
            "Linked Production project is missing.",
        );
    }
    let project = match ShadeProject::load(&path) {
        Ok(project) => project,
        Err(error) => {
            return failed(path, LinkedProductionLineageStatus::Unreadable, error);
        }
    };
    if project.project_role != ProjectRole::Production {
        return failed(
            path,
            LinkedProductionLineageStatus::InvalidRole,
            "Linked project is not marked as a Production project.",
        );
    }
    if !project.linked_projects.iter().any(|link| {
        link.role == ProjectRole::Source
            && paths_match(Path::new(&link.path), source_project_path)
    }) {
        return failed(
            path,
            LinkedProductionLineageStatus::WrongSource,
            "Linked Production project does not point back to this Source project.",
        );
    }
    if project.production_provenance.len() != project.faces.len()
        || project.production_provenance.is_empty()
    {
        return failed(
            path,
            LinkedProductionLineageStatus::InvalidProvenance,
            "Linked Production project does not have a one-to-one Face/provenance history.",
        );
    }
    if project.production_provenance.iter().any(|provenance| {
        !paths_match(
            Path::new(&provenance.source.source_project_path),
            source_project_path,
        )
    }) {
        return failed(
            path,
            LinkedProductionLineageStatus::InvalidProvenance,
            "Linked Production project contains provenance from a different Source project.",
        );
    }

    LinkedProductionLineage {
        project_path: path,
        project_name: Some(project.name),
        status: LinkedProductionLineageStatus::Ready,
        provenances: project.production_provenance,
        diagnostic: None,
    }
}

fn failed(
    path: PathBuf,
    status: LinkedProductionLineageStatus,
    diagnostic: impl Into<String>,
) -> LinkedProductionLineage {
    LinkedProductionLineage {
        project_path: path,
        project_name: None,
        status,
        provenances: Vec::new(),
        diagnostic: Some(diagnostic.into()),
    }
}

fn resolve_linked_path(recorded: &str, source_project_path: &Path) -> PathBuf {
    let path = PathBuf::from(recorded.trim());
    if path.is_absolute() {
        path
    } else {
        source_project_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    }
}

fn paths_match(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
        ConversionRenderingIntent, ConversionSourceRef, ConversionTargetDefinition,
        LinkedProjectRef, SeparationStrategy, TargetChannelDefinition,
    };
    use crate::model::IccProfileIdentity;
    use crate::production_project::{ProductionProjectSpec, build_production_project};

    fn hash(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn temp_path(label: &str, extension: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shade-production-lineage-{label}-{}-{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            extension
        ))
    }

    fn provenance(source: &Path, face: &Path, output: &Path) -> ProductionProvenance {
        ProductionProvenance {
            source: ConversionSourceRef {
                source_project_path: source.to_string_lossy().into_owned(),
                source_face_path: face.to_string_lossy().into_owned(),
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
            output_path: output.to_string_lossy().into_owned(),
            output_sha256: hash('o'),
            converted_at_unix_ms: 1,
        }
    }

    #[test]
    fn collects_provenance_from_valid_linked_production_project() {
        let source = temp_path("source", "shade");
        let face = temp_path("face", "tif");
        let output = temp_path("output", "tif");
        let production_path = temp_path("production", "shade");
        let production = build_production_project(ProductionProjectSpec {
            project_name: "Production",
            source_project_path: &source,
            output_tiff_path: &output,
            output_face_label: "Face 1",
            provenance: provenance(&source, &face, &output),
        })
        .unwrap();
        production
            .save_new(&production_path, std::slice::from_ref(&output))
            .unwrap();
        let source_project = ShadeProject {
            project_role: ProjectRole::Source,
            linked_projects: vec![LinkedProjectRef {
                role: ProjectRole::Production,
                path: production_path.to_string_lossy().into_owned(),
            }],
            ..ShadeProject::default()
        };

        let lineage = collect_linked_production_lineage(&source_project, &source);
        assert_eq!(lineage.len(), 1);
        assert_eq!(lineage[0].status, LinkedProductionLineageStatus::Ready);
        let matches = provenance_for_source_face(&lineage, &source, &face);
        assert_eq!(matches.len(), 1);
        let _ = std::fs::remove_file(production_path);
    }

    #[test]
    fn missing_link_remains_visible_as_diagnostic_state() {
        let source = temp_path("missing-source", "shade");
        let missing = temp_path("missing-production", "shade");
        let source_project = ShadeProject {
            project_role: ProjectRole::Source,
            linked_projects: vec![LinkedProjectRef {
                role: ProjectRole::Production,
                path: missing.to_string_lossy().into_owned(),
            }],
            ..ShadeProject::default()
        };
        let lineage = collect_linked_production_lineage(&source_project, &source);
        assert_eq!(lineage[0].status, LinkedProductionLineageStatus::Missing);
        assert!(lineage[0].diagnostic.is_some());
    }
}