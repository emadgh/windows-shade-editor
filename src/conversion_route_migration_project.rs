use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::color_conversion::{ConversionSourceRef, ProductionProvenance};
use crate::conversion_route_migration::RouteMigrationPlan;
use crate::conversion_route_migration_checkpoint::RouteMigrationExecutionStage;
use crate::conversion_route_migration_executor::{
    RouteMigrationRecoveryJournal, cleanup_completed_route_migration_outputs,
    persist_route_migration_journal, route_migration_backup_path,
};
use crate::conversion_transaction::{CommittedConversionOutput, ConversionJobCapture};
use crate::icc_conversion_worker::sha256_file;
use crate::model::ShadeProject;
use crate::production_project::{ProductionProjectSpec, build_production_project};
use crate::production_project_compat::{
    AppendConvertedFaceSpec, append_converted_face_to_production_project_at_path,
    validate_existing_production_project_baseline_at_path,
};
use crate::safe_fs;

/// Finish a fully swapped route migration by rebuilding and atomically saving the complete
/// Production `.shade` project. This is also the recovery entry point for a crash after the
/// project save but before the migration checkpoint was marked complete.
pub fn complete_route_migration_project(
    journal: &mut RouteMigrationRecoveryJournal,
) -> Result<ShadeProject, String> {
    journal.validate()?;
    if journal.checkpoint.stage == RouteMigrationExecutionStage::Complete {
        let project = ShadeProject::load(&journal.plan.production_project_path).map_err(|error| {
            format!(
                "Route migration is marked complete but the Production project cannot be reopened: {error}"
            )
        })?;
        // Once Complete is durable, old-output backups are cleanup transients rather than recovery
        // dependencies. A previous cleanup attempt may already have removed some/all backups before
        // crashing, so validate only the migrated project and canonical final TIFFs here.
        validate_completed_migrated_project(journal, &project)?;
        cleanup_completed_route_migration_outputs(journal)?;
        return Ok(project);
    }
    if journal.checkpoint.stage != RouteMigrationExecutionStage::ProductionProjectSavePending {
        return Err(
            "Route migration Production project cannot be saved before every TIFF swap is durably checkpointed."
                .to_owned(),
        );
    }

    verify_committed_route_outputs(journal)?;
    let (original_project, current_is_original) = load_original_production_project(journal)?;
    validate_original_project_against_capture(journal, &original_project)?;
    let expected = build_migrated_production_project(journal, &original_project)?;
    validate_expected_new_project(journal, &expected)?;

    if current_is_original {
        // Re-check immediately before entering the existing atomic .shade save boundary. The save
        // keeps the original project in its normal `.bak`, which is also sufficient to verify a
        // crash that happened after the project replacement but before checkpoint completion.
        verify_project_sha(
            &journal.plan.production_project_path,
            &journal.plan.intent.expected_project_sha256,
            "immediately before route migration project save",
        )?;
        let faces = expected.resolve_face_paths(&journal.plan.production_project_path);
        expected
            .save(&journal.plan.production_project_path, &faces)
            .map_err(|error| format!("Cannot save migrated Production project: {error}"))?;
    } else {
        // The current project no longer has the old captured hash. Only accept it if the old
        // `.shade.bak` is exact and the current project equals the deterministic reconstruction
        // from that exact old project. This recognizes save-success/checkpoint-failure without
        // treating unrelated edits as migration success.
        let current = ShadeProject::load(&journal.plan.production_project_path).map_err(|error| {
            format!(
                "Production project changed after migration capture and cannot be verified: {error}"
            )
        })?;
        ensure_projects_equivalent_at_path(
            &current,
            &expected,
            &journal.plan.production_project_path,
        )?;
    }

    let persisted = ShadeProject::load(&journal.plan.production_project_path).map_err(|error| {
        format!("Cannot reopen migrated Production project after save: {error}")
    })?;
    ensure_projects_equivalent_at_path(
        &persisted,
        &expected,
        &journal.plan.production_project_path,
    )?;
    validate_saved_migrated_project(journal, &persisted)?;

    journal.checkpoint.mark_complete(&journal.plan)?;
    persist_route_migration_journal(journal)?;
    cleanup_completed_route_migration_outputs(journal)?;
    Ok(persisted)
}

