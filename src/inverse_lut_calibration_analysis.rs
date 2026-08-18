use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::inverse_lut_path_validation::{InverseLutPathDiagnostic, InverseLutValidationPathKind};
use crate::inverse_lut_threshold_set::{
    InverseLutCalibrationSolverFamily, InverseLutThresholdCalibrationManifest,
    InverseLutValidationThresholdSet,
};
use crate::inverse_lut_validation::{
    InverseLutValidationReport, InverseLutValidationSummary, ValidationDistribution,
};
use crate::inverse_lut_validation_reference::InverseLutValidationReferenceMethod;
use crate::production_colorimetry::ProductionPcsCompatibilityMethod;

pub const INVERSE_LUT_THRESHOLD_CALIBRATION_ANALYSIS_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutCalibrationDistributionEnvelope {
    pub max_mean: f64,
    pub max_p95: f64,
    pub max_maximum: f64,
}

impl InverseLutCalibrationDistributionEnvelope {
    fn include(&mut self, distribution: ValidationDistribution) {
        self.max_mean = self.max_mean.max(distribution.mean);
        self.max_p95 = self.max_p95.max(distribution.p95);
        self.max_maximum = self.max_maximum.max(distribution.max);
    }

    fn validate(&self, name: &str, errors: &mut Vec<String>) {
        for (field, value) in [
            ("max_mean", self.max_mean),
            ("max_p95", self.max_p95),
            ("max_maximum", self.max_maximum),
        ] {
            if !value.is_finite() || value < 0.0 {
                errors.push(format!(
                    "Inverse-LUT calibration analysis {name}.{field} must be finite and >= 0."
                ));
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutCalibrationPointEnvelope {
    pub report_count: u64,
    pub total_samples: u64,
    pub supported_samples: u64,
    pub unsupported_samples: u64,
    pub max_unsupported_fraction: f64,
    pub lut_delta_e00: InverseLutCalibrationDistributionEnvelope,
    pub reference_delta_e00: InverseLutCalibrationDistributionEnvelope,
    pub lut_vs_reference_delta_e00: InverseLutCalibrationDistributionEnvelope,
    pub ink_l1: InverseLutCalibrationDistributionEnvelope,
    pub max_ink_l2: f64,
    pub max_channel_deviation: f64,
    pub max_u8_quantization_l1: f64,
    pub max_u16_quantization_l1: f64,
    pub constraint_violation_count: u64,
}

impl InverseLutCalibrationPointEnvelope {
    fn include(&mut self, summary: InverseLutValidationSummary) -> Result<(), String> {
        self.report_count = checked_add(self.report_count, 1, "report_count")?;
        self.total_samples =
            checked_add(self.total_samples, summary.total_samples, "total_samples")?;
        self.supported_samples = checked_add(
            self.supported_samples,
            summary.supported_samples,
            "supported_samples",
        )?;
        self.unsupported_samples = checked_add(
            self.unsupported_samples,
            summary.unsupported_samples,
            "unsupported_samples",
        )?;
        self.constraint_violation_count = checked_add(
            self.constraint_violation_count,
            summary.constraint_violation_count,
            "constraint_violation_count",
        )?;
        self.max_unsupported_fraction = self
            .max_unsupported_fraction
            .max(summary.unsupported_fraction);
        self.lut_delta_e00.include(summary.lut_delta_e00);
        self.reference_delta_e00
            .include(summary.reference_delta_e00);
        self.lut_vs_reference_delta_e00
            .include(summary.lut_vs_reference_delta_e00);
        self.ink_l1.include(summary.ink_l1);
        self.max_ink_l2 = self.max_ink_l2.max(summary.max_ink_l2);
        self.max_channel_deviation = self
            .max_channel_deviation
            .max(summary.max_channel_deviation);
        self.max_u8_quantization_l1 = self
            .max_u8_quantization_l1
            .max(summary.max_u8_quantization_l1);
        self.max_u16_quantization_l1 = self
            .max_u16_quantization_l1
            .max(summary.max_u16_quantization_l1);
        Ok(())
    }

    fn validate(&self, errors: &mut Vec<String>) {
        if self.report_count == 0 {
            errors.push("Inverse-LUT calibration analysis requires reports.".to_owned());
        }
        match self.supported_samples.checked_add(self.unsupported_samples) {
            Some(total) if total == self.total_samples => {}
            _ => errors.push(
                "Inverse-LUT calibration analysis aggregate sample counts are inconsistent."
                    .to_owned(),
            ),
        }
        if !self.max_unsupported_fraction.is_finite()
            || !(0.0..=1.0).contains(&self.max_unsupported_fraction)
        {
            errors.push(
                "Inverse-LUT calibration analysis max unsupported fraction is invalid.".to_owned(),
            );
        }
        self.lut_delta_e00.validate("lut_delta_e00", errors);
        self.reference_delta_e00
            .validate("reference_delta_e00", errors);
        self.lut_vs_reference_delta_e00
            .validate("lut_vs_reference_delta_e00", errors);
        self.ink_l1.validate("ink_l1", errors);
        for (name, value) in [
            ("max_ink_l2", self.max_ink_l2),
            ("max_channel_deviation", self.max_channel_deviation),
            ("max_u8_quantization_l1", self.max_u8_quantization_l1),
            ("max_u16_quantization_l1", self.max_u16_quantization_l1),
        ] {
            if !value.is_finite() || value < 0.0 {
                errors.push(format!(
                    "Inverse-LUT calibration analysis {name} must be finite and >= 0."
                ));
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutCalibrationPathEnvelope {
    pub kind: InverseLutValidationPathKind,
    pub report_count: u64,
    pub reports_with_unsupported_samples: u64,
    pub unsupported_samples: u64,
    pub max_channel_jump: Option<f64>,
    pub max_normalized_channel_jump: Option<f64>,
    pub max_vector_l1_jump: Option<f64>,
    pub max_vector_l2_jump: Option<f64>,
    pub max_total_ink_jump: Option<f64>,
    pub max_dominant_channel_switches: Option<u64>,
    pub max_channel_second_difference: Option<f64>,
    pub max_normalized_channel_second_difference: Option<f64>,
    pub max_vector_l1_second_difference: Option<f64>,
    pub max_vector_l2_second_difference: Option<f64>,
    pub max_total_ink_second_difference: Option<f64>,
    pub max_continuity_violation_count: Option<u64>,
    pub max_curvature_violation_count: Option<u64>,
}

impl InverseLutCalibrationPathEnvelope {
    fn new(kind: InverseLutValidationPathKind) -> Self {
        Self {
            kind,
            report_count: 0,
            reports_with_unsupported_samples: 0,
            unsupported_samples: 0,
            max_channel_jump: None,
            max_normalized_channel_jump: None,
            max_vector_l1_jump: None,
            max_vector_l2_jump: None,
            max_total_ink_jump: None,
            max_dominant_channel_switches: None,
            max_channel_second_difference: None,
            max_normalized_channel_second_difference: None,
            max_vector_l1_second_difference: None,
            max_vector_l2_second_difference: None,
            max_total_ink_second_difference: None,
            max_continuity_violation_count: None,
            max_curvature_violation_count: None,
        }
    }

    fn include(&mut self, diagnostic: &InverseLutPathDiagnostic) -> Result<(), String> {
        if diagnostic.kind != self.kind {
            return Err(format!(
                "Inverse-LUT calibration path order mismatch: expected {:?}, got {:?}.",
                self.kind, diagnostic.kind
            ));
        }
        diagnostic.validate()?;
        self.report_count = checked_add(self.report_count, 1, "path report_count")?;
        self.unsupported_samples = checked_add(
            self.unsupported_samples,
            diagnostic.unsupported_samples,
            "path unsupported_samples",
        )?;
        if diagnostic.unsupported_samples > 0 {
            self.reports_with_unsupported_samples = checked_add(
                self.reports_with_unsupported_samples,
                1,
                "path reports_with_unsupported_samples",
            )?;
            return Ok(());
        }

        include_max_f64(&mut self.max_channel_jump, diagnostic.max_channel_jump);
        include_max_f64(
            &mut self.max_normalized_channel_jump,
            diagnostic.max_normalized_channel_jump,
        );
        include_max_f64(&mut self.max_vector_l1_jump, diagnostic.max_vector_l1_jump);
        include_max_f64(&mut self.max_vector_l2_jump, diagnostic.max_vector_l2_jump);
        include_max_f64(&mut self.max_total_ink_jump, diagnostic.max_total_ink_jump);
        include_max_u64(
            &mut self.max_dominant_channel_switches,
            diagnostic.dominant_channel_switches,
        );
        include_max_f64(
            &mut self.max_channel_second_difference,
            diagnostic.max_channel_second_difference,
        );
        include_max_f64(
            &mut self.max_normalized_channel_second_difference,
            diagnostic.max_normalized_channel_second_difference,
        );
        include_max_f64(
            &mut self.max_vector_l1_second_difference,
            diagnostic.max_vector_l1_second_difference,
        );
        include_max_f64(
            &mut self.max_vector_l2_second_difference,
            diagnostic.max_vector_l2_second_difference,
        );
        include_max_f64(
            &mut self.max_total_ink_second_difference,
            diagnostic.max_total_ink_second_difference,
        );
        include_max_u64(
            &mut self.max_continuity_violation_count,
            diagnostic.continuity_violation_count,
        );
        include_max_u64(
            &mut self.max_curvature_violation_count,
            diagnostic.curvature_violation_count,
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutCalibrationAnalysisObservation {
    pub solver_family: InverseLutCalibrationSolverFamily,
    pub validation_report_content_id: String,
    pub characterization_id: String,
    pub recipe_sha256: String,
    pub lut_identity_content_id: String,
    pub reference_method: InverseLutValidationReferenceMethod,
    pub summary: InverseLutValidationSummary,
    pub path_diagnostics: Vec<InverseLutPathDiagnostic>,
    /// Diagnostic only. Calibration analysis may include failing provisional
    /// reports; this flag never authorizes production thresholds.
    pub report_passed_current_policy: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutThresholdCalibrationAnalysis {
    pub schema_version: u32,
    pub pcs_method: ProductionPcsCompatibilityMethod,
    pub threshold_set_content_id: String,
    pub calibration_manifest_content_id: String,
    pub observations: Vec<InverseLutCalibrationAnalysisObservation>,
    pub point_envelope: InverseLutCalibrationPointEnvelope,
    pub path_envelopes: Vec<InverseLutCalibrationPathEnvelope>,
    pub all_reports_passed_current_policy: bool,
}

impl InverseLutThresholdCalibrationAnalysis {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != INVERSE_LUT_THRESHOLD_CALIBRATION_ANALYSIS_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported inverse-LUT threshold calibration analysis schema {} (expected {}).",
                self.schema_version, INVERSE_LUT_THRESHOLD_CALIBRATION_ANALYSIS_SCHEMA_VERSION
            ));
        }
        if self.pcs_method != ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1 {
            errors.push(
                "Inverse-LUT threshold calibration analysis requires ICC PCS Lab D50/2°."
                    .to_owned(),
            );
        }
        for (name, value) in [
            (
                "threshold_set_content_id",
                self.threshold_set_content_id.as_str(),
            ),
            (
                "calibration_manifest_content_id",
                self.calibration_manifest_content_id.as_str(),
            ),
        ] {
            if !is_prefixed_sha256(value) {
                errors.push(format!(
                    "Inverse-LUT threshold calibration analysis {name} must be canonical sha256:<hex>."
                ));
            }
        }
        if self.observations.is_empty() {
            errors.push(
                "Inverse-LUT threshold calibration analysis requires observations.".to_owned(),
            );
        }

        let mut report_ids = BTreeSet::new();
        let mut families = BTreeSet::new();
        for (index, observation) in self.observations.iter().enumerate() {
            if !report_ids.insert(observation.validation_report_content_id.as_str()) {
                errors.push(format!(
                    "Inverse-LUT calibration analysis duplicates report {}.",
                    observation.validation_report_content_id
                ));
            }
            families.insert(observation.solver_family);
            if !is_prefixed_sha256(&observation.validation_report_content_id)
                || !is_prefixed_sha256(&observation.characterization_id)
                || !is_prefixed_sha256(&observation.lut_identity_content_id)
                || !is_bare_sha256(&observation.recipe_sha256)
            {
                errors.push(format!(
                    "Inverse-LUT calibration analysis observation {index} contains a non-canonical identity."
                ));
            }
            validate_summary(&observation.summary, index, &mut errors);
            if !reference_matches_solver_family(
                observation.solver_family,
                observation.reference_method,
            ) {
                errors.push(format!(
                    "Inverse-LUT calibration analysis observation {index} solver family does not match reference method."
                ));
            }
            for diagnostic in &observation.path_diagnostics {
                if let Err(error) = diagnostic.validate() {
                    errors.push(format!(
                        "Inverse-LUT calibration analysis observation {index} path diagnostic is invalid: {error}"
                    ));
                }
            }
        }
        for required in [
            InverseLutCalibrationSolverFamily::IndependentV1,
            InverseLutCalibrationSolverFamily::PositiveContinuityV2,
        ] {
            if !families.contains(&required) {
                errors.push(format!(
                    "Inverse-LUT calibration analysis is missing solver family {required:?}."
                ));
            }
        }

        self.point_envelope.validate(&mut errors);
        match compute_envelopes(&self.observations) {
            Ok((point, paths)) => {
                if point != self.point_envelope {
                    errors.push(
                        "Inverse-LUT calibration point envelope does not match observations."
                            .to_owned(),
                    );
                }
                if paths != self.path_envelopes {
                    errors.push(
                        "Inverse-LUT calibration path envelopes do not match observations."
                            .to_owned(),
                    );
                }
            }
            Err(error) => errors.push(error),
        }
        let expected_all_passed = self
            .observations
            .iter()
            .all(|observation| observation.report_passed_current_policy);
        if self.all_reports_passed_current_policy != expected_all_passed {
            errors.push(
                "Inverse-LUT calibration analysis all-reports-pass flag is inconsistent."
                    .to_owned(),
            );
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn content_id(&self) -> Result<String, String> {
        self.validate().map_err(|errors| errors.join("\n"))?;
        let bytes = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

/// Builds a deterministic, review-only calibration analysis. This function does
/// not select or approve numerical thresholds.
pub fn analyze_inverse_lut_threshold_calibration(
    threshold_set: &InverseLutValidationThresholdSet,
    manifest: &InverseLutThresholdCalibrationManifest,
    reports: &[InverseLutValidationReport],
) -> Result<InverseLutThresholdCalibrationAnalysis, String> {
    threshold_set
        .validate()
        .map_err(|errors| errors.join("\n"))?;
    manifest.validate().map_err(|errors| errors.join("\n"))?;
    let threshold_set_content_id = threshold_set.content_id()?;
    if manifest.threshold_set_content_id != threshold_set_content_id {
        return Err(format!(
            "Calibration manifest threshold set {} does not match supplied threshold set {}.",
            manifest.threshold_set_content_id, threshold_set_content_id
        ));
    }
    let calibration_manifest_content_id = manifest.content_id()?;

    let mut reports_by_id = BTreeMap::new();
    for (index, report) in reports.iter().enumerate() {
        report.validate().map_err(|errors| {
            format!(
                "Calibration report {index} is invalid: {}",
                errors.join("\n")
            )
        })?;
        let report_id = report.content_id()?;
        if reports_by_id.insert(report_id.clone(), report).is_some() {
            return Err(format!("Calibration report {report_id} is duplicated."));
        }
    }

    if reports_by_id.len() != manifest.observations.len() {
        return Err(format!(
            "Calibration report count {} does not match manifest observation count {}.",
            reports_by_id.len(),
            manifest.observations.len()
        ));
    }

    let mut observations = Vec::with_capacity(manifest.observations.len());
    for (index, expected) in manifest.observations.iter().enumerate() {
        let report = reports_by_id
            .remove(&expected.validation_report_content_id)
            .ok_or_else(|| {
                format!(
                    "Calibration manifest observation {index} references missing report {}.",
                    expected.validation_report_content_id
                )
            })?;
        for (name, expected_value, actual_value) in [
            (
                "characterization_id",
                expected.characterization_id.as_str(),
                report.characterization_id.as_str(),
            ),
            (
                "recipe_sha256",
                expected.recipe_sha256.as_str(),
                report.recipe_sha256.as_str(),
            ),
            (
                "lut_identity_content_id",
                expected.lut_identity_content_id.as_str(),
                report.lut_identity_content_id.as_str(),
            ),
        ] {
            if expected_value != actual_value {
                return Err(format!(
                    "Calibration manifest observation {index} {name} {expected_value} does not match report {actual_value}."
                ));
            }
        }
        if report.threshold_set_content_id != threshold_set_content_id {
            return Err(format!(
                "Calibration report {} is bound to threshold set {}, expected {}.",
                expected.validation_report_content_id,
                report.threshold_set_content_id,
                threshold_set_content_id
            ));
        }
        if report.policy != threshold_set.policy {
            return Err(format!(
                "Calibration report {} numerical policy does not match the supplied threshold set.",
                expected.validation_report_content_id
            ));
        }
        if !reference_matches_solver_family(expected.solver_family, report.reference_method) {
            return Err(format!(
                "Calibration report {} reference method {:?} does not match solver family {:?}.",
                expected.validation_report_content_id,
                report.reference_method,
                expected.solver_family
            ));
        }
        observations.push(InverseLutCalibrationAnalysisObservation {
            solver_family: expected.solver_family,
            validation_report_content_id: expected.validation_report_content_id.clone(),
            characterization_id: report.characterization_id.clone(),
            recipe_sha256: report.recipe_sha256.clone(),
            lut_identity_content_id: report.lut_identity_content_id.clone(),
            reference_method: report.reference_method,
            summary: report.summary,
            path_diagnostics: report.path_diagnostics.clone(),
            report_passed_current_policy: report.passed,
        });
    }
    if let Some(extra) = reports_by_id.keys().next() {
        return Err(format!(
            "Calibration report {extra} is not referenced by the manifest."
        ));
    }

    let (point_envelope, path_envelopes) = compute_envelopes(&observations)?;
    let all_reports_passed_current_policy = observations
        .iter()
        .all(|observation| observation.report_passed_current_policy);
    let analysis = InverseLutThresholdCalibrationAnalysis {
        schema_version: INVERSE_LUT_THRESHOLD_CALIBRATION_ANALYSIS_SCHEMA_VERSION,
        pcs_method: manifest.pcs_method,
        threshold_set_content_id,
        calibration_manifest_content_id,
        observations,
        point_envelope,
        path_envelopes,
        all_reports_passed_current_policy,
    };
    analysis.validate().map_err(|errors| errors.join("\n"))?;
    Ok(analysis)
}

fn compute_envelopes(
    observations: &[InverseLutCalibrationAnalysisObservation],
) -> Result<
    (
        InverseLutCalibrationPointEnvelope,
        Vec<InverseLutCalibrationPathEnvelope>,
    ),
    String,
> {
    if observations.is_empty() {
        return Err("Inverse-LUT calibration analysis requires observations.".to_owned());
    }
    let mut point = InverseLutCalibrationPointEnvelope::default();
    let mut paths = observations[0]
        .path_diagnostics
        .iter()
        .map(|diagnostic| InverseLutCalibrationPathEnvelope::new(diagnostic.kind))
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return Err("Inverse-LUT calibration analysis requires path diagnostics.".to_owned());
    }
    for observation in observations {
        point.include(observation.summary)?;
        if observation.path_diagnostics.len() != paths.len() {
            return Err(
                "Inverse-LUT calibration analysis observations have different path counts."
                    .to_owned(),
            );
        }
        for (envelope, diagnostic) in paths.iter_mut().zip(&observation.path_diagnostics) {
            envelope.include(diagnostic)?;
        }
    }
    Ok((point, paths))
}

fn validate_summary(summary: &InverseLutValidationSummary, index: usize, errors: &mut Vec<String>) {
    match summary
        .supported_samples
        .checked_add(summary.unsupported_samples)
    {
        Some(total) if total == summary.total_samples => {}
        _ => errors.push(format!(
            "Inverse-LUT calibration analysis observation {index} sample counts are inconsistent."
        )),
    }
    if summary.total_samples == 0 {
        errors.push(format!(
            "Inverse-LUT calibration analysis observation {index} cannot have zero samples."
        ));
    }
    let expected_fraction = if summary.total_samples == 0 {
        0.0
    } else {
        summary.unsupported_samples as f64 / summary.total_samples as f64
    };
    if !summary.unsupported_fraction.is_finite()
        || !(0.0..=1.0).contains(&summary.unsupported_fraction)
        || (summary.unsupported_fraction - expected_fraction).abs() > 1.0e-12
    {
        errors.push(format!(
            "Inverse-LUT calibration analysis observation {index} unsupported fraction is invalid."
        ));
    }
    for (name, distribution) in [
        ("lut_delta_e00", summary.lut_delta_e00),
        ("reference_delta_e00", summary.reference_delta_e00),
        (
            "lut_vs_reference_delta_e00",
            summary.lut_vs_reference_delta_e00,
        ),
        ("ink_l1", summary.ink_l1),
    ] {
        for (field, value) in [
            ("mean", distribution.mean),
            ("p95", distribution.p95),
            ("max", distribution.max),
        ] {
            if !value.is_finite() || value < 0.0 {
                errors.push(format!(
                    "Inverse-LUT calibration analysis observation {index} {name}.{field} must be finite and >= 0."
                ));
            }
        }
        if distribution.mean > distribution.p95 || distribution.p95 > distribution.max {
            errors.push(format!(
                "Inverse-LUT calibration analysis observation {index} {name} must satisfy mean <= p95 <= max."
            ));
        }
    }
    for (name, value) in [
        ("max_ink_l2", summary.max_ink_l2),
        ("max_channel_deviation", summary.max_channel_deviation),
        ("max_u8_quantization_l1", summary.max_u8_quantization_l1),
        ("max_u16_quantization_l1", summary.max_u16_quantization_l1),
    ] {
        if !value.is_finite() || value < 0.0 {
            errors.push(format!(
                "Inverse-LUT calibration analysis observation {index} {name} must be finite and >= 0."
            ));
        }
    }
    if summary.constraint_violation_count > summary.supported_samples {
        errors.push(format!(
            "Inverse-LUT calibration analysis observation {index} constraint violations exceed supported samples."
        ));
    }
}

fn reference_matches_solver_family(
    family: InverseLutCalibrationSolverFamily,
    method: InverseLutValidationReferenceMethod,
) -> bool {
    matches!(
        (family, method),
        (
            InverseLutCalibrationSolverFamily::IndependentV1,
            InverseLutValidationReferenceMethod::IndependentPointSolveV1
        ) | (
            InverseLutCalibrationSolverFamily::PositiveContinuityV2,
            InverseLutValidationReferenceMethod::FrozenJacobiTrilinearThenV2SolveV1
        )
    )
}

fn include_max_f64(target: &mut Option<f64>, value: Option<f64>) {
    if let Some(value) = value {
        *target = Some(target.map_or(value, |current| current.max(value)));
    }
}

fn include_max_u64(target: &mut Option<u64>, value: Option<u64>) {
    if let Some(value) = value {
        *target = Some(target.map_or(value, |current| current.max(value)));
    }
}

fn checked_add(left: u64, right: u64, name: &str) -> Result<u64, String> {
    left.checked_add(right)
        .ok_or_else(|| format!("Inverse-LUT calibration analysis {name} overflow."))
}

fn is_prefixed_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(is_bare_sha256)
}

fn is_bare_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inverse_lut_threshold_set::{
        INVERSE_LUT_THRESHOLD_CALIBRATION_MANIFEST_SCHEMA_VERSION,
        InverseLutThresholdCalibrationObservation,
    };
    use crate::inverse_lut_validation::INVERSE_LUT_VALIDATION_REPORT_SCHEMA_VERSION;

    fn prefixed(hex: char) -> String {
        format!("sha256:{}", hex.to_string().repeat(64))
    }

    fn bare(hex: char) -> String {
        hex.to_string().repeat(64)
    }

    fn path_kinds() -> [InverseLutValidationPathKind; 7] {
        [
            InverseLutValidationPathKind::NeutralAxis,
            InverseLutValidationPathKind::NearNeutralWarm,
            InverseLutValidationPathKind::NearNeutralCool,
            InverseLutValidationPathKind::AAxis,
            InverseLutValidationPathKind::BAxis,
            InverseLutValidationPathKind::AbDiagonal,
            InverseLutValidationPathKind::AbOpposedDiagonal,
        ]
    }

    fn supported_path(kind: InverseLutValidationPathKind) -> InverseLutPathDiagnostic {
        InverseLutPathDiagnostic {
            kind,
            sample_count: 4,
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
        }
    }

    fn report(
        threshold_set: &InverseLutValidationThresholdSet,
        family: InverseLutCalibrationSolverFamily,
        identity_hex: char,
    ) -> InverseLutValidationReport {
        let reference_method = match family {
            InverseLutCalibrationSolverFamily::IndependentV1 => {
                InverseLutValidationReferenceMethod::IndependentPointSolveV1
            }
            InverseLutCalibrationSolverFamily::PositiveContinuityV2 => {
                InverseLutValidationReferenceMethod::FrozenJacobiTrilinearThenV2SolveV1
            }
        };
        InverseLutValidationReport {
            schema_version: INVERSE_LUT_VALIDATION_REPORT_SCHEMA_VERSION,
            lut_identity_content_id: prefixed(identity_hex),
            lut_payload_sha256: bare('a'),
            recipe_sha256: bare(identity_hex),
            characterization_id: prefixed('c'),
            threshold_set_content_id: threshold_set.content_id().unwrap(),
            policy: threshold_set.policy,
            reference_method,
            path_diagnostics: path_kinds().into_iter().map(supported_path).collect(),
            summary: InverseLutValidationSummary {
                total_samples: 4,
                supported_samples: 4,
                unsupported_samples: 0,
                unsupported_fraction: 0.0,
                lut_delta_e00: ValidationDistribution::default(),
                reference_delta_e00: ValidationDistribution::default(),
                lut_vs_reference_delta_e00: ValidationDistribution::default(),
                ink_l1: ValidationDistribution::default(),
                max_ink_l2: 0.0,
                max_channel_deviation: 0.0,
                max_u8_quantization_l1: 0.0,
                max_u16_quantization_l1: 0.0,
                constraint_violation_count: 0,
            },
            passed: true,
        }
    }

    fn manifest(
        threshold_set: &InverseLutValidationThresholdSet,
        reports: &[(
            InverseLutCalibrationSolverFamily,
            InverseLutValidationReport,
        )],
    ) -> InverseLutThresholdCalibrationManifest {
        let observations = reports
            .iter()
            .map(
                |(family, report)| InverseLutThresholdCalibrationObservation {
                    solver_family: *family,
                    characterization_id: report.characterization_id.clone(),
                    recipe_sha256: report.recipe_sha256.clone(),
                    lut_identity_content_id: report.lut_identity_content_id.clone(),
                    validation_report_content_id: report.content_id().unwrap(),
                },
            )
            .collect();
        InverseLutThresholdCalibrationManifest {
            schema_version: INVERSE_LUT_THRESHOLD_CALIBRATION_MANIFEST_SCHEMA_VERSION,
            pcs_method: ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
            threshold_set_content_id: threshold_set.content_id().unwrap(),
            observations,
        }
    }

    fn fixture() -> (
        InverseLutValidationThresholdSet,
        InverseLutThresholdCalibrationManifest,
        Vec<InverseLutValidationReport>,
    ) {
        let threshold_set = InverseLutValidationThresholdSet::provisional_v1();
        let first = report(
            &threshold_set,
            InverseLutCalibrationSolverFamily::IndependentV1,
            '1',
        );
        let second = report(
            &threshold_set,
            InverseLutCalibrationSolverFamily::PositiveContinuityV2,
            '2',
        );
        let pairs = vec![
            (
                InverseLutCalibrationSolverFamily::IndependentV1,
                first.clone(),
            ),
            (
                InverseLutCalibrationSolverFamily::PositiveContinuityV2,
                second.clone(),
            ),
        ];
        let manifest = manifest(&threshold_set, &pairs);
        (threshold_set, manifest, vec![first, second])
    }

    #[test]
    fn analysis_is_deterministic_and_input_report_order_independent() {
        let (threshold_set, manifest, reports) = fixture();
        let first =
            analyze_inverse_lut_threshold_calibration(&threshold_set, &manifest, &reports).unwrap();
        let mut reversed = reports.clone();
        reversed.reverse();
        let second =
            analyze_inverse_lut_threshold_calibration(&threshold_set, &manifest, &reversed)
                .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.content_id().unwrap(), second.content_id().unwrap());
    }

    #[test]
    fn analysis_rejects_manifest_metadata_mismatch() {
        let (threshold_set, mut manifest, reports) = fixture();
        manifest.observations[0].recipe_sha256 = bare('f');
        assert!(
            analyze_inverse_lut_threshold_calibration(&threshold_set, &manifest, &reports).is_err()
        );
    }

    #[test]
    fn analysis_rejects_solver_family_reference_mismatch() {
        let (threshold_set, mut manifest, reports) = fixture();
        manifest.observations.swap(0, 1);
        manifest.observations[0].solver_family = InverseLutCalibrationSolverFamily::IndependentV1;
        manifest.observations[1].solver_family =
            InverseLutCalibrationSolverFamily::PositiveContinuityV2;
        assert!(
            analyze_inverse_lut_threshold_calibration(&threshold_set, &manifest, &reports).is_err()
        );
    }

    #[test]
    fn unsupported_path_coverage_remains_explicit() {
        let threshold_set = InverseLutValidationThresholdSet::provisional_v1();
        let mut first = report(
            &threshold_set,
            InverseLutCalibrationSolverFamily::IndependentV1,
            '1',
        );
        first.path_diagnostics[0] = InverseLutPathDiagnostic {
            kind: InverseLutValidationPathKind::NeutralAxis,
            sample_count: 4,
            unsupported_samples: 1,
            max_channel_jump: None,
            max_normalized_channel_jump: None,
            max_vector_l1_jump: None,
            max_vector_l2_jump: None,
            max_total_ink_jump: None,
            dominant_channel_switches: None,
            max_channel_second_difference: None,
            max_normalized_channel_second_difference: None,
            max_vector_l1_second_difference: None,
            max_vector_l2_second_difference: None,
            max_total_ink_second_difference: None,
            continuity_violation_count: None,
            curvature_violation_count: None,
        };
        first.passed = false;
        assert!(first.validate().is_ok());
        let second = report(
            &threshold_set,
            InverseLutCalibrationSolverFamily::PositiveContinuityV2,
            '2',
        );
        let pairs = vec![
            (
                InverseLutCalibrationSolverFamily::IndependentV1,
                first.clone(),
            ),
            (
                InverseLutCalibrationSolverFamily::PositiveContinuityV2,
                second.clone(),
            ),
        ];
        let manifest = manifest(&threshold_set, &pairs);
        let analysis =
            analyze_inverse_lut_threshold_calibration(&threshold_set, &manifest, &[first, second])
                .unwrap();
        assert_eq!(
            analysis.path_envelopes[0].reports_with_unsupported_samples,
            1
        );
        assert_eq!(analysis.path_envelopes[0].unsupported_samples, 1);
        assert_eq!(analysis.path_envelopes[0].max_channel_jump, Some(0.0));
        assert!(!analysis.all_reports_passed_current_policy);
    }

    #[test]
    fn analysis_rejects_tampered_negative_observation_summary() {
        let (threshold_set, manifest, reports) = fixture();
        let mut analysis =
            analyze_inverse_lut_threshold_calibration(&threshold_set, &manifest, &reports).unwrap();
        analysis.observations[0].summary.lut_delta_e00.mean = -0.25;
        let (point, paths) = compute_envelopes(&analysis.observations).unwrap();
        analysis.point_envelope = point;
        analysis.path_envelopes = paths;
        assert!(analysis.validate().is_err());
    }

    #[test]
    fn analysis_rejects_extra_report() {
        let (threshold_set, manifest, mut reports) = fixture();
        reports.push(report(
            &threshold_set,
            InverseLutCalibrationSolverFamily::IndependentV1,
            '3',
        ));
        assert!(
            analyze_inverse_lut_threshold_calibration(&threshold_set, &manifest, &reports).is_err()
        );
    }
}
