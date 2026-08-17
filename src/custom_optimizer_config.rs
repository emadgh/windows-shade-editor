use serde::{Deserialize, Serialize};

pub const CUSTOM_OPTIMIZER_SOLVER_CONFIG_SCHEMA_VERSION: u32 = 1;
pub const CUSTOM_OPTIMIZER_MAX_CHANNELS: usize = 12;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CustomOptimizerSolverMethod {
    /// Deterministic low-discrepancy device-space search followed by bounded
    /// beam refinement and pair-transfer moves. The `v1` suffix is part of the
    /// serialized method identity and must change if search semantics change.
    BoundedHaltonBeamV1,
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
        }
    }
}

impl CustomOptimizerSolverConfig {
    pub fn validate(&self, channel_count: usize) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != CUSTOM_OPTIMIZER_SOLVER_CONFIG_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported Custom Optimizer solver-config schema {} (expected {}).",
                self.schema_version, CUSTOM_OPTIMIZER_SOLVER_CONFIG_SCHEMA_VERSION
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
                "Custom Optimizer step_decay must be finite and in 0.1..=0.95.".to_owned(),
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

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid_for_supported_nchannel_targets() {
        for channel_count in [1usize, 4, 8, 12] {
            assert!(CustomOptimizerSolverConfig::default().validate(channel_count).is_ok());
        }
    }

    #[test]
    fn method_identity_is_explicitly_versioned_in_json() {
        let json = serde_json::to_string(&CustomOptimizerSolverConfig::default()).unwrap();
        assert!(json.contains("bounded_halton_beam_v1"));
        assert!(json.contains("\"schema_version\":1"));
    }

    #[test]
    fn invalid_runtime_bounds_fail_closed() {
        let mut config = CustomOptimizerSolverConfig::default();
        config.initial_samples = 1;
        config.preference_delta_e00 = f32::NAN;
        assert!(config.validate(4).is_err());
        assert!(CustomOptimizerSolverConfig::default().validate(13).is_err());
    }
}
