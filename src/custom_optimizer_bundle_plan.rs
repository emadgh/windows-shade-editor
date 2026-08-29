use std::collections::{BTreeMap, BTreeSet};

use crate::color_conversion::ConversionRecipe;
use crate::conversion_batch::batch_recipe_policy_sha256;
use crate::conversion_job_authority::ConversionJobAuthority;
use crate::custom_optimizer_evidence_binding::{
    captured_evidence_for_exact_recipe, selection_sha256_for_exact_recipe,
};
use crate::custom_optimizer_evidence_bundle::CustomOptimizerEvidenceBundle;
use crate::model::IccProfileIdentity;
use crate::source_transparency::SourceTransparencyPolicy;

/// Current Source-Face identity used to prove that a selected Custom Optimizer
/// evidence bundle still belongs to the exact Face being planned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomOptimizerFaceBindingInput {
    pub source_face_index: usize,
    pub source_profile_identity: IccProfileIdentity,
    pub source_transparency_policy: Option<SourceTransparencyPolicy>,
}

/// One exact per-Face recipe/authority pair prepared from a validated evidence
/// bundle. `selection_sha256` is cache identity only and grants no execution.
#[derive(Clone, Debug)]
pub struct CustomOptimizerFacePlanEntry {
    pub source_face_index: usize,
    pub recipe: ConversionRecipe,
    pub authority: ConversionJobAuthority,
    pub selection_sha256: String,
}

/// Deterministic batch-facing result. All entries are guaranteed to share one
/// target/engine/separation policy while retaining their own Source ICC and
/// transparency identities.
#[derive(Clone, Debug)]
pub struct CustomOptimizerFacePlan {
    pub batch_recipe_policy_sha256: String,
    pub entries: Vec<CustomOptimizerFacePlanEntry>,
}

pub fn build_custom_optimizer_face_plan(
    inputs: &[CustomOptimizerFaceBindingInput],
    bundles: &BTreeMap<usize, CustomOptimizerEvidenceBundle>,
) -> Result<CustomOptimizerFacePlan, Vec<String>> {
    if inputs.is_empty() {
        return Err(vec![
            "Custom Optimizer bundle plan requires at least one Source Face.".to_owned(),
        ]);
    }

    let mut errors = Vec::new();
    let mut seen_indices = BTreeSet::new();
    let mut previous_index = None;
    let mut expected_policy_sha256: Option<String> = None;
    let mut entries = Vec::with_capacity(inputs.len());

    for input in inputs {
        if !seen_indices.insert(input.source_face_index) {
            errors.push(format!(
                "Custom Optimizer bundle plan contains Source Face {} more than once.",
                input.source_face_index + 1
            ));
            continue;
        }
        if previous_index.is_some_and(|previous| input.source_face_index <= previous) {
            errors.push(
                "Custom Optimizer bundle plan inputs must preserve Source-project Face order."
                    .to_owned(),
            );
            continue;
        }
        previous_index = Some(input.source_face_index);

        let Some(bundle) = bundles.get(&input.source_face_index) else {
            errors.push(format!(
                "Source Face {} has no selected Custom Optimizer evidence bundle.",
                input.source_face_index + 1
            ));
            continue;
        };

        if let Err(binding_errors) = bundle.validate_source_binding(
            &input.source_profile_identity,
            input.source_transparency_policy,
        ) {
            errors.push(format!(
                "Source Face {} Custom Optimizer bundle is stale: {}",
                input.source_face_index + 1,
                binding_errors.join(" ")
            ));
            continue;
        }

        let recipe = bundle.recipe.clone();
        let policy_sha256 = match batch_recipe_policy_sha256(&recipe) {
            Ok(policy) => policy,
            Err(error) => {
                errors.push(format!(
                    "Source Face {} Custom Optimizer batch policy is invalid: {error}",
                    input.source_face_index + 1
                ));
                continue;
            }
        };
        if let Some(expected) = expected_policy_sha256.as_deref() {
            if !policy_sha256.eq_ignore_ascii_case(expected) {
                errors.push(format!(
                    "Source Face {} uses a different Custom Optimizer target/engine/separation policy. One batch cannot mix optimizer policies.",
                    input.source_face_index + 1
                ));
                continue;
            }
        } else {
            expected_policy_sha256 = Some(policy_sha256.clone());
        }

        let evidence = match captured_evidence_for_exact_recipe(bundle, &recipe) {
            Ok(evidence) => evidence,
            Err(error) => {
                errors.push(format!(
                    "Source Face {} Custom Optimizer evidence binding failed: {error}",
                    input.source_face_index + 1
                ));
                continue;
            }
        };
        let selection_sha256 = match selection_sha256_for_exact_recipe(bundle, &recipe) {
            Ok(identity) => identity,
            Err(error) => {
                errors.push(format!(
                    "Source Face {} Custom Optimizer selection identity failed: {error}",
                    input.source_face_index + 1
                ));
                continue;
            }
        };
        let authority = match ConversionJobAuthority::for_recipe(&recipe, Some(evidence)) {
            Ok(authority) => authority,
            Err(error) => {
                errors.push(format!(
                    "Source Face {} Custom Optimizer final-job authority binding failed: {error}",
                    input.source_face_index + 1
                ));
                continue;
            }
        };

        entries.push(CustomOptimizerFacePlanEntry {
            source_face_index: input.source_face_index,
            recipe,
            authority,
            selection_sha256,
        });
    }

    if !errors.is_empty() || entries.len() != inputs.len() {
        return Err(errors);
    }

    Ok(CustomOptimizerFacePlan {
        batch_recipe_policy_sha256: expected_policy_sha256
            .expect("non-empty validated bundle plan has one batch policy"),
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_face_plan_fails_closed() {
        let error = build_custom_optimizer_face_plan(&[], &BTreeMap::new()).unwrap_err();
        assert!(error.join(" ").contains("at least one Source Face"));
    }

    #[test]
    fn planner_keeps_authorization_outside_selection_binding() {
        let source = include_str!("custom_optimizer_bundle_plan.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        for required in [
            "bundle.validate_source_binding(",
            "batch_recipe_policy_sha256(&recipe)",
            "captured_evidence_for_exact_recipe(bundle, &recipe)",
            "selection_sha256_for_exact_recipe(bundle, &recipe)",
            "ConversionJobAuthority::for_recipe(&recipe, Some(evidence))",
        ] {
            assert!(runtime.contains(required), "missing exact bundle-plan gate: {required}");
        }
        assert!(!runtime.contains("validate_inverse_lut_production_eligibility"));
        assert!(!runtime.contains("load_and_authorize_custom_optimizer_evidence"));
        assert!(!runtime.contains("production_authorized: bool"));
    }

    #[test]
    fn planner_requires_one_policy_and_source_order() {
        let source = include_str!("custom_optimizer_bundle_plan.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(runtime.contains("input.source_face_index <= previous"));
        assert!(runtime.contains("!policy_sha256.eq_ignore_ascii_case(expected)"));
        assert!(runtime.contains("One batch cannot mix optimizer policies"));
    }
}
