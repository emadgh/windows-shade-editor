use std::collections::BTreeMap;

use windows_shade_editor::custom_optimizer_bundle_plan::{
    CustomOptimizerFaceBindingInput, CustomOptimizerFacePlan, build_custom_optimizer_face_plan,
};
use windows_shade_editor::custom_optimizer_evidence_bundle::CustomOptimizerEvidenceBundle;
use windows_shade_editor::source_transparency::SourceTransparencyPolicy;

use super::conversion_plan::ConversionFaceInspection;

/// Bridge the unified Color Conversion source inspections into the exact
/// Custom Optimizer bundle planner. This adapter only freezes current per-Face
/// Source ICC/transparency identity; it does not select a target, construct an
/// optimizer recipe, or mint production authority.
pub(crate) fn build_custom_optimizer_plan_for_inspections(
    inspections: &[ConversionFaceInspection],
    transparency_policies: &BTreeMap<usize, SourceTransparencyPolicy>,
    bundles: &BTreeMap<usize, CustomOptimizerEvidenceBundle>,
) -> Result<CustomOptimizerFacePlan, Vec<String>> {
    if inspections.is_empty() {
        return Err(vec!["Select at least one Source Face.".to_owned()]);
    }

    let mut errors = Vec::new();
    let mut inputs = Vec::with_capacity(inspections.len());

    for inspection in inspections {
        if !inspection.ready() {
            errors.push(format!(
                "Face {} ('{}') has blocking source preflight findings.",
                inspection.index + 1,
                inspection.label
            ));
            continue;
        }
        let Some(source_profile_identity) = inspection.profile_identity.clone() else {
            errors.push(format!(
                "Face {} ('{}') has no production Source ICC identity for Custom Optimizer binding.",
                inspection.index + 1,
                inspection.label
            ));
            continue;
        };
        inputs.push(CustomOptimizerFaceBindingInput {
            source_face_index: inspection.index,
            source_profile_identity,
            source_transparency_policy: transparency_policies.get(&inspection.index).copied(),
        });
    }

    if !errors.is_empty() || inputs.len() != inspections.len() {
        return Err(errors);
    }

    build_custom_optimizer_face_plan(&inputs, bundles)
}

#[cfg(test)]
mod tests {
    #[test]
    fn adapter_only_bridges_inspection_identity_into_exact_bundle_planner() {
        let source = include_str!("custom_optimizer_plan_adapter.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        for required in [
            "inspection.ready()",
            "inspection.profile_identity.clone()",
            "transparency_policies.get(&inspection.index).copied()",
            "build_custom_optimizer_face_plan(&inputs, bundles)",
        ] {
            assert!(runtime.contains(required), "missing inspection-plan bridge: {required}");
        }
        assert!(!runtime.contains("ConversionRecipe {") );
        assert!(!runtime.contains("ConversionJobAuthority::for_recipe"));
        assert!(!runtime.contains("production_authorized: bool"));
        assert!(!runtime.contains("validate_inverse_lut_production_eligibility"));
    }
}
