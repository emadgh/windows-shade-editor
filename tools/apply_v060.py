from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


# --- model v6 -------------------------------------------------------------
model = (ROOT / "src/model_v5.rs").read_text(encoding="utf-8")
model = replace_once(
    model,
    "use serde::{Deserialize, Serialize};\n\npub const SHADE_SCHEMA_VERSION: u32 = 5;",
    "use serde::{Deserialize, Serialize};\n\nuse crate::palette::ChannelPalette;\n\npub const SHADE_SCHEMA_VERSION: u32 = 6;",
    "model imports/schema",
)
model = replace_once(
    model,
    "    #[serde(default)]\n    pub test_code: TestCodeConfig,\n}",
    "    #[serde(default)]\n    pub test_code: TestCodeConfig,\n    /// Visual channel aliases/colors selected for this project. TIFF channel\n    /// names and separation order are never changed by this palette.\n    #[serde(default)]\n    pub channel_palette: Option<ChannelPalette>,\n}",
    "model project palette field",
)
model = replace_once(
    model,
    "            test_code: TestCodeConfig::default(),\n        }",
    "            test_code: TestCodeConfig::default(),\n            channel_palette: None,\n        }",
    "model project palette default",
)
model = model.replace("schema v5", "schema v6")
(ROOT / "src/model_v6.rs").write_text(model, encoding="utf-8")


# --- export fallback DPI --------------------------------------------------
export = (ROOT / "src/export_v4.rs").read_text(encoding="utf-8")
export = replace_once(
    export,
    "pub fn export_face(source: &Path, destination: &Path, project: &ShadeProject) -> Result<(), String> {\n    export_face_with_progress(source, destination, project, |_, _| {})\n}",
    "pub fn export_face(\n    source: &Path,\n    destination: &Path,\n    project: &ShadeProject,\n    default_dpi: f64,\n) -> Result<(), String> {\n    export_face_with_progress(source, destination, project, default_dpi, |_, _| {})\n}",
    "export_face signature",
)
export = replace_once(
    export,
    "    project: &ShadeProject,\n    mut progress: F,",
    "    project: &ShadeProject,\n    default_dpi: f64,\n    mut progress: F,",
    "export progress signature",
)
export = replace_once(
    export,
    "    let dpi_info = dpi::read_dpi(source);",
    "    let dpi_info = dpi::read_dpi(source, default_dpi);",
    "export dpi read",
)
export = replace_once(
    export,
    "    if let (Some(x), Some(y)) = (dpi_info.raw_x, dpi_info.raw_y) {\n        image.x_resolution(dpi::rational(x));\n        image.y_resolution(dpi::rational(y));\n        image.encoder().write_tag(Tag::ResolutionUnit, dpi_info.unit)\n            .map_err(|err| format!(\"Cannot preserve TIFF resolution unit: {err}\"))?;\n    }",
    "    let (resolution_x, resolution_y, resolution_unit) = dpi_info.effective_tiff_resolution();\n    image.x_resolution(dpi::rational(resolution_x));\n    image.y_resolution(dpi::rational(resolution_y));\n    image\n        .encoder()\n        .write_tag(Tag::ResolutionUnit, resolution_unit)\n        .map_err(|err| format!(\"Cannot preserve/write TIFF resolution unit: {err}\"))?;",
    "export resolution metadata",
)
(ROOT / "src/export_v6.rs").write_text(export, encoding="utf-8")


# --- application ----------------------------------------------------------
app_path = ROOT / "src/app_main.rs"
app = app_path.read_text(encoding="utf-8")
app = replace_once(app, 'mod dpi;\n#[path = "export_v4.rs"]\nmod export;\n#[path = "model_v5.rs"]\nmod model;\nmod render;\n#[path = "settings_v4.rs"]\nmod settings;', 'mod dpi;\n#[path = "export_v6.rs"]\nmod export;\n#[path = "model_v6.rs"]\nmod model;\nmod palette;\nmod render;\n#[path = "settings_v6.rs"]\nmod settings;', "app module wiring")
app = replace_once(
    app,
    "use model::{ChannelAdjustment, ShadeProject, TestCodePosition};\nuse settings::AppSettings;",
    "use model::{ChannelAdjustment, ShadeProject, TestCodePosition};\nuse palette::ChannelPalette;\nuse settings::AppSettings;",
    "app palette import",
)

