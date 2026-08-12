from pathlib import Path
import re

ROOT = Path('.')


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f'{label}: expected 1 occurrence, found {count}')
    return text.replace(old, new, 1)

# Cargo -----------------------------------------------------------------------
cargo_path = ROOT / 'Cargo.toml'
cargo = cargo_path.read_text(encoding='utf-8')
cargo = replace_once(cargo, 'version = "0.7.1"', 'version = "0.8.0"', 'Cargo version')
if 'base64 = ' not in cargo:
    cargo = replace_once(
        cargo,
        'fontdue = "0.9.3"\n',
        'fontdue = "0.9.3"\nbase64 = "0.22.1"\npng = "0.18.0"\n',
        'Cargo dependencies',
    )
cargo_path.write_text(cargo, encoding='utf-8')

# Model -----------------------------------------------------------------------
model_path = ROOT / 'src/model_v6.rs'
model = model_path.read_text(encoding='utf-8')
model = replace_once(
    model,
    'pub const SHADE_SCHEMA_VERSION: u32 = 7;',
    'pub const SHADE_SCHEMA_VERSION: u32 = 8;',
    'schema version',
)
model = replace_once(
    model,
    '    #[serde(default)]\n    pub channel_palette: Option<ChannelPalette>,\n}',
    '''    #[serde(default)]\n    pub channel_palette: Option<ChannelPalette>,\n    /// Embedded project thumbnail. This is a normal PNG encoded as base64 so\n    /// the .shade JSON remains self-contained and portable.\n    #[serde(default)]\n    pub thumbnail: Option<ProjectThumbnail>,\n    /// Cached source/project properties captured on save for fast inspection\n    /// without reopening every TIFF.\n    #[serde(default)]\n    pub file_metadata: Option<ProjectFileMetadata>,\n}''',
    'project fields',
)
model = replace_once(
    model,
    '            test_code: TestCodeConfig::default(),\n            channel_palette: None,\n        }',
    '            test_code: TestCodeConfig::default(),\n            channel_palette: None,\n            thumbnail: None,\n            file_metadata: None,\n        }',
    'project defaults',
)
model = replace_once(
    model,
    '''#[derive(Clone, Debug, Serialize, Deserialize)]\npub struct FaceRef {\n    pub path: String,\n    pub label: String,\n}\n''',
    '''#[derive(Clone, Debug, Serialize, Deserialize)]\npub struct FaceRef {\n    pub path: String,\n    pub label: String,\n}\n\n#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]\npub struct ProjectThumbnail {\n    pub mime_type: String,\n    pub width: u32,\n    pub height: u32,\n    pub data_base64: String,\n}\n\n#[derive(Clone, Debug, Serialize, Deserialize, Default)]\npub struct ProjectFileMetadata {\n    pub saved_at_unix_ms: i64,\n    pub face_count: usize,\n    pub active_face_index: usize,\n    pub total_source_bytes: u64,\n    pub faces: Vec<FaceFileMetadata>,\n}\n\n#[derive(Clone, Debug, Serialize, Deserialize, Default)]\npub struct FaceFileMetadata {\n    pub label: String,\n    pub source_file_name: String,\n    pub width: u32,\n    pub height: u32,\n    pub bit_depth: u8,\n    pub color_model: String,\n    pub channel_count: usize,\n    pub base_channel_count: usize,\n    pub channel_names: Vec<String>,\n    pub dpi_x: f64,\n    pub dpi_y: f64,\n    pub dpi_from_source: bool,\n    pub resolution_unit: u16,\n    pub file_size_bytes: u64,\n    pub modified_at_unix_ms: Option<i64>,\n}\n''',
    'metadata structs',
)
model = replace_once(
    model,
    '    /// Input coordinate of the draggable middle point.\n    pub midpoint_input: f32,',
    '    /// Whether the optional middle point participates in the Curve.\n    pub midpoint_enabled: bool,\n    /// Input coordinate of the optional draggable middle point.\n    pub midpoint_input: f32,',
    'curve midpoint enabled field',
)
model = replace_once(
    model,
    '            input_black: 0.0,\n            midpoint_input: 0.5,',
    '            input_black: 0.0,\n            midpoint_enabled: false,\n            midpoint_input: 0.5,',
    'curve midpoint default',
)
model = replace_once(
    model,
    '''        if source_schema < 7 {\n            migrate_relative_curves_to_three_points(&mut project.adjustments);\n            for snapshot in &mut project.snapshots {\n                migrate_relative_curves_to_three_points(&mut snapshot.adjustments);\n            }\n        }\n\n        project.schema_version = SHADE_SCHEMA_VERSION;''',
    '''        if source_schema < 7 {\n            migrate_relative_curves_to_three_points(&mut project.adjustments);\n            for snapshot in &mut project.snapshots {\n                migrate_relative_curves_to_three_points(&mut snapshot.adjustments);\n            }\n        }\n\n        // Schema <= 7 always had a midpoint. Keep it enabled when migrating so\n        // existing projects retain the exact same rendering. New v8 Curves\n        // start with only Black/White endpoints until the user adds a midpoint.\n        if source_schema < 8 {\n            enable_legacy_curve_midpoints(&mut project.adjustments);\n            for snapshot in &mut project.snapshots {\n                enable_legacy_curve_midpoints(&mut snapshot.adjustments);\n            }\n        }\n\n        project.schema_version = SHADE_SCHEMA_VERSION;''',
    'curve v8 migration',
)

