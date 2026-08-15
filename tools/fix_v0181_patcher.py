from pathlib import Path

path = Path(__file__).with_name("apply_v0181_feedback.py")
text = path.read_text(encoding="utf-8")

replacements = [
    (
        '''module_at = export_rs.find("#[cfg(test)]\\nmod tests {")
if module_at < 0:
    raise RuntimeError("export tests module not found")
''',
        '''module_at = export_rs.find("#[cfg(test)]\\nmod streaming_tests {")
if module_at < 0:
    raise RuntimeError("export streaming_tests module not found")
''',
        "test module target",
    ),
    (
        '''main_rs = replace_inside_function(
    main_rs,
    "    fn export_snapshot_dialog(&mut self, snapshot_id: u64)",
    "self.settings.export_all_template",
    "self.settings.snapshot_export_template",
    "single snapshot template",
)
main_rs = replace_inside_function(
    main_rs,
    "    fn export_snapshot_group_dialog(&mut self, snapshot_ids: Vec<u64>, label: String)",
    "self.settings.export_all_template",
    "self.settings.snapshot_export_template",
    "snapshot group template",
)
''',
        '''main_rs = replace_once(
    main_rs,
    ''' + 'r\'\'\'' + '''        let suggested = format!(
            "{}-{}.tif",
            sanitize_filename(&stem),
            sanitize_filename(&snapshot.name)
        );
''' + '\'\'\'' + ''',
    ''' + 'r\'\'\'' + '''        let today = Local::now().format("%Y-%m-%d").to_string();
        let test_code = self.project.effective_test_code_text();
        let context = export_batch::ExportNameContext {
            shade_name: None,
            project_name: &self.project.name,
            snapshot_name: &snapshot.name,
            test_code: &test_code,
            face_number: self.current_face + 1,
            face_name: &stem,
            source_name: &stem,
            date: &today,
        };
        let suggested = export_batch::render_export_filename(
            &self.settings.snapshot_export_template,
            &context,
        );
''' + '\'\'\'' + ''',
    "single snapshot template suggestion",
)
main_rs = replace_once(
    main_rs,
    "export_batch::render_export_filename(&self.settings.export_all_template, &context)",
    "export_batch::render_export_filename(&self.settings.snapshot_export_template, &context)",
    "snapshot group template",
)
''',
        "snapshot template implementation",
    ),
]

for old, new, label in replacements:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one patch target, found {count}")
    text = text.replace(old, new, 1)

path.write_text(text, encoding="utf-8", newline="\n")
print("Aligned v0.18.1 patcher with current source layout")
