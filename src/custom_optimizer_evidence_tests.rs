use std::fs;
use std::path::PathBuf;

use crate::color_conversion::{
    CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
    ConversionRenderingIntent, ConversionTargetDefinition, SeparationStrategy,
    TargetChannelDefinition,
};
use crate::conversion_recipe::recipe_sha256;
use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
use crate::custom_optimizer_evidence::{
    CapturedCustomOptimizerEvidence, CustomOptimizerEvidenceError,
    load_and_authorize_custom_optimizer_evidence,
};
use crate::inverse_lut_artifact::{
    VerifiedInverseLutArtifact, load_inverse_lut_artifact, write_inverse_lut_artifact,
};
use crate::inverse_lut_identity::{
    INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION, INVERSE_LUT_IDENTITY_SCHEMA_VERSION,
    InverseLutBuildPolicy, InverseLutContinuityFieldMethod, InverseLutForwardModelIdentity,
    InverseLutForwardModelMethod, InverseLutInterpolationMethod,
    InverseLutLocalForwardModelConfigIdentity, InverseLutNumericalPrecision,
    InverseLutOutputQuantization, InverseLutValidityEncoding, LabGridSpec,
};
use crate::inverse_lut_path_validation::{InverseLutPathDiagnostic, InverseLutValidationPathKind};
use crate::inverse_lut_production_eligibility::InverseLutProductionEligibilityError;
use crate::inverse_lut_threshold_set::{
    INVERSE_LUT_THRESHOLD_CALIBRATION_APPROVAL_SCHEMA_VERSION,
    INVERSE_LUT_THRESHOLD_CALIBRATION_MANIFEST_SCHEMA_VERSION, InverseLutCalibrationSolverFamily,
    InverseLutThresholdCalibrationApproval, InverseLutThresholdCalibrationManifest,
    InverseLutThresholdCalibrationObservation, InverseLutThresholdSetMethod,
    InverseLutValidationThresholdSet,
};
use crate::inverse_lut_validation::{InverseLutValidationSample, summarize_validation_samples};
use crate::inverse_lut_validation_artifact::{
    VerifiedInverseLutValidationArtifact, load_inverse_lut_validation_artifact,
    publish_inverse_lut_validation_artifact_if_absent,
};
use crate::inverse_lut_validation_reference::InverseLutValidationReferenceMethod;
use crate::model::IccProfileIdentity;
use crate::production_colorimetry::validate_characterization_for_icc_pcs_lab;

const NODE_COUNT: usize = 8;
const CHANNEL_COUNT: usize = 4;

struct FileFixture {
    folder: PathBuf,
    recipe: ConversionRecipe,
    capture: CapturedCustomOptimizerEvidence,
}

impl Drop for FileFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.folder);
    }
}

fn channels() -> Vec<String> {
    crate::color_conversion_test_support::channel_names()
}

fn characterization_id() -> String {
    crate::color_conversion_test_support::characterization_id()
}

fn recipe() -> ConversionRecipe {
    ConversionRecipe {
        schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
        engine_mode: ConversionEngineMode::CustomOptimizer,
        source_profile_identity: IccProfileIdentity {
            description: "Evidence fixture source".to_owned(),
            sha256: "fixture-source-profile".to_owned(),
        },
        target: ConversionTargetDefinition {
            name: "Evidence fixture target".to_owned(),
            channels: channels()
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
            characterization_id: Some(characterization_id()),
            total_ink_limit: Some(1.8),
        },
        rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
        black_point_compensation: true,
        strategy: SeparationStrategy::default(),
        custom_optimizer_solver: Some(CustomOptimizerSolverConfig::default()),
    }
}

fn identity(recipe: &ConversionRecipe) -> crate::inverse_lut_identity::InverseLutIdentityRecord {
    crate::inverse_lut_identity::InverseLutIdentityRecord {
        schema_version: INVERSE_LUT_IDENTITY_SCHEMA_VERSION,
        characterization_id: characterization_id(),
        forward_model: InverseLutForwardModelIdentity {
            method: InverseLutForwardModelMethod::LocalInverseDistanceWeightedV1,
            config: InverseLutLocalForwardModelConfigIdentity {
                neighbor_count: 2,
                distance_power: 2.0,
                max_support_distance: 0.5,
            },
        },
        recipe_sha256: recipe_sha256(recipe).unwrap(),
        channel_names: channels(),
        target_bit_depth: 16,
        build_policy: InverseLutBuildPolicy {
            schema_version: INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION,
            grid: LabGridSpec {
                l_min: 0.0,
                l_max: 100.0,
                l_samples: 2,
                a_min: -10.0,
                a_max: 10.0,
                a_samples: 2,
                b_min: -10.0,
                b_max: 10.0,
                b_samples: 2,
            },
            interpolation: InverseLutInterpolationMethod::TrilinearV1,
            validity_encoding: InverseLutValidityEncoding::ExplicitNodeValidityMaskV1,
            numerical_precision: InverseLutNumericalPrecision::NormalizedF32V1,
            output_quantization: InverseLutOutputQuantization::ClampScaleRoundV1,
            continuity_field: InverseLutContinuityFieldMethod::IndependentNodeSolvesV1,
        },
    }
}