/// Deterministically reconstruct the Production project that must exist after migration.
///
/// Same target compatibility retains Production-side adjustment/Snapshot state after the operator
/// has confirmed the route replacement risk. A target-space migration rebuilds a clean Production
/// project in the new channel topology. Explicit discard permission is required only when the
/// frozen migration plan proves that real Production-side work would be discarded.
pub fn build_migrated_production_project(
    journal: &RouteMigrationRecoveryJournal,
    original: &ShadeProject,
) -> Result<ShadeProject, String> {
    journal.validate()?;
    if original.faces.len() != journal.plan.faces.len()
        || original.production_provenance.len() != journal.plan.faces.len()
    {
        return Err(
            "Original Production project no longer matches the frozen route Face count."
                .to_owned(),
        );
    }
    if journal.checkpoint.committed_outputs.len() != journal.plan.faces.len() {
        return Err(
            "Cannot reconstruct migrated Production project before every TIFF is committed."
                .to_owned(),
        );
    }

    let same_compatibility = journal.plan.intent.previous_compatibility
        == journal.plan.intent.new_compatibility;
    let provenances = journal
        .plan
        .faces
        .iter()
        .zip(journal.checkpoint.committed_outputs.iter())
        .map(|(face, committed)| replacement_provenance(&face.replacement, committed))
        .collect::<Result<Vec<_>, _>>()?;

    if same_compatibility {
        let mut migrated = original.clone();
        for (ordinal, provenance) in provenances.into_iter().enumerate() {
            let planned = &journal.plan.faces[ordinal];
            if planned.production_face_index != ordinal {
                return Err(
                    "Route migration project reconstruction requires stable Production Face order."
                        .to_owned(),
                );
            }
            migrated.faces[ordinal].path = provenance.output_path.clone();
            migrated.production_provenance[ordinal] = provenance;
        }
        return Ok(migrated);
    }

    if journal.plan.requires_production_work_discard
        && !journal.plan.intent.allow_production_work_discard
    {
        return Err(
            "Target-space route migration requires explicit permission to discard Production-side adjustment/Snapshot state."
                .to_owned(),
        );
    }

    let first_provenance = provenances
        .first()
        .cloned()
        .ok_or_else(|| "Route migration has no committed Production Faces.".to_owned())?;
    let first_capture = &journal.plan.faces[0].replacement;
    let project_name = if original.name.trim().is_empty() {
        first_capture.production_project_name.as_str()
    } else {
        original.name.as_str()
    };
    let first_label = original
        .faces
        .first()
        .map(|face| face.label.as_str())
        .unwrap_or(first_capture.output_face_label.as_str());
    let mut migrated = build_production_project(ProductionProjectSpec {
        project_name,
        source_project_path: &journal.plan.source_project_path,
        output_tiff_path: Path::new(&first_provenance.output_path),
        output_face_label: first_label,
        provenance: first_provenance.clone(),
    })?;

    for ordinal in 1..provenances.len() {
        let label = original
            .faces
            .get(ordinal)
            .map(|face| face.label.as_str())
            .unwrap_or(journal.plan.faces[ordinal].replacement.output_face_label.as_str());
        append_converted_face_to_production_project_at_path(
            &mut migrated,
            &journal.plan.production_project_path,
            AppendConvertedFaceSpec {
                source_project_path: &journal.plan.source_project_path,
                output_face_label: label,
                provenance: provenances[ordinal].clone(),
            },
        )?;
    }
    Ok(migrated)
}

