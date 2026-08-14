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
}