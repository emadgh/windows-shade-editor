from pathlib import Path
import re

ROOT = Path('.')


def read(path):
    return (ROOT / path).read_text(encoding='utf-8')


def write(path, text):
    (ROOT / path).write_text(text, encoding='utf-8')


def replace_once(text, old, new, label):
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f'{label}: expected 1 match, found {count}')
    return text.replace(old, new, 1)


def replace_between(text, start, end, replacement, label):
    a = text.find(start)
    if a < 0:
        raise RuntimeError(f'{label}: start not found')
    b = text.find(end, a + len(start))
    if b < 0:
        raise RuntimeError(f'{label}: end not found')
    return text[:a] + replacement + text[b:]


# ---------------- export_batch.rs ----------------
write('src/export_batch.rs', r'''use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const DEFAULT_EXPORT_TEMPLATE: &str =
    "{shade-name|project-name} - ({snapshot-code}) - {face-number}";

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConflictPolicy {
    Overwrite,
    Skip,
    #[default]
    AutoNumber,
}

impl ConflictPolicy {
    pub fn label(self) -> &'static str {
        match self {
            Self::Overwrite => "Overwrite",
            Self::Skip => "Skip",
            Self::AutoNumber => "Auto-number",
        }
    }
}

pub struct ExportNameContext<'a> {
    pub shade_name: Option<&'a str>,
    pub project_name: &'a str,
    pub snapshot_code: &'a str,
    pub face_number: usize,
    pub face_name: &'a str,
}

pub fn render_export_filename(template: &str, context: &ExportNameContext<'_>) -> String {
    let project_name = nonempty(context.project_name).unwrap_or("Shade");
    let shade_name = context.shade_name.and_then(nonempty).unwrap_or(project_name);
    let mut value = if template.trim().is_empty() {
        DEFAULT_EXPORT_TEMPLATE.to_owned()
    } else {
        template.to_owned()
    };
    value = value.replace("{shade-name|project-name}", shade_name);
    value = value.replace("{shade-name}", shade_name);
    value = value.replace("{project-name}", project_name);
    value = value.replace("{snapshot-code}", context.snapshot_code.trim());
    value = value.replace("{face-number}", &context.face_number.to_string());
    value = value.replace("{face-name}", context.face_name.trim());
    let stem = sanitize_filename_stem(&value);
    format!("{stem}.tif")
}

pub fn sanitize_filename_stem(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut previous_separator = false;
    for ch in value.chars() {
        let invalid = ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*');
        let mapped = if invalid { '-' } else { ch };
        let separator = mapped == '-' || mapped.is_whitespace();
        if separator && previous_separator {
            continue;
        }
        output.push(mapped);
        previous_separator = separator;
    }
    let trimmed = output.trim().trim_matches('.').trim().trim_matches('-').trim();
    if trimmed.is_empty() {
        "shade-export".to_owned()
    } else {
        trimmed.to_owned()
    }
}

pub enum DestinationDecision {
    Write(PathBuf),
    Skip(PathBuf),
}

pub fn resolve_destination(folder: &Path, filename: &str, policy: ConflictPolicy) -> DestinationDecision {
    let target = folder.join(filename);
    if !target.exists() || policy == ConflictPolicy::Overwrite {
        return DestinationDecision::Write(target);
    }
    if policy == ConflictPolicy::Skip {
        return DestinationDecision::Skip(target);
    }

    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "shade-export".to_owned());
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "tif".to_owned());
    for number in 2u64.. {
        let candidate = folder.join(format!("{stem} ({number}).{extension}"));
        if !candidate.exists() {
            return DestinationDecision::Write(candidate);
        }
    }
    unreachable!()
}

pub fn folder_tiff_count(folder: &Path) -> usize {
    let Ok(entries) = fs::read_dir(folder) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|kind| kind.is_file()).unwrap_or(false))
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("tif") || ext.eq_ignore_ascii_case("tiff"))
        })
        .count()
}

fn nonempty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_default_template_uses_shade_name_and_snapshot_code() {
        let name = render_export_filename(
            DEFAULT_EXPORT_TEMPLATE,
            &ExportNameContext {
                shade_name: Some("Tile Blue"),
                project_name: "Untitled Shade",
                snapshot_code: "T-42",
                face_number: 3,
                face_name: "face-a",
            },
        );
        assert_eq!(name, "Tile Blue - (T-42) - 3.tif");
    }

    #[test]
    fn project_name_is_fallback_when_shade_name_is_missing() {
        let name = render_export_filename(
            DEFAULT_EXPORT_TEMPLATE,
            &ExportNameContext {
                shade_name: None,
                project_name: "Project A",
                snapshot_code: "S1",
                face_number: 1,
                face_name: "face-a",
            },
        );
        assert!(name.starts_with("Project A"));
    }

    #[test]
    fn windows_reserved_filename_characters_are_sanitized() {
        assert_eq!(sanitize_filename_stem("A*B:C?D"), "A-B-C-D");
    }
}
''')

