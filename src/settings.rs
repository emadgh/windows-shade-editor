use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

use crate::adjustment_tools::RelativePreset;
use crate::export_batch::{ConflictPolicy, DEFAULT_EXPORT_TEMPLATE, DEFAULT_FOLDER_TEMPLATE};
use crate::model::{
    DEFAULT_HISTORY_STEPS, IccProfileIdentity, MAX_SNAPSHOT_HISTORY_STATES, MIN_HISTORY_STEPS,
};
use crate::palette::{
    AUTO_PALETTE_ID, ChannelPalette, ChannelPaletteEntry, builtin_palettes, is_builtin_id,
};

pub const DEFAULT_DPI: f64 = 220.0;
pub const DEFAULT_ORIGINAL_HISTOGRAM_OPACITY: f32 = 0.32;
pub const DEFAULT_ORIGINAL_HISTOGRAM_PROMINENCE: f32 = 0.72;

static RUNTIME_ORIGINAL_HISTOGRAM_OPACITY_BITS: AtomicU32 =
    AtomicU32::new(DEFAULT_ORIGINAL_HISTOGRAM_OPACITY.to_bits());
static RUNTIME_ORIGINAL_HISTOGRAM_PROMINENCE_BITS: AtomicU32 =
    AtomicU32::new(DEFAULT_ORIGINAL_HISTOGRAM_PROMINENCE.to_bits());
static RUNTIME_CURVE_VALUE_DISPLAY_UNIT: AtomicU8 = AtomicU8::new(0);

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TonalDisplayMode {
    Light,
    Pigment,
}

impl Default for TonalDisplayMode {
    fn default() -> Self {
        Self::Light
    }
}

impl TonalDisplayMode {
    pub fn label(self) -> &'static str {
        // v0.18.4 shipped the two presentation names reversed. Keep the enum
        // variants stable for settings compatibility and correct only the UI label.
        match self {
            Self::Light => "Pigment",
            Self::Pigment => "Light",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Light => Self::Pigment,
            Self::Pigment => Self::Light,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum CurveValueDisplayUnit {
    Byte255,
    Percent,
}

impl Default for CurveValueDisplayUnit {
    fn default() -> Self {
        Self::Byte255
    }
}

impl CurveValueDisplayUnit {
    pub fn label(self) -> &'static str {
        match self {
            Self::Byte255 => "0–255",
            Self::Percent => "0–100%",
        }
    }

    fn runtime_code(self) -> u8 {
        match self {
            Self::Byte255 => 0,
            Self::Percent => 1,
        }
    }

    fn from_runtime_code(value: u8) -> Self {
        if value == 1 {
            Self::Percent
        } else {
            Self::Byte255
        }
    }
}

pub fn runtime_histogram_overlay_preferences() -> (f32, f32) {
    (
        f32::from_bits(RUNTIME_ORIGINAL_HISTOGRAM_OPACITY_BITS.load(Ordering::Relaxed))
            .clamp(0.0, 1.0),
        f32::from_bits(RUNTIME_ORIGINAL_HISTOGRAM_PROMINENCE_BITS.load(Ordering::Relaxed))
            .clamp(0.0, 1.0),
    )
}

pub fn runtime_curve_value_display_unit() -> CurveValueDisplayUnit {
    CurveValueDisplayUnit::from_runtime_code(
        RUNTIME_CURVE_VALUE_DISPLAY_UNIT.load(Ordering::Relaxed),
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub auto_update: bool,
    pub dark_mode: bool,
    pub max_preview_dimension: u32,
    pub history_steps: usize,
    pub show_clipping_warnings: bool,
    pub adjustment_tabs: bool,
    pub show_all_histograms: bool,
    pub sidebar_two_columns: bool,
    pub colorize_histograms: bool,
    pub colorize_adjustments: bool,
    pub show_curve_histogram: bool,
    pub compact_curve_controls: bool,
    pub original_histogram_opacity: f32,
    pub original_histogram_prominence: f32,
    pub curve_value_display_unit: CurveValueDisplayUnit,
    pub tonal_display_mode: TonalDisplayMode,
    pub validate_after_export: bool,
    pub export_all_test_code: bool,
    pub export_all_template: String,
    pub snapshot_export_template: String,
    pub export_folder_template: String,
    pub export_all_conflict_policy: ConflictPolicy,
    pub export_all_open_folder: bool,
    pub lzw_compression: bool,
    pub monitor_profile_path: Option<String>,
    pub monitor_profile_identity: Option<IccProfileIdentity>,
    pub gamut_warning: bool,
    pub default_dpi: f64,
    pub default_palette_id: String,
    pub custom_palettes: Vec<ChannelPalette>,
    pub relative_adjustment_presets: Vec<RelativePreset>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            auto_update: true,
            dark_mode: true,
            max_preview_dimension: 1800,
            history_steps: DEFAULT_HISTORY_STEPS,
            show_clipping_warnings: true,
            adjustment_tabs: false,
            show_all_histograms: false,
            sidebar_two_columns: false,
            colorize_histograms: true,
            colorize_adjustments: true,
            show_curve_histogram: true,
            compact_curve_controls: false,
            original_histogram_opacity: DEFAULT_ORIGINAL_HISTOGRAM_OPACITY,
            original_histogram_prominence: DEFAULT_ORIGINAL_HISTOGRAM_PROMINENCE,
            curve_value_display_unit: CurveValueDisplayUnit::Byte255,
            tonal_display_mode: TonalDisplayMode::Light,
            validate_after_export: false,
            export_all_test_code: false,
            export_all_template: DEFAULT_EXPORT_TEMPLATE.to_owned(),
            snapshot_export_template: DEFAULT_EXPORT_TEMPLATE.to_owned(),
            export_folder_template: DEFAULT_FOLDER_TEMPLATE.to_owned(),
            export_all_conflict_policy: ConflictPolicy::AutoNumber,
            export_all_open_folder: false,
            lzw_compression: true,
            monitor_profile_path: None,
            monitor_profile_identity: None,
            gamut_warning: false,
            default_dpi: DEFAULT_DPI,
            default_palette_id: AUTO_PALETTE_ID.to_owned(),
            custom_palettes: Vec::new(),
            relative_adjustment_presets: Vec::new(),
        }
    }
}

