from pathlib import Path

path = Path('scripts/apply_v0132_previous_shades_ui.py')
text = path.read_text(encoding='utf-8')
old = '''previous = replace_once(
    previous,
    '        assert_eq!(store.entries()[0].open_count, 2);',
    '        assert_eq!(store.entries()[0].open_count, 2);\\n        assert_eq!(store.entries()[0].display_name(), "example");',
    'history display name test',
)
'''
new = '''previous = replace_once(
    previous,
    '        assert_eq!(store.entries()[0].open_count, 2);',
    '        assert_eq!(store.entries()[0].open_count, 2);\\n        assert_eq!(store.entries()[0].display_name(), "Second");',
    'history display name test',
)
previous = replace_once(
    previous,
    '    #[test]\\n    fn sort_labels_are_stable() {',
    '    #[test]\\n    fn untitled_history_uses_shade_filename() {\\n        let entry = PreviousShadeEntry {\\n            path: "C:/work/blue-17.shade".to_owned(),\\n            project_name: "Untitled Shade".to_owned(),\\n            ..PreviousShadeEntry::default()\\n        };\\n        assert_eq!(entry.display_name(), "blue-17");\\n    }\\n\\n    #[test]\\n    fn sort_labels_are_stable() {',
    'untitled filename fallback test',
)
'''
if text.count(old) != 1:
    raise RuntimeError(f'expected one test patch block, found {text.count(old)}')
path.write_text(text.replace(old, new, 1), encoding='utf-8')
print('v0.13.2 regression test patch fixed')
