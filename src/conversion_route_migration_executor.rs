use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::conversion_route_migration::RouteMigrationPlan;
use crate::conversion_route_migration_checkpoint::{
    RouteMigrationCheckpoint, RouteMigrationExecutionStage, RouteMigrationFileObservation,
    RouteMigrationRecoveryAction, classify_route_migration_recovery,
};
use crate::conversion_transaction::{
    CapturedOutputPolicy, CommittedConversionOutput, ConversionCancellation, ConversionJobCapture,
    ConversionProgress, ConversionTransactionBackend,
};
use crate::icc_conversion_worker::{FilesystemIccConversionBackend, sha256_file};
use crate::safe_fs;

pub const ROUTE_MIGRATION_JOURNAL_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteMigrationRecoveryJournal {
    pub schema_version: u32,
    pub plan: RouteMigrationPlan,
    pub checkpoint: RouteMigrationCheckpoint,
}

impl RouteMigrationRecoveryJournal {
    pub fn new(plan: RouteMigrationPlan) -> Result<Self, String> {
        plan.validate()?;
        let journal = Self {
            schema_version: ROUTE_MIGRATION_JOURNAL_SCHEMA_VERSION,
            plan,
            checkpoint: RouteMigrationCheckpoint::default(),
        };
        journal.validate()?;
        Ok(journal)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != ROUTE_MIGRATION_JOURNAL_SCHEMA_VERSION {
            return Err(format!(
                "Unsupported route migration recovery journal schema {}.",
                self.schema_version
            ));
        }
        self.plan.validate()?;
        self.checkpoint.validate_for(&self.plan)
    }
}

pub trait RouteMigrationStagingBackend {
    fn stage_replacement(
        &mut self,
        capture: &ConversionJobCapture,
        staged_path: &Path,
        cancellation: &ConversionCancellation,
        report: &mut dyn FnMut(ConversionProgress),
    ) -> Result<CommittedConversionOutput, String>;
}

pub struct FilesystemRouteMigrationStagingBackend {
    backend: FilesystemIccConversionBackend,
}

impl FilesystemRouteMigrationStagingBackend {
    pub fn new(default_dpi: f64) -> Result<Self, String> {
        Ok(Self {
            backend: FilesystemIccConversionBackend::new(default_dpi)?,
        })
    }
}

impl RouteMigrationStagingBackend for FilesystemRouteMigrationStagingBackend {
    fn stage_replacement(
        &mut self,
        capture: &ConversionJobCapture,
        staged_path: &Path,
        cancellation: &ConversionCancellation,
        report: &mut dyn FnMut(ConversionProgress),
    ) -> Result<CommittedConversionOutput, String> {
        let mut staged_capture = capture.clone();
        staged_capture.output_policy = CapturedOutputPolicy::MustNotExist;
        staged_capture.output_tiff_path = staged_path.to_path_buf();
        staged_capture.validate()?;
        self.backend
            .render_convert_and_commit(&staged_capture, cancellation, report)
    }
}

/// Create the durable recovery journal before any replacement TIFF is rendered or moved.
/// Existing project/output identities are reverified here so stale UI/planner state cannot enter
/// the destructive boundary.
pub fn initialize_route_migration_journal(
    plan: RouteMigrationPlan,
) -> Result<RouteMigrationRecoveryJournal, String> {
    plan.validate()?;
    verify_project_identity(&plan)?;
    verify_uncommitted_previous_outputs(&plan, 0)?;
    let journal = RouteMigrationRecoveryJournal::new(plan)?;
    let path = route_migration_journal_path(&journal.plan);
    if path.exists() {
        return Err(format!(
            "Route migration recovery journal already exists at {}. Recover or resolve it before starting another migration.",
            path.display()
        ));
    }
    persist_route_migration_journal(&journal)?;
    Ok(journal)
}

