use crate::color_conversion::ConversionRecipe;
use crate::conversion_candidate_preview::{
    CandidatePreviewInput, CandidatePreviewResult,
    render_candidate_preview_with_custom_optimizer_evidence,
};
use crate::conversion_recipe::recipe_sha256;
use crate::conversion_transaction::ConversionCancellation;
use crate::custom_optimizer_evidence::CapturedCustomOptimizerEvidence;
use crate::custom_optimizer_evidence_bundle::CustomOptimizerEvidenceBundle;

/// Validate that a selected evidence bundle belongs to this exact immutable
/// conversion recipe. This is a transport/binding check only; it does not mint
/// production eligibility or bypass the measured approval gate.
pub fn validate_exact_bundle_recipe_binding(
    bundle: &CustomOptimizerEvidenceBundle,
    recipe: &ConversionRecipe,
) -> Result<(), String> {
    bundle.validate().map_err(|errors| errors.join("\n"))?;
    recipe
        .validate()
        .map_err(|errors| format!("Cannot bind invalid conversion recipe: {}", errors.join(" ")))?;

    let actual_recipe_sha256 = recipe_sha256(recipe)?;
    if actual_recipe_sha256 != bundle.recipe_sha256 {
        return Err(format!(
            "Custom Optimizer evidence bundle recipe SHA-256 {} does not match requested recipe {}.",
            bundle.recipe_sha256, actual_recipe_sha256
        ));
    }
    if &bundle.recipe != recipe {
        return Err(
            "Custom Optimizer evidence bundle recipe payload does not exactly match the requested immutable recipe."
                .to_owned(),
        );
    }
    Ok(())
}

/// Return the exact immutable evidence capture only after proving its bundle is
/// bound to the requested recipe. Final workers still reopen every referenced
/// artifact and independently authorize production execution.
pub fn captured_evidence_for_exact_recipe(
    bundle: &CustomOptimizerEvidenceBundle,
    recipe: &ConversionRecipe,
) -> Result<CapturedCustomOptimizerEvidence, String> {
    validate_exact_bundle_recipe_binding(bundle, recipe)?;
    Ok(bundle.evidence.clone())
}

/// Stable Candidate/UI cache identity for this exact recipe + evidence bundle.
/// This identity is not production authority.
pub fn selection_sha256_for_exact_recipe(
    bundle: &CustomOptimizerEvidenceBundle,
    recipe: &ConversionRecipe,
) -> Result<String, String> {
    validate_exact_bundle_recipe_binding(bundle, recipe)?;
    bundle.selection_sha256()
}

/// Render Candidate Preview from the same exact bundle that can later be carried
/// into final capture. The delegated production renderer reopens and authorizes
/// the evidence independently and therefore remains fail-closed on #205/#191.
pub fn render_candidate_preview_from_bundle(
    input: CandidatePreviewInput,
    bundle: &CustomOptimizerEvidenceBundle,
    cancellation: &ConversionCancellation,
) -> Result<CandidatePreviewResult, String> {
    validate_exact_bundle_recipe_binding(bundle, &input.recipe)?;
    render_candidate_preview_with_custom_optimizer_evidence(
        input,
        &bundle.evidence,
        cancellation,
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn binding_never_mints_or_carries_production_authority() {
        let source = include_str!("custom_optimizer_evidence_binding.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(runtime.contains("validate_exact_bundle_recipe_binding"));
        assert!(runtime.contains("&bundle.recipe != recipe"));
        assert!(runtime.contains("bundle.selection_sha256()"));
        assert!(runtime.contains("render_candidate_preview_with_custom_optimizer_evidence"));
        assert!(!runtime.contains("InverseLutProductionEligibility"));
        assert!(!runtime.contains("validate_inverse_lut_production_eligibility"));
        assert!(!runtime.contains("production_authorized: bool"));
    }
}