# ---------------- settings_v6.rs ----------------
path = 'src/settings_v6.rs'
text = read(path)
text = replace_once(
    text,
    'use serde::{Deserialize, Serialize};\n\nuse crate::palette::{',
    'use serde::{Deserialize, Serialize};\n\nuse crate::export_batch::{ConflictPolicy, DEFAULT_EXPORT_TEMPLATE};\nuse crate::palette::{',
    'settings import',
)
text = replace_once(
    text,
    '    pub export_all_test_code: bool,\n    pub lzw_compression: bool,',
    '    pub export_all_test_code: bool,\n    pub export_all_template: String,\n    pub export_all_conflict_policy: ConflictPolicy,\n    pub export_all_open_folder: bool,\n    pub lzw_compression: bool,',
    'settings export fields',
)
text = replace_once(
    text,
    '            export_all_test_code: false,\n            lzw_compression: true,',
    '            export_all_test_code: false,\n            export_all_template: DEFAULT_EXPORT_TEMPLATE.to_owned(),\n            export_all_conflict_policy: ConflictPolicy::AutoNumber,\n            export_all_open_folder: false,\n            lzw_compression: true,',
    'settings export defaults',
)
text = replace_once(
    text,
    '        self.default_dpi = self.default_dpi.clamp(36.0, 2400.0);\n\n        let mut used_ids',
    '        self.default_dpi = self.default_dpi.clamp(36.0, 2400.0);\n        if self.export_all_template.trim().is_empty() {\n            self.export_all_template = DEFAULT_EXPORT_TEMPLATE.to_owned();\n        }\n\n        let mut used_ids',
    'settings sanitize template',
)
text = replace_once(
    text,
    '    fn export_all_test_code_defaults_off() {\n        assert!(!AppSettings::default().export_all_test_code);\n    }',
    '    fn export_all_test_code_defaults_off() {\n        assert!(!AppSettings::default().export_all_test_code);\n    }\n\n    #[test]\n    fn export_all_defaults_are_safe() {\n        let settings = AppSettings::default();\n        assert_eq!(settings.export_all_template, DEFAULT_EXPORT_TEMPLATE);\n        assert_eq!(settings.export_all_conflict_policy, ConflictPolicy::AutoNumber);\n        assert!(!settings.export_all_open_folder);\n    }',
    'settings tests',
)
write(path, text)

# ---------------- model_v6.rs ----------------
path = 'src/model_v6.rs'
text = read(path)
text = replace_once(
    text,
    'pub struct ProjectThumbnail {\n    pub mime_type: String,\n    pub width: u32,\n    pub height: u32,\n    pub data_base64: String,\n}',
    'pub struct ProjectThumbnail {\n    pub mime_type: String,\n    #[serde(default = "default_thumbnail_version")]\n    pub thumbnail_version: u32,\n    pub width: u32,\n    pub height: u32,\n    #[serde(default)]\n    pub encoded_bytes: u64,\n    pub data_base64: String,\n}\n\nfn default_thumbnail_version() -> u32 {\n    1\n}',
    'thumbnail metadata schema',
)
write(path, text)

# ---------------- thumbnail.rs ----------------
path = 'src/thumbnail.rs'
text = read(path)
text = replace_once(
    text,
    '    Ok(ProjectThumbnail {\n        mime_type: "image/png".to_owned(),\n        width: width as u32,\n        height: height as u32,\n        data_base64: BASE64_STANDARD.encode(png),\n    })',
    '    let encoded_bytes = png.len() as u64;\n    Ok(ProjectThumbnail {\n        mime_type: "image/png".to_owned(),\n        thumbnail_version: 1,\n        width: width as u32,\n        height: height as u32,\n        encoded_bytes,\n        data_base64: BASE64_STANDARD.encode(png),\n    })',
    'project thumbnail metadata',
)
write(path, text)