# Build the first project with the configured default palette (or defer when Auto is selected).
app = replace_once(
    app,
    "        let (render_tx, render_rx) = mpsc::channel();\n        let log = app_log::AppLog::default();",
    "        let (render_tx, render_rx) = mpsc::channel();\n        let mut project = ShadeProject::default();\n        project.channel_palette = settings.default_project_palette();\n        let log = app_log::AppLog::default();",
    "new project bootstrap",
)
app = replace_once(
    app,
    "            project: ShadeProject::default(),",
    "            project,",
    "new project field",
)
app = replace_once(
    app,
    "        self.project = ShadeProject::default();\n        self.project_path = None;",
    "        self.project = ShadeProject::default();\n        self.project.channel_palette = self.settings.default_project_palette();\n        self.project_path = None;",
    "new_project palette",
)

# DPI on load.
app = replace_once(
    app,
    "        let max_dimension = self.settings.max_preview_dimension;\n        self.launch_job(\"Opening TIFF\", move |progress| {",
    "        let max_dimension = self.settings.max_preview_dimension;\n        let default_dpi = self.settings.default_dpi;\n        self.launch_job(\"Opening TIFF\", move |progress| {",
    "add face default dpi capture",
)
app = app.replace("dpi: dpi::read_dpi(&path),", "dpi: dpi::read_dpi(&path, default_dpi),", 1)
app = replace_once(
    app,
    "        let max_dimension = self.settings.max_preview_dimension;\n        self.launch_job(\"Opening project\", move |progress| {",
    "        let max_dimension = self.settings.max_preview_dimension;\n        let default_dpi = self.settings.default_dpi;\n        self.launch_job(\"Opening project\", move |progress| {",
    "open project default dpi capture",
)
app = app.replace("dpi: dpi::read_dpi(&source),", "dpi: dpi::read_dpi(&source, default_dpi),", 1)

# Every export path uses the same configurable fallback DPI.
for label, needle in [
    ("current export", "        let project = self.project.clone();\n        self.launch_job(\"Exporting TIFF\", move |progress| {"),
    ("all faces export", "        let project = self.project.clone();\n        self.launch_job(\"Exporting faces\", move |progress| {"),
]:
    app = replace_once(
        app,
        needle,
        needle.replace("        self.launch_job", "        let default_dpi = self.settings.default_dpi;\n        self.launch_job"),
        f"{label} dpi capture",
    )
app = replace_once(
    app,
    "        let mut project = self.project.clone();\n        project.adjustments = snapshot.adjustments.clone();",
    "        let mut project = self.project.clone();\n        let default_dpi = self.settings.default_dpi;\n        project.adjustments = snapshot.adjustments.clone();",
    "single snapshot dpi capture",
)
app = replace_once(
    app,
    "        let base_project = self.project.clone();\n        let snapshots = snapshot_ids",
    "        let base_project = self.project.clone();\n        let default_dpi = self.settings.default_dpi;\n        let snapshots = snapshot_ids",
    "snapshot group dpi capture",
)
# Insert default_dpi before each export progress closure. There are four call shapes in app_main.
app = app.replace("                &project,\n                |fraction, detail| {", "                &project,\n                default_dpi,\n                |fraction, detail| {")
app = app.replace("                        &project,\n                        |inner, detail| {", "                        &project,\n                        default_dpi,\n                        |inner, detail| {")

