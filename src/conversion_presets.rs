use serde::{Deserialize, Serialize};

use crate::color_conversion::{
    ConversionEngineMode, ConversionRecipe, ConversionTargetDefinition, SeparationStrategy,
};

pub const SEPARATION_PRESET_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PresetOrigin {
    BuiltIn,
    User,
}

/// Production-critical target identity captured by a reusable separation preset.
///
/// External ICC/DeviceLink paths are deliberately not part of this key: moving
/// the exact same verified profile bytes must not invalidate a preset. Stable
/// hashes, characterization identity, exact channel topology and output limits
/// are part of the binding because changing any of them can change production
/// separation behavior.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PresetTargetBinding {
    pub engine_mode: ConversionEngineMode,
    #[serde(default)]
    pub transform_sha256: Option<String>,
    #[serde(default)]
    pub characterization_id: Option<String>,
    pub channel_names: Vec<String>,
    pub bit_depth: u8,
    pub channel_max_coverage: Vec<Option<f32>>,
    #[serde(default)]
    pub target_total_ink_limit: Option<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SeparationPresetDefinition {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub notes: Option<String>,
    pub origin: PresetOrigin,
    pub target: PresetTargetBinding,
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
    ChannelLimitMismatch,
    TargetInkLimitMismatch,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PresetApplyError {
    UnsupportedSchema(u32),
    Incompatible(PresetCompatibility),
    InvalidRecipe(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuiltInPresetError {
    UnsupportedEngine,
    UnknownBlackChannel(String),
}

impl PresetTargetBinding {
    pub fn from_target(
        target: &ConversionTargetDefinition,
        engine_mode: ConversionEngineMode,
    ) -> Self {
        let transform_sha256 = match engine_mode {
            ConversionEngineMode::Icc => target
                .output_profile_identity
                .as_ref()
                .map(|identity| identity.sha256.trim().to_owned()),
            ConversionEngineMode::DeviceLink => target
                .device_link_identity
                .as_ref()
                .map(|identity| identity.sha256.trim().to_owned()),
            ConversionEngineMode::CustomOptimizer => None,
        };

        let characterization_id = match engine_mode {
            ConversionEngineMode::CustomOptimizer => target
                .characterization_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
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
            channel_max_coverage: target
                .channels
                .iter()
                .map(|channel| channel.max_coverage)
                .collect(),
            target_total_ink_limit: target.total_ink_limit,
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

        let current_limits = target
            .channels
            .iter()
            .map(|channel| channel.max_coverage)
            .collect::<Vec<_>>();
        if self.channel_max_coverage != current_limits {
            return PresetCompatibility::ChannelLimitMismatch;
        }
        if self.target_total_ink_limit != target.total_ink_limit {
            return PresetCompatibility::TargetInkLimitMismatch;
        }

        match engine_mode {
            ConversionEngineMode::Icc => {
                let current = target
                    .output_profile_identity
                    .as_ref()
                    .map(|identity| identity.sha256.trim());
                if self.transform_sha256.as_deref() != current {
                    return PresetCompatibility::TransformIdentityMismatch;
                }
            }
            ConversionEngineMode::DeviceLink => {
                let current = target
                    .device_link_identity
                    .as_ref()
                    .map(|identity| identity.sha256.trim());
                if self.transform_sha256.as_deref() != current {
                    return PresetCompatibility::TransformIdentityMismatch;
                }
            }
            ConversionEngineMode::CustomOptimizer => {
                let current = target
                    .characterization_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                if self.characterization_id.as_deref() != current {
                    return PresetCompatibility::CharacterizationMismatch;
                }
            }
        }

        PresetCompatibility::Compatible
    }
}

impl SeparationPresetDefinition {
    pub fn capture_from_recipe(
        id: impl Into<String>,
        name: impl Into<String>,
        notes: Option<String>,
        origin: PresetOrigin,
        recipe: &ConversionRecipe,
    ) -> Result<Self, Vec<String>> {
        recipe.validate()?;
        let name = name.into();
        let mut strategy = recipe.strategy.clone();
        strategy.preset_name = name.clone();

        Ok(Self {
            schema_version: SEPARATION_PRESET_SCHEMA_VERSION,
            id: id.into(),
            name,
            notes,
            origin,
            target: PresetTargetBinding::from_target(&recipe.target, recipe.engine_mode),
            strategy,
        })
    }

    pub fn compatibility_with_recipe(&self, recipe: &ConversionRecipe) -> PresetCompatibility {
        self.target
            .compatibility_with(&recipe.target, recipe.engine_mode)
    }

    /// Apply the preset non-destructively and return a complete versioned recipe.
    /// The caller's recipe is never mutated.
    pub fn apply_to_recipe(
        &self,
        recipe: &ConversionRecipe,
    ) -> Result<ConversionRecipe, PresetApplyError> {
        if self.schema_version != SEPARATION_PRESET_SCHEMA_VERSION {
            return Err(PresetApplyError::UnsupportedSchema(self.schema_version));
        }

        let compatibility = self.compatibility_with_recipe(recipe);
        if compatibility != PresetCompatibility::Compatible {
            return Err(PresetApplyError::Incompatible(compatibility));
        }

        let mut candidate = recipe.clone();
        candidate.strategy = self.strategy.clone();
        candidate.strategy.preset_name = self.name.clone();
        candidate
            .validate()
            .map_err(PresetApplyError::InvalidRecipe)?;
        Ok(candidate)
    }

    pub fn built_in_balanced(
        target: &ConversionTargetDefinition,
        engine_mode: ConversionEngineMode,
    ) -> Self {
        Self {
            schema_version: SEPARATION_PRESET_SCHEMA_VERSION,
            id: "builtin:balanced:v1".to_owned(),
            name: "Balanced".to_owned(),
            notes: Some("No additional ink preference beyond the selected production transform.".to_owned()),
            origin: PresetOrigin::BuiltIn,
            target: PresetTargetBinding::from_target(target, engine_mode),
            strategy: SeparationStrategy::default(),
        }
    }

    /// Deterministic Custom Optimizer baseline for near-neutral Black preference.
    /// It does not post-multiply transform channels; the optimizer must honor the
    /// target characterization and Delta-E/coverage constraints.
    pub fn built_in_black_focused(
        target: &ConversionTargetDefinition,
        engine_mode: ConversionEngineMode,
        black_channel: &str,
    ) -> Result<Self, BuiltInPresetError> {
        if engine_mode != ConversionEngineMode::CustomOptimizer {
            return Err(BuiltInPresetError::UnsupportedEngine);
        }

        let black = target
            .channels
            .iter()
            .find(|channel| channel.name == black_channel)
            .ok_or_else(|| BuiltInPresetError::UnknownBlackChannel(black_channel.to_owned()))?;
        let black_max = black.max_coverage.unwrap_or(1.0).min(0.7);

        Ok(Self {
            schema_version: SEPARATION_PRESET_SCHEMA_VERSION,
            id: "builtin:black-focused:v1".to_owned(),
            name: "Black-focused".to_owned(),
            notes: Some(
                "Prefer the designated Black ink for near-neutral candidates while preserving characterized color and coverage constraints."
                    .to_owned(),
            ),
            origin: PresetOrigin::BuiltIn,
            target: PresetTargetBinding::from_target(target, engine_mode),
            strategy: SeparationStrategy {
                preset_name: "Black-focused".to_owned(),
                black_channel: Some(black_channel.to_owned()),
                black_generation_strength: 0.8,
                black_start: 0.2,
                black_max,
                neutral_chroma_threshold: 8.0,
                total_ink_limit: target.total_ink_limit,
                max_delta_e00: Some(2.0),
                ..SeparationStrategy::default()
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        ConversionRenderingIntent, TargetChannelDefinition, CONVERSION_RECIPE_SCHEMA_VERSION,
    };
    use crate::conversion_recipe::recipe_sha256;
    use crate::model::IccProfileIdentity;

    fn identity(hash: &str) -> IccProfileIdentity {
        IccProfileIdentity {
            description: "Production transform".to_owned(),
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
                    max_coverage: Some(if name == "Black" { 0.6 } else { 0.8 }),
                })
                .collect(),
            bit_depth: 16,
            output_profile_identity: Some(identity("icc-v1")),
            output_profile_path: Some(r"C:\Color\output.icc".to_owned()),
            device_link_identity: Some(identity("link-v1")),
            device_link_path: Some(r"C:\Color\device-link.icc".to_owned()),
            characterization_id: Some("measurement-v1".to_owned()),
            total_ink_limit: Some(1.6),
        }
    }

    fn recipe(engine_mode: ConversionEngineMode) -> ConversionRecipe {
        ConversionRecipe {
            source_transparency_policy: None,
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode,
            source_profile_identity: identity("source-v1"),
            target: target(),
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: (engine_mode == ConversionEngineMode::CustomOptimizer)
                                .then(crate::custom_optimizer_config::CustomOptimizerSolverConfig::default),
        }
    }

    #[test]
    fn same_named_replaced_icc_invalidates_preset_by_hash() {
        let base = recipe(ConversionEngineMode::Icc);
        let preset = SeparationPresetDefinition::built_in_balanced(
            &base.target,
            ConversionEngineMode::Icc,
        );
        let mut changed = base.clone();
        changed.target.output_profile_identity = Some(identity("icc-v2"));
        assert_eq!(
            preset.compatibility_with_recipe(&changed),
            PresetCompatibility::TransformIdentityMismatch
        );
    }

    #[test]
    fn moving_same_verified_profile_does_not_invalidate_preset() {
        let base = recipe(ConversionEngineMode::Icc);
        let preset = SeparationPresetDefinition::built_in_balanced(
            &base.target,
            ConversionEngineMode::Icc,
        );
        let mut moved = base.clone();
        moved.target.output_profile_path = Some(r"D:\Profiles\output.icc".to_owned());
        assert_eq!(
            preset.compatibility_with_recipe(&moved),
            PresetCompatibility::Compatible
        );
    }

    #[test]
    fn exact_channel_order_and_limits_are_part_of_target_binding() {
        let base = recipe(ConversionEngineMode::CustomOptimizer);
        let preset = SeparationPresetDefinition::built_in_balanced(
            &base.target,
            ConversionEngineMode::CustomOptimizer,
        );

        let mut reordered = base.clone();
        reordered.target.channels.swap(0, 1);
        assert_eq!(
            preset.compatibility_with_recipe(&reordered),
            PresetCompatibility::ChannelTopologyMismatch
        );

        let mut relimited = base.clone();
        relimited.target.channels[0].max_coverage = Some(0.75);
        assert_eq!(
            preset.compatibility_with_recipe(&relimited),
            PresetCompatibility::ChannelLimitMismatch
        );
    }

    #[test]
    fn changed_characterization_invalidates_custom_optimizer_preset() {
        let base = recipe(ConversionEngineMode::CustomOptimizer);
        let preset = SeparationPresetDefinition::built_in_balanced(
            &base.target,
            ConversionEngineMode::CustomOptimizer,
        );
        let mut changed = base.clone();
        changed.target.characterization_id = Some("measurement-v2".to_owned());
        assert_eq!(
            preset.compatibility_with_recipe(&changed),
            PresetCompatibility::CharacterizationMismatch
        );
    }

    #[test]
    fn applying_preset_returns_a_new_explicit_recipe_and_keeps_source_unchanged() {
        let base = recipe(ConversionEngineMode::CustomOptimizer);
        let before_hash = recipe_sha256(&base).unwrap();
        let preset = SeparationPresetDefinition::built_in_black_focused(
            &base.target,
            ConversionEngineMode::CustomOptimizer,
            "Black",
        )
        .unwrap();

        let applied = preset.apply_to_recipe(&base).unwrap();
        assert_eq!(base.strategy, SeparationStrategy::default());
        assert_eq!(recipe_sha256(&base).unwrap(), before_hash);
        assert_ne!(recipe_sha256(&applied).unwrap(), before_hash);
        assert_eq!(applied.strategy.preset_name, "Black-focused");
        assert_eq!(applied.strategy.black_channel.as_deref(), Some("Black"));
    }

    #[test]
    fn black_focused_is_characterized_optimizer_only_and_respects_black_limit() {
        let base = target();
        assert_eq!(
            SeparationPresetDefinition::built_in_black_focused(
                &base,
                ConversionEngineMode::Icc,
                "Black"
            ),
            Err(BuiltInPresetError::UnsupportedEngine)
        );
        let preset = SeparationPresetDefinition::built_in_black_focused(
            &base,
            ConversionEngineMode::CustomOptimizer,
            "Black",
        )
        .unwrap();
        assert_eq!(preset.strategy.black_max, 0.6);
    }

    #[test]
    fn preset_schema_and_engine_mismatch_fail_closed() {
        let base = recipe(ConversionEngineMode::CustomOptimizer);
        let mut preset = SeparationPresetDefinition::built_in_balanced(
            &base.target,
            ConversionEngineMode::CustomOptimizer,
        );
        preset.schema_version += 1;
        assert_eq!(
            preset.apply_to_recipe(&base),
            Err(PresetApplyError::UnsupportedSchema(2))
        );

        let icc = recipe(ConversionEngineMode::Icc);
        preset.schema_version = SEPARATION_PRESET_SCHEMA_VERSION;
        assert_eq!(
            preset.apply_to_recipe(&icc),
            Err(PresetApplyError::Incompatible(
                PresetCompatibility::EngineModeMismatch
            ))
        );
    }

    #[test]
    fn captured_user_preset_round_trips_with_stable_binding() {
        let mut base = recipe(ConversionEngineMode::CustomOptimizer);
        base.strategy.black_channel = Some("Black".to_owned());
        base.strategy.black_generation_strength = 0.5;
        let preset = SeparationPresetDefinition::capture_from_recipe(
            "user:factory-neutral-v1",
            "Factory neutral",
            Some("Operator preset".to_owned()),
            PresetOrigin::User,
            &base,
        )
        .unwrap();

        let json = serde_json::to_string(&preset).unwrap();
        let restored: SeparationPresetDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, preset);
        assert_eq!(restored.strategy.preset_name, "Factory neutral");
    }
}
