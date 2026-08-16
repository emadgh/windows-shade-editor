#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConversionSourceState {
    pub has_faces: bool,
    pub has_saved_project_path: bool,
    pub has_unsaved_changes: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversionSaveGate {
    /// Conversion may capture the current saved Source state.
    Ready,
    /// There is no source Face to convert.
    NoSourceFaces,
    /// The Source project has never been saved and must receive a `.shade` path.
    SaveAsRequired,
    /// The Source project exists on disk but current state differs from the last save.
    SaveRequired,
}

impl ConversionSaveGate {
    pub fn can_start(self) -> bool {
        self == Self::Ready
    }

    pub fn requires_save(self) -> bool {
        matches!(self, Self::SaveAsRequired | Self::SaveRequired)
    }

    pub fn action_label(self) -> Option<&'static str> {
        match self {
            Self::SaveAsRequired => Some("Save Source Project As..."),
            Self::SaveRequired => Some("Save & Continue"),
            Self::Ready | Self::NoSourceFaces => None,
        }
    }
}

/// Decide whether `Convert Color...` may start from the current Source project.
///
/// This is deliberately separate from `ProjectLifecycleController`: conversion
/// is not a destructive project transition. It creates a derived production
/// artifact while the Source project remains open and immutable source files
/// remain untouched.
pub fn conversion_save_gate(state: ConversionSourceState) -> ConversionSaveGate {
    if !state.has_faces {
        return ConversionSaveGate::NoSourceFaces;
    }
    if !state.has_saved_project_path {
        return ConversionSaveGate::SaveAsRequired;
    }
    if state.has_unsaved_changes {
        return ConversionSaveGate::SaveRequired;
    }
    ConversionSaveGate::Ready
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_requires_a_source_face() {
        assert_eq!(
            conversion_save_gate(ConversionSourceState {
                has_faces: false,
                has_saved_project_path: false,
                has_unsaved_changes: false,
            }),
            ConversionSaveGate::NoSourceFaces
        );
    }

    #[test]
    fn never_saved_source_requires_save_as() {
        let gate = conversion_save_gate(ConversionSourceState {
            has_faces: true,
            has_saved_project_path: false,
            has_unsaved_changes: true,
        });
        assert_eq!(gate, ConversionSaveGate::SaveAsRequired);
        assert!(gate.requires_save());
        assert_eq!(gate.action_label(), Some("Save Source Project As..."));
    }

    #[test]
    fn dirty_saved_source_requires_save_and_continue() {
        let gate = conversion_save_gate(ConversionSourceState {
            has_faces: true,
            has_saved_project_path: true,
            has_unsaved_changes: true,
        });
        assert_eq!(gate, ConversionSaveGate::SaveRequired);
        assert!(gate.requires_save());
        assert_eq!(gate.action_label(), Some("Save & Continue"));
    }

    #[test]
    fn clean_saved_source_can_start_conversion() {
        let gate = conversion_save_gate(ConversionSourceState {
            has_faces: true,
            has_saved_project_path: true,
            has_unsaved_changes: false,
        });
        assert_eq!(gate, ConversionSaveGate::Ready);
        assert!(gate.can_start());
        assert!(!gate.requires_save());
    }
}