# Palette selection helper methods go before open_export_folder.
helper = '''    fn ensure_project_palette_for_model(&mut self, color_model: tiff_io::ColorModel) -> bool {
        if self.project.channel_palette.is_some() {
            return false;
        }
        let palette = self.settings.default_project_palette().or_else(|| match color_model {
            tiff_io::ColorModel::Rgb => Some(palette::builtin_rgb()),
            tiff_io::ColorModel::Cmyk => Some(palette::builtin_cmyk()),
            _ => None,
        });
        if let Some(palette) = palette {
            self.project.channel_palette = Some(palette);
            true
        } else {
            false
        }
    }

    fn select_project_palette(&mut self, palette: ChannelPalette) {
        if self.project.channel_palette.as_ref() == Some(&palette) {
            return;
        }
        let name = palette.name.clone();
        self.project.channel_palette = Some(palette);
        self.project_dirty = true;
        self.report_info(format!("Channel palette: {name}"));
    }

'''
app = replace_once(app, "    fn open_export_folder(&mut self, folder: &str) {", helper + "    fn open_export_folder(&mut self, folder: &str) {", "palette helper methods")

# Resolve Auto palette after Face load/open.
app = replace_once(
    app,
    "            JobResult::AddFaces { faces, errors } => {\n                let added = faces.len();\n                for item in faces {",
    "            JobResult::AddFaces { faces, errors } => {\n                let added = faces.len();\n                if let Some(first) = faces.first() {\n                    self.ensure_project_palette_for_model(first.preview.metadata.color_model);\n                }\n                for item in faces {",
    "palette resolve add faces",
)
app = replace_once(
    app,
    "                    self.current_face = 0;\n                    self.selected_channel = 0;\n                    self.solo_channel = None;\n                    self.adjustment_scope = AdjustmentScope::Selected;",
    "                    self.current_face = 0;\n                    self.selected_channel = 0;\n                    self.solo_channel = None;\n                    if let Some(first) = self.faces.first() {\n                        let color_model = first.preview.metadata.color_model;\n                        self.ensure_project_palette_for_model(color_model);\n                    }\n                    self.adjustment_scope = AdjustmentScope::Selected;",
    "palette resolve open project",
)

# Test Code channel aliases while retaining real TIFF channel keys.
app = replace_once(
    app,
    "        let fallback = self\n            .project\n            .active_snapshot_name()",
    "        let palette = self.project.channel_palette.clone();\n        let fallback = self\n            .project\n            .active_snapshot_name()",
    "test code palette clone",
)
old_combo = '''            if !channel_names.is_empty() {
                egui::ComboBox::from_label("Ink / channel")
                    .selected_text(&self.project.test_code.channel)
                    .show_ui(ui, |ui| {
                        for name in &channel_names {
                            changed |= ui
                                .selectable_value(
                                    &mut self.project.test_code.channel,
                                    name.clone(),
                                    name,
                                )
                                .changed();
                        }
                    });
            }'''
new_combo = '''            if !channel_names.is_empty() {
                let selected_index = channel_names
                    .iter()
                    .position(|name| name == &self.project.test_code.channel)
                    .unwrap_or(0);
                let selected_display = channel_display_name(
                    palette.as_ref(),
                    &channel_names[selected_index],
                    selected_index,
                );
                egui::ComboBox::from_label("Ink / channel")
                    .selected_text(selected_display)
                    .show_ui(ui, |ui| {
                        for (index, name) in channel_names.iter().enumerate() {
                            let display = channel_display_name(palette.as_ref(), name, index);
                            changed |= ui
                                .selectable_value(
                                    &mut self.project.test_code.channel,
                                    name.clone(),
                                    display,
                                )
                                .changed();
                        }
                    });
            }'''
app = replace_once(app, old_combo, new_combo, "test code channel aliases")

