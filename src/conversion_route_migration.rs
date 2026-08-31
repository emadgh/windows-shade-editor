use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::color_conversion::{ConversionEngineMode, ConversionRecipe};
use crate::conversion_batch::batch_recipe_policy_sha256;
use crate::conversion_recipe::recipe_sha256;
use crate::conversion_transaction::{CapturedOutputPolicy, ConversionJobCapture};
use crate::model::ShadeProject;
use crate::production_project_compat::{
    ProductionCompatibilityKey, validate_existing_production_project_baseline_at_path,
};
use crate::production_project_disposition::{
    CapturedRouteFaceOwnership, RouteMigrationCapture,
};
use crate::reconversion_policy::analyze_replacement_risk;

/// One complete, immutable project-wide route migration plan.
///
/// A migration is deliberately prepared for the entire existing Production route at once. The
/// executor must stage every `replacement` before replacing any final TIFF or the Production
/// project. This keeps the persisted Production project homogeneous: it must never contain a
/// partially migrated mix of old/new target compatibility.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteMigrationPlan {
    pub intent: RouteMigrationCapture,
    pub source_project_path: PathBuf,
    pub production_project_path: PathBuf,
    pub faces: Vec<RouteMigrationFacePlan>,
    pub requires_production_work_discard: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteMigrationFacePlan {
    pub production_face_index: usize,
    pub replacement: ConversionJobCapture,
    pub previous_output_sha256: String,
    pub previous_recipe_sha256: String,
    pub new_recipe_sha256: String,
}

impl RouteMigrationPlan {
    pub fn validate(&self) -> Result<(), String> {
        self.intent.validate()?;
        if self.source_project_path.as_os_str().is_empty()
            || self.production_project_path.as_os_str().is_empty()
        {
            return Err("Route migration plan requires Source and Production project paths.".to_owned());
        }
        if self.faces.len() != self.intent.migration_face_count
            || self.intent.route_faces.len() != self.faces.len()
        {
            return Err(
                "Route migration plan Face count diverges from the immutable migration intent."
                    .to_owned(),
            );
        }

        let mut sources = BTreeSet::new();
        let mut outputs = BTreeSet::new();
        let mut saw_recipe_change = false;
        for (ordinal, face) in self.faces.iter().enumerate() {
            face.replacement.validate()?;
            if face.production_face_index != ordinal {
                return Err(
                    "Route migration plan must preserve complete Production Face order.".to_owned(),
                );
            }
            if face.replacement.output_policy != CapturedOutputPolicy::TransactionalReplace {
                return Err(
                    "Route migration replacements must use transactional output replacement."
                        .to_owned(),
                );
            }
            if !paths_match(
                &face.replacement.source_project_path,
                &self.source_project_path,
            ) || !paths_match(
                &face.replacement.production_project_path,
                &self.production_project_path,
            ) {
                return Err(
                    "Route migration replacement capture targets a different Source/Production project."
                        .to_owned(),
                );
            }
            if !is_sha256(&face.previous_output_sha256)
                || !is_sha256(&face.previous_recipe_sha256)
                || !is_sha256(&face.new_recipe_sha256)
            {
                return Err(
                    "Route migration Face plan requires canonical output/recipe SHA-256 identities."
                        .to_owned(),
                );
            }
            let actual_new_recipe = recipe_sha256(&face.replacement.conversion_recipe)?;
            if !actual_new_recipe.eq_ignore_ascii_case(&face.new_recipe_sha256) {
                return Err(
                    "Route migration replacement recipe changed after plan capture.".to_owned(),
                );
            }
            let policy = batch_recipe_policy_sha256(&face.replacement.conversion_recipe)?;
            if !policy.eq_ignore_ascii_case(&self.intent.new_route_policy_sha256) {
                return Err(
                    "Route migration replacement no longer matches the captured new route policy."
                        .to_owned(),
                );
            }
            let compatibility = compatibility_from_recipe(&face.replacement.conversion_recipe)?;
            if !self.intent.new_compatibility.matches_runtime(&compatibility) {
                return Err(
                    "Route migration replacement no longer matches the captured new target compatibility."
                        .to_owned(),
                );
            }

            let owned = &self.intent.route_faces[ordinal];
            if !paths_match_str(&owned.source_face_path, &face.replacement.source_face_path)
                || !paths_match_str(&owned.output_path, &face.replacement.output_tiff_path)
                || !owned
                    .previous_recipe_sha256
                    .eq_ignore_ascii_case(&face.previous_recipe_sha256)
            {
                return Err(
                    "Route migration Face ownership diverges from the frozen migration intent."
                        .to_owned(),
                );
            }
            if !sources.insert(path_key(&face.replacement.source_face_path))
                || !outputs.insert(path_key(&face.replacement.output_tiff_path))
            {
                return Err(
                    "Route migration plan contains duplicate Source/output ownership.".to_owned(),
                );
            }
            saw_recipe_change |= !face
                .previous_recipe_sha256
                .eq_ignore_ascii_case(&face.new_recipe_sha256);
        }

        if !saw_recipe_change
            && self
                .intent
                .previous_route_policy_sha256
                .eq_ignore_ascii_case(&self.intent.new_route_policy_sha256)
        {
            return Err(
                "Selected Production route already matches the requested recipes; migration is unnecessary."
                    .to_owned(),
            );
        }
        if self.requires_production_work_discard && !self.intent.allow_production_work_discard {
            return Err(
                "Route migration would discard Production-side adjustments/Snapshots without explicit confirmation."
                    .to_owned(),
            );
        }
        Ok(())
    }
}

