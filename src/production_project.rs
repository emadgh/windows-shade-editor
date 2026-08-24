use std::path::Path;

use crate::color_conversion::production_provenance::validate_production_provenance;
use crate::color_conversion::{LinkedProjectRef, ProductionProvenance, ProjectRole};
use crate::conversion_route::{build_conversion_route_record, upsert_conversion_route};
use crate::model::{FaceRef, FaceStatus, ShadeProject};

/// Immutable inputs needed after a converted TIFF has been validated and
/// committed. Pixel conversion and project serialization stay separate so a
/// worker can recover a committed TIFF even if saving the `.shade` later fails.
pub struct ProductionProjectSpec<'a> {
    pub project_name: &'a str,
    pub source_project_path: &'a Path,
    pub output_tiff_path: &'a Path,
    pub output_face_label: &'a str,
    pub provenance: ProductionProvenance,
}

/// Build a new Production project without carrying source-domain adjustments,
/// snapshots, preview ICC choices or test-code state into the target ink space.
pub fn build_production_project(spec: ProductionProjectSpec<'_>) -> Result<ShadeProject, String> {
    validate_production_provenance(&spec.provenance)?;
    spec.provenance
        .recipe
        .validate()
        .map_err(|errors| format!("Invalid production conversion recipe: {}", errors.join(" ")))?;

    if spec.project_name.trim().is_empty() {
        return Err("Production project name cannot be empty.".to_owned());
    }
    if spec.output_face_label.trim().is_empty() {
        return Err("Production Face label cannot be empty.".to_owned());
    }
    if spec.source_project_path.as_os_str().is_empty() {
        return Err("Source project path cannot be empty.".to_owned());
    }
    if spec.output_tiff_path.as_os_str().is_empty() {
        return Err("Production TIFF path cannot be empty.".to_owned());
    }
    if !paths_match(&spec.provenance.output_path, spec.output_tiff_path) {
        return Err(
            "Production provenance output path does not match the committed TIFF path.".to_owned(),
        );
    }

    let channel_names = spec
        .provenance
        .recipe
        .target
        .channels
        .iter()
        .map(|channel| channel.name.trim().to_owned())
        .collect::<Vec<_>>();

    let mut project = ShadeProject {
        name: spec.project_name.trim().to_owned(),
        project_role: ProjectRole::Production,
        linked_projects: vec![LinkedProjectRef {
            role: ProjectRole::Source,
            path: spec.source_project_path.display().to_string(),
        }],
        production_provenance: vec![spec.provenance],
        faces: vec![FaceRef {
            path: spec.output_tiff_path.display().to_string(),
            label: spec.output_face_label.trim().to_owned(),
            status: FaceStatus::Accepted,
            production_source_profile: None,
        }],
        ..ShadeProject::default()
    };
    project.ensure_channels(&channel_names);
    Ok(project)
}

/// Mark a saved design project as a Source and retain a link to the exact Production project.
///
/// One Source can legitimately feed multiple Production projects/targets. Re-linking the same
/// Production path is idempotent, while a different path is appended instead of replacing the
/// previous relationship. This is intentionally separate from `build_production_project`: the
/// caller saves it only after the Production TIFF/project transaction succeeds.
pub fn link_source_project_to_production(
    source: &mut ShadeProject,
    production_project_path: &Path,
) -> Result<(), String> {
    if production_project_path.as_os_str().is_empty() {
        return Err("Production project path cannot be empty.".to_owned());
    }

    source.project_role = ProjectRole::Source;
    let new_path = production_project_path.display().to_string();
    if let Some(link) = source
        .linked_projects
        .iter_mut()
        .find(|link| {
            link.role == ProjectRole::Production
                && link.path.trim().eq_ignore_ascii_case(new_path.as_str())
        })
    {
        link.path = new_path;
    } else {
        source.linked_projects.push(LinkedProjectRef {
            role: ProjectRole::Production,
            path: new_path,
        });
    }
    Ok(())
}

/// Synchronize the Source-side link and persisted conversion-route mirror from an exact,
/// already-committed Production project. This is called after each durable batch checkpoint so a
/// restart never has to infer route settings from filenames or UI state.
pub fn sync_source_project_to_production_route(
    source: &mut ShadeProject,
    source_project_path: &Path,
    production_project_path: &Path,
    production_project: &ShadeProject,
) -> Result<(), String> {
    let route = build_conversion_route_record(
        source,
        source_project_path,
        production_project,
        production_project_path,
    )?;
    link_source_project_to_production(source, production_project_path)?;
    upsert_conversion_route(source, route)
}

