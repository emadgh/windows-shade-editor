use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::{Deserialize, Serialize};

use crate::model::{FaceFileMetadata, ProjectThumbnail, ShadeProject};
use crate::thumbnail;

const SNAPSHOT_CACHE_VERSION: u32 = 4;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct CachedSnapshot {
    pub id: u64,
    pub name: String,
    /// Effective code rendered by Test Code for this snapshot. When the project
    /// does not override Test Code text, this is the snapshot name itself.
    pub code: String,
    pub created_at_unix_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PreviousShadeEntry {
    pub path: String,
    pub project_name: String,
    pub last_opened_unix_ms: i64,
    pub saved_at_unix_ms: i64,
    pub open_count: u64,
    /// Versioned local index so old Project View JSON can be upgraded once
    /// without rereading every .shade file on each application start.
    pub snapshot_cache_version: u32,
    pub snapshots: Vec<CachedSnapshot>,
    pub test_code_text: String,
    pub face_count: usize,
    pub active_face_index: usize,
    pub active_face_label: String,
    pub active_face_width: u32,
    pub active_face_height: u32,
    pub total_source_bytes: u64,
    pub thumbnail: Option<ProjectThumbnail>,
}

impl Default for PreviousShadeEntry {
    fn default() -> Self {
        Self {
            path: String::new(),
            project_name: String::new(),
            last_opened_unix_ms: 0,
            saved_at_unix_ms: 0,
            open_count: 0,
            snapshot_cache_version: 0,
            snapshots: Vec::new(),
            test_code_text: String::new(),
            face_count: 0,
            active_face_index: 0,
            active_face_label: String::new(),
            active_face_width: 0,
            active_face_height: 0,
            total_source_bytes: 0,
            thumbnail: None,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PreviousShadesStore {
    entries: Vec<PreviousShadeEntry>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviousShadesSort {
    #[default]
    LastOpened,
    ProjectName,
    SavedAt,
    Path,
}

impl PreviousShadesSort {
    pub fn label(self) -> &'static str {
        match self {
            Self::LastOpened => "Last opened",
            Self::ProjectName => "Project name",
            Self::SavedAt => "Saved time",
            Self::Path => "Path",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DecodedThumbnail {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct ShadeInspection {
    pub path: PathBuf,
    pub project_name: String,
    pub snapshot_count: usize,
    pub active_snapshot_name: Option<String>,
    pub snapshots: Vec<CachedSnapshot>,
    pub test_code_enabled: bool,
    pub saved_at_unix_ms: i64,
    pub face_count: usize,
    pub active_face_index: usize,
    pub total_source_bytes: u64,
    pub active_face: Option<FaceFileMetadata>,
    pub thumbnail: Option<DecodedThumbnail>,
    pub thumbnail_error: Option<String>,
    pub file_modified_unix_ms: Option<i64>,
}

impl PreviousShadeEntry {
    fn refresh_from_project(&mut self, project: &ShadeProject) {
        self.project_name = project_display_name(&project.name, Path::new(&self.path));
        self.test_code_text = project.test_code.text.trim().to_owned();
        self.snapshots = snapshot_cache_from_project(project);
        self.face_count = project
            .file_metadata
            .as_ref()
            .map(|metadata| metadata.face_count)
            .filter(|count| *count > 0)
            .unwrap_or(project.faces.len());
        self.active_face_index = project
            .file_metadata
            .as_ref()
            .map(|metadata| metadata.active_face_index)
            .unwrap_or(0);
        self.active_face_label = project
            .file_metadata
            .as_ref()
            .and_then(|metadata| {
                metadata
                    .faces
                    .get(self.active_face_index)
                    .or_else(|| metadata.faces.first())
            })
            .map(|face| {
                if face.label.trim().is_empty() {
                    face.source_file_name.trim().to_owned()
                } else {
                    face.label.trim().to_owned()
                }
            })
            .filter(|value| !value.is_empty())
            .or_else(|| {
                project
                    .faces
                    .get(self.active_face_index)
                    .or_else(|| project.faces.first())
                    .map(|face| face.label.trim().to_owned())
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_default();
        let active_face_metadata = project.file_metadata.as_ref().and_then(|metadata| {
            metadata
                .faces
                .get(self.active_face_index)
                .or_else(|| metadata.faces.first())
        });
        self.active_face_width = active_face_metadata.map(|face| face.width).unwrap_or(0);
        self.active_face_height = active_face_metadata.map(|face| face.height).unwrap_or(0);
        self.total_source_bytes = project
            .file_metadata
            .as_ref()
            .map(|metadata| metadata.total_source_bytes)
            .unwrap_or(0);
        self.thumbnail = project
            .thumbnail
            .as_ref()
            .and_then(build_cached_list_thumbnail);
        self.snapshot_cache_version = SNAPSHOT_CACHE_VERSION;
    }

    pub fn display_name(&self) -> String {
        project_display_name(&self.project_name, Path::new(&self.path))
    }

    pub fn matches_query(&self, query_lower: &str) -> bool {
        let query = query_lower.trim();
        if query.is_empty() {
            return true;
        }
        contains_case_insensitive(&self.display_name(), query)
            || contains_case_insensitive(&self.path, query)
            || self.test_code_matches(query)
            || self.matching_snapshot(query).is_some()
    }

    pub fn matching_snapshot(&self, query_lower: &str) -> Option<&CachedSnapshot> {
        let query = query_lower.trim();
        if query.is_empty() {
            return None;
        }
        self.snapshots.iter().find(|snapshot| {
            contains_case_insensitive(&snapshot.name, query)
                || contains_case_insensitive(&snapshot.code, query)
                || snapshot_id_matches(snapshot.id, query)
        })
    }

    pub fn test_code_matches(&self, query_lower: &str) -> bool {
        !self.test_code_text.trim().is_empty()
            && contains_case_insensitive(&self.test_code_text, query_lower.trim())
    }

    pub fn latest_snapshot(&self) -> Option<&CachedSnapshot> {
        self.snapshots
            .iter()
            .max_by_key(|snapshot| (snapshot.created_at_unix_ms, snapshot.id))
    }

    pub fn recent_snapshots(&self, limit: usize) -> Vec<&CachedSnapshot> {
        let mut snapshots = self.snapshots.iter().collect::<Vec<_>>();
        snapshots.sort_by(|left, right| {
            (right.created_at_unix_ms, right.id).cmp(&(left.created_at_unix_ms, left.id))
        });
        snapshots.truncate(limit);
        snapshots
    }

    pub fn active_face_pixel_size(&self) -> Option<(u32, u32)> {
        (self.active_face_width > 0 && self.active_face_height > 0)
            .then_some((self.active_face_width, self.active_face_height))
    }

    pub fn active_face_display(&self) -> String {
        if !self.active_face_label.trim().is_empty() {
            return format!(
                "Face {} · {}",
                self.active_face_index.saturating_add(1),
                self.active_face_label.trim()
            );
        }
        if self.face_count > 0 {
            format!(
                "Face {}",
                self.active_face_index
                    .saturating_add(1)
                    .min(self.face_count)
            )
        } else {
            "No face metadata".to_owned()
        }
    }

    pub fn is_missing(&self) -> bool {
        !Path::new(&self.path).is_file()
    }
}

impl PreviousShadesStore {
    pub fn load() -> Result<Self, String> {
        let path = history_path();
        let Ok(text) = fs::read_to_string(&path) else {
            return Ok(Self::default());
        };
        let mut store: Self = serde_json::from_str(&text)
            .map_err(|err| format!("Cannot parse Project View history: {err}"))?;
        store.sanitize();
        // Cache migration is intentionally one-shot. Existing history entries
        // created before snapshot indexing are hydrated from the .shade file
        // only when the file is currently available.
        if store.refresh_stale_snapshot_cache() {
            let _ = store.save();
        }
        Ok(store)
    }

    pub fn save(&self) -> Result<(), String> {
        let path = history_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("Cannot create Project View directory: {err}"))?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| format!("Cannot serialize Project View history: {err}"))?;
        fs::write(path, text).map_err(|err| format!("Cannot save Project View history: {err}"))
    }

    pub fn entries(&self) -> &[PreviousShadeEntry] {
        &self.entries
    }

    pub fn remove_path(&mut self, path: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| !same_path(&entry.path, path));
        self.entries.len() != before
    }

    pub fn relink_path(&mut self, old_path: &str, new_path: &Path) -> Result<String, String> {
        let project = ShadeProject::load(new_path)?;
        let normalized = fs::canonicalize(new_path).unwrap_or_else(|_| new_path.to_path_buf());
        let display = normalized.to_string_lossy().into_owned();
        let Some(index) = self
            .entries
            .iter()
            .position(|entry| same_path(&entry.path, old_path))
        else {
            return Err("Project View entry no longer exists.".to_owned());
        };
        let mut entry = self.entries.remove(index);
        entry.path = display.clone();
        entry.project_name = project_display_name(&project.name, new_path);
        entry.last_opened_unix_ms = unix_ms_now();
        entry.saved_at_unix_ms = project
            .file_metadata
            .as_ref()
            .map(|metadata| metadata.saved_at_unix_ms)
            .unwrap_or(entry.saved_at_unix_ms);
        entry.refresh_from_project(&project);
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|existing| same_path(&existing.path, &display))
        {
            existing.open_count = existing.open_count.max(entry.open_count).max(1);
            existing.last_opened_unix_ms =
                existing.last_opened_unix_ms.max(entry.last_opened_unix_ms);
            existing.saved_at_unix_ms = entry.saved_at_unix_ms;
            existing.project_name = entry.project_name;
            existing.snapshot_cache_version = entry.snapshot_cache_version;
            existing.snapshots = entry.snapshots;
            existing.test_code_text = entry.test_code_text;
            existing.face_count = entry.face_count;
            existing.active_face_index = entry.active_face_index;
            existing.active_face_label = entry.active_face_label;
            existing.active_face_width = entry.active_face_width;
            existing.active_face_height = entry.active_face_height;
            existing.total_source_bytes = entry.total_source_bytes;
            existing.thumbnail = entry.thumbnail;
        } else {
            self.entries.push(entry);
        }
        Ok(display)
    }

    pub fn record_open(&mut self, path: &Path, project_name: &str) {
        let normalized = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let display = normalized.to_string_lossy().into_owned();
        let now = unix_ms_now();
        let loaded_project = ShadeProject::load(path).ok();
        let saved_at = loaded_project
            .as_ref()
            .and_then(|project| {
                project
                    .file_metadata
                    .as_ref()
                    .map(|metadata| metadata.saved_at_unix_ms)
            })
            .unwrap_or_else(|| {
                fs::metadata(path)
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(system_time_to_unix_ms)
                    .unwrap_or(0)
            });
        let cached_name = loaded_project
            .as_ref()
            .map(|project| project_display_name(&project.name, path))
            .unwrap_or_else(|| project_display_name(project_name, path));

        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| same_path(&entry.path, &display))
        {
            entry.path = display;
            entry.project_name = cached_name;
            entry.last_opened_unix_ms = now;
            entry.saved_at_unix_ms = saved_at;
            entry.open_count = entry.open_count.saturating_add(1).max(1);
            if let Some(project) = loaded_project.as_ref() {
                // This path is used after Open, Save and Quick Save, so every
                // successful project save immediately refreshes its snapshot
                // names/codes in the persistent Project View cache.
                entry.refresh_from_project(project);
            }
        } else {
            let mut entry = PreviousShadeEntry {
                path: display,
                project_name: cached_name,
                last_opened_unix_ms: now,
                saved_at_unix_ms: saved_at,
                open_count: 1,
                ..PreviousShadeEntry::default()
            };
            if let Some(project) = loaded_project.as_ref() {
                entry.refresh_from_project(project);
            }
            self.entries.push(entry);
        }
    }

    fn refresh_stale_snapshot_cache(&mut self) -> bool {
        let mut changed = false;
        for entry in &mut self.entries {
            if entry.snapshot_cache_version >= SNAPSHOT_CACHE_VERSION {
                continue;
            }
            let path = Path::new(&entry.path);
            let Ok(project) = ShadeProject::load(path) else {
                continue;
            };
            entry.refresh_from_project(&project);
            if let Some(saved_at) = project
                .file_metadata
                .as_ref()
                .map(|metadata| metadata.saved_at_unix_ms)
                .filter(|saved_at| *saved_at > 0)
            {
                entry.saved_at_unix_ms = saved_at;
            }
            changed = true;
        }
        changed
    }

    fn sanitize(&mut self) {
        let mut sanitized: Vec<PreviousShadeEntry> = Vec::new();
        for mut entry in self.entries.drain(..) {
            entry.path = entry.path.trim().to_owned();
            entry.project_name = entry.project_name.trim().to_owned();
            entry.test_code_text = entry.test_code_text.trim().to_owned();
            for snapshot in &mut entry.snapshots {
                snapshot.name = snapshot.name.trim().to_owned();
                snapshot.code = snapshot.code.trim().to_owned();
            }
            if entry.path.is_empty() {
                continue;
            }
            if let Some(existing) = sanitized
                .iter_mut()
                .find(|existing| same_path(&existing.path, &entry.path))
            {
                if entry.last_opened_unix_ms >= existing.last_opened_unix_ms {
                    existing.path = entry.path;
                    existing.project_name = entry.project_name;
                    existing.last_opened_unix_ms = entry.last_opened_unix_ms;
                    existing.saved_at_unix_ms = entry.saved_at_unix_ms;
                    existing.snapshot_cache_version = entry.snapshot_cache_version;
                    existing.snapshots = entry.snapshots.clone();
                    existing.test_code_text = entry.test_code_text.clone();
                    existing.face_count = entry.face_count;
                    existing.active_face_index = entry.active_face_index;
                    existing.active_face_label = entry.active_face_label.clone();
                    existing.active_face_width = entry.active_face_width;
                    existing.active_face_height = entry.active_face_height;
                    existing.total_source_bytes = entry.total_source_bytes;
                    existing.thumbnail = entry.thumbnail.clone();
                } else if entry.snapshot_cache_version > existing.snapshot_cache_version {
                    existing.snapshot_cache_version = entry.snapshot_cache_version;
                    existing.snapshots = entry.snapshots.clone();
                    existing.test_code_text = entry.test_code_text.clone();
                    existing.face_count = entry.face_count;
                    existing.active_face_index = entry.active_face_index;
                    existing.active_face_label = entry.active_face_label.clone();
                    existing.active_face_width = entry.active_face_width;
                    existing.active_face_height = entry.active_face_height;
                    existing.total_source_bytes = entry.total_source_bytes;
                    existing.thumbnail = entry.thumbnail.clone();
                }
                existing.open_count = existing.open_count.max(entry.open_count).max(1);
            } else {
                entry.open_count = entry.open_count.max(1);
                sanitized.push(entry);
            }
        }
        self.entries = sanitized;
    }
}

pub fn inspect(path: &Path) -> Result<ShadeInspection, String> {
    let project = ShadeProject::load(path)?;
    let metadata = project.file_metadata.clone();
    let saved_at_unix_ms = metadata
        .as_ref()
        .map(|metadata| metadata.saved_at_unix_ms)
        .unwrap_or(0);
    let face_count = metadata
        .as_ref()
        .map(|metadata| metadata.face_count)
        .filter(|count| *count > 0)
        .unwrap_or(project.faces.len());
    let active_face_index = metadata
        .as_ref()
        .map(|metadata| metadata.active_face_index)
        .unwrap_or(0);
    let total_source_bytes = metadata
        .as_ref()
        .map(|metadata| metadata.total_source_bytes)
        .unwrap_or(0);
    let active_face = metadata.as_ref().and_then(|metadata| {
        metadata
            .faces
            .get(active_face_index)
            .or_else(|| metadata.faces.first())
            .cloned()
    });
    let (thumbnail, thumbnail_error) = match project.thumbnail.as_ref() {
        Some(thumbnail) => match decode_thumbnail(thumbnail) {
            Ok(decoded) => (Some(decoded), None),
            Err(err) => (None, Some(err)),
        },
        None => (None, None),
    };
    let file_modified_unix_ms = fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(system_time_to_unix_ms);

    Ok(ShadeInspection {
        path: path.to_path_buf(),
        project_name: project_display_name(&project.name, path),
        snapshot_count: project.snapshots.len(),
        active_snapshot_name: project.active_snapshot_name().map(str::to_owned),
        snapshots: snapshot_cache_from_project(&project),
        test_code_enabled: project.test_code.enabled,
        saved_at_unix_ms,
        face_count,
        active_face_index,
        total_source_bytes,
        active_face,
        thumbnail,
        thumbnail_error,
        file_modified_unix_ms,
    })
}

fn decode_thumbnail(thumbnail: &ProjectThumbnail) -> Result<DecodedThumbnail, String> {
    if !thumbnail.mime_type.eq_ignore_ascii_case("image/png") {
        return Err(format!(
            "Unsupported project thumbnail type: {}",
            thumbnail.mime_type
        ));
    }
    let bytes = BASE64_STANDARD
        .decode(thumbnail.data_base64.as_bytes())
        .map_err(|err| format!("Invalid project thumbnail base64: {err}"))?;
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let mut reader = decoder
        .read_info()
        .map_err(|err| format!("Cannot read project thumbnail PNG: {err}"))?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| "Project thumbnail PNG exceeds decoder limits.".to_owned())?;
    let mut buffer = vec![0u8; size];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|err| format!("Cannot decode project thumbnail PNG: {err}"))?;
    let pixels = &buffer[..info.buffer_size()];
    if info.bit_depth != png::BitDepth::Eight {
        return Err("Project thumbnail PNG is not 8-bit.".to_owned());
    }
    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => {
            let mut rgba = Vec::with_capacity(info.width as usize * info.height as usize * 4);
            for pixel in pixels.chunks_exact(3) {
                rgba.extend_from_slice(pixel);
                rgba.push(255);
            }
            rgba
        }
        _ => return Err("Project thumbnail PNG must be RGB or RGBA.".to_owned()),
    };
    Ok(DecodedThumbnail {
        width: info.width as usize,
        height: info.height as usize,
        rgba,
    })
}

pub fn decode_cached_thumbnail(
    entry: &PreviousShadeEntry,
) -> Result<Option<DecodedThumbnail>, String> {
    entry.thumbnail.as_ref().map(decode_thumbnail).transpose()
}

fn project_display_name(project_name: &str, path: &Path) -> String {
    let trimmed = project_name.trim();
    if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("Untitled Shade") {
        return trimmed.to_owned();
    }
    path.file_stem()
        .map(|stem| stem.to_string_lossy().trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Shade project".to_owned())
}

fn snapshot_cache_from_project(project: &ShadeProject) -> Vec<CachedSnapshot> {
    let explicit_code = project.test_code.text.trim();
    project
        .snapshots
        .iter()
        .map(|snapshot| {
            let name = snapshot.name.trim().to_owned();
            CachedSnapshot {
                id: snapshot.id,
                code: if explicit_code.is_empty() {
                    name.clone()
                } else {
                    explicit_code.to_owned()
                },
                name,
                created_at_unix_ms: snapshot.created_at_unix_ms,
            }
        })
        .collect()
}

fn build_cached_list_thumbnail(source: &ProjectThumbnail) -> Option<ProjectThumbnail> {
    let decoded = decode_thumbnail(source).ok()?;
    let (width, height, rgba) =
        thumbnail::resize_rgba(decoded.width, decoded.height, &decoded.rgba, 72).ok()?;
    let png = thumbnail::encode_png(width as u32, height as u32, &rgba).ok()?;
    let encoded_bytes = png.len() as u64;
    Some(ProjectThumbnail {
        mime_type: "image/png".to_owned(),
        thumbnail_version: 1,
        width: width as u32,
        height: height as u32,
        encoded_bytes,
        data_base64: BASE64_STANDARD.encode(png),
    })
}

fn history_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("ShadeEditor").join("previous_shades.json")
}

fn unix_ms_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn system_time_to_unix_ms(value: SystemTime) -> Option<i64> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
}

fn contains_case_insensitive(value: &str, query_lower: &str) -> bool {
    !query_lower.is_empty() && value.to_lowercase().contains(query_lower)
}

fn snapshot_id_matches(id: u64, query: &str) -> bool {
    let normalized = query.trim().strip_prefix('#').unwrap_or(query.trim());
    normalized.parse::<u64>().ok() == Some(id)
}

fn same_path(left: &str, right: &str) -> bool {
    #[cfg(windows)]
    {
        left.eq_ignore_ascii_case(right)
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_deduplicates_paths_and_updates_open_count() {
        let mut store = PreviousShadesStore::default();
        store.record_open(Path::new("C:/work/example.shade"), "First");
        store.record_open(Path::new("C:/work/example.shade"), "Second");
        assert_eq!(store.entries().len(), 1);
        assert_eq!(store.entries()[0].project_name, "Second");
        assert_eq!(store.entries()[0].open_count, 2);
        assert_eq!(store.entries()[0].display_name(), "Second");
    }

    #[test]
    fn snapshot_cache_searches_name_code_and_id_and_refreshes_after_save() {
        let root =
            std::env::temp_dir().join(format!("shade-editor-previous-shades-{}", unix_ms_now()));
        fs::create_dir_all(&root).unwrap();
        let shade_path = root.join("cache-test.shade");

        let mut project = ShadeProject::default();
        project.name = "Floor Tile 24".to_owned();
        let snapshot_id = project.create_snapshot();
        project.rename_snapshot(snapshot_id, "Kiln-A-042").unwrap();
        project.test_code.text = "TC-042".to_owned();
        project.save(&shade_path, &[]).unwrap();

        let mut store = PreviousShadesStore::default();
        store.record_open(&shade_path, "fallback");
        let entry = &store.entries()[0];
        assert_eq!(entry.snapshot_cache_version, SNAPSHOT_CACHE_VERSION);
        assert_eq!(entry.snapshots.len(), 1);
        assert_eq!(entry.snapshots[0].name, "Kiln-A-042");
        assert_eq!(entry.snapshots[0].code, "TC-042");
        assert!(entry.matches_query("kiln-a-042"));
        assert!(entry.matches_query("tc-042"));
        assert!(entry.matches_query(&format!("#{snapshot_id}")));

        project.rename_snapshot(snapshot_id, "Kiln-A-043").unwrap();
        project.test_code.text = "TC-043".to_owned();
        project.save(&shade_path, &[]).unwrap();
        store.record_open(&shade_path, &project.name);
        let entry = &store.entries()[0];
        assert!(!entry.matches_query("kiln-a-042"));
        assert!(!entry.matches_query("tc-042"));
        assert!(entry.matches_query("kiln-a-043"));
        assert!(entry.matches_query("tc-043"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recent_snapshots_are_newest_first_and_limited() {
        let mut entry = PreviousShadeEntry::default();
        for id in 1..=10 {
            entry.snapshots.push(CachedSnapshot {
                id,
                name: format!("S{id}"),
                code: String::new(),
                created_at_unix_ms: id as i64 * 100,
            });
        }
        let recent = entry.recent_snapshots(8);
        assert_eq!(recent.len(), 8);
        assert_eq!(recent[0].name, "S10");
        assert_eq!(recent[7].name, "S3");
    }

    #[test]
    fn untitled_history_uses_shade_filename() {
        let entry = PreviousShadeEntry {
            path: "C:/work/blue-17.shade".to_owned(),
            project_name: "Untitled Shade".to_owned(),
            ..PreviousShadeEntry::default()
        };
        assert_eq!(entry.display_name(), "blue-17");
    }

    #[test]
    fn sort_labels_are_stable() {
        assert_eq!(PreviousShadesSort::LastOpened.label(), "Last opened");
        assert_eq!(PreviousShadesSort::ProjectName.label(), "Project name");
        assert_eq!(PreviousShadesSort::SavedAt.label(), "Saved time");
        assert_eq!(PreviousShadesSort::Path.label(), "Path");
    }
}