# ---------------- previous_shades.rs ----------------
path = 'src/previous_shades.rs'
text = read(path)
text = replace_once(text, 'const SNAPSHOT_CACHE_VERSION: u32 = 2;', 'const SNAPSHOT_CACHE_VERSION: u32 = 3;', 'history cache version')
text = replace_once(
    text,
    '    pub face_count: usize,\n    pub total_source_bytes: u64,\n    pub thumbnail: Option<ProjectThumbnail>,',
    '    pub face_count: usize,\n    pub active_face_index: usize,\n    pub active_face_label: String,\n    pub total_source_bytes: u64,\n    pub thumbnail: Option<ProjectThumbnail>,',
    'history active face fields',
)
text = replace_once(
    text,
    '            face_count: 0,\n            total_source_bytes: 0,\n            thumbnail: None,',
    '            face_count: 0,\n            active_face_index: 0,\n            active_face_label: String::new(),\n            total_source_bytes: 0,\n            thumbnail: None,',
    'history active face defaults',
)
text = replace_once(
    text,
    '        self.total_source_bytes = project\n            .file_metadata\n            .as_ref()\n            .map(|metadata| metadata.total_source_bytes)\n            .unwrap_or(0);',
    '        self.active_face_index = project\n            .file_metadata\n            .as_ref()\n            .map(|metadata| metadata.active_face_index)\n            .unwrap_or(0);\n        self.active_face_label = project\n            .file_metadata\n            .as_ref()\n            .and_then(|metadata| {\n                metadata\n                    .faces\n                    .get(self.active_face_index)\n                    .or_else(|| metadata.faces.first())\n            })\n            .map(|face| {\n                if face.label.trim().is_empty() {\n                    face.source_file_name.trim().to_owned()\n                } else {\n                    face.label.trim().to_owned()\n                }\n            })\n            .filter(|value| !value.is_empty())\n            .or_else(|| {\n                project\n                    .faces\n                    .get(self.active_face_index)\n                    .or_else(|| project.faces.first())\n                    .map(|face| face.label.trim().to_owned())\n                    .filter(|value| !value.is_empty())\n            })\n            .unwrap_or_default();\n        self.total_source_bytes = project\n            .file_metadata\n            .as_ref()\n            .map(|metadata| metadata.total_source_bytes)\n            .unwrap_or(0);',
    'refresh active face cache',
)
text = replace_once(
    text,
    '    pub fn test_code_matches(&self, query_lower: &str) -> bool {\n        !self.test_code_text.trim().is_empty()\n            && contains_case_insensitive(&self.test_code_text, query_lower.trim())\n    }\n}',
    '    pub fn test_code_matches(&self, query_lower: &str) -> bool {\n        !self.test_code_text.trim().is_empty()\n            && contains_case_insensitive(&self.test_code_text, query_lower.trim())\n    }\n\n    pub fn latest_snapshot(&self) -> Option<&CachedSnapshot> {\n        self.snapshots\n            .iter()\n            .max_by_key(|snapshot| (snapshot.created_at_unix_ms, snapshot.id))\n    }\n\n    pub fn active_face_display(&self) -> String {\n        if !self.active_face_label.trim().is_empty() {\n            return format!("Face {} · {}", self.active_face_index.saturating_add(1), self.active_face_label.trim());\n        }\n        if self.face_count > 0 {\n            format!("Face {}", self.active_face_index.saturating_add(1).min(self.face_count))\n        } else {\n            "No face metadata".to_owned()\n        }\n    }\n\n    pub fn is_missing(&self) -> bool {\n        !Path::new(&self.path).is_file()\n    }\n}',
    'history entry helpers',
)
text = replace_once(
    text,
    '    pub fn entries(&self) -> &[PreviousShadeEntry] {\n        &self.entries\n    }',
    '    pub fn entries(&self) -> &[PreviousShadeEntry] {\n        &self.entries\n    }\n\n    pub fn remove_path(&mut self, path: &str) -> bool {\n        let before = self.entries.len();\n        self.entries.retain(|entry| !same_path(&entry.path, path));\n        self.entries.len() != before\n    }\n\n    pub fn relink_path(&mut self, old_path: &str, new_path: &Path) -> Result<String, String> {\n        let project = ShadeProject::load(new_path)?;\n        let normalized = fs::canonicalize(new_path).unwrap_or_else(|_| new_path.to_path_buf());\n        let display = normalized.to_string_lossy().into_owned();\n        let Some(index) = self.entries.iter().position(|entry| same_path(&entry.path, old_path)) else {\n            return Err("Previous Shades entry no longer exists.".to_owned());\n        };\n        let mut entry = self.entries.remove(index);\n        entry.path = display.clone();\n        entry.project_name = project_display_name(&project.name, new_path);\n        entry.last_opened_unix_ms = unix_ms_now();\n        entry.saved_at_unix_ms = project\n            .file_metadata\n            .as_ref()\n            .map(|metadata| metadata.saved_at_unix_ms)\n            .unwrap_or(entry.saved_at_unix_ms);\n        entry.refresh_from_project(&project);\n        if let Some(existing) = self.entries.iter_mut().find(|existing| same_path(&existing.path, &display)) {\n            existing.open_count = existing.open_count.max(entry.open_count).max(1);\n            existing.last_opened_unix_ms = existing.last_opened_unix_ms.max(entry.last_opened_unix_ms);\n            existing.saved_at_unix_ms = entry.saved_at_unix_ms;\n            existing.project_name = entry.project_name;\n            existing.snapshot_cache_version = entry.snapshot_cache_version;\n            existing.snapshots = entry.snapshots;\n            existing.test_code_text = entry.test_code_text;\n            existing.face_count = entry.face_count;\n            existing.active_face_index = entry.active_face_index;\n            existing.active_face_label = entry.active_face_label;\n            existing.total_source_bytes = entry.total_source_bytes;\n            existing.thumbnail = entry.thumbnail;\n        } else {\n            self.entries.push(entry);\n        }\n        Ok(display)\n    }',
    'history mutation methods',
)
# merge cached active face fields in both sanitize branches
text = text.replace(
    '                    existing.face_count = entry.face_count;\n                    existing.total_source_bytes = entry.total_source_bytes;',
    '                    existing.face_count = entry.face_count;\n                    existing.active_face_index = entry.active_face_index;\n                    existing.active_face_label = entry.active_face_label.clone();\n                    existing.total_source_bytes = entry.total_source_bytes;',
)
text = replace_once(
    text,
    '    Some(ProjectThumbnail {\n        mime_type: "image/png".to_owned(),\n        width: width as u32,\n        height: height as u32,\n        data_base64: BASE64_STANDARD.encode(png),\n    })',
    '    let encoded_bytes = png.len() as u64;\n    Some(ProjectThumbnail {\n        mime_type: "image/png".to_owned(),\n        thumbnail_version: 1,\n        width: width as u32,\n        height: height as u32,\n        encoded_bytes,\n        data_base64: BASE64_STANDARD.encode(png),\n    })',
    'cached thumbnail metadata',
)
write(path, text)

# ---------------- app_main.rs ----------------
path = 'src/app_main.rs'
text = read(path)
text = replace_once(text, '#[path = "export_v6.rs"]\nmod export;', '#[path = "export_v6.rs"]\nmod export;\n#[path = "export_batch.rs"]\nmod export_batch;', 'export batch module')
text = replace_once(text, 'use std::collections::BTreeMap;', 'use std::collections::{BTreeMap, VecDeque};', 'VecDeque import')
text = replace_once(
    text,
    'const HISTORY_COMMIT_DELAY: Duration = Duration::from_millis(300);',
    'const HISTORY_COMMIT_DELAY: Duration = Duration::from_millis(300);\nconst PREVIOUS_SHADE_TEXTURE_CACHE_LIMIT: usize = 64;',
    'LRU constant',
)
text = replace_once(
    text,
    '            if let Some(path) = startup_project.clone() {\n                app.open_project_path(path);\n            }\n            Ok(Box::new(app))',
    '            if let Some(path) = startup_project.clone() {\n                app.show_previous_shades = false;\n                app.open_project_path(path);\n            } else {\n                app.show_previous_shades = true;\n            }\n            Ok(Box::new(app))',
    'startup project browser',
)
text = replace_once(
    text,
    '    previous_shade_texture: Option<egui::TextureHandle>,\n    previous_shade_list_textures: BTreeMap<String, egui::TextureHandle>,\n    remind_after_export: bool,',
    '    previous_shade_texture: Option<egui::TextureHandle>,\n    previous_shade_list_textures: BTreeMap<String, egui::TextureHandle>,\n    previous_shade_list_texture_lru: VecDeque<String>,\n    show_export_all: bool,\n    export_all_folder: String,\n    remind_after_export: bool,',
    'app state fields',
)
text = replace_once(
    text,
    '            previous_shade_texture: None,\n            previous_shade_list_textures: BTreeMap::new(),\n            remind_after_export: false,',
    '            previous_shade_texture: None,\n            previous_shade_list_textures: BTreeMap::new(),\n            previous_shade_list_texture_lru: VecDeque::new(),\n            show_export_all: false,\n            export_all_folder: String::new(),\n            remind_after_export: false,',
    'app state defaults',
)

