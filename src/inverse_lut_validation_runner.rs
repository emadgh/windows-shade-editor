use crate::color_conversion::ConversionRecipe;
use crate::conversion_recipe::recipe_sha256;
use crate::device_characterization::DeviceForwardModel;
use crate::inverse_lut_artifact::VerifiedInverseLutArtifact;
use crate::inverse_lut_holdout::{InverseLutHoldoutError, generate_inverse_lut_holdouts};
use crate::inverse_lut_path_validation::{
    InverseLutPathValidationError, analyze_inverse_lut_paths,
};
use crate::inverse_lut_runtime::{InverseLutLookupError, InverseLutRuntime};
use crate::inverse_lut_validation::{
    InverseLutValidationPolicy, InverseLutValidationReport, InverseLutValidationSample,
    summarize_validation_samples,
};
use crate::inverse_lut_validation_eval::{
    InverseLutValidationEvaluationError, evaluate_inverse_lut_validation_sample,
};
use crate::inverse_lut_validation_reference::{
    InverseLutValidationReferenceError, solve_validation_reference, validation_reference_method,
};

#[derive(Clone, Debug, PartialEq)]
pub enum InverseLutValidationRunError {
    InvalidArtifact(InverseLutLookupError),
    HoldoutGeneration(InverseLutHoldoutError),
    RecipeIdentity(String),
    Reference {
        sample_index: usize,
        error: InverseLutValidationReferenceError,
    },
    PathDiagnostics(InverseLutPathValidationError),
    Evaluation {
        sample_index: usize,
        error: InverseLutValidationEvaluationError,
    },
    Report(String),
}

/// Run deterministic inverse-LUT validation directly from a verified artifact.
///
/// The report identity is sourced exclusively from `VerifiedInverseLutArtifact`:
/// callers cannot supply or override the LUT identity content ID or payload hash.
/// V1/zero-continuity V2 use independent point references. Positive-continuity
/// V2 uses the exact persisted frozen Jacobi field as an off-grid trilinear
/// continuity reference and never invents raster/previous-pixel state.
///
/// The versioned ordered diagnostic paths are evaluated independently of the
/// aggregate holdout distribution and become part of report identity. Any
/// unsupported sample invalidates its whole path rather than bridging across it.
pub fn run_inverse_lut_validation(
    artifact: VerifiedInverseLutArtifact,
    recipe: &ConversionRecipe,
    model: &dyn DeviceForwardModel,
    policy: InverseLutValidationPolicy,
) -> Result<InverseLutValidationReport, InverseLutValidationRunError> {
    let lut_identity_content_id = artifact.identity_content_id.clone();
    let lut_payload_sha256 = artifact.payload_sha256.clone();
    let characterization_id = artifact.identity.characterization_id.clone();
    let grid = artifact.identity.build_policy.grid;

    let runtime = InverseLutRuntime::from_verified(artifact)
        .map_err(InverseLutValidationRunError::InvalidArtifact)?;
    let holdouts = generate_inverse_lut_holdouts(grid, policy.holdout_method)
        .map_err(InverseLutValidationRunError::HoldoutGeneration)?;
    let recipe_sha = recipe_sha256(recipe)
        .map_err(InverseLutValidationRunError::RecipeIdentity)?;

    // Resolve the numerical reference contract before the potentially long
    // holdout loop. This fails closed if solver semantics and persisted field
    // method do not agree.
    validation_reference_method(&runtime, recipe).map_err(|error| {
        InverseLutValidationRunError::Reference {
            sample_index: 0,
            error,
        }
    })?;

    // Persist first- and second-order diagnostics for the exact ordered V1
    // paths. This uses the existing gradient-continuity/curvature semantics and
    // never stitches across an unsupported runtime lookup.
    let path_diagnostics = analyze_inverse_lut_paths(
        &runtime,
        recipe,
        model,
        &holdouts.paths,
        policy.path_policy,
    )
    .map_err(InverseLutValidationRunError::PathDiagnostics)?;

    let total_hint = holdouts
        .paths
        .iter()
        .try_fold(holdouts.point_samples.len(), |total, path| {
            total.checked_add(path.samples.len())
        })
        .ok_or_else(|| {
            InverseLutValidationRunError::Report(
                "Inverse-LUT validation holdout count overflowed usize.".to_owned(),
            )
        })?;
    if total_hint == 0 {
        return Err(InverseLutValidationRunError::Report(
            "Deterministic inverse-LUT holdout method generated no samples.".to_owned(),
        ));
    }

    let mut samples = Vec::with_capacity(total_hint);
    let mut sample_index = 0usize;
    for lab in holdouts.point_samples.iter().copied().chain(
        holdouts
            .paths
            .iter()
            .flat_map(|path| path.samples.iter().copied()),
    ) {
        let reference = solve_validation_reference(&runtime, recipe, model, lab)
            .map_err(|error| InverseLutValidationRunError::Reference {
                sample_index,
                error,
            })?;
        let sample = match reference {
            Some(reference) => evaluate_inverse_lut_validation_sample(
                &runtime,
                recipe,
                model,
                lab,
                &reference,
            )
            .map_err(|error| InverseLutValidationRunError::Evaluation {
                sample_index,
                error,
            })?,
            None => unsupported_sample(),
        };
        samples.push(sample);
        sample_index = sample_index.checked_add(1).ok_or_else(|| {
            InverseLutValidationRunError::Report(
                "Inverse-LUT validation sample index overflowed usize.".to_owned(),
            )
        })?;
    }

    summarize_validation_samples(
        lut_identity_content_id,
        lut_payload_sha256,
        recipe_sha,
        characterization_id,
        policy,
        path_diagnostics,
        &samples,
    )
    .map_err(InverseLutValidationRunError::Report)
}

/// Compatibility entry point retained while #190 is still in review.
/// It now dispatches to the exact persisted field semantics and therefore also
/// supports positive-continuity V2 instead of rejecting it.
pub fn run_independent_inverse_lut_validation(
    artifact: VerifiedInverseLutArtifact,
    recipe: &ConversionRecipe,
    model: &dyn DeviceForwardModel,
    policy: InverseLutValidationPolicy,
) -> Result<InverseLutValidationReport, InverseLutValidationRunError> {
    run_inverse_lut_validation(artifact, recipe, model, policy)
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

    #[test]
    fn unsupported_reference_sample_is_schema_clean() {
        let sample = unsupported_sample();
        assert!(!sample.supported);
        assert!(sample.lut_delta_e00.is_none());
        assert!(sample.reference_delta_e00.is_none());
        assert!(sample.lut_vs_reference_delta_e00.is_none());
        assert!(sample.ink_l1.is_none());
        assert!(sample.ink_l2.is_none());
        assert!(sample.max_channel_deviation.is_none());
        assert!(sample.u8_quantization_l1.is_none());
        assert!(sample.u16_quantization_l1.is_none());
        assert!(sample.constraints_preserved);
    }
}
