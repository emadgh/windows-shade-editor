use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::{Deserialize, Serialize};

use crate::model::{FaceFileMetadata, ProjectThumbnail, ShadeProject};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PreviousShadeEntry {
    pub path: String,
    pub project_name: String,
    pub last_opened_unix_ms: i64,
    pub saved_at_unix_ms: i64,
    pub open_count: u64,
}

impl Default for PreviousShadeEntry {
    fn default() -> Self {
        Self {
  path: String::new(),
  project_name: String::new(),
  last_opened_unix_ms: 0,
  saved_at_unix_ms: 0,
  open_count: 0,
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

impl PreviousShadesStore {
    pub fn load() -> Result<Self, String> {
        let path = history_path();
        let Ok(text) = fs::read_to_string(&path) else {
  return Ok(Self::default());
        };
        let mut store: Self = serde_json::from_str(&text)
  .map_err(|err| format!("Cannot parse Previous Shades history: {err}"))?;
        store.sanitize();
        Ok(store)
    }

    pub fn save(&self) -> Result<(), String> {
        let path = history_path();
        if let Some(parent) = path.parent() {
  fs::create_dir_all(parent)
      .map_err(|err| format!("Cannot create Previous Shades directory: {err}"))?;
        }
        let text = serde_json::to_string_pretty(self)
  .map_err(|err| format!("Cannot serialize Previous Shades history: {err}"))?;
        fs::write(path, text)
  .map_err(|err| format!("Cannot save Previous Shades history: {err}"))
    }

    pub fn entries(&self) -> &[PreviousShadeEntry] {
        &self.entries
    }

    pub fn record_open(&mut self, path: &Path, project_name: &str) {
        let normalized = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let display = normalized.to_string_lossy().into_owned();
        let now = unix_ms_now();
        let saved_at = ShadeProject::load(path)
  .ok()
  .and_then(|project| project.file_metadata.map(|metadata| metadata.saved_at_unix_ms))
  .unwrap_or_else(|| {
      fs::metadata(path)
          .ok()
          .and_then(|metadata| metadata.modified().ok())
          .and_then(system_time_to_unix_ms)
          .unwrap_or(0)
  });
        if let Some(entry) = self
  .entries
  .iter_mut()
  .find(|entry| same_path(&entry.path, &display))
        {
  entry.path = display;
  entry.project_name = project_name.trim().to_owned();
  entry.last_opened_unix_ms = now;
  entry.saved_at_unix_ms = saved_at;
  entry.open_count = entry.open_count.saturating_add(1).max(1);
        } else {
  self.entries.push(PreviousShadeEntry {
      path: display,
      project_name: project_name.trim().to_owned(),
      last_opened_unix_ms: now,
      saved_at_unix_ms: saved_at,
      open_count: 1,
  });
        }
    }

    fn sanitize(&mut self) {
        let mut sanitized: Vec<PreviousShadeEntry> = Vec::new();
        for mut entry in self.entries.drain(..) {
  entry.path = entry.path.trim().to_owned();
  entry.project_name = entry.project_name.trim().to_owned();
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
        project_name: project.name.clone(),
        snapshot_count: project.snapshots.len(),
        active_snapshot_name: project.active_snapshot_name().map(str::to_owned),
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
    if info.bit_depth != png::BitDepth::Eight || info.color_type != png::ColorType::Rgba {
        return Err("Project thumbnail PNG is not 8-bit RGBA.".to_owned());
    }
    Ok(DecodedThumbnail {
        width: info.width as usize,
        height: info.height as usize,
        rgba: pixels.to_vec(),
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
    }

    #[test]
    fn sort_labels_are_stable() {
        assert_eq!(PreviousShadesSort::LastOpened.label(), "Last opened");
        assert_eq!(PreviousShadesSort::ProjectName.label(), "Project name");
        assert_eq!(PreviousShadesSort::SavedAt.label(), "Saved time");
        assert_eq!(PreviousShadesSort::Path.label(), "Path");
    }
}
