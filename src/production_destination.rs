use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::color_conversion::{ConversionRecipe, ProjectRole};
use crate::icc_conversion_worker::sha256_file;
use crate::model::ShadeProject;
use crate::production_project_compat::{
    ProductionCompatibilityKey, validate_existing_production_project_baseline_at_path,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProductionDestinationAvailability {
    Ready,
    Missing,
    Unreadable,
    Incompatible,
}

#[derive(Clone, Debug)]
pub struct ProductionDestinationCandidate {
    pub path: PathBuf,
    pub availability: ProductionDestinationAvailability,
    pub project_name: Option<String>,
    pub face_count: Option<usize>,
    pub compatibility: Option<ProductionCompatibilityKey>,
    pub project_sha256: Option<String>,
    /// Baseline recipe from the first validated Production provenance. The UI
    /// may use it only as a seed; external ICC/DeviceLink bytes must still be
    /// reopened and identity-verified before a conversion can be queued.
    pub baseline_recipe: Option<ConversionRecipe>,
    pub diagnostic: Option<String>,
}

impl ProductionDestinationCandidate {
    pub fn can_append(&self) -> bool {
        self.availability == ProductionDestinationAvailability::Ready
            && self.compatibility.is_some()
            && self.project_sha256.is_some()
            && self.baseline_recipe.is_some()
    }

    pub fn matches_recipe(&self, recipe: &ConversionRecipe) -> Result<bool, String> {
        let Some(expected) = self.compatibility.as_ref() else {
            return Ok(false);
        };
        Ok(expected == &compatibility_for_recipe(recipe)?)
    }
}

/// Build the exact target-side compatibility identity directly from a proposed
/// recipe. This lets the UI fail closed before enqueueing when an operator edits
/// target controls after selecting an existing Production project.
pub fn compatibility_for_recipe(
    recipe: &ConversionRecipe,
) -> Result<ProductionCompatibilityKey, String> {
    recipe
        .validate()
        .map_err(|errors| format!("Invalid conversion recipe: {}", errors.join(" ")))?;
    let target = &recipe.target;
    Ok(ProductionCompatibilityKey {
        engine_mode: recipe.engine_mode,
        output_profile_sha256: target
            .output_profile_identity
            .as_ref()
            .map(|identity| identity.sha256.trim().to_ascii_lowercase()),
        device_link_sha256: target
            .device_link_identity
            .as_ref()
            .map(|identity| identity.sha256.trim().to_ascii_lowercase()),
        characterization_id: target
            .characterization_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        channel_names: target
            .channels
            .iter()
            .map(|channel| channel.name.trim().to_owned())
            .collect(),
        bit_depth: target.bit_depth,
    })
}

pub fn inspect_linked_production_destinations(
    source_project: &ShadeProject,
    source_project_path: &Path,
) -> Vec<ProductionDestinationCandidate> {
    let mut seen = BTreeSet::new();
    source_project
        .linked_projects
        .iter()
        .filter(|link| link.role == ProjectRole::Production)
        .filter_map(|link| {
            let path = resolve_linked_project_path(&link.path, source_project_path);
            let key = path.to_string_lossy().to_ascii_lowercase();
            seen.insert(key).then(|| inspect_candidate(path, source_project_path))
        })
        .collect()
}

pub fn inspect_candidate(
    path: PathBuf,
    source_project_path: &Path,
) -> ProductionDestinationCandidate {
    if !path.exists() {
        return ProductionDestinationCandidate {
            path,
            availability: ProductionDestinationAvailability::Missing,
            project_name: None,
            face_count: None,
            compatibility: None,
            project_sha256: None,
            baseline_recipe: None,
            diagnostic: Some("Linked Production project is missing.".to_owned()),
        };
    }

    let before = match sha256_file(&path) {
        Ok(hash) => hash,
        Err(error) => return unreadable(path, error),
    };
    let project = match ShadeProject::load(&path) {
        Ok(project) => project,
        Err(error) => return unreadable(path, error),
    };
    let after = match sha256_file(&path) {
        Ok(hash) => hash,
        Err(error) => return unreadable(path, error),
    };
    if before != after {
        return unreadable(
            path,
            "Production project changed while it was being inspected; select again after the file is stable."
                .to_owned(),
        );
    }

    let project_name = Some(project.name.clone());
    let face_count = Some(project.faces.len());
    match validate_existing_production_project_baseline_at_path(
        &project,
        &path,
        source_project_path,
    ) {
        Ok(compatibility) => {
            let baseline_recipe = project
                .production_provenance
                .first()
                .map(|provenance| provenance.recipe.clone());
            ProductionDestinationCandidate {
                path,
                availability: ProductionDestinationAvailability::Ready,
                project_name,
                face_count,
                compatibility: Some(compatibility),
                project_sha256: Some(after),
                baseline_recipe,
                diagnostic: None,
            }
        }
        Err(error) => ProductionDestinationCandidate {
            path,
            availability: ProductionDestinationAvailability::Incompatible,
            project_name,
            face_count,
            compatibility: None,
            project_sha256: Some(after),
            baseline_recipe: None,
            diagnostic: Some(error),
        },
    }
}

pub fn resolve_linked_project_path(recorded: &str, source_project_path: &Path) -> PathBuf {
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

fn unreadable(path: PathBuf, error: impl Into<String>) -> ProductionDestinationCandidate {
    ProductionDestinationCandidate {
        path,
        availability: ProductionDestinationAvailability::Unreadable,
        project_name: None,
        face_count: None,
        compatibility: None,
        project_sha256: None,
        baseline_recipe: None,
        diagnostic: Some(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRenderingIntent,
        ConversionSourceRef, ConversionTargetDefinition, LinkedProjectRef, ProductionProvenance,
        SeparationStrategy, TargetChannelDefinition,
    };
    use crate::model::IccProfileIdentity;
    use crate::production_project::{ProductionProjectSpec, build_production_project};

    fn hash(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn temp_path(label: &str, extension: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shade-production-destination-{label}-{}-{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            extension
        ))
    }

    fn recipe() -> ConversionRecipe {
        ConversionRecipe {
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
        }
    }

    fn provenance(source: &Path, output: &Path) -> ProductionProvenance {
        ProductionProvenance {
            source: ConversionSourceRef {
                source_project_path: source.to_string_lossy().into_owned(),
                source_face_path: r"C:\Design\Face.tif".to_owned(),
                source_snapshot_id: None,
                source_file_sha256: hash('s'),
            },
            recipe: recipe(),
            custom_optimizer: None,
            profile_backed_optimizer: None,
            output_path: output.to_string_lossy().into_owned(),
            output_sha256: hash('o'),
            converted_at_unix_ms: 1,
        }
    }

    #[test]
    fn compatible_link_is_ready_with_sha_topology_and_recipe_seed() {
        let source_path = temp_path("source", "shade");
        let production_path = temp_path("production", "shade");
        let output_path = production_path.with_extension("tif");
        fs::write(&output_path, b"fixture").unwrap();
        let production = build_production_project(ProductionProjectSpec {
            project_name: "Production",
            source_project_path: &source_path,
            output_tiff_path: &output_path,
            output_face_label: "Face 1",
            provenance: provenance(&source_path, &output_path),
        })
        .unwrap();
        production
            .save_new(&production_path, std::slice::from_ref(&output_path))
            .unwrap();
        let source = ShadeProject {
            project_role: ProjectRole::Source,
            linked_projects: vec![LinkedProjectRef {
                role: ProjectRole::Production,
                path: production_path.to_string_lossy().into_owned(),
            }],
            ..ShadeProject::default()
        };

        let candidates = inspect_linked_production_destinations(&source, &source_path);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].can_append());
        assert_eq!(candidates[0].face_count, Some(1));
        assert_eq!(
            candidates[0].compatibility.as_ref().unwrap().channel_names,
            ["Cyan", "Magenta", "Yellow", "Black"]
        );
        assert!(candidates[0].baseline_recipe.is_some());
        assert!(candidates[0].matches_recipe(&recipe()).unwrap());
        let _ = fs::remove_file(output_path);
        let _ = fs::remove_file(production_path);
    }

    #[test]
    fn recipe_target_drift_is_detected_before_append_capture() {
        let candidate = ProductionDestinationCandidate {
            path: PathBuf::from(r"C:\Production\Job.shade"),
            availability: ProductionDestinationAvailability::Ready,
            project_name: Some("Production".to_owned()),
            face_count: Some(1),
            compatibility: Some(compatibility_for_recipe(&recipe()).unwrap()),
            project_sha256: Some(hash('p')),
            baseline_recipe: Some(recipe()),
            diagnostic: None,
        };
        let mut changed = recipe();
        changed.target.bit_depth = 8;
        assert!(!candidate.matches_recipe(&changed).unwrap());
    }

    #[test]
    fn missing_link_is_reported_without_guessing() {
        let source_path = temp_path("source-missing", "shade");
        let missing = source_path.with_file_name("missing-production.shade");
        let source = ShadeProject {
            project_role: ProjectRole::Source,
            linked_projects: vec![LinkedProjectRef {
                role: ProjectRole::Production,
                path: missing.to_string_lossy().into_owned(),
            }],
            ..ShadeProject::default()
        };
        let candidates = inspect_linked_production_destinations(&source, &source_path);
        assert_eq!(candidates[0].availability, ProductionDestinationAvailability::Missing);
        assert!(!candidates[0].can_append());
    }

    #[test]
    fn relative_link_resolves_against_source_project_directory() {
        let source_path = Path::new(r"C:\Design\Source.shade");
        assert_eq!(
            resolve_linked_project_path("Production\\Job.shade", source_path),
            PathBuf::from(r"C:\Design\Production\Job.shade")
        );
    }
}
