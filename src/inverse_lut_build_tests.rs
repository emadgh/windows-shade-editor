use std::path::PathBuf;

use crate::color_conversion::{
    CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
    ConversionRenderingIntent, ConversionTargetDefinition, SeparationStrategy, TargetChannelDefinition,
};
use crate::custom_optimizer_config::{
    ContinuityDistanceMetric, ContinuityPreferenceConfig, CustomOptimizerSolverConfig,
    CustomOptimizerSolverMethod,
};
use crate::device_characterization_model::{
    ForwardModelValidationPolicy, LocalForwardModelConfig, ValidatedLocalForwardModel,
};
use crate::device_characterization_package::{
    CharacterizationMeasurementMetadata, CharacterizationPackage, CharacterizationPayload,
    CharacterizationProductionContext, CharacterizationSample, CharacterizationValidationLevel,
    MeasuredLabColor, ValidatedCharacterizationPackage,
};
use crate::inverse_lut_artifact::load_inverse_lut_artifact;
use crate::inverse_lut_continuity_builder::lab_grid_points;
use crate::inverse_lut_identity::{
    INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION, InverseLutBuildPolicy,
    InverseLutContinuityFieldMethod, InverseLutContinuitySeedMethod,
    InverseLutInterpolationMethod, InverseLutNumericalPrecision, InverseLutOutputQuantization,
    InverseLutValidityEncoding, LabGridSpec,
};
use crate::inverse_lut_runtime::{
    InverseLutRuntime, build_inverse_lut_payload, publish_built_inverse_lut_if_absent,
};
use crate::model::IccProfileIdentity;

