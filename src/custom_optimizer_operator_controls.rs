use crate::color_conversion::{ConversionEngineMode, ConversionRecipe, SeparationStrategy};
use crate::conversion_recipe::recipe_sha256;
use crate::custom_optimizer_config::{
    CustomOptimizerObjectiveWeights, CustomOptimizerSolverConfig,
};
use crate::custom_optimizer_strategy_capability::CustomOptimizerStrategyCapability;

/// Exact operator-editable Custom Optimizer state that is persisted in the
/// immutable conversion recipe.
///
/// This type deliberately contains no UI-only approximation. `strategy` owns
/// Black/neutral/per-ink/ink-limit preferences, while `objective_weights` owns
/// the versioned solver objective fields consumed by the Custom Optimizer.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomOptimizerOperatorControls {
    pub strategy: SeparationStrategy,
    pub objective_weights: CustomOptimizerObjectiveWeights,
}

/// Recipe-bound capability for editing Custom Optimizer controls through the
/// unified production UI.
///
/// This is deliberately *not* inverse-LUT production eligibility. Changing a
/// strategy/objective field changes the recipe identity and therefore requires
/// Candidate/LUT/evidence invalidation and revalidation. The token proves only
/// that an exact typed strategy capability was validated for the pre-edit recipe;
/// final raster execution independently reopens measured or profile-backed authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomOptimizerOperatorControlCapability {
    baseline_recipe_sha256: String,
}

impl CustomOptimizerOperatorControlCapability {
    pub fn baseline_recipe_sha256(&self) -> &str {
        &self.baseline_recipe_sha256
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppliedCustomOptimizerOperatorControls {
    pub recipe: ConversionRecipe,
    pub recipe_sha256: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CustomOptimizerOperatorControlError {
    NotCustomOptimizerEngine(ConversionEngineMode),
    StrategyCapability(String),
    CapabilityRecipeMismatch {
        expected: String,
        actual: String,
    },
    MissingSolverConfig,
    MissingObjectiveWeights,
    InvalidRecipe(Vec<String>),
    RecipeIdentity(String),
}

impl std::fmt::Display for CustomOptimizerOperatorControlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotCustomOptimizerEngine(engine) => write!(
                formatter,
                "Custom Optimizer operator controls cannot be captured from {engine:?}."
            ),
            Self::StrategyCapability(error) => formatter.write_str(error),
            Self::CapabilityRecipeMismatch { expected, actual } => write!(
                formatter,
                "Custom Optimizer operator-control capability is stale: expected baseline recipe {expected}, found {actual}. Re-evaluate the exact current recipe before applying controls."
            ),
            Self::MissingSolverConfig => formatter.write_str(
                "Custom Optimizer operator controls require explicit persisted solver configuration.",
            ),
            Self::MissingObjectiveWeights => formatter.write_str(
                "Custom Optimizer operator controls require explicit persisted objective weights.",
            ),
            Self::InvalidRecipe(errors) => write!(
                formatter,
                "Custom Optimizer operator controls produced an invalid recipe: {}",
                errors.join(" ")
            ),
            Self::RecipeIdentity(error) => write!(
                formatter,
                "Cannot identify the Custom Optimizer recipe for operator controls: {error}"
            ),
        }
    }
}

