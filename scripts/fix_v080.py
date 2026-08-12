from pathlib import Path

app_path = Path('src/app_main.rs')
app = app_path.read_text(encoding='utf-8')

old_black = '            curve.input_black = input.clamp(0.0, (curve.midpoint_input - gap).max(0.0));'
new_black = '''            let max_input = if curve.midpoint_enabled {
                (curve.midpoint_input - gap).max(0.0)
            } else {
                (curve.input_white - gap).max(0.0)
            };
            curve.input_black = input.clamp(0.0, max_input);'''
if old_black in app:
    app = app.replace(old_black, new_black, 1)

old_white = '            curve.input_white = input.clamp((curve.midpoint_input + gap).min(1.0), 1.0);'
new_white = '''            let min_input = if curve.midpoint_enabled {
                (curve.midpoint_input + gap).min(1.0)
            } else {
                (curve.input_black + gap).min(1.0)
            };
            curve.input_white = input.clamp(min_input, 1.0);'''
if old_white in app:
    app = app.replace(old_white, new_white, 1)

old_state = '    let mut changed = false;\n    let points = ['
new_state = '    let mut changed = false;\n    let mut midpoint_removed_this_frame = false;\n    let points = ['
if old_state in app:
    app = app.replace(old_state, new_state, 1)

old_remove = '''            curve.midpoint_enabled = false;
            selected = CurvePointKind::Black;'''
new_remove = '''            curve.midpoint_enabled = false;
            midpoint_removed_this_frame = true;
            selected = CurvePointKind::Black;'''
if old_remove in app:
    app = app.replace(old_remove, new_remove, 1)

old_add_condition = '    if !curve.midpoint_enabled && graph_response.double_clicked() {'
new_add_condition = '    if !curve.midpoint_enabled && !midpoint_removed_this_frame && graph_response.double_clicked() {'
if old_add_condition in app:
    app = app.replace(old_add_condition, new_add_condition, 1)

app_path.write_text(app, encoding='utf-8')

model_path = Path('src/model_v6.rs')
model = model_path.read_text(encoding='utf-8')
old = '''fn snapshot_sequence_number(name: &str, prefix: &str) -> Option<u64> {
    let trimmed = name.trim();
    if trimmed.len() < prefix.len() || !trimmed[..prefix.len()].eq_ignore_ascii_case(prefix) {
        return None;
    }
    trimmed[prefix.len()..].parse::<u64>().ok()
}'''
new = '''fn snapshot_sequence_number(name: &str, prefix: &str) -> Option<u64> {
    let trimmed = name.trim();
    let head = trimmed.get(..prefix.len())?;
    if !head.eq_ignore_ascii_case(prefix) {
        return None;
    }
    trimmed.get(prefix.len()..)?.parse::<u64>().ok()
}'''
if old in model:
    model = model.replace(old, new, 1)
model_path.write_text(model, encoding='utf-8')