# Channel panel: Palette selector + aliases/colors.
app = replace_once(
    app,
    "        self.selected_channel = self.selected_channel.min(channel_names.len() - 1);\n\n        ui.heading(\"Channels\");",
    "        self.selected_channel = self.selected_channel.min(channel_names.len() - 1);\n        let mut active_palette = self.project.channel_palette.clone();\n        let palette_library = self.settings.palette_library();\n\n        ui.horizontal(|ui| {\n            ui.heading(\"Channels\");\n            let selected = active_palette\n                .as_ref()\n                .map(|palette| palette.name.as_str())\n                .unwrap_or(\"TIFF channel names\");\n            let mut requested_palette = None;\n            egui::ComboBox::from_id_salt(\"project-channel-palette\")\n                .selected_text(selected)\n                .width(155.0)\n                .show_ui(ui, |ui| {\n                    for palette in &palette_library {\n                        if ui\n                            .selectable_label(\n                                active_palette.as_ref().is_some_and(|current| current.id == palette.id),\n                                &palette.name,\n                            )\n                            .clicked()\n                        {\n                            requested_palette = Some(palette.clone());\n                        }\n                    }\n                });\n            if let Some(palette) = requested_palette {\n                active_palette = Some(palette.clone());\n                self.select_project_palette(palette);\n            }\n        });",
    "channel palette combo",
)
app = replace_once(app, "            let accent = channel_color(name, index);", "            let accent = channel_color(active_palette.as_ref(), name, index);", "channel row palette color")
app = replace_once(app, "            let label = format!(\"{indicator}  {name}{suffix}\");", "            let display_name = channel_display_name(active_palette.as_ref(), name, index);\n            let label = format!(\"{indicator}  {display_name}{suffix}\");", "channel row palette name")
app = app.replace(".then(|| channel_color(name, index));", ".then(|| channel_color(active_palette.as_ref(), name, index));", 1)
app = replace_once(app, "                ui.colored_label(accent.unwrap_or(ui.visuals().text_color()), name);", "                let display = channel_display_name(active_palette.as_ref(), name, index);\n                ui.colored_label(accent.unwrap_or(ui.visuals().text_color()), display);", "all histogram palette label")
app = replace_once(app, ".then(|| channel_color(&channel_names[index], index));", ".then(|| channel_color(active_palette.as_ref(), &channel_names[index], index));", "selected histogram palette color")
app = replace_once(app, "            ui.strong(format!(\"Histogram — {}\", channel_names[index]));", "            let display = channel_display_name(active_palette.as_ref(), &channel_names[index], index);\n            ui.strong(format!(\"Histogram — {display}\"));", "selected histogram palette label")

# Adjustment panel carries a cloned palette through helper UIs to avoid mut/immut borrow conflicts.
app = replace_once(
    app,
    "        let output_name = channel_names[self.selected_channel].clone();\n        let all_adjusted_histograms",
    "        let output_name = channel_names[self.selected_channel].clone();\n        let palette = self.project.channel_palette.clone();\n        let output_display = channel_display_name(palette.as_ref(), &output_name, self.selected_channel);\n        let all_adjusted_histograms",
    "adjustment palette clone",
)
app = replace_once(app, ".then(|| channel_color(&output_name, self.selected_channel));", ".then(|| channel_color(palette.as_ref(), &output_name, self.selected_channel));", "adjustment control palette")
app = replace_once(app, ".then(|| channel_color(&output_name, self.selected_channel));", ".then(|| channel_color(palette.as_ref(), &output_name, self.selected_channel));", "adjustment panel palette")
app = replace_once(app, "                &output_name,\n            );", "                &output_display,\n            );", "adjustment scope display name")
app = replace_once(app, "                    ui.colored_label(color, format!(\"Editing: {output_name}\"));", "                    ui.colored_label(color, format!(\"Editing: {output_display}\"));", "adjustment editing alias")
app = replace_once(
    app,
    "                        active_histogram.as_ref(),\n                        control_accent,\n                    ),",
    "                        active_histogram.as_ref(),\n                        control_accent,\n                        palette.as_ref(),\n                    ),",
    "selected adjustment palette arg",
)
app = replace_once(
    app,
    "                        &all_adjusted_histograms,\n                        control_accent,\n                    ),",
    "                        &all_adjusted_histograms,\n                        control_accent,\n                        palette.as_ref(),\n                    ),",
    "all adjustment palette arg",
)
app = replace_once(
    app,
    "        histogram: Option<&[u32; 256]>,\n        accent: Option<egui::Color32>,\n    ) -> bool {",
    "        histogram: Option<&[u32; 256]>,\n        accent: Option<egui::Color32>,\n        palette: Option<&ChannelPalette>,\n    ) -> bool {",
    "selected adjustment signature",
)
app = app.replace("mixer_ui(ui, adjustment, output_name, channel_names, accent)", "mixer_ui(ui, adjustment, output_name, channel_names, accent, palette)")
app = replace_once(
    app,
    "        histograms: &[[u32; 256]],\n        accent: Option<egui::Color32>,\n    ) -> bool {",
    "        histograms: &[[u32; 256]],\n        accent: Option<egui::Color32>,\n        palette: Option<&ChannelPalette>,\n    ) -> bool {",
    "all adjustment signature",
)
# all_curves_ui and all_mixers_ui calls: add palette argument after colorize.
app = app.replace("                    self.settings.colorize_adjustments,\n                    self.settings.show_curve_histogram,", "                    self.settings.colorize_adjustments,\n                    self.settings.show_curve_histogram,\n                    palette,")
app = app.replace("                    self.settings.colorize_adjustments,\n                ),", "                    self.settings.colorize_adjustments,\n                    palette,\n                ),")

