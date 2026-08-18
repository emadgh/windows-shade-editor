use crate::color_conversion::{ConversionEngineMode, ConversionRecipe};
use crate::conversion_recipe::recipe_sha256;
use crate::custom_optimizer_config::{
    CustomOptimizerObjectiveWeights, CustomOptimizerSolverConfig, CustomOptimizerSolverMethod,
};
use crate::device_characterization::{
    DeviceForwardModel, LabColor, delta_e_2000, evaluate_characterized_color,
};
use crate::inverse_lut_identity::quantize_normalized_coverage;
use crate::inverse_lut_runtime::{InverseLutLookupError, InverseLutRuntime};
use crate::inverse_lut_validation::InverseLutValidationSample;
use crate::inverse_separation_solver::{InverseSolveError, solve_inverse_separation};
use crate::separation_optimizer::{
    CandidateScoringWeights, characterize_candidate, evaluate_candidate,
};

#[derive(Clone, Debug, PartialEq)]
pub enum InverseLutValidationEvaluationError {
    InvalidRecipe(Vec<String>),
    NotCustomOptimizerRecipe,
    MissingSolverConfig,
    MissingObjectiveWeights(Vec<String>),
    RecipeIdentityMismatch {
        expected: String,
        actual: String,
    },
    CharacterizationIdentityMismatch {
        expected: String,
        actual: String,
    },
    ChannelTopologyMismatch {
        expected: Vec<String>,
        actual: Vec<String>,
    },
    TargetBitDepthMismatch {
        expected: u8,
        actual: u8,
    },
    PositiveContinuityRequiresFrozenFieldReference,
    ReferenceSolve(InverseSolveError),
    ReferenceTopologyMismatch {
        expected: usize,
        actual: usize,
    },
    InvalidReferenceCoverage {
        channel_index: usize,
        value: f32,
    },
    LutLookup(InverseLutLookupError),
    ForwardModel(String),
    Quantization(String),
}

/// Produce an authoritative reference separation for validation only when the
/// persisted solver semantics are independent at each Lab point.
///
/// Positive-weight V2 deliberately fails here. Its production identity is the
/// frozen offline Jacobi continuity field; inventing a previous-pixel/raster
/// reference for an arbitrary holdout would change the method being validated.
pub fn solve_independent_validation_reference(
    recipe: &ConversionRecipe,
    model: &dyn DeviceForwardModel,
    target_lab: LabColor,
) -> Result<Option<Vec<f32>>, InverseLutValidationEvaluationError> {
    validate_recipe_and_model(recipe, model)?;
    let solver = recipe
        .custom_optimizer_solver
        .ok_or(InverseLutValidationEvaluationError::MissingSolverConfig)?;
    if !solver_has_independent_point_semantics(solver) {
        return Err(
            InverseLutValidationEvaluationError::PositiveContinuityRequiresFrozenFieldReference,
        );
    }
    let weights = scoring_weights(solver, recipe.target.channels.len())?;
    match solve_inverse_separation(
        &recipe.target,
        &recipe.strategy,
        weights,
        model,
        target_lab,
        solver,
    ) {
        Ok(result) => Ok(Some(result.candidate.coverages)),
        Err(InverseSolveError::NoFeasibleCandidate) => Ok(None),
        Err(error) => Err(InverseLutValidationEvaluationError::ReferenceSolve(error)),
    }
}

/// Evaluate one deterministic holdout against a caller-supplied authoritative
/// reference separation. The caller owns how that reference is constructed.
/// This keeps positive-continuity V2 validation tied to its frozen field
/// semantics instead of silently substituting a traversal-dependent solver.
pub fn evaluate_inverse_lut_validation_sample(
    runtime: &InverseLutRuntime,
    recipe: &ConversionRecipe,
    model: &dyn DeviceForwardModel,
    target_lab: LabColor,
    reference_coverages: &[f32],
) -> Result<InverseLutValidationSample, InverseLutValidationEvaluationError> {
    validate_runtime_binding(runtime, recipe, model)?;
    validate_reference(reference_coverages, recipe.target.channels.len())?;

    let lut_coverages = match runtime.lookup(target_lab) {
        Ok(values) => values,
        Err(InverseLutLookupError::OutOfDomain { .. })
        | Err(InverseLutLookupError::UnsupportedCorner { .. }) => {
            return Ok(unsupported_sample());
        }
        Err(error) => return Err(InverseLutValidationEvaluationError::LutLookup(error)),
    };

    let lut_color = evaluate_characterized_color(model, target_lab, &lut_coverages)
        .map_err(InverseLutValidationEvaluationError::ForwardModel)?;
    let reference_color = evaluate_characterized_color(model, target_lab, reference_coverages)
        .map_err(InverseLutValidationEvaluationError::ForwardModel)?;
    let lut_vs_reference_delta_e00 =
        delta_e_2000(lut_color.predicted, reference_color.predicted);

    let (ink_l1, ink_l2, max_channel_deviation) =
        ink_deviation(&lut_coverages, reference_coverages)?;
    let quantization = runtime.identity().build_policy.output_quantization;
    let u8_quantization_l1 = quantization_l1(&lut_coverages, 8, quantization)?;
    let u16_quantization_l1 = quantization_l1(&lut_coverages, 16, quantization)?;

    let lut_candidate = characterize_candidate(
        &recipe.target,
        model,
        target_lab,
        lut_coverages,
    )
    .map_err(|error| InverseLutValidationEvaluationError::ForwardModel(format!("{error:?}")))?;
    let solver = recipe
        .custom_optimizer_solver
        .ok_or(InverseLutValidationEvaluationError::MissingSolverConfig)?;
    let weights = scoring_weights(solver, recipe.target.channels.len())?;
    let constraints_preserved =
        evaluate_candidate(&recipe.target, &recipe.strategy, weights, &lut_candidate).is_ok();

    Ok(InverseLutValidationSample {
        supported: true,
        lut_delta_e00: Some(lut_color.delta_e00),
        reference_delta_e00: Some(reference_color.delta_e00),
        lut_vs_reference_delta_e00: Some(lut_vs_reference_delta_e00),
        ink_l1: Some(ink_l1),
        ink_l2: Some(ink_l2),
        max_channel_deviation: Some(max_channel_deviation),
        u8_quantization_l1: Some(u8_quantization_l1),
        u16_quantization_l1: Some(u16_quantization_l1),
        constraints_preserved,
    })
}