fn replacement_provenance(
    capture: &ConversionJobCapture,
    committed: &crate::conversion_route_migration_checkpoint::CommittedRouteMigrationOutput,
) -> Result<ProductionProvenance, String> {
    if !paths_match(&capture.output_tiff_path, &committed.final_path)
        || !capture
            .conversion_recipe_sha256
            .eq_ignore_ascii_case(&crate::conversion_recipe::recipe_sha256(
                &capture.conversion_recipe,
            )?)
    {
        return Err(
            "Committed migration output no longer matches its immutable replacement capture."
                .to_owned(),
        );
    }
    let custom_optimizer = match capture.custom_optimizer_evidence.as_ref() {
        Some(evidence) => Some(
            evidence
                .production_provenance(&capture.conversion_recipe_sha256)
                .map_err(|errors| {
                    format!(
                        "Cannot persist migrated measured Custom Optimizer provenance: {}",
                        errors.join(" ")
                    )
                })?,
        ),
        None => None,
    };
    let profile_backed_optimizer = match capture.profile_backed_optimizer_execution.as_ref() {
        Some(execution) => Some(
            execution
                .production_provenance(&capture.conversion_recipe_sha256)
                .map_err(|errors| {
                    format!(
                        "Cannot persist migrated profile-backed Custom Optimizer provenance: {}",
                        errors.join(" ")
                    )
                })?,
        ),
        None => None,
    };
    Ok(ProductionProvenance {
        source: ConversionSourceRef {
            source_project_path: capture.source_project_path.display().to_string(),
            source_face_path: capture.source_face_path.display().to_string(),
            source_snapshot_id: capture.source_snapshot_id,
            source_file_sha256: capture.source_file_sha256.clone(),
        },
        recipe: capture.conversion_recipe.clone(),
        custom_optimizer,
        profile_backed_optimizer,
        output_path: committed.final_path.display().to_string(),
        output_sha256: committed.new_sha256.clone(),
        converted_at_unix_ms: committed.converted_at_unix_ms,
    })
}

fn verify_committed_final_outputs(journal: &RouteMigrationRecoveryJournal) -> Result<(), String> {
    for committed in &journal.checkpoint.committed_outputs {
        let actual_final = sha256_file(&committed.final_path).map_err(|error| {
            format!(
                "Cannot verify migrated Production TIFF {}: {error}",
                committed.final_path.display()
            )
        })?;
        if !actual_final.eq_ignore_ascii_case(committed.new_sha256.trim()) {
            return Err(format!(
                "Migrated Production TIFF {} no longer matches the committed migration identity.",
                committed.final_path.display()
            ));
        }
    }
    Ok(())
}

fn verify_committed_route_outputs(journal: &RouteMigrationRecoveryJournal) -> Result<(), String> {
    verify_committed_final_outputs(journal)?;
    for committed in &journal.checkpoint.committed_outputs {
        let actual_backup = sha256_file(&committed.backup_path).map_err(|error| {
            format!(
                "Cannot verify previous-output migration backup {}: {error}",
                committed.backup_path.display()
            )
        })?;
        if !actual_backup.eq_ignore_ascii_case(committed.previous_sha256.trim()) {
            return Err(format!(
                "Previous-output migration backup {} no longer matches captured route ownership.",
                committed.backup_path.display()
            ));
        }
    }
    Ok(())
}

fn load_original_production_project(
    journal: &RouteMigrationRecoveryJournal,
) -> Result<(ShadeProject, bool), String> {
    let current_sha = sha256_file(&journal.plan.production_project_path).map_err(|error| {
        format!(
            "Cannot verify Production project before migration project save: {error}"
        )
    })?;
    if current_sha.eq_ignore_ascii_case(journal.plan.intent.expected_project_sha256.trim()) {
        let project = ShadeProject::load(&journal.plan.production_project_path)?;
        return Ok((project, true));
    }

    let backup = safe_fs::backup_path(&journal.plan.production_project_path);
    verify_project_sha(
        &backup,
        &journal.plan.intent.expected_project_sha256,
        "while recovering a route migration project save",
    )?;
    let original = ShadeProject::load(&backup).map_err(|error| {
        format!(
            "Cannot load exact pre-migration Production project backup {}: {error}",
            backup.display()
        )
    })?;
    Ok((original, false))
}