/// Continue staging and TIFF commit to the project-save boundary.
///
/// Cancellation is honored while rendering staged outputs. Once every replacement is staged and
/// `CommitPending` is durably journaled, the function intentionally finishes the short output-swap
/// boundary instead of introducing a user-cancelled mixed route.
pub fn continue_route_migration_outputs<B, F>(
    journal: &mut RouteMigrationRecoveryJournal,
    backend: &mut B,
    cancellation: &ConversionCancellation,
    mut report: F,
) -> Result<(), String>
where
    B: RouteMigrationStagingBackend,
    F: FnMut(usize, usize, ConversionProgress),
{
    journal.validate()?;
    verify_project_identity(&journal.plan)?;

    if journal.checkpoint.stage == RouteMigrationExecutionStage::Staging {
        verify_recorded_staging_prefix(journal)?;
        while let Some(ordinal) = journal.checkpoint.next_staging_ordinal(&journal.plan)? {
            cancellation.check_before_commit()?;
            verify_project_identity(&journal.plan)?;
            verify_previous_output(&journal.plan, ordinal)?;
            let staged_path = route_migration_staged_path(&journal.plan, ordinal)?;
            let backup_path = route_migration_backup_path(&journal.plan, ordinal)?;
            if backup_path.exists() {
                return Err(format!(
                    "Unexpected route migration backup exists before commit: {}.",
                    backup_path.display()
                ));
            }
            if staged_path.exists() {
                // An uncheckpointed staging file is never trusted because its exact output SHA was
                // not made durable. The old final is still verified above, so deleting only this
                // transient file and rerendering is safe.
                fs::remove_file(&staged_path).map_err(|error| {
                    format!(
                        "Cannot remove uncheckpointed route migration staging file {}: {error}",
                        staged_path.display()
                    )
                })?;
            }
            let total = journal.plan.faces.len();
            let capture = &journal.plan.faces[ordinal].replacement;
            let mut face_report = |progress| report(ordinal, total, progress);
            let staged = backend.stage_replacement(
                capture,
                &staged_path,
                cancellation,
                &mut face_report,
            )?;
            if !paths_match(&staged.path, &staged_path) || !is_sha256(&staged.sha256) {
                return Err(
                    "Route migration staging backend returned an invalid output identity."
                        .to_owned(),
                );
            }
            // The staging backend has already validated, durably committed, and hashed this exact
            // path before returning. Do not immediately re-read a 200–300 MB TIFF solely to derive
            // the same SHA again. The durable checkpoint records that identity, and every stage is
            // re-observed from disk before the destructive swap boundary below (including after a
            // restart), so later mutation still fails closed before the old final is replaced.
            journal.checkpoint.record_staged(
                &journal.plan,
                staged_path,
                staged.sha256,
                staged.converted_at_unix_ms,
            )?;
            persist_route_migration_journal(journal)?;
        }
        journal.checkpoint.begin_commit(&journal.plan)?;
        persist_route_migration_journal(journal)?;
    }

    if journal.checkpoint.stage == RouteMigrationExecutionStage::CommitPending {
        // Do not honor cancellation after this durable boundary. All normal expensive transforms
        // are done; if recovery discovers that a stage must be reconstructed, that restage is part
        // of completing the already-committed migration transaction and must likewise ignore a
        // stale operator cancellation request.
        let commit_boundary_cancellation = ConversionCancellation::default();
        while let Some(ordinal) = journal.checkpoint.next_commit_ordinal(&journal.plan)? {
            verify_project_identity(&journal.plan)?;
            recover_or_commit_one_output(
                journal,
                backend,
                &commit_boundary_cancellation,
                ordinal,
                &mut report,
            )?;
        }
        journal
            .checkpoint
            .mark_project_save_pending(&journal.plan)?;
        persist_route_migration_journal(journal)?;
    }

    journal.validate()
}

pub fn load_route_migration_journal(
    path: &Path,
) -> Result<RouteMigrationRecoveryJournal, String> {
    let bytes = fs::read(path).map_err(|error| {
        format!(
            "Cannot read route migration recovery journal {}: {error}",
            path.display()
        )
    })?;
    let journal: RouteMigrationRecoveryJournal = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "Cannot parse route migration recovery journal {}: {error}",
            path.display()
        )
    })?;
    journal.validate()?;
    if !paths_match(path, &route_migration_journal_path(&journal.plan)) {
        return Err(
            "Route migration journal path does not match its immutable Production project identity."
                .to_owned(),
        );
    }
    Ok(journal)
}

