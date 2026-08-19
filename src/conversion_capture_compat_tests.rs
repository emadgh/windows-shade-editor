use std::path::PathBuf;

use crate::color_conversion::{
    CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
    ConversionRenderingIntent, ConversionTargetDefinition, SeparationStrategy,
    TargetChannelDefinition,
};
use crate::conversion_transaction::{
    CapturedOutputPolicy, CapturedSourceProfile, ConversionJobCapture,
};
use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
use crate::model::{IccProfileIdentity, ShadeProject};

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn icc_recipe() -> ConversionRecipe {
    ConversionRecipe {
        source_transparency_policy: None,
        schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
        engine_mode: ConversionEngineMode::Icc,
        source_profile_identity: IccProfileIdentity {
            description: "sRGB".to_owned(),
            sha256: HASH_A.to_owned(),
        },
        target: ConversionTargetDefinition {
            name: "Press CMYK".to_owned(),
            channels: ["Cyan", "Magenta", "Yellow", "Black"]
                .into_iter()
                .map(|name| TargetChannelDefinition {
                    name: name.to_owned(),
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: None,
                })
                .collect(),
            bit_depth: 16,
            output_profile_identity: Some(IccProfileIdentity {
                description: "Press".to_owned(),
                sha256: HASH_B.to_owned(),
            }),
            output_profile_path: Some(r"C:\Color\Press.icc".to_owned()),
            device_link_identity: None,
            device_link_path: None,
            characterization_id: None,
            total_ink_limit: None,
        },
        rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
        black_point_compensation: true,
        strategy: SeparationStrategy::default(),
        custom_optimizer_solver: None,
    }
}

fn custom_optimizer_recipe() -> ConversionRecipe {
    ConversionRecipe {
        source_transparency_policy: None,
        schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
        engine_mode: ConversionEngineMode::CustomOptimizer,
        source_profile_identity: IccProfileIdentity {
            description: "sRGB".to_owned(),
            sha256: HASH_A.to_owned(),
        },
        target: ConversionTargetDefinition {
            name: "Measured custom target".to_owned(),
            channels: crate::color_conversion_test_support::channel_names()
                .into_iter()
                .map(|name| TargetChannelDefinition {
                    name,
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: Some(1.0),
                })
                .collect(),
            bit_depth: 16,
            output_profile_identity: None,
            output_profile_path: None,
            device_link_identity: None,
            device_link_path: None,
            characterization_id: Some(crate::color_conversion_test_support::characterization_id()),
            total_ink_limit: Some(1.8),
        },
        rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
        black_point_compensation: true,
        strategy: SeparationStrategy::default(),
        custom_optimizer_solver: Some(CustomOptimizerSolverConfig::default()),
    }
}

fn capture_with_recipe(recipe: ConversionRecipe) -> Result<ConversionJobCapture, String> {
    ConversionJobCapture::capture(
        &ShadeProject::default(),
        PathBuf::from(r"C:\Design\Source.shade"),
        HASH_A.to_owned(),
        PathBuf::from(r"C:\Design\Face.tif"),
        None,
        HASH_A.to_owned(),
        CapturedSourceProfile::Embedded,
        recipe,
        CapturedOutputPolicy::MustNotExist,
        PathBuf::from(r"C:\Production\Face.tif"),
        PathBuf::from(r"C:\Production\Job.shade"),
        "Job".to_owned(),
        "Face".to_owned(),
    )
}

#[test]
fn legacy_icc_capture_json_omits_optional_custom_optimizer_evidence_and_round_trips() {
    let capture = capture_with_recipe(icc_recipe()).unwrap();
    let bytes = serde_json::to_vec(&capture).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(!text.contains("custom_optimizer_evidence"));

    let restored: ConversionJobCapture = serde_json::from_slice(&bytes).unwrap();
    assert!(restored.custom_optimizer_evidence.is_none());
    assert!(restored.validate().is_ok());
}

#[test]
fn custom_optimizer_cannot_use_legacy_capture_path_without_immutable_evidence() {
    let error = capture_with_recipe(custom_optimizer_recipe()).unwrap_err();
    assert!(error.contains("requires immutable production evidence"));
}
