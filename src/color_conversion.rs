use std::collections::{BTreeMap, BTreeSet};

pub mod production_provenance;

use serde::{Deserialize, Serialize};

use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
use crate::model::IccProfileIdentity;

pub const LEGACY_CONVERSION_RECIPE_SCHEMA_VERSION: u32 = 1;
pub const CONVERSION_RECIPE_SCHEMA_VERSION: u32 = 2;

/// Project role for the source/production derivation workflow.
///
/// Existing Shade Editor projects predate color conversion and must not be
/// reinterpreted automatically. `Standalone` is therefore the backward-safe
/// default until a project explicitly participates in a conversion workflow.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRole {
    #[default]
    Standalone,
    Source,
    Production,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinkedProjectRef {
    pub role: ProjectRole,
    pub path: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversionEngineMode {
    Icc,
    DeviceLink,
    CustomOptimizer,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversionRenderingIntent {
    Perceptual,
    RelativeColorimetric,
    Saturation,
    AbsoluteColorimetric,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TargetChannelDefinition {
    pub name: String,
    #[serde(default)]
    pub display_rgb: Option<[u8; 3]>,
    #[serde(default = "default_solidity")]
    pub solidity: f32,
    /// Maximum normalized channel coverage (0..=1). None means the target does
    /// not impose a Shade Editor-side limit beyond its characterized transform.
    #[serde(default)]
    pub max_coverage: Option<f32>,
}

fn default_solidity() -> f32 {
    1.0
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConversionTargetDefinition {
    pub name: String,
    pub channels: Vec<TargetChannelDefinition>,
    pub bit_depth: u8,
    /// ICC output profile used by the normal output-transform path.
    #[serde(default)]
    pub output_profile_identity: Option<IccProfileIdentity>,
    /// External ICC path used by the worker. The payload remains outside the
    /// recipe and is verified against `output_profile_identity` before use.
    #[serde(default)]
    pub output_profile_path: Option<String>,
    /// DeviceLink profile used by a precomputed device-to-device separation path.
    #[serde(default)]
    pub device_link_identity: Option<IccProfileIdentity>,
    /// External DeviceLink path, identity-verified before conversion.
    #[serde(default)]
    pub device_link_path: Option<String>,
    /// Versioned identifier for measured target characterization consumed by
    /// Shade Editor's future custom N-ink optimizer.
    #[serde(default)]
    pub characterization_id: Option<String>,
    /// Optional normalized aggregate channel-coverage limit. For N channels a
    /// value of 1.8 corresponds to 180% total laydown.
    #[serde(default)]
    pub total_ink_limit: Option<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SeparationStrategy {
    pub preset_name: String,
    /// Real target-channel name selected as the neutral/Black construction ink.
    pub black_channel: Option<String>,
    /// 0 = no extra Black preference; 1 = strongest allowed preference subject
    /// to characterization, color-error and ink-limit constraints.
    pub black_generation_strength: f32,
    /// Normalized tone threshold where additional Black generation may begin.
    pub black_start: f32,
    /// Maximum normalized Black coverage allowed by this strategy.
    pub black_max: f32,
    /// C* threshold used to classify near-neutral colors for Black-focused logic.
    pub neutral_chroma_threshold: f32,
    /// Per-ink preference in -1..=1. Negative values penalize/avoid an ink;
    /// positive values prefer it. These are optimizer weights, never post-transform
    /// channel multipliers.
    pub per_ink_bias: BTreeMap<String, f32>,
    /// Optional strategy-level total laydown override. The stricter of target
    /// and strategy limits must win when both are present.
    pub total_ink_limit: Option<f32>,
    /// Optional maximum tolerated color error for a biased candidate.
    pub max_delta_e00: Option<f32>,
}

impl Default for SeparationStrategy {
    fn default() -> Self {
        Self {
            preset_name: "Balanced".to_owned(),
            black_channel: None,
            black_generation_strength: 0.0,
            black_start: 0.25,
            black_max: 1.0,
            neutral_chroma_threshold: 8.0,
            per_ink_bias: BTreeMap::new(),
            total_ink_limit: None,
            max_delta_e00: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConversionRecipe {
    pub schema_version: u32,
    pub engine_mode: ConversionEngineMode,
    /// Exact source-profile bytes are not embedded here. The stable profile hash
    /// makes a recipe stale if the external/embedded profile changes.
    pub source_profile_identity: IccProfileIdentity,
    pub target: ConversionTargetDefinition,
    pub rendering_intent: ConversionRenderingIntent,
    pub black_point_compensation: bool,
    #[serde(default)]
    pub strategy: SeparationStrategy,
    /// Exact Custom Optimizer search method and numerical policy. Omitted
    /// for ICC/DeviceLink so legacy non-optimizer JSON remains byte-stable
    /// after deserialize/serialize round-trips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_optimizer_solver: Option<CustomOptimizerSolverConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversionSourceRef {
    pub source_project_path: String,
    pub source_face_path: String,
    pub source_snapshot_id: Option<u64>,
    pub source_file_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ProductionProvenance {
    pub source: ConversionSourceRef,
    pub recipe: ConversionRecipe,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_optimizer: Option<production_provenance::CustomOptimizerProductionProvenance>,
    pub output_path: String,
    pub output_sha256: String,
    pub converted_at_unix_ms: i64,
}

impl ConversionRecipe {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if !matches!(
            self.schema_version,
            LEGACY_CONVERSION_RECIPE_SCHEMA_VERSION | CONVERSION_RECIPE_SCHEMA_VERSION
        ) {
            errors.push(format!(
                "Unsupported conversion recipe schema {} (supported: {} and {}).",
                self.schema_version,
                LEGACY_CONVERSION_RECIPE_SCHEMA_VERSION,
                CONVERSION_RECIPE_SCHEMA_VERSION
            ));
        }
        if self.source_profile_identity.sha256.trim().is_empty() {
            errors.push("Source ICC identity must include a SHA-256 hash.".to_owned());
        }

        self.target.validate_into(&mut errors);
        self.strategy.validate_into(&self.target, &mut errors);

        match self.engine_mode {
            ConversionEngineMode::Icc => {
                if self.custom_optimizer_solver.is_some() {
                    errors.push(
                        "ICC recipes must not carry Custom Optimizer solver configuration."
                            .to_owned(),
                    );
                }
                if !has_profile_hash(self.target.output_profile_identity.as_ref()) {
                    errors.push(
                        "ICC conversion requires a target output profile with a stable hash."
                            .to_owned(),
                    );
                }
                if !has_profile_path(self.target.output_profile_path.as_deref()) {
                    errors.push(
                        "ICC conversion requires the external target profile path.".to_owned(),
                    );
                }
            }
            ConversionEngineMode::DeviceLink => {
                if self.custom_optimizer_solver.is_some() {
                    errors.push(
                        "DeviceLink recipes must not carry Custom Optimizer solver configuration."
                            .to_owned(),
                    );
                }
                if !has_profile_hash(self.target.device_link_identity.as_ref()) {
                    errors.push(
                        "DeviceLink conversion requires a DeviceLink profile with a stable hash."
                            .to_owned(),
                    );
                }
                if !has_profile_path(self.target.device_link_path.as_deref()) {
                    errors.push(
                        "DeviceLink conversion requires the external DeviceLink path.".to_owned(),
                    );
                }
            }
            ConversionEngineMode::CustomOptimizer => {
                if self
                    .target
                    .characterization_id
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty())
                {
                    errors.push(
                        "Custom N-ink optimization requires a versioned target characterization."
                            .to_owned(),
                    );
                }
                if self.schema_version == LEGACY_CONVERSION_RECIPE_SCHEMA_VERSION {
                    errors.push(
                        "Legacy Custom Optimizer recipe schema 1 has no solver provenance; recapture it as schema 2 before production execution."
                            .to_owned(),
                    );
                }
                match self.custom_optimizer_solver.as_ref() {
                    Some(config) => {
                        if let Err(config_errors) = config.validate(self.target.channels.len()) {
                            errors.extend(config_errors);
                        }
                    }
                    None if self.schema_version == CONVERSION_RECIPE_SCHEMA_VERSION => {
                        errors.push(
                            "Custom Optimizer recipe schema 2 requires explicit solver method/configuration."
                                .to_owned(),
                        );
                    }
                    None => {}
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl ConversionTargetDefinition {
    fn validate_into(&self, errors: &mut Vec<String>) {
        if self.name.trim().is_empty() {
            errors.push("Conversion target name cannot be empty.".to_owned());
        }
        if !matches!(self.bit_depth, 8 | 16) {
            errors.push("Conversion target bit depth must be 8 or 16.".to_owned());
        }
        if self.channels.is_empty() {
            errors.push("Conversion target must declare at least one output channel.".to_owned());
        }

        let mut names = BTreeSet::new();
        for channel in &self.channels {
            let name = channel.name.trim();
            if name.is_empty() {
                errors.push("Target channel names cannot be empty.".to_owned());
            } else if !names.insert(name.to_owned()) {
                errors.push(format!("Duplicate target channel name '{name}'."));
            }
            if !(0.0..=1.0).contains(&channel.solidity) {
                errors.push(format!(
                    "Channel '{}' Solidity must be in 0..=1.",
                    channel.name
                ));
            }
            if channel
                .max_coverage
                .is_some_and(|value| !(0.0..=1.0).contains(&value))
            {
                errors.push(format!(
                    "Channel '{}' maximum coverage must be in 0..=1.",
                    channel.name
                ));
            }
        }

        if self.total_ink_limit.is_some_and(|value| value <= 0.0) {
            errors.push("Target total ink limit must be greater than zero.".to_owned());
        }
    }
}

impl SeparationStrategy {
    fn validate_into(&self, target: &ConversionTargetDefinition, errors: &mut Vec<String>) {
        if !(0.0..=1.0).contains(&self.black_generation_strength) {
            errors.push("Black generation strength must be in 0..=1.".to_owned());
        }
        if !(0.0..=1.0).contains(&self.black_start) {
            errors.push("Black start must be in 0..=1.".to_owned());
        }
        if !(0.0..=1.0).contains(&self.black_max) {
            errors.push("Black maximum must be in 0..=1.".to_owned());
        }
        if self.neutral_chroma_threshold < 0.0 {
            errors.push("Neutral chroma threshold cannot be negative.".to_owned());
        }
        if self.total_ink_limit.is_some_and(|value| value <= 0.0) {
            errors.push("Strategy total ink limit must be greater than zero.".to_owned());
        }
        if self.max_delta_e00.is_some_and(|value| value < 0.0) {
            errors.push("Maximum Delta E00 cannot be negative.".to_owned());
        }

        let target_names = target
            .channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect::<BTreeSet<_>>();

        if let Some(black) = self.black_channel.as_deref() {
            if !target_names.contains(black) {
                errors.push(format!(
                    "Black strategy channel '{black}' is not present in the target topology."
                ));
            }
        }

        for (ink, bias) in &self.per_ink_bias {
            if !target_names.contains(ink.as_str()) {
                errors.push(format!(
                    "Ink-priority entry '{ink}' is not present in the target topology."
                ));
            }
            if !(-1.0..=1.0).contains(bias) {
                errors.push(format!("Ink-priority value for '{ink}' must be in -1..=1."));
            }
        }
    }
}

fn has_profile_hash(identity: Option<&IccProfileIdentity>) -> bool {
    identity.is_some_and(|identity| !identity.sha256.trim().is_empty())
}

fn has_profile_path(path: Option<&str>) -> bool {
    path.is_some_and(|path| !path.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, hash: &str) -> IccProfileIdentity {
        IccProfileIdentity {
            description: name.to_owned(),
            sha256: hash.to_owned(),
        }
    }

    fn seven_channel_target() -> ConversionTargetDefinition {
        ConversionTargetDefinition {
            name: "Ceramic 7C".to_owned(),
            channels: ["Blue", "Brown", "Beige", "Black", "Yellow", "Pink", "Green"]
                .into_iter()
                .map(|name| TargetChannelDefinition {
                    name: name.to_owned(),
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: Some(0.85),
                })
                .collect(),
            bit_depth: 16,
            output_profile_identity: Some(profile("Ceramic 7C", "target-hash")),
            output_profile_path: Some(r"C:\Color\Ceramic-7C.icc".to_owned()),
            device_link_identity: None,
            device_link_path: None,
            characterization_id: Some("ceramic-7c-measurement-v1".to_owned()),
            total_ink_limit: Some(1.8),
        }
    }

    #[test]
    fn standalone_is_backward_safe_default_project_role() {
        #[derive(Deserialize)]
        struct Envelope {
            #[serde(default)]
            role: ProjectRole,
        }

        let restored: Envelope = serde_json::from_str("{}").expect("deserialize legacy envelope");
        assert_eq!(restored.role, ProjectRole::Standalone);
    }

    #[test]
    fn black_focused_custom_recipe_validates_and_round_trips() {
        let mut strategy = SeparationStrategy {
            preset_name: "Black-focused".to_owned(),
            black_channel: Some("Black".to_owned()),
            black_generation_strength: 0.8,
            black_start: 0.2,
            black_max: 0.7,
            neutral_chroma_threshold: 8.0,
            max_delta_e00: Some(2.0),
            ..SeparationStrategy::default()
        };
        strategy.per_ink_bias.insert("Blue".to_owned(), -0.6);
        strategy.per_ink_bias.insert("Black".to_owned(), 0.8);

        let recipe = ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: profile("Adobe RGB (1998)", "source-hash"),
            target: seven_channel_target(),
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: true,
            strategy,
            custom_optimizer_solver: Some(
                crate::custom_optimizer_config::CustomOptimizerSolverConfig::default(),
            ),
        };

        assert!(recipe.validate().is_ok());
        let json = serde_json::to_string(&recipe).expect("serialize recipe");
        let restored: ConversionRecipe = serde_json::from_str(&json).expect("deserialize recipe");
        assert_eq!(restored, recipe);
    }

    #[test]
    fn target_rejects_duplicate_channel_names() {
        let mut target = seven_channel_target();
        target.channels.push(TargetChannelDefinition {
            name: "Blue".to_owned(),
            display_rgb: None,
            solidity: 1.0,
            max_coverage: None,
        });
        let recipe = ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: profile("sRGB", "source-hash"),
            target,
            rendering_intent: ConversionRenderingIntent::Perceptual,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: None,
        };

        let errors = recipe.validate().expect_err("duplicate channel must fail");
        assert!(
            errors
                .iter()
                .any(|error| error.contains("Duplicate target channel"))
        );
    }

    #[test]
    fn icc_recipe_requires_identity_verified_external_target_path() {
        let mut target = seven_channel_target();
        target.output_profile_path = None;
        let recipe = ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: profile("sRGB", "source-hash"),
            target,
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: true,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: None,
        };

        let errors = recipe
            .validate()
            .expect_err("target path must be reproducible");
        assert!(
            errors
                .iter()
                .any(|error| error.contains("external target profile path"))
        );
    }

    #[test]
    fn strategy_rejects_unknown_ink_bias() {
        let mut strategy = SeparationStrategy::default();
        strategy.per_ink_bias.insert("Orange".to_owned(), 0.4);
        let recipe = ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: profile("sRGB", "source-hash"),
            target: seven_channel_target(),
            rendering_intent: ConversionRenderingIntent::Perceptual,
            black_point_compensation: false,
            strategy,
            custom_optimizer_solver: None,
        };

        let errors = recipe.validate().expect_err("unknown ink bias must fail");
        assert!(errors.iter().any(|error| error.contains("Orange")));
    }

    #[test]
    fn engine_mode_requires_matching_characterization_source() {
        let mut target = seven_channel_target();
        target.characterization_id = None;
        let recipe = ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: profile("sRGB", "source-hash"),
            target,
            rendering_intent: ConversionRenderingIntent::Perceptual,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: Some(
                crate::custom_optimizer_config::CustomOptimizerSolverConfig::default(),
            ),
        };

        let errors = recipe
            .validate()
            .expect_err("custom optimizer without characterization must fail");
        assert!(
            errors
                .iter()
                .any(|error| error.contains("characterization"))
        );
    }
}
