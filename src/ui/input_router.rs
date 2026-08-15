#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputContext {
    Global,
    Curve,
    TextEdit,
    Modal,
    ProjectView,
}

pub fn classify(
    wants_keyboard_input: bool,
    curve_graph_focused: bool,
    modal_active: bool,
    project_view_active: bool,
) -> InputContext {
    if modal_active {
        InputContext::Modal
    } else if curve_graph_focused {
        InputContext::Curve
    } else if wants_keyboard_input {
        InputContext::TextEdit
    } else if project_view_active {
        InputContext::ProjectView
    } else {
        InputContext::Global
    }
}

impl InputContext {
    pub fn allows_save_shortcuts(self) -> bool {
        !matches!(self, Self::Modal)
    }

    pub fn allows_project_commands(self) -> bool {
        matches!(self, Self::Global | Self::Curve | Self::ProjectView)
    }

    pub fn allows_editor_shortcuts(self) -> bool {
        matches!(self, Self::Global | Self::Curve)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_owns_input_even_if_curve_was_previously_focused() {
        assert_eq!(classify(true, true, true, false), InputContext::Modal);
        assert!(!InputContext::Modal.allows_save_shortcuts());
        assert!(!InputContext::Modal.allows_editor_shortcuts());
    }

    #[test]
    fn text_edit_blocks_editor_and_destructive_project_shortcuts_but_keeps_save() {
        let context = classify(true, false, false, false);
        assert_eq!(context, InputContext::TextEdit);
        assert!(context.allows_save_shortcuts());
        assert!(!context.allows_project_commands());
        assert!(!context.allows_editor_shortcuts());
    }

    #[test]
    fn curve_context_keeps_channel_editor_shortcuts_available() {
        let context = classify(true, true, false, false);
        assert_eq!(context, InputContext::Curve);
        assert!(context.allows_project_commands());
        assert!(context.allows_editor_shortcuts());
    }
}
