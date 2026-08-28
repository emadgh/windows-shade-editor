use crate::color_conversion::{
    ConversionEngineMode, ConversionRecipe, ConversionTargetDefinition,
};
use crate::conversion_preset_runtime::{
    PresetApplicationAvailability, unified_strategy_preset_availability,
};
use crate::conversion_presets::{PresetApplyError, SeparationPresetDefinition};
use crate::custom_optimizer_operator_controls::{
    AppliedCustomOptimizerOperatorControls, CustomOptimizerOperatorControlError,
    CustomOptimizerOperatorControls,
};

/// Operator controls captured against an exact Custom Optimizer production target.
///
/// The binding intentionally excludes Source ICC/transparency identity so the same
/// operator choice can be applied to Current / Selected / All Faces while each Face
/// still gets its own immutable recipe and recipe SHA-256. Production-critical target
/// identity stays exact: characterization, topology, per-ink limits, total-ink limit,
/// bit depth and target name must all match before the controls can be replayed.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundCustomOptimizerOperatorControls {
    target: ConversionTargetDefinition,
    controls: CustomOptimizerOperatorControls,
}

#[derive(Debug, PartialEq)]
pub enum BoundCustomOptimizerOperatorControlError {
    NotCustomOptimizerEngine(ConversionEngineMode),
    ApplicationUnavailable(PresetApplicationAvailability),
    TargetMismatch,
    Preset(PresetApplyError),
    Operator(CustomOptimizerOperatorControlError),
}

impl std::fmt::Display for BoundCustomOptimizerOperatorControlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotCustomOptimizerEngine(engine) => write!(
                formatter,
                "Custom Optimizer operator controls cannot bind to {engine:?}."
            ),
            Self::ApplicationUnavailable(availability) => formatter.write_str(
                availability
                    .reason()
                    .unwrap_or("Custom Optimizer operator controls are unavailable."),
            ),
            Self::TargetMismatch => formatter.write_str(
                "Custom Optimizer operator controls are stale because the exact production target identity changed. Re-evaluate the preset/controls for the current characterization and ink topology.",
            ),
            Self::Preset(error) => write!(formatter, "Cannot apply conversion preset: {error:?}"),
            Self::Operator(error) => error.fmt(formatter),
        }
    }
}

impl From<PresetApplyError> for BoundCustomOptimizerOperatorControlError {
    fn from(value: PresetApplyError) -> Self {
        Self::Preset(value)
    }
}

impl From<CustomOptimizerOperatorControlError> for BoundCustomOptimizerOperatorControlError {
    fn from(value: CustomOptimizerOperatorControlError) -> Self {
        Self::Operator(value)
    }
}