pub fn persist_route_migration_journal(
    journal: &RouteMigrationRecoveryJournal,
) -> Result<(), String> {
    journal.validate()?;
    let bytes = serde_json::to_vec_pretty(journal)
        .map_err(|error| format!("Cannot serialize route migration recovery journal: {error}"))?;
    safe_fs::atomic_write(&route_migration_journal_path(&journal.plan), &bytes, None)
}

/// The durable journal follows the Production project because it describes the project-wide
/// migration transaction. Per-output stage/backup files deliberately do not use this parent.
pub fn route_migration_journal_path(plan: &RouteMigrationPlan) -> PathBuf {
    project_migration_sibling_path(plan, "journal", "json")
}

/// Staging must be a sibling of the exact final TIFF. `safe_fs::commit_staged_file` rejects
/// cross-directory/cross-volume publication, so deriving this path from the Production `.shade`
/// folder would make otherwise valid routes fail when their project and TIFF folders differ.
pub fn route_migration_staged_path(
    plan: &RouteMigrationPlan,
    ordinal: usize,
) -> Result<PathBuf, String> {
    output_migration_sibling_path(plan, ordinal, "stage")
}

/// The previous-output backup is also a sibling of its final TIFF. This keeps old→backup and
/// staged→final on one local filesystem namespace and lets both moves use write-through boundaries.
pub fn route_migration_backup_path(
    plan: &RouteMigrationPlan,
    ordinal: usize,
) -> Result<PathBuf, String> {
    output_migration_sibling_path(plan, ordinal, "old")
}

pub fn cleanup_completed_route_migration_outputs(
    journal: &RouteMigrationRecoveryJournal,
) -> Result<(), String> {
    journal.validate()?;
    if journal.checkpoint.stage != RouteMigrationExecutionStage::Complete {
        return Err("Route migration cleanup is allowed only after project completion.".to_owned());
    }
    for ordinal in 0..journal.plan.faces.len() {
        let staged = route_migration_staged_path(&journal.plan, ordinal)?;
        if staged.exists() {
            fs::remove_file(&staged).map_err(|error| {
                format!(
                    "Cannot remove completed route migration staging file {}: {error}",
                    staged.display()
                )
            })?;
        }
        let backup = route_migration_backup_path(&journal.plan, ordinal)?;
        if backup.exists() {
            fs::remove_file(&backup).map_err(|error| {
                format!(
                    "Cannot remove completed route migration backup {}: {error}",
                    backup.display()
                )
            })?;
        }
    }
    let journal_path = route_migration_journal_path(&journal.plan);
    if journal_path.exists() {
        fs::remove_file(&journal_path).map_err(|error| {
            format!(
                "Cannot remove completed route migration journal {}: {error}",
                journal_path.display()
            )
        })?;
    }
    Ok(())
}

