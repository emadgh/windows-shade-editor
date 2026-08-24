use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::color_conversion::{ConversionRecipe, ProductionProvenance, ProjectRole};
use crate::conversion_batch::batch_recipe_policy_sha256;
use crate::model::ShadeProject;

pub const CONVERSION_ROUTE_SCHEMA_VERSION: u32 = 1;

/// Source-project mirror of one linked Production conversion route.
///
/// Production projects remain the authority for committed output provenance. The Source project
/// stores this compact route mirror so the operator can reconstruct the exact conversion decision
/// after restart, even before the linked Production project is opened. `faces` deliberately keeps
/// the immutable Production provenance plus the external Source ICC locator (when one was used),
/// because the locator is execution state and is not part of `ProductionProvenance` itself.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConversionRouteRecord {
    pub schema_version: u32,
    pub production_project_path: String,
    pub output_folder: String,
    pub batch_recipe_policy_sha256: String,
    pub faces: Vec<ConversionRouteFaceRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConversionRouteFaceRecord {
    pub provenance: ProductionProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_profile_path: Option<String>,
}

impl ConversionRouteRecord {
    pub fn baseline_recipe(&self) -> Option<&ConversionRecipe> {
        self.faces.first().map(|face| &face.provenance.recipe)
    }

    pub fn converted_face_count(&self) -> usize {
        self.faces.len()
    }

    pub fn production_project_path(&self) -> PathBuf {
        PathBuf::from(&self.production_project_path)
    }

    pub fn output_folder(&self) -> PathBuf {
        PathBuf::from(&self.output_folder)
    }

    pub fn matches_recipe_policy(&self, recipe: &ConversionRecipe) -> Result<bool, String> {
        Ok(self
            .batch_recipe_policy_sha256
            .eq_ignore_ascii_case(&batch_recipe_policy_sha256(recipe)?))
    }

