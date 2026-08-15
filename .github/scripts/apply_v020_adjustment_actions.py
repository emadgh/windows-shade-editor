from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def read(rel: str) -> str:
    return (ROOT / rel).read_text(encoding="utf-8")


def write(rel: str, text: str) -> None:
    (ROOT / rel).write_text(text, encoding="utf-8", newline="\n")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


# Typed Adjustment UI actions + central dispatch.
actions_path = "src/ui/actions.rs"
actions = read(actions_path)
actions = replace_once(
    actions,
    "use crate::workflow::*;\n",
    "use std::collections::BTreeMap;\n\nuse crate::workflow::*;\n",
    "actions import",
)
actions = replace_once(
    actions,
    "#[derive(Clone, Debug, PartialEq)]\npub(crate) enum ExportQueueUiAction {\n    SetOpen(bool),\n    ResumeRecovered,\n    TogglePaused,\n    RetryAllFailed,\n    CancelAllWaiting,\n    ClearCompleted,\n    ClearFailed,\n    Resume(u64),\n    Cancel(u64),\n    Retry(u64),\n    RevealFolder(PathBuf),\n}\n\nimpl ShadeApp {",
    "#[derive(Clone, Debug, PartialEq)]\npub(crate) enum ExportQueueUiAction {\n    SetOpen(bool),\n    ResumeRecovered,\n    TogglePaused,\n    RetryAllFailed,\n    CancelAllWaiting,\n    ClearCompleted,\n    ClearFailed,\n    Resume(u64),\n    Cancel(u64),\n    Retry(u64),\n    RevealFolder(PathBuf),\n}\n\n#[derive(Clone, Debug, PartialEq)]\npub(crate) enum AdjustmentUiAction {\n    Undo,\n    Redo,\n    ClearHistory,\n    RestoreClearedHistory,\n    JumpHistory(usize),\n    SelectProjectPalette(palette::ChannelPalette),\n    ShowComposite,\n    SelectChannel(usize),\n    PersistSettings,\n    InvalidatePreviews,\n    QueueHistory(BTreeMap<String, model::ChannelAdjustment>),\n}\n\nimpl ShadeApp {",
    "AdjustmentUiAction enum",
)
dispatcher = r'''

    pub(crate) fn dispatch_adjustment_ui_action(
        &mut self,
        action: AdjustmentUiAction,
        ctx: &egui::Context,
    ) {
        match action {
            AdjustmentUiAction::Undo => self.undo_adjustment(ctx),
            AdjustmentUiAction::Redo => self.redo_adjustment(ctx),
            AdjustmentUiAction::ClearHistory => {
                let scope = self.project.active_snapshot_id;
                self.flush_history_now();
                self.history_clear_backup = Some((scope, self.history.clone()));
                self.history
                    .reset(&self.project.adjustments, "Current state");
                self.sync_history_to_active_snapshot();
                self.report_info("History cleared - Undo clear is available once");
            }
            AdjustmentUiAction::RestoreClearedHistory => {
                let scope = self.project.active_snapshot_id;
                if let Some((backup_scope, backup)) = self.history_clear_backup.take() {
                    if backup_scope == scope {
                        self.history = backup;
                        self.sync_history_to_active_snapshot();
                        self.report_info("Cleared history restored");
                    }
                }
            }
            AdjustmentUiAction::JumpHistory(index) => {
                self.flush_history_now();
                if let Some(adjustments) = self.history.jump(index) {
                    self.apply_history_adjustments(adjustments, "History state selected");
                }
            }
            AdjustmentUiAction::SelectProjectPalette(palette) => {
                self.select_project_palette(palette);
            }
            AdjustmentUiAction::ShowComposite => self.show_composite(),
            AdjustmentUiAction::SelectChannel(index) => self.select_channel(index, true),
            AdjustmentUiAction::PersistSettings => self.save_settings_quietly(),
            AdjustmentUiAction::InvalidatePreviews => self.mark_all_previews_dirty(),
            AdjustmentUiAction::QueueHistory(before) => {
                self.queue_adjustment_history(&before);
            }
        }
    }
'''
actions = replace_once(
    actions,
    "\n}\n\n#[cfg(test)]\nmod tests {",
    dispatcher + "\n}\n\n#[cfg(test)]\nmod tests {",
    "adjustment dispatcher",
)
test_block = r'''

    #[test]
    fn adjustment_actions_preserve_history_and_palette_payloads() {
        assert_eq!(
            AdjustmentUiAction::JumpHistory(17),
            AdjustmentUiAction::JumpHistory(17)
        );
        let palette = palette::builtin_cmyk();
        assert_eq!(
            AdjustmentUiAction::SelectProjectPalette(palette.clone()),
            AdjustmentUiAction::SelectProjectPalette(palette)
        );
        let mut before = BTreeMap::new();
        before.insert("Cyan".to_owned(), model::ChannelAdjustment::default());
        assert_eq!(
            AdjustmentUiAction::QueueHistory(before.clone()),
            AdjustmentUiAction::QueueHistory(before)
        );
    }
'''
if not actions.endswith("}\n"):
    raise RuntimeError("actions tests: unexpected file ending")