fn validate_runtime_binding(
    runtime: &InverseLutRuntime,
    recipe: &ConversionRecipe,
    model: &dyn DeviceForwardModel,
) -> Result<(), InverseLutValidationEvaluationError> {
    validate_recipe_and_model(recipe, model)?;
    let identity = runtime.identity();
    let actual_recipe_sha = recipe_sha256(recipe)
        .map_err(|error| InverseLutValidationEvaluationError::ForwardModel(error.to_string()))?;
    if identity.recipe_sha256 != actual_recipe_sha {
        return Err(InverseLutValidationEvaluationError::RecipeIdentityMismatch {
            expected: identity.recipe_sha256.clone(),
            actual: actual_recipe_sha,
        });
    }
    if identity.characterization_id != model.identity().id {
        return Err(
            InverseLutValidationEvaluationError::CharacterizationIdentityMismatch {
                expected: identity.characterization_id.clone(),
                actual: model.identity().id.clone(),
            },
        );
    }
    if identity.channel_names != model.identity().channel_names {
        return Err(InverseLutValidationEvaluationError::ChannelTopologyMismatch {
            expected: identity.channel_names.clone(),
            actual: model.identity().channel_names.clone(),
        });
    }
    if identity.target_bit_depth != recipe.target.bit_depth {
        return Err(InverseLutValidationEvaluationError::TargetBitDepthMismatch {
            expected: identity.target_bit_depth,
            actual: recipe.target.bit_depth,
        });
    }
    Ok(())
}

fn validate_recipe_and_model(
    recipe: &ConversionRecipe,
    model: &dyn DeviceForwardModel,
) -> Result<(), InverseLutValidationEvaluationError> {
    recipe
        .validate()
        .map_err(InverseLutValidationEvaluationError::InvalidRecipe)?;
    if recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
        return Err(InverseLutValidationEvaluationError::NotCustomOptimizerRecipe);
    }
    let expected_characterization = recipe
        .target
        .characterization_id
        .as_deref()
        .unwrap_or_default();
    if expected_characterization != model.identity().id {
        return Err(
            InverseLutValidationEvaluationError::CharacterizationIdentityMismatch {
                expected: expected_characterization.to_owned(),
                actual: model.identity().id.clone(),
            },
        );
    }
    let expected_channels = recipe
        .target
        .channels
        .iter()
        .map(|channel| channel.name.clone())
        .collect::<Vec<_>>();
    if expected_channels != model.identity().channel_names {
        return Err(InverseLutValidationEvaluationError::ChannelTopologyMismatch {
            expected: expected_channels,
            actual: model.identity().channel_names.clone(),
        });
    }
    Ok(())
}

fn scoring_weights(
    solver: CustomOptimizerSolverConfig,
    channel_count: usize,
) -> Result<CandidateScoringWeights, InverseLutValidationEvaluationError> {
    solver
        .validate(channel_count)
        .map_err(InverseLutValidationEvaluationError::MissingObjectiveWeights)?;
    let objective = solver.objective_weights.ok_or_else(|| {
        InverseLutValidationEvaluationError::MissingObjectiveWeights(vec![
            "Validation reference requires persisted production objective weights.".to_owned(),
        ])
    })?;
    objective
        .validate()
        .map_err(InverseLutValidationEvaluationError::MissingObjectiveWeights)?;
    Ok(weights_from_objective(objective))
}

fn weights_from_objective(objective: CustomOptimizerObjectiveWeights) -> CandidateScoringWeights {
    CandidateScoringWeights {
        color_error: objective.color_error,
        ink_preference: objective.ink_preference,
        neutral_black: objective.neutral_black,
        total_ink: objective.total_ink,
    }
}

