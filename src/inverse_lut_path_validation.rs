use serde::{Deserialize, Serialize};

use crate::color_conversion::ConversionRecipe;
use crate::device_characterization::{DeviceForwardModel, evaluate_characterized_color};
use crate::gradient_continuity::{
    GradientContinuityPolicy, GradientSampleDiagnostic, build_report,
};
use crate::gradient_validation::{
    GradientCurvaturePolicy, analyze_gradient_curvature,
};
use crate::inverse_lut_holdout::{InverseLutHoldoutPath, InverseLutHoldoutPathKind};
use crate::inverse_lut_runtime::{InverseLutLookupError, InverseLutRuntime};
use crate::inverse_lut_validation_reference::{
    InverseLutValidationReferenceError, validation_reference_method,
};
use crate::inverse_separation_solver::InverseSolverStats;

pub const INVERSE_LUT_PATH_VALIDATION_POLICY_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutPathValidationPolicy {
    pub schema_version: u32,
    pub max_channel_jump: f32,
    pub max_normalized_channel_jump: f32,
    pub max_vector_l1_jump: f32,
    pub max_vector_l2_jump: f32,
    pub max_total_ink_jump: f32,
    pub max_dominant_channel_switches_per_path: u64,
    pub max_channel_second_difference: f32,
    pub max_normalized_channel_second_difference: f32,
    pub max_vector_l1_second_difference: f32,
    pub max_vector_l2_second_difference: f32,
    pub max_total_ink_second_difference: f32,
}

impl Default for InverseLutPathValidationPolicy {
    fn default() -> Self {
        Self {
            schema_version: INVERSE_LUT_PATH_VALIDATION_POLICY_SCHEMA_VERSION,
            // Provisional production gates. #190 must calibrate/freeze these
            // against measured fixtures before the raster worker can consume a
            // passing report.
            max_channel_jump: 0.20,
            max_normalized_channel_jump: 0.25,
            max_vector_l1_jump: 0.50,
            max_vector_l2_jump: 0.30,
            max_total_ink_jump: 0.40,
            max_dominant_channel_switches_per_path: 8,
            max_channel_second_difference: 0.15,
            max_normalized_channel_second_difference: 0.20,
            max_vector_l1_second_difference: 0.35,
            max_vector_l2_second_difference: 0.25,
            max_total_ink_second_difference: 0.30,
        }
    }
}

