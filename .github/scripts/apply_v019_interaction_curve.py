from pathlib import Path
import re


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if text.count(old) != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {text.count(old)}")
    return text.replace(old, new, 1)


root = Path(__file__).resolve().parents[2]
main_path = root / "src" / "main.rs"
workflow_path = root / "src" / "workflow.rs"
router_path = root / "src" / "input_router.rs"
main = main_path.read_text(encoding="utf-8")
workflow = workflow_path.read_text(encoding="utf-8")

main = replace_once(main, "mod history;\nmod model;", "mod history;\nmod input_router;\nmod model;", "main module declaration")

anchor = '''fn nudge_curve_point(\n    curve: &mut model::Curve,\n    selected: CurvePointKind,\n    horizontal_units: i32,\n    vertical_units: i32,\n    mode: TonalDisplayMode,\n) {\n    let (input, output) = curve_point_xy(*curve, selected);\n    let mut display_input = tonal_display_value(input, mode);\n    let mut display_output = tonal_display_value(output, mode);\n    display_input += horizontal_units as f32 / 255.0;\n    display_output += vertical_units as f32 / 255.0;\n    set_curve_point(\n        curve,\n        selected,\n        tonal_working_value(display_input, mode),\n        tonal_working_value(display_output, mode),\n    );\n}\n'''
helper = anchor + '''\nfn remove_selected_curve_point(\n    curve: &mut model::Curve,\n    selected: CurvePointKind,\n) -> (CurvePointKind, bool) {\n    if selected == CurvePointKind::Midpoint && curve.midpoint_enabled {\n        curve.midpoint_enabled = false;\n        (CurvePointKind::Black, true)\n    } else {\n        (selected, false)\n    }\n}\n\nfn reset_selected_curve_point(curve: &mut model::Curve, selected: CurvePointKind) {\n    match selected {\n        CurvePointKind::Black => set_curve_point(curve, selected, 0.0, 0.0),\n        CurvePointKind::Midpoint => {\n            let input = curve.midpoint_input;\n            set_curve_point(curve, selected, input, input);\n        }\n        CurvePointKind::White => set_curve_point(curve, selected, 1.0, 1.0),\n    }\n}\n'''
main = replace_once(main, anchor, helper, "curve helper insertion")

old_double = '''        if point == CurvePointKind::Midpoint && response.double_clicked() {\n            curve.midpoint_enabled = false;\n            midpoint_removed_this_frame = true;\n            selected = CurvePointKind::Black;\n            ui.data_mut(|data| data.insert_temp(selection_id, selected));\n            changed = true;\n            continue;\n        }\n'''
new_double = '''        if point == CurvePointKind::Midpoint && response.double_clicked() {\n            let (next, removed) = remove_selected_curve_point(curve, point);\n            midpoint_removed_this_frame = removed;\n            selected = next;\n            ui.data_mut(|data| data.insert_temp(selection_id, selected));\n            changed |= removed;\n            continue;\n        }\n'''
main = replace_once(main, old_double, new_double, "curve double-click removal")

