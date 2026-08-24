pub(crate) mod actions;
pub(crate) mod adjustments;
pub(crate) mod color_conversion;
pub(crate) mod conversion_batch;
pub(crate) mod conversion_candidate_preview;
pub(crate) mod curve_editor;
pub(crate) mod export_queue;
pub(crate) mod faces;
pub(crate) mod history_panel;
pub(crate) mod input_router;
pub(crate) mod levels_mixer;
pub(crate) mod linked_projects;
pub(crate) mod match_color;
pub(crate) mod preview_status;
pub(crate) mod project_navigation;
#[path = "../project_link_navigation.rs"]
pub(crate) mod project_link_navigation_core;
pub(crate) mod project_view_state;
pub(crate) mod reference_panel;
pub(crate) mod settings_panel;
pub(crate) mod snapshots_panel;
pub(crate) mod status_bar;
pub(crate) mod test_code_panel;
pub(crate) mod viewport_controls;

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
        assert!(!workflow.contains("fn ui_faces"));
        assert!(!main.contains("enum CurvePointKind"));
        assert!(!main.contains("fn curve_editor_graph"));
        assert!(!main.contains(r#""Recent projects""#));
        assert!(!main.contains("mod input_router;"));
    }

    #[test]
    fn extracted_presentation_uses_typed_actions_for_cross_domain_mutations() {
        let faces = include_str!("faces.rs");
        for forbidden in [
            "app.remove_current_face()",
            "app.mark_project_dirty()",
            "relink_current_face_dialog(app)",
            "relink_missing_faces_folder_dialog(app)",
            "app.current_face = index;",
        ] {
            assert!(!faces.contains(forbidden));
        }
        let navigation = include_str!("project_navigation.rs");
        for forbidden in [
            "self.new_project();",
            "self.open_project_dialog();",
            "self.save_project(",
            "self.export_current_dialog();",
            "self.export_all_dialog();",
            "self.request_project_transition(",
            "self.validate_current_face_dialog();",
            "self.inspect_tiff_dialog();",
        ] {
            assert!(!navigation.contains(forbidden));
        }
        let linked_projects = include_str!("linked_projects.rs");
        assert!(linked_projects.contains("NavigationUiAction::OpenLinkedProject"));
        assert!(linked_projects.contains("NavigationUiAction::RelinkLinkedProject"));
        assert!(!linked_projects.contains("request_project_transition("));
        assert!(!linked_projects.contains("mark_project_dirty("));
    }

    #[test]
    fn export_queue_exposes_clear_jobs_cancel_and_cancel_all_controls() {
        let export_queue = include_str!("export_queue.rs");
        let actions = include_str!("actions.rs");
        for required in ["Clear Jobs", "Cancel All", "small_button(\"Cancel\")"] {
            assert!(export_queue.contains(required));
        }
        assert!(actions.contains("ExportQueueUiAction::ClearJobs"));
        assert!(!export_queue.contains("Cancel waiting"));
        assert!(!export_queue.contains("Stop after current"));
    }

    #[test]
    fn project_view_transient_state_stays_behind_focused_state_object() {
        let main = include_str!("../main.rs");
        for legacy_field in [
            "show_previous_shades: bool",
            "previous_shades_query: String",
            "previous_shades_sort: previous_shades::PreviousShadesSort",
            "previous_shades_selected: Option<String>",
            "previous_shade_preview: Option<previous_shades::ShadeInspection>",
            "previous_shade_preview_error: Option<String>",
            "previous_shade_texture: Option<egui::TextureHandle>",
            "previous_shade_list_textures: BTreeMap<String, egui::TextureHandle>",
            "previous_shade_list_texture_lru: VecDeque<String>",
        ] {
            assert!(!main.contains(legacy_field));
        }
        assert!(main.contains("project_view: ui::project_view_state::ProjectViewState"));
    }

    #[test]
    fn color_conversion_ui_stays_decomposed_and_wired_through_status_bar() {
        let status_bar = include_str!("status_bar.rs");
        let conversion = include_str!("color_conversion.rs");
        let batch = include_str!("conversion_batch.rs");
        let candidate = include_str!("conversion_candidate_preview.rs");
        assert!(status_bar.contains("ui_color_conversion_status"));
        assert!(status_bar.contains("ui_color_conversion_window"));
        assert!(status_bar.contains("ui_conversion_candidate_status"));
        assert!(status_bar.contains("ui_conversion_candidate_window"));
        assert!(status_bar.contains("ui_conversion_batch_status"));
        assert!(status_bar.contains("ui_conversion_batch_window"));
        assert!(conversion.contains("build_conversion_preflight"));
        assert!(!conversion.contains("K *= 2"));
        assert!(batch.contains("ConversionBatchScope::CurrentFace"));
        assert!(batch.contains("ConversionBatchScope::SelectedFaces"));
        assert!(batch.contains("ConversionBatchScope::AllFaces"));
        assert!(batch.contains("ConversionBatchQueue::load_persistent"));
        assert!(batch.contains("ConversionBatchCapture::capture"));
        assert!(candidate.contains("render_candidate_preview"));
        assert!(candidate.contains("Queue this exact conversion"));
        assert!(candidate.contains("Return to Source-adjusted view"));
    }
}
