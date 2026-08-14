from pathlib import Path

path = Path('src/app_main.rs')
text = path.read_text(encoding='utf-8')

old = '''            let channel_button_label = if output_modified {
                format!("{output_display}  •")
            } else {
                output_display.clone()
            };'''
new = '''            let channel_button_label = if output_modified {
                format!("{output_display}  •")
            } else {
                output_display.to_owned()
            };'''
if old not in text:
    raise RuntimeError('modified channel label marker not found')
text = text.replace(old, new, 1)

old = '''        let mut reveal = false;
        let mut start = false;
        let mut changed = false;'''
new = '''        let mut reveal = false;
        let mut start = false;
        let mut cancel = false;
        let mut changed = false;'''
if old not in text:
    raise RuntimeError('export dialog state marker not found')
text = text.replace(old, new, 1)

old = '''                    if ui.button("Cancel").clicked() {
                        open = false;
                    }'''
new = '''                    cancel = ui.button("Cancel").clicked();'''
if old not in text:
    raise RuntimeError('cancel button marker not found')
text = text.replace(old, new, 1)

old = '''            });
        self.show_export_all = open;
        if changed {'''
new = '''            });
        if cancel {
            open = false;
        }
        self.show_export_all = open;
        if changed {'''
if old not in text:
    raise RuntimeError('export dialog close marker not found')
text = text.replace(old, new, 1)

path.write_text(text, encoding='utf-8')

path = Path('src/export_batch.rs')
text = path.read_text(encoding='utf-8')
start = text.find('pub fn sanitize_filename_stem(value: &str) -> String {')
end = text.find('pub enum DestinationDecision {', start)
if start < 0 or end < 0:
    raise RuntimeError('filename sanitizer function boundaries not found')
replacement = r'''pub fn sanitize_filename_stem(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for ch in value.chars() {
        let invalid = ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*');
        output.push(if invalid { '-' } else { ch });
    }
    let trimmed = output.trim().trim_matches('.').trim().trim_matches('-').trim();
    if trimmed.is_empty() {
        "shade-export".to_owned()
    } else {
        trimmed.to_owned()
    }
}

'''
text = text[:start] + replacement + text[end:]
path.write_text(text, encoding='utf-8')