pattern = re.compile(r'''        let \(left, right, up, down, shift\) = ui\.input\(\|input\| \{\n            \(\n                input\.key_pressed\(egui::Key::ArrowLeft\),\n                input\.key_pressed\(egui::Key::ArrowRight\),\n                input\.key_pressed\(egui::Key::ArrowUp\),\n                input\.key_pressed\(egui::Key::ArrowDown\),\n                input\.modifiers\.shift,\n            \)\n        \}\);\n        if left \|\| right \|\| up \|\| down \{\n            // Focus navigation is decided at the start of the frame, before this\n            // custom graph sees the key event\. Cancel that pending movement so the\n            // first arrow press after selecting a point cannot escape the graph\.\n            ui\.memory_mut\(\|memory\| memory\.move_focus\(egui::FocusDirection::None\)\);\n            let units = if shift \{ 10 \} else \{ 1 \};\n            let horizontal = if left \{\n                -units\n            \} else if right \{\n                units\n            \} else \{\n                0\n            \};\n            let vertical = if down \{\n                -units\n            \} else if up \{\n                units\n            \} else \{\n                0\n            \};\n            nudge_curve_point\(curve, selected, horizontal, vertical, display_mode\);\n            changed = true;\n        \}\n''')
replacement = '''        let (left, right, up, down, shift, delete, home) = ui.input(|input| {\n            (\n                input.key_pressed(egui::Key::ArrowLeft),\n                input.key_pressed(egui::Key::ArrowRight),\n                input.key_pressed(egui::Key::ArrowUp),\n                input.key_pressed(egui::Key::ArrowDown),\n                input.modifiers.shift,\n                input.key_pressed(egui::Key::Delete)\n                    || input.key_pressed(egui::Key::Backspace),\n                input.key_pressed(egui::Key::Home),\n            )\n        });\n        if delete {\n            let (next, removed) = remove_selected_curve_point(curve, selected);\n            if removed {\n                selected = next;\n                ui.data_mut(|data| data.insert_temp(selection_id, selected));\n                changed = true;\n            }\n        } else if home {\n            reset_selected_curve_point(curve, selected);\n            changed = true;\n        } else if left || right || up || down {\n            // Focus navigation is decided at the start of the frame, before this\n            // custom graph sees the key event. Cancel that pending movement so the\n            // first arrow press after selecting a point cannot escape the graph.\n            ui.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));\n            let units = if shift { 10 } else { 1 };\n            let horizontal = if left {\n                -units\n            } else if right {\n                units\n            } else {\n                0\n            };\n            let vertical = if down {\n                -units\n            } else if up {\n                units\n            } else {\n                0\n            };\n            nudge_curve_point(curve, selected, horizontal, vertical, display_mode);\n            changed = true;\n        }\n'''
main, count = pattern.subn(replacement, main, count=1)
if count != 1:
    raise SystemExit(f"curve keyboard block: expected one match, found {count}")

old_help = '            ui.small("Double-click the Curve line to add the midpoint; double-click the midpoint to remove it. Arrow keys move the selected point by 1; Shift+Arrow moves by 10. Input / Output stay 0-255 in both Light and Pigment display modes.");'
new_help = '            ui.small("Double-click the Curve line to add the midpoint; double-click or press Delete/Backspace on the midpoint to remove it. Arrow keys move the selected point by 1; Shift+Arrow moves by 10; Home returns the selected point to the identity line. Input / Output stay 0-255 in both Light and Pigment display modes.");'
main = replace_once(main, old_help, new_help, "curve help")

curves_end = '''        changed\n    })\n}\n\nfn mixer_ui(\n'''
curve_tests = '''        changed\n    })\n}\n\n#[cfg(test)]\nmod curve_qol_tests {\n    use super::*;\n\n    #[test]\n    fn delete_only_removes_optional_midpoint_and_keeps_selection_valid() {\n        let mut curve = model::Curve {\n            midpoint_enabled: true,\n            midpoint_input: 0.5,\n            midpoint: 0.6,\n            ..Default::default()\n        };\n        let (selected, changed) =\n            remove_selected_curve_point(&mut curve, CurvePointKind::Midpoint);\n        assert!(changed);\n        assert!(!curve.midpoint_enabled);\n        assert_eq!(selected, CurvePointKind::Black);\n\n        let before = curve;\n        let (selected, changed) = remove_selected_curve_point(&mut curve, CurvePointKind::White);\n        assert!(!changed);\n        assert_eq!(selected, CurvePointKind::White);\n        assert_eq!(curve, before);\n    }\n\n    #[test]\n    fn home_returns_selected_point_to_identity_without_breaking_input_order() {\n        let mut curve = model::Curve {\n            input_black: 0.10,\n            black: 0.25,\n            midpoint_enabled: true,\n            midpoint_input: 0.55,\n            midpoint: 0.75,\n            input_white: 0.90,\n            white: 0.80,\n        };\n        reset_selected_curve_point(&mut curve, CurvePointKind::Midpoint);\n        assert!((curve.midpoint - curve.midpoint_input).abs() < f32::EPSILON);\n        assert!(curve.input_black < curve.midpoint_input);\n        assert!(curve.midpoint_input < curve.input_white);\n\n        reset_selected_curve_point(&mut curve, CurvePointKind::Black);\n        assert_eq!((curve.input_black, curve.black), (0.0, 0.0));\n        reset_selected_curve_point(&mut curve, CurvePointKind::White);\n        assert_eq!((curve.input_white, curve.white), (1.0, 1.0));\n    }\n}\n\nfn mixer_ui(\n'''
main = replace_once(main, curves_end, curve_tests, "curve tests")
main_path.write_text(main, encoding="utf-8")

router_path.write_text(r'''#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
''', encoding="utf-8")