    pub fn face_for_source(&self, source_face_path: &Path) -> Option<&ConversionRouteFaceRecord> {
        self.faces.iter().find(|face| {
            paths_match(
                Path::new(&face.provenance.source.source_face_path),
                source_face_path,
            )
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != CONVERSION_ROUTE_SCHEMA_VERSION {
            return Err(format!(
                "Unsupported conversion route schema {} (expected {}).",
                self.schema_version, CONVERSION_ROUTE_SCHEMA_VERSION
            ));
        }
        if self.production_project_path.trim().is_empty() {
            return Err("Conversion route Production project path cannot be empty.".to_owned());
        }
        if self.output_folder.trim().is_empty() {
            return Err("Conversion route output folder cannot be empty.".to_owned());
        }
        if !is_sha256(&self.batch_recipe_policy_sha256) {
            return Err("Conversion route requires a canonical recipe-policy SHA-256.".to_owned());
        }
        if self.faces.is_empty() {
            return Err("Conversion route requires at least one committed Face.".to_owned());
        }

        let expected_folder = Path::new(&self.output_folder);
        let mut sources = BTreeSet::new();
        let mut outputs = BTreeSet::new();
        for face in &self.faces {
            face.provenance.recipe.validate().map_err(|errors| {
                format!("Invalid conversion route recipe: {}", errors.join(" "))
            })?;
            let policy = batch_recipe_policy_sha256(&face.provenance.recipe)?;
            if !policy.eq_ignore_ascii_case(&self.batch_recipe_policy_sha256) {
                return Err(
                    "Conversion route contains a Face captured with a different target policy."
                        .to_owned(),
                );
            }
            if face.provenance.source.source_face_path.trim().is_empty()
                || face.provenance.output_path.trim().is_empty()
            {
                return Err("Conversion route Face paths cannot be empty.".to_owned());
            }
            if !sources.insert(path_key(Path::new(&face.provenance.source.source_face_path))) {
                return Err("Conversion route contains the same Source Face more than once.".to_owned());
            }
            if !outputs.insert(path_key(Path::new(&face.provenance.output_path))) {
                return Err("Conversion route contains the same Production output more than once.".to_owned());
            }
            let output_parent = Path::new(&face.provenance.output_path)
                .parent()
                .unwrap_or_else(|| Path::new("."));
            if !paths_match(output_parent, expected_folder) {
                return Err(format!(
                    "Conversion route output {} is outside the recorded destination folder {}.",
                    face.provenance.output_path, self.output_folder
                ));
            }
            if face
                .source_profile_path
                .as_deref()
                .is_some_and(|path| path.trim().is_empty())
            {
                return Err("Conversion route Source ICC locator cannot be empty.".to_owned());
            }
        }
        Ok(())
    }
}

/// Build the Source-side route mirror from the exact Production project that has just been
/// committed. This never guesses from filenames: Source Face identity, recipe and output mapping
/// come from Production provenance, while the Source ICC locator is recovered from the saved
/// Source Face assignment that was used when the batch was captured.
pub fn build_conversion_route_record(
    source_project: &ShadeProject,
    source_project_path: &Path,
    production_project: &ShadeProject,
    production_project_path: &Path,
) -> Result<ConversionRouteRecord, String> {
    if production_project.project_role != ProjectRole::Production {
        return Err("Conversion route destination is not a Production project.".to_owned());
    }
    if production_project.faces.len() != production_project.production_provenance.len()
        || production_project.production_provenance.is_empty()
    {
        return Err(
            "Production Face/provenance pairing is incomplete; route mirror was not updated."
                .to_owned(),
        );
    }

    let resolved_source_faces = source_project.resolve_face_paths(source_project_path);
    let first = &production_project.production_provenance[0];
    let policy_sha256 = batch_recipe_policy_sha256(&first.recipe)?;
    let output_folder = Path::new(&first.output_path)
        .parent()
        .ok_or_else(|| "Production output does not have a destination folder.".to_owned())?
        .to_path_buf();

    let mut faces = Vec::with_capacity(production_project.production_provenance.len());
    for provenance in &production_project.production_provenance {
        if !paths_match(
            Path::new(&provenance.source.source_project_path),
            source_project_path,
        ) {
            return Err(
                "Production provenance references a different Source project; route mirror was not updated."
                    .to_owned(),
            );
        }
        let face_index = resolved_source_faces
            .iter()
            .position(|path| paths_match(path, Path::new(&provenance.source.source_face_path)))
            .ok_or_else(|| {
                format!(
                    "Production provenance references Source Face {} which is not present in the Source project.",
                    provenance.source.source_face_path
                )
            })?;
        let source_profile_path = source_project
            .faces
            .get(face_index)
            .and_then(|face| face.production_source_profile.as_ref())
            .map(|assignment| assignment.path.clone());
        if let Some(assignment) = source_project
            .faces
            .get(face_index)
            .and_then(|face| face.production_source_profile.as_ref())
        {
            if assignment.identity != provenance.recipe.source_profile_identity {
                return Err(format!(
                    "Source ICC assignment for Face {} changed before route persistence; save/reconvert before updating the route mirror.",
                    face_index + 1
                ));
            }
        }
        faces.push(ConversionRouteFaceRecord {
            provenance: provenance.clone(),
            source_profile_path,
        });
    }

    let route = ConversionRouteRecord {
        schema_version: CONVERSION_ROUTE_SCHEMA_VERSION,
        production_project_path: production_project_path.to_string_lossy().into_owned(),
        output_folder: output_folder.to_string_lossy().into_owned(),
        batch_recipe_policy_sha256: policy_sha256,
        faces,
    };
    route.validate()?;
    Ok(route)
}

/// Upsert one route by its linked Production project path. Different Production links are retained
/// independently; re-converting/appending to the same route refreshes its committed Face mirror.
pub fn upsert_conversion_route(
    source_project: &mut ShadeProject,
    route: ConversionRouteRecord,
) -> Result<(), String> {
    route.validate()?;
    if let Some(existing) = source_project.conversion_routes.iter_mut().find(|existing| {
        paths_match(
            Path::new(&existing.production_project_path),
            Path::new(&route.production_project_path),
        )
    }) {
        *existing = route;
    } else {
        source_project.conversion_routes.push(route);
        source_project.conversion_routes.sort_by(|left, right| {
            path_key(Path::new(&left.production_project_path))
                .cmp(&path_key(Path::new(&right.production_project_path)))
        });
    }
    Ok(())
}

pub fn route_for_production_path<'a>(
    source_project: &'a ShadeProject,
    production_project_path: &Path,
) -> Option<&'a ConversionRouteRecord> {
    source_project.conversion_routes.iter().find(|route| {
        paths_match(
            Path::new(&route.production_project_path),
            production_project_path,
        )
    })
}

pub fn matching_routes_for_recipe<'a>(
    source_project: &'a ShadeProject,
    recipe: &ConversionRecipe,
) -> Result<Vec<&'a ConversionRouteRecord>, String> {
    let policy = batch_recipe_policy_sha256(recipe)?;
    Ok(source_project
        .conversion_routes
        .iter()
        .filter(|route| {
            route
                .batch_recipe_policy_sha256
                .eq_ignore_ascii_case(&policy)
        })
        .collect())
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\").to_ascii_lowercase()
}