fn recover_or_commit_one_output<B, F>(
    journal: &mut RouteMigrationRecoveryJournal,
    backend: &mut B,
    cancellation: &ConversionCancellation,
    ordinal: usize,
    report: &mut F,
) -> Result<(), String>
where
    B: RouteMigrationStagingBackend,
    F: FnMut(usize, usize, ConversionProgress),
{
    let staged_path = route_migration_staged_path(&journal.plan, ordinal)?;
    let backup_path = route_migration_backup_path(&journal.plan, ordinal)?;
    let final_path = journal.plan.faces[ordinal]
        .replacement
        .output_tiff_path
        .clone();
    let observation = RouteMigrationFileObservation {
        final_sha256: optional_sha256(&final_path)?,
        staged_sha256: optional_sha256(&staged_path)?,
        backup_sha256: optional_sha256(&backup_path)?,
    };
    match classify_route_migration_recovery(
        &journal.plan,
        &journal.checkpoint,
        ordinal,
        &observation,
    )? {
        RouteMigrationRecoveryAction::ReadyToCommit => {
            move_previous_output_to_backup(
                &final_path,
                &backup_path,
                &journal.plan.faces[ordinal].previous_output_sha256,
            )?;
            commit_staged_output(
                &staged_path,
                &final_path,
                &journal.checkpoint.staged_outputs[ordinal].sha256,
            )?;
        }
        RouteMigrationRecoveryAction::FinishInterruptedSwap => {
            commit_staged_output(
                &staged_path,
                &final_path,
                &journal.checkpoint.staged_outputs[ordinal].sha256,
            )?;
        }
        RouteMigrationRecoveryAction::RecordAlreadyCommitted => {}
        RouteMigrationRecoveryAction::Restage => {
            if backup_path.exists() {
                return Err(
                    "Route migration cannot restage after previous output backup has begun."
                        .to_owned(),
                );
            }
            verify_previous_output(&journal.plan, ordinal)?;
            if staged_path.exists() {
                fs::remove_file(&staged_path).map_err(|error| {
                    format!(
                        "Cannot remove invalid route migration staging file {}: {error}",
                        staged_path.display()
                    )
                })?;
            }
            let total = journal.plan.faces.len();
            let capture = &journal.plan.faces[ordinal].replacement;
            let mut face_report = |progress| report(ordinal, total, progress);
            let staged = backend.stage_replacement(
                capture,
                &staged_path,
                cancellation,
                &mut face_report,
            )?;
            // Restage is different from the initial non-destructive staging pass: the next
            // operation moves the old final into its backup. Keep this pre-boundary readback so a
            // faulty backend or external mutation cannot advance into the destructive swap.
            let actual = sha256_file(&staged_path)?;
            if !actual.eq_ignore_ascii_case(staged.sha256.trim()) {
                return Err("Restaged route migration TIFF failed SHA verification.".to_owned());
            }
            journal.checkpoint.staged_outputs[ordinal].sha256 = staged.sha256.to_ascii_lowercase();
            journal.checkpoint.staged_outputs[ordinal].converted_at_unix_ms = staged.converted_at_unix_ms;
            persist_route_migration_journal(journal)?;
            move_previous_output_to_backup(
                &final_path,
                &backup_path,
                &journal.plan.faces[ordinal].previous_output_sha256,
            )?;
            commit_staged_output(
                &staged_path,
                &final_path,
                &journal.checkpoint.staged_outputs[ordinal].sha256,
            )?;
        }
        RouteMigrationRecoveryAction::AlreadyCheckpointed => {
            return Err(
                "Route migration next-commit cursor points at an output already checkpointed."
                    .to_owned(),
            );
        }
    }
    journal
        .checkpoint
        .record_committed(&journal.plan, backup_path)?;
    persist_route_migration_journal(journal)
}

fn verify_recorded_staging_prefix(journal: &RouteMigrationRecoveryJournal) -> Result<(), String> {
    for staged in &journal.checkpoint.staged_outputs {
        verify_previous_output(&journal.plan, staged.ordinal)?;
        let actual = optional_sha256(&staged.staged_path)?;
        if !actual.is_some_and(|hash| hash.eq_ignore_ascii_case(staged.sha256.trim())) {
            return Err(format!(
                "Checkpointed route migration staging TIFF {} is missing or changed; recover before continuing.",
                staged.staged_path.display()
            ));
        }
        let backup = route_migration_backup_path(&journal.plan, staged.ordinal)?;
        if backup.exists() {
            return Err(format!(
                "Route migration backup {} exists while checkpoint is still staging.",
                backup.display()
            ));
        }
    }
    Ok(())
}

fn verify_project_identity(plan: &RouteMigrationPlan) -> Result<(), String> {
    let actual = sha256_file(&plan.production_project_path).map_err(|error| {
        format!(
            "Cannot verify Production project {} before route migration: {error}",
            plan.production_project_path.display()
        )
    })?;
    if !actual.eq_ignore_ascii_case(plan.intent.expected_project_sha256.trim()) {
        return Err(
            "Production project changed after route migration capture; destructive migration is blocked."
                .to_owned(),
        );
    }
    Ok(())
}