next_name_pattern = re.compile(
    r'    fn next_snapshot_name\(&self\) -> String \{.*?\n    \}\n\n    pub fn snapshot_name_available',
    re.S,
)
next_name_replacement = '''    fn next_snapshot_name(&self) -> String {\n        let Some(first) = self.snapshots.first() else {\n            return "Test 1".to_owned();\n        };\n\n        let (prefix, first_number) = snapshot_sequence_seed(&first.name);\n        let mut next_number = first_number.saturating_add(1).max(1);\n        for snapshot in &self.snapshots {\n            if let Some(number) = snapshot_sequence_number(&snapshot.name, &prefix) {\n                next_number = next_number.max(number.saturating_add(1));\n            }\n        }\n\n        loop {\n            let candidate = format!("{prefix}{next_number}");\n            if self.snapshot_name_available(&candidate, None) {\n                return candidate;\n            }\n            next_number = next_number.saturating_add(1);\n        }\n    }\n\n    pub fn snapshot_name_available'''
model, n = next_name_pattern.subn(next_name_replacement, model, count=1)
if n != 1:
    raise SystemExit(f'next_snapshot_name replacement failed: {n}')

migration_anchor = 'fn migrate_relative_curves_to_three_points(adjustments: &mut BTreeMap<String, ChannelAdjustment>) {'
if migration_anchor not in model:
    raise SystemExit('migration anchor missing')
helpers = '''fn snapshot_sequence_seed(name: &str) -> (String, u64) {\n    let trimmed = name.trim();\n    let split = trimmed\n        .char_indices()\n        .rev()\n        .find(|(_, ch)| !ch.is_ascii_digit())\n        .map(|(index, ch)| index + ch.len_utf8())\n        .unwrap_or(0);\n    let (prefix, digits) = trimmed.split_at(split);\n    if !digits.is_empty() {\n        if let Ok(number) = digits.parse::<u64>() {\n            return (prefix.to_owned(), number);\n        }\n    }\n    (format!("{}-", trimmed.trim_end_matches('-')), 1)\n}\n\nfn snapshot_sequence_number(name: &str, prefix: &str) -> Option<u64> {\n    let trimmed = name.trim();\n    if trimmed.len() < prefix.len() || !trimmed[..prefix.len()].eq_ignore_ascii_case(prefix) {\n        return None;\n    }\n    trimmed[prefix.len()..].parse::<u64>().ok()\n}\n\nfn enable_legacy_curve_midpoints(adjustments: &mut BTreeMap<String, ChannelAdjustment>) {\n    for adjustment in adjustments.values_mut() {\n        adjustment.curve.midpoint_enabled = true;\n    }\n}\n\n'''
model = model.replace(migration_anchor, helpers + migration_anchor, 1)