/// Freeze a destructive migration for the complete current Production route.
///
/// This is a planning boundary only. It does not modify TIFFs or the `.shade` file. Execution must
/// re-check `expected_project_sha256` and every `previous_output_sha256`, stage every replacement,
/// then atomically commit the complete route plus Production project/recovery record.
#[allow(clippy::too_many_arguments)]
pub fn prepare_route_migration_plan(
    production_project: &ShadeProject,
    production_project_path: &Path,
    expected_project_sha256: impl Into<String>,
    replacements: Vec<ConversionJobCapture>,
    confirm_destructive_migration: bool,
    allow_production_work_discard: bool,
) -> Result<RouteMigrationPlan, String> {
    let expected_project_sha256 = expected_project_sha256.into();
    if production_project.production_provenance.is_empty()
        || production_project.faces.len() != production_project.production_provenance.len()
    {
        return Err(
            "Route migration requires a complete Production Face/provenance route.".to_owned(),
        );
    }
    if replacements.len() != production_project.production_provenance.len() {
        return Err(format!(
            "Route migration is project-wide: captured {} replacement Face(s) for {} existing route Face(s).",
            replacements.len(),
            production_project.production_provenance.len()
        ));
    }

    let source_project_path = PathBuf::from(
        &production_project
            .production_provenance
            .first()
            .expect("non-empty provenance checked above")
            .source
            .source_project_path,
    );
    let previous_compatibility = validate_existing_production_project_baseline_at_path(
        production_project,
        production_project_path,
        &source_project_path,
    )?;
    let previous_route_policy_sha256 = batch_recipe_policy_sha256(
        &production_project.production_provenance[0].recipe,
    )?;
    for provenance in &production_project.production_provenance {
        let policy = batch_recipe_policy_sha256(&provenance.recipe)?;
        if !policy.eq_ignore_ascii_case(&previous_route_policy_sha256) {
            return Err(
                "Existing Production route contains multiple conversion policies; destructive migration is blocked until lineage is repaired."
                    .to_owned(),
            );
        }
    }

    let first_replacement = replacements
        .first()
        .ok_or_else(|| "Route migration requires replacement captures.".to_owned())?;
    let new_route_policy_sha256 =
        batch_recipe_policy_sha256(&first_replacement.conversion_recipe)?;
    let new_compatibility = compatibility_from_recipe(&first_replacement.conversion_recipe)?;

    let mut replacement_by_source = Vec::with_capacity(replacements.len());
    let mut replacement_sources = BTreeSet::new();
    for replacement in replacements {
        replacement.validate()?;
        if replacement.output_policy != CapturedOutputPolicy::TransactionalReplace {
            return Err(
                "Destructive route migration requires transactional replacement captures."
                    .to_owned(),
            );
        }
        if !paths_match(&replacement.source_project_path, &source_project_path)
            || !paths_match(&replacement.production_project_path, production_project_path)
        {
            return Err(
                "Route migration replacement capture references a different Source or Production project."
                    .to_owned(),
            );
        }
        let policy = batch_recipe_policy_sha256(&replacement.conversion_recipe)?;
        if !policy.eq_ignore_ascii_case(&new_route_policy_sha256) {
            return Err(
                "All route migration Faces must share one new target/engine/separation policy."
                    .to_owned(),
            );
        }
        let compatibility = compatibility_from_recipe(&replacement.conversion_recipe)?;
        if compatibility != new_compatibility {
            return Err(
                "All route migration Faces must share one new Production target compatibility."
                    .to_owned(),
            );
        }
        let key = path_key(&replacement.source_face_path);
        if !replacement_sources.insert(key) {
            return Err("Route migration contains duplicate replacement Source Faces.".to_owned());
        }
        replacement_by_source.push(replacement);
    }

    let mut route_faces = Vec::with_capacity(replacement_by_source.len());
    let mut faces = Vec::with_capacity(replacement_by_source.len());
    let mut requires_production_work_discard = false;
    for (production_face_index, previous) in production_project
        .production_provenance
        .iter()
        .enumerate()
    {
        let matches = replacement_by_source
            .iter()
            .enumerate()
            .filter(|(_, replacement)| {
                paths_match_str(
                    &previous.source.source_face_path,
                    &replacement.source_face_path,
                )
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let replacement_index = match matches.as_slice() {
            [index] => *index,
            [] => {
                return Err(format!(
                    "Route migration is missing replacement capture for Source Face {}.",
                    previous.source.source_face_path
                ));
            }
            _ => {
                return Err(format!(
                    "Route migration has ambiguous replacement captures for Source Face {}.",
                    previous.source.source_face_path
                ));
            }
        };
        let replacement = replacement_by_source[replacement_index].clone();
        if !paths_match_str(&previous.output_path, &replacement.output_tiff_path) {
            return Err(format!(
                "Route migration must preserve deterministic output ownership for Source Face {} (expected {}, captured {}).",
                previous.source.source_face_path,
                previous.output_path,
                replacement.output_tiff_path.display()
            ));
        }
        if !is_sha256(&previous.output_sha256) {
            return Err(format!(
                "Existing Production provenance for Source Face {} has no canonical output SHA-256.",
                previous.source.source_face_path
            ));
        }
        let previous_recipe_sha256 = recipe_sha256(&previous.recipe)?;
        let new_recipe_sha256 = recipe_sha256(&replacement.conversion_recipe)?;
        route_faces.push(CapturedRouteFaceOwnership {
            source_face_path: previous.source.source_face_path.clone(),
            output_path: previous.output_path.clone(),
            previous_recipe_sha256: previous_recipe_sha256.clone(),
        });
        let risk = analyze_replacement_risk(
            production_project,
            production_project_path,
            previous,
        )?;
        requires_production_work_discard |= risk.requires_explicit_confirmation;
        faces.push(RouteMigrationFacePlan {
            production_face_index,
            replacement,
            previous_output_sha256: previous.output_sha256.to_ascii_lowercase(),
            previous_recipe_sha256,
            new_recipe_sha256,
        });
    }

    if requires_production_work_discard && !allow_production_work_discard {
        return Err(
            "Route migration can invalidate Production-side adjustments/Snapshots. Explicit discard confirmation is required."
                .to_owned(),
        );
    }

    let intent = RouteMigrationCapture::capture(
        expected_project_sha256,
        &previous_compatibility,
        &new_compatibility,
        previous_route_policy_sha256,
        new_route_policy_sha256,
        route_faces,
        0,
        faces.len(),
        confirm_destructive_migration,
        allow_production_work_discard,
    )?;
    let plan = RouteMigrationPlan {
        intent,
        source_project_path,
        production_project_path: production_project_path.to_path_buf(),
        faces,
        requires_production_work_discard,
    };
    plan.validate()?;
    Ok(plan)
}

fn compatibility_from_recipe(recipe: &ConversionRecipe) -> Result<ProductionCompatibilityKey, String> {
    recipe.validate().map_err(|errors| {
        format!(
            "Invalid route migration replacement recipe: {}",
            errors.join(" ")
        )
    })?;
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

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn paths_match(left: &Path, right: &Path) -> bool {
    path_key(left) == path_key(right)
}

fn paths_match_str(left: &str, right: &Path) -> bool {
    path_key(Path::new(left)) == path_key(right)
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .trim()
        .replace('/', "\\")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionRenderingIntent, ConversionSourceRef,
        ConversionTargetDefinition, ProductionProvenance, SeparationStrategy,
        TargetChannelDefinition,
    };
    use crate::conversion_transaction::CapturedSourceProfile;
    use crate::model::{AdjustmentSnapshot, IccProfileIdentity, Levels, ShadeProject};
    use crate::production_project::{ProductionProjectSpec, build_production_project};
    use crate::production_project_compat::{
        AppendConvertedFaceSpec, append_converted_face_to_production_project_at_path,
    };

    fn hash(character: char) -> String {
        assert!(character.is_ascii());
        format!("{:02x}", character as u8).repeat(32)
    }

    fn identity(description: &str, character: char) -> IccProfileIdentity {
        IccProfileIdentity {
            description: description.to_owned(),
            sha256: hash(character),
        }
    }

    fn recipe(target_hash: char, source_hash: char) -> ConversionRecipe {
        ConversionRecipe {
            source_transparency_policy: None,
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: identity("Source RGB", source_hash),
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
                output_profile_identity: Some(identity("Press", target_hash)),
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

    fn provenance(source_face: &str, output: &str, target_hash: char, source_hash: char) -> ProductionProvenance {
        ProductionProvenance {
            source: ConversionSourceRef {
                source_project_path: r"C:\Design\Source.shade".to_owned(),
                source_face_path: source_face.to_owned(),
                source_snapshot_id: Some(7),
                source_file_sha256: hash('s'),
            },
            recipe: recipe(target_hash, source_hash),
            custom_optimizer: None,
            profile_backed_optimizer: None,
            output_path: output.to_owned(),
            output_sha256: hash('o'),
            converted_at_unix_ms: 10,
        }
    }

    fn production_project(two_faces: bool) -> ShadeProject {
        let project_path = Path::new(r"C:\Production\Job.shade");
        let first = provenance(
            r"C:\Design\Face-1.tif",
            r"C:\Production\Face-1.tif",
            'p',
            'a',
        );
        let first_output_path = first.output_path.clone();
        let mut project = build_production_project(ProductionProjectSpec {
            project_name: "Production",
            source_project_path: Path::new(r"C:\Design\Source.shade"),
            output_tiff_path: Path::new(&first_output_path),
            output_face_label: "Face 1",
            provenance: first,
        })
        .unwrap();
        if two_faces {
            append_converted_face_to_production_project_at_path(
                &mut project,
                project_path,
                AppendConvertedFaceSpec {
                    source_project_path: Path::new(r"C:\Design\Source.shade"),
                    output_face_label: "Face 2",
                    provenance: provenance(
                        r"C:\Design\Face-2.tif",
                        r"C:\Production\Face-2.tif",
                        'p',
                        'b',
                    ),
                },
            )
            .unwrap();
        }
        project
    }

    fn replacement(source_face: &str, output: &str, target_hash: char, source_hash: char) -> ConversionJobCapture {
        ConversionJobCapture::capture(
            &ShadeProject::default(),
            PathBuf::from(r"C:\Design\Source.shade"),
            hash('q'),
            PathBuf::from(source_face),
            Some(7),
            hash('s'),
            CapturedSourceProfile::Embedded,
            recipe(target_hash, source_hash),
            CapturedOutputPolicy::TransactionalReplace,
            PathBuf::from(output),
            PathBuf::from(r"C:\Production\Job.shade"),
            "Production".to_owned(),
            "Face".to_owned(),
        )
        .unwrap()
    }

    #[test]
    fn full_route_target_migration_freezes_old_bytes_and_old_new_recipe_identity() {
        let project = production_project(true);
        let plan = prepare_route_migration_plan(
            &project,
            Path::new(r"C:\Production\Job.shade"),
            hash('j'),
            vec![
                replacement(
                    r"C:\Design\Face-2.tif",
                    r"C:\Production\Face-2.tif",
                    'n',
                    'b',
                ),
                replacement(
                    r"C:\Design\Face-1.tif",
                    r"C:\Production\Face-1.tif",
                    'n',
                    'a',
                ),
            ],
            true,
            false,
        )
        .unwrap();

        assert_eq!(plan.faces.len(), 2);
        assert_eq!(plan.faces[0].production_face_index, 0);
        assert_eq!(plan.faces[0].previous_output_sha256, hash('o'));
        assert_ne!(
            plan.intent.previous_compatibility.output_profile_sha256,
            plan.intent.new_compatibility.output_profile_sha256
        );
        assert!(plan.intent.confirm_destructive_migration);
        assert!(plan.validate().is_ok());
    }

    #[test]
    fn partial_route_migration_is_rejected_before_any_write() {
        let project = production_project(true);
        let error = prepare_route_migration_plan(
            &project,
            Path::new(r"C:\Production\Job.shade"),
            hash('j'),
            vec![replacement(
                r"C:\Design\Face-1.tif",
                r"C:\Production\Face-1.tif",
                'n',
                'a',
            )],
            true,
            false,
        )
        .expect_err("partial migration must fail");
        assert!(error.contains("project-wide"));
    }

    #[test]
    fn migration_cannot_redirect_owned_output_path() {
        let project = production_project(false);
        let error = prepare_route_migration_plan(
            &project,
            Path::new(r"C:\Production\Job.shade"),
            hash('j'),
            vec![replacement(
                r"C:\Design\Face-1.tif",
                r"C:\Production\Other.tif",
                'n',
                'a',
            )],
            true,
            false,
        )
        .expect_err("redirected output must fail");
        assert!(error.contains("preserve deterministic output ownership"));
    }

    #[test]
    fn production_work_requires_separate_discard_confirmation() {
        let mut project = production_project(false);
        project.adjustments.get_mut("Black").unwrap().levels = Levels {
            gamma: 0.9,
            ..Levels::default()
        };
        project.snapshots.push(AdjustmentSnapshot {
            id: 1,
            name: "Production tweak".to_owned(),
            created_at_unix_ms: 1,
            adjustments: project.adjustments.clone(),
            exports: Vec::new(),
            history: Default::default(),
        });
        let error = prepare_route_migration_plan(
            &project,
            Path::new(r"C:\Production\Job.shade"),
            hash('j'),
            vec![replacement(
                r"C:\Design\Face-1.tif",
                r"C:\Production\Face-1.tif",
                'n',
                'a',
            )],
            true,
            false,
        )
        .expect_err("production work must require explicit discard confirmation");
        assert!(error.contains("Explicit discard confirmation"));
    }

    #[test]
    fn source_recipe_change_with_same_target_is_still_a_real_migration() {
        let project = production_project(false);
        let plan = prepare_route_migration_plan(
            &project,
            Path::new(r"C:\Production\Job.shade"),
            hash('j'),
            vec![replacement(
                r"C:\Design\Face-1.tif",
                r"C:\Production\Face-1.tif",
                'p',
                'z',
            )],
            true,
            false,
        )
        .unwrap();
        assert_eq!(
            plan.intent.previous_route_policy_sha256,
            plan.intent.new_route_policy_sha256
        );
        assert_ne!(
            plan.faces[0].previous_recipe_sha256,
            plan.faces[0].new_recipe_sha256
        );
    }
}
