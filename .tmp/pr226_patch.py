from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected 1 occurrence, got {count}")
    return text.replace(old, new, 1)


def insert_before_once(text: str, marker: str, insertion: str, label: str) -> str:
    count = text.count(marker)
    if count != 1:
        raise SystemExit(f"{label}: expected 1 marker, got {count}")
    return text.replace(marker, insertion + marker, 1)


# ---------------------------------------------------------------------------
# Production eligibility contract
# ---------------------------------------------------------------------------
p = Path("src/inverse_lut_production_eligibility.rs")
text = p.read_text(encoding="utf-8")

text = replace_once(
    text,
    "use crate::inverse_lut_threshold_set::InverseLutValidationThresholdSet;\n",
    """use crate::inverse_lut_threshold_set::{
    InverseLutCalibrationSolverFamily, InverseLutThresholdCalibrationApproval,
    InverseLutThresholdCalibrationManifest, InverseLutValidationThresholdSet,
};
""",
    "threshold-set import",
)
text = replace_once(
    text,
    "pub const INVERSE_LUT_PRODUCTION_ELIGIBILITY_SCHEMA_VERSION: u32 = 3;",
    "pub const INVERSE_LUT_PRODUCTION_ELIGIBILITY_SCHEMA_VERSION: u32 = 4;",
    "eligibility schema",
)
text = replace_once(
    text,
    "    pub threshold_set_content_id: String,\n    pub recipe_sha256: String,\n",
    """    pub threshold_set_content_id: String,
    pub calibration_manifest_content_id: String,
    pub calibration_approval_content_id: String,
    pub recipe_sha256: String,
""",
    "eligibility calibration fields",
)
text = insert_before_once(
    text,
    '            ("characterization_id", self.characterization_id.as_str()),\n',
    """            (
                "calibration_manifest_content_id",
                self.calibration_manifest_content_id.as_str(),
            ),
            (
                "calibration_approval_content_id",
                self.calibration_approval_content_id.as_str(),
            ),
""",
    "eligibility calibration ID validation",
)
text = replace_once(
    text,
    "    ThresholdPolicyMismatch,\n",
    """    ThresholdPolicyMismatch,
    InvalidCalibrationManifest(Vec<String>),
    CalibrationManifestIdentity(String),
    InvalidCalibrationApproval(Vec<String>),
    CalibrationApprovalIdentity(String),
    CalibrationManifestThresholdSetMismatch {
        manifest: String,
        actual: String,
    },
    CalibrationPcsMethodMismatch {
        compatibility: ProductionPcsCompatibilityMethod,
        manifest: ProductionPcsCompatibilityMethod,
        approval: ProductionPcsCompatibilityMethod,
    },
    CalibrationObservationMissing {
        validation_report_content_id: String,
        solver_family: InverseLutCalibrationSolverFamily,
    },
    CalibrationApprovalBinding(Vec<String>),
    CalibrationApprovalCheck(String),
    CalibrationApprovalNotProductionApproved {
        calibration_approval_content_id: String,
    },
""",
    "calibration error variants",
)
legacy_error_start = text.find("    /// Structural validation is complete, but #205 has not yet frozen thresholds")
legacy_error_end = text.find("}\n\n/// Revalidate every production-critical binding", legacy_error_start)
if legacy_error_start < 0 or legacy_error_end < 0:
    raise SystemExit("Cannot locate legacy ThresholdsNotProductionFrozen variant")
text = text[:legacy_error_start] + text[legacy_error_end:]

text = replace_once(
    text,
    "    threshold_set: &InverseLutValidationThresholdSet,\n    pcs_compatibility: &ValidatedProductionPcsCompatibility,\n",
    """    threshold_set: &InverseLutValidationThresholdSet,
    calibration_manifest: &InverseLutThresholdCalibrationManifest,
    calibration_approval: &InverseLutThresholdCalibrationApproval,
    pcs_compatibility: &ValidatedProductionPcsCompatibility,
""",
    "mint calibration arguments",
)
threshold_id_start = text.find("    let threshold_set_content_id = threshold_set\n")
pcs_start = text.find("    pcs_compatibility\n", threshold_id_start)
if threshold_id_start < 0 or pcs_start < 0:
    raise SystemExit("Cannot locate threshold identity / PCS validation boundary")
calibration_validation = """    calibration_manifest
        .validate()
        .map_err(InverseLutProductionEligibilityError::InvalidCalibrationManifest)?;
    let calibration_manifest_content_id = calibration_manifest
        .content_id()
        .map_err(InverseLutProductionEligibilityError::CalibrationManifestIdentity)?;
    calibration_approval
        .validate()
        .map_err(InverseLutProductionEligibilityError::InvalidCalibrationApproval)?;
    let calibration_approval_content_id = calibration_approval
        .content_id()
        .map_err(InverseLutProductionEligibilityError::CalibrationApprovalIdentity)?;
"""
text = text[:pcs_start] + calibration_validation + text[pcs_start:]