export_all_impl = r'''    fn export_all_dialog(&mut self) {
        if self.job.is_some() || self.faces.is_empty() {
            return;
        }
        if self.faces.iter().any(|face| !face.available) {
            self.report_error("Export all requires every Face source TIFF to be available. Relink missing Faces first.");
            return;
        }
        if self.export_all_folder.trim().is_empty() {
            let initial = self
                .project_path
                .as_ref()
                .and_then(|path| path.parent())
                .or_else(|| self.faces.get(self.current_face).and_then(|face| face.path.parent()))
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.export_all_folder = initial;
        }
        self.show_export_all = true;
    }

    fn start_export_all(&mut self) {
        if self.job.is_some() || self.faces.is_empty() {
            return;
        }
        let folder = PathBuf::from(self.export_all_folder.trim());
        if self.export_all_folder.trim().is_empty() {
            self.report_error("Choose an Export All folder first.");
            return;
        }
        if let Err(err) = std::fs::create_dir_all(&folder) {
            self.report_error(format!("Cannot create Export All folder {}: {err}", folder.display()));
            return;
        }
        if self.faces.iter().any(|face| !face.available) {
            self.report_error("Export all requires every Face source TIFF to be available. Relink missing Faces first.");
            return;
        }

        let sources = self.faces.iter().map(|face| face.path.clone()).collect::<Vec<_>>();
        let face_names = self
            .project
            .faces
            .iter()
            .map(|face| face.label.clone())
            .collect::<Vec<_>>();
        let shade_name = self
            .project_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|value| value.to_string_lossy().into_owned());
        let project_name = self.project.name.clone();
        let snapshot_code = self.project.effective_test_code_text();
        let template = self.settings.export_all_template.clone();
        let conflict_policy = self.settings.export_all_conflict_policy;
        let open_after = self.settings.export_all_open_folder;
        let mut project = self.project.clone();
        project.test_code.enabled = self.settings.export_all_test_code;
        let default_dpi = self.settings.default_dpi;
        let force_lzw = self.settings.lzw_compression;
        let validate_after_export = self.settings.validate_after_export;
        self.remind_after_export = self.snapshot_project_needs_save_reminder();
        self.show_export_all = false;
        let _ = self.settings.save();

        self.launch_job("Exporting faces", move |progress| {
            let total = sources.len().max(1);
            let result = (|| -> Result<String, String> {
                let mut written = 0usize;
                let mut skipped = 0usize;
                for (index, source) in sources.iter().enumerate() {
                    let face_name = face_names
                        .get(index)
                        .map(String::as_str)
                        .filter(|name| !name.trim().is_empty())
                        .or_else(|| source.file_stem().and_then(|value| value.to_str()))
                        .unwrap_or("face");
                    let filename = export_batch::render_export_filename(
                        &template,
                        &export_batch::ExportNameContext {
                            shade_name: shade_name.as_deref(),
                            project_name: &project_name,
                            snapshot_code: &snapshot_code,
                            face_number: index + 1,
                            face_name,
                        },
                    );
                    let destination = match export_batch::resolve_destination(
                        &folder,
                        &filename,
                        conflict_policy,
                    ) {
                        export_batch::DestinationDecision::Write(path) => path,
                        export_batch::DestinationDecision::Skip(path) => {
                            skipped += 1;
                            Self::set_progress(
                                &progress,
                                Some((index + 1) as f32 / total as f32),
                                "Exporting faces",
                                &format!("Skipped existing {}", path.file_name().map(|value| value.to_string_lossy()).unwrap_or_default()),
                            );
                            continue;
                        }
                    };
                    export::export_face_with_progress_options(
                        source,
                        &destination,
                        &project,
                        default_dpi,
                        export::ExportOptions { force_lzw },
                        |phase, detail| {
                            let inner = if validate_after_export { phase * 0.88 } else { phase };
                            let overall = (index as f32 + inner) / total as f32;
                            Self::set_progress(&progress, Some(overall), "Exporting faces", detail);
                        },
                    )?;
                    if validate_after_export {
                        let overall = (index as f32 + 0.92) / total as f32;
                        Self::set_progress(
                            &progress,
                            Some(overall),
                            "Validating exported TIFF",
                            &destination.display().to_string(),
                        );
                        validation::validate_export_transport_with_options(
                            source,
                            &destination,
                            force_lzw,
                        )?;
                    }
                    written += 1;
                }
                Self::set_progress(&progress, Some(1.0), "Exporting faces", "Complete");
                if open_after {
                    let _ = open_folder(&folder);
                }
                Ok(if skipped > 0 {
                    format!("Exported {written} face(s) · skipped {skipped} existing file(s)")
                } else {
                    format!("Exported {written} face(s)")
                })
            })();
            JobResult::Export(SnapshotExportBatchResult {
                result,
                marks: Vec::new(),
            })
        });
    }

    fn ui_export_all_window(&mut self, ctx: &egui::Context) {
        if !self.show_export_all {
            return;
        }
        let mut open = self.show_export_all;
        let folder = PathBuf::from(self.export_all_folder.trim());
        let existing_tiffs = if folder.is_dir() {
            export_batch::folder_tiff_count(&folder)
        } else {
            0
        };
        let shade_name = self
            .project_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|value| value.to_string_lossy().into_owned());
        let snapshot_code = self.project.effective_test_code_text();
        let face_name = self
            .project
            .faces
            .first()
            .map(|face| face.label.as_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("face");
        let preview_name = export_batch::render_export_filename(
            &self.settings.export_all_template,
            &export_batch::ExportNameContext {
                shade_name: shade_name.as_deref(),
                project_name: &self.project.name,
                snapshot_code: &snapshot_code,
                face_number: 1,
                face_name,
            },
        );
        let mut browse = false;
        let mut reveal = false;
        let mut start = false;
        let mut changed = false;
        egui::Window::new("Export All Faces")
            .open(&mut open)
            .resizable(true)
            .default_width(620.0)
            .show(ctx, |ui| {
                ui.strong("Export folder");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.export_all_folder)
                            .desired_width(f32::INFINITY),
                    );
                    browse = ui.button("Browse...").clicked();
                });
                if existing_tiffs > 0 {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        format!("Warning: this folder already contains {existing_tiffs} TIFF file(s). Mixing source/old exports can cause mistakes."),
                    );
                    reveal = ui.button("Reveal folder").clicked();
                }

                ui.add_space(8.0);
                ui.strong("File name template");
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut self.settings.export_all_template)
                            .desired_width(f32::INFINITY),
                    )
                    .changed();
                ui.small("Tokens: {shade-name|project-name}, {shade-name}, {project-name}, {snapshot-code}, {face-number}, {face-name}");
                ui.small("Windows-reserved characters such as * are converted to '-' in the generated filename.");
                ui.horizontal_wrapped(|ui| {
                    ui.label("Preview:");
                    ui.monospace(&preview_name);
                });

                ui.add_space(8.0);
                ui.strong("If a file already exists");
                ui.horizontal_wrapped(|ui| {
                    changed |= ui
                        .radio_value(
                            &mut self.settings.export_all_conflict_policy,
                            export_batch::ConflictPolicy::Overwrite,
                            "Overwrite",
                        )
                        .changed();
                    changed |= ui
                        .radio_value(
                            &mut self.settings.export_all_conflict_policy,
                            export_batch::ConflictPolicy::Skip,
                            "Skip",
                        )
                        .changed();
                    changed |= ui
                        .radio_value(
                            &mut self.settings.export_all_conflict_policy,
                            export_batch::ConflictPolicy::AutoNumber,
                            "Auto-number",
                        )
                        .changed();
                });
                ui.small("Auto-number is the safe default and produces names such as '... (2).tif'.");

                ui.add_space(8.0);
                changed |= ui
                    .checkbox(
                        &mut self.settings.export_all_open_folder,
                        "Open folder after export",
                    )
                    .changed();
                changed |= ui
                    .checkbox(
                        &mut self.settings.export_all_test_code,
                        "Write Test Code on every exported Face",
                    )
                    .changed();

                ui.separator();
                ui.horizontal(|ui| {
                    start = ui
                        .add_enabled(
                            !self.export_all_folder.trim().is_empty()
                                && self.job.is_none()
                                && !self.faces.is_empty(),
                            egui::Button::new("Export All"),
                        )
                        .clicked();
                    if ui.button("Cancel").clicked() {
                        open = false;
                    }
                });
            });
        self.show_export_all = open;
        if changed {
            self.settings.sanitize();
            if let Err(err) = self.settings.save() {
                self.log.error(&err);
            }
        }
        if browse {
            let mut dialog = rfd::FileDialog::new();
            if folder.is_dir() {
                dialog = dialog.set_directory(&folder);
            }
            if let Some(selected) = dialog.pick_folder() {
                self.export_all_folder = selected.to_string_lossy().into_owned();
            }
        }
        if reveal && folder.is_dir() {
            if let Err(err) = open_folder(&folder) {
                self.report_error(err);
            }
        }
        if start {
            self.start_export_all();
        }
    }

'''
text = replace_between(
    text,
    '    fn export_all_dialog(&mut self) {',
    '    fn export_snapshot_dialog(&mut self, snapshot_id: u64) {',
    export_all_impl,
    'replace export all methods',
)

