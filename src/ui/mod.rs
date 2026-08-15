pub(crate) mod adjustments;
pub(crate) mod curve_editor;
pub(crate) mod export_queue;
pub(crate) mod faces;
pub(crate) mod input_router;
pub(crate) mod project_navigation;
pub(crate) mod status_bar;

#[cfg(test)]
mod tests {
    #[test]
    fn decomposed_ui_does_not_regress_back_into_application_shells() {
        let main = include_str!("../main.rs");
        let workflow = include_str!("../workflow.rs");
        for method in [
            "ui_history",
            "ui_channels_histogram",
            "ui_adjustment_quick_tools",
            "ui_adjustments",
            "ui_selected_adjustment",
            "ui_export_queue_window",
            "project_save_state_label",
            "ui_status",
            "ui_previous_shades_window",
        ] {
            assert!(
                !main.contains(&format!("fn {method}")),
                "{method} regressed into main.rs"
            );
        }
        assert!(
            !workflow.contains("fn ui_faces"),
            "Faces UI regressed into workflow.rs"
        );
        assert!(
            !main.contains("enum CurvePointKind"),
            "Curve editor state regressed into main.rs"
        );
        assert!(
            !main.contains("fn curve_editor_graph"),
            "Curve editor graph regressed into main.rs"
        );
        assert!(
            !main.contains(r#""Recent projects""#),
            "Recent menu regressed into main.rs"
        );
        assert!(
            !main.contains("mod input_router;"),
            "Input router regressed to the crate root"
        );
    }
}