approval_gate = """    if calibration_manifest.threshold_set_content_id != threshold_set_content_id {
        return Err(
            InverseLutProductionEligibilityError::CalibrationManifestThresholdSetMismatch {
                manifest: calibration_manifest.threshold_set_content_id.clone(),
                actual: threshold_set_content_id.clone(),
            },
        );
    }
    if calibration_manifest.pcs_method != pcs_compatibility.method
        || calibration_approval.pcs_method != pcs_compatibility.method
    {
        return Err(InverseLutProductionEligibilityError::CalibrationPcsMethodMismatch {
            compatibility: pcs_compatibility.method,
            manifest: calibration_manifest.pcs_method,
            approval: calibration_approval.pcs_method,
        });
    }

    let solver_family = solver_family_for_reference(actual_reference_method);
    let observation_matches = calibration_manifest.observations.iter().any(|observation| {
        observation.solver_family == solver_family
            && observation.characterization_id == validation.report.characterization_id
            && observation.recipe_sha256 == actual_recipe_sha
            && observation.lut_identity_content_id == runtime.identity_content_id()
            && observation.validation_report_content_id == actual_report_id
    });
    if !observation_matches {
        return Err(InverseLutProductionEligibilityError::CalibrationObservationMissing {
            validation_report_content_id: actual_report_id.clone(),
            solver_family,
        });
    }

    calibration_approval
        .validate_bindings(threshold_set, calibration_manifest)
        .map_err(InverseLutProductionEligibilityError::CalibrationApprovalBinding)?;
    let production_approved = calibration_approval
        .is_production_approved(threshold_set, calibration_manifest)
        .map_err(InverseLutProductionEligibilityError::CalibrationApprovalCheck)?;
    if !production_approved {
        return Err(
            InverseLutProductionEligibilityError::CalibrationApprovalNotProductionApproved {
                calibration_approval_content_id: calibration_approval_content_id.clone(),
            },
        );
    }
"""
text = replace_once(
    text,
    "    ensure_production_thresholds_frozen(threshold_set, &threshold_set_content_id)?;\n",
    approval_gate,
    "approval production gate",
)
text = replace_once(
    text,
    "        threshold_set_content_id,\n        recipe_sha256: actual_recipe_sha,\n",
    """        threshold_set_content_id,
        calibration_manifest_content_id,
        calibration_approval_content_id,
        recipe_sha256: actual_recipe_sha,
""",
    "minted calibration IDs",
)
legacy_helper_start = text.find("/// No policy schema is production-approved yet.")
legacy_helper_end = text.find("fn is_prefixed_sha256(value: &str) -> bool {", legacy_helper_start)
if legacy_helper_start < 0 or legacy_helper_end < 0:
    raise SystemExit("Cannot locate legacy threshold gate helper")
solver_helper = """fn solver_family_for_reference(
    method: InverseLutValidationReferenceMethod,
) -> InverseLutCalibrationSolverFamily {
    match method {
        InverseLutValidationReferenceMethod::IndependentPointSolveV1 => {
            InverseLutCalibrationSolverFamily::IndependentV1
        }
        InverseLutValidationReferenceMethod::FrozenJacobiTrilinearThenV2SolveV1 => {
            InverseLutCalibrationSolverFamily::PositiveContinuityV2
        }
    }
}

"""
text = text[:legacy_helper_start] + solver_helper + text[legacy_helper_end:]

local_threshold_field = "            threshold_set_content_id: pcs_id('9'),\n"
count = text.count(local_threshold_field)
if count != 2:
    raise SystemExit(f"Expected 2 local eligibility threshold fields, got {count}")
text = text.replace(
    local_threshold_field,
    local_threshold_field
    + "            calibration_manifest_content_id: pcs_id('a'),\n"
    + "            calibration_approval_content_id: pcs_id('b'),\n",
)
text = replace_once(
    text,
    """        let mut changed_threshold_set = base.clone();
        changed_threshold_set.threshold_set_content_id = pcs_id('a');
        assert_ne!(changed_threshold_set.content_id().unwrap(), base_id);

        let mut changed = base;
""",
    """        let mut changed_threshold_set = base.clone();
        changed_threshold_set.threshold_set_content_id = pcs_id('c');
        assert_ne!(changed_threshold_set.content_id().unwrap(), base_id);

        let mut changed_manifest = base.clone();
        changed_manifest.calibration_manifest_content_id = pcs_id('d');
        assert_ne!(changed_manifest.content_id().unwrap(), base_id);

        let mut changed_approval = base.clone();
        changed_approval.calibration_approval_content_id = pcs_id('e');
        assert_ne!(changed_approval.content_id().unwrap(), base_id);

        let mut changed = base;
""",
    "local calibration content ID assertions",
)
legacy_test_start = text.find(
    "    #[test]\n    fn provisional_threshold_set_is_never_implicitly_promoted_to_production()"
)
if legacy_test_start < 0:
    raise SystemExit("Cannot locate legacy threshold gate unit test")