# Viewport DPI label.
app = replace_once(
    app,
    "            } else {\n                ui.label(\"DPI not set (72 used for test-code sizing)\");\n            }",
    "            } else {\n                ui.label(format!(\"{:.0} × {:.0} DPI (default)\", dpi_info.dpi_x, dpi_info.dpi_y));\n            }",
    "viewport fallback dpi label",
)

# Settings: configurable DPI and palette library.
app = replace_once(
    app,
    "                changed |= ui\n                    .add(\n                        egui::Slider::new(&mut self.settings.max_preview_dimension, 600..=4000)\n                            .text(\"Preview max dimension\"),\n                    )\n                    .changed();\n                ui.separator();",
    "                changed |= ui\n                    .add(\n                        egui::Slider::new(&mut self.settings.max_preview_dimension, 600..=4000)\n                            .text(\"Preview max dimension\"),\n                    )\n                    .changed();\n                let old_default_dpi = self.settings.default_dpi;\n                changed |= ui\n                    .add(\n                        egui::Slider::new(&mut self.settings.default_dpi, 72.0..=1200.0)\n                            .text(\"Default DPI\")\n                            .suffix(\" dpi\"),\n                    )\n                    .changed();\n                ui.small(\"Used when a TIFF has no valid physical DPI. Default: 220. Exported TIFFs receive this DPI when the source does not provide one.\");\n                let dpi_changed = (old_default_dpi - self.settings.default_dpi).abs() > f64::EPSILON;\n                if dpi_changed {\n                    for face in &mut self.faces {\n                        if face.dpi.used_default {\n                            face.dpi = dpi::DpiInfo::with_default(self.settings.default_dpi);\n                        }\n                    }\n                }\n                ui.separator();",
    "settings default dpi",
)
settings_palette_section = '''                ui.separator();
                ui.heading("Channel palettes");
                ui.small("Palettes change only UI channel names/colors. TIFF channel names and separation order stay untouched. The active project palette is saved inside the .shade file.");
                let palette_library = self.settings.palette_library();
                let default_palette_name = if self.settings.default_palette_id == palette::AUTO_PALETTE_ID {
                    "Automatic — CMYK/RGB from first Face".to_owned()
                } else {
                    palette_library
                        .iter()
                        .find(|palette| palette.id == self.settings.default_palette_id)
                        .map(|palette| palette.name.clone())
                        .unwrap_or_else(|| "Automatic — CMYK/RGB from first Face".to_owned())
                };
                egui::ComboBox::from_label("Default palette for new projects")
                    .selected_text(default_palette_name)
                    .show_ui(ui, |ui| {
                        changed |= ui
                            .selectable_value(
                                &mut self.settings.default_palette_id,
                                palette::AUTO_PALETTE_ID.to_owned(),
                                "Automatic — CMYK/RGB from first Face",
                            )
                            .changed();
                        for palette in &palette_library {
                            changed |= ui
                                .selectable_value(
                                    &mut self.settings.default_palette_id,
                                    palette.id.clone(),
                                    &palette.name,
                                )
                                .changed();
                        }
                    });

                ui.label("Built-in palettes (read-only)");
                for builtin in palette::builtin_palettes() {
                    egui::CollapsingHeader::new(&builtin.name)
                        .id_salt(format!("builtin-palette-{}", builtin.id))
                        .show(ui, |ui| {
                            for entry in &builtin.channels {
                                palette_entry_readonly(ui, entry);
                            }
                        });
                }

                let mut delete_palette = None;
                let mut add_channel_to = None;
                let mut remove_channel = None;
                for custom in &mut self.settings.custom_palettes {
                    let custom_id = custom.id.clone();
                    egui::CollapsingHeader::new(format!("Custom — {}", custom.name))
                        .id_salt(format!("custom-palette-{custom_id}"))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Palette name");
                                changed |= ui.text_edit_singleline(&mut custom.name).changed();
                                if ui.small_button("Delete palette").clicked() {
                                    delete_palette = Some(custom_id.clone());
                                }
                            });
                            ui.add_space(3.0);
                            for (index, entry) in custom.channels.iter_mut().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.label(format!("{}", index + 1));
                                    changed |= ui
                                        .add(egui::TextEdit::singleline(&mut entry.name).desired_width(130.0))
                                        .changed();
                                    changed |= ui.color_edit_button_srgb(&mut entry.color).changed();
                                    if ui.small_button("−").on_hover_text("Remove channel slot").clicked() {
                                        remove_channel = Some((custom_id.clone(), index));
                                    }
                                });
                            }
                            if ui.small_button("+ Channel slot").clicked() {
                                add_channel_to = Some(custom_id.clone());
                            }
                        });
                }
                if let Some((id, index)) = remove_channel {
                    if let Some(custom) = self.settings.custom_palettes.iter_mut().find(|item| item.id == id) {
                        if index < custom.channels.len() {
                            custom.channels.remove(index);
                            changed = true;
                        }
                    }
                }
                if let Some(id) = add_channel_to {
                    if let Some(custom) = self.settings.custom_palettes.iter_mut().find(|item| item.id == id) {
                        let number = custom.channels.len() + 1;
                        let color = palette::fallback_channel_color("Spot", number - 1);
                        custom.channels.push(palette::ChannelPaletteEntry {
                            name: format!("Ink {number}"),
                            color,
                        });
                        changed = true;
                    }
                }
                if let Some(id) = delete_palette {
                    changed |= self.settings.delete_custom_palette(&id);
                }
                if ui.button("+ New custom palette").clicked() {
                    self.settings.create_custom_palette();
                    changed = true;
                }
'''
app = replace_once(
    app,
    "                if dark_changed {\n                    apply_theme(ctx, self.settings.dark_mode);\n                }",
    settings_palette_section + "                if dark_changed {\n                    apply_theme(ctx, self.settings.dark_mode);\n                }",
    "settings palette section",
)
# Make settings window usable for palette editing.
app = replace_once(app, '.resizable(false)\n            .show(ctx, |ui| {\n                ui.heading("Application");', '.resizable(true)\n            .default_size([640.0, 760.0])\n            .show(ctx, |ui| {\n                egui::ScrollArea::vertical().show(ui, |ui| {\n                ui.heading("Application");', "settings window scroll start")
app = replace_once(app, "                }\n            });\n        self.show_settings = open;", "                }\n                });\n            });\n        self.show_settings = open;", "settings window scroll end")

