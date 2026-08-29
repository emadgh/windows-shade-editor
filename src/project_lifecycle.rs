use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectTransition {
    New,
    Open(PathBuf),
    Exit,
    Recover,
}

impl ProjectTransition {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::New => "create a new project",
            Self::Open(_) => "open another project",
            Self::Exit => "exit Shade Editor",
            Self::Recover => "replace the current state with recovery data",
        }
    }

    pub fn action_label(&self) -> &'static str {
        match self {
            Self::New => "create new",
            Self::Open(_) => "open",
            Self::Exit => "exit",
            Self::Recover => "recover",
        }
    }

    /// Queued exports own immutable export recipes and can continue safely when
    /// the active project changes. Only transitions that terminate the process
    /// must wait for the queue to become idle.
    pub fn blocks_on_export_queue(&self) -> bool {
        matches!(self, Self::Exit)
    }
}

#[derive(Clone, Debug)]
pub struct BackupRestoreCandidate {
    pub primary_path: PathBuf,
    pub backup_path: PathBuf,
    pub primary_error: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransitionRequest {
    BlockedByOperation,
    BlockedByExportQueue,
    AwaitingConfirmation,
    Execute(ProjectTransition),
}

#[derive(Clone, Debug)]
pub struct ProjectLifecycleController {
    pub pending: Option<ProjectTransition>,
    pub after_save: Option<ProjectTransition>,
    pub allow_close_once: bool,
    pub session_id: u64,
    pub opening_path: Option<PathBuf>,
    pub backup_restore: Option<BackupRestoreCandidate>,
}

fn startup_project_argument() -> Option<PathBuf> {
    std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("shade"))
        })
}

fn prepare_transition_authority(transition: &ProjectTransition) {
    match transition {
        ProjectTransition::Open(path) => {
            // Failure is intentionally non-fatal for Open itself. Persistence records the failed
            // authority attempt and blocks same-path Save, so malformed/missing/racing projects
            // can still surface their normal Open/backup diagnostics without risking overwrite.
            let _ = crate::project_persistence::prepare_active_source_project_open(path);
        }
        ProjectTransition::New | ProjectTransition::Recover | ProjectTransition::Exit => {
            crate::project_persistence::disarm_prepared_active_source_project_open();
        }
    }
}

impl Default for ProjectLifecycleController {
    fn default() -> Self {
        // Startup/double-click Open bypasses request() and calls open_project_path directly. Stage
        // its exact-byte evidence while the controller is constructed, before the background Open
        // worker can read the project. Normal in-app Open transitions are staged by request().
        if let Some(path) = startup_project_argument() {
            let _ = crate::project_persistence::prepare_active_source_project_open(&path);
        } else {
            crate::project_persistence::disarm_prepared_active_source_project_open();
        }
        Self {
            pending: None,
            after_save: None,
            allow_close_once: false,
            session_id: 1,
            opening_path: None,
            backup_restore: None,
        }
    }
}

impl ProjectLifecycleController {
    pub fn request(
        &mut self,
        transition: ProjectTransition,
        operation_busy: bool,
        export_queue_pending: bool,
        project_dirty: bool,
        has_faces: bool,
        has_saved_path: bool,
    ) -> TransitionRequest {
        if operation_busy {
            return TransitionRequest::BlockedByOperation;
        }
        if export_queue_pending && transition.blocks_on_export_queue() {
            return TransitionRequest::BlockedByExportQueue;
        }

        // Prepare Open evidence before either immediate execution or a dirty-project confirmation
        // dialog. If the operator spends time in that dialog and the target changes, the generation
        // comparison will fail closed. Discard-and-open refreshes this evidence in cancel_pending(),
        // while Save-and-open refreshes it after the successful Save.
        prepare_transition_authority(&transition);

        if requires_save_confirmation(project_dirty, has_faces, has_saved_path) {
            self.pending = Some(transition);
            return TransitionRequest::AwaitingConfirmation;
        }
        TransitionRequest::Execute(transition)
    }

    pub fn begin_save_then(&mut self, transition: ProjectTransition) {
        self.pending = None;
        self.after_save = Some(transition);
    }

    pub fn save_failed(&mut self) {
        if let Some(transition) = self.after_save.take() {
            self.pending = Some(transition);
        }
    }

    pub fn take_after_successful_save(
        &mut self,
        operation_busy: bool,
        project_dirty: bool,
    ) -> Option<ProjectTransition> {
        if operation_busy || project_dirty {
            return None;
        }
        let transition = self.after_save.take()?;
        // The Save may have taken long enough for the requested Open target to change. Refresh the
        // candidate at the last lifecycle boundary before execution; later changes are detected by
        // generation/fingerprint verification when the new project session is installed.
        prepare_transition_authority(&transition);
        Some(transition)
    }

