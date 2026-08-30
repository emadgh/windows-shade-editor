pub(crate) mod actions;
pub(crate) mod adjustments;
pub(crate) mod characterization_intake;
pub(crate) mod color_conversion;
pub(crate) mod conversion_audit;
pub(crate) mod conversion_batch;
pub(crate) mod conversion_candidate_cache;
pub(crate) mod conversion_candidate_preview;
pub(crate) mod conversion_candidate_softproof;
pub(crate) mod custom_optimizer_plan_adapter;
pub(crate) mod conversion_plan;
pub(crate) mod conversion_presets;
pub(crate) mod conversion_route_migration;
pub(crate) mod curve_editor;
pub(crate) mod export_queue;
pub(crate) mod external_validation;
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
    fn color_conversion_has_one_operator_surface_and_shared_runtime_cores() {
        let main = include_str!("../main.rs");
        let status_bar = include_str!("status_bar.rs");
        let conversion_source = include_str!("color_conversion.rs");
        let audit_source = include_str!("conversion_audit.rs");
        let external_validation_source = include_str!("external_validation.rs");
        let migration_source = include_str!("conversion_route_migration.rs");
        let plan_source = include_str!("conversion_plan.rs");
        let batch_source = include_str!("conversion_batch.rs");
        let candidate_source = include_str!("conversion_candidate_preview.rs");
        let preset_source = include_str!("conversion_presets.rs");
        let intake_source = include_str!("characterization_intake.rs");
        let conversion = conversion_source.split("\n#[cfg(test)]").next().unwrap_or(conversion_source);
        let audit = audit_source.split("\n#[cfg(test)]").next().unwrap_or(audit_source);
        let external_validation = external_validation_source
            .split("\n#[cfg(test)]")
            .next()
            .unwrap_or(external_validation_source);
        let migration = migration_source.split("\n#[cfg(test)]").next().unwrap_or(migration_source);
        let plan = plan_source.split("\n#[cfg(test)]").next().unwrap_or(plan_source);
        let batch = batch_source.split("\n#[cfg(test)]").next().unwrap_or(batch_source);
        let candidate = candidate_source.split("\n#[cfg(test)]").next().unwrap_or(candidate_source);
        let presets = preset_source.split("\n#[cfg(test)]").next().unwrap_or(preset_source);
        let intake = intake_source.split("\n#[cfg(test)]").next().unwrap_or(intake_source);

        assert!(status_bar.contains("ui_color_conversion_window"));
        assert!(status_bar.contains("ui_conversion_route_migration"));
        assert!(status_bar.contains("ui_conversion_audit_menu"));
        assert!(status_bar.contains("ui_external_validation_packet_menu"));
        assert!(status_bar.contains("poll_conversion_candidate_runtime"));
        assert!(status_bar.contains("poll_conversion_batch_runtime"));
        for removed in [
            "ui_color_conversion_status",
            "ui_conversion_candidate_status",
            "ui_conversion_candidate_window",
            "ui_conversion_batch_status",
            "ui_conversion_batch_window",
        ] {
            assert!(!status_bar.contains(removed), "duplicate conversion surface remains: {removed}");
        }

        assert!(conversion.contains("target: ConversionTargetState"));
        assert!(conversion.contains("Current Face"));
        assert!(conversion.contains("Selected Faces"));
        assert!(conversion.contains("All Faces"));
        assert!(conversion.contains("Destination folder"));
        assert!(plan.contains("build_conversion_preflight_for_source_with_policy"));
        assert!(plan.contains("build_conversion_recipe"));
        assert!(plan.contains("deterministic_converted_filename"));
        assert!(plan.contains("restore_target_from_route"));
        assert!(plan.contains("update_existing_route"));
        assert!(conversion.contains("Restore saved route settings"));
        assert!(conversion.contains("allow_production_work_discard"));
        assert!(migration.contains("Replace / migrate this existing conversion route"));
        assert!(migration.contains("Create new conversion route / Production link"));
        assert!(migration.contains("Resume exact saved migration"));

        assert!(batch.contains("ConversionBatchQueue::load_persistent"));
        assert!(batch.contains("ConversionBatchCapture::capture"));
        assert!(!batch.contains("ConversionBatchUiConfig"));
        assert!(!batch.contains("egui::Window::new"));

        assert!(candidate.contains("render_candidate_preview"));
        assert!(candidate.contains("sync_conversion_candidate"));
        assert!(!candidate.contains("CandidateConfig"));
        assert!(!candidate.contains("egui::Window::new"));

        assert!(presets.contains("PresetRuntimeController"));
        assert!(presets.contains("unified_strategy_preset_availability"));
        assert!(!presets.contains("egui::Window::new"));

        assert!(intake.contains("build_characterization_package_from_table"));
        assert!(intake.contains("save_characterization_package"));
        assert!(!intake.contains("egui::Window::new"));
        assert!(!intake.contains("CharacterizationPackage::new"));

        assert!(audit.contains("project.conversion_audits"));
        assert!(audit.contains("validate_against_provenance"));
        assert!(audit.contains("to_portable_pretty_json"));
        assert!(!audit.contains("ConversionAuditRecord::from_committed_job"));
        assert!(!audit.contains("build_conversion_preflight"));
        assert!(external_validation.contains("ExternalValidationPacket::from_conversion_audit"));
        assert!(external_validation.contains("validate_against_provenance"));

        let validation_ui = external_validation
            .split("fn packet_for_bound_audit")
            .next()
            .unwrap_or(external_validation);
        let import_call = validation_ui
            .find("select_and_validate_completed_packet(&audit)")
            .expect("external acceptance reporting must come from the validated import path");
        let acceptance_call = validation_ui
            .find("packet.externally_accepted()")
            .expect("validated returned evidence must expose explicit complete acceptance status");
        assert!(
            import_call < acceptance_call,
            "external acceptance status was checked before the exact-audit import path"
        );

        let import_helper = external_validation
            .split("fn select_and_validate_completed_packet")
            .nth(1)
            .and_then(|tail| tail.split("fn load_validation_packet").next())
            .expect("completed external-validation import helper must remain explicit");
        assert!(
            import_helper.contains("packet.validate_against_conversion_audit(audit)?"),
            "completed external-validation evidence must bind to the exact persisted audit before it is returned"
        );

        assert!(main.contains("app_features::COLOR_CONVERSION_LABEL"));
    }
}