apply_curve_pattern = re.compile(r'pub fn apply_curve\(value: f32, curve: Curve\) -> f32 \{.*?\n\}\n\nfn lerp', re.S)
apply_curve_replacement = '''pub fn curve_linear_output(value: f32, curve: Curve) -> f32 {\n    let epsilon = 1.0 / 65_535.0;\n    let x0 = curve.input_black.clamp(0.0, 1.0 - epsilon);\n    let x2 = curve.input_white.clamp(x0 + epsilon, 1.0);\n    let y0 = curve.black.clamp(0.0, 1.0);\n    let y2 = curve.white.clamp(0.0, 1.0);\n    let x = value.clamp(0.0, 1.0);\n    if x <= x0 {\n        return y0;\n    }\n    if x >= x2 {\n        return y2;\n    }\n    let t = (x - x0) / (x2 - x0).max(epsilon);\n    lerp(y0, y2, t).clamp(0.0, 1.0)\n}\n\npub fn apply_curve(value: f32, curve: Curve) -> f32 {\n    if !curve.midpoint_enabled {\n        return curve_linear_output(value, curve);\n    }\n\n    let epsilon = 1.0 / 65_535.0;\n    let x0 = curve.input_black.clamp(0.0, 1.0 - epsilon * 2.0);\n    let x2 = curve.input_white.clamp(x0 + epsilon * 2.0, 1.0);\n    let x1 = curve.midpoint_input.clamp(x0 + epsilon, x2 - epsilon);\n    let y0 = curve.black.clamp(0.0, 1.0);\n    let y1 = curve.midpoint.clamp(0.0, 1.0);\n    let y2 = curve.white.clamp(0.0, 1.0);\n    let x = value.clamp(0.0, 1.0);\n\n    if x <= x0 {\n        return y0;\n    }\n    if x >= x2 {\n        return y2;\n    }\n    if x <= x1 {\n        let t = (x - x0) / (x1 - x0).max(epsilon);\n        lerp(y0, y1, t).clamp(0.0, 1.0)\n    } else {\n        let t = (x - x1) / (x2 - x1).max(epsilon);\n        lerp(y1, y2, t).clamp(0.0, 1.0)\n    }\n}\n\nfn lerp'''
model, n = apply_curve_pattern.subn(apply_curve_replacement, model, count=1)
if n != 1:
    raise SystemExit(f'apply_curve replacement failed: {n}')

# Existing v7 tests that intentionally exercise the middle point must enable it.
model = model.replace(
    '        let curve = Curve {\n            midpoint_input: 0.25,',
    '        let curve = Curve {\n            midpoint_enabled: true,\n            midpoint_input: 0.25,',
    1,
)
model = model.replace(
    '        let curve = Curve {\n            midpoint_input: 0.30,',
    '        let curve = Curve {\n            midpoint_enabled: true,\n            midpoint_input: 0.30,',
    1,
)

test_anchor = '''    #[test]\n    fn levels_gamma_is_relative_to_output_range() {'''
extra_tests = '''    #[test]\n    fn midpoint_is_disabled_by_default_and_two_points_are_linear() {\n        let curve = Curve::default();\n        assert!(!curve.midpoint_enabled);\n        for value in [0.0, 0.1, 0.5, 0.9, 1.0] {\n            assert!((apply_curve(value, curve) - value).abs() < 0.0001);\n        }\n    }\n\n    #[test]\n    fn enabled_midpoint_changes_the_piecewise_curve() {\n        let curve = Curve {\n            midpoint_enabled: true,\n            midpoint_input: 0.5,\n            midpoint: 0.8,\n            ..Curve::default()\n        };\n        assert!((apply_curve(0.5, curve) - 0.8).abs() < 0.0001);\n    }\n\n    #[test]\n    fn snapshot_names_follow_first_snapshot_trailing_number() {\n        let mut project = ShadeProject::default();\n        let first = project.create_snapshot();\n        project.rename_snapshot(first, "XN-A1-1").unwrap();\n        let second = project.create_snapshot();\n        let third = project.create_snapshot();\n        assert_eq!(project.snapshots.iter().find(|s| s.id == second).unwrap().name, "XN-A1-2");\n        assert_eq!(project.snapshots.iter().find(|s| s.id == third).unwrap().name, "XN-A1-3");\n    }\n\n    #[test]\n    fn snapshot_names_append_sequence_when_seed_has_no_number() {\n        let mut project = ShadeProject::default();\n        let first = project.create_snapshot();\n        project.rename_snapshot(first, "Kiln-Test").unwrap();\n        let second = project.create_snapshot();\n        assert_eq!(project.snapshots.iter().find(|s| s.id == second).unwrap().name, "Kiln-Test-2");\n    }\n\n'''
if test_anchor not in model:
    raise SystemExit('model test anchor missing')