fn verify_uncommitted_previous_outputs(
    plan: &RouteMigrationPlan,
    start_ordinal: usize,
) -> Result<(), String> {
    for ordinal in start_ordinal..plan.faces.len() {
        verify_previous_output(plan, ordinal)?;
    }
    Ok(())
}

fn verify_previous_output(plan: &RouteMigrationPlan, ordinal: usize) -> Result<(), String> {
    let face = plan
        .faces
        .get(ordinal)
        .ok_or_else(|| "Route migration previous-output ordinal is outside the plan.".to_owned())?;
    let actual = sha256_file(&face.replacement.output_tiff_path).map_err(|error| {
        format!(
            "Cannot verify previous Production TIFF {}: {error}",
            face.replacement.output_tiff_path.display()
        )
    })?;
    if !actual.eq_ignore_ascii_case(face.previous_output_sha256.trim()) {
        return Err(format!(
            "Production TIFF {} no longer matches captured route ownership; migration overwrite is blocked.",
            face.replacement.output_tiff_path.display()
        ));
    }
    Ok(())
}

fn move_previous_output_to_backup(
    final_path: &Path,
    backup_path: &Path,
    expected_sha256: &str,
) -> Result<(), String> {
    if backup_path.exists() {
        return Err(format!(
            "Route migration backup destination is already occupied: {}.",
            backup_path.display()
        ));
    }
    let actual = sha256_file(final_path)?;
    if !actual.eq_ignore_ascii_case(expected_sha256.trim()) {
        return Err(
            "Production TIFF changed immediately before backup; migration commit is blocked."
                .to_owned(),
        );
    }
    safe_fs::commit_staged_file_if_absent(final_path, backup_path).map_err(|error| {
        format!(
            "Cannot move previous Production TIFF {} to durable migration backup {}: {error}",
            final_path.display(),
            backup_path.display()
        )
    })?;
    let backup_sha = sha256_file(backup_path)?;
    if !backup_sha.eq_ignore_ascii_case(expected_sha256.trim()) {
        return Err(
            "Route migration backup bytes do not match the captured previous output."
                .to_owned(),
        );
    }
    Ok(())
}

fn commit_staged_output(
    staged_path: &Path,
    final_path: &Path,
    expected_sha256: &str,
) -> Result<(), String> {
    let staged_sha = sha256_file(staged_path)?;
    if !staged_sha.eq_ignore_ascii_case(expected_sha256.trim()) {
        return Err("Route migration staged TIFF changed before atomic commit.".to_owned());
    }
    safe_fs::commit_staged_file(staged_path, final_path)?;
    let final_sha = sha256_file(final_path)?;
    if !final_sha.eq_ignore_ascii_case(expected_sha256.trim()) {
        return Err("Route migration final TIFF failed post-commit SHA verification.".to_owned());
    }
    Ok(())
}

fn optional_sha256(path: &Path) -> Result<Option<String>, String> {
    if path.exists() {
        sha256_file(path).map(Some)
    } else {
        Ok(None)
    }
}

fn project_migration_sibling_path(
    plan: &RouteMigrationPlan,
    role: &str,
    extension: &str,
) -> PathBuf {
    let parent = plan
        .production_project_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    parent.join(migration_file_name(plan, role, extension))
}

fn output_migration_sibling_path(
    plan: &RouteMigrationPlan,
    ordinal: usize,
    role: &str,
) -> Result<PathBuf, String> {
    let face = plan
        .faces
        .get(ordinal)
        .ok_or_else(|| format!("Route migration {role} ordinal is outside the plan."))?;
    let parent = face
        .replacement
        .output_tiff_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    Ok(parent.join(migration_file_name(
        plan,
        &format!("{role}-{ordinal:04}"),
        "tif",
    )))
}

