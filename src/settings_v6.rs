use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::export_batch::{ConflictPolicy, DEFAULT_EXPORT_TEMPLATE};
use crate::palette::{
    AUTO_PALETTE_ID, ChannelPalette, ChannelPaletteEntry, builtin_palettes, is_builtin_id,
};

pub const DEFAULT_DPI: f64 = 220.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub auto_update: bool,
    pub dark_mode: bool,
    pub max_preview_dimension: u32,
    pub adjustment_tabs: bool,
    pub show_all_histograms: bool,
    pub sidebar_two_columns: bool,
    pub colorize_histograms: bool,
    pub colorize_adjustments: bool,
    pub show_curve_histogram: bool,
    pub compact_curve_controls: bool,
    pub validate_after_export: bool,
    pub export_all_test_code: bool,
    pub export_all_template: String,
    pub export_all_conflict_policy: ConflictPolicy,
    pub export_all_open_folder: bool,
    pub lzw_compression: bool,
    pub default_dpi: f64,
    pub default_palette_id: String,
    pub custom_palettes: Vec<ChannelPalette>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            auto_update: true,
            dark_mode: true,
            max_preview_dimension: 1800,
            adjustment_tabs: false,
            show_all_histograms: false,
            sidebar_two_columns: false,
            colorize_histograms: true,
            colorize_adjustments: true,
            show_curve_histogram: true,
            compact_curve_controls: false,
            validate_after_export: false,
            export_all_test_code: false,
            export_all_template: DEFAULT_EXPORT_TEMPLATE.to_owned(),
            export_all_conflict_policy: ConflictPolicy::AutoNumber,
            export_all_open_folder: false,
            lzw_compression: true,
            default_dpi: DEFAULT_DPI,
            default_palette_id: AUTO_PALETTE_ID.to_owned(),
            custom_palettes: Vec::new(),
        }
    }
}

impl AppSettings {
    pub fn load() -> Self {
        let path = settings_path();
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        let mut settings: Self = serde_json::from_str(&text).unwrap_or_default();
        settings.sanitize();
        settings
    }

    pub fn save(&self) -> Result<(), String> {
        let path = settings_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("Cannot create settings directory: {err}"))?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| format!("Cannot serialize settings: {err}"))?;
        fs::write(path, text).map_err(|err| format!("Cannot save settings: {err}"))
    }

    pub fn sanitize(&mut self) {
        if !self.default_dpi.is_finite() {
            self.default_dpi = DEFAULT_DPI;
        }
        self.default_dpi = self.default_dpi.clamp(36.0, 2400.0);
        if self.export_all_template.trim().is_empty() {
            self.export_all_template = DEFAULT_EXPORT_TEMPLATE.to_owned();
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
    fn compact_curve_controls_default_off() {
        assert!(!AppSettings::default().compact_curve_controls);
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
    fn export_all_test_code_defaults_off() {
        assert!(!AppSettings::default().export_all_test_code);
    }

    #[test]
    fn export_all_defaults_are_safe() {
        let settings = AppSettings::default();
        assert_eq!(settings.export_all_template, DEFAULT_EXPORT_TEMPLATE);
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
