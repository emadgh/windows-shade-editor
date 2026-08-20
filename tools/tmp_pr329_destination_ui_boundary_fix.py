from pathlib import Path

path = Path('work/src/ui/color_conversion.rs')
text = path.read_text(encoding='utf-8')
old = '''        let production_candidates = self
            .project_path
            .as_deref()
            .map(|source_project_path| {
                inspect_linked_production_destinations(&self.project, source_project_path)
            })
            .unwrap_or_default();'''
new = '''        let production_candidates = self
            .project_path
            .as_deref()
            .and_then(|source_project_path| {
                serde_json::to_value(&self.project)
                    .ok()
                    .and_then(|value| {
                        serde_json::from_value::<windows_shade_editor::model::ShadeProject>(value)
                            .ok()
                    })
                    .map(|source_project| {
                        inspect_linked_production_destinations(
                            &source_project,
                            source_project_path,
                        )
                    })
            })
            .unwrap_or_default();'''
count = text.count(old)
if count != 1:
    raise SystemExit(f'candidate discovery boundary: expected exactly one target, found {count}')
path.write_text(text.replace(old, new), encoding='utf-8', newline='\n')