fn migration_file_name(plan: &RouteMigrationPlan, role: &str, extension: &str) -> String {
    let token = plan
        .intent
        .expected_project_sha256
        .get(..12)
        .unwrap_or("migration");
    format!(".shade-migrate-{token}-{role}.{extension}")
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn paths_match(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
        ConversionRenderingIntent, ConversionTargetDefinition, SeparationStrategy,
        TargetChannelDefinition,
    };
    use crate::conversion_recipe::recipe_sha256;
    use crate::conversion_route_migration::{RouteMigrationFacePlan, RouteMigrationPlan};
    use crate::model::{IccProfileIdentity, ShadeProject};
    use crate::production_project_compat::ProductionCompatibilityKey;
    use crate::production_project_disposition::{
        CapturedRouteFaceOwnership, RouteMigrationCapture,
    };

    fn hash_bytes(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn hash(character: char) -> String {
        assert!(character.is_ascii());
        format!("{:02x}", character as u8).repeat(32)
    }

    fn recipe(source_hash: char) -> ConversionRecipe {
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

    fn compatibility() -> ProductionCompatibilityKey {
        ProductionCompatibilityKey {
            engine_mode: ConversionEngineMode::Icc,
            output_profile_sha256: Some(hash('p')),
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

    fn temp_folder(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shade-route-migrate-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn plan(folder: &Path, project_bytes: &[u8], old_output: &[u8]) -> RouteMigrationPlan {
        let source_path = folder.join("Source.shade");
        let source_face = folder.join("Source-Face.tif");
        let production_path = folder.join("Production.shade");
        let final_path = folder.join("Face.tif");
        let old_recipe = recipe('a');
        let new_recipe = recipe('b');
        let old_recipe_sha = recipe_sha256(&old_recipe).unwrap();
        let new_recipe_sha = recipe_sha256(&new_recipe).unwrap();
        let replacement = ConversionJobCapture::capture(
            &ShadeProject::default(),
            source_path.clone(),
            hash('s'),
            source_face.clone(),
            Some(1),
            hash('f'),
            crate::conversion_transaction::CapturedSourceProfile::Embedded,
            new_recipe,
            CapturedOutputPolicy::TransactionalReplace,
            final_path.clone(),
            production_path.clone(),
            "Production".to_owned(),
            "Face".to_owned(),
        )
        .unwrap();
        let policy = crate::conversion_batch::batch_recipe_policy_sha256(&old_recipe).unwrap();
        let intent = RouteMigrationCapture::capture(
            hash_bytes(project_bytes),
            &compatibility(),
            &compatibility(),
            policy.clone(),
            policy,
            vec![CapturedRouteFaceOwnership {
                source_face_path: source_face.display().to_string(),
                output_path: final_path.display().to_string(),
                previous_recipe_sha256: old_recipe_sha.clone(),
            }],
            0,
            1,
            true,
            false,
        )
        .unwrap();
        RouteMigrationPlan {
            intent,
            source_project_path: source_path,
            production_project_path: production_path,
            faces: vec![RouteMigrationFacePlan {
                production_face_index: 0,
                replacement,
                previous_output_sha256: hash_bytes(old_output),
                previous_recipe_sha256: old_recipe_sha,
                new_recipe_sha256: new_recipe_sha,
            }],
            requires_production_work_discard: false,
        }
    }

    struct MockStager {
        bytes: Vec<u8>,
    }

    impl RouteMigrationStagingBackend for MockStager {
        fn stage_replacement(
            &mut self,
            _capture: &ConversionJobCapture,
            staged_path: &Path,
            cancellation: &ConversionCancellation,
            _report: &mut dyn FnMut(ConversionProgress),
        ) -> Result<CommittedConversionOutput, String> {
            cancellation.check_before_commit()?;
            fs::write(staged_path, &self.bytes).map_err(|error| error.to_string())?;
            OpenOptions::new()
                .write(true)
                .open(staged_path)
                .and_then(|file| file.sync_all())
                .map_err(|error| error.to_string())?;
            Ok(CommittedConversionOutput {
                path: staged_path.to_path_buf(),
                sha256: hash_bytes(&self.bytes),
                converted_at_unix_ms: 123,
            })
        }
    }

    struct MismatchedShaStager {
        bytes: Vec<u8>,
        claimed_sha256: String,
        calls: usize,
    }

    impl RouteMigrationStagingBackend for MismatchedShaStager {
        fn stage_replacement(
            &mut self,
            _capture: &ConversionJobCapture,
            staged_path: &Path,
            cancellation: &ConversionCancellation,
            _report: &mut dyn FnMut(ConversionProgress),
        ) -> Result<CommittedConversionOutput, String> {
            cancellation.check_before_commit()?;
            self.calls += 1;
            fs::write(staged_path, &self.bytes).map_err(|error| error.to_string())?;
            OpenOptions::new()
                .write(true)
                .open(staged_path)
                .and_then(|file| file.sync_all())
                .map_err(|error| error.to_string())?;
            Ok(CommittedConversionOutput {
                path: staged_path.to_path_buf(),
                sha256: self.claimed_sha256.clone(),
                converted_at_unix_ms: 123,
            })
        }
    }

    #[test]
    fn migration_output_transients_follow_owned_tiff_not_project_folder() {
        let project_folder = temp_folder("split-project");
        let output_folder = temp_folder("split-output");
        let mut plan = plan(&project_folder, b"old-project", b"old-output");
        plan.faces[0].replacement.output_tiff_path = output_folder.join("Face.tif");

        assert_eq!(
            route_migration_journal_path(&plan).parent(),
            Some(project_folder.as_path())
        );
        assert_eq!(
            route_migration_staged_path(&plan, 0).unwrap().parent(),
            Some(output_folder.as_path())
        );
        assert_eq!(
            route_migration_backup_path(&plan, 0).unwrap().parent(),
            Some(output_folder.as_path())
        );
    }

    #[test]
    fn filesystem_boundary_stages_everything_then_swaps_with_backup() {
        let folder = temp_folder("full");
        fs::create_dir_all(&folder).unwrap();
        let project_bytes = b"old-project";
        let old_output = b"old-output";
        let new_output = b"new-output";
        let plan = plan(&folder, project_bytes, old_output);
        fs::write(&plan.production_project_path, project_bytes).unwrap();
        fs::write(&plan.faces[0].replacement.output_tiff_path, old_output).unwrap();

        let mut journal = initialize_route_migration_journal(plan.clone()).unwrap();
        let mut backend = MockStager {
            bytes: new_output.to_vec(),
        };
        continue_route_migration_outputs(
            &mut journal,
            &mut backend,
            &ConversionCancellation::default(),
            |_ordinal, _total, _progress| {},
        )
        .unwrap();

        assert_eq!(
            journal.checkpoint.stage,
            RouteMigrationExecutionStage::ProductionProjectSavePending
        );
        assert_eq!(
            fs::read(&plan.faces[0].replacement.output_tiff_path).unwrap(),
            new_output
        );
        assert_eq!(
            fs::read(route_migration_backup_path(&plan, 0).unwrap()).unwrap(),
            old_output
        );
        assert!(route_migration_journal_path(&plan).exists());
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn mismatched_staging_backend_sha_fails_before_old_output_moves() {
        let folder = temp_folder("mismatched-stage-sha");
        fs::create_dir_all(&folder).unwrap();
        let project_bytes = b"old-project";
        let old_output = b"old-output";
        let corrupt_output = b"corrupt-new-output";
        let plan = plan(&folder, project_bytes, old_output);
        fs::write(&plan.production_project_path, project_bytes).unwrap();
        fs::write(&plan.faces[0].replacement.output_tiff_path, old_output).unwrap();

        let mut journal = initialize_route_migration_journal(plan.clone()).unwrap();
        let mut backend = MismatchedShaStager {
            bytes: corrupt_output.to_vec(),
            claimed_sha256: hash_bytes(b"claimed-new-output"),
            calls: 0,
        };
        let error = continue_route_migration_outputs(
            &mut journal,
            &mut backend,
            &ConversionCancellation::default(),
            |_ordinal, _total, _progress| {},
        )
        .expect_err("A restaged TIFF whose bytes do not match the backend identity must fail");

        assert!(error.contains("Restaged route migration TIFF failed SHA verification"));
        assert_eq!(backend.calls, 2);
        assert_eq!(
            fs::read(&plan.faces[0].replacement.output_tiff_path).unwrap(),
            old_output
        );
        assert!(!route_migration_backup_path(&plan, 0).unwrap().exists());
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn recovery_finishes_crash_after_old_output_was_moved_to_backup() {
        let folder = temp_folder("swap-recovery");
        fs::create_dir_all(&folder).unwrap();
        let project_bytes = b"old-project";
        let old_output = b"old-output";
        let new_output = b"new-output";
        let plan = plan(&folder, project_bytes, old_output);
        fs::write(&plan.production_project_path, project_bytes).unwrap();
        fs::write(&plan.faces[0].replacement.output_tiff_path, old_output).unwrap();
        let mut journal = initialize_route_migration_journal(plan.clone()).unwrap();
        let staged_path = route_migration_staged_path(&plan, 0).unwrap();
        fs::write(&staged_path, new_output).unwrap();
        journal
            .checkpoint
            .record_staged(&plan, staged_path.clone(), hash_bytes(new_output), 123)
            .unwrap();
        journal.checkpoint.begin_commit(&plan).unwrap();
        persist_route_migration_journal(&journal).unwrap();
        let backup_path = route_migration_backup_path(&plan, 0).unwrap();
        safe_fs::commit_staged_file_if_absent(
            &plan.faces[0].replacement.output_tiff_path,
            &backup_path,
        )
        .unwrap();

        let mut backend = MockStager { bytes: Vec::new() };
        continue_route_migration_outputs(
            &mut journal,
            &mut backend,
            &ConversionCancellation::default(),
            |_ordinal, _total, _progress| {},
        )
        .unwrap();

        assert_eq!(
            fs::read(&plan.faces[0].replacement.output_tiff_path).unwrap(),
            new_output
        );
        assert_eq!(fs::read(&backup_path).unwrap(), old_output);
        assert_eq!(
            journal.checkpoint.stage,
            RouteMigrationExecutionStage::ProductionProjectSavePending
        );
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn commit_pending_restage_ignores_stale_operator_cancellation() {
        let folder = temp_folder("commit-pending-restage-cancel");
        fs::create_dir_all(&folder).unwrap();
        let project_bytes = b"old-project";
        let old_output = b"old-output";
        let new_output = b"new-output";
        let plan = plan(&folder, project_bytes, old_output);
        fs::write(&plan.production_project_path, project_bytes).unwrap();
        fs::write(&plan.faces[0].replacement.output_tiff_path, old_output).unwrap();

        let mut journal = initialize_route_migration_journal(plan.clone()).unwrap();
        let staged_path = route_migration_staged_path(&plan, 0).unwrap();
        fs::write(&staged_path, new_output).unwrap();
        journal
            .checkpoint
            .record_staged(&plan, staged_path.clone(), hash_bytes(new_output), 123)
            .unwrap();
        journal.checkpoint.begin_commit(&plan).unwrap();
        persist_route_migration_journal(&journal).unwrap();
        fs::remove_file(&staged_path).unwrap();

        let cancellation = ConversionCancellation::default();
        cancellation.request();
        let mut backend = MockStager {
            bytes: new_output.to_vec(),
        };
        continue_route_migration_outputs(
            &mut journal,
            &mut backend,
            &cancellation,
            |_ordinal, _total, _progress| {},
        )
        .unwrap();

        assert_eq!(
            journal.checkpoint.stage,
            RouteMigrationExecutionStage::ProductionProjectSavePending
        );
        assert_eq!(
            fs::read(&plan.faces[0].replacement.output_tiff_path).unwrap(),
            new_output
        );
        assert_eq!(
            fs::read(route_migration_backup_path(&plan, 0).unwrap()).unwrap(),
            old_output
        );
        let _ = fs::remove_dir_all(folder);
    }
}
