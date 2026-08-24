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
    const CONVERSION_PLAN_UI: &str = include_str!("ui/conversion_plan.rs");
    const CANDIDATE_UI: &str = include_str!("ui/conversion_candidate_preview.rs");
    const BATCH_UI: &str = include_str!("ui/conversion_batch.rs");

    fn production_source(source: &str) -> &str {
        source.split("\n#[cfg(test)]").next().unwrap_or(source)
    }

    fn production_ui_contains(needle: &str) -> bool {
        MAIN.contains(needle)
            || PROJECT_NAVIGATION_UI.contains(needle)
            || STATUS_BAR_UI.contains(needle)
            || production_source(COLOR_CONVERSION_UI).contains(needle)
    }

    #[test]
    fn required_production_backends_are_compiled_into_the_application() {
        for module in [
            "mod export_queue;",
            "mod tiff_inspect;",
            "mod color_management;",
            "mod conversion_tiff;",
            "mod production_project;",
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
    fn production_color_conversion_has_one_operator_entry_and_shared_cores() {
        let conversion = production_source(COLOR_CONVERSION_UI);
        let plan = production_source(CONVERSION_PLAN_UI);
        let candidate = production_source(CANDIDATE_UI);
        let batch = production_source(BATCH_UI);

        assert!(MAIN.contains("app_features::COLOR_CONVERSION_LABEL"));
        assert!(MAIN.contains("open_color_conversion(ui.ctx())"));
        assert!(STATUS_BAR_UI.contains("ui_color_conversion_window"));
        assert!(STATUS_BAR_UI.contains("poll_conversion_candidate_runtime"));
        assert!(STATUS_BAR_UI.contains("poll_conversion_batch_runtime"));
        for removed in [
            "ui_color_conversion_status",
            "ui_conversion_candidate_status",
            "ui_conversion_candidate_window",
            "ui_conversion_batch_status",
            "ui_conversion_batch_window",
        ] {
            assert!(!STATUS_BAR_UI.contains(removed));
        }

        assert!(conversion.contains("target: ConversionTargetState"));
        assert!(conversion.contains("Current Face"));
        assert!(conversion.contains("Selected Faces"));
        assert!(conversion.contains("All Faces"));
        assert!(conversion.contains("Destination folder"));
        assert!(conversion.contains("sync_conversion_candidate"));
        assert!(conversion.contains("queue_unified_conversion_plan"));

        assert!(plan.contains("build_conversion_preflight_for_source_with_policy"));
        assert!(plan.contains("build_conversion_recipe"));
        assert!(plan.contains("deterministic_converted_filename"));
        assert!(!plan.contains("next_versioned_output_path"));
        assert!(candidate.contains("render_candidate_preview"));
        assert!(!candidate.contains("CandidateConfig"));
        assert!(batch.contains("ConversionBatchCapture::capture"));
        assert!(batch.contains("ConversionBatchQueue::load_persistent"));
        assert!(!batch.contains("ConversionBatchUiConfig"));
    }

    #[test]
    fn destructive_project_actions_route_through_typed_transition_guard() {
        assert!(MAIN.contains("request_project_transition(ProjectTransition::New"));
        assert!(MAIN.contains("ProjectTransition::Open"));
        assert!(MAIN.contains("request_project_transition(ProjectTransition::Exit"));
        assert!(MAIN.contains("request_project_transition(ProjectTransition::Recover"));
        assert!(MAIN.contains("conversion_batch_blocks_project_transition"));
    }
}
