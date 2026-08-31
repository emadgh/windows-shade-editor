use std::path::{Path, PathBuf};

use crate::color_conversion::{
    CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
    ConversionRenderingIntent, ConversionTargetDefinition, SeparationStrategy,
    TargetChannelDefinition,
};
use crate::conversion_job_authority::ConversionJobAuthority;
use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
use crate::model::IccProfileIdentity;
use crate::production_target::{
    ProductionTargetProfileInspection, validate_target_channel_names,
    verify_production_target_profile,
};
use crate::profile_backed_optimizer_execution_capture::CapturedProfileBackedOptimizerExecution;
use crate::source_transparency::SourceTransparencyPolicy;
use crate::tiff_io::ColorModel;

/// Exact immutable inputs required to construct the profile-backed Custom Optimizer
/// recipe used by the unified Production Color Conversion UI.
///
/// The target ICC is reopened and SHA-verified before recipe construction. `strategy`
/// and `solver` are the same persisted fields consumed later by Candidate Preview,
/// inverse-LUT identity and final production; this contract deliberately owns no
/// UI-only approximation or measured-characterization evidence.
#[derive(Clone, Debug)]
pub struct ProfileBackedUnifiedRecipeInput {
    pub source_profile_identity: IccProfileIdentity,
    pub source_transparency_policy: Option<SourceTransparencyPolicy>,
    pub source_model: ColorModel,
    pub target_profile_path: PathBuf,
    pub target_profile_identity: IccProfileIdentity,
    pub target_name: String,
    pub channel_names: Vec<String>,
    pub channel_names_confirmed: bool,
    pub output_bit_depth: u8,
    pub rendering_intent: ConversionRenderingIntent,
    pub strategy: SeparationStrategy,
    pub solver: CustomOptimizerSolverConfig,
}

/// Reopen an existing Output ICC through the already-established production-target
/// verifier. Profile-backed Custom Optimizer intentionally reuses Output-ICC target
/// semantics; only the later separation execution engine differs from Standard ICC.
pub fn verify_profile_backed_output_target(
    path: &Path,
    expected_identity: &IccProfileIdentity,
    source_model: ColorModel,
) -> Result<ProductionTargetProfileInspection, String> {
    verify_production_target_profile(
        path,
        expected_identity,
        ConversionEngineMode::Icc,
        source_model,
    )
}

/// Construct the exact profile-backed Custom Optimizer recipe after freshly reopening
/// and SHA-verifying the selected Output ICC.
pub fn build_profile_backed_unified_recipe(
    input: ProfileBackedUnifiedRecipeInput,
) -> Result<ConversionRecipe, String> {
    let verified = verify_profile_backed_output_target(
        &input.target_profile_path,
        &input.target_profile_identity,
        input.source_model,
    )?;
    compose_profile_backed_unified_recipe(input, &verified)
}

fn compose_profile_backed_unified_recipe(
    input: ProfileBackedUnifiedRecipeInput,
    verified: &ProductionTargetProfileInspection,
) -> Result<ConversionRecipe, String> {
    if verified.identity != input.target_profile_identity {
        return Err(
            "Verified profile-backed Output ICC identity does not match the selected target."
                .to_owned(),
        );
    }
    if verified.path != input.target_profile_path {
        return Err(
            "Verified profile-backed Output ICC path does not match the selected target."
                .to_owned(),
        );
    }
    validate_target_channel_names(&input.channel_names, verified.output_channel_count)?;
    if !verified.channel_names_authoritative && !input.channel_names_confirmed {
        return Err(
            "Confirm the real production channel order before profile-backed optimization."
                .to_owned(),
        );
    }
    if input.target_name.trim().is_empty() {
        return Err("Target name cannot be empty.".to_owned());
    }
    if !matches!(input.output_bit_depth, 8 | 16) {
        return Err("Output bit depth must be 8 or 16.".to_owned());
    }

    let recipe = ConversionRecipe {
        source_transparency_policy: input.source_transparency_policy,
        schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
        engine_mode: ConversionEngineMode::CustomOptimizer,
        source_profile_identity: input.source_profile_identity,
        target: ConversionTargetDefinition {
            name: input.target_name.trim().to_owned(),
            channels: input
                .channel_names
                .iter()
                .map(|name| TargetChannelDefinition {
                    name: name.trim().to_owned(),
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: None,
                })
                .collect(),
            bit_depth: input.output_bit_depth,
            output_profile_identity: Some(verified.identity.clone()),
            output_profile_path: Some(verified.path.to_string_lossy().into_owned()),
            device_link_identity: None,
            device_link_path: None,
            characterization_id: None,
            total_ink_limit: None,
        },
        rendering_intent: input.rendering_intent,
        // BPC is a Standard ICC source→target transform option. The profile-backed
        // optimizer uses the Output ICC as a device→PCS forward model instead.
        black_point_compensation: false,
        strategy: input.strategy,
        custom_optimizer_solver: Some(input.solver),
    };
    recipe.validate().map_err(|errors| errors.join(" "))?;
    Ok(recipe)
}

