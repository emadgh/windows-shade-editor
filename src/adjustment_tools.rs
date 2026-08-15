use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{ChannelAdjustment, Curve, Levels, MixerRow};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardPart {
    All,
    Levels,
    Curve,
    Mixer,
}

#[derive(Clone, Debug)]
pub enum AdjustmentClipboard {
    All(ChannelAdjustment),
    Levels(Levels),
    Curve(Curve),
    Mixer(MixerRow),
}

impl AdjustmentClipboard {
    pub fn capture(adjustment: &ChannelAdjustment, part: ClipboardPart) -> Self {
        match part {
            ClipboardPart::All => Self::All(adjustment.clone()),
            ClipboardPart::Levels => Self::Levels(adjustment.levels),
            ClipboardPart::Curve => Self::Curve(adjustment.curve),
            ClipboardPart::Mixer => Self::Mixer(adjustment.mixer.clone()),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::All(_) => "All adjustments",
            Self::Levels(_) => "Levels",
            Self::Curve(_) => "Curve",
            Self::Mixer(_) => "Mixer",
        }
    }

    pub fn is_mixer_only(&self) -> bool {
        matches!(self, Self::Mixer(_))
    }

    pub fn paste_into(&self, target: &mut ChannelAdjustment, allow_mixer: bool) -> bool {
        let before = target.clone();
        match self {
            Self::All(source) => {
                target.enabled = source.enabled;
                target.levels = source.levels;
                target.curve = source.curve;
                if allow_mixer {
                    target.mixer = source.mixer.clone();
                }
            }
            Self::Levels(value) => target.levels = *value,
            Self::Curve(value) => target.curve = *value,
            Self::Mixer(value) if allow_mixer => target.mixer = value.clone(),
            Self::Mixer(_) => {}
        }
        *target != before
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RelativePreset {
    pub name: String,
    /// Exact project channel name -> relative percent change of that output's
    /// diagonal mixer coefficient. +2 means multiply current value by 1.02.
    pub channel_percent: BTreeMap<String, f32>,
}

#[derive(Clone, Debug, Default)]
pub struct RelativePresetDraft {
    pub name: String,
    pub channel_percent: BTreeMap<String, f32>,
}

#[derive(Clone, Copy, Debug)]
pub struct BuiltinRelativePreset {
    pub id: &'static str,
    pub label: &'static str,
}

pub const BUILTIN_RELATIVE_PRESETS: [BuiltinRelativePreset; 6] = [
    BuiltinRelativePreset {
        id: "warmer",
        label: "Warmer",
    },
    BuiltinRelativePreset {
        id: "cooler",
        label: "Cooler",
    },
    BuiltinRelativePreset {
        id: "richer",
        label: "Darker / Richer",
    },
    BuiltinRelativePreset {
        id: "lighter",
        label: "Lighter",
    },
    BuiltinRelativePreset {
        id: "redder",
        label: "Redder",
    },
    BuiltinRelativePreset {
        id: "beiger",
        label: "More beige",
    },
];

fn normalized_channel(actual: &str, display: &str) -> String {
    format!("{} {}", actual.trim(), display.trim()).to_ascii_lowercase()
}

fn contains_role(value: &str, names: &[&str]) -> bool {
    names.iter().any(|name| {
        value == *name
            || value
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|part| part == *name)
    })
}

fn builtin_delta(id: &str, actual: &str, display: &str) -> f32 {
    let name = normalized_channel(actual, display);
    let cyan_blue = contains_role(&name, &["cyan", "blue", "c"]);
    let magenta_red = contains_role(&name, &["magenta", "red", "m"]);
    let yellow = contains_role(&name, &["yellow", "y"]);
    let beige_brown = contains_role(&name, &["beige", "brown"]);
    let black = contains_role(&name, &["black", "key", "k"]);
    match id {
        "warmer" if yellow || beige_brown => 2.0,
        "warmer" if magenta_red => 1.0,
        "warmer" if cyan_blue => -2.0,
        "cooler" if yellow || beige_brown => -2.0,
        "cooler" if magenta_red => -1.0,
        "cooler" if cyan_blue => 2.0,
        "richer" => 2.0,
        "lighter" => -2.0,
        "redder" if magenta_red => 2.0,
        "redder" if cyan_blue => -1.0,
        "beiger" if yellow || beige_brown => 2.0,
        "beiger" if magenta_red => 1.0,
        "beiger" if cyan_blue => -1.5,
        "beiger" if black => -0.5,
        _ => 0.0,
    }
}

fn apply_percent(
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    channel: &str,
    percent: f32,
) -> bool {
    if !percent.is_finite() || percent.abs() < f32::EPSILON {
        return false;
    }
    let adjustment = adjustments.entry(channel.to_owned()).or_default();
    let coefficient = adjustment
        .mixer
        .coefficients
        .entry(channel.to_owned())
        .or_insert(1.0);
    let before = *coefficient;
    *coefficient = (before * (1.0 + percent.clamp(-25.0, 25.0) / 100.0)).clamp(-2.0, 2.0);
    (*coefficient - before).abs() > f32::EPSILON
}

pub fn apply_builtin(
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    channel_names: &[String],
    display_names: &[String],
    id: &str,
) -> bool {
    let mut changed = false;
    for (index, channel) in channel_names.iter().enumerate() {
        let display = display_names
            .get(index)
            .map(String::as_str)
            .unwrap_or(channel);
        changed |= apply_percent(adjustments, channel, builtin_delta(id, channel, display));
    }
    changed
}

pub fn apply_custom(
    adjustments: &mut BTreeMap<String, ChannelAdjustment>,
    channel_names: &[String],
    preset: &RelativePreset,
) -> bool {
    let mut changed = false;
    for channel in channel_names {
        if let Some(percent) = preset.channel_percent.get(channel) {
            changed |= apply_percent(adjustments, channel, *percent);
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warmer_is_relative_and_accumulates_without_overwriting_other_mix_values() {
        let channels = vec!["Blue".to_owned(), "Yellow".to_owned()];
        let display = channels.clone();
        let mut adjustments = BTreeMap::new();
        let blue = adjustments
            .entry("Blue".to_owned())
            .or_insert_with(ChannelAdjustment::default);
        blue.mixer.coefficients.insert("Blue".to_owned(), 0.90);
        blue.mixer.coefficients.insert("Yellow".to_owned(), 0.15);
        assert!(apply_builtin(
            &mut adjustments,
            &channels,
            &display,
            "warmer"
        ));
        let blue = &adjustments["Blue"].mixer.coefficients;
        assert!((blue["Blue"] - 0.882).abs() < 0.0001);
        assert!((blue["Yellow"] - 0.15).abs() < 0.0001);
        assert!((adjustments["Yellow"].mixer.coefficients["Yellow"] - 1.02).abs() < 0.0001);
        apply_builtin(&mut adjustments, &channels, &display, "warmer");
        assert!(adjustments["Blue"].mixer.coefficients["Blue"] < 0.882);
    }

    #[test]
    fn clipboard_mixer_is_blocked_for_master_but_levels_are_allowed() {
        let mut source = ChannelAdjustment::default();
        source.levels.gamma = 1.25;
        source.mixer.coefficients.insert("Cyan".to_owned(), 0.8);
        let mixer = AdjustmentClipboard::capture(&source, ClipboardPart::Mixer);
        let mut target = ChannelAdjustment::default();
        assert!(!mixer.paste_into(&mut target, false));
        let levels = AdjustmentClipboard::capture(&source, ClipboardPart::Levels);
        assert!(levels.paste_into(&mut target, false));
        assert_eq!(target.levels.gamma, 1.25);
    }
}
