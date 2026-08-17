use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::color_conversion::{ConversionEngineMode, ConversionRecipe};
use crate::conversion_recipe::recipe_sha256;
use crate::custom_optimizer_config::{CustomOptimizerSolverConfig, CustomOptimizerSolverMethod};
use crate::device_characterization::DeviceForwardModel;
use crate::device_characterization_model::{LocalForwardModelConfig, ValidatedLocalForwardModel};

pub const INVERSE_LUT_IDENTITY_SCHEMA_VERSION: u32 = 1;
pub const INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION: u32 = 1;
pub const MAX_INVERSE_LUT_GRID_CELLS: u64 = 1_000_000;
pub const MAX_INVERSE_LUT_AXIS_SAMPLES: u16 = 257;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InverseLutForwardModelMethod {
    /// Exact numerical interpretation implemented by `ValidatedLocalForwardModel`
    /// in the v1 local inverse-distance-weighted model.
    LocalInverseDistanceWeightedV1,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InverseLutInterpolationMethod {
    TrilinearV1,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InverseLutUnsupportedCellEncoding {
    /// Unsupported nodes/cells are explicitly represented by a validity mask.
    /// Runtime lookup must not silently extrapolate across them.
    ExplicitValidityMaskV1,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InverseLutNumericalPrecision {
    /// Normalized channel coverages stored as IEEE-754 f32 values.
    NormalizedF32V1,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InverseLutContinuityFieldMethod {
    /// Every PCS grid node is solved independently. This exactly represents V1
    /// and V2 with zero continuity weight, where the solver ignores references.
    IndependentNodeSolvesV1,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct LabGridSpec {
    pub l_min: f64,
    pub l_max: f64,
    pub l_samples: u16,
    pub a_min: f64,
    pub a_max: f64,
    pub a_samples: u16,
    pub b_min: f64,
    pub b_max: f64,
    pub b_samples: u16,
}

impl LabGridSpec {
    pub fn cell_count(&self) -> Option<u64> {
        u64::from(self.l_samples)
            .checked_mul(u64::from(self.a_samples))?
            .checked_mul(u64::from(self.b_samples))
    }

    fn validate_into(&self, errors: &mut Vec<String>) {
        for (name, value) in [
            ("L* minimum", self.l_min),
            ("L* maximum", self.l_max),
            ("a* minimum", self.a_min),
            ("a* maximum", self.a_max),
            ("b* minimum", self.b_min),
            ("b* maximum", self.b_max),
        ] {
            if !value.is_finite() {
                errors.push(format!("Inverse LUT {name} must be finite."));
            }
        }

        if self.l_min.is_finite()
            && self.l_max.is_finite()
            && (!(0.0..=100.0).contains(&self.l_min)
                || !(0.0..=100.0).contains(&self.l_max)
                || self.l_min >= self.l_max)
        {
            errors.push(
                "Inverse LUT L* bounds must satisfy 0 <= l_min < l_max <= 100.".to_owned(),
            );
        }
        if self.a_min.is_finite() && self.a_max.is_finite() && self.a_min >= self.a_max {
            errors.push("Inverse LUT a* bounds must satisfy a_min < a_max.".to_owned());
        }
        if self.b_min.is_finite() && self.b_max.is_finite() && self.b_min >= self.b_max {
            errors.push("Inverse LUT b* bounds must satisfy b_min < b_max.".to_owned());
        }

        for (name, count) in [
            ("L*", self.l_samples),
            ("a*", self.a_samples),
            ("b*", self.b_samples),
        ] {
            if !(2..=MAX_INVERSE_LUT_AXIS_SAMPLES).contains(&count) {
                errors.push(format!(
                    "Inverse LUT {name} axis must contain 2..={MAX_INVERSE_LUT_AXIS_SAMPLES} samples, got {count}."
                ));
            }
        }

        match self.cell_count() {
            Some(count) if count <= MAX_INVERSE_LUT_GRID_CELLS => {}
            Some(count) => errors.push(format!(
                "Inverse LUT grid contains {count} nodes; maximum bounded grid size is {MAX_INVERSE_LUT_GRID_CELLS}."
            )),
            None => errors.push("Inverse LUT grid node count overflowed u64.".to_owned()),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct InverseLutBuildPolicy {
    pub schema_version: u32,
    pub grid: LabGridSpec,
    pub interpolation: InverseLutInterpolationMethod,
    pub unsupported_cells: InverseLutUnsupportedCellEncoding,
    pub numerical_precision: InverseLutNumericalPrecision,
    pub continuity_field: InverseLutContinuityFieldMethod,
}

impl InverseLutBuildPolicy {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported inverse LUT build-policy schema {} (expected {}).",
                self.schema_version, INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION
            ));
        }
        self.grid.validate_into(&mut errors);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct InverseLutForwardModelIdentity {
    pub method: InverseLutForwardModelMethod,
    pub config: LocalForwardModelConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct InverseLutIdentityRecord {
    pub schema_version: u32,
    /// Existing content address from `CharacterizationPackage::id`.
    pub characterization_id: String,
    /// Exact numerical model used to interpret the measured package.
    pub forward_model: InverseLutForwardModelIdentity,
    /// Existing deterministic fingerprint of target + strategy + solver policy.
    pub recipe_sha256: String,
    /// Redundant explicit topology guard for cache loading and diagnostics.
    pub channel_names: Vec<String>,
    pub target_bit_depth: u8,
    pub build_policy: InverseLutBuildPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InverseLutIdentityError {
    InvalidRecipe(Vec<String>),
    NotCustomOptimizerRecipe,
    MissingSolverConfig,
    ModelIdentityMismatch { recipe: String, model: String },
    ModelTopologyMismatch { recipe: Vec<String>, model: Vec<String> },
    PositiveContinuityRequiresFieldConstruction,
    InvalidBuildPolicy(Vec<String>),
    InvalidIdentityRecord(Vec<String>),
    FingerprintFailed(String),
}

impl InverseLutIdentityRecord {
    /// Construct a content-addressable identity from the actual validated local
    /// forward model and exact conversion recipe.
    ///
    /// Positive-weight V2 continuity is deliberately rejected for now: a static
    /// Lab->inks grid cannot discard the explicit reference dependency. #179 must
    /// define and validate a versioned offline continuity-field construction
    /// method before such a LUT can receive a production identity.
    pub fn from_local_model(
        recipe: &ConversionRecipe,
        model: &ValidatedLocalForwardModel,
        build_policy: InverseLutBuildPolicy,
    ) -> Result<Self, InverseLutIdentityError> {
        recipe
            .validate()
            .map_err(InverseLutIdentityError::InvalidRecipe)?;
        if recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
            return Err(InverseLutIdentityError::NotCustomOptimizerRecipe);
        }
        build_policy
            .validate()
            .map_err(InverseLutIdentityError::InvalidBuildPolicy)?;

        let solver = recipe
            .custom_optimizer_solver
            .as_ref()
            .ok_or(InverseLutIdentityError::MissingSolverConfig)?;
        validate_solver_lut_semantics(solver)?;

        let model_identity = model.identity();
        let recipe_characterization = recipe
            .target
            .characterization_id
            .as_deref()
            .unwrap_or_default();
        if recipe_characterization != model_identity.id {
            return Err(InverseLutIdentityError::ModelIdentityMismatch {
                recipe: recipe_characterization.to_owned(),
                model: model_identity.id.clone(),
            });
        }

        let recipe_channels = recipe
            .target
            .channels
            .iter()
            .map(|channel| channel.name.clone())
            .collect::<Vec<_>>();
        if recipe_channels != model_identity.channel_names {
            return Err(InverseLutIdentityError::ModelTopologyMismatch {
                recipe: recipe_channels,
                model: model_identity.channel_names.clone(),
            });
        }

        let recipe_sha256 = recipe_sha256(recipe)
            .map_err(InverseLutIdentityError::FingerprintFailed)?;
        let record = Self {
            schema_version: INVERSE_LUT_IDENTITY_SCHEMA_VERSION,
            characterization_id: model_identity.id.clone(),
            forward_model: InverseLutForwardModelIdentity {
                method: InverseLutForwardModelMethod::LocalInverseDistanceWeightedV1,
                config: model.config(),
            },
            recipe_sha256,
            channel_names: model_identity.channel_names.clone(),
            target_bit_depth: recipe.target.bit_depth,
            build_policy,
        };
        record
            .validate()
            .map_err(InverseLutIdentityError::InvalidIdentityRecord)?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != INVERSE_LUT_IDENTITY_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported inverse LUT identity schema {} (expected {}).",
                self.schema_version, INVERSE_LUT_IDENTITY_SCHEMA_VERSION
            ));
        }
        if !is_prefixed_sha256(&self.characterization_id) {
            errors.push(
                "Inverse LUT characterization_id must be canonical lowercase 'sha256:<64 hex>'."
                    .to_owned(),
            );
        }
        if !is_bare_sha256(&self.recipe_sha256) {
            errors.push(
                "Inverse LUT recipe_sha256 must be canonical lowercase 64-character hex."
                    .to_owned(),
            );
        }
        validate_forward_model_config(self.forward_model.config, &mut errors);
        validate_topology(&self.channel_names, &mut errors);
        if !matches!(self.target_bit_depth, 8 | 16) {
            errors.push(format!(
                "Inverse LUT target bit depth must be 8 or 16, got {}.",
                self.target_bit_depth
            ));
        }
        if let Err(policy_errors) = self.build_policy.validate() {
            errors.extend(policy_errors);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Stable content address for the exact characterization/model/recipe/LUT
    /// construction identity. This identifies the LUT contract, not a signature.
    pub fn content_id(&self) -> Result<String, String> {
        self.validate().map_err(|errors| errors.join("\n"))?;
        let bytes = serde_json::to_vec(self).map_err(|err| err.to_string())?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

pub fn validate_solver_lut_semantics(
    solver: &CustomOptimizerSolverConfig,
) -> Result<(), InverseLutIdentityError> {
    match (solver.method, solver.continuity_preference) {
        (CustomOptimizerSolverMethod::BoundedHaltonBeamV1, _) => Ok(()),
        (CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2, Some(policy))
            if policy.weight == 0.0 =>
        {
            Ok(())
        }
        (CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2, Some(_)) => {
            Err(InverseLutIdentityError::PositiveContinuityRequiresFieldConstruction)
        }
        (CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2, None) => {
            Err(InverseLutIdentityError::MissingSolverConfig)
        }
    }
}

fn validate_forward_model_config(config: LocalForwardModelConfig, errors: &mut Vec<String>) {
    if config.neighbor_count < 2 {
        errors.push("Inverse LUT local forward model requires at least two neighbors.".to_owned());
    }
    if !config.distance_power.is_finite() || config.distance_power <= 0.0 {
        errors.push("Inverse LUT forward-model distance power must be finite and > 0.".to_owned());
    }
    if !config.max_support_distance.is_finite()
        || config.max_support_distance <= 0.0
        || config.max_support_distance > 1.0
    {
        errors.push(
            "Inverse LUT forward-model support distance must be finite and in (0, 1]."
                .to_owned(),
        );
    }
}

fn validate_topology(channel_names: &[String], errors: &mut Vec<String>) {
    if !(4..=12).contains(&channel_names.len()) {
        errors.push(format!(
            "Inverse LUT production topology must contain 4..=12 channels, got {}.",
            channel_names.len()
        ));
    }
    let mut unique = BTreeSet::new();
    for name in channel_names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            errors.push("Inverse LUT channel names cannot be empty.".to_owned());
            continue;
        }
        if !unique.insert(trimmed.to_ascii_lowercase()) {
            errors.push(format!("Duplicate inverse LUT channel '{trimmed}'."));
        }
    }
}

fn is_prefixed_sha256(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(is_bare_sha256)
}

fn is_bare_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_optimizer_config::{
        ContinuityDistanceMetric, ContinuityPreferenceConfig,
    };

    fn grid() -> LabGridSpec {
        LabGridSpec {
            l_min: 0.0,
            l_max: 100.0,
            l_samples: 33,
            a_min: -128.0,
            a_max: 127.0,
            a_samples: 33,
            b_min: -128.0,
            b_max: 127.0,
            b_samples: 33,
        }
    }

    fn policy() -> InverseLutBuildPolicy {
        InverseLutBuildPolicy {
            schema_version: INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION,
            grid: grid(),
            interpolation: InverseLutInterpolationMethod::TrilinearV1,
            unsupported_cells: InverseLutUnsupportedCellEncoding::ExplicitValidityMaskV1,
            numerical_precision: InverseLutNumericalPrecision::NormalizedF32V1,
            continuity_field: InverseLutContinuityFieldMethod::IndependentNodeSolvesV1,
        }
    }

    fn record() -> InverseLutIdentityRecord {
        InverseLutIdentityRecord {
            schema_version: INVERSE_LUT_IDENTITY_SCHEMA_VERSION,
            characterization_id: format!("sha256:{}", "a".repeat(64)),
            forward_model: InverseLutForwardModelIdentity {
                method: InverseLutForwardModelMethod::LocalInverseDistanceWeightedV1,
                config: LocalForwardModelConfig {
                    neighbor_count: 8,
                    distance_power: 2.0,
                    max_support_distance: 0.45,
                },
            },
            recipe_sha256: "b".repeat(64),
            channel_names: ["Blue", "Brown", "Beige", "Black"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            target_bit_depth: 16,
            build_policy: policy(),
        }
    }

    fn v2(weight: f32) -> CustomOptimizerSolverConfig {
        CustomOptimizerSolverConfig {
            method: CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2,
            continuity_preference: Some(ContinuityPreferenceConfig {
                weight,
                distance_metric: ContinuityDistanceMetric::NormalizedL2,
                max_normalized_channel_jump: 0.2,
                dominant_channel_switch_penalty: 0.25,
            }),
            ..CustomOptimizerSolverConfig::default()
        }
    }

    #[test]
    fn identical_identity_record_has_identical_content_address() {
        let first = record();
        let second = first.clone();
        assert_eq!(first.content_id().unwrap(), second.content_id().unwrap());
    }

    #[test]
    fn every_numerical_identity_component_changes_content_address() {
        let base = record();
        let base_id = base.content_id().unwrap();

        let mut changed = base.clone();
        changed.characterization_id = format!("sha256:{}", "c".repeat(64));
        assert_ne!(base_id, changed.content_id().unwrap());

        let mut changed = base.clone();
        changed.recipe_sha256 = "d".repeat(64);
        assert_ne!(base_id, changed.content_id().unwrap());

        let mut changed = base.clone();
        changed.forward_model.config.neighbor_count += 1;
        assert_ne!(base_id, changed.content_id().unwrap());

        let mut changed = base.clone();
        changed.channel_names.swap(0, 1);
        assert_ne!(base_id, changed.content_id().unwrap());

        let mut changed = base.clone();
        changed.build_policy.grid.l_samples += 1;
        assert_ne!(base_id, changed.content_id().unwrap());
    }

    #[test]
    fn malformed_hashes_topology_and_grid_fail_closed() {
        let mut invalid = record();
        invalid.characterization_id = "measurement-v1".to_owned();
        invalid.recipe_sha256 = "ABC".repeat(21) + "D";
        invalid.channel_names[1] = "blue".to_owned();
        invalid.build_policy.grid.l_samples = MAX_INVERSE_LUT_AXIS_SAMPLES + 1;
        let errors = invalid.validate().expect_err("invalid identity must fail");
        assert!(errors.iter().any(|error| error.contains("characterization_id")));
        assert!(errors.iter().any(|error| error.contains("recipe_sha256")));
        assert!(errors.iter().any(|error| error.contains("Duplicate")));
        assert!(errors.iter().any(|error| error.contains("axis")));
    }

    #[test]
    fn grid_memory_contract_is_bounded() {
        let mut too_large = policy();
        too_large.grid.l_samples = 100;
        too_large.grid.a_samples = 100;
        too_large.grid.b_samples = 101;
        let errors = too_large.validate().expect_err("oversized grid must fail");
        assert!(errors.iter().any(|error| error.contains("maximum bounded grid")));
    }

    #[test]
    fn positive_v2_continuity_fails_closed_until_field_construction_is_versioned() {
        assert_eq!(
            validate_solver_lut_semantics(&v2(1.0)),
            Err(InverseLutIdentityError::PositiveContinuityRequiresFieldConstruction)
        );
        assert!(validate_solver_lut_semantics(&v2(0.0)).is_ok());
        assert!(validate_solver_lut_semantics(&CustomOptimizerSolverConfig::default()).is_ok());
    }

    #[test]
    fn non_finite_model_or_grid_values_fail_closed() {
        let mut invalid = record();
        invalid.forward_model.config.distance_power = f64::NAN;
        invalid.build_policy.grid.a_max = f64::INFINITY;
        let errors = invalid.validate().expect_err("non-finite identity must fail");
        assert!(errors.iter().any(|error| error.contains("distance power")));
        assert!(errors.iter().any(|error| error.contains("a* maximum")));
    }
}