/// Bind the exact execution capture produced by Candidate/LUT preparation to the same
/// immutable recipe that will be queued. This never accepts or synthesizes measured
/// #191/#205 evidence.
pub fn profile_backed_unified_job_authority(
    recipe: &ConversionRecipe,
    execution: CapturedProfileBackedOptimizerExecution,
) -> Result<ConversionJobAuthority, String> {
    ConversionJobAuthority::for_profile_backed_recipe(recipe, execution)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn verified_target() -> ProductionTargetProfileInspection {
        ProductionTargetProfileInspection {
            path: PathBuf::from(r"C:\Color\Ceramic-7C.icc"),
            identity: IccProfileIdentity {
                description: "Ceramic 7C".to_owned(),
                sha256: hash('b'),
            },
            device_class_label: "Output / printer".to_owned(),
            source_space_label: None,
            output_space_label: "7CLR".to_owned(),
            output_channel_count: 7,
            channel_names: [
                "Blue", "Brown", "Beige", "Black", "Green", "Orange", "Red",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            channel_names_authoritative: true,
        }
    }

    fn input() -> ProfileBackedUnifiedRecipeInput {
        let target = verified_target();
        ProfileBackedUnifiedRecipeInput {
            source_profile_identity: IccProfileIdentity {
                description: "Source RGB".to_owned(),
                sha256: hash('a'),
            },
            source_transparency_policy: None,
            source_model: ColorModel::Rgb,
            target_profile_path: target.path,
            target_profile_identity: target.identity,
            target_name: "Durst ceramic 7C".to_owned(),
            channel_names: target.channel_names,
            channel_names_confirmed: true,
            output_bit_depth: 16,
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            strategy: SeparationStrategy::default(),
            solver: CustomOptimizerSolverConfig::default(),
        }
    }

    #[test]
    fn verified_output_target_composes_profile_backed_recipe_without_measurement() {
        let input = input();
        let verified = verified_target();
        let recipe = compose_profile_backed_unified_recipe(input, &verified).unwrap();
        assert_eq!(recipe.engine_mode, ConversionEngineMode::CustomOptimizer);
        assert!(recipe.target.characterization_id.is_none());
        assert_eq!(
            recipe.target.output_profile_identity.as_ref(),
            Some(&verified.identity)
        );
        assert_eq!(
            recipe.target.output_profile_path.as_deref(),
            Some(verified.path.to_string_lossy().as_ref())
        );
        assert!(recipe.target.device_link_identity.is_none());
        assert!(recipe.custom_optimizer_solver.is_some());
        assert!(!recipe.black_point_compensation);
        assert!(recipe.validate().is_ok());
    }

    #[test]
    fn exact_strategy_and_solver_are_persisted_in_same_recipe() {
        let mut input = input();
        input.strategy.black_channel = Some("Black".to_owned());
        input.strategy.black_generation_strength = 0.8;
        input.strategy.per_ink_bias.insert("Black".to_owned(), 0.7);
        input.strategy.per_ink_bias.insert("Blue".to_owned(), -0.4);
        input.solver.objective_weights.as_mut().unwrap().neutral_black = 1.7;
        let expected_strategy = input.strategy.clone();
        let expected_solver = input.solver;

        let recipe = compose_profile_backed_unified_recipe(input, &verified_target()).unwrap();
        assert_eq!(recipe.strategy, expected_strategy);
        assert_eq!(recipe.custom_optimizer_solver, Some(expected_solver));
    }

    #[test]
    fn unconfirmed_non_authoritative_channel_order_fails_closed() {
        let mut input = input();
        input.channel_names_confirmed = false;
        let mut verified = verified_target();
        verified.channel_names_authoritative = false;
        let error = compose_profile_backed_unified_recipe(input, &verified).unwrap_err();
        assert!(error.contains("Confirm the real production channel order"));
    }

    #[test]
    fn verified_target_path_or_sha_drift_fails_before_recipe_identity() {
        let sha_drift_input = input();
        let mut verified = verified_target();
        verified.identity.sha256 = hash('c');
        let error = compose_profile_backed_unified_recipe(sha_drift_input, &verified).unwrap_err();
        assert!(error.contains("identity does not match"));

        let path_drift_input = input();
        let mut verified = verified_target();
        verified.path = PathBuf::from(r"C:\Color\Replacement.icc");
        let error = compose_profile_backed_unified_recipe(path_drift_input, &verified).unwrap_err();
        assert!(error.contains("path does not match"));
    }
}
