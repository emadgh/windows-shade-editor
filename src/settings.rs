use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub auto_update: bool,
    pub dark_mode: bool,
    pub max_preview_dimension: u32,
    /// false = show Levels, Curve and Mixer vertically; true = compact tabs.
    pub adjustment_tabs: bool,
    /// false = selected channel histogram; true = one histogram per channel.
    pub show_all_histograms: bool,
    /// false = one tools column; true = channels/histogram beside adjustments/test code.
    pub sidebar_two_columns: bool,
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
        }
    }
}

impl AppSettings {
    pub fn load() -> Self {
        let path = settings_path();
        let Ok(text) = fs::read_to_string(path) else { return Self::default(); };
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = settings_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("Cannot create settings directory: {err}"))?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| format!("Cannot serialize settings: {err}"))?;
        fs::write(path, text).map_err(|err| format!("Cannot save settings: {err}"))
    }
}

pub fn settings_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("ShadeEditor").join("settings.json")
}
