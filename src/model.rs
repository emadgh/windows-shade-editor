use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::color_conversion::{LinkedProjectRef, ProductionProvenance, ProjectRole};
use crate::safe_fs;

use crate::palette::ChannelPalette;

pub const SHADE_SCHEMA_VERSION: u32 = 9;
pub const TEST_CODE_ALL_CHANNELS: &str = "__all_channels__";
/// Reserved adjustment-map key for Photoshop-style Master/All Channels Levels and Curve.
/// It is intentionally stored beside per-channel adjustments so snapshots/history/export
/// recipes preserve it without changing the .shade schema. TIFF channel names never use it.
pub const MASTER_ADJUSTMENT_KEY: &str = "__shade_editor_master__";
pub const MIN_HISTORY_STEPS: usize = 10;
pub const DEFAULT_HISTORY_STEPS: usize = 50;
pub const MAX_SNAPSHOT_HISTORY_STATES: usize = 200;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PreviewRenderingIntent {
    Perceptual,
    RelativeColorimetric,
    Saturation,
    AbsoluteColorimetric,
}

impl Default for PreviewRenderingIntent {
    fn default() -> Self {
        Self::Perceptual
    }
}

impl PreviewRenderingIntent {
    pub fn label(self) -> &'static str {
        match self {
            Self::Perceptual => "Perceptual",
            Self::RelativeColorimetric => "Relative colorimetric",
            Self::Saturation => "Saturation",
            Self::AbsoluteColorimetric => "Absolute colorimetric",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct IccProfileIdentity {
    pub description: String,
    pub sha256: String,
}

/// Explicit production interpretation of one source Face's samples.
///
/// This is deliberately separate from `PreviewColorSettings`: assigning this
/// profile does not transform source pixels and preview-only profile choices do
/// not satisfy production-conversion preflight. ICC bytes remain external; the
/// stable identity detects a missing or replaced profile before conversion.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ProductionSourceProfileAssignment {
    pub path: String,
    pub identity: IccProfileIdentity,
}

/// Project-owned display-only color setup. It is serialized in `.shade`, but is
/// never consumed by TIFF export and never changes source TIFF metadata/samples.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PreviewColorSettings {
    pub enabled: bool,
    /// None = use the active TIFF's embedded profile. Some(path) = temporarily
    /// assign that ICC/ICM profile to the TIFF base channels for app preview.
    pub assigned_profile_path: Option<String>,
    /// Identity metadata only; ICC payloads are never embedded in `.shade`.
    pub assigned_profile_identity: Option<IccProfileIdentity>,
    pub rendering_intent: PreviewRenderingIntent,
    pub black_point_compensation: bool,
    /// Optional printer/RIP proof-device profile used only for on-screen soft proof.
    pub soft_proof_enabled: bool,
    pub proof_profile_path: Option<String>,
    /// Identity metadata only; ICC payloads are never embedded in `.shade`.
    pub proof_profile_identity: Option<IccProfileIdentity>,
    pub proofing_intent: PreviewRenderingIntent,
}

impl Default for PreviewColorSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            assigned_profile_path: None,
            assigned_profile_identity: None,
            rendering_intent: PreviewRenderingIntent::Perceptual,
            black_point_compensation: false,
            soft_proof_enabled: false,
            proof_profile_path: None,
            proof_profile_identity: None,
            proofing_intent: PreviewRenderingIntent::RelativeColorimetric,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShadeProject {
    pub schema_version: u32,
    pub name: String,
    /// Explicit source/production role. Legacy projects remain standalone.
    #[serde(default)]
    pub project_role: ProjectRole,
    /// Portable references to the source/production project on the other side
    /// of a conversion workflow. Missing links never make this project unusable.
    #[serde(default)]
    pub linked_projects: Vec<LinkedProjectRef>,
    /// Per-output lineage captured from immutable conversion job inputs/results.
    /// Source projects normally keep this empty; Production projects keep one
    /// entry per converted Face/version.
    #[serde(default)]
    pub production_provenance: Vec<ProductionProvenance>,
    pub faces: Vec<FaceRef>,
    pub adjustments: BTreeMap<String, ChannelAdjustment>,
    #[serde(default)]
    pub snapshots: Vec<AdjustmentSnapshot>,
    #[serde(default)]
    pub active_snapshot_id: Option<u64>,
    #[serde(default = "default_next_snapshot_id")]
    pub next_snapshot_id: u64,
    #[serde(default)]
    pub test_code: TestCodeConfig,
    /// Visual channel aliases/colors selected for this project. TIFF channel
    /// names and separation order are never changed by this palette.
    #[serde(default)]
    pub channel_palette: Option<ChannelPalette>,
    /// Preview-only ICC assignment and transform options for this project.
    #[serde(default)]
    pub preview_color: PreviewColorSettings,
    /// Embedded project thumbnail. This is a normal PNG encoded as base64 so
    /// the .shade JSON remains self-contained and portable.
    #[serde(default)]
    pub thumbnail: Option<ProjectThumbnail>,
    /// Cached source/project properties captured on save for fast inspection
    /// without reopening every TIFF.
    #[serde(default)]
    pub file_metadata: Option<ProjectFileMetadata>,
}

