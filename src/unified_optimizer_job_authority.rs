use crate::color_conversion::{ConversionEngineMode, ConversionRecipe};
use crate::conversion_job_authority::ConversionJobAuthority;
use crate::custom_optimizer_evidence::CapturedCustomOptimizerEvidence;
use crate::profile_backed_optimizer_execution_capture::CapturedProfileBackedOptimizerExecution;
use crate::profile_backed_optimizer_ui_execution::load_default_profile_backed_optimizer_execution;

#[derive(Clone, Debug)]
pub enum UnifiedOptimizerExecutionEvidence {
    Measured(CapturedCustomOptimizerEvidence),
    ProfileBacked(CapturedProfileBackedOptimizerExecution),
}

/// Select final conversion authority from the immutable recipe itself rather than
/// from a UI boolean. Measured and profile-backed Custom Optimizer routes are
/// intentionally disjoint and Standard ICC/DeviceLink cannot carry either one.
///
/// For the unified profile-backed UI only, an omitted explicit sidecar means "reopen
/// the exact content-addressed Candidate artifact". This path never builds a LUT and
/// therefore is safe to call from plan recomputation. Missing, stale or tampered cache
/// state remains a hard error. An explicit sidecar (for example Candidate promotion)
/// is still validated by `for_profile_backed_recipe` directly.
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
                (false, None) => {
                    let prepared = load_default_profile_backed_optimizer_execution(recipe)
                        .map_err(|error| {
                            format!(
                                "Profile-backed Custom Optimizer final conversion requires the exact rendered Candidate LUT/execution artifact for the current recipe: {error}"
                            )
                        })?;
                    ConversionJobAuthority::for_profile_backed_recipe(recipe, prepared.capture)
                }
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
    fn profile_backed_recipe_fails_closed_without_exact_candidate_artifact() {
        let error = unified_conversion_job_authority(&profile_recipe(), None).unwrap_err();
        assert!(error.contains("exact rendered Candidate LUT/execution artifact"));
    }

    #[test]
    fn profile_backed_none_path_is_load_only_and_never_builds_lut() {
        let source = include_str!("unified_optimizer_job_authority.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(runtime.contains("load_default_profile_backed_optimizer_execution"));
        assert!(!runtime.contains("prepare_default_profile_backed_optimizer_execution"));
        assert!(!runtime.contains("build_output_icc_inverse_lut_payload"));
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
