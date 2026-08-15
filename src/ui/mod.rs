pub(crate) mod adjustments;
pub(crate) mod export_queue;
pub(crate) mod status_bar;

#[cfg(test)]
mod tests {
    #[test]
    fn decomposed_ui_methods_do_not_regress_back_into_main() {
        let main = include_str!("../main.rs");
        for method in [
            "ui_history",
            "ui_channels_histogram",
            "ui_adjustment_quick_tools",
            "ui_adjustments",
            "ui_selected_adjustment",
            "ui_export_queue_window",
            "project_save_state_label",
            "ui_status",
        ] {
            assert!(
                !main.contains(&format!("fn {method}")),
                "{method} should remain in a focused src/ui module, not src/main.rs"
            );
        }
    }
}