model = model.replace(test_anchor, extra_tests + test_anchor, 1)
model = model.replace('A snapshot named ‘{candidate}’ already exists.', "A snapshot named '{candidate}' already exists.")
model_path.write_text(model, encoding='utf-8')

# App -------------------------------------------------------------------------
app_path = ROOT / 'src/app_main.rs'
app = app_path.read_text(encoding='utf-8')
if 'mod thumbnail;' not in app:
    app = replace_once(app, 'mod render;\n', 'mod render;\nmod thumbnail;\n', 'thumbnail module')

save_pattern = re.compile(r'    fn save_project\(&mut self, save_as: bool\) \{.*?\n    \}\n\n    fn export_current_dialog', re.S)
save_replacement = '''    fn save_project(&mut self, save_as: bool) {\n        if self.job.is_some() || self.faces.is_empty() {\n            return;\n        }\n        let target = if !save_as {\n            self.project_path.clone()\n        } else {\n            None\n        };\n        let target = match target {\n            Some(path) => Some(path),\n            None => {\n                let mut dialog = rfd::FileDialog::new()\n                    .add_filter("Shade project", &["shade"])\n                    .set_file_name(format!("{}.shade", sanitize_filename(&self.project.name)));\n                if let Some(parent) = self.faces.first().and_then(|face| face.path.parent()) {\n                    dialog = dialog.set_directory(parent);\n                }\n                dialog.save_file()\n            }\n        };\n        let Some(path) = target else {\n            return;\n        };\n        let mut project = self.project.clone();\n        project.file_metadata = Some(build_project_file_metadata(\n            &self.project,\n            &self.faces,\n            self.current_face,\n        ));\n        let thumbnail_face = self\n            .faces\n            .get(self.current_face)\n            .map(|face| Arc::clone(&face.preview));\n        let face_paths = self\n            .faces\n            .iter()\n            .map(|face| face.path.clone())\n            .collect::<Vec<_>>();\n        let result_path = path.clone();\n        self.launch_job("Saving project", move |progress| {\n            Self::set_progress(\n                &progress,\n                Some(0.15),\n                "Saving project",\n                "Building project thumbnail",\n            );\n            let result = (|| -> Result<(), String> {\n                if let Some(face) = thumbnail_face.as_deref() {\n                    project.thumbnail = Some(thumbnail::build_project_thumbnail(face, &project)?);\n                }\n                Self::set_progress(\n                    &progress,\n                    Some(0.55),\n                    "Saving project",\n                    "Serializing project and metadata",\n                );\n                project.save(&path, &face_paths)\n            })();\n            Self::set_progress(&progress, Some(1.0), "Saving project", "Complete");\n            JobResult::Save {\n                path: result_path,\n                result,\n            }\n        });\n    }\n\n    fn export_current_dialog'''
app, n = save_pattern.subn(save_replacement, app, count=1)
if n != 1:
    raise SystemExit(f'save_project replacement failed: {n}')

# File-information order: filename | bit depth | dimensions | resolution | model | channels.
old_info = '''            ui.strong(title);\n            ui.separator();\n            ui.label(format!("{} × {} px", meta.width, meta.height));\n            ui.label(format!("{}-bit", meta.bit_depth));\n            ui.label(meta.color_model.title());\n            ui.label(format!("{} channels", meta.samples_per_pixel));\n            if dpi_info.has_physical_resolution {\n                ui.label(format!("{:.0} × {:.0} DPI", dpi_info.dpi_x, dpi_info.dpi_y));\n            } else {\n                ui.label(format!(\n                    "{:.0} × {:.0} DPI (default)",\n                    dpi_info.dpi_x, dpi_info.dpi_y\n                ));\n            }'''
new_info = '''            ui.strong(title);\n            ui.separator();\n            ui.label(format!("{}-bit", meta.bit_depth));\n            ui.label(format!("{} x {} px", meta.width, meta.height));\n            if dpi_info.has_physical_resolution {\n                ui.label(format!("{:.0} x {:.0} DPI", dpi_info.dpi_x, dpi_info.dpi_y));\n            } else {\n                ui.label(format!(\n                    "{:.0} x {:.0} DPI (default)",\n                    dpi_info.dpi_x, dpi_info.dpi_y\n                ));\n            }\n            ui.label(meta.color_model.title());\n            ui.label(format!("{} channels", meta.samples_per_pixel));'''
app = replace_once(app, old_info, new_info, 'file info order')

