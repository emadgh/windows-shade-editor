use super::actions::AdjustmentUiAction;
use crate::*;
use eframe::egui;

pub(crate) fn ui_history(app: &mut ShadeApp, ui: &mut egui::Ui) {
    let scope = app.project.active_snapshot_id;
    let can_undo_clear = app
        .history_clear_backup
        .as_ref()
        .is_some_and(|(backup_scope, _)| *backup_scope == scope);
    let mut clear = false;
    let mut undo_clear = false;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(app.history.can_undo(), egui::Button::new("Undo").small())
            .on_hover_text("Ctrl+Alt+Z")
            .clicked()
        {
            app.dispatch_adjustment_ui_action(AdjustmentUiAction::Undo, ui.ctx());
        }
        if ui
            .add_enabled(app.history.can_redo(), egui::Button::new("Redo").small())
            .on_hover_text("Ctrl+Shift+Z")
            .clicked()
        {
            app.dispatch_adjustment_ui_action(AdjustmentUiAction::Redo, ui.ctx());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if can_undo_clear {
                undo_clear = ui.small_button("Undo clear").clicked();
            }
            clear = ui
                .add_enabled(
                    app.history.len() > 1,
                    egui::Button::new("Clear").small(),
                )
                .on_hover_text("Clear adjustment history")
                .clicked();
        });
    });
    if let Some(name) = app.project.active_snapshot_name() {
        ui.small(format!("{name} · {} saved steps max", app.settings.history_steps));
    } else {
        ui.small("Working adjustment history");
    }

    if clear {
        app.dispatch_adjustment_ui_action(AdjustmentUiAction::ClearHistory, ui.ctx());
    } else if undo_clear {
        app.dispatch_adjustment_ui_action(AdjustmentUiAction::RestoreClearedHistory, ui.ctx());
    }

    let rows = app
        .history
        .entries()
        .iter()
        .enumerate()
        .map(|(index, entry)| (index, entry.label.clone()))
        .collect::<Vec<_>>();
    let cursor = app.history.cursor();
    let mut requested = None;
    egui::ScrollArea::vertical()
        .id_salt("compact-adjustment-history")
        .max_height(210.0)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for (index, label) in rows {
                if clickable_row(ui, index == cursor, &label, None, None, 28.0).clicked() {
                    requested = Some(index);
                }
            }
        });
    if let Some(index) = requested {
        app.dispatch_adjustment_ui_action(AdjustmentUiAction::JumpHistory(index), ui.ctx());
    }
}
