use serde::{Deserialize, Serialize};

use crate::color_conversion::{ConversionEngineMode, ConversionRecipe};
use crate::conversion_recipe::recipe_sha256;
use crate::custom_optimizer_config::{
    CustomOptimizerObjectiveWeights, CustomOptimizerSolverConfig, CustomOptimizerSolverMethod,
};
use crate::device_characterization::{DeviceForwardModel, LabColor};
use crate::inverse_lut_identity::InverseLutContinuityFieldMethod;
use crate::inverse_lut_runtime::{InverseLutLookupError, InverseLutRuntime};
use crate::inverse_lut_validation_eval::{
    InverseLutValidationEvaluationError, solve_independent_validation_reference,
};
use crate::inverse_separation_solver::{
    InverseSolveError, solve_inverse_separation_with_reference,
};
use crate::separation_optimizer::CandidateScoringWeights;

/// Exact off-grid reference construction used by inverse-LUT validation.
///
/// The method is versioned independently from the holdout method because a
/// future field/interpolation rule must not silently change the numerical
/// meaning of an already persisted validation report.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InverseLutValidationReferenceMethod {
    /// V1 and zero-continuity V2 are solved independently at each holdout.
    IndependentPointSolveV1,
    /// Positive-continuity V2 samples the exact persisted frozen Jacobi field
    /// with the LUT's versioned trilinear interpolation, then performs one V2
    /// reference solve at the holdout using that interpolated field value.
    /// No raster traversal or previous-pixel state participates.
    FrozenJacobiTrilinearThenV2SolveV1,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InverseLutValidationReferenceError {
    InvalidRecipe(Vec<String>),
    NotCustomOptimizerRecipe,
    MissingSolverConfig,
    MissingObjectiveWeights(Vec<String>),
    RecipeIdentity(String),
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
    ContinuityFieldSolverMismatch(String),
    Lookup(InverseLutLookupError),
    IndependentEvaluation(InverseLutValidationEvaluationError),
    ReferenceSolve(InverseSolveError),
}

pub fn validation_reference_method(
    runtime: &InverseLutRuntime,
    recipe: &ConversionRecipe,
) -> Result<InverseLutValidationReferenceMethod, InverseLutValidationReferenceError> {
    validate_runtime_recipe_binding(runtime, recipe)?;
    let solver = recipe
        .custom_optimizer_solver
        .ok_or(InverseLutValidationReferenceError::MissingSolverConfig)?;
    classify_reference_method(runtime.identity().build_policy.continuity_field, solver)
}

/// Construct the authoritative off-grid reference for one validation holdout.
///
/// Positive-weight V2 is intentionally derived from the exact persisted frozen
/// field. The runtime lookup supplies only the continuity reference; the returned
/// validation separation is a fresh solver result at the holdout target Lab.
pub fn solve_validation_reference(
    runtime: &InverseLutRuntime,
    recipe: &ConversionRecipe,
    model: &dyn DeviceForwardModel,
    target_lab: LabColor,
) -> Result<Option<Vec<f32>>, InverseLutValidationReferenceError> {
    validate_runtime_model_binding(runtime, recipe, model)?;
    match validation_reference_method(runtime, recipe)? {
        InverseLutValidationReferenceMethod::IndependentPointSolveV1 => {
            solve_independent_validation_reference(recipe, model, target_lab)
                .map_err(InverseLutValidationReferenceError::IndependentEvaluation)
        }
        InverseLutValidationReferenceMethod::FrozenJacobiTrilinearThenV2SolveV1 => {
            let field_reference = match runtime.lookup(target_lab) {
                Ok(values) => values,
                Err(InverseLutLookupError::OutOfDomain { .. })
                | Err(InverseLutLookupError::UnsupportedCorner { .. }) => return Ok(None),
                Err(error) => return Err(InverseLutValidationReferenceError::Lookup(error)),
            };
            let solver = recipe
                .custom_optimizer_solver
                .ok_or(InverseLutValidationReferenceError::MissingSolverConfig)?;
            let weights = scoring_weights(solver, recipe.target.channels.len())?;
            match solve_inverse_separation_with_reference(
                &recipe.target,
                &recipe.strategy,
                weights,
                model,
                target_lab,
                solver,
                Some(&field_reference),
            ) {
                Ok(result) => Ok(Some(result.candidate.coverages)),
                Err(InverseSolveError::NoFeasibleCandidate) => Ok(None),
                Err(error) => Err(InverseLutValidationReferenceError::ReferenceSolve(error)),
            }
        }
    }
}

fn classify_reference_method(
    field: InverseLutContinuityFieldMethod,
    solver: CustomOptimizerSolverConfig,
) -> Result<InverseLutValidationReferenceMethod, InverseLutValidationReferenceError> {
    let positive_v2 = matches!(
        (solver.method, solver.continuity_preference),
        (CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2, Some(policy))
            if policy.weight > 0.0
    );
    match (field, positive_v2) {
        (InverseLutContinuityFieldMethod::IndependentNodeSolvesV1, false) => {
            Ok(InverseLutValidationReferenceMethod::IndependentPointSolveV1)
        }
        (InverseLutContinuityFieldMethod::JacobiSixNeighborV1 { .. }, true) => Ok(
            InverseLutValidationReferenceMethod::FrozenJacobiTrilinearThenV2SolveV1,
        ),
        (InverseLutContinuityFieldMethod::IndependentNodeSolvesV1, true) => Err(
            InverseLutValidationReferenceError::ContinuityFieldSolverMismatch(
                "Positive-continuity V2 requires a persisted Jacobi continuity field."
                    .to_owned(),
            ),
        ),
        (InverseLutContinuityFieldMethod::JacobiSixNeighborV1 { .. }, false) => Err(
            InverseLutValidationReferenceError::ContinuityFieldSolverMismatch(
                "A persisted Jacobi continuity field requires positive-continuity V2 solver semantics."
                    .to_owned(),
            ),
        ),
    }
}