# Snapshot group checkmarks: vector widgets, not Unicode glyphs.
all_check_old = '''            if all_exported {\n                open_all_folder = ui\n                    .small_button("✓")\n                    .on_hover_text("Open the latest export folder for these snapshots")\n                    .clicked();\n            }'''
all_check_new = '''            if all_exported {\n                open_all_folder = ui\n                    .add(VectorIconButton::check().min_size(egui::vec2(20.0, 20.0)))\n                    .on_hover_text("Open the latest export folder for these snapshots")\n                    .clicked();\n            }'''
app = replace_once(app, all_check_old, all_check_new, 'all snapshot check')

day_check_old = '''                if day_exported\n                    && ui\n                        .small_button("✓")\n                        .on_hover_text("Open the latest export folder for this day")\n                        .clicked()\n                {'''
day_check_new = '''                if day_exported\n                    && ui\n                        .add(VectorIconButton::check().min_size(egui::vec2(20.0, 20.0)))\n                        .on_hover_text("Open the latest export folder for this day")\n                        .clicked()\n                {'''
app = replace_once(app, day_check_old, day_check_new, 'day snapshot check')

# Channel rows: vector square Solo indicator.
channel_loop_pattern = re.compile(
    r'        for \(index, name\) in channel_names\.iter\(\)\.enumerate\(\) \{\n            let suffix = if index >= base_count \{.*?\n        \}\n        if self\.solo_channel\.is_some\(\)',
    re.S,
)
channel_loop_new = '''        for (index, name) in channel_names.iter().enumerate() {\n            let suffix = if index >= base_count { "  spot" } else { "" };\n            let accent = channel_color(active_palette.as_ref(), name, index);\n            let is_solo = self.solo_channel == Some(index);\n            let display_name = channel_display_name(active_palette.as_ref(), name, index);\n            let label = format!("{display_name}{suffix}");\n            let response = clickable_channel_row(\n                ui,\n                self.selected_channel == index,\n                is_solo,\n                &label,\n                accent,\n                32.0,\n            )\n            .on_hover_text("Click to select for editing. Click the active channel again to toggle solo preview.");\n            if response.clicked() {\n                self.select_channel(index, true);\n            }\n        }\n        if self.solo_channel.is_some()'''
app, n = channel_loop_pattern.subn(channel_loop_new, app, count=1)
if n != 1:
    raise SystemExit(f'channel loop replacement failed: {n}')

# Palette swatches are painter geometry, not Unicode squares.
palette_pattern = re.compile(r'fn palette_entry_readonly\(ui: &mut egui::Ui, entry: &palette::ChannelPaletteEntry\) \{.*?\n\}', re.S)
palette_new = '''fn palette_entry_readonly(ui: &mut egui::Ui, entry: &palette::ChannelPaletteEntry) {\n    let [r, g, b] = entry.color;\n    ui.horizontal(|ui| {\n        let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());\n        ui.painter()\n            .rect_filled(rect, 2.0, egui::Color32::from_rgb(r, g, b));\n        ui.label(&entry.name);\n    });\n}'''
app, n = palette_pattern.subn(palette_new, app, count=1)
if n != 1:
    raise SystemExit(f'palette swatch replacement failed: {n}')

