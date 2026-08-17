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
pub const MAX_INVERSE_LUT_GRID_NODES: u64 = 1_000_000;
pub const MAX_INVERSE_LUT_AXIS_SAMPLES: u16 = 257;
pub const INVERSE_LUT_JACOBI_FIELD_METHOD_MAX_ITERATIONS: u16 = 64;
pub const MAX_INVERSE_LUT_JACOBI_FIELD_GRID_NODES: u64 = 250_000;
pub const MAX_INVERSE_LUT_JACOBI_FIELD_NODE_SOLVES: u64 = 4_000_000;

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
pub enum InverseLutValidityEncoding {
    /// Every sampled PCS grid node carries an explicit validity bit. Runtime
    /// trilinear interpolation is allowed only when every required corner node
    /// is valid; unsupported regions are never bridged or extrapolated.
    ExplicitNodeValidityMaskV1,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InverseLutNumericalPrecision {
    /// Normalized channel coverages stored as IEEE-754 f32 values.
    NormalizedF32V1,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InverseLutOutputQuantization {
    /// Reject non-finite input, clamp normalized coverage to 0..=1, scale by
    /// the selected target integer maximum, then use Rust `f32::round`.
    ClampScaleRoundV1,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InverseLutContinuitySeedMethod {
    /// Every supported grid node is first solved independently through the
    /// exact BoundedHaltonBeamV1 search using the V2 solver's numerical knobs.
    IndependentV1NodeSolveV1,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InverseLutContinuityFieldMethod {
    /// Every PCS grid node is solved independently. This exactly represents V1
    /// and V2 with zero continuity weight, where the solver ignores references.
    IndependentNodeSolvesV1,
    /// Synchronous six-neighbor Jacobi field. L-, L+, a-, a+, b-, b+ neighbor
    /// accumulation order, immutable previous snapshots and fixed iteration
    /// count are part of the versioned method contract.
    JacobiSixNeighborV1 {
        seed_method: InverseLutContinuitySeedMethod,
        iterations: u16,
        self_weight: f32,
    },
}

impl InverseLutContinuityFieldMethod {
    pub fn validate_for_grid(&self, grid: &LabGridSpec) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if let Self::JacobiSixNeighborV1 {
            iterations,
            self_weight,
            ..
        } = self
        {
            if !(1..=INVERSE_LUT_JACOBI_FIELD_METHOD_MAX_ITERATIONS).contains(iterations) {
                errors.push(format!(
                    "Jacobi inverse-LUT field iterations must be in 1..={INVERSE_LUT_JACOBI_FIELD_METHOD_MAX_ITERATIONS}."
                ));
            }
            if !self_weight.is_finite() || !(0.0..=1.0).contains(self_weight) {
                errors.push(
                    "Jacobi inverse-LUT field self_weight must be finite and in 0..=1.".to_owned(),
                );
            }
            if let Some(nodes) = grid.node_count() {
                if nodes > MAX_INVERSE_LUT_JACOBI_FIELD_GRID_NODES {
                    errors.push(format!(
                        "Jacobi inverse-LUT field contains {nodes} nodes; maximum is {MAX_INVERSE_LUT_JACOBI_FIELD_GRID_NODES}."
                    ));
                }
                match nodes.checked_mul(u64::from(*iterations) + 1) {
                    Some(solves) if solves <= MAX_INVERSE_LUT_JACOBI_FIELD_NODE_SOLVES => {}
                    Some(solves) => errors.push(format!(
                        "Jacobi inverse-LUT field requires up to {solves} node solves; maximum is {MAX_INVERSE_LUT_JACOBI_FIELD_NODE_SOLVES}."
                    )),
                    None => errors.push(
                        "Jacobi inverse-LUT field work budget overflowed u64.".to_owned(),
                    ),
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

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
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
    pub fn node_count(&self) -> Option<u64> {
        u64::from(self.l_samples)
            .checked_mul(u64::from(self.a_samples))?
            .checked_mul(u64::from(self.b_samples))
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        self.validate_into(&mut errors);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
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
            errors.push("Inverse LUT L* bounds must satisfy 0 <= l_min < l_max <= 100.".to_owned());
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

        match self.node_count() {
            Some(count) if count <= MAX_INVERSE_LUT_GRID_NODES => {}
            Some(count) => errors.push(format!(
                "Inverse LUT grid contains {count} nodes; maximum bounded grid size is {MAX_INVERSE_LUT_GRID_NODES}."
            )),
            None => errors.push("Inverse LUT grid node count overflowed u64.".to_owned()),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutBuildPolicy {
    pub schema_version: u32,
    pub grid: LabGridSpec,
    pub interpolation: InverseLutInterpolationMethod,
    pub validity_encoding: InverseLutValidityEncoding,
    pub numerical_precision: InverseLutNumericalPrecision,
    pub output_quantization: InverseLutOutputQuantization,
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
        if let Err(field_errors) = self.continuity_field.validate_for_grid(&self.grid) {
            errors.extend(field_errors);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutLocalForwardModelConfigIdentity {
    pub neighbor_count: usize,
    pub distance_power: f64,
    pub max_support_distance: f64,
}

impl InverseLutLocalForwardModelConfigIdentity {
    pub fn from_runtime(config: LocalForwardModelConfig) -> Self {
        // Deliberately destructure every runtime field. Adding a future field to
        // LocalForwardModelConfig must create a compile error here so numerical
        // identity cannot drift without an explicit schema/method review.
        let LocalForwardModelConfig {
            neighbor_count,
            distance_power,
            max_support_distance,
        } = config;
        Self {
            neighbor_count,
            distance_power,
            max_support_distance,
        }
    }

    pub fn runtime_config(self) -> LocalForwardModelConfig {
        LocalForwardModelConfig {
            neighbor_count: self.neighbor_count,
            distance_power: self.distance_power,
            max_support_distance: self.max_support_distance,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutForwardModelIdentity {
    pub method: InverseLutForwardModelMethod,
    pub config: InverseLutLocalForwardModelConfigIdentity,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
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
    ModelIdentityMismatch {
        recipe: String,
        model: String,
    },
    ModelTopologyMismatch {
        recipe: Vec<String>,
        model: Vec<String>,
    },
    PositiveContinuityRequiresFieldConstruction,
    ContinuityFieldPolicyDoesNotMatchSolver,
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
        validate_solver_lut_semantics(solver, build_policy.continuity_field)?;

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

        let recipe_sha256 =
            recipe_sha256(recipe).map_err(InverseLutIdentityError::FingerprintFailed)?;
        let record = Self {
            schema_version: INVERSE_LUT_IDENTITY_SCHEMA_VERSION,
            characterization_id: model_identity.id.clone(),
            forward_model: InverseLutForwardModelIdentity {
                method: InverseLutForwardModelMethod::LocalInverseDistanceWeightedV1,
                config: InverseLutLocalForwardModelConfigIdentity::from_runtime(model.config()),
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

fn validate_solver_lut_semantics(
    solver: &CustomOptimizerSolverConfig,
    field_method: InverseLutContinuityFieldMethod,
) -> Result<(), InverseLutIdentityError> {
    let independent = matches!(
        field_method,
        InverseLutContinuityFieldMethod::IndependentNodeSolvesV1
    );
    match (solver.method, solver.continuity_preference) {
        (CustomOptimizerSolverMethod::BoundedHaltonBeamV1, _) => {
            if independent {
                Ok(())
            } else {
                Err(InverseLutIdentityError::ContinuityFieldPolicyDoesNotMatchSolver)
            }
        }
        (CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2, Some(policy))
            if policy.weight == 0.0 =>
        {
            if independent {
                Ok(())
            } else {
                Err(InverseLutIdentityError::ContinuityFieldPolicyDoesNotMatchSolver)
            }
        }
        (CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2, Some(_)) => {
            if matches!(
                field_method,
                InverseLutContinuityFieldMethod::JacobiSixNeighborV1 { .. }
            ) {
                Ok(())
            } else {
                Err(InverseLutIdentityError::PositiveContinuityRequiresFieldConstruction)
            }
        }
        (CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2, None) => {
            Err(InverseLutIdentityError::MissingSolverConfig)
        }
    }
}

pub fn quantize_normalized_coverage(
    value: f32,
    bit_depth: u8,
    method: InverseLutOutputQuantization,
) -> Result<u16, String> {
    if !value.is_finite() {
        return Err("Cannot quantize a non-finite normalized coverage.".to_owned());
    }
    let maximum = match bit_depth {
        8 => 255.0f32,
        16 => 65_535.0f32,
        other => return Err(format!("Unsupported inverse LUT output bit depth {other}.")),
    };
    match method {
        InverseLutOutputQuantization::ClampScaleRoundV1 => {
            Ok((value.clamp(0.0, 1.0) * maximum).round() as u16)
        }
    }
}

fn validate_forward_model_config(
    config: InverseLutLocalForwardModelConfigIdentity,
    errors: &mut Vec<String>,
) {
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
            "Inverse LUT forward-model support distance must be finite and in (0, 1].".to_owned(),
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
    value.strip_prefix("sha256:").is_some_and(is_bare_sha256)
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
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionRenderingIntent, ConversionTargetDefinition,
        SeparationStrategy, TargetChannelDefinition,
    };
    use crate::custom_optimizer_config::{ContinuityDistanceMetric, ContinuityPreferenceConfig};
    use crate::device_characterization_model::ForwardModelValidationPolicy;
    use crate::device_characterization_package::{
        CharacterizationMeasurementMetadata, CharacterizationPackage, CharacterizationPayload,
        CharacterizationProductionContext, CharacterizationSample, CharacterizationValidationLevel,
        MeasuredLabColor,
    };
    use crate::model::IccProfileIdentity;

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
            validity_encoding: InverseLutValidityEncoding::ExplicitNodeValidityMaskV1,
            numerical_precision: InverseLutNumericalPrecision::NormalizedF32V1,
            output_quantization: InverseLutOutputQuantization::ClampScaleRoundV1,
            continuity_field: InverseLutContinuityFieldMethod::IndependentNodeSolvesV1,
        }
    }

    fn jacobi_policy() -> InverseLutBuildPolicy {
        let mut value = policy();
        value.continuity_field = InverseLutContinuityFieldMethod::JacobiSixNeighborV1 {
            seed_method: InverseLutContinuitySeedMethod::IndependentV1NodeSolveV1,
            iterations: 16,
            self_weight: 0.35,
        };
        value
    }

    fn record() -> InverseLutIdentityRecord {
        InverseLutIdentityRecord {
            schema_version: INVERSE_LUT_IDENTITY_SCHEMA_VERSION,
            characterization_id: format!("sha256:{}", "a".repeat(64)),
            forward_model: InverseLutForwardModelIdentity {
                method: InverseLutForwardModelMethod::LocalInverseDistanceWeightedV1,
                config: InverseLutLocalForwardModelConfigIdentity {
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

    fn integration_package()
    -> crate::device_characterization_package::ValidatedCharacterizationPackage {
        let mut samples = Vec::new();
        for blue in [0.0f32, 0.5] {
            for brown in [0.0f32, 0.5] {
                for beige in [0.0f32, 0.5] {
                    for black in [0.0f32, 0.5] {
                        let coverages = vec![blue, brown, beige, black];
                        let lab = MeasuredLabColor {
                            l: 95.0
                                - 20.0 * f64::from(blue)
                                - 16.0 * f64::from(brown)
                                - 10.0 * f64::from(beige)
                                - 42.0 * f64::from(black),
                            a: -3.0 * f64::from(blue)
                                + 7.0 * f64::from(brown)
                                + 2.0 * f64::from(beige),
                            b: -12.0 * f64::from(blue)
                                + 8.0 * f64::from(brown)
                                + 4.0 * f64::from(beige),
                        };
                        samples.push(CharacterizationSample { coverages, lab });
                    }
                }
            }
        }
        CharacterizationPackage::new(CharacterizationPayload {
            revision: "lut-identity-fixture-v1".to_owned(),
            validation_level: CharacterizationValidationLevel::ProductionValidated,
            output_bit_depth: 16,
            channel_names: ["Blue", "Brown", "Beige", "Black"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            measured_channel_max_coverage: vec![0.5; 4],
            measured_total_ink_limit: 2.0,
            production_context: CharacterizationProductionContext {
                machine_id: "fixture-machine".to_owned(),
                rip_name: "fixture-rip".to_owned(),
                rip_version: "1".to_owned(),
                linearization_id: "fixture-linearization".to_owned(),
                substrate: "fixture-substrate".to_owned(),
                glaze: None,
                body: None,
                product_family: None,
            },
            measurement: CharacterizationMeasurementMetadata {
                instrument_model: "fixture-instrument".to_owned(),
                instrument_serial: None,
                illuminant: "D50".to_owned(),
                observer: "2deg".to_owned(),
                measurement_condition: "M1".to_owned(),
                measured_at_unix_ms: None,
                operator_or_lab: None,
            },
            samples,
        })
        .unwrap()
        .validated()
        .unwrap()
    }

    fn integration_model(
        package: &crate::device_characterization_package::ValidatedCharacterizationPackage,
    ) -> ValidatedLocalForwardModel {
        ValidatedLocalForwardModel::build(
            package,
            LocalForwardModelConfig {
                neighbor_count: 4,
                distance_power: 2.0,
                max_support_distance: 1.0,
            },
            ForwardModelValidationPolicy {
                max_mean_delta_e00: 100.0,
                max_p95_delta_e00: 100.0,
                max_delta_e00: 100.0,
                max_unsupported_fraction: 0.0,
            },
        )
        .unwrap()
    }

    fn integration_recipe(characterization_id: String) -> ConversionRecipe {
        ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: IccProfileIdentity {
                description: "Fixture source".to_owned(),
                sha256: "fixture-source-hash".to_owned(),
            },
            target: ConversionTargetDefinition {
                name: "Fixture 4C".to_owned(),
                channels: ["Blue", "Brown", "Beige", "Black"]
                    .into_iter()
                    .map(|name| TargetChannelDefinition {
                        name: name.to_owned(),
                        display_rgb: None,
                        solidity: 1.0,
                        max_coverage: Some(0.5),
                    })
                    .collect(),
                bit_depth: 16,
                output_profile_identity: None,
                output_profile_path: None,
                device_link_identity: None,
                device_link_path: None,
                characterization_id: Some(characterization_id),
                total_ink_limit: Some(2.0),
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: Some(CustomOptimizerSolverConfig::default()),
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
    fn local_model_recipe_constructor_binds_real_content_address_and_model_config() {
        let package = integration_package();
        let model = integration_model(&package);
        let recipe = integration_recipe(package.package().id.clone());
        let first = InverseLutIdentityRecord::from_local_model(&recipe, &model, policy())
            .expect("valid LUT identity");
        let second = InverseLutIdentityRecord::from_local_model(&recipe, &model, policy())
            .expect("repeat identity");
        assert_eq!(first.characterization_id, package.package().id);
        assert_eq!(first.forward_model.config.runtime_config(), model.config());
        assert_eq!(first.recipe_sha256, recipe_sha256(&recipe).unwrap());
        assert_eq!(first.content_id().unwrap(), second.content_id().unwrap());
    }

    #[test]
    fn local_model_constructor_fails_closed_on_model_identity_mismatch() {
        let package = integration_package();
        let model = integration_model(&package);
        let recipe = integration_recipe(format!("sha256:{}", "e".repeat(64)));
        let error = InverseLutIdentityRecord::from_local_model(&recipe, &model, policy())
            .expect_err("mismatched model identity must fail");
        assert!(matches!(
            error,
            InverseLutIdentityError::ModelIdentityMismatch { .. }
        ));
    }

    #[test]
    fn local_model_constructor_rejects_positive_v2_until_continuity_field_exists() {
        let package = integration_package();
        let model = integration_model(&package);
        let mut recipe = integration_recipe(package.package().id.clone());
        recipe.custom_optimizer_solver = Some(v2(1.0));
        recipe.validate().expect("V2 recipe itself is valid");
        assert_eq!(
            InverseLutIdentityRecord::from_local_model(&recipe, &model, policy()),
            Err(InverseLutIdentityError::PositiveContinuityRequiresFieldConstruction)
        );
        recipe.custom_optimizer_solver = Some(v2(0.0));
        assert!(InverseLutIdentityRecord::from_local_model(&recipe, &model, policy()).is_ok());
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
        assert!(
            errors
                .iter()
                .any(|error| error.contains("characterization_id"))
        );
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
        assert!(
            errors
                .iter()
                .any(|error| error.contains("maximum bounded grid"))
        );
    }

    #[test]
    fn positive_v2_continuity_fails_closed_until_field_construction_is_versioned() {
        assert_eq!(
            validate_solver_lut_semantics(
                &v2(1.0),
                InverseLutContinuityFieldMethod::IndependentNodeSolvesV1,
            ),
            Err(InverseLutIdentityError::PositiveContinuityRequiresFieldConstruction)
        );
        assert!(
            validate_solver_lut_semantics(
                &v2(0.0),
                InverseLutContinuityFieldMethod::IndependentNodeSolvesV1,
            )
            .is_ok()
        );
        assert!(
            validate_solver_lut_semantics(
                &CustomOptimizerSolverConfig::default(),
                InverseLutContinuityFieldMethod::IndependentNodeSolvesV1,
            )
            .is_ok()
        );
    }

    #[test]
    fn unknown_identity_or_model_config_fields_fail_deserialization() {
        let mut value = serde_json::to_value(record()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("future_identity_semantics".to_owned(), serde_json::json!(1));
        assert!(serde_json::from_value::<InverseLutIdentityRecord>(value).is_err());

        let mut value = serde_json::to_value(record()).unwrap();
        value["forward_model"]["config"]
            .as_object_mut()
            .unwrap()
            .insert("future_model_knob".to_owned(), serde_json::json!(0.5));
        assert!(serde_json::from_value::<InverseLutIdentityRecord>(value).is_err());
    }

    #[test]
    fn output_quantization_is_explicit_clamped_and_deterministic() {
        let method = InverseLutOutputQuantization::ClampScaleRoundV1;
        assert_eq!(quantize_normalized_coverage(-0.2, 8, method).unwrap(), 0);
        assert_eq!(quantize_normalized_coverage(0.5, 8, method).unwrap(), 128);
        assert_eq!(quantize_normalized_coverage(1.2, 8, method).unwrap(), 255);
        assert_eq!(
            quantize_normalized_coverage(0.5, 16, method).unwrap(),
            32_768
        );
        assert_eq!(
            quantize_normalized_coverage(1.0, 16, method).unwrap(),
            65_535
        );
        assert!(quantize_normalized_coverage(f32::NAN, 16, method).is_err());
        assert!(quantize_normalized_coverage(0.5, 12, method).is_err());
    }

    #[test]
    fn non_finite_model_or_grid_values_fail_closed() {
        let mut invalid = record();
        invalid.forward_model.config.distance_power = f64::NAN;
        invalid.build_policy.grid.a_max = f64::INFINITY;
        let errors = invalid
            .validate()
            .expect_err("non-finite identity must fail");
        assert!(errors.iter().any(|error| error.contains("distance power")));
        assert!(errors.iter().any(|error| error.contains("a* maximum")));
    }
    #[test]
    fn positive_v2_accepts_versioned_jacobi_field_policy() {
        let package = integration_package();
        let model = integration_model(&package);
        let mut recipe = integration_recipe(package.package().id.clone());
        recipe.custom_optimizer_solver = Some(v2(1.0));
        let identity = InverseLutIdentityRecord::from_local_model(&recipe, &model, jacobi_policy())
            .expect("positive V2 must bind to the versioned Jacobi field policy");
        assert!(matches!(
            identity.build_policy.continuity_field,
            InverseLutContinuityFieldMethod::JacobiSixNeighborV1 { .. }
        ));
    }

    #[test]
    fn jacobi_field_policy_is_rejected_for_v1_and_zero_weight_v2() {
        let package = integration_package();
        let model = integration_model(&package);
        let mut recipe = integration_recipe(package.package().id.clone());
        assert_eq!(
            InverseLutIdentityRecord::from_local_model(&recipe, &model, jacobi_policy()),
            Err(InverseLutIdentityError::ContinuityFieldPolicyDoesNotMatchSolver)
        );
        recipe.custom_optimizer_solver = Some(v2(0.0));
        assert_eq!(
            InverseLutIdentityRecord::from_local_model(&recipe, &model, jacobi_policy()),
            Err(InverseLutIdentityError::ContinuityFieldPolicyDoesNotMatchSolver)
        );
    }

    #[test]
    fn jacobi_field_parameters_participate_in_content_address() {
        let mut first = record();
        first.build_policy = jacobi_policy();
        let first_id = first.content_id().unwrap();
        let mut second = first.clone();
        if let InverseLutContinuityFieldMethod::JacobiSixNeighborV1 {
            ref mut iterations, ..
        } = second.build_policy.continuity_field
        {
            *iterations += 1;
        }
        assert_ne!(first_id, second.content_id().unwrap());
    }

    #[test]
    fn jacobi_field_policy_has_bounded_parameters_and_work() {
        let mut invalid = jacobi_policy();
        invalid.continuity_field = InverseLutContinuityFieldMethod::JacobiSixNeighborV1 {
            seed_method: InverseLutContinuitySeedMethod::IndependentV1NodeSolveV1,
            iterations: 0,
            self_weight: f32::NAN,
        };
        let errors = invalid
            .validate()
            .expect_err("invalid Jacobi policy must fail");
        assert!(errors.iter().any(|error| error.contains("iterations")));
        assert!(errors.iter().any(|error| error.contains("self_weight")));

        let mut oversized = jacobi_policy();
        oversized.grid.l_samples = 100;
        oversized.grid.a_samples = 50;
        oversized.grid.b_samples = 50;
        oversized.continuity_field = InverseLutContinuityFieldMethod::JacobiSixNeighborV1 {
            seed_method: InverseLutContinuitySeedMethod::IndependentV1NodeSolveV1,
            iterations: 16,
            self_weight: 0.35,
        };
        let errors = oversized
            .validate()
            .expect_err("Jacobi work budget must fail");
        assert!(errors.iter().any(|error| error.contains("node solves")));
    }
}
