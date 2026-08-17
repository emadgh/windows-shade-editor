/// Stable UI labels for production features whose backend/UI wiring must not silently disappear.
pub const EXPORT_QUEUE_LABEL: &str = "Export Queue";
pub const TIFF_INSPECTOR_LABEL: &str = "Inspect TIFF...";
pub const COLOR_MANAGEMENT_LABEL: &str = "Color Management / ICC Preview";
pub const COLOR_CONVERSION_LABEL: &str = "Convert Color...";
pub const SOFT_PROOF_LABEL: &str = "Printer / RIP Soft Proof";
pub const MONITOR_PROFILE_LABEL: &str = "Monitor / Display ICC";

#[cfg(test)]
mod tests {
    const MAIN: &str = include_str!("main.rs");
    const PROJECT_NAVIGATION_UI: &str = include_str!("ui/project_navigation.rs");
    const STATUS_BAR_UI: &str = include_str!("ui/status_bar.rs");
    const COLOR_CONVERSION_UI: &str = include_str!("ui/color_conversion.rs");

    fn production_ui_contains(needle: &str) -> bool {
        MAIN.contains(needle)
            || PROJECT_NAVIGATION_UI.contains(needle)
            || STATUS_BAR_UI.contains(needle)
            || COLOR_CONVERSION_UI.contains(needle)
    }

    #[test]
    fn required_production_backends_are_compiled_into_the_application() {
        for module in [
            "mod export_queue;",
            "mod tiff_inspect;",
            "mod color_management;",
            "mod production_project;",
            "mod project_lifecycle;",
        ] {
            assert!(
                MAIN.contains(module),
                "missing production module wiring: {module}"
            );
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
        for entry in [
            "app_features::EXPORT_QUEUE_LABEL",
            "app_features::TIFF_INSPECTOR_LABEL",
            "Color Management / ICC Preview",
            "app_features::COLOR_CONVERSION_LABEL",
            super::SOFT_PROOF_LABEL,
            super::MONITOR_PROFILE_LABEL,
        ] {
            assert!(
                production_ui_contains(entry),
                "missing production feature entry point: {entry}"
            );
        }
    }

    #[test]
    fn color_conversion_entry_remains_bound_to_shared_preflight_contract() {
        assert!(STATUS_BAR_UI.contains("ui_color_conversion_status"));
        assert!(STATUS_BAR_UI.contains("ui_color_conversion_window"));
        assert!(COLOR_CONVERSION_UI.contains("build_conversion_preflight"));
        assert!(COLOR_CONVERSION_UI.contains("Assign Production Source ICC"));
        assert!(COLOR_CONVERSION_UI.contains("production_source_profile"));
        assert!(COLOR_CONVERSION_UI.contains("inspect_production_source_profile"));
        assert!(COLOR_CONVERSION_UI.contains("Continue to Target Setup"));
        assert!(COLOR_CONVERSION_UI.contains("Select Output ICC"));
        assert!(COLOR_CONVERSION_UI.contains("verify_production_target_profile"));
        assert!(COLOR_CONVERSION_UI.contains("build_target_setup_review"));
        assert!(COLOR_CONVERSION_UI.contains("RGB source — not production separated"));
    }

    #[test]
    fn destructive_project_actions_route_through_typed_transition_guard() {
        assert!(MAIN.contains("request_project_transition(ProjectTransition::New"));
        assert!(MAIN.contains("ProjectTransition::Open"));
        assert!(MAIN.contains("request_project_transition(ProjectTransition::Exit"));
        assert!(MAIN.contains("request_project_transition(ProjectTransition::Recover"));
    }
}