fn package() -> ValidatedCharacterizationPackage {
    let mut samples = Vec::new();
    for blue in [0.0f32, 0.5] {
        for brown in [0.0f32, 0.5] {
            for beige in [0.0f32, 0.5] {
                for black in [0.0f32, 0.5] {
                    let coverages = vec![blue, brown, beige, black];
                    let lab = MeasuredLabColor {
                        l: 95.0
                            - 20.0 * f64::from(blue)
                            - 16.0 * f64::from(brown)
                            - 10.0 * f64::from(beige)
                            - 42.0 * f64::from(black),
                        a: -3.0 * f64::from(blue)
                            + 7.0 * f64::from(brown)
                            + 2.0 * f64::from(beige),
                        b: -12.0 * f64::from(blue)
                            + 8.0 * f64::from(brown)
                            + 4.0 * f64::from(beige),
                    };
                    samples.push(CharacterizationSample { coverages, lab });
                }
            }
        }
    }
    CharacterizationPackage::new(CharacterizationPayload {
        revision: "inverse-lut-build-fixture-v1".to_owned(),
        validation_level: CharacterizationValidationLevel::ProductionValidated,
        output_bit_depth: 16,
        channel_names: ["Blue", "Brown", "Beige", "Black"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        measured_channel_max_coverage: vec![0.5; 4],
        measured_total_ink_limit: 2.0,
        production_context: CharacterizationProductionContext {
            machine_id: "fixture-machine".to_owned(),
            rip_name: "fixture-rip".to_owned(),
            rip_version: "1".to_owned(),
            linearization_id: "fixture-linearization".to_owned(),
            substrate: "fixture-substrate".to_owned(),
            glaze: None,
            body: None,
            product_family: None,
        },
        measurement: CharacterizationMeasurementMetadata {
            instrument_model: "fixture-instrument".to_owned(),
            instrument_serial: None,
            illuminant: "D50".to_owned(),
            observer: "2deg".to_owned(),
            measurement_condition: "M1".to_owned(),
            measured_at_unix_ms: None,
            operator_or_lab: None,
        },
        samples,
    })
    .unwrap()
    .validated()
    .unwrap()
}

fn model(package: &ValidatedCharacterizationPackage) -> ValidatedLocalForwardModel {
    ValidatedLocalForwardModel::build(
        package,
        LocalForwardModelConfig {
            neighbor_count: 4,
            distance_power: 2.0,
            max_support_distance: 1.0,
        },
        ForwardModelValidationPolicy {
            max_mean_delta_e00: 100.0,
            max_p95_delta_e00: 100.0,
            max_delta_e00: 100.0,
            max_unsupported_fraction: 0.0,
        },
    )
    .unwrap()
}

fn fast_solver() -> CustomOptimizerSolverConfig {
    CustomOptimizerSolverConfig {
        initial_samples: 32,
        beam_width: 4,
        refinement_rounds: 1,
        initial_step_fraction: 0.15,
        step_decay: 0.5,
        preference_delta_e00: 0.1,
        ..CustomOptimizerSolverConfig::default()
    }
}

fn recipe(characterization_id: String) -> ConversionRecipe {
    ConversionRecipe {
        source_transparency_policy: None,
        schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
        engine_mode: ConversionEngineMode::CustomOptimizer,
        source_profile_identity: IccProfileIdentity {
            description: "Fixture source".to_owned(),
            sha256: "fixture-source-hash".to_owned(),
        },
        target: ConversionTargetDefinition {
            name: "Fixture 4C".to_owned(),
            channels: ["Blue", "Brown", "Beige", "Black"]
                .into_iter()
                .map(|name| TargetChannelDefinition {
                    name: name.to_owned(),
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: Some(0.5),
                })
                .collect(),
            bit_depth: 16,
            output_profile_identity: None,
            output_profile_path: None,
            device_link_identity: None,
            device_link_path: None,
            characterization_id: Some(characterization_id),
            total_ink_limit: Some(2.0),
        },
        rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
        black_point_compensation: false,
        strategy: SeparationStrategy::default(),
        custom_optimizer_solver: Some(fast_solver()),
    }
}

fn grid() -> LabGridSpec {
    LabGridSpec {
        l_min: 90.0,
        l_max: 95.0,
        l_samples: 2,
        a_min: -1.0,
        a_max: 1.0,
        a_samples: 2,
        b_min: -2.0,
        b_max: 2.0,
        b_samples: 2,
    }
}

fn policy(continuity_field: InverseLutContinuityFieldMethod) -> InverseLutBuildPolicy {
    InverseLutBuildPolicy {
        schema_version: INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION,
        grid: grid(),
        interpolation: InverseLutInterpolationMethod::TrilinearV1,
        validity_encoding: InverseLutValidityEncoding::ExplicitNodeValidityMaskV1,
        numerical_precision: InverseLutNumericalPrecision::NormalizedF32V1,
        output_quantization: InverseLutOutputQuantization::ClampScaleRoundV1,
        continuity_field,
    }
}

fn temp_lut_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "shade-editor-{name}-{}-{}.selut",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn independent_builder_is_deterministic_and_round_trips_through_artifact_runtime() {
    let package = package();
    let model = model(&package);
    let recipe = recipe(package.package().id.clone());
    let policy = policy(InverseLutContinuityFieldMethod::IndependentNodeSolvesV1);

    let first = build_inverse_lut_payload(&recipe, &model, policy).unwrap();
    let second = build_inverse_lut_payload(&recipe, &model, policy).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.stats.node_count, 8);
    assert_eq!(
        first.stats.supported_nodes + first.stats.unsupported_nodes,
        first.stats.node_count
    );
    assert!(first.stats.supported_nodes > 0);

    let destination = temp_lut_path("round-trip");
    publish_built_inverse_lut_if_absent(&destination, &first).unwrap();
    let loaded = load_inverse_lut_artifact(&destination).unwrap();
    let runtime = InverseLutRuntime::from_verified(loaded).unwrap();
    assert_eq!(runtime.identity_content_id(), first.identity.content_id().unwrap());

    let (_shape, labs) = lab_grid_points(policy.grid).unwrap();
    let channel_count = recipe.target.channels.len();
    let supported_index = first.validity.iter().position(|valid| *valid).unwrap();
    let start = supported_index * channel_count;
    assert_eq!(
        runtime.lookup(labs[supported_index]).unwrap(),
        first.coverages[start..start + channel_count]
    );
    let _ = std::fs::remove_file(destination);
}

#[test]
fn positive_v2_builder_uses_versioned_continuity_field_contract() {
    let package = package();
    let model = model(&package);
    let mut recipe = recipe(package.package().id.clone());
    let solver = recipe.custom_optimizer_solver.as_mut().unwrap();
    solver.method = CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2;
    solver.continuity_preference = Some(ContinuityPreferenceConfig {
        weight: 1.0,
        distance_metric: ContinuityDistanceMetric::NormalizedL2,
        max_normalized_channel_jump: 0.25,
        dominant_channel_switch_penalty: 0.25,
    });
    let field = InverseLutContinuityFieldMethod::JacobiSixNeighborV1 {
        seed_method: InverseLutContinuitySeedMethod::IndependentV1NodeSolveV1,
        iterations: 1,
        self_weight: 0.35,
    };
    let policy = policy(field);

    let first = build_inverse_lut_payload(&recipe, &model, policy).unwrap();
    let second = build_inverse_lut_payload(&recipe, &model, policy).unwrap();
    assert_eq!(first.identity.content_id().unwrap(), second.identity.content_id().unwrap());
    assert_eq!(first.validity, second.validity);
    assert_eq!(first.coverages, second.coverages);
    assert_eq!(first.identity.build_policy.continuity_field, field);
    assert!(first.stats.continuity_seed_attempts > 0);
    assert!(first.stats.continuity_solves > 0);
}
