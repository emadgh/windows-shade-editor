/// Stable UI labels for production features whose backend/UI wiring must not silently disappear.
pub const EXPORT_QUEUE_LABEL: &str = "Export Queue";
pub const TIFF_INSPECTOR_LABEL: &str = "Inspect TIFF...";
pub const COLOR_MANAGEMENT_LABEL: &str = "Color Management / ICC Preview";
pub const SOFT_PROOF_LABEL: &str = "Printer / RIP Soft Proof";
pub const MONITOR_PROFILE_LABEL: &str = "Monitor / Display ICC";

#[cfg(test)]
mod tests {
    const MAIN: &str = include_str!("main.rs");

    #[test]
    fn required_production_backends_are_compiled_into_the_application() {
        for module in [
            "mod export_queue;",
            "mod tiff_inspect;",
            "mod color_management;",
            "mod project_lifecycle;",
        ] {
            assert!(MAIN.contains(module), "missing production module wiring: {module}");
        }
    }

    #[test]
    fn required_production_windows_are_called_from_the_app_update_path() {
        for call in [
            "self.ui_export_queue_window(ui.ctx())",
            "self.ui_tiff_inspector_window(ui.ctx())",
            "self.ui_color_management_window(ui.ctx())",
            "self.ui_project_transition_confirmation(ui.ctx())",
        ] {
            assert!(MAIN.contains(call), "missing production UI wiring: {call}");
        }
    }

    #[test]
    fn toolbar_and_color_management_entry_points_remain_present() {
        for label in [
            super::EXPORT_QUEUE_LABEL,
            super::TIFF_INSPECTOR_LABEL,
            super::COLOR_MANAGEMENT_LABEL,
            super::SOFT_PROOF_LABEL,
            super::MONITOR_PROFILE_LABEL,
        ] {
            assert!(MAIN.contains(label), "missing production feature entry point: {label}");
        }
    }

    #[test]
    fn destructive_project_actions_route_through_typed_transition_guard() {
        assert!(MAIN.contains("request_project_transition(ProjectTransition::New"));
        assert!(MAIN.contains("ProjectTransition::Open"));
        assert!(MAIN.contains("request_project_transition(ProjectTransition::Exit"));
        assert!(MAIN.contains("request_project_transition(ProjectTransition::Recover"));
    }
}