actions = actions[:-2] + test_block + "}\n"
write(actions_path, actions)


# Adjustments presentation keeps local value editing, but routes orchestration side effects.
adj_path = "src/ui/adjustments.rs"
adj = read(adj_path)
adj = replace_once(
    adj,
    "use super::curve_editor::curves_ui;\n",
    "use super::actions::AdjustmentUiAction;\nuse super::curve_editor::curves_ui;\n",
    "adjustments action import",
)
adj = adj.replace(
    "self.undo_adjustment(ui.ctx());",
    "self.dispatch_adjustment_ui_action(AdjustmentUiAction::Undo, ui.ctx());",
)
adj = adj.replace(
    "self.redo_adjustment(ui.ctx());",
    "self.dispatch_adjustment_ui_action(AdjustmentUiAction::Redo, ui.ctx());",
)
adj = replace_once(
    adj,
    '''        if clear {
            self.flush_history_now();
            self.history_clear_backup = Some((scope, self.history.clone()));
            self.history
                .reset(&self.project.adjustments, "Current state");
            self.sync_history_to_active_snapshot();
            self.report_info("History cleared - Undo clear is available once");
        } else if undo_clear {
            if let Some((backup_scope, backup)) = self.history_clear_backup.take() {
                if backup_scope == scope {
                    self.history = backup;
                    self.sync_history_to_active_snapshot();
                    self.report_info("Cleared history restored");
                }
            }
        }
''',
    '''        if clear {
            self.dispatch_adjustment_ui_action(AdjustmentUiAction::ClearHistory, ui.ctx());
        } else if undo_clear {
            self.dispatch_adjustment_ui_action(
                AdjustmentUiAction::RestoreClearedHistory,
                ui.ctx(),
            );
        }
''',
    "history clear routing",
)
adj = replace_once(
    adj,
    '''        if let Some(index) = requested {
            self.flush_history_now();
            if let Some(adjustments) = self.history.jump(index) {
                self.apply_history_adjustments(adjustments, "History state selected");
            }
        }
''',
    '''        if let Some(index) = requested {
            self.dispatch_adjustment_ui_action(
                AdjustmentUiAction::JumpHistory(index),
                ui.ctx(),
            );
        }
''',
    "history jump routing",
)
adj = replace_once(
    adj,
    "                self.select_project_palette(palette);",
    "                self.dispatch_adjustment_ui_action(\n                    AdjustmentUiAction::SelectProjectPalette(palette),\n                    ui.ctx(),\n                );",
    "palette routing",
)
adj = adj.replace(
    "self.show_composite();",
    "self.dispatch_adjustment_ui_action(AdjustmentUiAction::ShowComposite, ui.ctx());",
)
adj = replace_once(
    adj,
    "                self.select_channel(index, true);",
    "                self.dispatch_adjustment_ui_action(\n                    AdjustmentUiAction::SelectChannel(index),\n                    ui.ctx(),\n                );",
    "channel selection routing",
)
adj = adj.replace(
    "self.save_settings_quietly();",
    "self.dispatch_adjustment_ui_action(AdjustmentUiAction::PersistSettings, ui.ctx());",
)
adj = adj.replace(
    "self.mark_all_previews_dirty();",
    "self.dispatch_adjustment_ui_action(AdjustmentUiAction::InvalidatePreviews, ui.ctx());",
)
adj = replace_once(
    adj,
    "            self.queue_adjustment_history(&adjustments_before);",
    "            self.dispatch_adjustment_ui_action(\n                AdjustmentUiAction::QueueHistory(adjustments_before),\n                ui.ctx(),\n            );",
    "history queue routing",
)
for forbidden in [
    "self.undo_adjustment(",
    "self.redo_adjustment(",
    "self.flush_history_now()",
    "self.sync_history_to_active_snapshot()",
    "self.apply_history_adjustments(",
    "self.select_project_palette(",
    "self.show_composite()",
    "self.select_channel(",
    "self.save_settings_quietly()",
    "self.mark_all_previews_dirty()",
    "self.queue_adjustment_history(",
]:
    if forbidden in adj:
        raise RuntimeError(f"adjustments presentation still bypasses typed actions: {forbidden}")