legacy_test_end = text.find("\n    }\n}", legacy_test_start)
if legacy_test_end < 0:
    raise SystemExit("Cannot locate legacy threshold gate unit test end")
legacy_test_end += len("\n    }")
solver_test = """    #[test]
    fn reference_methods_map_to_explicit_calibration_solver_families() {
        assert_eq!(
            solver_family_for_reference(InverseLutValidationReferenceMethod::IndependentPointSolveV1),
            InverseLutCalibrationSolverFamily::IndependentV1
        );
        assert_eq!(
            solver_family_for_reference(
                InverseLutValidationReferenceMethod::FrozenJacobiTrilinearThenV2SolveV1
            ),
            InverseLutCalibrationSolverFamily::PositiveContinuityV2
        );
    }"""
text = text[:legacy_test_start] + solver_test + text[legacy_test_end:]
p.write_text(text, encoding="utf-8", newline="\n")


# ---------------------------------------------------------------------------
# Eligibility integration tests
# ---------------------------------------------------------------------------
p = Path("src/inverse_lut_production_eligibility_tests.rs")
text = p.read_text(encoding="utf-8")
text = replace_once(
    text,
    """use crate::inverse_lut_production_eligibility::{
    InverseLutProductionEligibilityError, validate_inverse_lut_production_eligibility,
};
use crate::inverse_lut_threshold_set::InverseLutValidationThresholdSet;
""",
    """use crate::inverse_lut_production_eligibility::{
    InverseLutProductionEligibility, InverseLutProductionEligibilityError,
    validate_inverse_lut_production_eligibility,
};
use crate::inverse_lut_threshold_set::{
    INVERSE_LUT_THRESHOLD_CALIBRATION_APPROVAL_SCHEMA_VERSION,
    INVERSE_LUT_THRESHOLD_CALIBRATION_MANIFEST_SCHEMA_VERSION,
    InverseLutCalibrationSolverFamily, InverseLutThresholdCalibrationApproval,
    InverseLutThresholdCalibrationManifest, InverseLutThresholdCalibrationObservation,
    InverseLutThresholdSetMethod, InverseLutValidationThresholdSet,
};
""",
    "integration imports",
)
text = replace_once(
    text,
    """fn threshold_set() -> InverseLutValidationThresholdSet {
    InverseLutValidationThresholdSet::provisional_v1()
}
""",
    """fn threshold_set() -> InverseLutValidationThresholdSet {
    let mut threshold_set = InverseLutValidationThresholdSet::provisional_v1();
    threshold_set.method = InverseLutThresholdSetMethod::MeasuredCeramicD50TwoDegreeV1;
    threshold_set
}
""",
    "measured threshold fixture",
)

call_count = text.count("validate_inverse_lut_production_eligibility(")
if call_count != 8:
    raise SystemExit(f"Expected 8 eligibility call sites before wrapper insertion, got {call_count}")
text = text.replace(
    "validate_inverse_lut_production_eligibility(",
    "validate_with_calibration(",
)

helper = """fn prefixed_id(hex: char) -> String {
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

"""
text = insert_before_once(text, "struct FixtureModel {\n", helper, "fixture calibration helpers")
text = replace_once(
    text,
    "fn exact_bindings_still_fail_closed_until_thresholds_are_frozen() {",
    "fn exact_bindings_still_fail_closed_until_calibration_approval_is_allowlisted() {",
    "baseline test name",
)
text = replace_once(
    text,
    "Err(InverseLutProductionEligibilityError::ThresholdsNotProductionFrozen { .. })",
    "Err(InverseLutProductionEligibilityError::CalibrationApprovalNotProductionApproved { .. })",
    "baseline fail-closed error",
)

new_tests = """#[test]
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

"""
text = insert_before_once(
    text,
    "#[test]\nfn forged_lut_payload_is_rehashed_before_eligibility() {\n",
    new_tests,
    "new calibration integration tests",
)
p.write_text(text, encoding="utf-8", newline="\n")