# Function signatures and display/color aliases in Mixer/All Curves.
app = replace_once(
    app,
    "    channel_names: &[String],\n    accent: Option<egui::Color32>,\n) -> bool {\n    with_accent(ui, accent, |ui| {\n        if let Some(color) = accent {\n            ui.colored_label(color, format!(\"Output: {output_name}\"));\n        } else {\n            ui.label(format!(\"Output: {output_name}\"));\n        }",
    "    channel_names: &[String],\n    accent: Option<egui::Color32>,\n    palette: Option<&ChannelPalette>,\n) -> bool {\n    with_accent(ui, accent, |ui| {\n        let output_index = channel_names.iter().position(|name| name == output_name).unwrap_or(0);\n        let output_display = channel_display_name(palette, output_name, output_index);\n        if let Some(color) = accent {\n            ui.colored_label(color, format!(\"Output: {output_display}\"));\n        } else {\n            ui.label(format!(\"Output: {output_display}\"));\n        }",
    "mixer signature/output alias",
)
app = replace_once(app, "            let row_accent = accent.map(|_| channel_color(name, index));", "            let row_accent = accent.map(|_| channel_color(palette, name, index));", "mixer row palette color")
app = replace_once(app, "                    .text(name)\n                    .trailing_fill(true);", "                    .text(channel_display_name(palette, name, index))\n                    .trailing_fill(true);", "mixer row alias")
app = replace_once(
    app,
    "    colorize: bool,\n    show_histogram: bool,\n) -> bool {",
    "    colorize: bool,\n    show_histogram: bool,\n    palette: Option<&ChannelPalette>,\n) -> bool {",
    "all curves signature",
)
app = replace_once(app, "    let broadcast_accent = colorize.then(|| channel_color(template_name, template_index));", "    let broadcast_accent = colorize.then(|| channel_color(palette, template_name, template_index));", "broadcast palette color")
app = replace_once(app, "        let accent = colorize.then(|| channel_color(name, index));", "        let accent = colorize.then(|| channel_color(palette, name, index));", "per curve palette color")
app = replace_once(app, "            egui::RichText::new(format!(\"●  {name}\")).color(color)", "            egui::RichText::new(format!(\"●  {}\", channel_display_name(palette, name, index))).color(color)", "per curve alias colored")
app = replace_once(app, "            egui::RichText::new(name)", "            egui::RichText::new(channel_display_name(palette, name, index))", "per curve alias plain")
app = replace_once(
    app,
    "    channel_names: &[String],\n    colorize: bool,\n) -> bool {\n    let mut changed = false;\n    for (index, output_name) in channel_names.iter().enumerate() {\n        ui.collapsing(format!(\"Output — {output_name}\"), |ui| {\n            let adjustment = adjustments.entry(output_name.clone()).or_default();\n            let accent = colorize.then(|| channel_color(output_name, index));\n            changed |= mixer_ui(ui, adjustment, output_name, channel_names, accent);",
    "    channel_names: &[String],\n    colorize: bool,\n    palette: Option<&ChannelPalette>,\n) -> bool {\n    let mut changed = false;\n    for (index, output_name) in channel_names.iter().enumerate() {\n        let display = channel_display_name(palette, output_name, index);\n        ui.collapsing(format!(\"Output — {display}\"), |ui| {\n            let adjustment = adjustments.entry(output_name.clone()).or_default();\n            let accent = colorize.then(|| channel_color(palette, output_name, index));\n            changed |= mixer_ui(ui, adjustment, output_name, channel_names, accent, palette);",
    "all mixers palette",
)