fn validate_original_project_against_capture(
    journal: &RouteMigrationRecoveryJournal,
    original: &ShadeProject,
) -> Result<(), String> {
    let compatibility = validate_existing_production_project_baseline_at_path(
        original,
        &journal.plan.production_project_path,
        &journal.plan.source_project_path,
    )?;
    if !journal
        .plan
        .intent
        .previous_compatibility
        .matches_runtime(&compatibility)
    {
        return Err(
            "Pre-migration Production project target compatibility no longer matches the captured route."
                .to_owned(),
        );
    }
    for (ordinal, previous) in original.production_provenance.iter().enumerate() {
        let planned = journal.plan.faces.get(ordinal).ok_or_else(|| {
            "Pre-migration Production project has an unexpected Face count.".to_owned()
        })?;
        let previous_recipe = crate::conversion_recipe::recipe_sha256(&previous.recipe)?;
        if !previous_recipe.eq_ignore_ascii_case(&planned.previous_recipe_sha256)
            || !previous
                .output_sha256
                .eq_ignore_ascii_case(&planned.previous_output_sha256)
            || !paths_match(
                Path::new(&previous.source.source_face_path),
                &planned.replacement.source_face_path,
            )
            || !paths_match(
                Path::new(&previous.output_path),
                &planned.replacement.output_tiff_path,
            )
        {
            return Err(format!(
                "Pre-migration Production Face {} no longer matches frozen route ownership/provenance.",
                ordinal + 1
            ));
        }
    }
    Ok(())
}

fn validate_expected_new_project(
    journal: &RouteMigrationRecoveryJournal,
    project: &ShadeProject,
) -> Result<(), String> {
    let compatibility = validate_existing_production_project_baseline_at_path(
        project,
        &journal.plan.production_project_path,
        &journal.plan.source_project_path,
    )?;
    if !journal
        .plan
        .intent
        .new_compatibility
        .matches_runtime(&compatibility)
    {
        return Err(
            "Reconstructed migrated Production project does not match captured new target compatibility."
                .to_owned(),
        );
    }
    if project.production_provenance.len() != journal.plan.faces.len() {
        return Err("Migrated Production project has an unexpected provenance count.".to_owned());
    }
    for (ordinal, provenance) in project.production_provenance.iter().enumerate() {
        let planned = &journal.plan.faces[ordinal];
        let committed = &journal.checkpoint.committed_outputs[ordinal];
        let recipe = crate::conversion_recipe::recipe_sha256(&provenance.recipe)?;
        if !recipe.eq_ignore_ascii_case(&planned.new_recipe_sha256)
            || !provenance
                .output_sha256
                .eq_ignore_ascii_case(&committed.new_sha256)
            || !paths_match(
                Path::new(&provenance.output_path),
                &planned.replacement.output_tiff_path,
            )
        {
            return Err(format!(
                "Migrated Production Face {} does not match its planned recipe/output identity.",
                ordinal + 1
            ));
        }
    }
    Ok(())
}

fn validate_saved_migrated_project(
    journal: &RouteMigrationRecoveryJournal,
    project: &ShadeProject,
) -> Result<(), String> {
    validate_expected_new_project(journal, project)?;
    verify_committed_route_outputs(journal)
}

fn validate_completed_migrated_project(
    journal: &RouteMigrationRecoveryJournal,
    project: &ShadeProject,
) -> Result<(), String> {
    validate_expected_new_project(journal, project)?;
    verify_committed_final_outputs(journal)
}

fn ensure_projects_equivalent_at_path(
    actual: &ShadeProject,
    expected: &ShadeProject,
    project_path: &Path,
) -> Result<(), String> {
    let actual = normalized_project_value(actual, project_path)?;
    let expected = normalized_project_value(expected, project_path)?;
    if actual != expected {
        return Err(
            "Current Production project is not the deterministic migrated project; automatic recovery is blocked."
                .to_owned(),
        );
    }
    Ok(())
}

fn normalized_project_value(project: &ShadeProject, project_path: &Path) -> Result<Value, String> {
    let mut normalized = project.clone();
    let resolved = project.resolve_face_paths(project_path);
    for (face, path) in normalized.faces.iter_mut().zip(resolved) {
        face.path = path.to_string_lossy().into_owned();
    }
    serde_json::to_value(normalized)
        .map_err(|error| format!("Cannot normalize Production project for recovery comparison: {error}"))
}