write(adj_path, adj)


# Architecture guard for the Adjustment presentation boundary.
mod_path = "src/ui/mod.rs"
mod_text = read(mod_path)
marker = '''        }
    }

    #[test]
    fn project_view_transient_state_stays_behind_focused_state_object() {'''
adjustment_guard = '''        }

        let adjustments = include_str!("adjustments.rs");
        for forbidden in [
            "self.undo_adjustment(",
            "self.redo_adjustment(",
            "self.flush_history_now()",
            "self.sync_history_to_active_snapshot()",
            "self.apply_history_adjustments(",
            "self.select_project_palette(",
            "self.show_composite()",
            "self.select_channel(",
            "self.save_settings_quietly()",
            "self.mark_all_previews_dirty()",
            "self.queue_adjustment_history(",
        ] {
            assert!(
                !adjustments.contains(forbidden),
                "Adjustments presentation bypassed typed actions with {forbidden}"
            );
        }
    }

    #[test]
    fn project_view_transient_state_stays_behind_focused_state_object() {'''
mod_text = replace_once(mod_text, marker, adjustment_guard, "adjustments architecture guard")
write(mod_path, mod_text)


# Document the action domain.
docs_path = "docs/UI_ACTIONS.md"
docs = read(docs_path)
docs = replace_once(
    docs,
    "- `ExportQueueUiAction` — window state plus resume/pause/retry/cancel/clear/reveal intents for queued exports.\n",
    "- `ExportQueueUiAction` — window state plus resume/pause/retry/cancel/clear/reveal intents for queued exports.\n- `AdjustmentUiAction` — history navigation/clear, palette/channel/composite selection, settings persistence, preview invalidation and history commit side effects.\n",
    "UI action docs",
)
docs += "\nAdjustment controls still edit Levels/Curve/Mixer values locally. The typed boundary is intentionally limited to orchestration side effects so the UI does not duplicate history, rendering, settings-persistence or project mutation policy.\n"
write(docs_path, docs)


# Final patch version for completion of the #48 architecture follow-up.
cargo_path = "Cargo.toml"
cargo = read(cargo_path)
cargo = replace_once(cargo, 'version = "0.20.1"', 'version = "0.20.2"', "Cargo version")
write(cargo_path, cargo)

lock_path = "Cargo.lock"
lock = read(lock_path)
lock = replace_once(
    lock,
    'name = "windows-shade-editor"\nversion = "0.20.1"',
    'name = "windows-shade-editor"\nversion = "0.20.2"',
    "Cargo.lock root version",
)
write(lock_path, lock)

write("VERSION", "0.20.2\n")

notes_path = "RELEASE_NOTES.md"
notes = read(notes_path)
notes = """# Shade Editor 0.20.2

- Complete the typed UI-action architecture follow-up for high-value Adjustment surfaces.
- Route Undo/Redo, history clear/restore/jump, palette/channel/composite selection, settings persistence, preview invalidation and adjustment-history commits through `AdjustmentUiAction`.
- Keep Levels/Curve/Mixer value editing local to the Adjustment presentation layer; no framework rewrite or visual redesign.
- Add architecture regression coverage so Adjustment presentation cannot directly regain history/render/settings orchestration calls.
- Preserve revision-aware autosave, Snapshot history, render-generation invalidation and all existing adjustment behavior.

""" + notes
write(notes_path, notes)
