use sha2::{Digest, Sha256};

use crate::color_conversion::{
    CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
    ConversionRenderingIntent, ConversionTargetDefinition, SeparationStrategy,
    TargetChannelDefinition,
};
use crate::conversion_recipe::recipe_sha256;
use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
use crate::device_characterization::{CharacterizationIdentity, DeviceForwardModel, LabColor};
use crate::inverse_lut_artifact::VerifiedInverseLutArtifact;
use crate::inverse_lut_identity::{
    INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION, INVERSE_LUT_IDENTITY_SCHEMA_VERSION,
    InverseLutBuildPolicy, InverseLutContinuityFieldMethod, InverseLutForwardModelIdentity,
    InverseLutForwardModelMethod, InverseLutInterpolationMethod,
    InverseLutLocalForwardModelConfigIdentity, InverseLutNumericalPrecision,
    InverseLutOutputQuantization, InverseLutValidityEncoding, LabGridSpec,
};
use crate::inverse_lut_path_validation::{InverseLutPathDiagnostic, InverseLutValidationPathKind};
use crate::inverse_lut_production_eligibility::{
    InverseLutProductionEligibility, InverseLutProductionEligibilityError,
    validate_inverse_lut_production_eligibility,
};
use crate::inverse_lut_threshold_set::{
    INVERSE_LUT_THRESHOLD_CALIBRATION_APPROVAL_SCHEMA_VERSION,
    INVERSE_LUT_THRESHOLD_CALIBRATION_MANIFEST_SCHEMA_VERSION, InverseLutCalibrationSolverFamily,
    InverseLutThresholdCalibrationApproval, InverseLutThresholdCalibrationManifest,
    InverseLutThresholdCalibrationObservation, InverseLutThresholdSetMethod,
    InverseLutValidationThresholdSet,
};
use crate::inverse_lut_validation::{InverseLutValidationSample, summarize_validation_samples};
use crate::inverse_lut_validation_artifact::VerifiedInverseLutValidationArtifact;
use crate::inverse_lut_validation_reference::InverseLutValidationReferenceMethod;
use crate::model::IccProfileIdentity;
use crate::production_colorimetry::{
    PRODUCTION_PCS_COMPATIBILITY_SCHEMA_VERSION, ProductionPcsCompatibilityMethod,
    ValidatedProductionPcsCompatibility,
};

const NODE_COUNT: usize = 8;
const CHANNEL_COUNT: usize = 4;

fn characterization_id() -> String {
    format!("sha256:{}", "1".repeat(64))
}

fn channels() -> Vec<String> {
    vec![
        "Cyan".to_owned(),
        "Magenta".to_owned(),
        "Yellow".to_owned(),
        "Black".to_owned(),
    ]
}

fn pcs_compatibility() -> ValidatedProductionPcsCompatibility {
    ValidatedProductionPcsCompatibility {
        schema_version: PRODUCTION_PCS_COMPATIBILITY_SCHEMA_VERSION,
        method: ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
        characterization_id: characterization_id(),
        canonical_illuminant: "D50".to_owned(),
        canonical_observer: "2deg".to_owned(),
    }
}

fn threshold_set() -> InverseLutValidationThresholdSet {
    let mut threshold_set = InverseLutValidationThresholdSet::provisional_v1();
    threshold_set.method = InverseLutThresholdSetMethod::MeasuredCeramicD50TwoDegreeV1;
    threshold_set
}

