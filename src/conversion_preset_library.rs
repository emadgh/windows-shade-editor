use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::color_conversion::ConversionEngineMode;
use crate::conversion_presets::{
    PresetOrigin, SEPARATION_PRESET_SCHEMA_VERSION, SeparationPresetDefinition,
};

pub const PRESET_LIBRARY_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SeparationPresetLibrary {
    pub schema_version: u32,
    pub presets: Vec<SeparationPresetDefinition>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresetLibraryError {
    UnsupportedLibrarySchema(u32),
    UnsupportedPresetSchema { id: String, schema_version: u32 },
    EmptyId,
    EmptyName { id: String },
    DuplicateId(String),
    UnknownId(String),
    BuiltInImmutable(String),
    EmptyTargetTopology { id: String },
    DuplicateTargetChannel { id: String, channel: String },
    TargetLimitTopologyMismatch { id: String },
    InvalidBitDepth { id: String, bit_depth: u8 },
    MissingTransformIdentity { id: String },
    MissingCharacterizationIdentity { id: String },
    InvalidChannelLimit { id: String, channel: String },
    InvalidTotalInkLimit { id: String },
    InvalidStrategy { id: String, reason: String },
    InvalidJson(String),
}

impl Default for SeparationPresetLibrary {
    fn default() -> Self {
        Self::new()
    }
}

impl SeparationPresetLibrary {
    pub fn new() -> Self {
        Self {
            schema_version: PRESET_LIBRARY_SCHEMA_VERSION,
            presets: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), PresetLibraryError> {
        if self.schema_version != PRESET_LIBRARY_SCHEMA_VERSION {
            return Err(PresetLibraryError::UnsupportedLibrarySchema(
                self.schema_version,
            ));
        }

        let mut ids = BTreeSet::new();
        for preset in &self.presets {
            validate_preset(preset)?;
            if !ids.insert(preset.id.as_str()) {
                return Err(PresetLibraryError::DuplicateId(preset.id.clone()));
            }
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&SeparationPresetDefinition> {
        self.presets.iter().find(|preset| preset.id == id)
    }

    pub fn insert(&mut self, preset: SeparationPresetDefinition) -> Result<(), PresetLibraryError> {
        validate_preset(&preset)?;
        if self.get(&preset.id).is_some() {
            return Err(PresetLibraryError::DuplicateId(preset.id));
        }
        self.presets.push(preset);
        Ok(())
    }

    pub fn duplicate_as_user(
        &mut self,
        source_id: &str,
        new_id: impl Into<String>,
        new_name: impl Into<String>,
    ) -> Result<&SeparationPresetDefinition, PresetLibraryError> {
        let source = self
            .get(source_id)
            .cloned()
            .ok_or_else(|| PresetLibraryError::UnknownId(source_id.to_owned()))?;
        let mut duplicate = source;
        duplicate.id = new_id.into();
        duplicate.name = new_name.into();
        duplicate.origin = PresetOrigin::User;
        duplicate.strategy.preset_name = duplicate.name.clone();
        self.insert(duplicate)?;
        Ok(self.presets.last().expect("preset inserted above"))
    }

    pub fn rename_user(
        &mut self,
        id: &str,
        new_name: impl Into<String>,
    ) -> Result<(), PresetLibraryError> {
        let preset = self
            .presets
            .iter_mut()
            .find(|preset| preset.id == id)
            .ok_or_else(|| PresetLibraryError::UnknownId(id.to_owned()))?;
        if preset.origin == PresetOrigin::BuiltIn {
            return Err(PresetLibraryError::BuiltInImmutable(id.to_owned()));
        }
        let new_name = new_name.into();
        if new_name.trim().is_empty() {
            return Err(PresetLibraryError::EmptyName { id: id.to_owned() });
        }
        preset.name = new_name;
        preset.strategy.preset_name = preset.name.clone();
        Ok(())
    }

    pub fn delete_user(&mut self, id: &str) -> Result<(), PresetLibraryError> {
        let index = self
            .presets
            .iter()
            .position(|preset| preset.id == id)
            .ok_or_else(|| PresetLibraryError::UnknownId(id.to_owned()))?;
        if self.presets[index].origin == PresetOrigin::BuiltIn {
            return Err(PresetLibraryError::BuiltInImmutable(id.to_owned()));
        }
        self.presets.remove(index);
        Ok(())
    }

    pub fn export_preset_json(&self, id: &str) -> Result<String, PresetLibraryError> {
        let preset = self
            .get(id)
            .ok_or_else(|| PresetLibraryError::UnknownId(id.to_owned()))?;
        serde_json::to_string_pretty(preset)
            .map_err(|err| PresetLibraryError::InvalidJson(err.to_string()))
    }

    /// Imported definitions are always treated as user presets. A serialized
    /// `built_in` marker from another installation cannot grant immutability or
    /// impersonate a built-in shipped by this binary.
    pub fn import_user_preset_json(
        &mut self,
        json: &str,
    ) -> Result<&SeparationPresetDefinition, PresetLibraryError> {
        let mut preset: SeparationPresetDefinition = serde_json::from_str(json)
            .map_err(|err| PresetLibraryError::InvalidJson(err.to_string()))?;
        preset.origin = PresetOrigin::User;
        self.insert(preset)?;
        Ok(self.presets.last().expect("preset inserted above"))
    }

    pub fn to_json_pretty(&self) -> Result<String, PresetLibraryError> {
        self.validate()?;
        serde_json::to_string_pretty(self)
            .map_err(|err| PresetLibraryError::InvalidJson(err.to_string()))
    }

    pub fn from_json(json: &str) -> Result<Self, PresetLibraryError> {
        let library: Self = serde_json::from_str(json)
            .map_err(|err| PresetLibraryError::InvalidJson(err.to_string()))?;
        library.validate()?;
        Ok(library)
    }
}

fn validate_preset(preset: &SeparationPresetDefinition) -> Result<(), PresetLibraryError> {
    if preset.schema_version != SEPARATION_PRESET_SCHEMA_VERSION {
        return Err(PresetLibraryError::UnsupportedPresetSchema {
            id: preset.id.clone(),
            schema_version: preset.schema_version,
        });
    }
    if preset.id.trim().is_empty() {
        return Err(PresetLibraryError::EmptyId);
    }
    if preset.name.trim().is_empty() {
        return Err(PresetLibraryError::EmptyName {
            id: preset.id.clone(),
        });
    }
    if preset.target.channel_names.is_empty() {
        return Err(PresetLibraryError::EmptyTargetTopology {
            id: preset.id.clone(),
        });
    }
    if preset.target.channel_names.len() != preset.target.channel_max_coverage.len() {
        return Err(PresetLibraryError::TargetLimitTopologyMismatch {
            id: preset.id.clone(),
        });
    }
    if !matches!(preset.target.bit_depth, 8 | 16) {
        return Err(PresetLibraryError::InvalidBitDepth {
            id: preset.id.clone(),
            bit_depth: preset.target.bit_depth,
        });
    }

    match preset.target.engine_mode {
        ConversionEngineMode::Icc | ConversionEngineMode::DeviceLink => {
            if preset
                .target
                .transform_sha256
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                return Err(PresetLibraryError::MissingTransformIdentity {
                    id: preset.id.clone(),
                });
            }
        }
        ConversionEngineMode::CustomOptimizer => {
            if preset
                .target
                .characterization_id
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                return Err(PresetLibraryError::MissingCharacterizationIdentity {
                    id: preset.id.clone(),
                });
            }
        }
    }

    let mut channel_names = BTreeSet::new();
    for (channel, limit) in preset
        .target
        .channel_names
        .iter()
        .zip(&preset.target.channel_max_coverage)
    {
        let normalized = channel.trim().to_ascii_lowercase();
        if normalized.is_empty() || !channel_names.insert(normalized) {
            return Err(PresetLibraryError::DuplicateTargetChannel {
                id: preset.id.clone(),
                channel: channel.clone(),
            });
        }
        if limit.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
            return Err(PresetLibraryError::InvalidChannelLimit {
                id: preset.id.clone(),
                channel: channel.clone(),
            });
        }
    }
    if preset
        .target
        .target_total_ink_limit
        .is_some_and(|value| !value.is_finite() || value <= 0.0)
    {
        return Err(PresetLibraryError::InvalidTotalInkLimit {
            id: preset.id.clone(),
        });
    }

    validate_strategy(preset, &channel_names)?;
    Ok(())
}

fn validate_strategy(
    preset: &SeparationPresetDefinition,
    target_names: &BTreeSet<String>,
) -> Result<(), PresetLibraryError> {
    let strategy = &preset.strategy;
    let invalid = |reason: &str| PresetLibraryError::InvalidStrategy {
        id: preset.id.clone(),
        reason: reason.to_owned(),
    };

    if !strategy.black_generation_strength.is_finite()
        || !(0.0..=1.0).contains(&strategy.black_generation_strength)
    {
        return Err(invalid(
            "Black generation strength must be finite and in 0..=1.",
        ));
    }
    if !strategy.black_start.is_finite() || !(0.0..=1.0).contains(&strategy.black_start) {
        return Err(invalid("Black start must be finite and in 0..=1."));
    }
    if !strategy.black_max.is_finite() || !(0.0..=1.0).contains(&strategy.black_max) {
        return Err(invalid("Black maximum must be finite and in 0..=1."));
    }
    if !strategy.neutral_chroma_threshold.is_finite() || strategy.neutral_chroma_threshold < 0.0 {
        return Err(invalid(
            "Neutral chroma threshold must be finite and non-negative.",
        ));
    }
    if strategy
        .total_ink_limit
        .is_some_and(|value| !value.is_finite() || value <= 0.0)
    {
        return Err(invalid(
            "Strategy total ink limit must be finite and greater than zero.",
        ));
    }
    if strategy
        .max_delta_e00
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        return Err(invalid(
            "Maximum Delta E00 must be finite and non-negative.",
        ));
    }
    if let Some(black) = strategy.black_channel.as_deref() {
        if !target_names.contains(&black.trim().to_ascii_lowercase()) {
            return Err(invalid("Black channel is not part of the target topology."));
        }
    }
    for (ink, bias) in &strategy.per_ink_bias {
        if !target_names.contains(&ink.trim().to_ascii_lowercase()) {
            return Err(invalid(
                "Ink-priority channel is not part of the target topology.",
            ));
        }
        if !bias.is_finite() || !(-1.0..=1.0).contains(bias) {
            return Err(invalid("Ink-priority value must be finite and in -1..=1."));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{ConversionTargetDefinition, TargetChannelDefinition};
    use crate::conversion_presets::SeparationPresetDefinition;
    use crate::model::IccProfileIdentity;

    fn target() -> ConversionTargetDefinition {
        ConversionTargetDefinition {
            name: "Ceramic 4C".to_owned(),
            channels: ["Blue", "Brown", "Beige", "Black"]
                .into_iter()
                .map(|name| TargetChannelDefinition {
                    name: name.to_owned(),
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: Some(0.8),
                })
                .collect(),
            bit_depth: 16,
            output_profile_identity: Some(IccProfileIdentity {
                description: "Target".to_owned(),
                sha256: "icc-v1".to_owned(),
            }),
            output_profile_path: Some(r"C:\Color\target.icc".to_owned()),
            device_link_identity: None,
            device_link_path: None,
            characterization_id: Some("measurement-v1".to_owned()),
            total_ink_limit: Some(1.6),
        }
    }

    #[test]
    fn built_in_entries_are_immutable_but_can_be_duplicated_as_user_presets() {
        let mut library = SeparationPresetLibrary::new();
        library
            .insert(SeparationPresetDefinition::built_in_balanced(
                &target(),
                ConversionEngineMode::Icc,
            ))
            .unwrap();

        assert_eq!(
            library.rename_user("builtin:balanced:v1", "Changed"),
            Err(PresetLibraryError::BuiltInImmutable(
                "builtin:balanced:v1".to_owned()
            ))
        );
        let duplicate = library
            .duplicate_as_user("builtin:balanced:v1", "user:balanced-1", "Balanced copy")
            .unwrap();
        assert_eq!(duplicate.origin, PresetOrigin::User);
        assert_eq!(duplicate.strategy.preset_name, "Balanced copy");
        library.delete_user("user:balanced-1").unwrap();
        assert!(library.get("user:balanced-1").is_none());
    }

    #[test]
    fn import_export_never_grants_built_in_identity() {
        let target = target();
        let built_in =
            SeparationPresetDefinition::built_in_balanced(&target, ConversionEngineMode::Icc);
        let json = serde_json::to_string(&built_in).unwrap();
        assert!(!json.contains("target.icc"));

        let mut library = SeparationPresetLibrary::new();
        let imported = library.import_user_preset_json(&json).unwrap();
        assert_eq!(imported.origin, PresetOrigin::User);
    }

    #[test]
    fn duplicate_ids_and_malformed_target_bindings_fail_closed() {
        let target = target();
        let preset =
            SeparationPresetDefinition::built_in_balanced(&target, ConversionEngineMode::Icc);
        let mut library = SeparationPresetLibrary::new();
        library.insert(preset.clone()).unwrap();
        assert_eq!(
            library.insert(preset),
            Err(PresetLibraryError::DuplicateId(
                "builtin:balanced:v1".to_owned()
            ))
        );

        let mut malformed =
            SeparationPresetDefinition::built_in_balanced(&target, ConversionEngineMode::Icc);
        malformed.id = "user:bad".to_owned();
        malformed.target.channel_max_coverage.pop();
        assert_eq!(
            library.insert(malformed),
            Err(PresetLibraryError::TargetLimitTopologyMismatch {
                id: "user:bad".to_owned()
            })
        );
    }

    #[test]
    fn full_library_round_trip_is_validated() {
        let mut library = SeparationPresetLibrary::new();
        library
            .insert(SeparationPresetDefinition::built_in_balanced(
                &target(),
                ConversionEngineMode::Icc,
            ))
            .unwrap();
        let json = library.to_json_pretty().unwrap();
        let restored = SeparationPresetLibrary::from_json(&json).unwrap();
        assert_eq!(restored, library);
    }

    #[test]
    fn imported_non_finite_or_unknown_strategy_data_is_rejected() {
        let mut preset =
            SeparationPresetDefinition::built_in_balanced(&target(), ConversionEngineMode::Icc);
        preset.id = "user:invalid".to_owned();
        preset.origin = PresetOrigin::User;
        preset
            .strategy
            .per_ink_bias
            .insert("Orange".to_owned(), 0.4);
        let mut library = SeparationPresetLibrary::new();
        assert!(matches!(
            library.insert(preset),
            Err(PresetLibraryError::InvalidStrategy { .. })
        ));
    }
}