fn validate_runtime_recipe_binding(
    runtime: &InverseLutRuntime,
    recipe: &ConversionRecipe,
) -> Result<(), InverseLutValidationReferenceError> {
    recipe
        .validate()
        .map_err(InverseLutValidationReferenceError::InvalidRecipe)?;
    if recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
        return Err(InverseLutValidationReferenceError::NotCustomOptimizerRecipe);
    }
    let actual_recipe_sha =
        recipe_sha256(recipe).map_err(InverseLutValidationReferenceError::RecipeIdentity)?;
    if runtime.identity().recipe_sha256 != actual_recipe_sha {
        return Err(InverseLutValidationReferenceError::RecipeIdentityMismatch {
            expected: runtime.identity().recipe_sha256.clone(),
            actual: actual_recipe_sha,
        });
    }
    Ok(())
}

fn validate_runtime_model_binding(
    runtime: &InverseLutRuntime,
    recipe: &ConversionRecipe,
    model: &dyn DeviceForwardModel,
) -> Result<(), InverseLutValidationReferenceError> {
    validate_runtime_recipe_binding(runtime, recipe)?;
    if runtime.identity().characterization_id != model.identity().id {
        return Err(
            InverseLutValidationReferenceError::CharacterizationIdentityMismatch {
                expected: runtime.identity().characterization_id.clone(),
                actual: model.identity().id.clone(),
            },
        );
    }
    if runtime.identity().channel_names != model.identity().channel_names {
        return Err(
            InverseLutValidationReferenceError::ChannelTopologyMismatch {
                expected: runtime.identity().channel_names.clone(),
                actual: model.identity().channel_names.clone(),
            },
        );
    }
    Ok(())
}

fn scoring_weights(
    solver: CustomOptimizerSolverConfig,
    channel_count: usize,
) -> Result<CandidateScoringWeights, InverseLutValidationReferenceError> {
    solver
        .validate(channel_count)
        .map_err(InverseLutValidationReferenceError::MissingObjectiveWeights)?;
    let objective = solver.objective_weights.ok_or_else(|| {
        InverseLutValidationReferenceError::MissingObjectiveWeights(vec![
            "Validation reference requires persisted production objective weights.".to_owned(),
        ])
    })?;
    objective
        .validate()
        .map_err(InverseLutValidationReferenceError::MissingObjectiveWeights)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_optimizer_config::{ContinuityDistanceMetric, ContinuityPreferenceConfig};
    use crate::inverse_lut_identity::{
        INVERSE_LUT_JACOBI_FIELD_METHOD_MAX_ITERATIONS, InverseLutContinuitySeedMethod,
    };

    fn positive_v2() -> CustomOptimizerSolverConfig {
        CustomOptimizerSolverConfig {
            method: CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2,
            continuity_preference: Some(ContinuityPreferenceConfig {
                weight: 1.0,
                distance_metric: ContinuityDistanceMetric::NormalizedL2,
                max_normalized_channel_jump: 0.2,
                dominant_channel_switch_penalty: 0.25,
            }),
            ..CustomOptimizerSolverConfig::default()
        }
    }

    fn jacobi() -> InverseLutContinuityFieldMethod {
        InverseLutContinuityFieldMethod::JacobiSixNeighborV1 {
            seed_method: InverseLutContinuitySeedMethod::IndependentV1NodeSolveV1,
            iterations: INVERSE_LUT_JACOBI_FIELD_METHOD_MAX_ITERATIONS.min(4),
            self_weight: 0.35,
        }
    }

    #[test]
    fn independent_field_uses_independent_reference_method() {
        assert_eq!(
            classify_reference_method(
                InverseLutContinuityFieldMethod::IndependentNodeSolvesV1,
                CustomOptimizerSolverConfig::default(),
            )
            .unwrap(),
            InverseLutValidationReferenceMethod::IndependentPointSolveV1
        );
    }

    #[test]
    fn positive_v2_jacobi_uses_frozen_field_reference_method() {
        assert_eq!(
            classify_reference_method(jacobi(), positive_v2()).unwrap(),
            InverseLutValidationReferenceMethod::FrozenJacobiTrilinearThenV2SolveV1
        );
    }

    #[test]
    fn field_and_solver_semantics_cannot_be_mixed() {
        assert!(
            classify_reference_method(
                InverseLutContinuityFieldMethod::IndependentNodeSolvesV1,
                positive_v2(),
            )
            .is_err()
        );
        assert!(
            classify_reference_method(jacobi(), CustomOptimizerSolverConfig::default()).is_err()
        );
    }
}
