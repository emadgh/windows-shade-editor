use serde::{Deserialize, Serialize};

use crate::color_conversion::{
    ConversionEngineMode, ConversionTargetDefinition, SeparationStrategy,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetCompatibilityKey {
    pub engine_mode: ConversionEngineMode,
    #[serde(default)]
    pub transform_sha256: Option<String>,
    #[serde(default)]
    pub characterization_id: Option<String>,
    pub channel_names: Vec<String>,
    pub bit_depth: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SeparationPreset {
    pub name: String,
    pub target: TargetCompatibilityKey,
    pub strategy: SeparationStrategy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresetCompatibility {
    Compatible,
    EngineModeMismatch,
    TransformIdentityMismatch,
    CharacterizationMismatch,
    ChannelTopologyMismatch,
    BitDepthMismatch,
}

impl TargetCompatibilityKey {
    pub fn from_target(
        target: &ConversionTargetDefinition,
        engine_mode: ConversionEngineMode,
    ) -> Self {
        let transform_sha256 = match engine_mode {
            ConversionEngineMode::Icc => target
                .output_profile_identity
                .as_ref()
                .map(|identity| identity.sha256.clone()),
            ConversionEngineMode::DeviceLink => target
                .device_link_identity
                .as_ref()
                .map(|identity| identity.sha256.clone()),
            ConversionEngineMode::CustomOptimizer => None,
        };

        let characterization_id = match engine_mode {
            ConversionEngineMode::CustomOptimizer => target.characterization_id.clone(),
            ConversionEngineMode::Icc | ConversionEngineMode::DeviceLink => None,
        };

        Self {
            engine_mode,
            transform_sha256,
            characterization_id,
            channel_names: target
                .channels
                .iter()
                .map(|channel| channel.name.clone())
                .collect(),
            bit_depth: target.bit_depth,
        }
    }

    pub fn compatibility_with(
        &self,
        target: &ConversionTargetDefinition,
        engine_mode: ConversionEngineMode,
    ) -> PresetCompatibility {
        if self.engine_mode != engine_mode {
            return PresetCompatibility::EngineModeMismatch;
        }
        if self.bit_depth != target.bit_depth {
            return PresetCompatibility::BitDepthMismatch;
        }

        let channel_names = target
            .channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect::<Vec<_>>();
        let expected_names = self
            .channel_names
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        if expected_names != channel_names {
            return PresetCompatibility::ChannelTopologyMismatch;
        }

        match engine_mode {
            ConversionEngineMode::Icc => {
                let current = target
                    .output_profile_identity
                    .as_ref()
                    .map(|identity| identity.sha256.as_str());
                if self.transform_sha256.as_deref() != current {
                    return PresetCompatibility::TransformIdentityMismatch;
                }
            }
            ConversionEngineMode::DeviceLink => {
                let current = target
                    .device_link_identity
                    .as_ref()
                    .map(|identity| identity.sha256.as_str());
                if self.transform_sha256.as_deref() != current {
                    return PresetCompatibility::TransformIdentityMismatch;
                }
            }
            ConversionEngineMode::CustomOptimizer => {
                if self.characterization_id.as_deref() != target.characterization_id.as_deref() {
                    return PresetCompatibility::CharacterizationMismatch;
                }
            }
        }

        PresetCompatibility::Compatible
    }
}

impl SeparationPreset {
    pub fn compatibility_with(
        &self,
        target: &ConversionTargetDefinition,
        engine_mode: ConversionEngineMode,
    ) -> PresetCompatibility {
        self.target.compatibility_with(target, engine_mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::TargetChannelDefinition;
    use crate::model::IccProfileIdentity;

    fn identity(hash: &str) -> IccProfileIdentity {
        IccProfileIdentity {
            description: "Target".to_owned(),
            sha256: hash.to_owned(),
        }
    }

    fn target() -> ConversionTargetDefinition {
        ConversionTargetDefinition {
            name: "Ceramic 4C".to_owned(),
            channels: ["Blue", "Brown", "Beige", "Black"]
                .into_iter()
                .map(|name| TargetChannelDefinition {
                    name: name.to_owned(),
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: None,
                })
                .collect(),
            bit_depth: 16,
            output_profile_identity: Some(identity("icc-v1")),
            device_link_identity: Some(identity("link-v1")),
            characterization_id: Some("measurement-v1".to_owned()),
            total_ink_limit: Some(1.6),
        }
    }

    #[test]
    fn preset_is_compatible_only_with_exact_channel_order() {
        let base = target();
        let key = TargetCompatibilityKey::from_target(&base, ConversionEngineMode::Icc);
        assert_eq!(
            key.compatibility_with(&base, ConversionEngineMode::Icc),
            PresetCompatibility::Compatible
        );

        let mut reordered = base.clone();
        reordered.channels.swap(0, 1);
        assert_eq!(
            key.compatibility_with(&reordered, ConversionEngineMode::Icc),
            PresetCompatibility::ChannelTopologyMismatch
        );
    }

    #[test]
    fn replacing_same_named_icc_invalidates_preset_by_hash() {
        let base = target();
        let key = TargetCompatibilityKey::from_target(&base, ConversionEngineMode::Icc);
        let mut changed = base.clone();
        changed.output_profile_identity = Some(identity("icc-v2"));

        assert_eq!(
            key.compatibility_with(&changed, ConversionEngineMode::Icc),
            PresetCompatibility::TransformIdentityMismatch
        );
    }

    #[test]
    fn custom_optimizer_preset_is_bound_to_characterization_version() {
        let base = target();
        let key =
            TargetCompatibilityKey::from_target(&base, ConversionEngineMode::CustomOptimizer);
        let mut changed = base.clone();
        changed.characterization_id = Some("measurement-v2".to_owned());

        assert_eq!(
            key.compatibility_with(&changed, ConversionEngineMode::CustomOptimizer),
            PresetCompatibility::CharacterizationMismatch
        );
    }

    #[test]
    fn preset_cannot_cross_engine_modes() {
        let base = target();
        let key = TargetCompatibilityKey::from_target(&base, ConversionEngineMode::DeviceLink);
        assert_eq!(
            key.compatibility_with(&base, ConversionEngineMode::Icc),
            PresetCompatibility::EngineModeMismatch
        );
    }
}