fn paths_match(recorded: &str, actual: &Path) -> bool {
    recorded
        .trim()
        .eq_ignore_ascii_case(actual.to_string_lossy().as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
        ConversionRenderingIntent, ConversionSourceRef, ConversionTargetDefinition,
        SeparationStrategy, TargetChannelDefinition,
    };
    use crate::model::{ChannelAdjustment, IccProfileIdentity};

    fn identity(description: &str, hash: &str) -> IccProfileIdentity {
        IccProfileIdentity {
            description: description.to_owned(),
            sha256: hash.to_owned(),
        }
    }

    fn provenance(output: &Path) -> ProductionProvenance {
        ProductionProvenance {
            source: ConversionSourceRef {
                source_project_path: r"C:\Design\Source.shade".to_owned(),
                source_face_path: r"C:\Design\Face.png".to_owned(),
                source_snapshot_id: Some(7),
                source_file_sha256: "source-file-hash".to_owned(),
            },
            recipe: ConversionRecipe {
                source_transparency_policy: None,
                schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
                engine_mode: ConversionEngineMode::Icc,
                source_profile_identity: identity("sRGB", "source-profile-hash"),
                target: ConversionTargetDefinition {
                    name: "Press CMYK".to_owned(),
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
                    output_profile_identity: Some(identity("Press CMYK", "target-hash")),
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
            output_path: output.display().to_string(),
            output_sha256: "output-hash".to_owned(),
            converted_at_unix_ms: 1234,
        }
    }

    #[test]
    fn production_project_has_target_channels_without_source_adjustments() {
        let output = Path::new(r"C:\Production\Face_CMYK.tif");
        let project = build_production_project(ProductionProjectSpec {
            project_name: "Production",
            source_project_path: Path::new(r"C:\Design\Source.shade"),
            output_tiff_path: output,
            output_face_label: "Face CMYK",
            provenance: provenance(output),
        })
        .unwrap();

        assert_eq!(project.project_role, ProjectRole::Production);
        assert_eq!(project.faces.len(), 1);
        assert_eq!(project.production_provenance.len(), 1);
        assert_eq!(project.linked_projects[0].role, ProjectRole::Source);
        assert_eq!(
            project.adjustments.keys().cloned().collect::<Vec<_>>(),
            ["Black", "Cyan", "Magenta", "Yellow"]
        );
        assert!(project.snapshots.is_empty());
    }

    #[test]
    fn source_retains_multiple_production_links_without_touching_adjustments() {
        let mut source = ShadeProject::default();
        source
            .adjustments
            .insert("Red".to_owned(), ChannelAdjustment::default());

        let first = Path::new(r"C:\Production\Job.shade");
        let second = Path::new(r"C:\Production\Job-v2.shade");
        link_source_project_to_production(&mut source, first).unwrap();
        link_source_project_to_production(&mut source, second).unwrap();
        link_source_project_to_production(&mut source, first).unwrap();

        assert_eq!(source.project_role, ProjectRole::Source);
        assert_eq!(source.linked_projects.len(), 2);
        assert!(source.linked_projects.iter().any(|link| link.path.ends_with("Job.shade")));
        assert!(source.linked_projects.iter().any(|link| link.path.ends_with("Job-v2.shade")));
        assert!(source.adjustments.contains_key("Red"));
    }

    #[test]
    fn legacy_project_without_lineage_fields_remains_standalone() {
        let mut value = serde_json::to_value(ShadeProject::default()).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("project_role");
        object.remove("linked_projects");
        object.remove("production_provenance");

        let restored: ShadeProject = serde_json::from_value(value).unwrap();
        assert_eq!(restored.project_role, ProjectRole::Standalone);
        assert!(restored.linked_projects.is_empty());
        assert!(restored.production_provenance.is_empty());
    }

    #[test]
    fn mismatched_provenance_output_is_rejected() {
        let output = Path::new(r"C:\Production\Face_CMYK.tif");
        let error = build_production_project(ProductionProjectSpec {
            project_name: "Production",
            source_project_path: Path::new(r"C:\Design\Source.shade"),
            output_tiff_path: output,
            output_face_label: "Face CMYK",
            provenance: provenance(Path::new(r"C:\Production\Other.tif")),
        })
        .expect_err("output identity must be exact");
        assert!(error.contains("does not match"));
    }
}
