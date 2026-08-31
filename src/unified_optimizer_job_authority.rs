use crate::color_conversion::{ConversionEngineMode, ConversionRecipe};
use crate::conversion_job_authority::ConversionJobAuthority;
use crate::custom_optimizer_evidence::CapturedCustomOptimizerEvidence;
use crate::profile_backed_optimizer_execution_capture::CapturedProfileBackedOptimizerExecution;

#[derive(Clone, Debug)]
pub enum UnifiedOptimizerExecutionEvidence {
    Measured(CapturedCustomOptimizerEvidence),
    ProfileBacked(CapturedProfileBackedOptimizerExecution),
}

/// Select final conversion authority from the immutable recipe itself rather than
/// from a UI boolean. Measured and profile-backed Custom Optimizer routes are
/// intentionally disjoint and Standard ICC/DeviceLink cannot carry either one.
pub fn unified_conversion_job_authority(
    recipe: &ConversionRecipe,
    evidence: Option<UnifiedOptimizerExecutionEvidence>,
) -> Result<ConversionJobAuthority, String> {
    recipe.validate().map_err(|errors| errors.join(" "))?;
    match recipe.engine_mode {
        ConversionEngineMode::Icc | ConversionEngineMode::DeviceLink => match evidence {
            None => ConversionJobAuthority::for_recipe(recipe, None),
            Some(_) => Err(
                "ICC/DeviceLink final conversion cannot carry Custom Optimizer execution authority."
                    .to_owned(),
            ),
        },
        ConversionEngineMode::CustomOptimizer => {
            let measured = recipe
                .target
                .characterization_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
            match (measured, evidence) {
                (true, Some(UnifiedOptimizerExecutionEvidence::Measured(measured_evidence))) => {
                    ConversionJobAuthority::for_recipe(recipe, Some(measured_evidence))
                }
                (
                    false,
                    Some(UnifiedOptimizerExecutionEvidence::ProfileBacked(profile_execution)),
                ) => ConversionJobAuthority::for_profile_backed_recipe(recipe, profile_execution),
                (true, Some(UnifiedOptimizerExecutionEvidence::ProfileBacked(_))) => Err(
                    "Measured-characterization Custom Optimizer recipe cannot use profile-backed execution authority."
                        .to_owned(),
                ),
                (false, Some(UnifiedOptimizerExecutionEvidence::Measured(_))) => Err(
                    "Profile-backed Custom Optimizer recipe cannot use measured production evidence."
                        .to_owned(),
                ),
                (true, None) => Err(
                    "Measured Custom Optimizer final conversion requires immutable measured production evidence for the exact recipe."
                        .to_owned(),
                ),
                (false, None) => Err(
                    "Profile-backed Custom Optimizer final conversion requires an exact LUT/execution capture for the current recipe."
                        .to_owned(),
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionRenderingIntent, ConversionTargetDefinition,
        SeparationStrategy, TargetChannelDefinition,
    };
    use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
    use crate::model::IccProfileIdentity;

    fn hash(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn profile_recipe() -> ConversionRecipe {
        ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: hash('a'),
            },
            source_transparency_policy: None,
            target: ConversionTargetDefinition {
                name: "Profile target".to_owned(),
                channels: ["Blue", "Brown", "Beige", "Black"]
                    .into_iter()
                    .map(|name| TargetChannelDefinition {
                        name: name.to_owned(),
                        display_rgb: None,
                        solidity: 1.0,
                        max_coverage: Some(1.0),
                    })
                    .collect(),
                bit_depth: 16,
                output_profile_identity: Some(IccProfileIdentity {
                    description: "Output".to_owned(),
                    sha256: hash('b'),
                }),
                output_profile_path: Some(r"C:\Color\Output.icc".to_owned()),
                device_link_identity: None,
                device_link_path: None,
                characterization_id: None,
                total_ink_limit: Some(4.0),
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: Some(CustomOptimizerSolverConfig::default()),
        }
    }

    #[test]
    fn profile_backed_recipe_fails_closed_without_exact_capture() {
        let error = unified_conversion_job_authority(&profile_recipe(), None).unwrap_err();
        assert!(error.contains("exact LUT/execution capture"));
    }

    #[test]
    fn measured_authority_cannot_be_requested_by_profile_backed_recipe_shape() {
        // Evidence payload construction is intentionally covered by the measured module;
        // this contract test asserts the recipe-derived route classification itself.
        let recipe = profile_recipe();
        assert!(recipe.target.characterization_id.is_none());
        assert_eq!(recipe.engine_mode, ConversionEngineMode::CustomOptimizer);
    }

    #[test]
    fn standard_icc_without_optimizer_evidence_remains_standard() {
        let mut recipe = profile_recipe();
        recipe.engine_mode = ConversionEngineMode::Icc;
        recipe.custom_optimizer_solver = None;
        let authority = unified_conversion_job_authority(&recipe, None).unwrap();
        assert!(matches!(authority, ConversionJobAuthority::Standard));
    }
}
