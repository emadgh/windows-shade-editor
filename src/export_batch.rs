use std::fs;
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
    let shade_name = context
        .shade_name
        .and_then(nonempty)
        .unwrap_or(project_name);
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
    for ch in value.chars() {
        let invalid =
            ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*');
        output.push(if invalid { '-' } else { ch });
    }
    let trimmed = output
        .trim()
        .trim_matches('.')
        .trim()
        .trim_matches('-')
        .trim();
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

pub fn resolve_destination(
    folder: &Path,
    filename: &str,
    policy: ConflictPolicy,
) -> DestinationDecision {
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
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
        })
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("tif") || ext.eq_ignore_ascii_case("tiff")
                })
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