# Direct Curve editor with an optional midpoint added/removed by double-click.
curve_graph_pattern = re.compile(r'fn curve_editor_graph\(.*?\n\}\n\nfn curve_point_fields', re.S)
curve_graph_new = '''fn curve_editor_graph(\n    ui: &mut egui::Ui,\n    curve: &mut model::Curve,\n    histogram: Option<&[u32; 256]>,\n    accent: Option<egui::Color32>,\n) -> (bool, CurvePointKind) {\n    let desired = egui::vec2(ui.available_width().min(340.0).max(150.0), 210.0);\n    let (rect, graph_response) = ui.allocate_exact_size(desired, egui::Sense::click());\n    let graph_id = ui.make_persistent_id("three-point-curve-editor");\n    let selection_id = graph_id.with("selected-point");\n    let mut selected = ui\n        .data(|data| data.get_temp::<CurvePointKind>(selection_id))\n        .unwrap_or(CurvePointKind::Black);\n    if !curve.midpoint_enabled && selected == CurvePointKind::Midpoint {\n        selected = CurvePointKind::Black;\n    }\n    let mut changed = false;\n    let points = [\n        CurvePointKind::Black,\n        CurvePointKind::Midpoint,\n        CurvePointKind::White,\n    ];\n\n    for point in points {\n        if point == CurvePointKind::Midpoint && !curve.midpoint_enabled {\n            continue;\n        }\n        let (input, output) = curve_point_xy(*curve, point);\n        let center = curve_point_screen(rect, input, output);\n        let hit_rect = egui::Rect::from_center_size(center, egui::vec2(22.0, 22.0));\n        let response = ui.interact(\n            hit_rect,\n            graph_id.with(point),\n            egui::Sense::click_and_drag(),\n        );\n        if point == CurvePointKind::Midpoint && response.double_clicked() {\n            curve.midpoint_enabled = false;\n            selected = CurvePointKind::Black;\n            ui.data_mut(|data| data.insert_temp(selection_id, selected));\n            changed = true;\n            continue;\n        }\n        if response.clicked() || response.drag_started() {\n            selected = point;\n            ui.data_mut(|data| data.insert_temp(selection_id, point));\n        }\n        if response.dragged() {\n            if let Some(pointer) = response.interact_pointer_pos() {\n                let input = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);\n                let output = ((rect.bottom() - pointer.y) / rect.height()).clamp(0.0, 1.0);\n                set_curve_point(curve, point, input, output);\n                selected = point;\n                ui.data_mut(|data| data.insert_temp(selection_id, point));\n                changed = true;\n            }\n        }\n    }\n\n    if !curve.midpoint_enabled && graph_response.double_clicked() {\n        if let Some(pointer) = graph_response.interact_pointer_pos() {\n            let input = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);\n            let gap = 1.0 / 255.0;\n            if input > curve.input_black + gap && input < curve.input_white - gap {\n                let output = model::curve_linear_output(input, *curve);\n                let line_point = curve_point_screen(rect, input, output);\n                if pointer.distance(line_point) <= 16.0 {\n                    curve.midpoint_enabled = true;\n                    curve.midpoint_input = input;\n                    curve.midpoint = output;\n                    selected = CurvePointKind::Midpoint;\n                    ui.data_mut(|data| data.insert_temp(selection_id, selected));\n                    changed = true;\n                }\n            }\n        }\n    }\n\n    let painter = ui.painter_at(rect);\n    painter.rect_stroke(\n        rect,\n        2.0,\n        ui.visuals().widgets.noninteractive.bg_stroke,\n        egui::StrokeKind::Inside,\n    );\n    if let Some(bins) = histogram {\n        let max_value = bins.iter().copied().max().unwrap_or(1).max(1) as f32;\n        let hist_color = accent\n            .unwrap_or(ui.visuals().weak_text_color())\n            .gamma_multiply(0.30);\n        for (index, value) in bins.iter().enumerate() {\n            let x = egui::lerp(rect.x_range(), index as f32 / 255.0);\n            let h = *value as f32 / max_value * rect.height();\n            painter.line_segment(\n                [\n                    egui::pos2(x, rect.bottom()),\n                    egui::pos2(x, rect.bottom() - h),\n                ],\n                egui::Stroke::new(1.0, hist_color),\n            );\n        }\n    }\n    painter.line_segment(\n        [\n            egui::pos2(rect.left(), rect.bottom()),\n            egui::pos2(rect.right(), rect.top()),\n        ],\n        egui::Stroke::new(1.0, ui.visuals().weak_text_color()),\n    );\n    let curve_color = accent.unwrap_or(ui.visuals().selection.stroke.color);\n    let mut last = None;\n    for step in 0..=128 {\n        let x = step as f32 / 128.0;\n        let y = model::apply_curve(x, *curve);\n        let point = curve_point_screen(rect, x, y);\n        if let Some(previous) = last {\n            painter.line_segment([previous, point], egui::Stroke::new(2.0, curve_color));\n        }\n        last = Some(point);\n    }\n    for point in points {\n        if point == CurvePointKind::Midpoint && !curve.midpoint_enabled {\n            continue;\n        }\n        let (input, output) = curve_point_xy(*curve, point);\n        let center = curve_point_screen(rect, input, output);\n        let is_selected = point == selected;\n        let radius = if is_selected { 6.5 } else { 5.0 };\n        let fill = if is_selected {\n            curve_color\n        } else {\n            ui.visuals().extreme_bg_color\n        };\n        painter.circle_filled(center, radius, fill);\n        painter.circle_stroke(center, radius, egui::Stroke::new(2.0, curve_color));\n    }\n    (changed, selected)\n}\n\nfn curve_point_fields'''
app, n = curve_graph_pattern.subn(curve_graph_new, app, count=1)
if n != 1:
    raise SystemExit(f'curve editor replacement failed: {n}')