# Add LRU helper immediately before Project Browser window.
marker = '    fn ui_previous_shades_window(&mut self, ctx: &egui::Context) {'
lru_helper = r'''    fn ensure_previous_shade_list_texture(
        &mut self,
        ctx: &egui::Context,
        entry: &previous_shades::PreviousShadeEntry,
    ) {
        let key = entry.path.clone();
        if self.previous_shade_list_textures.contains_key(&key) {
            self.previous_shade_list_texture_lru.retain(|item| item != &key);
            self.previous_shade_list_texture_lru.push_back(key);
            return;
        }
        let Ok(Some(thumbnail)) = previous_shades::decode_cached_thumbnail(entry) else {
            return;
        };
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [thumbnail.width, thumbnail.height],
            &thumbnail.rgba,
        );
        let texture = ctx.load_texture(
            format!("previous-shade-list:{}", entry.path),
            image,
            egui::TextureOptions::LINEAR,
        );
        self.previous_shade_list_textures.insert(key.clone(), texture);
        self.previous_shade_list_texture_lru.retain(|item| item != &key);
        self.previous_shade_list_texture_lru.push_back(key);
        while self.previous_shade_list_texture_lru.len() > PREVIOUS_SHADE_TEXTURE_CACHE_LIMIT {
            if let Some(oldest) = self.previous_shade_list_texture_lru.pop_front() {
                self.previous_shade_list_textures.remove(&oldest);
            }
        }
    }

'''
text = replace_once(text, marker, lru_helper + marker, 'insert thumbnail LRU helper')