impl AppSettings {
    pub fn load() -> Self {
        let path = settings_path();
        let mut settings = match fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        };
        settings.sanitize();
        settings.sync_runtime_display_preferences();
        settings
    }

    pub fn save(&self) -> Result<(), String> {
        self.sync_runtime_display_preferences();
        let path = settings_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("Cannot create settings directory: {err}"))?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| format!("Cannot serialize settings: {err}"))?;
        fs::write(path, text).map_err(|err| format!("Cannot save settings: {err}"))
    }

    fn sync_runtime_display_preferences(&self) {
        RUNTIME_ORIGINAL_HISTOGRAM_OPACITY_BITS.store(
            self.original_histogram_opacity.clamp(0.0, 1.0).to_bits(),
            Ordering::Relaxed,
        );
        RUNTIME_ORIGINAL_HISTOGRAM_PROMINENCE_BITS.store(
            self.original_histogram_prominence.clamp(0.0, 1.0).to_bits(),
            Ordering::Relaxed,
        );
        RUNTIME_CURVE_VALUE_DISPLAY_UNIT.store(
            self.curve_value_display_unit.runtime_code(),
            Ordering::Relaxed,
        );
    }

    pub fn sanitize(&mut self) {
        if !self.default_dpi.is_finite() {
            self.default_dpi = DEFAULT_DPI;
        }
        self.default_dpi = self.default_dpi.clamp(36.0, 2400.0);
        self.history_steps = self
            .history_steps
            .clamp(MIN_HISTORY_STEPS, MAX_SNAPSHOT_HISTORY_STATES);
        if !self.original_histogram_opacity.is_finite() {
            self.original_histogram_opacity = DEFAULT_ORIGINAL_HISTOGRAM_OPACITY;
        }
        self.original_histogram_opacity = self.original_histogram_opacity.clamp(0.0, 1.0);
        if !self.original_histogram_prominence.is_finite() {
            self.original_histogram_prominence = DEFAULT_ORIGINAL_HISTOGRAM_PROMINENCE;
        }
        self.original_histogram_prominence = self.original_histogram_prominence.clamp(0.0, 1.0);
        if self.export_all_template.trim().is_empty() {
            self.export_all_template = DEFAULT_EXPORT_TEMPLATE.to_owned();
        }
        if self.snapshot_export_template.trim().is_empty() {
            self.snapshot_export_template = DEFAULT_EXPORT_TEMPLATE.to_owned();
        }
        if self
            .monitor_profile_path
            .as_ref()
            .is_some_and(|path| path.trim().is_empty())
        {
            self.monitor_profile_path = None;
            self.monitor_profile_identity = None;
        }

        let mut used_ids = HashSet::new();
        self.custom_palettes.retain_mut(|palette| {
            palette.id = palette.id.trim().to_owned();
            palette.name = palette.name.trim().to_owned();
            if palette.id.is_empty()
                || palette.name.is_empty()
                || is_builtin_id(&palette.id)
                || palette.id == AUTO_PALETTE_ID
                || !used_ids.insert(palette.id.clone())
            {
                return false;
            }
            palette.channels.retain_mut(|entry| {
                entry.name = entry.name.trim().to_owned();
                !entry.name.is_empty()
            });
            true
        });

        if self.default_palette_id != AUTO_PALETTE_ID
            && self.palette_by_id(&self.default_palette_id).is_none()
        {
            self.default_palette_id = AUTO_PALETTE_ID.to_owned();
        }

        let mut preset_names = HashSet::new();
        self.relative_adjustment_presets.retain_mut(|preset| {
            preset.name = preset.name.trim().to_owned();
            if preset.name.is_empty() || !preset_names.insert(preset.name.to_ascii_lowercase()) {
                return false;
            }
            preset
                .channel_percent
                .retain(|channel, value| !channel.trim().is_empty() && value.is_finite());
            for value in preset.channel_percent.values_mut() {
                *value = value.clamp(-25.0, 25.0);
            }
            true
        });
    }

    pub fn palette_library(&self) -> Vec<ChannelPalette> {
        let mut palettes = builtin_palettes();
        palettes.extend(self.custom_palettes.iter().cloned());
        palettes
    }

    pub fn palette_by_id(&self, id: &str) -> Option<ChannelPalette> {
        builtin_palettes()
            .into_iter()
            .find(|palette| palette.id == id)
            .or_else(|| {
                self.custom_palettes
                    .iter()
                    .find(|palette| palette.id == id)
                    .cloned()
            })
    }

    pub fn default_project_palette(&self) -> Option<ChannelPalette> {
        if self.default_palette_id == AUTO_PALETTE_ID {
            None
        } else {
            self.palette_by_id(&self.default_palette_id)
        }
    }

    pub fn create_custom_palette(&mut self) -> String {
        let mut number = 1usize;
        loop {
            let id = format!("custom:palette-{number}");
            if self.custom_palettes.iter().all(|palette| palette.id != id) {
                let name = format!("Custom Palette {number}");
                self.custom_palettes.push(ChannelPalette {
                    id: id.clone(),
                    name,
                    channels: vec![
                        ChannelPaletteEntry {
                            name: "Ink 1".to_owned(),
                            color: [0, 190, 220],
                        },
                        ChannelPaletteEntry {
                            name: "Ink 2".to_owned(),
                            color: [225, 45, 150],
                        },
                        ChannelPaletteEntry {
                            name: "Ink 3".to_owned(),
                            color: [225, 190, 20],
                        },
                        ChannelPaletteEntry {
                            name: "Ink 4".to_owned(),
                            color: [155, 155, 155],
                        },
                    ],
                });
                return id;
            }
            number += 1;
        }
    }

    pub fn delete_custom_palette(&mut self, id: &str) -> bool {
        let before = self.custom_palettes.len();
        self.custom_palettes.retain(|palette| palette.id != id);
        if self.default_palette_id == id {
            self.default_palette_id = AUTO_PALETTE_ID.to_owned();
        }
        self.custom_palettes.len() != before
    }
}

