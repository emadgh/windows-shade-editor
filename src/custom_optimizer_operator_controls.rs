use crate::color_conversion::{ConversionEngineMode, ConversionRecipe, SeparationStrategy};
use crate::conversion_preset_runtime::{
    PresetApplicationAvailability, unified_strategy_preset_availability,
};
use crate::conversion_recipe::recipe_sha256;
use crate::custom_optimizer_config::{
    CustomOptimizerObjectiveWeights, CustomOptimizerSolverConfig,
};

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

#[derive(Clone, Debug, PartialEq)]
pub struct AppliedCustomOptimizerOperatorControls {
    pub recipe: ConversionRecipe,
    pub recipe_sha256: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CustomOptimizerOperatorControlError {
    NotCustomOptimizerEngine(ConversionEngineMode),
    ApplicationUnavailable(PresetApplicationAvailability),
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
            Self::ApplicationUnavailable(availability) => formatter.write_str(
                availability
                    .reason()
                    .unwrap_or("Custom Optimizer operator controls are unavailable."),
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
                "Cannot identify the Custom Optimizer recipe after applying operator controls: {error}"
            ),
        }
    }
}

impl CustomOptimizerOperatorControls {
    /// Capture the exact persisted strategy/objective fields from an existing
    /// Custom Optimizer recipe. Production authorization is intentionally not
    /// required merely to inspect/edit a draft; applying it to the executable
    /// recipe remains guarded by `apply_to_recipe`.
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

    /// Apply controls to the exact immutable recipe consumed by Candidate
    /// Preview/final conversion. The same central availability gate used by
    /// strategy presets is reused here so ICC/DeviceLink and an unapproved
    /// Custom Optimizer can never receive pixel-affecting operator controls via
    /// a parallel bypass.
    pub fn apply_to_recipe(
        &self,
        recipe: &ConversionRecipe,
        custom_optimizer_production_authorized: bool,
    ) -> Result<AppliedCustomOptimizerOperatorControls, CustomOptimizerOperatorControlError> {
        let availability = unified_strategy_preset_availability(
            recipe.engine_mode,
            custom_optimizer_production_authorized,
        );
        if availability != PresetApplicationAvailability::Available {
            return Err(CustomOptimizerOperatorControlError::ApplicationUnavailable(
                availability,
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

    /// Return the exact solver configuration that would be persisted after
    /// applying these controls. This is useful to render UI fields without
    /// inventing a second solver schema.
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

        let applied = controls.apply_to_recipe(&recipe, true).unwrap();
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
    fn authorization_gate_remains_fail_closed_until_measured_approval_exists() {
        let recipe = custom_recipe();
        let controls = CustomOptimizerOperatorControls::from_recipe(&recipe).unwrap();
        let error = controls.apply_to_recipe(&recipe, false).unwrap_err();
        assert_eq!(
            error,
            CustomOptimizerOperatorControlError::ApplicationUnavailable(
                PresetApplicationAvailability::CustomOptimizerNotProductionAuthorized
            )
        );
    }

    #[test]
    fn icc_and_devicelink_cannot_use_optimizer_controls_as_recipe_only_metadata() {
        let controls = CustomOptimizerOperatorControls::from_recipe(&custom_recipe()).unwrap();
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
            let error = controls.apply_to_recipe(&recipe, true).unwrap_err();
            assert_eq!(
                error,
                CustomOptimizerOperatorControlError::ApplicationUnavailable(
                    PresetApplicationAvailability::EngineDoesNotConsumeSeparationStrategy
                )
            );
        }
    }

    #[test]
    fn invalid_operator_state_never_mints_a_recipe_identity() {
        let recipe = custom_recipe();
        let mut controls = CustomOptimizerOperatorControls::from_recipe(&recipe).unwrap();
        controls
            .strategy
            .per_ink_bias
            .insert("Not a target ink".to_owned(), 0.5);
        controls.objective_weights.color_error = f32::NAN;
        assert!(matches!(
            controls.apply_to_recipe(&recipe, true),
            Err(CustomOptimizerOperatorControlError::InvalidRecipe(_))
        ));
    }

    #[test]
    fn applying_controls_preserves_non_control_recipe_identity_inputs() {
        let recipe = custom_recipe();
        let mut controls = CustomOptimizerOperatorControls::from_recipe(&recipe).unwrap();
        controls.strategy.black_channel = Some("Black".to_owned());
        controls.strategy.black_generation_strength = 0.5;
        let applied = controls.apply_to_recipe(&recipe, true).unwrap();

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
