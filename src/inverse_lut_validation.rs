use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::inverse_lut_path_validation::{
    InverseLutPathDiagnostic, InverseLutPathValidationPolicy, InverseLutValidationPathKind,
    path_diagnostics_pass,
};

pub const INVERSE_LUT_VALIDATION_POLICY_SCHEMA_VERSION: u32 = 2;
pub const INVERSE_LUT_VALIDATION_REPORT_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InverseLutHoldoutMethod {
    /// Deterministic cell-center samples plus explicitly versioned neutral,
    /// chromatic and saturated paths. Grid nodes themselves are excluded.
    CellCentersAndFixedPathsV1,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutValidationPolicy {
    pub schema_version: u32,
    pub holdout_method: InverseLutHoldoutMethod,
    pub path_policy: InverseLutPathValidationPolicy,
    pub max_mean_delta_e00: f64,
    pub max_p95_delta_e00: f64,
    pub max_delta_e00: f64,
    pub max_mean_lut_vs_reference_delta_e00: f64,
    pub max_p95_lut_vs_reference_delta_e00: f64,
    pub max_lut_vs_reference_delta_e00: f64,
    pub max_mean_ink_l1: f64,
    pub max_p95_ink_l1: f64,
    pub max_ink_l1: f64,
    pub max_ink_l2: f64,
    pub max_channel_deviation: f64,
    pub max_unsupported_fraction: f64,
    pub max_u8_quantization_l1: f64,
    pub max_u16_quantization_l1: f64,
}

impl Default for InverseLutValidationPolicy {
    fn default() -> Self {
        Self {
            schema_version: INVERSE_LUT_VALIDATION_POLICY_SCHEMA_VERSION,
            holdout_method: InverseLutHoldoutMethod::CellCentersAndFixedPathsV1,
            path_policy: InverseLutPathValidationPolicy::default(),
            // Deliberately conservative placeholders. Production acceptance must
            // explicitly review/version these values against measured fixtures;
            // callers cannot rely on hidden constants in the runner.
            max_mean_delta_e00: 1.0,
            max_p95_delta_e00: 2.0,
            max_delta_e00: 3.0,
            max_mean_lut_vs_reference_delta_e00: 0.75,
            max_p95_lut_vs_reference_delta_e00: 1.5,
            max_lut_vs_reference_delta_e00: 2.0,
            max_mean_ink_l1: 0.15,
            max_p95_ink_l1: 0.30,
            max_ink_l1: 0.50,
            max_ink_l2: 0.30,
            max_channel_deviation: 0.25,
            max_unsupported_fraction: 0.05,
            max_u8_quantization_l1: 0.03,
            max_u16_quantization_l1: 0.001,
        }
    }
}

impl InverseLutValidationPolicy {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != INVERSE_LUT_VALIDATION_POLICY_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported inverse-LUT validation policy schema {} (expected {}).",
                self.schema_version, INVERSE_LUT_VALIDATION_POLICY_SCHEMA_VERSION
            ));
        }
        if let Err(path_errors) = self.path_policy.validate() {
            errors.extend(path_errors);
        }
        for (name, value) in [
            ("max_mean_delta_e00", self.max_mean_delta_e00),
            ("max_p95_delta_e00", self.max_p95_delta_e00),
            ("max_delta_e00", self.max_delta_e00),
            (
                "max_mean_lut_vs_reference_delta_e00",
                self.max_mean_lut_vs_reference_delta_e00,
            ),
            (
                "max_p95_lut_vs_reference_delta_e00",
                self.max_p95_lut_vs_reference_delta_e00,
            ),
            (
                "max_lut_vs_reference_delta_e00",
                self.max_lut_vs_reference_delta_e00,
            ),
            ("max_mean_ink_l1", self.max_mean_ink_l1),
            ("max_p95_ink_l1", self.max_p95_ink_l1),
            ("max_ink_l1", self.max_ink_l1),
            ("max_ink_l2", self.max_ink_l2),
            ("max_channel_deviation", self.max_channel_deviation),
            ("max_u8_quantization_l1", self.max_u8_quantization_l1),
            ("max_u16_quantization_l1", self.max_u16_quantization_l1),
        ] {
            if !value.is_finite() || value < 0.0 {
                errors.push(format!("Inverse-LUT validation {name} must be finite and >= 0."));
            }
        }
        if !self.max_unsupported_fraction.is_finite()
            || !(0.0..=1.0).contains(&self.max_unsupported_fraction)
        {
            errors.push(
                "Inverse-LUT validation max_unsupported_fraction must be finite and in 0..=1."
                    .to_owned(),
            );
        }
        if self.max_mean_delta_e00 > self.max_p95_delta_e00
            || self.max_p95_delta_e00 > self.max_delta_e00
        {
            errors.push(
                "Inverse-LUT DeltaE thresholds must satisfy mean <= p95 <= max.".to_owned(),
            );
        }
        if self.max_mean_lut_vs_reference_delta_e00 > self.max_p95_lut_vs_reference_delta_e00
            || self.max_p95_lut_vs_reference_delta_e00 > self.max_lut_vs_reference_delta_e00
        {
            errors.push(
                "Inverse-LUT LUT-vs-reference DeltaE thresholds must satisfy mean <= p95 <= max."
                    .to_owned(),
            );
        }
        if self.max_mean_ink_l1 > self.max_p95_ink_l1
            || self.max_p95_ink_l1 > self.max_ink_l1
        {
            errors.push("Inverse-LUT ink-L1 thresholds must satisfy mean <= p95 <= max.".to_owned());
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutValidationSample {
    pub supported: bool,
    /// Color error of LUT output evaluated through the authoritative forward model.
    pub lut_delta_e00: Option<f64>,
    /// Color error of the authoritative reference separation for the same target.
    pub reference_delta_e00: Option<f64>,
    /// Direct CIEDE2000 difference between LUT-predicted and reference-predicted Lab.
    pub lut_vs_reference_delta_e00: Option<f64>,
    /// Normalized device-space distance between LUT and reference separation.
    pub ink_l1: Option<f64>,
    pub ink_l2: Option<f64>,
    pub max_channel_deviation: Option<f64>,
    pub u8_quantization_l1: Option<f64>,
    pub u16_quantization_l1: Option<f64>,
    pub constraints_preserved: bool,
}

impl InverseLutValidationSample {
    fn validate(&self, index: usize) -> Result<(), String> {
        if !self.supported {
            if self.lut_delta_e00.is_some()
                || self.reference_delta_e00.is_some()
                || self.lut_vs_reference_delta_e00.is_some()
                || self.ink_l1.is_some()
                || self.ink_l2.is_some()
                || self.max_channel_deviation.is_some()
                || self.u8_quantization_l1.is_some()
                || self.u16_quantization_l1.is_some()
            {
                return Err(format!(
                    "Unsupported inverse-LUT validation sample {index} must not carry numeric metrics."
                ));
            }
            return Ok(());
        }
        for (name, value) in [
            ("lut_delta_e00", self.lut_delta_e00),
            ("reference_delta_e00", self.reference_delta_e00),
            (
                "lut_vs_reference_delta_e00",
                self.lut_vs_reference_delta_e00,
            ),
            ("ink_l1", self.ink_l1),
            ("ink_l2", self.ink_l2),
            ("max_channel_deviation", self.max_channel_deviation),
            ("u8_quantization_l1", self.u8_quantization_l1),
            ("u16_quantization_l1", self.u16_quantization_l1),
        ] {
            match value {
                Some(value) if value.is_finite() && value >= 0.0 => {}
                _ => {
                    return Err(format!(
                        "Supported inverse-LUT validation sample {index} requires finite non-negative {name}."
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ValidationDistribution {
    pub mean: f64,
    pub p95: f64,
    pub max: f64,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutValidationSummary {
    pub total_samples: u64,
    pub supported_samples: u64,
    pub unsupported_samples: u64,
    pub unsupported_fraction: f64,
    pub lut_delta_e00: ValidationDistribution,
    pub reference_delta_e00: ValidationDistribution,
    pub lut_vs_reference_delta_e00: ValidationDistribution,
    pub ink_l1: ValidationDistribution,
    pub max_ink_l2: f64,
    pub max_channel_deviation: f64,
    pub max_u8_quantization_l1: f64,
    pub max_u16_quantization_l1: f64,
    pub constraint_violation_count: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutValidationReport {
    pub schema_version: u32,
    pub lut_identity_content_id: String,
    pub lut_payload_sha256: String,
    pub recipe_sha256: String,
    pub characterization_id: String,
    pub policy: InverseLutValidationPolicy,
    /// Exactly the ordered diagnostic paths required by `holdout_method`.
    /// Missing/reordered paths are rejected so a failing path cannot be omitted
    /// from a persisted report without invalidating its content identity.
    pub path_diagnostics: Vec<InverseLutPathDiagnostic>,
    pub summary: InverseLutValidationSummary,
    pub passed: bool,
}

impl InverseLutValidationReport {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != INVERSE_LUT_VALIDATION_REPORT_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported inverse-LUT validation report schema {} (expected {}).",
                self.schema_version, INVERSE_LUT_VALIDATION_REPORT_SCHEMA_VERSION
            ));
        }
        if !is_prefixed_sha256(&self.lut_identity_content_id) {
            errors.push("Validation report LUT identity must be canonical sha256:<hex>.".to_owned());
        }
        if !is_bare_sha256(&self.lut_payload_sha256) {
            errors.push("Validation report LUT payload SHA-256 must be canonical lowercase hex.".to_owned());
        }
        if !is_bare_sha256(&self.recipe_sha256) {
            errors.push("Validation report recipe SHA-256 must be canonical lowercase hex.".to_owned());
        }
        if !is_prefixed_sha256(&self.characterization_id) {
            errors.push("Validation report characterization ID must be canonical sha256:<hex>.".to_owned());
        }
        if let Err(policy_errors) = self.policy.validate() {
            errors.extend(policy_errors);
        }
        validate_path_set(
            self.policy.holdout_method,
            &self.path_diagnostics,
            &mut errors,
        );
        if self.summary.total_samples
            != self.summary.supported_samples + self.summary.unsupported_samples
        {
            errors.push("Validation report sample counts are inconsistent.".to_owned());
        }
        if !self.summary.unsupported_fraction.is_finite()
            || !(0.0..=1.0).contains(&self.summary.unsupported_fraction)
        {
            errors.push("Validation report unsupported fraction is invalid.".to_owned());
        }
        let expected_pass = summary_passes(&self.summary, &self.policy)
            && path_diagnostics_pass(&self.path_diagnostics, &self.policy.path_policy);
        if self.passed != expected_pass {
            errors.push("Validation report pass flag does not match its metrics/policy.".to_owned());
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

pub fn summarize_validation_samples(
    lut_identity_content_id: String,
    lut_payload_sha256: String,
    recipe_sha256: String,
    characterization_id: String,
    policy: InverseLutValidationPolicy,
    path_diagnostics: Vec<InverseLutPathDiagnostic>,
    samples: &[InverseLutValidationSample],
) -> Result<InverseLutValidationReport, String> {
    policy.validate().map_err(|errors| errors.join("\n"))?;
    if samples.is_empty() {
        return Err("Inverse-LUT validation requires at least one sample.".to_owned());
    }
    let mut path_errors = Vec::new();
    validate_path_set(policy.holdout_method, &path_diagnostics, &mut path_errors);
    if !path_errors.is_empty() {
        return Err(path_errors.join("\n"));
    }
    for (index, sample) in samples.iter().enumerate() {
        sample.validate(index)?;
    }

    let supported = samples.iter().filter(|sample| sample.supported).collect::<Vec<_>>();
    let total_samples = samples.len() as u64;
    let supported_samples = supported.len() as u64;
    let unsupported_samples = total_samples - supported_samples;
    let unsupported_fraction = unsupported_samples as f64 / total_samples as f64;

    let mut lut_delta = Vec::with_capacity(supported.len());
    let mut reference_delta = Vec::with_capacity(supported.len());
    let mut lut_vs_reference_delta = Vec::with_capacity(supported.len());
    let mut ink_l1 = Vec::with_capacity(supported.len());
    let mut max_ink_l2 = 0.0f64;
    let mut max_channel_deviation = 0.0f64;
    let mut max_u8_quantization_l1 = 0.0f64;
    let mut max_u16_quantization_l1 = 0.0f64;
    let mut constraint_violation_count = 0u64;
    for sample in supported {
        lut_delta.push(sample.lut_delta_e00.expect("validated supported metric"));
        reference_delta.push(sample.reference_delta_e00.expect("validated supported metric"));
        lut_vs_reference_delta.push(
            sample
                .lut_vs_reference_delta_e00
                .expect("validated supported metric"),
        );
        ink_l1.push(sample.ink_l1.expect("validated supported metric"));
        max_ink_l2 = max_ink_l2.max(sample.ink_l2.expect("validated supported metric"));
        max_channel_deviation = max_channel_deviation
            .max(sample.max_channel_deviation.expect("validated supported metric"));
        max_u8_quantization_l1 = max_u8_quantization_l1
            .max(sample.u8_quantization_l1.expect("validated supported metric"));
        max_u16_quantization_l1 = max_u16_quantization_l1
            .max(sample.u16_quantization_l1.expect("validated supported metric"));
        if !sample.constraints_preserved {
            constraint_violation_count = constraint_violation_count
                .checked_add(1)
                .ok_or_else(|| "Validation constraint count overflowed u64.".to_owned())?;
        }
    }

    let summary = InverseLutValidationSummary {
        total_samples,
        supported_samples,
        unsupported_samples,
        unsupported_fraction,
        lut_delta_e00: distribution(&mut lut_delta),
        reference_delta_e00: distribution(&mut reference_delta),
        lut_vs_reference_delta_e00: distribution(&mut lut_vs_reference_delta),
        ink_l1: distribution(&mut ink_l1),
        max_ink_l2,
        max_channel_deviation,
        max_u8_quantization_l1,
        max_u16_quantization_l1,
        constraint_violation_count,
    };
    let passed = summary_passes(&summary, &policy)
        && path_diagnostics_pass(&path_diagnostics, &policy.path_policy);
    let report = InverseLutValidationReport {
        schema_version: INVERSE_LUT_VALIDATION_REPORT_SCHEMA_VERSION,
        lut_identity_content_id,
        lut_payload_sha256,
        recipe_sha256,
        characterization_id,
        policy,
        path_diagnostics,
        summary,
        passed,
    };
    report.validate().map_err(|errors| errors.join("\n"))?;
    Ok(report)
}

fn validate_path_set(
    holdout_method: InverseLutHoldoutMethod,
    diagnostics: &[InverseLutPathDiagnostic],
    errors: &mut Vec<String>,
) {
    let expected = match holdout_method {
        InverseLutHoldoutMethod::CellCentersAndFixedPathsV1 => [
            InverseLutValidationPathKind::NeutralAxis,
            InverseLutValidationPathKind::NearNeutralWarm,
            InverseLutValidationPathKind::NearNeutralCool,
            InverseLutValidationPathKind::AAxis,
            InverseLutValidationPathKind::BAxis,
            InverseLutValidationPathKind::AbDiagonal,
            InverseLutValidationPathKind::AbOpposedDiagonal,
        ],
    };
    if diagnostics.len() != expected.len() {
        errors.push(format!(
            "Inverse-LUT validation requires exactly {} ordered path diagnostics, got {}.",
            expected.len(),
            diagnostics.len()
        ));
        return;
    }
    for (index, (diagnostic, expected_kind)) in
        diagnostics.iter().zip(expected).enumerate()
    {
        if diagnostic.kind != expected_kind {
            errors.push(format!(
                "Inverse-LUT validation path {index} kind/order mismatch: expected {expected_kind:?}, got {:?}.",
                diagnostic.kind
            ));
        }
        if let Err(error) = diagnostic.validate() {
            errors.push(format!("Inverse-LUT validation path {index} is invalid: {error}"));
        }
    }
}

fn distribution(values: &mut [f64]) -> ValidationDistribution {
    if values.is_empty() {
        return ValidationDistribution::default();
    }
    values.sort_by(f64::total_cmp);
    let sum = values.iter().sum::<f64>();
    let p95_index = ((values.len() * 95).div_ceil(100)).saturating_sub(1);
    ValidationDistribution {
        mean: sum / values.len() as f64,
        p95: values[p95_index],
        max: *values.last().expect("non-empty distribution"),
    }
}

fn summary_passes(summary: &InverseLutValidationSummary, policy: &InverseLutValidationPolicy) -> bool {
    summary.supported_samples > 0
        && summary.unsupported_fraction <= policy.max_unsupported_fraction
        && summary.lut_delta_e00.mean <= policy.max_mean_delta_e00
        && summary.lut_delta_e00.p95 <= policy.max_p95_delta_e00
        && summary.lut_delta_e00.max <= policy.max_delta_e00
        && summary.lut_vs_reference_delta_e00.mean <= policy.max_mean_lut_vs_reference_delta_e00
        && summary.lut_vs_reference_delta_e00.p95 <= policy.max_p95_lut_vs_reference_delta_e00
        && summary.lut_vs_reference_delta_e00.max <= policy.max_lut_vs_reference_delta_e00
        && summary.ink_l1.mean <= policy.max_mean_ink_l1
        && summary.ink_l1.p95 <= policy.max_p95_ink_l1
        && summary.ink_l1.max <= policy.max_ink_l1
        && summary.max_ink_l2 <= policy.max_ink_l2
        && summary.max_channel_deviation <= policy.max_channel_deviation
        && summary.max_u8_quantization_l1 <= policy.max_u8_quantization_l1
        && summary.max_u16_quantization_l1 <= policy.max_u16_quantization_l1
        && summary.constraint_violation_count == 0
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