impl Default for ShadeProject {
    fn default() -> Self {
        Self {
            schema_version: SHADE_SCHEMA_VERSION,
            name: String::new(),
            project_role: ProjectRole::Standalone,
            linked_projects: Vec::new(),
            production_provenance: Vec::new(),
            faces: Vec::new(),
            adjustments: BTreeMap::new(),
            snapshots: Vec::new(),
            active_snapshot_id: None,
            next_snapshot_id: default_next_snapshot_id(),
            test_code: TestCodeConfig::default(),
            channel_palette: None,
            preview_color: PreviewColorSettings::default(),
            thumbnail: None,
            file_metadata: None,
        }
    }
}

fn default_next_snapshot_id() -> u64 {
    1
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FaceStatus {
    #[default]
    Accepted,
    Rejected,
}

impl FaceStatus {
    pub fn is_rejected(self) -> bool {
        self == Self::Rejected
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FaceRef {
    pub path: String,
    pub label: String,
    #[serde(default)]
    pub status: FaceStatus,
    /// Optional per-Face source-profile interpretation used only by production
    /// color conversion. An embedded source ICC remains the default when this
    /// explicit override is absent.
    #[serde(default)]
    pub production_source_profile: Option<ProductionSourceProfileAssignment>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectThumbnail {
    pub mime_type: String,
    #[serde(default = "default_thumbnail_version")]
    pub thumbnail_version: u32,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub encoded_bytes: u64,
    pub data_base64: String,
}

fn default_thumbnail_version() -> u32 {
    1
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ProjectFileMetadata {
    pub saved_at_unix_ms: i64,
    pub face_count: usize,
    pub active_face_index: usize,
    pub total_source_bytes: u64,
    pub faces: Vec<FaceFileMetadata>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct FaceFileMetadata {
    pub label: String,
    pub source_file_name: String,
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub color_model: String,
    pub channel_count: usize,
    pub base_channel_count: usize,
    pub channel_names: Vec<String>,
    pub dpi_x: f64,
    pub dpi_y: f64,
    pub dpi_from_source: bool,
    pub resolution_unit: u16,
    pub file_size_bytes: u64,
    pub modified_at_unix_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChannelAdjustment {
    pub enabled: bool,
    pub levels: Levels,
    pub curve: Curve,
    pub mixer: MixerRow,
}

impl Default for ChannelAdjustment {
    fn default() -> Self {
        Self {
            enabled: true,
            levels: Levels::default(),
            curve: Curve::default(),
            mixer: MixerRow::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Levels {
    pub input_black: f32,
    pub gamma: f32,
    pub input_white: f32,
    pub output_black: f32,
    pub output_white: f32,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            input_black: 0.0,
            gamma: 1.0,
            input_white: 1.0,
            output_black: 0.0,
            output_white: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Curve {
    /// Input coordinate of the black endpoint.
    pub input_black: f32,
    /// Whether the optional middle point participates in the Curve.
    pub midpoint_enabled: bool,
    /// Input coordinate of the optional draggable middle point.
    pub midpoint_input: f32,
    /// Input coordinate of the white endpoint.
    pub input_white: f32,
    /// Output coordinate of the black endpoint.
    pub black: f32,
    /// Output coordinate of the draggable middle point.
    pub midpoint: f32,
    /// Output coordinate of the white endpoint.
    pub white: f32,
}

impl Default for Curve {
    fn default() -> Self {
        Self {
            input_black: 0.0,
            midpoint_enabled: false,
            midpoint_input: 0.5,
            input_white: 1.0,
            black: 0.0,
            midpoint: 0.5,
            white: 1.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct MixerRow {
    pub coefficients: BTreeMap<String, f32>,
    pub constant: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotExportRecord {
    pub face_key: String,
    pub folder: String,
    pub exported_at_unix_ms: i64,
    #[serde(default)]
    pub test_code: String,
    #[serde(default)]
    pub adjustment_sha256: String,
    #[serde(default)]
    pub destination: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct SnapshotHistoryState {
    pub label: String,
    pub adjustments: BTreeMap<String, ChannelAdjustment>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct SnapshotAdjustmentHistory {
    pub entries: Vec<SnapshotHistoryState>,
    pub cursor: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdjustmentSnapshot {
    pub id: u64,
    pub name: String,
    /// Snapshot creation time as Unix milliseconds.
    #[serde(default)]
    pub created_at_unix_ms: i64,
    pub adjustments: BTreeMap<String, ChannelAdjustment>,
    /// Latest successful export per source Face. This is UI history only and
    /// never prevents another export.
    #[serde(default)]
    pub exports: Vec<SnapshotExportRecord>,
    /// Adjustment undo/redo history owned by this Snapshot. Stored in the
    /// .shade project so switching Snapshots and reopening a project preserves
    /// each Snapshot's independent edit trail.
    #[serde(default)]
    pub history: SnapshotAdjustmentHistory,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TestCodePosition {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Default for TestCodePosition {
    fn default() -> Self {
        Self::TopLeft
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TestCodeConfig {
    pub enabled: bool,
    pub text: String,
    pub channel: String,
    pub font_family: String,
    pub font_size_pt: f32,
    pub margin_cm: f32,
    pub position: TestCodePosition,
}

impl Default for TestCodeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            text: String::new(),
            channel: TEST_CODE_ALL_CHANNELS.to_owned(),
            font_family: "Tahoma".to_owned(),
            font_size_pt: 12.0,
            margin_cm: 1.0,
            position: TestCodePosition::TopLeft,
        }
    }
}

impl ShadeProject {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text =
            fs::read_to_string(path).map_err(|err| format!("Cannot read .shade file: {err}"))?;
        let mut project: Self =
            serde_json::from_str(&text).map_err(|err| format!("Invalid .shade file: {err}"))?;
        if project.schema_version != SHADE_SCHEMA_VERSION {
            return Err(format!(
                "Unsupported .shade schema {}. This build accepts schema {} only; pre-production migration code has been removed.",
                project.schema_version, SHADE_SCHEMA_VERSION
            ));
        }
        project.ensure_snapshot_histories();
        Ok(project)
    }

    pub fn save(&self, path: &Path, resolved_face_paths: &[PathBuf]) -> Result<(), String> {
        let mut portable = self.clone();
        portable.schema_version = SHADE_SCHEMA_VERSION;
        let project_dir = path.parent().unwrap_or_else(|| Path::new("."));
        for (face, source) in portable.faces.iter_mut().zip(resolved_face_paths.iter()) {
            face.path = make_portable_path(source, project_dir);
        }
        let text = serde_json::to_string_pretty(&portable)
            .map_err(|err| format!("Cannot serialize project: {err}"))?;
        let backup = safe_fs::backup_path(path);
        safe_fs::atomic_write(path, text.as_bytes(), Some(&backup))
            .map_err(|err| format!("Cannot safely save .shade file {}: {err}", path.display()))
    }

    pub fn save_new(&self, path: &Path, resolved_face_paths: &[PathBuf]) -> Result<(), String> {
        let mut portable = self.clone();
        portable.schema_version = SHADE_SCHEMA_VERSION;
        let project_dir = path.parent().unwrap_or_else(|| Path::new("."));
        for (face, source) in portable.faces.iter_mut().zip(resolved_face_paths.iter()) {
            face.path = make_portable_path(source, project_dir);
        }
        let text = serde_json::to_string_pretty(&portable)
            .map_err(|err| format!("Cannot serialize project: {err}"))?;
        safe_fs::atomic_write_if_absent(path, text.as_bytes()).map_err(|err| {
            format!(
                "Cannot safely create new .shade file {}: {err}",
                path.display()
            )
        })
    }

    pub fn resolve_face_paths(&self, shade_path: &Path) -> Vec<PathBuf> {
        let base = shade_path.parent().unwrap_or_else(|| Path::new("."));
        self.faces
            .iter()
            .map(|face| {
                let path = PathBuf::from(&face.path);
                if path.is_absolute() {
                    path
                } else {
                    base.join(path)
                }
            })
            .collect()
    }

    pub fn ensure_channels(&mut self, names: &[String]) {
        ensure_adjustment_channels(&mut self.adjustments, names);
        for snapshot in &mut self.snapshots {
            ensure_adjustment_channels(&mut snapshot.adjustments, names);
        }
        if self.test_code.channel != TEST_CODE_ALL_CHANNELS
            && !names.iter().any(|name| name == &self.test_code.channel)
        {
            self.test_code.channel = names
                .get(3)
                .or_else(|| names.first())
                .cloned()
                .unwrap_or_default();
        }
    }

    pub fn reset_adjustments(&mut self, names: &[String]) {
        self.adjustments.clear();
        ensure_adjustment_channels(&mut self.adjustments, names);
    }

    pub fn ensure_snapshot_histories(&mut self) {
        for snapshot in &mut self.snapshots {
            if snapshot.history.entries.is_empty() {
                snapshot.history.entries.push(SnapshotHistoryState {
                    label: "Snapshot state".to_owned(),
                    adjustments: snapshot.adjustments.clone(),
                });
                snapshot.history.cursor = 0;
                continue;
            }
            if snapshot.history.entries.len() > MAX_SNAPSHOT_HISTORY_STATES {
                let overflow = snapshot.history.entries.len() - MAX_SNAPSHOT_HISTORY_STATES;
                snapshot.history.entries.drain(0..overflow);
                snapshot.history.cursor = snapshot.history.cursor.saturating_sub(overflow);
            }
            snapshot.history.cursor = snapshot
                .history
                .cursor
                .min(snapshot.history.entries.len().saturating_sub(1));
        }
    }

    fn next_snapshot_name(&self) -> String {
        let Some(first) = self.snapshots.first() else {
            return "Test 1".to_owned();
        };

        let (prefix, first_number) = snapshot_sequence_seed(&first.name);
        let mut next_number = first_number.saturating_add(1).max(1);
        for snapshot in &self.snapshots {
            if let Some(number) = snapshot_sequence_number(&snapshot.name, &prefix) {
                next_number = next_number.max(number.saturating_add(1));
            }
        }

        loop {
            let candidate = format!("{prefix}{next_number}");
            if self.snapshot_name_available(&candidate, None) {
                return candidate;
            }
            next_number = next_number.saturating_add(1);
        }
    }

    pub fn snapshot_name_available(&self, candidate: &str, except_id: Option<u64>) -> bool {
        let candidate = candidate.trim();
        !candidate.is_empty()
            && self.snapshots.iter().all(|snapshot| {
                except_id == Some(snapshot.id)
                    || !snapshot.name.trim().eq_ignore_ascii_case(candidate)
            })
    }

    pub fn create_snapshot(&mut self) -> u64 {
        let id = self.next_snapshot_id.max(1);
        self.next_snapshot_id = id.saturating_add(1);
        let name = self.next_snapshot_name();
        self.snapshots.push(AdjustmentSnapshot {
            id,
            name,
            created_at_unix_ms: now_unix_ms(),
            adjustments: self.adjustments.clone(),
            exports: Vec::new(),
            history: SnapshotAdjustmentHistory {
                entries: vec![SnapshotHistoryState {
                    label: "Snapshot created".to_owned(),
                    adjustments: self.adjustments.clone(),
                }],
                cursor: 0,
            },
        });
        self.active_snapshot_id = Some(id);
        id
    }

    pub fn rename_snapshot(&mut self, id: u64, candidate: &str) -> Result<bool, String> {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            return Err("Snapshot name cannot be empty.".to_owned());
        }
        if !self.snapshot_name_available(candidate, Some(id)) {
            return Err(format!("A snapshot named '{candidate}' already exists."));
        }
        let Some(snapshot) = self.snapshots.iter_mut().find(|snapshot| snapshot.id == id) else {
            return Err("Snapshot no longer exists.".to_owned());
        };
        if snapshot.name == candidate {
            return Ok(false);
        }
        snapshot.name = candidate.to_owned();
        Ok(true)
    }
    pub fn apply_snapshot(&mut self, id: u64) -> bool {
        let Some(snapshot) = self.snapshots.iter().find(|snapshot| snapshot.id == id) else {
            return false;
        };
        self.adjustments = snapshot.adjustments.clone();
        self.active_snapshot_id = Some(id);
        true
    }

    pub fn update_snapshot(&mut self, id: u64) -> bool {
        let Some(snapshot) = self.snapshots.iter_mut().find(|snapshot| snapshot.id == id) else {
            return false;
        };
        snapshot.adjustments = self.adjustments.clone();
        self.active_snapshot_id = Some(id);
        true
    }

    pub fn delete_snapshot(&mut self, id: u64) -> bool {
        let original_len = self.snapshots.len();
        self.snapshots.retain(|snapshot| snapshot.id != id);
        if self.active_snapshot_id == Some(id) {
            self.active_snapshot_id = None;
        }
        self.snapshots.len() != original_len
    }

    pub fn active_snapshot_matches(&self) -> bool {
        let Some(id) = self.active_snapshot_id else {
            return false;
        };
        self.snapshots
            .iter()
            .find(|snapshot| snapshot.id == id)
            .map(|snapshot| snapshot.adjustments == self.adjustments)
            .unwrap_or(false)
    }

    pub fn active_snapshot_name(&self) -> Option<&str> {
        let id = self.active_snapshot_id?;
        self.snapshots
            .iter()
            .find(|snapshot| snapshot.id == id)
            .map(|snapshot| snapshot.name.as_str())
    }

    pub fn effective_test_code_text(&self) -> String {
        let explicit = self.test_code.text.trim();
        if !explicit.is_empty() {
            return explicit.to_owned();
        }
        self.active_snapshot_name()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or("Test")
            .to_owned()
    }

    pub fn record_snapshot_export(
        &mut self,
        id: u64,
        face_key: String,
        folder: String,
        exported_at_unix_ms: i64,
    ) -> bool {
        self.record_snapshot_export_with_identity(
            id,
            face_key,
            folder,
            exported_at_unix_ms,
            String::new(),
            String::new(),
            String::new(),
        )
    }

    /// Append a committed Snapshot export to durable history. Do not replace
    /// older records: previously used codes/states remain auditable.
    pub fn record_snapshot_export_with_identity(
        &mut self,
        id: u64,
        face_key: String,
        folder: String,
        exported_at_unix_ms: i64,
        test_code: String,
        adjustment_sha256: String,
        destination: String,
    ) -> bool {
        let Some(snapshot) = self.snapshots.iter_mut().find(|snapshot| snapshot.id == id) else {
            return false;
        };
        snapshot.exports.push(SnapshotExportRecord {
            face_key,
            folder,
            exported_at_unix_ms,
            test_code,
            adjustment_sha256,
            destination,
        });
        true
    }

    pub fn snapshot_export_for_face(
        &self,
        id: u64,
        face_key: &str,
    ) -> Option<&SnapshotExportRecord> {
        self.snapshots
            .iter()
            .find(|snapshot| snapshot.id == id)?
            .exports
            .iter()
            .filter(|record| record.face_key == face_key)
            .max_by_key(|record| record.exported_at_unix_ms)
    }
}

fn snapshot_sequence_seed(name: &str) -> (String, u64) {
    let trimmed = name.trim();
    let split = trimmed
        .char_indices()
        .rev()
        .find(|(_, ch)| !ch.is_ascii_digit())
        .map(|(index, ch)| index + ch.len_utf8())
        .unwrap_or(0);
    let (prefix, digits) = trimmed.split_at(split);
    if !digits.is_empty() {
        if let Ok(number) = digits.parse::<u64>() {
            return (prefix.to_owned(), number);
        }
    }
    (format!("{}-", trimmed.trim_end_matches('-')), 1)
}

fn snapshot_sequence_number(name: &str, prefix: &str) -> Option<u64> {
    let trimmed = name.trim();
    let head = trimmed.get(..prefix.len())?;
    if !head.eq_ignore_ascii_case(prefix) {
        return None;
    }
    trimmed.get(prefix.len()..)?.parse::<u64>().ok()
}

fn ensure_adjustment_channels(
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    names: &[String],
) {
    for name in names {
        adjustments.entry(name.clone()).or_default();
    }
    for output in names {
        let row = &mut adjustments.entry(output.clone()).or_default().mixer;
        if row.coefficients.is_empty() {
            for input in names {
                row.coefficients
                    .insert(input.clone(), if input == output { 1.0 } else { 0.0 });
            }
        } else {
            for input in names {
                row.coefficients
                    .entry(input.clone())
                    .or_insert(if input == output { 1.0 } else { 0.0 });
            }
        }
    }
}

fn make_portable_path(source: &Path, project_dir: &Path) -> String {
    if source.parent() == Some(project_dir) {
        return source
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
    }
    if let Ok(relative) = source.strip_prefix(project_dir) {
        return relative.to_string_lossy().into_owned();
    }
    source.to_string_lossy().into_owned()
}

pub fn apply_levels(value: f32, levels: Levels) -> f32 {
    let black = levels.input_black.clamp(0.0, 0.9999);
    let white = levels.input_white.clamp(black + 0.0001, 1.0);
    let gamma = levels.gamma.clamp(0.05, 10.0);
    let normalized = ((value - black) / (white - black)).clamp(0.0, 1.0);
    let gamma_corrected = normalized.powf(1.0 / gamma);
    let out_black = levels.output_black.clamp(0.0, 1.0);
    let out_white = levels.output_white.clamp(0.0, 1.0);
    out_black + gamma_corrected * (out_white - out_black)
}

pub fn levels_gamma_mid_output(levels: Levels) -> f32 {
    let out_black = levels.output_black.clamp(0.0, 1.0);
    let out_white = levels.output_white.clamp(0.0, 1.0);
    let gamma = levels.gamma.clamp(0.05, 10.0);
    out_black + 0.5_f32.powf(1.0 / gamma) * (out_white - out_black)
}

pub fn curve_mid_output(curve: Curve) -> f32 {
    curve.midpoint.clamp(0.0, 1.0)
}

pub fn curve_linear_output(value: f32, curve: Curve) -> f32 {
    let epsilon = 1.0 / 65_535.0;
    let x0 = curve.input_black.clamp(0.0, 1.0 - epsilon);
    let x2 = curve.input_white.clamp(x0 + epsilon, 1.0);
    let y0 = curve.black.clamp(0.0, 1.0);
    let y2 = curve.white.clamp(0.0, 1.0);
    let x = value.clamp(0.0, 1.0);
    if x <= x0 {
        return y0;
    }
    if x >= x2 {
        return y2;
    }
    let t = (x - x0) / (x2 - x0).max(epsilon);
    lerp(y0, y2, t).clamp(0.0, 1.0)
}

pub fn apply_curve(value: f32, curve: Curve) -> f32 {
    if !curve.midpoint_enabled {
        return curve_linear_output(value, curve);
    }

    let epsilon = 1.0 / 65_535.0;
    let x0 = curve.input_black.clamp(0.0, 1.0 - epsilon * 2.0);
    let x2 = curve.input_white.clamp(x0 + epsilon * 2.0, 1.0);
    let x1 = curve.midpoint_input.clamp(x0 + epsilon, x2 - epsilon);
    let y0 = curve.black.clamp(0.0, 1.0);
    let y1 = curve.midpoint.clamp(0.0, 1.0);
    let y2 = curve.white.clamp(0.0, 1.0);
    let x = value.clamp(0.0, 1.0);

    if x <= x0 {
        return y0;
    }
    if x >= x2 {
        return y2;
    }
    if x <= x1 {
        let t = (x - x0) / (x1 - x0).max(epsilon);
        lerp(y0, y1, t).clamp(0.0, 1.0)
    } else {
        let t = (x - x1) / (x2 - x1).max(epsilon);
        lerp(y1, y2, t).clamp(0.0, 1.0)
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_color_settings_default_to_embedded_non_destructive_preview() {
        let project = ShadeProject::default();
        assert!(project.preview_color.enabled);
        assert!(project.preview_color.assigned_profile_path.is_none());
        assert_eq!(
            project.preview_color.rendering_intent,
            PreviewRenderingIntent::Perceptual
        );
        assert!(!project.preview_color.black_point_compensation);
        assert!(!project.preview_color.soft_proof_enabled);
        assert!(project.preview_color.proof_profile_path.is_none());
        assert_eq!(
            project.preview_color.proofing_intent,
            PreviewRenderingIntent::RelativeColorimetric
        );

        let json = serde_json::to_string(&project).expect("serialize project");
        let restored: ShadeProject = serde_json::from_str(&json).expect("deserialize project");
        assert_eq!(restored.preview_color, project.preview_color);
    }

    fn channels() -> Vec<String> {
        ["Cyan", "Magenta", "Yellow", "Black", "purpol", "bgreen"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn identity_adjustments_are_identity() {
        for value in [0.0, 0.1, 0.5, 0.9, 1.0] {
            let leveled = apply_levels(value, Levels::default());
            let curved = apply_curve(leveled, Curve::default());
            assert!((value - curved).abs() < 0.0001);
        }
    }

    #[test]
    fn curve_middle_point_moves_in_both_input_and_output_axes() {
        let curve = Curve {
            midpoint_enabled: true,
            midpoint_input: 0.25,
            midpoint: 0.70,
            ..Curve::default()
        };
        assert!((apply_curve(0.0, curve) - 0.0).abs() < 0.0001);
        assert!((apply_curve(0.25, curve) - 0.70).abs() < 0.0001);
        assert!((apply_curve(1.0, curve) - 1.0).abs() < 0.0001);
        assert!((apply_curve(0.125, curve) - 0.35).abs() < 0.0001);
    }

    #[test]
    fn three_collinear_points_preserve_a_straight_curve() {
        let curve = Curve {
            midpoint_enabled: true,
            midpoint_input: 0.30,
            midpoint: 0.30,
            ..Curve::default()
        };
        for value in [0.0, 0.1, 0.3, 0.6, 1.0] {
            assert!((apply_curve(value, curve) - value).abs() < 0.0001);
        }
    }

    #[test]
    fn curve_input_endpoints_define_the_active_input_range() {
        let curve = Curve {
            input_black: 0.2,
            input_white: 0.8,
            ..Curve::default()
        };
        assert!((apply_curve(0.2, curve) - 0.0).abs() < 0.0001);
        assert!((apply_curve(0.5, curve) - 0.5).abs() < 0.0001);
        assert!((apply_curve(0.8, curve) - 1.0).abs() < 0.0001);
        assert!((apply_curve(0.0, curve) - 0.0).abs() < 0.0001);
        assert!((apply_curve(1.0, curve) - 1.0).abs() < 0.0001);
    }

    #[test]
    fn midpoint_is_disabled_by_default_and_two_points_are_linear() {
        let curve = Curve::default();
        assert!(!curve.midpoint_enabled);
        for value in [0.0, 0.1, 0.5, 0.9, 1.0] {
            assert!((apply_curve(value, curve) - value).abs() < 0.0001);
        }
    }

    #[test]
    fn enabled_midpoint_changes_the_piecewise_curve() {
        let curve = Curve {
            midpoint_enabled: true,
            midpoint_input: 0.5,
            midpoint: 0.8,
            ..Curve::default()
        };
        assert!((apply_curve(0.5, curve) - 0.8).abs() < 0.0001);
    }

    #[test]
    fn snapshot_names_follow_first_snapshot_trailing_number() {
        let mut project = ShadeProject::default();
        let first = project.create_snapshot();
        project.rename_snapshot(first, "XN-A1-1").unwrap();
        let second = project.create_snapshot();
        let third = project.create_snapshot();
        assert_eq!(
            project
                .snapshots
                .iter()
                .find(|s| s.id == second)
                .unwrap()
                .name,
            "XN-A1-2"
        );
        assert_eq!(
            project
                .snapshots
                .iter()
                .find(|s| s.id == third)
                .unwrap()
                .name,
            "XN-A1-3"
        );
    }

    #[test]
    fn snapshot_names_append_sequence_when_seed_has_no_number() {
        let mut project = ShadeProject::default();
        let first = project.create_snapshot();
        project.rename_snapshot(first, "Kiln-Test").unwrap();
        let second = project.create_snapshot();
        assert_eq!(
            project
                .snapshots
                .iter()
                .find(|s| s.id == second)
                .unwrap()
                .name,
            "Kiln-Test-2"
        );
    }

    #[test]
    fn levels_gamma_is_relative_to_output_range() {
        let levels = Levels {
            output_black: 0.2,
            output_white: 0.8,
            ..Levels::default()
        };
        assert!((apply_levels(0.5, levels) - 0.5).abs() < 0.0001);
        assert!((levels_gamma_mid_output(levels) - 0.5).abs() < 0.0001);

        let lowered_white = Levels {
            output_white: 0.6,
            ..levels
        };
        assert!((apply_levels(0.5, lowered_white) - 0.4).abs() < 0.0001);
        assert!((levels_gamma_mid_output(lowered_white) - 0.4).abs() < 0.0001);
    }

    #[test]
    fn reset_restores_identity_for_every_channel() {
        let names = channels();
        let mut project = ShadeProject::default();
        project.ensure_channels(&names);
        project.adjustments.get_mut("purpol").unwrap().levels.gamma = 1.8;
        project.adjustments.get_mut("Cyan").unwrap().mixer.constant = 0.2;
        project
            .adjustments
            .entry(MASTER_ADJUSTMENT_KEY.to_owned())
            .or_default()
            .levels
            .gamma = 1.2;
        project.reset_adjustments(&names);
        assert!(!project.adjustments.contains_key(MASTER_ADJUSTMENT_KEY));

        for output in &names {
            let adjustment = project.adjustments.get(output).unwrap();
            assert_eq!(adjustment.levels, Levels::default());
            assert_eq!(adjustment.curve, Curve::default());
            assert_eq!(adjustment.mixer.constant, 0.0);
            for input in &names {
                let expected = if input == output { 1.0 } else { 0.0 };
                assert_eq!(
                    adjustment.mixer.coefficients.get(input).copied(),
                    Some(expected)
                );
            }
        }
    }

    #[test]
    fn snapshots_capture_switch_update_and_delete_adjustments() {
        let names = channels();
        let mut project = ShadeProject::default();
        project.ensure_channels(&names);
        project.adjustments.get_mut("purpol").unwrap().levels.gamma = 1.4;
        project
            .adjustments
            .entry(MASTER_ADJUSTMENT_KEY.to_owned())
            .or_default()
            .curve
            .midpoint = 0.62;
        let first = project.create_snapshot();
        assert!(project.active_snapshot_matches());
        project.adjustments.get_mut("purpol").unwrap().levels.gamma = 2.0;
        let second = project.create_snapshot();
        assert!(project.apply_snapshot(first));
        assert_eq!(project.adjustments.get("purpol").unwrap().levels.gamma, 1.4);
        assert_eq!(
            project
                .adjustments
                .get(MASTER_ADJUSTMENT_KEY)
                .unwrap()
                .curve
                .midpoint,
            0.62
        );
        project.adjustments.get_mut("purpol").unwrap().levels.gamma = 1.6;
        assert!(project.update_snapshot(first));
        assert!(project.apply_snapshot(second));
        assert_eq!(project.adjustments.get("purpol").unwrap().levels.gamma, 2.0);
        assert!(project.delete_snapshot(second));
    }

    #[test]
    fn blank_test_code_uses_snapshot_name() {
        let mut project = ShadeProject::default();
        project.create_snapshot();
        project.snapshots[0].name = "Moonstone T-07".to_owned();
        assert_eq!(project.effective_test_code_text(), "Moonstone T-07");
        project.test_code.text = "Manual".to_owned();
        assert_eq!(project.effective_test_code_text(), "Manual");
    }
    #[test]
    fn snapshot_export_history_preserves_prior_codes_and_returns_latest_per_face() {
        let mut project = ShadeProject::default();
        let id = project.create_snapshot();
        assert!(project.record_snapshot_export_with_identity(
            id,
            "face-a.tif".to_owned(),
            r"C:\exports\one".to_owned(),
            100,
            "TEST-41".to_owned(),
            "a".repeat(64),
            r"C:\exports\one\TEST-41.tif".to_owned(),
        ));
        let first = project
            .snapshot_export_for_face(id, "face-a.tif")
            .expect("first committed export");
        assert_eq!(first.test_code, "TEST-41");
        assert_eq!(first.adjustment_sha256, "a".repeat(64));
        assert_eq!(first.destination, r"C:\exports\one\TEST-41.tif");

        assert!(project.record_snapshot_export_with_identity(
            id,
            "face-a.tif".to_owned(),
            r"C:\exports\two".to_owned(),
            200,
            "TEST-42".to_owned(),
            "b".repeat(64),
            r"C:\exports\two\TEST-42.tif".to_owned(),
        ));

        let snapshot = project
            .snapshots
            .iter()
            .find(|snapshot| snapshot.id == id)
            .expect("snapshot");
        assert_eq!(snapshot.exports.len(), 2);
        assert_eq!(snapshot.exports[0].test_code, "TEST-41");
        assert_eq!(snapshot.exports[1].test_code, "TEST-42");

        let latest = project
            .snapshot_export_for_face(id, "face-a.tif")
            .expect("latest committed export");
        assert_eq!(latest.folder, r"C:\exports\two");
        assert_eq!(latest.exported_at_unix_ms, 200);
        assert_eq!(latest.test_code, "TEST-42");
        assert_eq!(latest.adjustment_sha256, "b".repeat(64));
        assert_eq!(latest.destination, r"C:\exports\two\TEST-42.tif");
    }

    #[test]
    fn snapshot_names_are_unique_and_rename_rejects_duplicates() {
        let mut project = ShadeProject::default();
        let first = project.create_snapshot();
        let second = project.create_snapshot();
        assert_eq!(
            project
                .snapshots
                .iter()
                .find(|item| item.id == first)
                .unwrap()
                .name,
            "Test 1"
        );
        assert_eq!(
            project
                .snapshots
                .iter()
                .find(|item| item.id == second)
                .unwrap()
                .name,
            "Test 2"
        );
        assert!(project.rename_snapshot(second, " test 1 ").is_err());
        assert!(project.rename_snapshot(second, "Reference").unwrap());
        assert!(project.snapshot_name_available("Test 2", None));
        assert!(
            project
                .snapshots
                .iter()
                .all(|item| item.created_at_unix_ms > 0)
        );
    }
    #[test]
    fn loader_rejects_old_schema_without_migration() {
        let mut project = ShadeProject::default();
        project.schema_version = 8;
        let path = std::env::temp_dir().join(format!(
            "shade-editor-old-schema-{}-{}.shade",
            std::process::id(),
            now_unix_ms()
        ));
        fs::write(&path, serde_json::to_string(&project).unwrap()).unwrap();
        let error = ShadeProject::load(&path).unwrap_err();
        let _ = fs::remove_file(&path);
        assert!(error.contains("accepts schema 9 only"));
    }
}

#[cfg(test)]
mod face_status_tests {
    use super::*;

    #[test]
    fn legacy_face_without_status_defaults_to_accepted() {
        let face: FaceRef = serde_json::from_str(r#"{"path":"face.tif","label":"Face 1"}"#)
            .expect("legacy FaceRef should deserialize");
        assert_eq!(face.status, FaceStatus::Accepted);
        assert!(face.production_source_profile.is_none());
    }

    #[test]
    fn rejected_status_round_trips() {
        let face = FaceRef {
            path: "face.tif".to_owned(),
            label: "Face 1".to_owned(),
            status: FaceStatus::Rejected,
            production_source_profile: None,
        };
        let json = serde_json::to_string(&face).unwrap();
        let loaded: FaceRef = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.status, FaceStatus::Rejected);
    }

    #[test]
    fn production_source_profile_assignment_round_trips_without_icc_bytes() {
        let face = FaceRef {
            path: "face.tif".to_owned(),
            label: "Face 1".to_owned(),
            status: FaceStatus::Accepted,
            production_source_profile: Some(ProductionSourceProfileAssignment {
                path: r"C:\Color\AdobeRGB.icc".to_owned(),
                identity: IccProfileIdentity {
                    description: "Adobe RGB (1998)".to_owned(),
                    sha256: "abc123".to_owned(),
                },
            }),
        };

        let json = serde_json::to_string(&face).unwrap();
        let loaded: FaceRef = serde_json::from_str(&json).unwrap();
        assert_eq!(
            loaded.production_source_profile,
            face.production_source_profile
        );
        assert!(!json.contains("payload"));
    }
}