shortcut_pattern = re.compile(r'''pub\(super\) fn handle_shortcuts\(app: &mut ShadeApp, ctx: &egui::Context\) \{.*?\n\}\n\nfn select_all_channels_shortcut''', re.S)
shortcut_new = r'''pub(super) fn handle_shortcuts(app: &mut ShadeApp, ctx: &egui::Context) {
    let curve_graph_focused = ctx.data(|data| {
        data.get_temp::<bool>(egui::Id::new("shade-editor-curve-graph-focused"))
            .unwrap_or(false)
    });
    let modal_active = app.lifecycle.pending.is_some()
        || app.lifecycle.after_save.is_some()
        || app.lifecycle.backup_restore.is_some()
        || app.pending_snapshot_action.is_some()
        || app.recovery_candidate.is_some();
    let input_context = input_router::classify(
        ctx.wants_keyboard_input(),
        curve_graph_focused,
        modal_active,
        app.show_previous_shades,
    );

    let (new_project, save, save_as, export_face, export_all, update_snapshot) =
        ctx.input(|input| {
            (
                input.key_pressed(egui::Key::N)
                    && input.modifiers.ctrl
                    && !input.modifiers.shift
                    && !input.modifiers.alt,
                input.key_pressed(egui::Key::S)
                    && input.modifiers.ctrl
                    && !input.modifiers.shift
                    && !input.modifiers.alt,
                input.key_pressed(egui::Key::S)
                    && input.modifiers.ctrl
                    && input.modifiers.shift
                    && !input.modifiers.alt,
                input.key_pressed(egui::Key::E)
                    && input.modifiers.ctrl
                    && !input.modifiers.shift
                    && !input.modifiers.alt,
                input.key_pressed(egui::Key::E)
                    && input.modifiers.ctrl
                    && input.modifiers.shift
                    && !input.modifiers.alt,
                input.key_pressed(egui::Key::Enter) && input.modifiers.ctrl && !input.modifiers.alt,
            )
        });

    if input_context.allows_save_shortcuts() {
        if save_as {
            app.save_project(true);
        } else if save {
            app.save_project(false);
        }
    }
    if input_context.allows_project_commands() {
        if new_project {
            app.show_previous_shades = false;
            app.new_project();
        }
        if export_all {
            app.export_all_dialog();
        } else if export_face {
            app.export_current_dialog();
        }
        if update_snapshot {
            update_active_snapshot(app);
        }
    }

    if !input_context.allows_editor_shortcuts() {
        return;
    }

    let (channel, all_channels, settings, fit, solo) = ctx.input(|input| {
        let no_modifiers = !input.modifiers.ctrl && !input.modifiers.alt && !input.modifiers.shift;
        let keys = [
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
            egui::Key::Num7,
            egui::Key::Num8,
            egui::Key::Num9,
        ];
        let channel = no_modifiers
            .then(|| keys.iter().position(|key| input.key_pressed(*key)))
            .flatten();
        // Backtick is the logical key for both ` and Shift+` (~) in egui.
        let all_channels =
            !input.modifiers.ctrl && !input.modifiers.alt && input.key_pressed(egui::Key::Backtick);
        (
            channel,
            all_channels,
            no_modifiers && input.key_pressed(egui::Key::G),
            no_modifiers && input.key_pressed(egui::Key::F),
            no_modifiers && input.key_pressed(egui::Key::S),
        )
    });

    if settings {
        app.show_settings = true;
    }
    if all_channels {
        select_all_channels_shortcut(app);
    }
    if fit {
        app.fit_requested = true;
        app.viewport_recenter = true;
    }
    if let Some(channel) = channel {
        select_channel_shortcut(app, channel);
    }
    if solo && active_face_available(app) {
        let previous = app.solo_channel;
        app.solo_channel = if app.solo_channel == Some(app.selected_channel) {
            None
        } else {
            Some(app.selected_channel)
        };
        if app.solo_channel != previous {
            app.mark_current_preview_dirty();
        }
    }
}

fn select_all_channels_shortcut'''
workflow, count = shortcut_pattern.subn(shortcut_new, workflow, count=1)
if count != 1:
    raise SystemExit(f"workflow shortcut router: expected one match, found {count}")
workflow_path.write_text(workflow, encoding="utf-8")

# Remove bootstrap files from the final feature tree. The running workflow has
# already loaded both files, so it can finish validation and commit the clean diff.
Path(__file__).unlink()
bootstrap_workflow = root / ".github" / "workflows" / "apply-v019-interaction-curve.yml"
if bootstrap_workflow.exists():
    bootstrap_workflow.unlink()