impl CustomOptimizerOperatorControls {
    /// Capture the exact persisted strategy/objective fields from an existing
    /// Custom Optimizer recipe. Production authorization is intentionally not
    /// required merely to inspect/edit a draft; applying it to the unified
    /// production recipe requires a recipe-bound strategy capability.
    pub fn from_recipe(
        recipe: &ConversionRecipe,
    ) -> Result<Self, CustomOptimizerOperatorControlError> {
        if recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
            return Err(CustomOptimizerOperatorControlError::NotCustomOptimizerEngine(
                recipe.engine_mode,
            ));
        }
        recipe
            .validate()
            .map_err(CustomOptimizerOperatorControlError::InvalidRecipe)?;
        let solver = recipe
            .custom_optimizer_solver
            .as_ref()
            .ok_or(CustomOptimizerOperatorControlError::MissingSolverConfig)?;
        let objective_weights = solver
            .objective_weights
            .ok_or(CustomOptimizerOperatorControlError::MissingObjectiveWeights)?;
        Ok(Self {
            strategy: recipe.strategy.clone(),
            objective_weights,
        })
    }

    /// Mint a control-edit capability only after validating the typed strategy
    /// capability against this exact recipe. No boolean can grant editing authority.
    pub fn authorize_for_recipe(
        recipe: &ConversionRecipe,
        strategy_capability: &CustomOptimizerStrategyCapability,
    ) -> Result<CustomOptimizerOperatorControlCapability, CustomOptimizerOperatorControlError> {
        if recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
            return Err(CustomOptimizerOperatorControlError::NotCustomOptimizerEngine(
                recipe.engine_mode,
            ));
        }
        strategy_capability
            .validate_for_recipe(recipe)
            .map_err(CustomOptimizerOperatorControlError::StrategyCapability)?;
        recipe
            .validate()
            .map_err(CustomOptimizerOperatorControlError::InvalidRecipe)?;
        let baseline_recipe_sha256 = recipe_sha256(recipe)
            .map_err(CustomOptimizerOperatorControlError::RecipeIdentity)?;
        Ok(CustomOptimizerOperatorControlCapability {
            baseline_recipe_sha256,
        })
    }

    /// Apply controls to the exact immutable recipe consumed by Candidate
    /// Preview/final conversion. The capability is an editing guard only; the
    /// changed recipe invalidates prior Candidate/LUT authority and final conversion
    /// must independently authorize exact measured or profile-backed evidence.
    pub fn apply_to_recipe(
        &self,
        recipe: &ConversionRecipe,
        capability: &CustomOptimizerOperatorControlCapability,
    ) -> Result<AppliedCustomOptimizerOperatorControls, CustomOptimizerOperatorControlError> {
        let actual_baseline = recipe_sha256(recipe)
            .map_err(CustomOptimizerOperatorControlError::RecipeIdentity)?;
        if actual_baseline != capability.baseline_recipe_sha256 {
            return Err(
                CustomOptimizerOperatorControlError::CapabilityRecipeMismatch {
                    expected: capability.baseline_recipe_sha256.clone(),
                    actual: actual_baseline,
                },
            );
        }
        if recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
            return Err(CustomOptimizerOperatorControlError::NotCustomOptimizerEngine(
                recipe.engine_mode,
            ));
        }

        let mut candidate = recipe.clone();
        candidate.strategy = self.strategy.clone();
        let solver = candidate
            .custom_optimizer_solver
            .as_mut()
            .ok_or(CustomOptimizerOperatorControlError::MissingSolverConfig)?;
        solver.objective_weights = Some(self.objective_weights);
        candidate
            .validate()
            .map_err(CustomOptimizerOperatorControlError::InvalidRecipe)?;
        let recipe_sha256 = recipe_sha256(&candidate)
            .map_err(CustomOptimizerOperatorControlError::RecipeIdentity)?;
        Ok(AppliedCustomOptimizerOperatorControls {
            recipe: candidate,
            recipe_sha256,
        })
    }

    pub fn solver_with_controls(
        &self,
        solver: &CustomOptimizerSolverConfig,
    ) -> CustomOptimizerSolverConfig {
        let mut candidate = *solver;
        candidate.objective_weights = Some(self.objective_weights);
        candidate
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionRenderingIntent,
        ConversionTargetDefinition, TargetChannelDefinition,
    };
    use crate::custom_optimizer_strategy_capability::{
        CustomOptimizerStrategyAuthorityKind, CustomOptimizerStrategyCapability,
    };
    use crate::model::IccProfileIdentity;

    fn hash(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn target() -> ConversionTargetDefinition {
        ConversionTargetDefinition {
            name: "Measured ceramic 4C".to_owned(),
            channels: ["Blue", "Brown", "Beige", "Black"]
                .into_iter()
                .map(|name| TargetChannelDefinition {
                    name: name.to_owned(),
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: Some(if name == "Black" { 0.70 } else { 0.85 }),
                })
                .collect(),
            bit_depth: 16,
            output_profile_identity: None,
            output_profile_path: None,
            device_link_identity: None,
            device_link_path: None,
            characterization_id: Some("sha256:".to_owned() + &hash('c')),
            total_ink_limit: Some(1.8),
        }
    }

    fn custom_recipe() -> ConversionRecipe {
        ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: hash('a'),
            },
            source_transparency_policy: None,
            target: target(),
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: Some(CustomOptimizerSolverConfig::default()),
        }
    }

    fn measured_capability(recipe: &ConversionRecipe) -> CustomOptimizerStrategyCapability {
        CustomOptimizerStrategyCapability::for_recipe(
            recipe,
            CustomOptimizerStrategyAuthorityKind::MeasuredProduction,
        )
        .unwrap()
    }

    #[test]
    fn capture_reads_only_the_exact_persisted_control_fields() {
        let recipe = custom_recipe();
        let controls = CustomOptimizerOperatorControls::from_recipe(&recipe).unwrap();
        assert_eq!(controls.strategy, recipe.strategy);
        assert_eq!(
            controls.objective_weights,
            recipe
                .custom_optimizer_solver
                .as_ref()
                .unwrap()
                .objective_weights
                .unwrap()
        );
    }

    #[test]
    fn authorized_controls_change_the_same_immutable_recipe_identity() {
        let recipe = custom_recipe();
        let original_sha = recipe_sha256(&recipe).unwrap();
        let strategy_capability = measured_capability(&recipe);
        let capability = CustomOptimizerOperatorControls::authorize_for_recipe(
            &recipe,
            &strategy_capability,
        )
        .unwrap();
        assert_eq!(capability.baseline_recipe_sha256(), original_sha);

        let mut controls = CustomOptimizerOperatorControls::from_recipe(&recipe).unwrap();
        controls.strategy.preset_name = "Black-focused operator".to_owned();
        controls.strategy.black_channel = Some("Black".to_owned());
        controls.strategy.black_generation_strength = 0.8;
        controls.strategy.black_start = 0.2;
        controls.strategy.black_max = 0.7;
        controls.strategy.max_delta_e00 = Some(2.0);
        controls.strategy.per_ink_bias.insert("Black".to_owned(), 0.8);
        controls.strategy.per_ink_bias.insert("Blue".to_owned(), -0.4);
        controls.objective_weights.neutral_black = 1.8;
        controls.objective_weights.ink_preference = 1.4;

        let applied = controls.apply_to_recipe(&recipe, &capability).unwrap();
        assert_eq!(recipe.strategy, SeparationStrategy::default());
        assert_eq!(recipe_sha256(&recipe).unwrap(), original_sha);
        assert_ne!(applied.recipe_sha256, original_sha);
        assert_eq!(applied.recipe.strategy, controls.strategy);
        assert_eq!(
            applied
                .recipe
                .custom_optimizer_solver
                .as_ref()
                .unwrap()
                .objective_weights,
            Some(controls.objective_weights)
        );
        assert!(applied.recipe.validate().is_ok());
    }

    #[test]
    fn stale_strategy_capability_fails_closed() {
        let recipe = custom_recipe();
        let strategy_capability = measured_capability(&recipe);
        let mut changed = recipe.clone();
        changed.strategy.black_generation_strength = 0.7;
        let error = CustomOptimizerOperatorControls::authorize_for_recipe(
            &changed,
            &strategy_capability,
        )
        .unwrap_err();
        assert!(matches!(error, CustomOptimizerOperatorControlError::StrategyCapability(_)));
    }

    #[test]
    fn capability_is_bound_to_the_exact_pre_edit_recipe() {
        let recipe = custom_recipe();
        let strategy_capability = measured_capability(&recipe);
        let capability = CustomOptimizerOperatorControls::authorize_for_recipe(
            &recipe,
            &strategy_capability,
        )
        .unwrap();
        let controls = CustomOptimizerOperatorControls::from_recipe(&recipe).unwrap();

        let mut changed = recipe.clone();
        changed.target.name.push_str(" changed");
        let error = controls.apply_to_recipe(&changed, &capability).unwrap_err();
        assert!(matches!(
            error,
            CustomOptimizerOperatorControlError::CapabilityRecipeMismatch { .. }
        ));
    }

    #[test]
    fn icc_and_devicelink_cannot_mint_optimizer_control_capability() {
        for engine in [ConversionEngineMode::Icc, ConversionEngineMode::DeviceLink] {
            let mut recipe = custom_recipe();
            recipe.engine_mode = engine;
            recipe.custom_optimizer_solver = None;
            recipe.target.characterization_id = None;
            match engine {
                ConversionEngineMode::Icc => {
                    recipe.target.output_profile_identity = Some(IccProfileIdentity {
                        description: "Target".to_owned(),
                        sha256: hash('b'),
                    });
                    recipe.target.output_profile_path = Some(r"C:\Color\target.icc".to_owned());
                }
                ConversionEngineMode::DeviceLink => {
                    recipe.target.device_link_identity = Some(IccProfileIdentity {
                        description: "DeviceLink".to_owned(),
                        sha256: hash('d'),
                    });
                    recipe.target.device_link_path = Some(r"C:\Color\link.icc".to_owned());
                }
                ConversionEngineMode::CustomOptimizer => unreachable!(),
            }
            let measured = CustomOptimizerStrategyCapability::for_recipe(
                &recipe,
                CustomOptimizerStrategyAuthorityKind::MeasuredProduction,
            );
            assert!(measured.is_err());
        }
    }

    #[test]
    fn invalid_operator_state_never_mints_a_changed_recipe_identity() {
        let recipe = custom_recipe();
        let strategy_capability = measured_capability(&recipe);
        let capability = CustomOptimizerOperatorControls::authorize_for_recipe(
            &recipe,
            &strategy_capability,
        )
        .unwrap();
        let mut controls = CustomOptimizerOperatorControls::from_recipe(&recipe).unwrap();
        controls
            .strategy
            .per_ink_bias
            .insert("Not a target ink".to_owned(), 0.5);
        controls.objective_weights.color_error = f32::NAN;
        assert!(matches!(
            controls.apply_to_recipe(&recipe, &capability),
            Err(CustomOptimizerOperatorControlError::InvalidRecipe(_))
        ));
    }

    #[test]
    fn applying_controls_preserves_non_control_recipe_identity_inputs() {
        let recipe = custom_recipe();
        let strategy_capability = measured_capability(&recipe);
        let capability = CustomOptimizerOperatorControls::authorize_for_recipe(
            &recipe,
            &strategy_capability,
        )
        .unwrap();
        let mut controls = CustomOptimizerOperatorControls::from_recipe(&recipe).unwrap();
        controls.strategy.black_channel = Some("Black".to_owned());
        controls.strategy.black_generation_strength = 0.5;
        let applied = controls.apply_to_recipe(&recipe, &capability).unwrap();

        assert_eq!(applied.recipe.source_profile_identity, recipe.source_profile_identity);
        assert_eq!(applied.recipe.source_transparency_policy, recipe.source_transparency_policy);
        assert_eq!(applied.recipe.target, recipe.target);
        assert_eq!(applied.recipe.rendering_intent, recipe.rendering_intent);
        assert_eq!(
            applied.recipe.black_point_compensation,
            recipe.black_point_compensation
        );
        assert_eq!(
            applied.recipe.custom_optimizer_solver.as_ref().unwrap().method,
            recipe.custom_optimizer_solver.as_ref().unwrap().method
        );
    }
}