impl InverseLutPathValidationPolicy {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != INVERSE_LUT_PATH_VALIDATION_POLICY_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported inverse-LUT path-validation policy schema {} (expected {}).",
                self.schema_version, INVERSE_LUT_PATH_VALIDATION_POLICY_SCHEMA_VERSION
            ));
        }
        for (name, value) in [
            ("max_channel_jump", self.max_channel_jump),
            ("max_normalized_channel_jump", self.max_normalized_channel_jump),
            ("max_vector_l1_jump", self.max_vector_l1_jump),
            ("max_vector_l2_jump", self.max_vector_l2_jump),
            ("max_total_ink_jump", self.max_total_ink_jump),
            (
                "max_channel_second_difference",
                self.max_channel_second_difference,
            ),
            (
                "max_normalized_channel_second_difference",
                self.max_normalized_channel_second_difference,
            ),
            (
                "max_vector_l1_second_difference",
                self.max_vector_l1_second_difference,
            ),
            (
                "max_vector_l2_second_difference",
                self.max_vector_l2_second_difference,
            ),
            (
                "max_total_ink_second_difference",
                self.max_total_ink_second_difference,
            ),
        ] {
            if !value.is_finite() || value < 0.0 {
                errors.push(format!(
                    "Inverse-LUT path-validation {name} must be finite and >= 0."
                ));
            }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }

    fn continuity_policy(self) -> GradientContinuityPolicy {
        GradientContinuityPolicy {
            max_channel_jump: self.max_channel_jump,
            max_normalized_channel_jump: self.max_normalized_channel_jump,
            max_vector_l1_jump: self.max_vector_l1_jump,
            max_vector_l2_jump: self.max_vector_l2_jump,
            max_total_ink_jump: self.max_total_ink_jump,
        }
    }

    fn curvature_policy(self) -> GradientCurvaturePolicy {
        GradientCurvaturePolicy {
            max_channel_second_difference: self.max_channel_second_difference,
            max_normalized_channel_second_difference: self
                .max_normalized_channel_second_difference,
            max_vector_l1_second_difference: self.max_vector_l1_second_difference,
            max_vector_l2_second_difference: self.max_vector_l2_second_difference,
            max_total_ink_second_difference: self.max_total_ink_second_difference,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InverseLutValidationPathKind {
    NeutralAxis,
    NearNeutralWarm,
    NearNeutralCool,
    AAxis,
    BAxis,
    AbDiagonal,
    AbOpposedDiagonal,
}

impl From<InverseLutHoldoutPathKind> for InverseLutValidationPathKind {
    fn from(value: InverseLutHoldoutPathKind) -> Self {
        match value {
            InverseLutHoldoutPathKind::NeutralAxis => Self::NeutralAxis,
            InverseLutHoldoutPathKind::NearNeutralWarm => Self::NearNeutralWarm,
            InverseLutHoldoutPathKind::NearNeutralCool => Self::NearNeutralCool,
            InverseLutHoldoutPathKind::AAxis => Self::AAxis,
            InverseLutHoldoutPathKind::BAxis => Self::BAxis,
            InverseLutHoldoutPathKind::AbDiagonal => Self::AbDiagonal,
            InverseLutHoldoutPathKind::AbOpposedDiagonal => Self::AbOpposedDiagonal,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutPathDiagnostic {
    pub kind: InverseLutValidationPathKind,
    pub sample_count: u64,
    pub unsupported_samples: u64,
    pub max_channel_jump: Option<f64>,
    pub max_normalized_channel_jump: Option<f64>,
    pub max_vector_l1_jump: Option<f64>,
    pub max_vector_l2_jump: Option<f64>,
    pub max_total_ink_jump: Option<f64>,
    pub dominant_channel_switches: Option<u64>,
    pub max_channel_second_difference: Option<f64>,
    pub max_normalized_channel_second_difference: Option<f64>,
    pub max_vector_l1_second_difference: Option<f64>,
    pub max_vector_l2_second_difference: Option<f64>,
    pub max_total_ink_second_difference: Option<f64>,
    pub continuity_violation_count: Option<u64>,
    pub curvature_violation_count: Option<u64>,
}

impl InverseLutPathDiagnostic {
    pub fn validate(&self) -> Result<(), String> {
        if self.sample_count == 0 {
            return Err("Inverse-LUT path diagnostic cannot be empty.".to_owned());
        }
        if self.unsupported_samples > self.sample_count {
            return Err("Inverse-LUT path unsupported count exceeds sample count.".to_owned());
        }
        let numeric = [
            self.max_channel_jump,
            self.max_normalized_channel_jump,
            self.max_vector_l1_jump,
            self.max_vector_l2_jump,
            self.max_total_ink_jump,
            self.max_channel_second_difference,
            self.max_normalized_channel_second_difference,
            self.max_vector_l1_second_difference,
            self.max_vector_l2_second_difference,
            self.max_total_ink_second_difference,
        ];
        if self.unsupported_samples > 0 {
            if numeric.iter().any(Option::is_some)
                || self.dominant_channel_switches.is_some()
                || self.continuity_violation_count.is_some()
                || self.curvature_violation_count.is_some()
            {
                return Err(
                    "Unsupported inverse-LUT path must not bridge samples or carry continuity metrics."
                        .to_owned(),
                );
            }
            return Ok(());
        }
        if numeric
            .iter()
            .any(|value| !matches!(value, Some(value) if value.is_finite() && *value >= 0.0))
            || self.dominant_channel_switches.is_none()
            || self.continuity_violation_count.is_none()
            || self.curvature_violation_count.is_none()
        {
            return Err(
                "Supported inverse-LUT path requires complete finite non-negative diagnostics."
                    .to_owned(),
            );
        }
        Ok(())
    }

    pub fn passes(&self, policy: &InverseLutPathValidationPolicy) -> bool {
        if self.validate().is_err() || policy.validate().is_err() || self.unsupported_samples != 0 {
            return false;
        }
        self.max_channel_jump.unwrap_or(f64::INFINITY) <= f64::from(policy.max_channel_jump)
            && self.max_normalized_channel_jump.unwrap_or(f64::INFINITY)
                <= f64::from(policy.max_normalized_channel_jump)
            && self.max_vector_l1_jump.unwrap_or(f64::INFINITY)
                <= f64::from(policy.max_vector_l1_jump)
            && self.max_vector_l2_jump.unwrap_or(f64::INFINITY)
                <= f64::from(policy.max_vector_l2_jump)
            && self.max_total_ink_jump.unwrap_or(f64::INFINITY)
                <= f64::from(policy.max_total_ink_jump)
            && self.dominant_channel_switches.unwrap_or(u64::MAX)
                <= policy.max_dominant_channel_switches_per_path
            && self.max_channel_second_difference.unwrap_or(f64::INFINITY)
                <= f64::from(policy.max_channel_second_difference)
            && self
                .max_normalized_channel_second_difference
                .unwrap_or(f64::INFINITY)
                <= f64::from(policy.max_normalized_channel_second_difference)
            && self.max_vector_l1_second_difference.unwrap_or(f64::INFINITY)
                <= f64::from(policy.max_vector_l1_second_difference)
            && self.max_vector_l2_second_difference.unwrap_or(f64::INFINITY)
                <= f64::from(policy.max_vector_l2_second_difference)
            && self.max_total_ink_second_difference.unwrap_or(f64::INFINITY)
                <= f64::from(policy.max_total_ink_second_difference)
            && self.continuity_violation_count == Some(0)
            && self.curvature_violation_count == Some(0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum InverseLutPathValidationError {
    InvalidPolicy(Vec<String>),
    ReferenceContract(InverseLutValidationReferenceError),
    CharacterizationIdentityMismatch { expected: String, actual: String },
    ChannelTopologyMismatch { expected: Vec<String>, actual: Vec<String> },
    EmptyPath { path_index: usize },
    Lookup {
        path_index: usize,
        sample_index: usize,
        error: InverseLutLookupError,
    },
    ForwardModel {
        path_index: usize,
        sample_index: usize,
        error: String,
    },
    InvalidDeltaE {
        path_index: usize,
        sample_index: usize,
        value: f64,
    },
    Curvature {
        path_index: usize,
        error: String,
    },
    CountOverflow,
}

pub fn analyze_inverse_lut_paths(
    runtime: &InverseLutRuntime,
    recipe: &ConversionRecipe,
    model: &dyn DeviceForwardModel,
    paths: &[InverseLutHoldoutPath],
    policy: InverseLutPathValidationPolicy,
) -> Result<Vec<InverseLutPathDiagnostic>, InverseLutPathValidationError> {
    policy
        .validate()
        .map_err(InverseLutPathValidationError::InvalidPolicy)?;
    validation_reference_method(runtime, recipe)
        .map_err(InverseLutPathValidationError::ReferenceContract)?;
    if runtime.identity().characterization_id != model.identity().id {
        return Err(
            InverseLutPathValidationError::CharacterizationIdentityMismatch {
                expected: runtime.identity().characterization_id.clone(),
                actual: model.identity().id.clone(),
            },
        );
    }
    if runtime.identity().channel_names != model.identity().channel_names {
        return Err(InverseLutPathValidationError::ChannelTopologyMismatch {
            expected: runtime.identity().channel_names.clone(),
            actual: model.identity().channel_names.clone(),
        });
    }

    let continuity_policy = policy.continuity_policy();
    let curvature_policy = policy.curvature_policy();
    let mut output = Vec::with_capacity(paths.len());
    for (path_index, path) in paths.iter().enumerate() {
        if path.samples.is_empty() {
            return Err(InverseLutPathValidationError::EmptyPath { path_index });
        }
        let mut unsupported_samples = 0u64;
        let mut samples = Vec::with_capacity(path.samples.len());
        for (sample_index, target_lab) in path.samples.iter().copied().enumerate() {
            let coverages = match runtime.lookup(target_lab) {
                Ok(values) => values,
                Err(InverseLutLookupError::OutOfDomain { .. })
                | Err(InverseLutLookupError::UnsupportedCorner { .. }) => {
                    unsupported_samples = unsupported_samples
                        .checked_add(1)
                        .ok_or(InverseLutPathValidationError::CountOverflow)?;
                    continue;
                }
                Err(error) => {
                    return Err(InverseLutPathValidationError::Lookup {
                        path_index,
                        sample_index,
                        error,
                    });
                }
            };
            let color = evaluate_characterized_color(model, target_lab, &coverages).map_err(
                |error| InverseLutPathValidationError::ForwardModel {
                    path_index,
                    sample_index,
                    error,
                },
            )?;
            if !color.delta_e00.is_finite() || color.delta_e00 > f64::from(f32::MAX) {
                return Err(InverseLutPathValidationError::InvalidDeltaE {
                    path_index,
                    sample_index,
                    value: color.delta_e00,
                });
            }
            let total_ink = coverages.iter().copied().sum::<f32>();
            samples.push(GradientSampleDiagnostic {
                index: sample_index,
                target_lab,
                coverages,
                delta_e00: color.delta_e00 as f32,
                total_ink,
                solver_stats: InverseSolverStats::default(),
            });
        }

        let sample_count = u64::try_from(path.samples.len())
            .map_err(|_| InverseLutPathValidationError::CountOverflow)?;
        if unsupported_samples > 0 {
            output.push(unsupported_path(path.kind.into(), sample_count, unsupported_samples));
            continue;
        }

        let continuity = build_report(
            &recipe.target,
            &recipe.strategy,
            samples,
            &continuity_policy,
        );
        let curvature = analyze_gradient_curvature(
            &recipe.target,
            &recipe.strategy,
            &continuity,
            &curvature_policy,
        )
        .map_err(|error| InverseLutPathValidationError::Curvature {
            path_index,
            error: format!("{error:?}"),
        })?;

        output.push(InverseLutPathDiagnostic {
            kind: path.kind.into(),
            sample_count,
            unsupported_samples: 0,
            max_channel_jump: Some(f64::from(continuity.max_channel_jump)),
            max_normalized_channel_jump: Some(f64::from(
                continuity.max_normalized_channel_jump,
            )),
            max_vector_l1_jump: Some(f64::from(continuity.max_vector_l1_jump)),
            max_vector_l2_jump: Some(f64::from(continuity.max_vector_l2_jump)),
            max_total_ink_jump: Some(f64::from(continuity.max_total_ink_jump)),
            dominant_channel_switches: Some(
                u64::try_from(continuity.dominant_channel_switches)
                    .map_err(|_| InverseLutPathValidationError::CountOverflow)?,
            ),
            max_channel_second_difference: Some(f64::from(
                curvature.max_channel_second_difference,
            )),
            max_normalized_channel_second_difference: Some(f64::from(
                curvature.max_normalized_channel_second_difference,
            )),
            max_vector_l1_second_difference: Some(f64::from(
                curvature.max_vector_l1_second_difference,
            )),
            max_vector_l2_second_difference: Some(f64::from(
                curvature.max_vector_l2_second_difference,
            )),
            max_total_ink_second_difference: Some(f64::from(
                curvature.max_total_ink_second_difference,
            )),
            continuity_violation_count: Some(
                u64::try_from(continuity.violation_count)
                    .map_err(|_| InverseLutPathValidationError::CountOverflow)?,
            ),
            curvature_violation_count: Some(
                u64::try_from(curvature.violation_count)
                    .map_err(|_| InverseLutPathValidationError::CountOverflow)?,
            ),
        });
    }
    Ok(output)
}

pub fn path_diagnostics_pass(
    diagnostics: &[InverseLutPathDiagnostic],
    policy: &InverseLutPathValidationPolicy,
) -> bool {
    !diagnostics.is_empty()
        && policy.validate().is_ok()
        && diagnostics.iter().all(|path| path.passes(policy))
}

fn unsupported_path(
    kind: InverseLutValidationPathKind,
    sample_count: u64,
    unsupported_samples: u64,
) -> InverseLutPathDiagnostic {
    InverseLutPathDiagnostic {
        kind,
        sample_count,
        unsupported_samples,
        max_channel_jump: None,
        max_normalized_channel_jump: None,
        max_vector_l1_jump: None,
        max_vector_l2_jump: None,
        max_total_ink_jump: None,
        dominant_channel_switches: None,
        max_channel_second_difference: None,
        max_normalized_channel_second_difference: None,
        max_vector_l1_second_difference: None,
        max_vector_l2_second_difference: None,
        max_total_ink_second_difference: None,
        continuity_violation_count: None,
        curvature_violation_count: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_path_cannot_carry_bridged_metrics() {
        let path = unsupported_path(InverseLutValidationPathKind::NeutralAxis, 5, 1);
        assert!(path.validate().is_ok());
        assert!(!path.passes(&InverseLutPathValidationPolicy::default()));

        let mut tampered = path;
        tampered.max_channel_jump = Some(0.0);
        assert!(tampered.validate().is_err());
    }

    #[test]
    fn policy_rejects_non_finite_or_negative_thresholds() {
        let mut policy = InverseLutPathValidationPolicy::default();
        policy.max_vector_l1_jump = f32::NAN;
        assert!(policy.validate().is_err());

        let mut policy = InverseLutPathValidationPolicy::default();
        policy.max_total_ink_second_difference = -0.1;
        assert!(policy.validate().is_err());
    }
}
