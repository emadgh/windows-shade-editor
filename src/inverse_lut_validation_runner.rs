use crate::color_conversion::ConversionRecipe;
use crate::conversion_recipe::recipe_sha256;
use crate::device_characterization::DeviceForwardModel;
use crate::inverse_lut_artifact::VerifiedInverseLutArtifact;
use crate::inverse_lut_holdout::{InverseLutHoldoutError, generate_inverse_lut_holdouts};
use crate::inverse_lut_runtime::{InverseLutLookupError, InverseLutRuntime};
use crate::inverse_lut_validation::{
    InverseLutValidationPolicy, InverseLutValidationReport, InverseLutValidationSample,
    summarize_validation_samples,
};
use crate::inverse_lut_validation_eval::{
    InverseLutValidationEvaluationError, evaluate_inverse_lut_validation_sample,
    solve_independent_validation_reference,
};

#[derive(Clone, Debug, PartialEq)]
pub enum InverseLutValidationRunError {
    InvalidArtifact(InverseLutLookupError),
    HoldoutGeneration(InverseLutHoldoutError),
    RecipeIdentity(String),
    Evaluation {
        sample_index: usize,
        error: InverseLutValidationEvaluationError,
    },
    Report(String),
}

/// Run the deterministic validation report for solver semantics whose reference
/// solution is independent at each Lab holdout point.
///
/// The report identity is sourced exclusively from a `VerifiedInverseLutArtifact`:
/// callers cannot supply or override the LUT identity content ID or payload hash.
/// Positive-weight Continuity V2 deliberately fails through
/// `solve_independent_validation_reference`; its reference must instead be
/// derived from the exact frozen Jacobi field used by the LUT build.
pub fn run_independent_inverse_lut_validation(
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

    // The runtime evaluator verifies the exact recipe/model/artifact binding.
    // Do this before a potentially long holdout loop so stale inputs fail fast.
    let binding_probe = holdouts
        .point_samples
        .first()
        .copied()
        .or_else(|| holdouts.paths.iter().find_map(|path| path.samples.first().copied()));
    let Some(binding_probe) = binding_probe else {
        return Err(InverseLutValidationRunError::Report(
            "Deterministic inverse-LUT holdout method generated no samples.".to_owned(),
        ));
    };

    // Solving the reference first also rejects positive-weight V2 here rather
    // than after partially evaluating a report under the wrong semantics.
    let probe_reference = solve_independent_validation_reference(recipe, model, binding_probe)
        .map_err(|error| InverseLutValidationRunError::Evaluation {
            sample_index: 0,
            error,
        })?;
    if let Some(reference) = probe_reference.as_deref() {
        evaluate_inverse_lut_validation_sample(&runtime, recipe, model, binding_probe, reference)
            .map_err(|error| InverseLutValidationRunError::Evaluation {
                sample_index: 0,
                error,
            })?;
    }

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
    let mut samples = Vec::with_capacity(total_hint);
    let mut sample_index = 0usize;

    for lab in holdouts.point_samples.iter().copied().chain(
        holdouts
            .paths
            .iter()
            .flat_map(|path| path.samples.iter().copied()),
    ) {
        let reference = solve_independent_validation_reference(recipe, model, lab)
            .map_err(|error| InverseLutValidationRunError::Evaluation {
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
        &samples,
    )
    .map_err(InverseLutValidationRunError::Report)
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
