use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const SHADE_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShadeProject {
    pub schema_version: u32,
    pub name: String,
    pub faces: Vec<FaceRef>,
    pub adjustments: BTreeMap<String, ChannelAdjustment>,
    #[serde(default)]
    pub snapshots: Vec<AdjustmentSnapshot>,
    #[serde(default)]
    pub active_snapshot_id: Option<u64>,
    #[serde(default = "default_next_snapshot_id")]
    pub next_snapshot_id: u64,
    pub test_code: TestCodeConfig,
}

impl Default for ShadeProject {
    fn default() -> Self {
        Self {
            schema_version: SHADE_SCHEMA_VERSION,
            name: "Untitled Shade".to_owned(),
            faces: Vec::new(),
            adjustments: BTreeMap::new(),
            snapshots: Vec::new(),
            active_snapshot_id: None,
            next_snapshot_id: default_next_snapshot_id(),
            test_code: TestCodeConfig::default(),
        }
    }
}

fn default_next_snapshot_id() -> u64 { 1 }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FaceRef {
    pub path: String,
    pub label: String,
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
pub struct Curve {
    /// Output at input 0.0.
    pub black: f32,
    /// Output at input 0.5.
    pub midpoint: f32,
    /// Output at input 1.0.
    pub white: f32,
}

impl Default for Curve {
    fn default() -> Self {
        Self {
            black: 0.0,
            midpoint: 0.5,
            white: 1.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct MixerRow {
    /// Source channel name -> coefficient. Missing entries are zero, except the
    /// output channel itself which is treated as 1.0 when the map is empty.
    pub coefficients: BTreeMap<String, f32>,
    pub constant: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdjustmentSnapshot {
    pub id: u64,
    pub name: String,
    pub adjustments: BTreeMap<String, ChannelAdjustment>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestCodeConfig {
    pub enabled: bool,
    pub text: String,
    pub channel: String,
    pub scale: u32,
    pub margin_px: u32,
}

impl Default for TestCodeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            text: String::new(),
            channel: "Black".to_owned(),
            scale: 3,
            margin_px: 24,
        }
    }
}

impl ShadeProject {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path)
            .map_err(|err| format!("Cannot read .shade file: {err}"))?;
        let mut project: Self = serde_json::from_str(&text)
            .map_err(|err| format!("Invalid .shade file: {err}"))?;
        if project.schema_version > SHADE_SCHEMA_VERSION {
            return Err(format!(
                "This project uses .shade schema {}, but this app supports up to {}.",
                project.schema_version, SHADE_SCHEMA_VERSION
            ));
        }
        project.schema_version = SHADE_SCHEMA_VERSION;
        let minimum_next_id = project.snapshots.iter().map(|snapshot| snapshot.id).max().unwrap_or(0) + 1;
        project.next_snapshot_id = project.next_snapshot_id.max(minimum_next_id).max(1);
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
        fs::write(path, text).map_err(|err| format!("Cannot save .shade file: {err}"))
    }

    pub fn resolve_face_paths(&self, shade_path: &Path) -> Vec<PathBuf> {
        let base = shade_path.parent().unwrap_or_else(|| Path::new("."));
        self.faces
            .iter()
            .map(|face| {
                let path = PathBuf::from(&face.path);
                if path.is_absolute() { path } else { base.join(path) }
            })
            .collect()
    }

    pub fn ensure_channels(&mut self, names: &[String]) {
        ensure_adjustment_channels(&mut self.adjustments, names);
        for snapshot in &mut self.snapshots {
            ensure_adjustment_channels(&mut snapshot.adjustments, names);
        }
        if !names.iter().any(|name| name == &self.test_code.channel) {
            self.test_code.channel = names.get(3).or_else(|| names.first()).cloned().unwrap_or_default();
        }
    }

    pub fn reset_adjustments(&mut self, names: &[String]) {
        self.adjustments.clear();
        ensure_adjustment_channels(&mut self.adjustments, names);
    }

    pub fn create_snapshot(&mut self) -> u64 {
        let id = self.next_snapshot_id.max(1);
        self.next_snapshot_id = id.saturating_add(1);
        let number = self.snapshots.len() + 1;
        self.snapshots.push(AdjustmentSnapshot {
            id,
            name: format!("Test {number}"),
            adjustments: self.adjustments.clone(),
        });
        self.active_snapshot_id = Some(id);
        id
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
        let Some(id) = self.active_snapshot_id else { return false; };
        self.snapshots
            .iter()
            .find(|snapshot| snapshot.id == id)
            .map(|snapshot| snapshot.adjustments == self.adjustments)
            .unwrap_or(false)
    }
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
                row.coefficients.insert(input.clone(), if input == output { 1.0 } else { 0.0 });
            }
        } else {
            for input in names {
                row.coefficients.entry(input.clone()).or_insert(if input == output { 1.0 } else { 0.0 });
            }
        }
    }
}

