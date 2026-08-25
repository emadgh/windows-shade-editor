use std::path::Path;

use crate::conversion_route_migration::RouteMigrationPlan;
use crate::conversion_route_migration_checkpoint::RouteMigrationExecutionStage;
use crate::conversion_route_migration_discovery::{
    discover_pending_route_migration, initialize_route_migration_if_clear,
};
use crate::conversion_route_migration_executor::{
    FilesystemRouteMigrationStagingBackend, RouteMigrationRecoveryJournal,
    continue_route_migration_outputs,
};
use crate::conversion_route_migration_project::complete_route_migration_project;
use crate::conversion_transaction::{ConversionCancellation, ConversionProgress};
use crate::model::ShadeProject;

/// Execute a newly captured project-wide destructive route migration through the complete
/// recovery-safe boundary: durable journal -> stage every TIFF -> swap every TIFF -> atomically
/// save the homogeneous Production project -> cleanup migration transients.
///
/// The caller must already have obtained explicit destructive-route confirmation (and, when
/// required by the plan, Production-work discard confirmation). The immutable plan validates
/// those confirmations again before any filesystem mutation begins.
pub fn execute_new_route_migration<F>(
    plan: RouteMigrationPlan,
    default_dpi: f64,
    cancellation: &ConversionCancellation,
    report: F,
) -> Result<ShadeProject, String>
where
    F: FnMut(usize, usize, ConversionProgress),
{
    let mut journal = initialize_route_migration_if_clear(plan)?;
    drive_route_migration(&mut journal, default_dpi, cancellation, report)
}

/// Resume the exact unfinished journal that owns `production_project_path`.
///
/// Recovery never recaptures the current UI state. It continues only the immutable journal that
/// was persisted before destructive work began, which prevents a post-crash settings change from
/// silently redefining the migration that is being recovered.
pub fn resume_route_migration<F>(
    production_project_path: &Path,
    default_dpi: f64,
    cancellation: &ConversionCancellation,
    report: F,
) -> Result<ShadeProject, String>
where
    F: FnMut(usize, usize, ConversionProgress),
{
    let mut journal = discover_pending_route_migration(production_project_path)?.ok_or_else(|| {
        format!(
            "No unfinished route migration journal exists for {}.",
            production_project_path.display()
        )
    })?;
    drive_route_migration(&mut journal, default_dpi, cancellation, report)
}

/// Drive one already validated journal from its durable checkpoint to completion.
///
/// Output work is intentionally skipped once the journal reaches `ProductionProjectSavePending`.
/// This matters for the crash window after a successful `.shade` replacement: the current project
/// SHA is then the *new* SHA, so re-entering the old-project output preflight would incorrectly
/// reject a valid project-save recovery. `complete_route_migration_project` handles that window by
/// proving the old `.shade.bak` identity and deterministic reconstructed new project instead.
pub fn drive_route_migration<F>(
    journal: &mut RouteMigrationRecoveryJournal,
    default_dpi: f64,
    cancellation: &ConversionCancellation,
    mut report: F,
) -> Result<ShadeProject, String>
where
    F: FnMut(usize, usize, ConversionProgress),
{
    journal.validate()?;

    if stage_requires_output_runtime(journal.checkpoint.stage) {
        let mut backend = FilesystemRouteMigrationStagingBackend::new(default_dpi)?;
        continue_route_migration_outputs(
            journal,
            &mut backend,
            cancellation,
            |ordinal, total, progress| report(ordinal, total, progress),
        )?;
    }

    match journal.checkpoint.stage {
        RouteMigrationExecutionStage::ProductionProjectSavePending
        | RouteMigrationExecutionStage::Complete => complete_route_migration_project(journal),
        RouteMigrationExecutionStage::Staging | RouteMigrationExecutionStage::CommitPending => Err(
            "Route migration output runtime returned before reaching the Production project-save boundary."
                .to_owned(),
        ),
    }
}

pub fn stage_requires_output_runtime(stage: RouteMigrationExecutionStage) -> bool {
    matches!(
        stage,
        RouteMigrationExecutionStage::Staging | RouteMigrationExecutionStage::CommitPending
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_save_recovery_does_not_reenter_old_project_output_preflight() {
        assert!(stage_requires_output_runtime(
            RouteMigrationExecutionStage::Staging
        ));
        assert!(stage_requires_output_runtime(
            RouteMigrationExecutionStage::CommitPending
        ));
        assert!(!stage_requires_output_runtime(
            RouteMigrationExecutionStage::ProductionProjectSavePending
        ));
        assert!(!stage_requires_output_runtime(
            RouteMigrationExecutionStage::Complete
        ));
    }
}
