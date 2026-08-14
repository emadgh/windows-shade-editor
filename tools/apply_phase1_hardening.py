from pathlib import Path


def once(text, old, new, label):
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected 1, got {count}")
    return text.replace(old, new, 1)


def first(text, old, new, label):
    count = text.count(old)
    if count < 1:
        raise RuntimeError(f"{label}: expected at least 1, got 0")
    return text.replace(old, new, 1)


main = Path("src/main.rs")
text = main.read_text(encoding="utf-8")
text = once(text, "mod export_queue;\nmod history;", "mod export_queue;\nmod export_recipe;\nmod history;", "export recipe module")

text = once(
    text,
    "        let mut history = history::AdjustmentHistory::default();\n        history.reset(&project.adjustments, \"Start\");\n        Self {",
    "        let mut history = history::AdjustmentHistory::default();\n        history.reset(&project.adjustments, \"Start\");\n        let export_queue = export_queue::ExportQueue::load_persistent().unwrap_or_else(|err| {\n            log.error(&err);\n            export_queue::ExportQueue::new()\n        });\n        Self {",
    "load persistent queue",
)
text = once(text, "            export_queue: export_queue::ExportQueue::new(),", "            export_queue,", "queue init")

text = once(text, "            project: self.project.clone(),", "            recipe: export_recipe::ExportRecipe::from_project(&self.project),", "current export recipe")
text = once(text, "                project: project.clone(),", "                recipe: export_recipe::ExportRecipe::from_project(&project),", "export all recipe")
text = once(text, "            project,\n            default_dpi:", "            recipe: export_recipe::ExportRecipe::from_project(&project),\n            default_dpi:", "single snapshot recipe")
text = once(text, "                project,\n                default_dpi:", "                recipe: export_recipe::ExportRecipe::from_project(&project),\n                default_dpi:", "snapshot group recipe")

text = once(
    text,
    "        let snapshot_code = self.project.effective_test_code_text();\n        let template = self.settings.export_all_template.clone();",
    "        let test_code = self.project.effective_test_code_text();\n        let snapshot_name = self.project.active_snapshot_name().unwrap_or(\"Working\").to_owned();\n        let template = self.settings.export_all_template.clone();",
    "export all token values",
)
text = first(text, "                snapshot_code: &snapshot_code,", "                snapshot_name: &snapshot_name,\n                test_code: &test_code,", "export all context")
text = once(text, "                label: format!(\"{face_name} / {snapshot_code}\"),", "                label: format!(\"{face_name} / {snapshot_name}\"),", "export all label")

text = once(
    text,
    "        let snapshot_code = self.project.effective_test_code_text();\n        let first_source = self",
    "        let test_code = self.project.effective_test_code_text();\n        let snapshot_name = self.project.active_snapshot_name().unwrap_or(\"Working\").to_owned();\n        let first_source = self",
    "export preview token values",
)
text = first(text, "            snapshot_code: &snapshot_code,", "            snapshot_name: &snapshot_name,\n            test_code: &test_code,", "export preview context")
text = text.replace(
    'ui.small("Tokens: {project}, {face}, {snapshot}, {source}, {date}. Legacy tokens remain supported.");',
    'ui.small("Tokens: {project}, {face}, {snapshot}, {testcode}, {source}, {date}. Legacy {snapshot-code} remains Test Code compatible.");',
    1,
)

text = once(text, "            let snapshot_code = project.effective_test_code_text();", "            let test_code = project.effective_test_code_text();", "group test code")
text = first(text, "                snapshot_code: &snapshot_code,", "                snapshot_name: &snapshot.name,\n                test_code: &test_code,", "group context")

text = once(
    text,
    "    fn poll_export_queue(&mut self) {\n        let completions = self.export_queue.poll();",
    "    fn poll_export_queue(&mut self) {\n        let completions = self.export_queue.poll();\n        if let Some(err) = self.export_queue.take_persistence_error() {\n            self.log.error(&format!(\"Export Queue persistence: {err}\"));\n        }",
    "queue persistence logging",
)