fn make_portable_path(source: &Path, project_dir: &Path) -> String {
    if source.parent() == Some(project_dir) {
        return source.file_name().unwrap_or_default().to_string_lossy().into_owned();
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
    let out_white = levels.output_white.clamp(out_black, 1.0);
    out_black + gamma_corrected * (out_white - out_black)
}

pub fn apply_curve(value: f32, curve: Curve) -> f32 {
    let x = value.clamp(0.0, 1.0);
    let y = if x <= 0.5 {
        lerp(curve.black, curve.midpoint, x * 2.0)
    } else {
        lerp(curve.midpoint, curve.white, (x - 0.5) * 2.0)
    };
    y.clamp(0.0, 1.0)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn curve_control_points_are_exact() {
        let curve = Curve { black: 0.1, midpoint: 0.7, white: 0.9 };
        assert!((apply_curve(0.0, curve) - 0.1).abs() < 0.0001);
        assert!((apply_curve(0.5, curve) - 0.7).abs() < 0.0001);
        assert!((apply_curve(1.0, curve) - 0.9).abs() < 0.0001);
    }

    #[test]
    fn reset_restores_identity_for_every_channel() {
        let names = channels();
        let mut project = ShadeProject::default();
        project.ensure_channels(&names);
        project.adjustments.get_mut("purpol").unwrap().levels.gamma = 1.8;
        project.adjustments.get_mut("Cyan").unwrap().mixer.constant = 0.2;
        project.reset_adjustments(&names);

        for output in &names {
            let adjustment = project.adjustments.get(output).unwrap();
            assert_eq!(adjustment.levels, Levels::default());
            assert_eq!(adjustment.curve, Curve::default());
            assert_eq!(adjustment.mixer.constant, 0.0);
            for input in &names {
                let expected = if input == output { 1.0 } else { 0.0 };
                assert_eq!(adjustment.mixer.coefficients.get(input).copied(), Some(expected));
            }
        }
    }

    #[test]
    fn snapshots_capture_switch_update_and_delete_adjustments() {
        let names = channels();
        let mut project = ShadeProject::default();
        project.ensure_channels(&names);

        project.adjustments.get_mut("purpol").unwrap().levels.gamma = 1.4;
        let first = project.create_snapshot();
        assert!(project.active_snapshot_matches());

        project.adjustments.get_mut("purpol").unwrap().levels.gamma = 2.0;
        assert!(!project.active_snapshot_matches());
        let second = project.create_snapshot();
        assert!(project.active_snapshot_matches());

        assert!(project.apply_snapshot(first));
        assert_eq!(project.adjustments.get("purpol").unwrap().levels.gamma, 1.4);
        project.adjustments.get_mut("purpol").unwrap().levels.gamma = 1.6;
        assert!(project.update_snapshot(first));
        assert!(project.apply_snapshot(second));
        assert_eq!(project.adjustments.get("purpol").unwrap().levels.gamma, 2.0);
        assert!(project.apply_snapshot(first));
        assert_eq!(project.adjustments.get("purpol").unwrap().levels.gamma, 1.6);

        assert!(project.delete_snapshot(second));
        assert!(project.snapshots.iter().all(|snapshot| snapshot.id != second));
    }
}