fn recipe() -> ConversionRecipe {
    ConversionRecipe {
        schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
        engine_mode: ConversionEngineMode::CustomOptimizer,
        source_profile_identity: IccProfileIdentity {
            description: "Eligibility fixture source".to_owned(),
            sha256: "fixture-source-profile".to_owned(),
        },
        target: ConversionTargetDefinition {
            name: "Eligibility fixture target".to_owned(),
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

fn payload_sha256(validity: &[bool], coverages: &[f32]) -> String {
    let mut hasher = Sha256::new();
    for valid in validity {
        hasher.update([u8::from(*valid)]);
    }
    for value in coverages.iter().copied() {
        let canonical = if value == 0.0 { 0.0 } else { value };
        hasher.update(canonical.to_bits().to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn lut(recipe: &ConversionRecipe) -> VerifiedInverseLutArtifact {
    let validity = vec![true; NODE_COUNT];
    let coverages = vec![0.25; NODE_COUNT * CHANNEL_COUNT];
    let identity = identity(recipe);
    VerifiedInverseLutArtifact {
        identity_content_id: identity.content_id().unwrap(),
        identity,
        payload_sha256: payload_sha256(&validity, &coverages),
        validity,
        coverages,
    }
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
    let thresholds = threshold_set();
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

fn prefixed_id(hex: char) -> String {
    format!("sha256:{}", hex.to_string().repeat(64))
}

fn calibration_manifest(
    recipe: &ConversionRecipe,
    lut: &VerifiedInverseLutArtifact,
    validation: &VerifiedInverseLutValidationArtifact,
    threshold_set: &InverseLutValidationThresholdSet,
) -> InverseLutThresholdCalibrationManifest {
    InverseLutThresholdCalibrationManifest {
        schema_version: INVERSE_LUT_THRESHOLD_CALIBRATION_MANIFEST_SCHEMA_VERSION,
        pcs_method: ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
        threshold_set_content_id: threshold_set.content_id().unwrap(),
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
    threshold_set: &InverseLutValidationThresholdSet,
    manifest: &InverseLutThresholdCalibrationManifest,
) -> InverseLutThresholdCalibrationApproval {
    InverseLutThresholdCalibrationApproval {
        schema_version: INVERSE_LUT_THRESHOLD_CALIBRATION_APPROVAL_SCHEMA_VERSION,
        pcs_method: ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
        threshold_set_content_id: threshold_set.content_id().unwrap(),
        calibration_manifest_content_id: manifest.content_id().unwrap(),
    }
}

fn validate_with_calibration(
    lut: &VerifiedInverseLutArtifact,
    validation: &VerifiedInverseLutValidationArtifact,
    threshold_set: &InverseLutValidationThresholdSet,
    pcs: &ValidatedProductionPcsCompatibility,
    recipe: &ConversionRecipe,
    model: &dyn DeviceForwardModel,
) -> Result<InverseLutProductionEligibility, InverseLutProductionEligibilityError> {
    let manifest = calibration_manifest(recipe, lut, validation, threshold_set);
    let approval = calibration_approval(threshold_set, &manifest);
    validate_inverse_lut_production_eligibility(
        lut,
        validation,
        threshold_set,
        &manifest,
        &approval,
        pcs,
        recipe,
        model,
    )
}

struct FixtureModel {
    identity: CharacterizationIdentity,
}

impl FixtureModel {
    fn new() -> Self {
        Self {
            identity: CharacterizationIdentity {
                id: characterization_id(),
                channel_names: channels(),
            },
        }
    }
}

impl DeviceForwardModel for FixtureModel {
    fn identity(&self) -> &CharacterizationIdentity {
        &self.identity
    }

    fn predict_lab(&self, coverages: &[f32]) -> Result<LabColor, String> {
        if coverages.len() != CHANNEL_COUNT {
            return Err("fixture topology mismatch".to_owned());
        }
        Ok(LabColor {
            l: 50.0,
            a: 0.0,
            b: 0.0,
        })
    }
}

#[test]
fn exact_bindings_still_fail_closed_until_calibration_approval_is_allowlisted() {
    let recipe = recipe();
    let lut = lut(&recipe);
    let validation = validation(&recipe, &lut);
    let pcs = pcs_compatibility();
    let thresholds = threshold_set();
    let model = FixtureModel::new();

    assert!(matches!(
        validate_with_calibration(&lut, &validation, &thresholds, &pcs, &recipe, &model),
        Err(InverseLutProductionEligibilityError::CalibrationApprovalNotProductionApproved { .. })
    ));
}

#[test]
fn stale_report_for_another_payload_cannot_authorize_lut() {
    let recipe = recipe();
    let lut = lut(&recipe);
    let mut validation = validation(&recipe, &lut);
    validation.report.lut_payload_sha256 = "9".repeat(64);
    validation.report_content_id = validation.report.content_id().unwrap();
    let pcs = pcs_compatibility();
    let thresholds = threshold_set();
    let model = FixtureModel::new();

    assert!(matches!(
        validate_with_calibration(&lut, &validation, &thresholds, &pcs, &recipe, &model),
        Err(InverseLutProductionEligibilityError::LutPayloadMismatch { .. })
    ));
}

#[test]
fn stale_report_for_another_recipe_cannot_authorize_lut() {
    let recipe = recipe();
    let lut = lut(&recipe);
    let validation = validation(&recipe, &lut);
    let pcs = pcs_compatibility();
    let thresholds = threshold_set();
    let model = FixtureModel::new();
    let mut changed_recipe = recipe.clone();
    changed_recipe.black_point_compensation = !changed_recipe.black_point_compensation;
    assert!(changed_recipe.validate().is_ok());

    assert!(matches!(
        validate_with_calibration(
            &lut,
            &validation,
            &thresholds,
            &pcs,
            &changed_recipe,
            &model,
        ),
        Err(InverseLutProductionEligibilityError::RecipeMismatch { .. })
    ));
}

#[test]
fn mismatched_reference_method_cannot_authorize_lut() {
    let recipe = recipe();
    let lut = lut(&recipe);
    let mut validation = validation(&recipe, &lut);
    validation.report.reference_method =
        InverseLutValidationReferenceMethod::FrozenJacobiTrilinearThenV2SolveV1;
    validation.report_content_id = validation.report.content_id().unwrap();
    let pcs = pcs_compatibility();
    let thresholds = threshold_set();
    let model = FixtureModel::new();

    assert!(matches!(
        validate_with_calibration(&lut, &validation, &thresholds, &pcs, &recipe, &model),
        Err(InverseLutProductionEligibilityError::ReferenceMethodMismatch { .. })
    ));
}

#[test]
fn pcs_evidence_for_another_characterization_cannot_authorize_lut() {
    let recipe = recipe();
    let lut = lut(&recipe);
    let validation = validation(&recipe, &lut);
    let mut pcs = pcs_compatibility();
    let thresholds = threshold_set();
    pcs.characterization_id = format!("sha256:{}", "8".repeat(64));
    let model = FixtureModel::new();

    assert!(matches!(
        validate_with_calibration(&lut, &validation, &thresholds, &pcs, &recipe, &model),
        Err(InverseLutProductionEligibilityError::PcsCharacterizationMismatch { .. })
    ));
}

#[test]
fn malformed_pcs_evidence_fails_before_threshold_gate() {
    let recipe = recipe();
    let lut = lut(&recipe);
    let validation = validation(&recipe, &lut);
    let mut pcs = pcs_compatibility();
    let thresholds = threshold_set();
    pcs.canonical_illuminant = "D65".to_owned();
    let model = FixtureModel::new();

    assert!(matches!(
        validate_with_calibration(&lut, &validation, &thresholds, &pcs, &recipe, &model),
        Err(InverseLutProductionEligibilityError::InvalidPcsCompatibility(_))
    ));
}

#[test]
fn stale_threshold_set_cannot_authorize_report() {
    let recipe = recipe();
    let lut = lut(&recipe);
    let validation = validation(&recipe, &lut);
    let pcs = pcs_compatibility();
    let mut thresholds = threshold_set();
    thresholds.policy.max_delta_e00 += 0.25;
    let model = FixtureModel::new();

    assert!(matches!(
        validate_with_calibration(&lut, &validation, &thresholds, &pcs, &recipe, &model,),
        Err(InverseLutProductionEligibilityError::ThresholdSetMismatch { .. })
    ));
}

#[test]
fn calibration_manifest_must_include_exact_current_report_observation() {
    let recipe = recipe();
    let lut = lut(&recipe);
    let validation = validation(&recipe, &lut);
    let pcs = pcs_compatibility();
    let thresholds = threshold_set();
    let model = FixtureModel::new();
    let mut manifest = calibration_manifest(&recipe, &lut, &validation, &thresholds);
    manifest.observations[0].validation_report_content_id = prefixed_id('e');
    let approval = calibration_approval(&thresholds, &manifest);

    assert!(matches!(
        validate_inverse_lut_production_eligibility(
            &lut,
            &validation,
            &thresholds,
            &manifest,
            &approval,
            &pcs,
            &recipe,
            &model,
        ),
        Err(InverseLutProductionEligibilityError::CalibrationObservationMissing { .. })
    ));
}

#[test]
fn calibration_approval_must_bind_exact_manifest() {
    let recipe = recipe();
    let lut = lut(&recipe);
    let validation = validation(&recipe, &lut);
    let pcs = pcs_compatibility();
    let thresholds = threshold_set();
    let model = FixtureModel::new();
    let manifest = calibration_manifest(&recipe, &lut, &validation, &thresholds);
    let mut approval = calibration_approval(&thresholds, &manifest);
    approval.calibration_manifest_content_id = prefixed_id('e');

    assert!(matches!(
        validate_inverse_lut_production_eligibility(
            &lut,
            &validation,
            &thresholds,
            &manifest,
            &approval,
            &pcs,
            &recipe,
            &model,
        ),
        Err(InverseLutProductionEligibilityError::CalibrationApprovalBinding(_))
    ));
}

#[test]
fn forged_lut_payload_is_rehashed_before_eligibility() {
    let recipe = recipe();
    let mut lut = lut(&recipe);
    let validation = validation(&recipe, &lut);
    let pcs = pcs_compatibility();
    let thresholds = threshold_set();
    let model = FixtureModel::new();
    lut.coverages[0] = 0.75;

    assert!(matches!(
        validate_with_calibration(&lut, &validation, &thresholds, &pcs, &recipe, &model),
        Err(InverseLutProductionEligibilityError::InvalidLut(_))
    ));
}