    pub fn cancel_pending(&mut self) {
        // This method serves both Cancel and Discard-and-continue in the current UI. Refreshing an
        // Open here makes Discard-and-open precise. A plain Cancel may leave inert prepared evidence,
        // but it cannot arm Save authority because only a later project-session bump can finalize it;
        // every subsequent transition replaces or disarms it first.
        if let Some(transition) = self.pending.as_ref() {
            prepare_transition_authority(transition);
        }
        self.pending = None;
        self.after_save = None;
    }

    pub fn bump_session(&mut self) -> u64 {
        // New/Open/Recovery change the active project identity. Persistence first clears the prior
        // exact-byte baseline, then accepts a prepared Open candidate only if the file still has the
        // same SHA-256, path identity and filesystem generation captured before Open. New/Recovery
        // arrive with no prepared candidate and therefore remain deliberately unarmed.
        let _ = crate::project_persistence::rotate_active_source_project_session();
        self.session_id = self.session_id.wrapping_add(1).max(1);
        self.session_id
    }
}

/// Never-saved projects with Faces are protected even if the dirty bit was
/// accidentally cleared. This is intentionally stricter than a classic dirty-bit guard.
pub fn requires_save_confirmation(
    project_dirty: bool,
    has_faces: bool,
    has_saved_path: bool,
) -> bool {
    project_dirty || (has_faces && !has_saved_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_or_never_saved_face_project_is_protected() {
        assert!(requires_save_confirmation(true, false, true));
        assert!(requires_save_confirmation(false, true, false));
    }

    #[test]
    fn clean_saved_or_empty_project_can_transition() {
        assert!(!requires_save_confirmation(false, true, true));
        assert!(!requires_save_confirmation(false, false, false));
    }

    #[test]
    fn new_open_and_exit_share_the_same_dirty_guard() {
        for transition in [
            ProjectTransition::New,
            ProjectTransition::Open(PathBuf::from("other.shade")),
            ProjectTransition::Exit,
        ] {
            let mut lifecycle = ProjectLifecycleController::default();
            assert_eq!(
                lifecycle.request(transition.clone(), false, false, true, true, true),
                TransitionRequest::AwaitingConfirmation
            );
            assert_eq!(lifecycle.pending, Some(transition));
        }
    }

    #[test]
    fn active_operation_blocks_every_destructive_transition() {
        for transition in [
            ProjectTransition::New,
            ProjectTransition::Open(PathBuf::from("other.shade")),
            ProjectTransition::Exit,
            ProjectTransition::Recover,
        ] {
            let mut lifecycle = ProjectLifecycleController::default();
            assert_eq!(
                lifecycle.request(transition, true, false, false, false, false),
                TransitionRequest::BlockedByOperation
            );
            assert!(lifecycle.pending.is_none());
        }
    }

    #[test]
    fn export_queue_allows_project_switch_but_blocks_exit() {
        for transition in [
            ProjectTransition::New,
            ProjectTransition::Open(PathBuf::from("other.shade")),
            ProjectTransition::Recover,
        ] {
            let mut lifecycle = ProjectLifecycleController::default();
            assert_eq!(
                lifecycle.request(transition.clone(), false, true, false, false, false,),
                TransitionRequest::Execute(transition)
            );
        }

        let mut lifecycle = ProjectLifecycleController::default();
        assert_eq!(
            lifecycle.request(ProjectTransition::Exit, false, true, false, false, false),
            TransitionRequest::BlockedByExportQueue
        );
        assert!(lifecycle.pending.is_none());
    }

    #[test]
    fn dirty_project_still_requires_confirmation_while_queue_is_active() {
        let transition = ProjectTransition::Open(PathBuf::from("other.shade"));
        let mut lifecycle = ProjectLifecycleController::default();
        assert_eq!(
            lifecycle.request(transition.clone(), false, true, true, true, true),
            TransitionRequest::AwaitingConfirmation
        );
        assert_eq!(lifecycle.pending, Some(transition));
    }

    #[test]
    fn failed_save_restores_pending_transition() {
        let mut lifecycle = ProjectLifecycleController::default();
        lifecycle.begin_save_then(ProjectTransition::Open(PathBuf::from("next.shade")));
        lifecycle.save_failed();
        assert_eq!(
            lifecycle.pending,
            Some(ProjectTransition::Open(PathBuf::from("next.shade")))
        );
        assert!(lifecycle.after_save.is_none());
    }

    #[test]
    fn startup_project_argument_only_accepts_shade_extension() {
        assert!(Path::new("project.shade").extension().is_some());
        assert_eq!(
            Path::new("project.shade")
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("shade")
        );
    }
}