# Replace fallback channel_color with project-aware helper and add readonly swatch helper.
start = app.index("fn channel_color(name: &str, index: usize) -> egui::Color32 {")
end = app.index("\nfn clickable_row(", start)
new_helpers = '''fn channel_display_name<'a>(
    palette: Option<&'a ChannelPalette>,
    actual_name: &'a str,
    index: usize,
) -> &'a str {
    palette
        .map(|palette| palette.display_name(actual_name, index))
        .unwrap_or(actual_name)
}

fn channel_color(
    palette: Option<&ChannelPalette>,
    name: &str,
    index: usize,
) -> egui::Color32 {
    let [r, g, b] = palette
        .map(|palette| palette.color(name, index))
        .unwrap_or_else(|| palette::fallback_channel_color(name, index));
    egui::Color32::from_rgb(r, g, b)
}

fn palette_entry_readonly(ui: &mut egui::Ui, entry: &palette::ChannelPaletteEntry) {
    let [r, g, b] = entry.color;
    ui.horizontal(|ui| {
        ui.colored_label(egui::Color32::from_rgb(r, g, b), "■");
        ui.label(&entry.name);
    });
}
'''
app = app[:start] + new_helpers + app[end:]

app_path.write_text(app, encoding="utf-8")

# --- Cargo ---------------------------------------------------------------
cargo_path = ROOT / "Cargo.toml"
cargo = cargo_path.read_text(encoding="utf-8")
cargo = replace_once(cargo, 'version = "0.5.0"', 'version = "0.6.0"', "Cargo version")
cargo_path.write_text(cargo, encoding="utf-8")