previous_ui = r'''    fn ui_previous_shades_window(&mut self, ctx: &egui::Context) {
        if !self.show_previous_shades {
            return;
        }
        let mut open = self.show_previous_shades;
        let query_before = self.previous_shades_query.clone();
        let mut requested_open: Option<String> = None;
        let mut requested_select: Option<String> = None;
        let mut requested_reveal: Option<String> = None;
        let mut requested_remove: Option<String> = None;
        let mut requested_relink: Option<String> = None;

        egui::Window::new("Previous Shades")
            .open(&mut open)
            .default_width(1040.0)
            .default_height(680.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Project Browser");
                    ui.separator();
                    ui.label(format!("{} project(s)", self.previous_shades.entries().len()));
                });
                ui.horizontal(|ui| {
                    ui.label("Search");
                    let search = ui.add(
                        egui::TextEdit::singleline(&mut self.previous_shades_query)
                            .hint_text("Project, path, Snapshot name / ID / Test Code")
                            .desired_width(390.0),
                    );
                    if !search.has_focus() && !ctx.wants_keyboard_input() {
                        let typed = ctx.input(|input| {
                            input
                                .events
                                .iter()
                                .filter_map(|event| match event {
                                    egui::Event::Text(text) if !text.chars().all(char::is_control) => {
                                        Some(text.as_str())
                                    }
                                    _ => None,
                                })
                                .collect::<String>()
                        });
                        if !typed.is_empty() {
                            self.previous_shades_query.push_str(&typed);
                            search.request_focus();
                        }
                    }
                    ui.label("Sort");
                    egui::ComboBox::from_id_salt("previous-shades-sort")
                        .selected_text(self.previous_shades_sort.label())
                        .show_ui(ui, |ui| {
                            for sort in [
                                previous_shades::PreviousShadesSort::LastOpened,
                                previous_shades::PreviousShadesSort::ProjectName,
                                previous_shades::PreviousShadesSort::SavedAt,
                                previous_shades::PreviousShadesSort::Path,
                            ] {
                                ui.selectable_value(&mut self.previous_shades_sort, sort, sort.label());
                            }
                        });
                });

                let query = self.previous_shades_query.trim().to_lowercase();
                let entries = self.previous_shades.entries();
                let mut indices = entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| entry.matches_query(&query))
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                match self.previous_shades_sort {
                    previous_shades::PreviousShadesSort::LastOpened => indices.sort_by(|a, b| {
                        entries[*b].last_opened_unix_ms.cmp(&entries[*a].last_opened_unix_ms)
                    }),
                    previous_shades::PreviousShadesSort::ProjectName => indices.sort_by(|a, b| {
                        entries[*a].display_name().to_lowercase().cmp(&entries[*b].display_name().to_lowercase())
                    }),
                    previous_shades::PreviousShadesSort::SavedAt => indices.sort_by(|a, b| {
                        entries[*b].saved_at_unix_ms.cmp(&entries[*a].saved_at_unix_ms)
                    }),
                    previous_shades::PreviousShadesSort::Path => indices.sort_by(|a, b| {
                        entries[*a].path.to_lowercase().cmp(&entries[*b].path.to_lowercase())
                    }),
                }
                let paths = indices
                    .iter()
                    .map(|index| entries[*index].path.clone())
                    .collect::<Vec<_>>();

                if query_before != self.previous_shades_query {
                    requested_select = paths.first().cloned();
                }
                let current_path = requested_select
                    .as_deref()
                    .or(self.previous_shades_selected.as_deref());
                let current_position = current_path.and_then(|path| paths.iter().position(|item| item == path));
                let (up, down, enter) = ctx.input(|input| {
                    (
                        input.key_pressed(egui::Key::ArrowUp),
                        input.key_pressed(egui::Key::ArrowDown),
                        input.key_pressed(egui::Key::Enter),
                    )
                });
                if !paths.is_empty() && (up || down) {
                    let next = match (current_position, up, down) {
                        (Some(position), true, _) => position.saturating_sub(1),
                        (Some(position), _, true) => (position + 1).min(paths.len() - 1),
                        (None, _, true) => 0,
                        (None, true, _) => paths.len() - 1,
                        _ => 0,
                    };
                    requested_select = paths.get(next).cloned();
                }
                if enter {
                    requested_open = requested_select
                        .clone()
                        .or_else(|| self.previous_shades_selected.clone())
                        .filter(|path| Path::new(path).is_file());
                }

                ui.separator();
                ui.columns(2, |columns| {
                    columns[0].set_min_width(520.0);
                    if paths.is_empty() {
                        columns[0].label("No matching .shade projects.");
                    } else {
                        egui::ScrollArea::vertical()
                            .id_salt("previous-shades-list")
                            .auto_shrink([false, false])
                            .show_rows(&mut columns[0], 72.0, indices.len(), |ui, range| {
                                for row in range {
                                    let entry = self.previous_shades.entries()[indices[row]].clone();
                                    self.ensure_previous_shade_list_texture(ctx, &entry);
                                    let label = entry.display_name();
                                    let source_bytes = if entry.total_source_bytes > 0 {
                                        format_byte_count(entry.total_source_bytes)
                                    } else {
                                        "-".to_owned()
                                    };
                                    let metadata = format!(
                                        "{} face(s) · {} · {}",
                                        entry.face_count,
                                        source_bytes,
                                        entry.active_face_display(),
                                    );
                                    let latest = entry
                                        .latest_snapshot()
                                        .map(|snapshot| {
                                            let code = snapshot.code.trim();
                                            if code.is_empty() || code == snapshot.name.trim() {
                                                format!("Latest: {}", snapshot.name)
                                            } else {
                                                format!("Latest: {} · {}", snapshot.name, code)
                                            }
                                        })
                                        .unwrap_or_else(|| "No Snapshots".to_owned());
                                    let detail = if entry.is_missing() {
                                        format!("{latest} · MISSING")
                                    } else {
                                        latest
                                    };
                                    let selected = requested_select
                                        .as_deref()
                                        .or(self.previous_shades_selected.as_deref())
                                        == Some(entry.path.as_str());
                                    let thumbnail = self.previous_shade_list_textures.get(&entry.path);
                                    let response = previous_shade_history_row(
                                        ui,
                                        selected,
                                        &label,
                                        &metadata,
                                        &detail,
                                        thumbnail,
                                    );
                                    if response.clicked() {
                                        requested_select = Some(entry.path.clone());
                                    }
                                    if response.double_clicked() && !entry.is_missing() {
                                        requested_open = Some(entry.path.clone());
                                    }
                                }
                            });
                    }

                    let selected_path = requested_select
                        .as_deref()
                        .or(self.previous_shades_selected.as_deref());
                    if let Some(path) = selected_path {
                        let cached = self
                            .previous_shades
                            .entries()
                            .iter()
                            .find(|entry| entry.path == path)
                            .cloned();
                        columns[1].horizontal_wrapped(|ui| {
                            let exists = Path::new(path).is_file();
                            if ui.add_enabled(exists, egui::Button::new("Open")).clicked() {
                                requested_open = Some(path.to_owned());
                            }
                            if ui.add_enabled(exists, egui::Button::new("Reveal in Explorer")).clicked() {
                                requested_reveal = Some(path.to_owned());
                            }
                            if !exists && ui.button("Relink missing...").clicked() {
                                requested_relink = Some(path.to_owned());
                            }
                            if ui.button("Remove from history").clicked() {
                                requested_remove = Some(path.to_owned());
                            }
                        });
                        columns[1].separator();
                        if let Some(texture) = self.previous_shade_texture.as_ref() {
                            let available = columns[1].available_width().min(420.0);
                            let size = texture.size_vec2();
                            let scale = (available / size.x.max(1.0)).min(1.0);
                            columns[1].image((texture.id(), size * scale));
                        }
                        if let Some(preview) = self.previous_shade_preview.as_ref() {
                            columns[1].heading(&preview.project_name);
                            egui::Grid::new("previous-shade-metadata")
                                .num_columns(2)
                                .striped(true)
                                .show(&mut columns[1], |ui| {
                                    ui.strong("Saved");
                                    ui.label(format_previous_shade_time(preview.saved_at_unix_ms));
                                    ui.end_row();
                                    ui.strong("Faces");
                                    ui.label(preview.face_count.to_string());
                                    ui.end_row();
                                    ui.strong("Active Face");
                                    ui.label(format!("{}", preview.active_face_index.saturating_add(1)));
                                    ui.end_row();
                                    ui.strong("Source bytes");
                                    ui.label(format_byte_count(preview.total_source_bytes));
                                    ui.end_row();
                                });
                            columns[1].separator();
                            columns[1].strong(format!("Snapshots · {}", preview.snapshots.len()));
                            egui::ScrollArea::vertical()
                                .id_salt("previous-shade-snapshots")
                                .max_height(220.0)
                                .show(&mut columns[1], |ui| {
                                    for snapshot in &preview.snapshots {
                                        let active = preview.active_snapshot_name.as_deref() == Some(snapshot.name.as_str());
                                        let suffix = if snapshot.code.trim().is_empty() || snapshot.code == snapshot.name {
                                            format!("#{}", snapshot.id)
                                        } else {
                                            format!("#{} · {}", snapshot.id, snapshot.code)
                                        };
                                        if active {
                                            ui.strong(format!("{} · {} · active", snapshot.name, suffix));
                                        } else {
                                            ui.label(format!("{} · {}", snapshot.name, suffix));
                                        }
                                    }
                                });
                        } else if let Some(error) = self.previous_shade_preview_error.as_ref() {
                            columns[1].colored_label(egui::Color32::YELLOW, error);
                            if let Some(entry) = cached.as_ref() {
                                columns[1].label(format!("Cached: {} face(s) · {}", entry.face_count, entry.active_face_display()));
                                if let Some(snapshot) = entry.latest_snapshot() {
                                    columns[1].label(format!("Latest Snapshot: {} · #{}", snapshot.name, snapshot.id));
                                }
                            }
                        } else {
                            columns[1].label("Select a project to inspect its thumbnail, Snapshots and metadata.");
                        }
                    } else {
                        columns[1].label("Select a project to inspect its thumbnail, Snapshots and metadata.");
                    }
                });
            });

        self.show_previous_shades = open;
        if let Some(path) = requested_select {
            if self.previous_shades_selected.as_deref() != Some(path.as_str()) {
                self.load_previous_shade_preview(ctx, &path);
            }
        }
        if let Some(path) = requested_reveal {
            if let Err(err) = reveal_in_explorer(Path::new(&path)) {
                self.report_error(err);
            }
        }
        if let Some(old_path) = requested_relink {
            if let Some(new_path) = rfd::FileDialog::new()
                .add_filter("Shade projects", &["shade"])
                .pick_file()
            {
                match self.previous_shades.relink_path(&old_path, &new_path) {
                    Ok(new_display) => {
                        if let Err(err) = self.previous_shades.save() {
                            self.log.error(&err);
                        }
                        self.previous_shade_list_textures.remove(&old_path);
                        self.previous_shade_list_texture_lru.retain(|item| item != &old_path);
                        self.load_previous_shade_preview(ctx, &new_display);
                    }
                    Err(err) => self.report_error(err),
                }
            }
        }
        if let Some(path) = requested_remove {
            if self.previous_shades.remove_path(&path) {
                if let Err(err) = self.previous_shades.save() {
                    self.log.error(&err);
                }
                self.previous_shade_list_textures.remove(&path);
                self.previous_shade_list_texture_lru.retain(|item| item != &path);
                if self.previous_shades_selected.as_deref() == Some(path.as_str()) {
                    self.previous_shades_selected = None;
                    self.previous_shade_preview = None;
                    self.previous_shade_preview_error = None;
                    self.previous_shade_texture = None;
                }
            }
        }
        if let Some(path) = requested_open {
            self.show_previous_shades = false;
            self.open_project_path(PathBuf::from(path));
        }
    }

'''
text = replace_between(
    text,
    '    fn ui_previous_shades_window(&mut self, ctx: &egui::Context) {',
    '    fn ui_snapshot_save_reminder(&mut self, ctx: &egui::Context) {',
    previous_ui,
    'replace Previous Shades UI',
)

