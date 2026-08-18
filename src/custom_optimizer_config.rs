use serde::{Deserialize, Serialize};

pub const LEGACY_CUSTOM_OPTIMIZER_SOLVER_CONFIG_SCHEMA_VERSION: u32 = 1;
pub const CUSTOM_OPTIMIZER_SOLVER_CONFIG_SCHEMA_VERSION: u32 = 2;
pub const CUSTOM_OPTIMIZER_OBJECTIVE_WEIGHTS_SCHEMA_VERSION: u32 = 1;
pub const CUSTOM_OPTIMIZER_MAX_CHANNELS: usize = 12;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CustomOptimizerSolverMethod {
    /// Deterministic low-discrepancy device-space search followed by bounded
    /// beam refinement and pair-transfer moves. The `v1` suffix is part of the
    /// serialized method identity and must never be reinterpreted.
    BoundedHaltonBeamV1,
    /// V2 preserves the V1 search/constraint semantics and adds an explicit
    /// continuity preference only inside the existing color-equivalence window.
    BoundedHaltonBeamContinuityV2,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContinuityDistanceMetric {
    NormalizedL1,
    NormalizedL2,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CustomOptimizerObjectiveWeights {
    pub schema_version: u32,
    pub color_error: f32,
    pub ink_preference: f32,
    pub neutral_black: f32,
    pub total_ink: f32,
}

impl Default for CustomOptimizerObjectiveWeights {
    fn default() -> Self {
        Self {
            schema_version: CUSTOM_OPTIMIZER_OBJECTIVE_WEIGHTS_SCHEMA_VERSION,
            color_error: 1.0,
            ink_preference: 1.0,
            neutral_black: 1.0,
            total_ink: 0.25,
        }
    }
}

impl CustomOptimizerObjectiveWeights {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != CUSTOM_OPTIMIZER_OBJECTIVE_WEIGHTS_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported Custom Optimizer objective-weight schema {} (expected {}).",
                self.schema_version, CUSTOM_OPTIMIZER_OBJECTIVE_WEIGHTS_SCHEMA_VERSION
            ));
        }
        for (name, value) in [
            ("color_error", self.color_error),
            ("ink_preference", self.ink_preference),
            ("neutral_black", self.neutral_black),
            ("total_ink", self.total_ink),
        ] {
            if !value.is_finite() || !(0.0..=100.0).contains(&value) {
                errors.push(format!(
                    "Custom Optimizer objective weight {name} must be finite and in 0..=100."
                ));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct ContinuityPreferenceConfig {
    /// Weight applied to normalized ink-vector distance. Zero disables every
    /// continuity preference and must reproduce V1 candidate ordering.
    pub weight: f32,
    pub distance_metric: ContinuityDistanceMetric,
    /// Preferred maximum normalized jump for any single channel. This is a
    /// ranking preference, never a replacement for target/channel hard limits.
    pub max_normalized_channel_jump: f32,
    /// Additional ranking penalty when the dominant channel changes relative to
    /// the explicit reference separation.
    pub dominant_channel_switch_penalty: f32,
}

impl ContinuityPreferenceConfig {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if !self.weight.is_finite() || !(0.0..=100.0).contains(&self.weight) {
            errors.push("Continuity weight must be finite and in 0..=100.".to_owned());
        }
        if !self.max_normalized_channel_jump.is_finite()
            || !(0.0..=1.0).contains(&self.max_normalized_channel_jump)
        {
            errors.push(
                "Continuity max_normalized_channel_jump must be finite and in 0..=1."
                    .to_owned(),
            );
        }
        if !self.dominant_channel_switch_penalty.is_finite()
            || !(0.0..=100.0).contains(&self.dominant_channel_switch_penalty)
        {
            errors.push(
                "Continuity dominant_channel_switch_penalty must be finite and in 0..=100."
                    .to_owned(),
            );
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct CustomOptimizerSolverConfig {
    pub schema_version: u32,
    pub method: CustomOptimizerSolverMethod,
    /// Deterministic low-discrepancy samples used for broad device-space search.
    pub initial_samples: usize,
    /// Number of best candidates retained between refinement rounds.
    pub beam_width: usize,
    /// Number of local coordinate/pair-transfer refinement passes.
    pub refinement_rounds: usize,
    /// First local step as a fraction of each channel's allowed coverage range.
    pub initial_step_fraction: f32,
    /// Multiplicative step reduction after every refinement round.
    pub step_decay: f32,
    /// Candidate color differences within this CIEDE2000 distance from the best
    /// feasible color in a search stage are colorimetrically equivalent for
    /// production-preference ranking. Zero means strict minimum-DeltaE ranking.
    pub preference_delta_e00: f32,
    /// Explicit production objective weights. Schema-v1 solver configs predate
    /// this provenance and therefore omit the block; they remain readable but
    /// cannot identify a production inverse LUT safely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective_weights: Option<CustomOptimizerObjectiveWeights>,
    /// Present only for the explicitly versioned V2 method.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuity_preference: Option<ContinuityPreferenceConfig>,
}

impl Default for CustomOptimizerSolverConfig {
    fn default() -> Self {
        Self {
            schema_version: CUSTOM_OPTIMIZER_SOLVER_CONFIG_SCHEMA_VERSION,
            method: CustomOptimizerSolverMethod::BoundedHaltonBeamV1,
            initial_samples: 384,
            beam_width: 24,
            refinement_rounds: 4,
            initial_step_fraction: 0.18,
            step_decay: 0.5,
            preference_delta_e00: 0.10,
            objective_weights: Some(CustomOptimizerObjectiveWeights::default()),
            continuity_preference: None,
        }
    }
}

impl CustomOptimizerSolverConfig {
    pub fn validate(&self, channel_count: usize) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if !matches!(
            self.schema_version,
            LEGACY_CUSTOM_OPTIMIZER_SOLVER_CONFIG_SCHEMA_VERSION
                | CUSTOM_OPTIMIZER_SOLVER_CONFIG_SCHEMA_VERSION
        ) {
            errors.push(format!(
                "Unsupported Custom Optimizer solver-config schema {} (supported: {} and {}).",
                self.schema_version,
                LEGACY_CUSTOM_OPTIMIZER_SOLVER_CONFIG_SCHEMA_VERSION,
                CUSTOM_OPTIMIZER_SOLVER_CONFIG_SCHEMA_VERSION
            ));
        }
        if !(1..=CUSTOM_OPTIMIZER_MAX_CHANNELS).contains(&channel_count) {
            errors.push(format!(
                "Custom Optimizer solver supports 1..={} channels, got {channel_count}.",
                CUSTOM_OPTIMIZER_MAX_CHANNELS
            ));
        }
        if !(32..=16_384).contains(&self.initial_samples) {
            errors.push("Custom Optimizer initial_samples must be in 32..=16384.".to_owned());
        }
        if !(4..=256).contains(&self.beam_width) {
            errors.push("Custom Optimizer beam_width must be in 4..=256.".to_owned());
        }
        if self.refinement_rounds > 8 {
            errors.push("Custom Optimizer refinement_rounds must be <= 8.".to_owned());
        }
        if !self.initial_step_fraction.is_finite()
            || !(0.005..=0.5).contains(&self.initial_step_fraction)
        {
            errors.push(
                "Custom Optimizer initial_step_fraction must be finite and in 0.005..=0.5."
                    .to_owned(),
            );
        }
        if !self.step_decay.is_finite() || !(0.1..=0.95).contains(&self.step_decay) {
            errors.push(
                "Custom Optimizer step_decay must be finite and in 0.1..=0.95."
                    .to_owned(),
            );
        }
        if !self.preference_delta_e00.is_finite()
            || !(0.0..=1.0).contains(&self.preference_delta_e00)
        {
            errors.push(
                "Custom Optimizer preference_delta_e00 must be finite and in 0..=1.0."
                    .to_owned(),
            );
        }

        match (self.schema_version, self.objective_weights) {
            (LEGACY_CUSTOM_OPTIMIZER_SOLVER_CONFIG_SCHEMA_VERSION, Some(_)) => errors.push(
                "Legacy Custom Optimizer solver-config schema 1 must not carry objective_weights."
                    .to_owned(),
            ),
            (CUSTOM_OPTIMIZER_SOLVER_CONFIG_SCHEMA_VERSION, None) => errors.push(
                "Custom Optimizer solver-config schema 2 requires explicit objective_weights."
                    .to_owned(),
            ),
            (CUSTOM_OPTIMIZER_SOLVER_CONFIG_SCHEMA_VERSION, Some(weights)) => {
                if let Err(weight_errors) = weights.validate() {
                    errors.extend(weight_errors);
                }
            }
            _ => {}
        }

        match (self.method, self.continuity_preference) {
            (CustomOptimizerSolverMethod::BoundedHaltonBeamV1, Some(_)) => errors.push(
                "BoundedHaltonBeamV1 must not carry continuity_preference; use the versioned V2 method."
                    .to_owned(),
            ),
            (CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2, None) => errors.push(
                "BoundedHaltonBeamContinuityV2 requires an explicit continuity_preference block."
                    .to_owned(),
            ),
            (CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2, Some(policy)) => {
                if let Err(policy_errors) = policy.validate() {
                    errors.extend(policy_errors);
                }
            }
            (CustomOptimizerSolverMethod::BoundedHaltonBeamV1, None) => {}
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn production_objective_weights(
        &self,
    ) -> Result<CustomOptimizerObjectiveWeights, Vec<String>> {
        self.validate(1).or_else(|errors| {
            // Channel count is validated by callers with the real topology; this
            // helper only needs the schema/provenance check below.
            let only_channel_count_error = errors
                .iter()
                .all(|error| error.starts_with("Custom Optimizer solver supports"));
            if only_channel_count_error {
                Ok(())
            } else {
                Err(errors)
            }
        })?;
        self.objective_weights.ok_or_else(|| {
            vec![
                "Custom Optimizer objective-weight provenance is missing; recapture the recipe with solver-config schema 2 before production LUT use."
                    .to_owned(),
            ]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn continuity(weight: f32) -> ContinuityPreferenceConfig {
        ContinuityPreferenceConfig {
            weight,
            distance_metric: ContinuityDistanceMetric::NormalizedL2,
            max_normalized_channel_jump: 0.20,
            dominant_channel_switch_penalty: 0.25,
        }
    }

    fn legacy_v1() -> CustomOptimizerSolverConfig {
        CustomOptimizerSolverConfig {
            schema_version: LEGACY_CUSTOM_OPTIMIZER_SOLVER_CONFIG_SCHEMA_VERSION,
            objective_weights: None,
            ..CustomOptimizerSolverConfig::default()
        }
    }

    #[test]
    fn default_config_is_current_and_valid_for_supported_nchannel_targets() {
        let config = CustomOptimizerSolverConfig::default();
        assert_eq!(config.schema_version, CUSTOM_OPTIMIZER_SOLVER_CONFIG_SCHEMA_VERSION);
        assert_eq!(config.objective_weights, Some(CustomOptimizerObjectiveWeights::default()));
        for channel_count in [1usize, 4, 8, 12] {
            assert!(config.validate(channel_count).is_ok());
        }
    }

    #[test]
    fn legacy_v1_json_identity_omits_objective_block() {
        let json = serde_json::to_string(&legacy_v1()).unwrap();
        assert!(json.contains("bounded_halton_beam_v1"));
        assert!(json.contains("\"schema_version\":1"));
        assert!(!json.contains("objective_weights"));
    }

    #[test]
    fn current_schema_requires_explicit_valid_objective_weights() {
        let missing = CustomOptimizerSolverConfig {
            objective_weights: None,
            ..CustomOptimizerSolverConfig::default()
        };
        assert!(missing.validate(4).is_err());

        let invalid = CustomOptimizerSolverConfig {
            objective_weights: Some(CustomOptimizerObjectiveWeights {
                ink_preference: f32::NAN,
                ..CustomOptimizerObjectiveWeights::default()
            }),
            ..CustomOptimizerSolverConfig::default()
        };
        assert!(invalid.validate(4).is_err());
    }

    #[test]
    fn legacy_v1_is_readable_but_has_no_production_objective_provenance() {
        let legacy = legacy_v1();
        assert!(legacy.validate(4).is_ok());
        assert!(legacy.production_objective_weights().is_err());
        assert_eq!(
            CustomOptimizerSolverConfig::default().production_objective_weights(),
            Ok(CustomOptimizerObjectiveWeights::default())
        );
    }

    #[test]
    fn v2_requires_explicit_valid_continuity_policy() {
        let missing = CustomOptimizerSolverConfig {
            method: CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2,
            ..CustomOptimizerSolverConfig::default()
        };
        assert!(missing.validate(4).is_err());

        let valid = CustomOptimizerSolverConfig {
            method: CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2,
            continuity_preference: Some(continuity(1.0)),
            ..CustomOptimizerSolverConfig::default()
        };
        assert!(valid.validate(4).is_ok());
    }

    #[test]
    fn v1_rejects_continuity_policy_instead_of_reinterpreting_old_method() {
        let invalid = CustomOptimizerSolverConfig {
            continuity_preference: Some(continuity(1.0)),
            ..CustomOptimizerSolverConfig::default()
        };
        assert!(invalid.validate(4).is_err());
    }

    #[test]
    fn zero_continuity_weight_is_valid_for_exact_v1_ordering_mode() {
        let config = CustomOptimizerSolverConfig {
            method: CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2,
            continuity_preference: Some(continuity(0.0)),
            ..CustomOptimizerSolverConfig::default()
        };
        assert!(config.validate(4).is_ok());
    }

    #[test]
    fn invalid_runtime_bounds_fail_closed() {
        let mut config = CustomOptimizerSolverConfig::default();
        config.initial_samples = 1;
        config.preference_delta_e00 = f32::NAN;
        assert!(config.validate(4).is_err());
        assert!(CustomOptimizerSolverConfig::default().validate(13).is_err());

        let invalid_policy = CustomOptimizerSolverConfig {
            method: CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2,
            continuity_preference: Some(ContinuityPreferenceConfig {
                weight: f32::NAN,
                distance_metric: ContinuityDistanceMetric::NormalizedL1,
                max_normalized_channel_jump: 1.1,
                dominant_channel_switch_penalty: -1.0,
            }),
            ..CustomOptimizerSolverConfig::default()
        };
        assert!(invalid_policy.validate(4).is_err());
    }
}
