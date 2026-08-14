use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const DEFAULT_EXPORT_TEMPLATE: &str = "{project}_{face}_{snapshot}_{date}";
pub const DEFAULT_FOLDER_TEMPLATE: &str = "";

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
    pub source_name: &'a str,
    pub date: &'a str,
}

fn render_tokens(template: &str, context: &ExportNameContext<'_>) -> String {
    let project_name = nonempty(context.project_name).unwrap_or("Shade");
    let shade_name = context
        .shade_name
        .and_then(nonempty)
        .unwrap_or(project_name);
    let snapshot = nonempty(context.snapshot_code).unwrap_or("Working");
    let face = nonempty(context.face_name).unwrap_or("face");
    let source = nonempty(context.source_name).unwrap_or(face);
    let mut value = template.to_owned();

    // New compact tokens.
    value = value.replace("{project}", project_name);
    value = value.replace("{face}", face);
    value = value.replace("{snapshot}", snapshot);
    value = value.replace("{source}", source);
    value = value.replace("{date}", context.date);

    // Backward-compatible tokens from earlier Shade Editor builds.
    value = value.replace("{shade-name|project-name}", shade_name);
    value = value.replace("{shade-name}", shade_name);
    value = value.replace("{project-name}", project_name);
    value = value.replace("{snapshot-code}", snapshot);
    value = value.replace("{face-number}", &context.face_number.to_string());
    value = value.replace("{face-name}", face);
    value
}

pub fn render_export_filename(template: &str, context: &ExportNameContext<'_>) -> String {
    let template = if template.trim().is_empty() {
        DEFAULT_EXPORT_TEMPLATE
    } else {
        template
    };
    let stem = sanitize_filename_stem(&render_tokens(template, context));
    format!("{stem}.tif")
}

pub fn render_export_folder(
    base_folder: &Path,
    template: &str,
    context: &ExportNameContext<'_>,
) -> PathBuf {
    if template.trim().is_empty() {
        return base_folder.to_path_buf();
    }
    let rendered = render_tokens(template, context);
    let mut output = base_folder.to_path_buf();
    for component in rendered.split(['/', '\\']) {
        let component = component.trim();
        if component.is_empty() || component == "." || component == ".." {
            continue;
        }
        output.push(sanitize_folder_component(component));
    }
    output
}

pub fn sanitize_filename_stem(value: &str) -> String {
    sanitize_component(value, "shade-export")
}

fn sanitize_folder_component(value: &str) -> String {
    sanitize_component(value, "output")
}

fn sanitize_component(value: &str, fallback: &str) -> String {
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
        fallback.to_owned()
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
    let mut reserved = BTreeSet::new();
    resolve_destination_reserved(folder, filename, policy, &mut reserved)
}

pub fn resolve_destination_reserved(
    folder: &Path,
    filename: &str,
    policy: ConflictPolicy,
    reserved: &mut BTreeSet<PathBuf>,
) -> DestinationDecision {
    let target = folder.join(filename);
    if policy == ConflictPolicy::Overwrite {
        reserved.insert(target.clone());
        return DestinationDecision::Write(target);
    }
    if !target.exists() && !reserved.contains(&target) {
        reserved.insert(target.clone());
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
        if !candidate.exists() && !reserved.contains(&candidate) {
            reserved.insert(candidate.clone());
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

    fn context<'a>() -> ExportNameContext<'a> {
        ExportNameContext {
            shade_name: Some("Tile Blue"),
            project_name: "Project A",
            snapshot_code: "T-42",
            face_number: 3,
            face_name: "Face 3",
            source_name: "source-file",
            date: "2026-08-14",
        }
    }

    #[test]
    fn new_template_tokens_render() {
        let name = render_export_filename("{project}_{face}_{snapshot}_{date}", &context());
        assert_eq!(name, "Project A_Face 3_T-42_2026-08-14.tif");
    }

    #[test]
    fn legacy_tokens_still_render() {
        let name = render_export_filename(
            "{shade-name|project-name} - ({snapshot-code}) - {face-number}",
            &context(),
        );
        assert_eq!(name, "Tile Blue - (T-42) - 3.tif");
    }

    #[test]
    fn folder_template_is_safe_and_nested() {
        let folder = render_export_folder(
            Path::new(r"C:\exports"),
            "{project}/{date}/{snapshot}",
            &context(),
        );
        assert!(folder.ends_with(Path::new(r"Project A\2026-08-14\T-42")));
    }

    #[test]
    fn windows_reserved_filename_characters_are_sanitized() {
        assert_eq!(sanitize_filename_stem("A*B:C?D"), "A-B-C-D");
    }
}