# Update helper text for optional midpoint behavior.
app = app.replace(
    'ui.small("Drag any of the three points directly. Input / Output use Photoshop-style 0-255 values.");',
    'ui.small("Double-click the Curve line to add the midpoint; double-click the midpoint to remove it. Drag active points directly. Input / Output use Photoshop-style 0-255 values.");',
)

# Add a vector channel row helper after clickable_row.
clickable_end = '''    response\n}\n\nfn snapshot_row_with_actions'''
channel_helper = '''    response\n}\n\nfn clickable_channel_row(\n    ui: &mut egui::Ui,\n    selected: bool,\n    solo: bool,\n    label: &str,\n    accent: egui::Color32,\n    height: f32,\n) -> egui::Response {\n    let width = ui.available_width().max(1.0);\n    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());\n    let visuals = ui.visuals();\n    let fill = if selected {\n        visuals.selection.bg_fill.gamma_multiply(0.72)\n    } else if response.hovered() {\n        visuals.widgets.hovered.bg_fill\n    } else {\n        egui::Color32::TRANSPARENT\n    };\n    if fill != egui::Color32::TRANSPARENT {\n        ui.painter().rect_filled(rect, 4.0, fill);\n    }\n\n    let indicator = egui::Rect::from_center_size(\n        egui::pos2(rect.left() + 14.0, rect.center().y),\n        egui::vec2(11.0, 11.0),\n    );\n    if solo {\n        ui.painter().rect_filled(indicator, 1.5, accent);\n    } else {\n        ui.painter().rect_stroke(\n            indicator,\n            1.5,\n            egui::Stroke::new(1.5, accent),\n            egui::StrokeKind::Inside,\n        );\n    }\n    ui.painter().text(\n        egui::pos2(rect.left() + 28.0, rect.center().y),\n        egui::Align2::LEFT_CENTER,\n        label,\n        egui::FontId::proportional(14.0),\n        accent,\n    );\n    response\n}\n\nfn snapshot_row_with_actions'''
if clickable_end not in app:
    raise SystemExit('clickable_row helper anchor missing')
app = app.replace(clickable_end, channel_helper, 1)