fn verify_project_sha(path: &Path, expected: &str, operation: &str) -> Result<(), String> {
    let actual = sha256_file(path).map_err(|error| {
        format!(
            "Cannot verify Production project {} {operation}: {error}",
            path.display()
        )
    })?;
    if !actual.eq_ignore_ascii_case(expected.trim()) {
        return Err(format!(
            "Production project {} changed {operation}; route migration recovery is blocked.",
            path.display()
        ));
    }
    Ok(())
}

fn paths_match(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .replace('/', "\\")
        .eq_ignore_ascii_case(&right.to_string_lossy().replace('/', "\\"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
        ConversionRenderingIntent, ConversionSourceRef, ConversionTargetDefinition,
        SeparationStrategy, TargetChannelDefinition,
    };
    use crate::conversion_recipe::recipe_sha256;
    use crate::conversion_route_migration::{RouteMigrationFacePlan, RouteMigrationPlan};
    use crate::conversion_route_migration_checkpoint::{
        CommittedRouteMigrationOutput, RouteMigrationCheckpoint,
    };
    use crate::conversion_transaction::{CapturedOutputPolicy, CapturedSourceProfile};
    use crate::model::IccProfileIdentity;
    use crate::production_project_compat::ProductionCompatibilityKey;
    use crate::production_project_disposition::{
        CapturedRouteFaceOwnership, RouteMigrationCapture,
    };

    fn hash(character: char) -> String {
        assert!(character.is_ascii());
        format!("{:02x}", character as u8).repeat(32)
    }

    fn recipe(target_hash: char, source_hash: char) -> ConversionRecipe {
        ConversionRecipe {
            source_transparency_policy: None,
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: hash(source_hash),
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
                    sha256: hash(target_hash),
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

    fn compatibility(target_hash: char) -> ProductionCompatibilityKey {
        ProductionCompatibilityKey {
            engine_mode: ConversionEngineMode::Icc,
            output_profile_sha256: Some(hash(target_hash)),
            device_link_sha256: None,
            characterization_id: None,
            channel_names: vec![
                "Cyan".to_owned(),
                "Magenta".to_owned(),
                "Yellow".to_owned(),
                "Black".to_owned(),
            ],
            bit_depth: 16,
        }
    }

    fn journal(target_changes: bool) -> (RouteMigrationRecoveryJournal, ShadeProject) {
        let output = PathBuf::from(r"C:\Production\Face.tif");
        let source = PathBuf::from(r"C:\Design\Face.tif");
        let project_path = PathBuf::from(r"C:\Production\Job.shade");
        let source_project = PathBuf::from(r"C:\Design\Source.shade");
        let old_target = 'p';
        let new_target = if target_changes { 'n' } else { 'p' };
        let old_recipe = recipe(old_target, 'a');
        let new_recipe = recipe(new_target, 'b');
        let old_provenance = ProductionProvenance {
            source: ConversionSourceRef {
                source_project_path: source_project.display().to_string(),
                source_face_path: source.display().to_string(),
                source_snapshot_id: Some(1),
                source_file_sha256: hash('s'),
            },
            recipe: old_recipe.clone(),
            custom_optimizer: None,
            profile_backed_optimizer: None,
            output_path: output.display().to_string(),
            output_sha256: hash('o'),
            converted_at_unix_ms: 1,
        };
        let original = build_production_project(ProductionProjectSpec {
            project_name: "Production",
            source_project_path: &source_project,
            output_tiff_path: &output,
            output_face_label: "Face 1",
            provenance: old_provenance,
        })
        .unwrap();
        let replacement = ConversionJobCapture::capture(
            &ShadeProject::default(),
            source_project.clone(),
            hash('q'),
            source.clone(),
            Some(2),
            hash('s'),
            CapturedSourceProfile::Embedded,
            new_recipe.clone(),
            CapturedOutputPolicy::TransactionalReplace,
            output.clone(),
            project_path.clone(),
            "Production".to_owned(),
            "Face 1".to_owned(),
        )
        .unwrap();
        let old_recipe_sha = recipe_sha256(&old_recipe).unwrap();
        let new_recipe_sha = recipe_sha256(&new_recipe).unwrap();
        let old_policy = crate::conversion_batch::batch_recipe_policy_sha256(&old_recipe).unwrap();
        let new_policy = crate::conversion_batch::batch_recipe_policy_sha256(&new_recipe).unwrap();
        let intent = RouteMigrationCapture::capture(
            hash('j'),
            &compatibility(old_target),
            &compatibility(new_target),
            old_policy,
            new_policy,
            vec![CapturedRouteFaceOwnership {
                source_face_path: source.display().to_string(),
                output_path: output.display().to_string(),
                previous_recipe_sha256: old_recipe_sha.clone(),
            }],
            0,
            1,
            true,
            target_changes,
        )
        .unwrap();
        let plan = RouteMigrationPlan {
            intent,
            source_project_path: source_project,
            production_project_path: project_path,
            faces: vec![RouteMigrationFacePlan {
                production_face_index: 0,
                replacement,
                previous_output_sha256: hash('o'),
                previous_recipe_sha256: old_recipe_sha,
                new_recipe_sha256: new_recipe_sha,
            }],
            requires_production_work_discard: false,
        };
        let mut checkpoint = RouteMigrationCheckpoint::default();
        checkpoint.staged_outputs.push(
            crate::conversion_route_migration_checkpoint::StagedRouteMigrationOutput {
                ordinal: 0,
                staged_path: PathBuf::from(r"C:\Production\.stage.tif"),
                sha256: hash('x'),
                converted_at_unix_ms: 20,
            },
        );
        checkpoint.stage = RouteMigrationExecutionStage::CommitPending;
        checkpoint.committed_outputs.push(CommittedRouteMigrationOutput {
            ordinal: 0,
            final_path: output,
            backup_path: PathBuf::from(r"C:\Production\.old.tif"),
            previous_sha256: hash('o'),
            new_sha256: hash('x'),
            converted_at_unix_ms: 20,
        });
        checkpoint.stage = RouteMigrationExecutionStage::ProductionProjectSavePending;
        let journal = RouteMigrationRecoveryJournal {
            schema_version: crate::conversion_route_migration_executor::ROUTE_MIGRATION_JOURNAL_SCHEMA_VERSION,
            plan,
            checkpoint,
        };
        (journal, original)
    }

    #[test]
    fn same_target_migration_preserves_production_adjustments_and_snapshots() {
        let (journal, mut original) = journal(false);
        original.adjustments.get_mut("Black").unwrap().levels.gamma = 0.9;
        let snapshot_id = original.create_snapshot();
        let migrated = build_migrated_production_project(&journal, &original).unwrap();
        assert_eq!(migrated.adjustments, original.adjustments);
        assert_eq!(migrated.snapshots.len(), 1);
        assert_eq!(migrated.active_snapshot_id, Some(snapshot_id));
        assert_eq!(
            recipe_sha256(&migrated.production_provenance[0].recipe).unwrap(),
            journal.plan.faces[0].new_recipe_sha256
        );
    }

    #[test]
    fn clean_target_space_migration_does_not_require_discard_permission() {
        let (mut journal, original) = journal(true);
        journal.plan.intent.allow_production_work_discard = false;
        assert!(!journal.plan.requires_production_work_discard);
        let migrated = build_migrated_production_project(&journal, &original).unwrap();
        assert!(migrated.snapshots.is_empty());
        assert_eq!(migrated.adjustments.get("Black").unwrap().levels.gamma, 1.0);
        assert_eq!(
            migrated.production_provenance[0]
                .recipe
                .target
                .output_profile_identity
                .as_ref()
                .unwrap()
                .sha256,
            hash('n')
        );
    }

    #[test]
    fn target_space_migration_rebuilds_clean_production_state() {
        let (journal, mut original) = journal(true);
        original.adjustments.get_mut("Black").unwrap().levels.gamma = 0.9;
        original.create_snapshot();
        let migrated = build_migrated_production_project(&journal, &original).unwrap();
        assert!(migrated.snapshots.is_empty());
        assert_eq!(migrated.adjustments.get("Black").unwrap().levels.gamma, 1.0);
        assert_eq!(
            migrated.production_provenance[0]
                .recipe
                .target
                .output_profile_identity
                .as_ref()
                .unwrap()
                .sha256,
            hash('n')
        );
    }
}