fn threshold_set() -> InverseLutValidationThresholdSet {
    let mut threshold_set = InverseLutValidationThresholdSet::provisional_v1();
    threshold_set.method = InverseLutThresholdSetMethod::MeasuredCeramicD50TwoDegreeV1;
    threshold_set
}

fn paths() -> Vec<InverseLutPathDiagnostic> {
    [
        InverseLutValidationPathKind::NeutralAxis,
        InverseLutValidationPathKind::NearNeutralWarm,
        InverseLutValidationPathKind::NearNeutralCool,
        InverseLutValidationPathKind::AAxis,
        InverseLutValidationPathKind::BAxis,
        InverseLutValidationPathKind::AbDiagonal,
        InverseLutValidationPathKind::AbOpposedDiagonal,
    ]
    .into_iter()
    .map(|kind| InverseLutPathDiagnostic {
        kind,
        sample_count: 5,
        unsupported_samples: 0,
        max_channel_jump: Some(0.0),
        max_normalized_channel_jump: Some(0.0),
        max_vector_l1_jump: Some(0.0),
        max_vector_l2_jump: Some(0.0),
        max_total_ink_jump: Some(0.0),
        dominant_channel_switches: Some(0),
        max_channel_second_difference: Some(0.0),
        max_normalized_channel_second_difference: Some(0.0),
        max_vector_l1_second_difference: Some(0.0),
        max_vector_l2_second_difference: Some(0.0),
        max_total_ink_second_difference: Some(0.0),
        continuity_violation_count: Some(0),
        curvature_violation_count: Some(0),
    })
    .collect()
}

fn validation(
    recipe: &ConversionRecipe,
    lut: &VerifiedInverseLutArtifact,
    thresholds: &InverseLutValidationThresholdSet,
) -> VerifiedInverseLutValidationArtifact {
    let sample = InverseLutValidationSample {
        supported: true,
        lut_delta_e00: Some(0.1),
        reference_delta_e00: Some(0.1),
        lut_vs_reference_delta_e00: Some(0.0),
        ink_l1: Some(0.0),
        ink_l2: Some(0.0),
        max_channel_deviation: Some(0.0),
        u8_quantization_l1: Some(0.0),
        u16_quantization_l1: Some(0.0),
        constraints_preserved: true,
    };
    let report = summarize_validation_samples(
        lut.identity_content_id.clone(),
        lut.payload_sha256.clone(),
        recipe_sha256(recipe).unwrap(),
        characterization_id(),
        thresholds.content_id().unwrap(),
        thresholds.policy,
        InverseLutValidationReferenceMethod::IndependentPointSolveV1,
        paths(),
        &[sample],
    )
    .unwrap();
    VerifiedInverseLutValidationArtifact {
        report_content_id: report.content_id().unwrap(),
        report,
    }
}

