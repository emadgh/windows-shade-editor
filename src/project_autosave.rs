use std::path::PathBuf;
use std::time::Duration;

pub const DEBOUNCE: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub struct Completion {
    pub revision: u64,
    pub path: PathBuf,
    pub result: Result<(), String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Eligibility {
    pub dirty: bool,
    pub has_project_path: bool,
    pub has_faces: bool,
    pub save_busy: bool,
    pub other_operation_busy: bool,
    pub transition_pending: bool,
    pub snapshot_choice_pending: bool,
    pub snapshot_has_unupdated_changes: bool,
    pub quiet_for: Duration,
}

pub fn should_start(value: Eligibility) -> bool {
    value.dirty
        && value.has_project_path
        && value.has_faces
        && !value.save_busy
        && !value.other_operation_busy
        && !value.transition_pending
        && !value.snapshot_choice_pending
        && !value.snapshot_has_unupdated_changes
        && value.quiet_for >= DEBOUNCE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> Eligibility {
        Eligibility {
            dirty: true,
            has_project_path: true,
            has_faces: true,
            quiet_for: DEBOUNCE,
            ..Default::default()
        }
    }

    #[test]
    fn autosave_requires_a_saved_project_and_two_seconds_of_quiet() {
        assert!(should_start(ready()));
        assert!(!should_start(Eligibility {
            has_project_path: false,
            ..ready()
        }));
        assert!(!should_start(Eligibility {
            quiet_for: Duration::from_millis(1999),
            ..ready()
        }));
    }

    #[test]
    fn autosave_never_silently_commits_a_stale_snapshot_or_modal_choice() {
        assert!(!should_start(Eligibility {
            snapshot_has_unupdated_changes: true,
            ..ready()
        }));
        assert!(!should_start(Eligibility {
            snapshot_choice_pending: true,
            ..ready()
        }));
        assert!(!should_start(Eligibility {
            transition_pending: true,
            ..ready()
        }));
    }

    #[test]
    fn autosave_does_not_race_another_save_or_operation() {
        assert!(!should_start(Eligibility {
            save_busy: true,
            ..ready()
        }));
        assert!(!should_start(Eligibility {
            other_operation_busy: true,
            ..ready()
        }));
    }
}