fn paths_match(left: &Path, right: &Path) -> bool {
    path_key(left) == path_key(right)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRenderingIntent,
        ConversionSourceRef, ConversionTargetDefinition, SeparationStrategy, TargetChannelDefinition,
    };
    use crate::model::{FaceRef, FaceStatus, IccProfileIdentity, ProductionSourceProfileAssignment};
    use crate::production_project::{ProductionProjectSpec, build_production_project};

    fn hash(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn recipe(source_hash: char) -> ConversionRecipe {
        ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: IccProfileIdentity {
                description: "Source RGB".to_owned(),
                sha256: hash(source_hash),
            },
            source_transparency_policy: None,
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
        }
    }

    fn provenance(source_face: &str, output: &str, source_hash: char) -> ProductionProvenance {
        ProductionProvenance {
            source: ConversionSourceRef {
                source_project_path: r"C:\Design\Source.shade".to_owned(),
                source_face_path: source_face.to_owned(),
                source_snapshot_id: Some(7),
                source_file_sha256: hash('s'),
            },
            recipe: recipe(source_hash),
            custom_optimizer: None,
            output_path: output.to_owned(),
            output_sha256: hash('o'),
            converted_at_unix_ms: 10,
        }
    }

    fn source() -> ShadeProject {
        ShadeProject {
            name: "Source".to_owned(),
            faces: vec![FaceRef {
                path: r"C:\Design\Face-1.tif".to_owned(),
                label: "Face 1".to_owned(),
                status: FaceStatus::Accepted,
                production_source_profile: Some(ProductionSourceProfileAssignment {
                    path: r"C:\Color\Source.icc".to_owned(),
                    identity: IccProfileIdentity {
                        description: "Source RGB".to_owned(),
                        sha256: hash('a'),
                    },
                }),
            }],
            ..ShadeProject::default()
        }
    }

    fn production() -> ShadeProject {
        let output = Path::new(r"C:\Production\Face-1.tif");
        build_production_project(ProductionProjectSpec {
            project_name: "Production",
            source_project_path: Path::new(r"C:\Design\Source.shade"),
            output_tiff_path: output,
            output_face_label: "Face 1",
            provenance: provenance(r"C:\Design\Face-1.tif", r"C:\Production\Face-1.tif", 'a'),
        })
        .unwrap()
    }

    #[test]
    fn source_route_mirror_preserves_exact_policy_provenance_and_source_icc_locator() {
        let source = source();
        let production = production();
        let route = build_conversion_route_record(
            &source,
            Path::new(r"C:\Design\Source.shade"),
            &production,
            Path::new(r"C:\Production\Job.shade"),
        )
        .unwrap();
        assert_eq!(route.converted_face_count(), 1);
        assert_eq!(
            route.faces[0].source_profile_path.as_deref(),
            Some(r"C:\Color\Source.icc")
        );
        assert!(route.matches_recipe_policy(&recipe('z')).unwrap());
        assert_eq!(
            route.baseline_recipe().unwrap().source_profile_identity.sha256,
            hash('a')
        );
    }

    #[test]
    fn same_production_path_is_updated_while_distinct_routes_are_retained() {
        let source = source();
        let production = production();
        let mut route = build_conversion_route_record(
            &source,
            Path::new(r"C:\Design\Source.shade"),
            &production,
            Path::new(r"C:\Production\Job.shade"),
        )
        .unwrap();
        let mut project = source;
        upsert_conversion_route(&mut project, route.clone()).unwrap();
        route.faces[0].provenance.converted_at_unix_ms = 20;
        upsert_conversion_route(&mut project, route).unwrap();
        assert_eq!(project.conversion_routes.len(), 1);
        assert_eq!(project.conversion_routes[0].faces[0].provenance.converted_at_unix_ms, 20);

        let mut second = project.conversion_routes[0].clone();
        second.production_project_path = r"C:\Production\Other.shade".to_owned();
        upsert_conversion_route(&mut project, second).unwrap();
        assert_eq!(project.conversion_routes.len(), 2);
    }

    #[test]
    fn target_policy_drift_no_longer_matches_route() {
        let source = source();
        let production = production();
        let route = build_conversion_route_record(
            &source,
            Path::new(r"C:\Design\Source.shade"),
            &production,
            Path::new(r"C:\Production\Job.shade"),
        )
        .unwrap();
        let mut changed = recipe('a');
        changed.black_point_compensation = false;
        assert!(!route.matches_recipe_policy(&changed).unwrap());
    }
}
