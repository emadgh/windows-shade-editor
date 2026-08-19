use crate::color_conversion::{
    CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
    ConversionRenderingIntent, ConversionTargetDefinition, SeparationStrategy, TargetChannelDefinition,
};
use crate::conversion_recipe::recipe_sha256;
use crate::custom_optimizer_config::{
    CustomOptimizerObjectiveWeights, CustomOptimizerSolverConfig,
    LEGACY_CUSTOM_OPTIMIZER_SOLVER_CONFIG_SCHEMA_VERSION,
};
use crate::model::IccProfileIdentity;

fn recipe() -> ConversionRecipe {
    ConversionRecipe {
        source_transparency_policy: None,
        schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
        engine_mode: ConversionEngineMode::CustomOptimizer,
        source_profile_identity: IccProfileIdentity {
            description: "source".to_owned(),
            sha256: "source-hash".to_owned(),
        },
        target: ConversionTargetDefinition {
            name: "Objective identity fixture".to_owned(),
            channels: ["Blue", "Brown", "Beige", "Black"]
                .into_iter()
                .map(|name| TargetChannelDefinition {
                    name: name.to_owned(),
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: Some(0.9),
                })
                .collect(),
            bit_depth: 16,
            output_profile_identity: None,
            output_profile_path: None,
            device_link_identity: None,
            device_link_path: None,
            characterization_id: Some("fixture-characterization".to_owned()),
            total_ink_limit: Some(2.0),
        },
        rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
        black_point_compensation: false,
        strategy: SeparationStrategy::default(),
        custom_optimizer_solver: Some(CustomOptimizerSolverConfig::default()),
    }
}

#[test]
fn every_persisted_objective_weight_participates_in_recipe_sha256() {
    let base = recipe();
    assert!(base.validate().is_ok());
    let base_hash = recipe_sha256(&base).unwrap();

    for mutate in [
        |weights: &mut CustomOptimizerObjectiveWeights| weights.color_error += 0.25,
        |weights: &mut CustomOptimizerObjectiveWeights| weights.ink_preference += 0.25,
        |weights: &mut CustomOptimizerObjectiveWeights| weights.neutral_black += 0.25,
        |weights: &mut CustomOptimizerObjectiveWeights| weights.total_ink += 0.25,
    ] {
        let mut changed = base.clone();
        let weights = changed
            .custom_optimizer_solver
            .as_mut()
            .and_then(|solver| solver.objective_weights.as_mut())
            .unwrap();
        mutate(weights);
        assert!(changed.validate().is_ok());
        assert_ne!(recipe_sha256(&changed).unwrap(), base_hash);
    }
}

#[test]
fn legacy_solver_policy_remains_readable_but_has_no_production_objective_provenance() {
    let mut legacy = recipe();
    let solver = legacy.custom_optimizer_solver.as_mut().unwrap();
    solver.schema_version = LEGACY_CUSTOM_OPTIMIZER_SOLVER_CONFIG_SCHEMA_VERSION;
    solver.objective_weights = None;

    assert!(legacy.validate().is_ok());
    let json = serde_json::to_string(&legacy).unwrap();
    assert!(!json.contains("objective_weights"));
    let restored: ConversionRecipe = serde_json::from_str(&json).unwrap();
    let restored_solver = restored.custom_optimizer_solver.unwrap();
    assert!(restored_solver.production_objective_weights().is_err());
}
