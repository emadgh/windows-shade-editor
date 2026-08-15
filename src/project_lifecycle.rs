use std::path::PathBuf;

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

impl Default for ProjectLifecycleController {
    fn default() -> Self {
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
        self.after_save.take()
    }

    pub fn cancel_pending(&mut self) {
        self.pending = None;
        self.after_save = None;
    }

    pub fn bump_session(&mut self) -> u64 {
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
                lifecycle.request(
                    transition.clone(),
                    false,
                    true,
                    false,
                    false,
                    false,
                ),
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
}