text = text.replace(
    'if ui.small_button("Cancel").clicked() {\n                                                        cancel_id = Some(*id);\n                                                    }',
    'let button = if *status == export_queue::ExportQueueStatus::Processing {\n                                                        "Stop after current"\n                                                    } else {\n                                                        "Cancel"\n                                                    };\n                                                    if ui.small_button(button).clicked() {\n                                                        cancel_id = Some(*id);\n                                                    }',
    1,
)
text = text.replace(
    'ui.small("Processing items finish their current atomic TIFF write safely. Cancel on a Processing item stops the queue after that file is safely committed.");',
    'ui.small("Waiting items can be cancelled immediately. Processing items use Stop after current: the current atomic TIFF finishes safely, then remaining waiting items are cancelled.");',
    1,
)

main.write_text(text, encoding="utf-8")

export = Path("src/export.rs")
e = export.read_text(encoding="utf-8")
e = e.replace(
    "ColorModel::Rgb | ColorModel::Cmyk\n    )",
    "ColorModel::Rgb | ColorModel::Cmyk | ColorModel::Gray\n    )",
    1,
)
e = e.replace(
    '"Export currently supports RGB and CMYK Photoshop TIFF; this file is {}.",',
    '"Export currently supports RGB, CMYK and Gray TIFF; this file is {}.",',
    1,
)
e = e.replace(
    "ColorModel::Rgb | ColorModel::Cmyk) {",
    "ColorModel::Rgb | ColorModel::Cmyk | ColorModel::Gray) {",
    1,
)
e = e.replace(
    '"Export currently supports RGB and CMYK Photoshop TIFF; this file is {}.",',
    '"Export currently supports RGB, CMYK and Gray TIFF; this file is {}.",',
    1,
)
needle = '''        (ColorModel::Cmyk, 16, OutputPixels::U16(data)) => {
            let mut image = encoder
                .new_image::<colortype::CMYK16>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create CMYK 16-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 4, metadata, dpi_info)?;
            if let Some(rows) = rows_per_strip {
                image
                    .rows_per_strip(rows)
                    .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            }
            image
                .write_data(data)
                .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
'''
insert = needle + '''        (ColorModel::Gray, 8, OutputPixels::U8(data)) => {
            let mut image = encoder
                .new_image::<colortype::Gray8>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create Gray 8-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 1, metadata, dpi_info)?;
            if let Some(rows) = rows_per_strip {
                image
                    .rows_per_strip(rows)
                    .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            }
            image
                .write_data(data)
                .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (ColorModel::Gray, 16, OutputPixels::U16(data)) => {
            let mut image = encoder
                .new_image::<colortype::Gray16>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create Gray 16-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 1, metadata, dpi_info)?;
            if let Some(rows) = rows_per_strip {
                image
                    .rows_per_strip(rows)
                    .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            }
            image
                .write_data(data)
                .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
'''
if e.count(needle) != 1:
    raise RuntimeError(f"Gray encoder insertion: expected 1, got {e.count(needle)}")
e = e.replace(needle, insert, 1)

test_marker = "    #[test]\n    fn spot_zero_working_coverage_exports_as_no_ink_with_photoshop_polarity() {"
gray_test = '''    #[test]
    fn gray_adjustment_pipeline_preserves_single_channel_semantics() {
        let names = vec!["Gray".to_owned()];
        let mut metadata = test_metadata(&names, 1, vec![None]);
        metadata.color_model = ColorModel::Gray;
        let mut project = ShadeProject::default();
        project.ensure_channels(&names);
        project.adjustments.get_mut("Gray").unwrap().levels.output_white = 0.5;
        let input = [32_768u16];
        let output = adjusted_strip(&input, &metadata, &project);
        assert_eq!(output.len(), 1);
        assert!(output[0] < input[0]);
    }

'''
if e.count(test_marker) != 1:
    raise RuntimeError("Gray test insertion marker missing")
e = e.replace(test_marker, gray_test + test_marker, 1)
export.write_text(e, encoding="utf-8")