# Project file metadata helper near unix_ms_now.
unix_anchor = 'fn unix_ms_now() -> i64 {'
metadata_helper = '''fn build_project_file_metadata(\n    project: &ShadeProject,\n    faces: &[RuntimeFace],\n    active_face_index: usize,\n) -> model::ProjectFileMetadata {\n    let mut total_source_bytes = 0u64;\n    let mut entries = Vec::with_capacity(faces.len());\n    for (index, face) in faces.iter().enumerate() {\n        let fs_metadata = std::fs::metadata(&face.path).ok();\n        let file_size_bytes = fs_metadata.as_ref().map(|meta| meta.len()).unwrap_or(0);\n        total_source_bytes = total_source_bytes.saturating_add(file_size_bytes);\n        let modified_at_unix_ms = fs_metadata\n            .as_ref()\n            .and_then(|meta| meta.modified().ok())\n            .and_then(system_time_unix_ms);\n        let tiff = &face.preview.metadata;\n        let label = project\n            .faces\n            .get(index)\n            .map(|face| face.label.clone())\n            .unwrap_or_else(|| {\n                face.path\n                    .file_name()\n                    .map(|name| name.to_string_lossy().into_owned())\n                    .unwrap_or_else(|| format!("Face {}", index + 1))\n            });\n        entries.push(model::FaceFileMetadata {\n            label,\n            source_file_name: face\n                .path\n                .file_name()\n                .map(|name| name.to_string_lossy().into_owned())\n                .unwrap_or_default(),\n            width: tiff.width,\n            height: tiff.height,\n            bit_depth: tiff.bit_depth,\n            color_model: tiff.color_model.title().to_owned(),\n            channel_count: tiff.samples_per_pixel,\n            base_channel_count: tiff.base_channel_count,\n            channel_names: tiff.channel_names.clone(),\n            dpi_x: face.dpi.dpi_x,\n            dpi_y: face.dpi.dpi_y,\n            dpi_from_source: face.dpi.has_physical_resolution,\n            resolution_unit: face.dpi.unit,\n            file_size_bytes,\n            modified_at_unix_ms,\n        });\n    }\n    model::ProjectFileMetadata {\n        saved_at_unix_ms: unix_ms_now(),\n        face_count: entries.len(),\n        active_face_index: active_face_index.min(entries.len().saturating_sub(1)),\n        total_source_bytes,\n        faces: entries,\n    }\n}\n\nfn system_time_unix_ms(value: std::time::SystemTime) -> Option<i64> {\n    value\n        .duration_since(std::time::UNIX_EPOCH)\n        .ok()\n        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)\n}\n\n'''
if unix_anchor not in app:
    raise SystemExit('unix helper anchor missing')
app = app.replace(unix_anchor, metadata_helper + unix_anchor, 1)

# Remove remaining font-dependent UI symbol glyphs. Icons are either painter geometry
# or plain ASCII text after these replacements.
app = app.replace('format!("Restart → {}", info.version)', 'format!("Restart {}", info.version)')
app = app.replace('format!("Empty → {fallback}")', 'format!("Empty uses {fallback}")')
app = app.replace('"—".to_owned()', '"-".to_owned()')
app = app.replace('" • modified"', '" * modified"')
app = app.replace('"−"', '"-"')
app = app.replace('Copyright © 2026 Emad Ghasemi', 'Copyright (c) 2026 Emad Ghasemi')
app = app.replace('Histogram — ', 'Histogram - ')

# Fail the source migration if known icon/symbol glyphs remain in the UI source.
for glyph in ['✓', '■', '□', '→', '•', '×', '−', '—', '©']:
    if glyph in app:
        raise SystemExit(f'font-dependent UI glyph still present: {glyph!r}')

app_path.write_text(app, encoding='utf-8')

# Release notes ---------------------------------------------------------------
notes_path = ROOT / 'RELEASE_NOTES.md'
notes = notes_path.read_text(encoding='utf-8')
entry = '''# Shade Editor 0.8.0\n\nOptional midpoint Curve editing, deterministic Snapshot naming, and richer self-contained project files.\n\n- Curve starts with Black/White endpoints only. Double-click near the rendered line to add the midpoint at the calculated on-line position; double-click the midpoint to remove it.\n- Existing schema v7 projects migrate with their midpoint enabled so their rendering is unchanged.\n- Remaining UI icon glyphs are replaced by vector painter geometry or plain ASCII text, including Snapshot checks, Channel Solo indicators, and palette swatches.\n- Face information is ordered as filename, bit depth, pixel dimensions, DPI, color model, and channel count.\n- Snapshot names follow the first Snapshot's trailing numeric sequence (for example XN-A1-1, XN-A1-2, XN-A1-3).\n- .shade schema v8 embeds a PNG project thumbnail (max 256 px) and cached file/project metadata: face count, active face, source dimensions, bit depth, color model, channels, DPI, file size, and source modified time.\n- Thumbnail generation runs in the existing background Save job.\n\n'''
if not notes.startswith('# Shade Editor 0.8.0'):
    notes = entry + notes
notes_path.write_text(notes, encoding='utf-8')