fn prefixed_id(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn calibration_manifest(
    recipe: &ConversionRecipe,
    lut: &VerifiedInverseLutArtifact,
    validation: &VerifiedInverseLutValidationArtifact,
    thresholds: &InverseLutValidationThresholdSet,
) -> InverseLutThresholdCalibrationManifest {
    InverseLutThresholdCalibrationManifest {
        schema_version: INVERSE_LUT_THRESHOLD_CALIBRATION_MANIFEST_SCHEMA_VERSION,
        pcs_method: crate::production_colorimetry::ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
        threshold_set_content_id: thresholds.content_id().unwrap(),
        observations: vec![
            InverseLutThresholdCalibrationObservation {
                solver_family: InverseLutCalibrationSolverFamily::IndependentV1,
                characterization_id: characterization_id(),
                recipe_sha256: recipe_sha256(recipe).unwrap(),
                lut_identity_content_id: lut.identity_content_id.clone(),
                validation_report_content_id: validation.report_content_id.clone(),
            },
            InverseLutThresholdCalibrationObservation {
                solver_family: InverseLutCalibrationSolverFamily::PositiveContinuityV2,
                characterization_id: characterization_id(),
                recipe_sha256: recipe_sha256(recipe).unwrap(),
                lut_identity_content_id: lut.identity_content_id.clone(),
                validation_report_content_id: prefixed_id('f'),
            },
        ],
    }
}

fn calibration_approval(
    thresholds: &InverseLutValidationThresholdSet,
    manifest: &InverseLutThresholdCalibrationManifest,
) -> InverseLutThresholdCalibrationApproval {
    InverseLutThresholdCalibrationApproval {
        schema_version: INVERSE_LUT_THRESHOLD_CALIBRATION_APPROVAL_SCHEMA_VERSION,
        pcs_method: crate::production_colorimetry::ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
        threshold_set_content_id: thresholds.content_id().unwrap(),
        calibration_manifest_content_id: manifest.content_id().unwrap(),
    }
}

fn temp_folder(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "shade-custom-optimizer-evidence-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn file_fixture(label: &str) -> FileFixture {
    let folder = temp_folder(label);
    fs::create_dir_all(&folder).unwrap();
    let recipe = recipe();
    let identity = identity(&recipe);
    let validity = vec![true; NODE_COUNT];
    let coverages = vec![0.25f32; NODE_COUNT * CHANNEL_COUNT];
    let lut_path = folder.join("fixture.shade-lut");
    write_inverse_lut_artifact(&lut_path, &identity, &validity, &coverages).unwrap();
    let lut = load_inverse_lut_artifact(&lut_path).unwrap();

    let thresholds = threshold_set();
    let validation = validation(&recipe, &lut, &thresholds);
    let validation_path = folder.join("fixture.shade-lut-validation");
    publish_inverse_lut_validation_artifact_if_absent(&validation_path, &validation.report)
        .unwrap();
    let validation = load_inverse_lut_validation_artifact(&validation_path).unwrap();

    let characterization =
        crate::color_conversion_test_support::validated_characterization_package();
    let characterization_path = folder.join("fixture.shade-characterization.json");
    fs::write(
        &characterization_path,
        serde_json::to_vec(characterization.package()).unwrap(),
    )
    .unwrap();
    let pcs = validate_characterization_for_icc_pcs_lab(&characterization).unwrap();
    let manifest = calibration_manifest(&recipe, &lut, &validation, &thresholds);
    let approval = calibration_approval(&thresholds, &manifest);
    let capture = CapturedCustomOptimizerEvidence::from_verified(
        lut_path,
        &lut,
        validation_path,
        &validation,
        characterization_path,
        &characterization,
        thresholds,
        manifest,
        approval,
        &pcs,
    )
    .unwrap();

    FileFixture {
        folder,
        recipe,
        capture,
    }
}

#[test]
fn captured_evidence_json_round_trip_is_deterministic_and_self_validating() {
    let fixture = file_fixture("roundtrip");
    let first = serde_json::to_vec(&fixture.capture).unwrap();
    let restored: CapturedCustomOptimizerEvidence = serde_json::from_slice(&first).unwrap();
    let second = serde_json::to_vec(&restored).unwrap();
    assert_eq!(restored, fixture.capture);
    assert_eq!(first, second);
    assert!(restored.validate().is_ok());
}

#[test]
fn stale_embedded_threshold_identity_is_rejected_before_file_loading() {
    let mut fixture = file_fixture("threshold-id");
    fixture.capture.threshold_set_content_id = prefixed_id('9');
    assert!(matches!(
        load_and_authorize_custom_optimizer_evidence(&fixture.capture, &fixture.recipe),
        Err(CustomOptimizerEvidenceError::InvalidCapture(_))
    ));
}

#[test]
fn stale_lut_locator_target_is_rejected_by_recomputed_identity() {
    let mut fixture = file_fixture("lut-id");
    fixture.capture.lut_identity_content_id = prefixed_id('8');
    assert!(matches!(
        load_and_authorize_custom_optimizer_evidence(&fixture.capture, &fixture.recipe),
        Err(CustomOptimizerEvidenceError::LutIdentityMismatch { .. })
    ));
}

#[test]
fn stale_validation_locator_target_is_rejected_by_recomputed_identity() {
    let mut fixture = file_fixture("validation-id");
    fixture.capture.validation_report_content_id = prefixed_id('7');
    assert!(matches!(
        load_and_authorize_custom_optimizer_evidence(&fixture.capture, &fixture.recipe),
        Err(CustomOptimizerEvidenceError::ValidationIdentityMismatch { .. })
    ));
}

#[test]
fn stale_characterization_locator_target_is_rejected_by_recomputed_identity() {
    let mut fixture = file_fixture("characterization-id");
    fixture.capture.characterization_id = prefixed_id('6');
    assert!(matches!(
        load_and_authorize_custom_optimizer_evidence(&fixture.capture, &fixture.recipe),
        Err(CustomOptimizerEvidenceError::CharacterizationIdentityMismatch { .. })
    ));
}

#[test]
fn changed_recipe_is_rejected_against_reopened_lut_identity() {
    let fixture = file_fixture("recipe");
    let mut changed = fixture.recipe.clone();
    changed.black_point_compensation = !changed.black_point_compensation;
    assert!(changed.validate().is_ok());
    assert!(matches!(
        load_and_authorize_custom_optimizer_evidence(&fixture.capture, &changed),
        Err(CustomOptimizerEvidenceError::LutRecipeMismatch { .. })
    ));
}

#[test]
fn exact_reopened_evidence_reaches_and_stops_at_empty_production_allowlist() {
    let fixture = file_fixture("allowlist");
    assert!(matches!(
        load_and_authorize_custom_optimizer_evidence(&fixture.capture, &fixture.recipe),
        Err(CustomOptimizerEvidenceError::Authorization(
            InverseLutProductionEligibilityError::CalibrationApprovalNotProductionApproved { .. }
        ))
    ));
}