impl BoundCustomOptimizerOperatorControls {
    /// Capture exact strategy + objective-weight state and bind it to the exact
    /// production target. Merely capturing draft state does not authorize output.
    pub fn from_recipe(
        recipe: &ConversionRecipe,
    ) -> Result<Self, BoundCustomOptimizerOperatorControlError> {
        if recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
            return Err(BoundCustomOptimizerOperatorControlError::NotCustomOptimizerEngine(
                recipe.engine_mode,
            ));
        }
        let controls = CustomOptimizerOperatorControls::from_recipe(recipe)?;
        Ok(Self {
            target: recipe.target.clone(),
            controls,
        })
    }

    pub fn target(&self) -> &ConversionTargetDefinition {
        &self.target
    }

    pub fn controls(&self) -> &CustomOptimizerOperatorControls {
        &self.controls
    }

    /// Apply the bound controls to another Face recipe for the same exact target.
    ///
    /// Authorization is re-evaluated for every concrete recipe. Passing `false`
    /// remains fail-closed; callers must eventually derive `true` only from the
    /// #191 production-authorization path backed by approved #205 measured evidence.
    pub fn apply_to_recipe(
        &self,
        recipe: &ConversionRecipe,
        custom_optimizer_production_authorized: bool,
    ) -> Result<AppliedCustomOptimizerOperatorControls, BoundCustomOptimizerOperatorControlError>
    {
        if recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
            return Err(BoundCustomOptimizerOperatorControlError::NotCustomOptimizerEngine(
                recipe.engine_mode,
            ));
        }
        if recipe.target != self.target {
            return Err(BoundCustomOptimizerOperatorControlError::TargetMismatch);
        }
        let capability = CustomOptimizerOperatorControls::authorize_for_recipe(
            recipe,
            custom_optimizer_production_authorized,
        )?;
        Ok(self.controls.apply_to_recipe(recipe, &capability)?)
    }

    /// Convert one compatible preset into the exact target-bound operator state
    /// that Candidate Preview and final conversion can share.
    ///
    /// The central engine-semantics guard is evaluated before the preset is
    /// allowed to alter a recipe. The preset owns strategy only; existing
    /// versioned solver objective weights are deliberately preserved and captured
    /// from the exact baseline recipe by `from_recipe`.
    pub fn from_preset(
        preset: &SeparationPresetDefinition,
        recipe: &ConversionRecipe,
        custom_optimizer_production_authorized: bool,
    ) -> Result<
        (Self, AppliedCustomOptimizerOperatorControls),
        BoundCustomOptimizerOperatorControlError,
    > {
        let availability = unified_strategy_preset_availability(
            recipe.engine_mode,
            custom_optimizer_production_authorized,
        );
        if availability != PresetApplicationAvailability::Available {
            return Err(
                BoundCustomOptimizerOperatorControlError::ApplicationUnavailable(availability),
            );
        }

        let preset_recipe = preset.apply_to_recipe(recipe)?;
        let binding = Self::from_recipe(&preset_recipe)?;
        let applied = binding.apply_to_recipe(recipe, custom_optimizer_production_authorized)?;

        // The preset changes strategy only. Re-applying the target-bound controls
        // must therefore reconstruct the exact same immutable recipe, including
        // the baseline objective weights.
        debug_assert_eq!(applied.recipe, preset_recipe);
        Ok((binding, applied))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionRenderingIntent,
        TargetChannelDefinition,
    };
    use crate::conversion_recipe::recipe_sha256;
    use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
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

    fn recipe(source_hash: char) -> ConversionRecipe {
        ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: IccProfileIdentity {
                description: format!("Source {source_hash}"),
                sha256: hash(source_hash),
            },
            source_transparency_policy: None,
            target: target(),
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: Default::default(),
            custom_optimizer_solver: Some(CustomOptimizerSolverConfig::default()),
        }
    }

    #[test]
    fn preset_becomes_exact_target_bound_operator_state() {
        let baseline = recipe('a');
        let preset = SeparationPresetDefinition::built_in_black_focused(
            &baseline.target,
            ConversionEngineMode::CustomOptimizer,
            "Black",
        )
        .unwrap();
        let baseline_sha = recipe_sha256(&baseline).unwrap();

        let (binding, applied) =
            BoundCustomOptimizerOperatorControls::from_preset(&preset, &baseline, true).unwrap();

        assert_eq!(binding.target(), &baseline.target);
        assert_eq!(applied.recipe.strategy.preset_name, "Black-focused");
        assert_eq!(applied.recipe.strategy.black_channel.as_deref(), Some("Black"));
        assert_ne!(applied.recipe_sha256, baseline_sha);
        assert_eq!(
            applied.recipe.custom_optimizer_solver.as_ref().unwrap().objective_weights,
            baseline.custom_optimizer_solver.as_ref().unwrap().objective_weights
        );
    }

    #[test]
    fn one_target_binding_replays_across_faces_without_reusing_source_identity() {
        let first = recipe('a');
        let second = recipe('b');
        let preset = SeparationPresetDefinition::built_in_black_focused(
            &first.target,
            ConversionEngineMode::CustomOptimizer,
            "Black",
        )
        .unwrap();
        let (binding, first_applied) =
            BoundCustomOptimizerOperatorControls::from_preset(&preset, &first, true).unwrap();
        let second_applied = binding.apply_to_recipe(&second, true).unwrap();

        assert_ne!(
            first_applied.recipe.source_profile_identity,
            second_applied.recipe.source_profile_identity
        );
        assert_eq!(first_applied.recipe.target, second_applied.recipe.target);
        assert_eq!(first_applied.recipe.strategy, second_applied.recipe.strategy);
        assert_eq!(
            first_applied.recipe.custom_optimizer_solver,
            second_applied.recipe.custom_optimizer_solver
        );
        assert_ne!(first_applied.recipe_sha256, second_applied.recipe_sha256);
    }

    #[test]
    fn target_drift_rejects_stale_operator_state() {
        let baseline = recipe('a');
        let binding = BoundCustomOptimizerOperatorControls::from_recipe(&baseline).unwrap();

        let mut characterization_changed = recipe('b');
        characterization_changed.target.characterization_id = Some("different-measurement".to_owned());
        assert_eq!(
            binding.apply_to_recipe(&characterization_changed, true).unwrap_err(),
            BoundCustomOptimizerOperatorControlError::TargetMismatch
        );

        let mut topology_changed = recipe('b');
        topology_changed.target.channels.swap(0, 1);
        assert_eq!(
            binding.apply_to_recipe(&topology_changed, true).unwrap_err(),
            BoundCustomOptimizerOperatorControlError::TargetMismatch
        );
    }

    #[test]
    fn missing_production_authorization_remains_fail_closed() {
        let baseline = recipe('a');
        let preset = SeparationPresetDefinition::built_in_black_focused(
            &baseline.target,
            ConversionEngineMode::CustomOptimizer,
            "Black",
        )
        .unwrap();
        assert_eq!(
            BoundCustomOptimizerOperatorControls::from_preset(&preset, &baseline, false)
                .unwrap_err(),
            BoundCustomOptimizerOperatorControlError::ApplicationUnavailable(
                PresetApplicationAvailability::CustomOptimizerNotProductionAuthorized
            )
        );
    }

    #[test]
    fn icc_recipe_cannot_capture_or_apply_optimizer_operator_state() {
        let baseline = recipe('a');
        let binding = BoundCustomOptimizerOperatorControls::from_recipe(&baseline).unwrap();
        let mut icc = baseline.clone();
        icc.engine_mode = ConversionEngineMode::Icc;
        icc.target.characterization_id = None;
        icc.target.output_profile_identity = Some(IccProfileIdentity {
            description: "Target ICC".to_owned(),
            sha256: hash('d'),
        });
        icc.target.output_profile_path = Some(r"C:\Color\target.icc".to_owned());
        icc.custom_optimizer_solver = None;

        assert_eq!(
            binding.apply_to_recipe(&icc, true).unwrap_err(),
            BoundCustomOptimizerOperatorControlError::NotCustomOptimizerEngine(
                ConversionEngineMode::Icc
            )
        );
    }
}
