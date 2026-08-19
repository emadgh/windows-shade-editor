use std::path::Path;

use crate::color_conversion::production_provenance::validate_production_provenance;
use crate::color_conversion::{ConversionEngineMode, ProductionProvenance, ProjectRole};
use crate::model::{FaceRef, FaceStatus, ShadeProject};

/// Stable target-side identity required for multiple converted Faces to coexist
/// inside one Production project. Source-side interpretation and per-Face
/// snapshot details are deliberately excluded: those remain recorded in each
/// Face's own `ProductionProvenance`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductionCompatibilityKey {
    pub engine_mode: ConversionEngineMode,
    pub output_profile_sha256: Option<String>,
    pub device_link_sha256: Option<String>,
    pub characterization_id: Option<String>,
    pub channel_names: Vec<String>,
    pub bit_depth: u8,
}

impl ProductionCompatibilityKey {
    pub fn from_provenance(provenance: &ProductionProvenance) -> Result<Self, String> {
        validate_production_provenance(provenance)?;
        provenance.recipe.validate().map_err(|errors| {
            format!("Invalid Production conversion recipe: {}", errors.join(" "))
        })?;
        let target = &provenance.recipe.target;
        Ok(Self {
            engine_mode: provenance.recipe.engine_mode,
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
}

pub struct AppendConvertedFaceSpec<'a> {
    pub source_project_path: &'a Path,
    pub output_face_label: &'a str,
    pub provenance: ProductionProvenance,
}

/// Validate that an incoming converted Face can be appended to an existing
/// Production project without changing the project's target-side semantics.
pub fn validate_existing_production_project_for_append(
    project: &ShadeProject,
    source_project_path: &Path,
    incoming: &ProductionProvenance,
) -> Result<ProductionCompatibilityKey, String> {
    if project.project_role != ProjectRole::Production {
        return Err("Append destination is not a Production project.".to_owned());
    }
    if source_project_path.as_os_str().is_empty() {
        return Err("Source project path cannot be empty for Production append.".to_owned());
    }
    if project.faces.is_empty() || project.production_provenance.is_empty() {
        return Err(
            "Existing Production project has no committed Face/provenance baseline to validate."
                .to_owned(),
        );
    }
    if project.faces.len() != project.production_provenance.len() {
        return Err(format!(
            "Existing Production project has {} Faces but {} provenance records; append is blocked until lineage is repaired.",
            project.faces.len(),
            project.production_provenance.len()
        ));
    }
    if !project.linked_projects.iter().any(|link| {
        link.role == ProjectRole::Source && paths_match(&link.path, source_project_path)
    }) {
        return Err(format!(
            "Existing Production project is not linked to Source project {}.",
            source_project_path.display()
        ));
    }
    if !paths_match(&incoming.source.source_project_path, source_project_path) {
        return Err(
            "Incoming conversion provenance references a different Source project.".to_owned(),
        );
    }

    let canonical = ProductionCompatibilityKey::from_provenance(
        project
            .production_provenance
            .first()
            .expect("non-empty provenance checked above"),
    )?;

    for (index, (face, provenance)) in project
        .faces
        .iter()
        .zip(project.production_provenance.iter())
        .enumerate()
    {
        if !paths_match(&provenance.output_path, Path::new(&face.path)) {
            return Err(format!(
                "Existing Production Face {} does not match its persisted provenance output path.",
                index + 1
            ));
        }
        let key = ProductionCompatibilityKey::from_provenance(provenance)?;
        if key != canonical {
            return Err(format!(
                "Existing Production project already contains incompatible target provenance at Face {}: {}",
                index + 1,
                describe_key_mismatch(&canonical, &key)
            ));
        }
    }

    for channel in &canonical.channel_names {
        if !project.adjustments.contains_key(channel) {
            return Err(format!(
                "Existing Production project is missing target channel adjustment state for '{channel}'."
            ));
        }
    }

    let incoming_key = ProductionCompatibilityKey::from_provenance(incoming)?;
    if incoming_key != canonical {
        return Err(format!(
            "Incoming converted Face is incompatible with the existing Production target: {}",
            describe_key_mismatch(&canonical, &incoming_key)
        ));
    }

    if project.faces.iter().any(|face| paths_match(&face.path, Path::new(&incoming.output_path)))
        || project
            .production_provenance
            .iter()
            .any(|provenance| paths_match(&provenance.output_path, Path::new(&incoming.output_path)))
    {
        return Err(
            "Incoming converted TIFF is already present in the Production project.".to_owned(),
        );
    }

    Ok(canonical)
}

/// Append one already-committed converted TIFF and its immutable provenance.
/// Existing Faces, target adjustments and Snapshots are not rewritten.
pub fn append_converted_face_to_production_project(
    project: &mut ShadeProject,
    spec: AppendConvertedFaceSpec<'_>,
) -> Result<(), String> {
    let label = spec.output_face_label.trim();
    if label.is_empty() {
        return Err("Production Face label cannot be empty.".to_owned());
    }
    if spec.provenance.output_path.trim().is_empty() {
        return Err("Production provenance output path cannot be empty.".to_owned());
    }

    validate_existing_production_project_for_append(
        project,
        spec.source_project_path,
        &spec.provenance,
    )?;

    project.faces.push(FaceRef {
        path: spec.provenance.output_path.clone(),
        label: label.to_owned(),
        status: FaceStatus::Accepted,
        production_source_profile: None,
    });
    project.production_provenance.push(spec.provenance);
    Ok(())
}

fn describe_key_mismatch(
    expected: &ProductionCompatibilityKey,
    actual: &ProductionCompatibilityKey,
) -> String {
    if expected.engine_mode != actual.engine_mode {
        return format!(
            "conversion engine differs ({:?} vs {:?})",
            expected.engine_mode, actual.engine_mode
        );
    }
    if expected.output_profile_sha256 != actual.output_profile_sha256 {
        return "target output ICC identity differs".to_owned();
    }
    if expected.device_link_sha256 != actual.device_link_sha256 {
        return "DeviceLink identity differs".to_owned();
    }
    if expected.characterization_id != actual.characterization_id {
        return "target characterization identity differs".to_owned();
    }
    if expected.channel_names != actual.channel_names {
        return format!(
            "target channel order/names differ ({:?} vs {:?})",
            expected.channel_names, actual.channel_names
        );
    }
    if expected.bit_depth != actual.bit_depth {
        return format!(
            "target bit depth differs ({} vs {})",
            expected.bit_depth, actual.bit_depth
        );
    }
    "target compatibility identity differs".to_owned()
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
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionRecipe, ConversionRenderingIntent,
        ConversionSourceRef, ConversionTargetDefinition, SeparationStrategy,
        TargetChannelDefinition,
    };
    use crate::model::IccProfileIdentity;
    use crate::production_project::{ProductionProjectSpec, build_production_project};

    fn hash(character: char) -> String {
        std::iter::repeat(character).take(64).collect()
    }

    fn identity(description: &str, character: char) -> IccProfileIdentity {
        IccProfileIdentity {
            description: description.to_owned(),
            sha256: hash(character),
        }
    }

    fn provenance(output: &Path, source_face: &str, source_profile_hash: char) -> ProductionProvenance {
        ProductionProvenance {
            source: ConversionSourceRef {
                source_project_path: r"C:\Design\Source.shade".to_owned(),
                source_face_path: source_face.to_owned(),
                source_snapshot_id: Some(7),
                source_file_sha256: hash('s'),
            },
            recipe: ConversionRecipe {
                schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
                engine_mode: ConversionEngineMode::Icc,
                source_profile_identity: identity("Source RGB", source_profile_hash),
                source_transparency_policy: None,
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
                    output_profile_identity: Some(identity("Press CMYK", 't')),
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
            output_sha256: hash('o'),
            converted_at_unix_ms: 1234,
        }
    }

    fn production_project() -> ShadeProject {
        let output = Path::new(r"C:\Production\Face-1.tif");
        build_production_project(ProductionProjectSpec {
            project_name: "Production",
            source_project_path: Path::new(r"C:\Design\Source.shade"),
            output_tiff_path: output,
            output_face_label: "Face 1",
            provenance: provenance(output, r"C:\Design\Face-1.png", 'a'),
        })
        .unwrap()
    }

    #[test]
    fn compatible_face_appends_without_rewriting_existing_state() {
        let mut project = production_project();
        let existing_path = project.faces[0].path.clone();
        let adjustment_count = project.adjustments.len();
        let snapshot_count = project.snapshots.len();
        let output = Path::new(r"C:\Production\Face-2.tif");
        let incoming = provenance(output, r"C:\Design\Face-2.jpg", 'b');

        append_converted_face_to_production_project(
            &mut project,
            AppendConvertedFaceSpec {
                source_project_path: Path::new(r"C:\Design\Source.shade"),
                output_face_label: "Face 2",
                provenance: incoming,
            },
        )
        .unwrap();

        assert_eq!(project.faces.len(), 2);
        assert_eq!(project.production_provenance.len(), 2);
        assert_eq!(project.faces[0].path, existing_path);
        assert_eq!(project.adjustments.len(), adjustment_count);
        assert_eq!(project.snapshots.len(), snapshot_count);
        assert!(project.faces[1].path.ends_with("Face-2.tif"));
        assert_eq!(project.production_provenance[1].recipe.source_profile_identity.sha256, hash('b'));
    }

    #[test]
    fn target_profile_mismatch_is_rejected() {
        let mut project = production_project();
        let output = Path::new(r"C:\Production\Face-2.tif");
        let mut incoming = provenance(output, r"C:\Design\Face-2.png", 'a');
        incoming.recipe.target.output_profile_identity = Some(identity("Other Press", 'x'));

        let error = append_converted_face_to_production_project(
            &mut project,
            AppendConvertedFaceSpec {
                source_project_path: Path::new(r"C:\Design\Source.shade"),
                output_face_label: "Face 2",
                provenance: incoming,
            },
        )
        .expect_err("target identity mismatch must fail");
        assert!(error.contains("output ICC identity differs"));
        assert_eq!(project.faces.len(), 1);
    }

    #[test]
    fn channel_order_and_bit_depth_mismatch_are_rejected() {
        let project = production_project();
        let output = Path::new(r"C:\Production\Face-2.tif");

        let mut channels = provenance(output, r"C:\Design\Face-2.png", 'a');
        channels.recipe.target.channels.swap(0, 1);
        let error = validate_existing_production_project_for_append(
            &project,
            Path::new(r"C:\Design\Source.shade"),
            &channels,
        )
        .expect_err("channel order mismatch must fail");
        assert!(error.contains("channel order/names differ"));

        let mut depth = provenance(output, r"C:\Design\Face-2.png", 'a');
        depth.recipe.target.bit_depth = 8;
        let error = validate_existing_production_project_for_append(
            &project,
            Path::new(r"C:\Design\Source.shade"),
            &depth,
        )
        .expect_err("bit depth mismatch must fail");
        assert!(error.contains("bit depth differs"));
    }

    #[test]
    fn engine_family_mismatch_is_rejected() {
        let project = production_project();
        let output = Path::new(r"C:\Production\Face-2.tif");
        let mut incoming = provenance(output, r"C:\Design\Face-2.png", 'a');
        incoming.recipe.engine_mode = ConversionEngineMode::DeviceLink;
        incoming.recipe.target.output_profile_identity = None;
        incoming.recipe.target.output_profile_path = None;
        incoming.recipe.target.device_link_identity = Some(identity("DeviceLink", 'd'));
        incoming.recipe.target.device_link_path = Some(r"C:\Color\PressLink.icc".to_owned());

        let error = validate_existing_production_project_for_append(
            &project,
            Path::new(r"C:\Design\Source.shade"),
            &incoming,
        )
        .expect_err("engine family mismatch must fail");
        assert!(error.contains("conversion engine differs"));
    }

    #[test]
    fn duplicate_output_and_source_link_mismatch_are_rejected() {
        let mut project = production_project();
        let duplicate = provenance(
            Path::new(r"C:\Production\Face-1.tif"),
            r"C:\Design\Face-2.png",
            'a',
        );
        let error = append_converted_face_to_production_project(
            &mut project,
            AppendConvertedFaceSpec {
                source_project_path: Path::new(r"C:\Design\Source.shade"),
                output_face_label: "Face 2",
                provenance: duplicate,
            },
        )
        .expect_err("duplicate output must fail");
        assert!(error.contains("already present"));

        let output = Path::new(r"C:\Production\Face-2.tif");
        let incoming = provenance(output, r"C:\Design\Face-2.png", 'a');
        let error = validate_existing_production_project_for_append(
            &project,
            Path::new(r"C:\Design\Other.shade"),
            &incoming,
        )
        .expect_err("different Source project must fail");
        assert!(error.contains("not linked") || error.contains("different Source"));
    }
}