pub fn settings_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("ShadeEditor").join("settings.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_dpi_is_220() {
        assert_eq!(AppSettings::default().default_dpi, 220.0);
    }

    #[test]
    fn history_steps_default_to_fifty_and_sanitize_to_safe_bounds() {
        let mut settings = AppSettings::default();
        assert_eq!(settings.history_steps, DEFAULT_HISTORY_STEPS);
        settings.history_steps = 1;
        settings.sanitize();
        assert_eq!(settings.history_steps, MIN_HISTORY_STEPS);
        settings.history_steps = usize::MAX;
        settings.sanitize();
        assert_eq!(settings.history_steps, MAX_SNAPSHOT_HISTORY_STATES);
    }

    #[test]
    fn clipping_defaults_are_safe() {
        assert!(AppSettings::default().show_clipping_warnings);
    }

    #[test]
    fn compact_curve_controls_default_off() {
        assert!(!AppSettings::default().compact_curve_controls);
    }

    #[test]
    fn histogram_overlay_defaults_match_current_appearance_and_sanitize() {
        let settings = AppSettings::default();
        assert_eq!(
            settings.original_histogram_opacity,
            DEFAULT_ORIGINAL_HISTOGRAM_OPACITY
        );
        assert_eq!(
            settings.original_histogram_prominence,
            DEFAULT_ORIGINAL_HISTOGRAM_PROMINENCE
        );

        let mut settings = AppSettings::default();
        settings.original_histogram_opacity = 2.0;
        settings.original_histogram_prominence = -1.0;
        settings.sanitize();
        assert_eq!(settings.original_histogram_opacity, 1.0);
        assert_eq!(settings.original_histogram_prominence, 0.0);

        settings.original_histogram_opacity = f32::NAN;
        settings.original_histogram_prominence = f32::INFINITY;
        settings.sanitize();
        assert_eq!(
            settings.original_histogram_opacity,
            DEFAULT_ORIGINAL_HISTOGRAM_OPACITY
        );
        assert_eq!(
            settings.original_histogram_prominence,
            DEFAULT_ORIGINAL_HISTOGRAM_PROMINENCE
        );
    }

    #[test]
    fn curve_value_display_defaults_to_255_scale() {
        assert_eq!(
            AppSettings::default().curve_value_display_unit,
            CurveValueDisplayUnit::Byte255
        );
        assert_eq!(CurveValueDisplayUnit::Byte255.label(), "0–255");
        assert_eq!(CurveValueDisplayUnit::Percent.label(), "0–100%");
    }

    #[test]
    fn runtime_display_preferences_follow_saved_settings() {
        let mut settings = AppSettings::default();
        settings.original_histogram_opacity = 0.81;
        settings.original_histogram_prominence = 0.44;
        settings.curve_value_display_unit = CurveValueDisplayUnit::Percent;
        settings.sync_runtime_display_preferences();
        let (opacity, prominence) = runtime_histogram_overlay_preferences();
        assert!((opacity - 0.81).abs() < 1e-6);
        assert!((prominence - 0.44).abs() < 1e-6);
        assert_eq!(runtime_curve_value_display_unit(), CurveValueDisplayUnit::Percent);
    }

    #[test]
    fn tonal_display_labels_match_the_corrected_ui_names() {
        assert_eq!(TonalDisplayMode::Light.label(), "Pigment");
        assert_eq!(TonalDisplayMode::Pigment.label(), "Light");
        assert_eq!(TonalDisplayMode::Light.toggled(), TonalDisplayMode::Pigment);
    }

    #[test]
    fn tonal_display_defaults_to_light() {
        assert_eq!(
            AppSettings::default().tonal_display_mode,
            TonalDisplayMode::Light
        );
    }

    #[test]
    fn post_export_validation_defaults_off() {
        assert!(!AppSettings::default().validate_after_export);
    }

    #[test]
    fn lzw_compression_defaults_on() {
        assert!(AppSettings::default().lzw_compression);
    }

    #[test]
    fn monitor_color_management_defaults_are_non_intrusive() {
        let settings = AppSettings::default();
        assert!(settings.monitor_profile_path.is_none());
        assert!(settings.monitor_profile_identity.is_none());
        assert!(!settings.gamut_warning);
    }

    #[test]
    fn export_all_test_code_defaults_off() {
        assert!(!AppSettings::default().export_all_test_code);
    }

    #[test]
    fn export_all_defaults_are_safe() {
        let settings = AppSettings::default();
        assert_eq!(settings.export_all_template, DEFAULT_EXPORT_TEMPLATE);
        assert_eq!(settings.snapshot_export_template, DEFAULT_EXPORT_TEMPLATE);
        assert_eq!(settings.export_folder_template, DEFAULT_FOLDER_TEMPLATE);
        assert_eq!(
            settings.export_all_conflict_policy,
            ConflictPolicy::AutoNumber
        );
        assert!(!settings.export_all_open_folder);
    }

    #[test]
    fn builtins_are_always_available() {
        let settings = AppSettings::default();
        let library = settings.palette_library();
        assert!(library.iter().any(|palette| palette.id == "builtin:cmyk"));
        assert!(library.iter().any(|palette| palette.id == "builtin:rgb"));
    }

    #[test]
    fn custom_palette_ids_are_stable() {
        let mut settings = AppSettings::default();
        let first = settings.create_custom_palette();
        let second = settings.create_custom_palette();
        assert_ne!(first, second);
        assert!(settings.palette_by_id(&first).is_some());
    }
}