# Modified-state indicator in Adjustments.
text = replace_once(
    text,
    '        let panel_accent = (self.adjustment_scope == AdjustmentScope::Selected)\n            .then(|| channel_color(palette.as_ref(), &output_name, self.selected_channel));\n\n        ui.horizontal_wrapped(|ui| {\n            ui.heading("Adjustments");',
    '        let panel_accent = (self.adjustment_scope == AdjustmentScope::Selected)\n            .then(|| channel_color(palette.as_ref(), &output_name, self.selected_channel));\n        let modified_count = channel_names\n            .iter()\n            .filter(|name| {\n                self.project\n                    .adjustments\n                    .get(*name)\n                    .is_some_and(adjustment_is_modified)\n            })\n            .count();\n        let output_modified = self\n            .project\n            .adjustments\n            .get(&output_name)\n            .is_some_and(adjustment_is_modified);\n\n        ui.horizontal_wrapped(|ui| {\n            ui.heading("Adjustments");\n            if modified_count > 0 {\n                ui.small(format!("{modified_count}/{} modified", channel_names.len()));\n            }',
    'adjustment modified count',
)
text = replace_once(
    text,
    '            let selected = self.adjustment_scope == AdjustmentScope::Selected;\n            let response = with_accent(ui, control_accent, |ui| {\n                ui.add(egui::Button::new(output_display).selected(selected))\n            });',
    '            let selected = self.adjustment_scope == AdjustmentScope::Selected;\n            let channel_button_label = if output_modified {\n                format!("{output_display}  •")\n            } else {\n                output_display.clone()\n            };\n            let response = with_accent(ui, control_accent, |ui| {\n                ui.add(egui::Button::new(channel_button_label).selected(selected))\n            });',
    'selected channel modified dot',
)
text = replace_once(
    text,
    '        self.ui_previous_shades_window(ui.ctx());\n        self.ui_recovery_window(ui.ctx());',
    '        self.ui_previous_shades_window(ui.ctx());\n        self.ui_export_all_window(ui.ctx());\n        self.ui_recovery_window(ui.ctx());',
    'export all window update call',
)

