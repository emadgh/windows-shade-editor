from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if text.count(old) != 1:
        raise SystemExit(f"{label}: expected exactly one match, got {text.count(old)}")
    return text.replace(old, new, 1)

app_path = Path("src/app_main.rs")
app = app_path.read_text(encoding="utf-8")

app = replace_once(
    app,
    '#[path = "tiff_io.rs"]\nmod tiff_io;\n#[path = "update_v4.rs"]',
    '#[path = "tiff_io.rs"]\nmod tiff_io;\nmod validation;\n#[path = "update_v4.rs"]',
    "validation module",
)

anchor = '''    fn export_all_dialog(&mut self) {\n'''
method = '''    fn validate_current_face_dialog(&mut self) {\n        if self.job.is_some() {\n            return;\n        }\n        let Some(face) = self.faces.get(self.current_face) else {\n            return;\n        };\n        let mut dialog = rfd::FileDialog::new();\n        if let Some(parent) = face.path.parent() {\n            dialog = dialog.set_directory(parent);\n        }\n        let Some(folder) = dialog.pick_folder() else {\n            return;\n        };\n        let source = face.path.clone();\n        let default_dpi = self.settings.default_dpi;\n        self.launch_job("Validating TIFF", move |progress| {\n            let result = validation::validate_no_adjustment_roundtrip(\n                &source,\n                &folder,\n                default_dpi,\n                |fraction, detail| {\n                    Self::set_progress(\n                        &progress,\n                        Some(fraction),\n                        "Validating TIFF",\n                        detail,\n                    );\n                },\n            )\n            .map(|artifacts| {\n                let result = if artifacts.report.passed { "PASS" } else { "FAIL" };\n                format!(\n                    "TIFF round-trip {result} · report {}",\n                    artifacts.markdown_path.display()\n                )\n            });\n            JobResult::Export(SnapshotExportBatchResult {\n                result,\n                marks: Vec::new(),\n            })\n        });\n    }\n\n'''
app = replace_once(app, anchor, method + anchor, "validator method")

old_toolbar = '''                if ui\n                    .add_enabled(\n                        enabled && !self.faces.is_empty(),\n                        egui::Button::new("Export all"),\n                    )\n                    .clicked()\n                {\n                    self.export_all_dialog();\n                }\n                ui.separator();\n                if ui.button("Settings").clicked() {\n'''
new_toolbar = '''                if ui\n                    .add_enabled(\n                        enabled && !self.faces.is_empty(),\n                        egui::Button::new("Export all"),\n                    )\n                    .clicked()\n                {\n                    self.export_all_dialog();\n                }\n                if ui\n                    .add_enabled(\n                        enabled && !self.faces.is_empty(),\n                        egui::Button::new("Validate face"),\n                    )\n                    .on_hover_text("Run a no-adjustment export through the production TIFF backend, re-decode it, and compare pixels plus critical Photoshop/TIFF metadata.")\n                    .clicked()\n                {\n                    self.validate_current_face_dialog();\n                }\n                ui.separator();\n                if ui.button("Settings").clicked() {\n'''
app = replace_once(app, old_toolbar, new_toolbar, "toolbar validator button")
app_path.write_text(app, encoding="utf-8")

cargo = Path("Cargo.toml")
text = cargo.read_text(encoding="utf-8")
text = replace_once(text, 'version = "0.10.0"', 'version = "0.10.1"', "cargo version")
cargo.write_text(text, encoding="utf-8")

notes_path = Path("RELEASE_NOTES.md")
notes = notes_path.read_text(encoding="utf-8")
header = '''# Shade Editor 0.10.1\n\nProduction round-trip validator.\n\n- Adds **Validate face** beside Export actions. It creates a no-adjustment TIFF through the exact production export backend, re-decodes both source and export, and writes JSON + Markdown validation reports.\n- Validation checks decoded sample equality, dimensions, bit depth, color model/channel order, compression/predictor/orientation, physical DPI, ICC, Photoshop Image Resources 34377, ImageSourceData 37724, and parsed Photoshop Spot display metadata.\n- The validator uses a fresh identity project and disables Test Code so the result is a true transport/interchange check independent of the current shade recipe.\n- Adds regression coverage proving the validator exercises the real six-channel export backend.\n- `.shade` schema remains v9. Photoshop/RIP application-level interpretation remains an external production gate even when the automated report passes.\n\n'''
if not notes.startswith("# Shade Editor 0.10.1"):
    notes = header + notes
notes_path.write_text(notes, encoding="utf-8")

readme_path = Path("README.md")
readme = readme_path.read_text(encoding="utf-8")
needle = '- Atomic same-directory TIFF export replacement to avoid partial destination files.\n'
addition = needle + '- One-click current-Face production round-trip validation with pixel and critical TIFF/Photoshop metadata comparison reports.\n'
readme = replace_once(readme, needle, addition, "README validator feature")
readme_path.write_text(readme, encoding="utf-8")