fn solver_has_independent_point_semantics(solver: CustomOptimizerSolverConfig) -> bool {
    match (solver.method, solver.continuity_preference) {
        (CustomOptimizerSolverMethod::BoundedHaltonBeamV1, _) => true,
        (CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2, Some(policy)) => {
            policy.weight == 0.0
        }
        (CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2, None) => false,
    }
}

fn validate_reference(
    reference_coverages: &[f32],
    expected: usize,
) -> Result<(), InverseLutValidationEvaluationError> {
    if reference_coverages.len() != expected {
        return Err(InverseLutValidationEvaluationError::ReferenceTopologyMismatch {
            expected,
            actual: reference_coverages.len(),
        });
    }
    for (channel_index, value) in reference_coverages.iter().copied().enumerate() {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(InverseLutValidationEvaluationError::InvalidReferenceCoverage {
                channel_index,
                value,
            });
        }
    }
    Ok(())
}

fn ink_deviation(
    lut: &[f32],
    reference: &[f32],
) -> Result<(f64, f64, f64), InverseLutValidationEvaluationError> {
    if lut.len() != reference.len() {
        return Err(InverseLutValidationEvaluationError::ReferenceTopologyMismatch {
            expected: lut.len(),
            actual: reference.len(),
        });
    }
    let mut l1 = 0.0f64;
    let mut l2_squared = 0.0f64;
    let mut maximum = 0.0f64;
    for (left, right) in lut.iter().copied().zip(reference.iter().copied()) {
        let delta = f64::from((left - right).abs());
        l1 += delta;
        l2_squared += delta * delta;
        maximum = maximum.max(delta);
    }
    Ok((l1, l2_squared.sqrt(), maximum))
}

fn quantization_l1(
    coverages: &[f32],
    bit_depth: u8,
    method: crate::inverse_lut_identity::InverseLutOutputQuantization,
) -> Result<f64, InverseLutValidationEvaluationError> {
    let maximum = match bit_depth {
        8 => 255.0f64,
        16 => 65_535.0f64,
        other => {
            return Err(InverseLutValidationEvaluationError::Quantization(format!(
                "Unsupported validation quantization depth {other}."
            )));
        }
    };
    let mut l1 = 0.0f64;
    for value in coverages.iter().copied() {
        let quantized = quantize_normalized_coverage(value, bit_depth, method)
            .map_err(InverseLutValidationEvaluationError::Quantization)?;
        let restored = f64::from(quantized) / maximum;
        l1 += (f64::from(value) - restored).abs();
    }
    Ok(l1)
}

fn unsupported_sample() -> InverseLutValidationSample {
    InverseLutValidationSample {
        supported: false,
        lut_delta_e00: None,
        reference_delta_e00: None,
        lut_vs_reference_delta_e00: None,
        ink_l1: None,
        ink_l2: None,
        max_channel_deviation: None,
        u8_quantization_l1: None,
        u16_quantization_l1: None,
        constraints_preserved: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_optimizer_config::{
        ContinuityDistanceMetric, ContinuityPreferenceConfig,
    };
    use crate::inverse_lut_identity::InverseLutOutputQuantization;

    #[test]
    fn positive_v2_requires_frozen_field_reference() {
        let positive = CustomOptimizerSolverConfig {
            method: CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2,
            continuity_preference: Some(ContinuityPreferenceConfig {
                weight: 1.0,
                distance_metric: ContinuityDistanceMetric::NormalizedL2,
                max_normalized_channel_jump: 0.2,
                dominant_channel_switch_penalty: 0.25,
            }),
            ..CustomOptimizerSolverConfig::default()
        };
        assert!(!solver_has_independent_point_semantics(positive));

        let zero = CustomOptimizerSolverConfig {
            continuity_preference: Some(ContinuityPreferenceConfig {
                weight: 0.0,
                ..positive.continuity_preference.unwrap()
            }),
            ..positive
        };
        assert!(solver_has_independent_point_semantics(zero));
        assert!(solver_has_independent_point_semantics(
            CustomOptimizerSolverConfig::default()
        ));
    }

    #[test]
    fn ink_deviation_reports_l1_l2_and_max_channel() {
        let (l1, l2, maximum) = ink_deviation(&[0.1, 0.7, 0.2], &[0.2, 0.4, 0.2]).unwrap();
        assert!((l1 - 0.4).abs() < 1.0e-6);
        assert!((l2 - (0.1f64 * 0.1 + 0.3 * 0.3).sqrt()).abs() < 1.0e-6);
        assert!((maximum - 0.3).abs() < 1.0e-6);
    }

    #[test]
    fn quantization_sensitivity_uses_the_versioned_rounding_policy() {
        let values = [0.12345f32, 0.5, 0.98765];
        let u8 = quantization_l1(&values, 8, InverseLutOutputQuantization::ClampScaleRoundV1)
            .unwrap();
        let u16 = quantization_l1(
            &values,
            16,
            InverseLutOutputQuantization::ClampScaleRoundV1,
        )
        .unwrap();
        assert!(u8 > 0.0);
        assert!(u16 > 0.0);
        assert!(u16 < u8);
    }
}