# Add reveal helper and adjustment state helper before open_folder.
text = replace_once(
    text,
    'fn open_folder(path: &Path) -> Result<(), String> {',
    r'''fn adjustment_is_modified(adjustment: &ChannelAdjustment) -> bool {
    let default = ChannelAdjustment::default();
    adjustment.levels != default.levels
        || adjustment.mixer != default.mixer
        || adjustment.curve != default.curve
}

fn reveal_in_explorer(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        if path.is_file() {
            std::process::Command::new("explorer.exe")
                .arg("/select,")
                .arg(path)
                .spawn()
                .map_err(|err| format!("Cannot reveal {} in Explorer: {err}", path.display()))?;
            return Ok(());
        }
    }
    let folder = path.parent().unwrap_or(path);
    open_folder(folder)
}

fn open_folder(path: &Path) -> Result<(), String> {''',
    'reveal and modified helpers',
)
write(path, text)

# ---------------- version / notes ----------------
path = 'Cargo.toml'
text = read(path)
text = replace_once(text, 'version = "0.13.2"', 'version = "0.14.0"', 'cargo version')
write(path, text)

path = 'Cargo.lock'
text = read(path)
needle = 'name = "windows-shade-editor"\nversion = "0.13.2"'
if needle not in text:
    raise RuntimeError('Cargo.lock package version marker not found')
text = text.replace(needle, 'name = "windows-shade-editor"\nversion = "0.14.0"', 1)
write(path, text)

notes = read('RELEASE_NOTES.md')
header = '''# Shade Editor 0.14.0\n\n- Export All workspace with destination field, folder TIFF warning, template naming, overwrite/skip/auto-number policies, and optional Explorer reveal after export.\n- Previous Shades promoted to a Project Browser with Remove from history, relink for missing .shade files, row-level latest Snapshot/active Face details, Explorer reveal, lazy row rendering, and a bounded thumbnail LRU cache.\n- Project thumbnails now persist thumbnail_version and encoded_bytes metadata alongside width/height.\n- Adjustment panel shows per-channel modified state and the total number of modified channels.\n\n'''
if not notes.startswith('# Shade Editor 0.14.0'):
    notes = header + notes
write('RELEASE_NOTES.md', notes)
