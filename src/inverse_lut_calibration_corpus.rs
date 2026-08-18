use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::device_characterization_model::{
    ForwardModelValidationPolicy, ForwardModelValidationReport, LocalForwardModelConfig,
    ValidatedLocalForwardModel,
};
use crate::device_characterization_package::{
    CharacterizationMeasurementMetadata, CharacterizationProductionContext,
    CharacterizationValidationLevel, ValidatedCharacterizationPackage,
};
use crate::production_colorimetry::{
    PRODUCTION_PCS_COMPATIBILITY_SCHEMA_VERSION, ProductionPcsCompatibilityMethod,
    ValidatedProductionPcsCompatibility, validate_characterization_for_icc_pcs_lab,
};

pub const INVERSE_LUT_CALIBRATION_CORPUS_SCHEMA_VERSION: u32 = 1;
const LOCAL_FORWARD_MODEL_METHOD_V1: &str = "leave_one_out_local_idw_v1";

#[derive(Clone, Debug)]
pub struct InverseLutCalibrationCorpusInput<'a> {
    pub characterization: &'a ValidatedCharacterizationPackage,
    pub forward_model_config: LocalForwardModelConfig,
    pub forward_model_policy: ForwardModelValidationPolicy,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutCalibrationCorpusEntry {
    pub characterization_id: String,
    pub characterization_revision: String,
    pub validation_level: CharacterizationValidationLevel,
    pub pcs_compatibility_content_id: String,
    pub output_bit_depth: u8,
    pub channel_names: Vec<String>,
    pub measured_channel_max_coverage: Vec<f32>,
    pub measured_total_ink_limit: f32,
    pub sample_count: u64,
    pub production_context: CharacterizationProductionContext,
    pub measurement: CharacterizationMeasurementMetadata,
    pub forward_model_config: LocalForwardModelConfig,
    pub forward_model_policy: ForwardModelValidationPolicy,
    pub forward_model_validation: ForwardModelValidationReport,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InverseLutMeasuredCalibrationCorpus {
    pub schema_version: u32,
    pub pcs_method: ProductionPcsCompatibilityMethod,
    /// Entries are stored in ascending characterization content-ID order.
    pub entries: Vec<InverseLutCalibrationCorpusEntry>,
}

impl InverseLutMeasuredCalibrationCorpus {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != INVERSE_LUT_CALIBRATION_CORPUS_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported inverse-LUT calibration corpus schema {} (expected {}).",
                self.schema_version, INVERSE_LUT_CALIBRATION_CORPUS_SCHEMA_VERSION
            ));
        }
        if self.pcs_method != ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1 {
            errors.push(
                "Inverse-LUT measured calibration corpus requires ICC PCS Lab D50/2°."
                    .to_owned(),
            );
        }
        if self.entries.is_empty() {
            errors.push("Inverse-LUT measured calibration corpus cannot be empty.".to_owned());
        }

        let mut previous_id: Option<&str> = None;
        let mut ids = BTreeSet::new();
        for (index, entry) in self.entries.iter().enumerate() {
            if let Some(previous) = previous_id {
                if previous >= entry.characterization_id.as_str() {
                    errors.push(
                        "Inverse-LUT calibration corpus entries must be strictly sorted by characterization ID."
                            .to_owned(),
                    );
                }
            }
            previous_id = Some(&entry.characterization_id);
            if !ids.insert(entry.characterization_id.as_str()) {
                errors.push(format!(
                    "Inverse-LUT calibration corpus duplicates characterization {}.",
                    entry.characterization_id
                ));
            }
            validate_entry(entry, index, self.pcs_method, &mut errors);
        }

        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }

    pub fn content_id(&self) -> Result<String, String> {
        self.validate().map_err(|errors| errors.join("\n"))?;
        let bytes = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }

    /// Rebuild the entire corpus from the exact external characterization
    /// packages and model settings. This is intentionally stricter than
    /// self-validation: a self-consistent JSON document is not accepted as
    /// evidence for different measured packages or interpolation policies.
    pub fn validate_bindings(
        &self,
        inputs: &[InverseLutCalibrationCorpusInput<'_>],
    ) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if let Err(mut validation_errors) = self.validate() {
            errors.append(&mut validation_errors);
        }
        match build_inverse_lut_measured_calibration_corpus(inputs) {
            Ok(rebuilt) if rebuilt != *self => errors.push(
                "Inverse-LUT measured calibration corpus does not match the supplied characterization/model evidence."
                    .to_owned(),
            ),
            Ok(_) => {}
            Err(error) => errors.push(format!(
                "Supplied measured calibration evidence cannot reproduce the corpus: {error}"
            )),
        }
        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }
}

/// Build an audit-ready measured corpus without making any claim that the input
/// set is representative enough for production threshold calibration.
///
/// Representativeness and final threshold selection remain explicit review
/// steps in #205. This function only proves exact package identity, D50/2° PCS
/// compatibility and successful measured forward-model validation.
pub fn build_inverse_lut_measured_calibration_corpus(
    inputs: &[InverseLutCalibrationCorpusInput<'_>],
) -> Result<InverseLutMeasuredCalibrationCorpus, String> {
    if inputs.is_empty() {
        return Err("Inverse-LUT measured calibration corpus cannot be empty.".to_owned());
    }

    let mut entries = Vec::with_capacity(inputs.len());
    for (index, input) in inputs.iter().enumerate() {
        let package = input.characterization;
        package
            .package()
            .validate()
            .map_err(|errors| format!("Characterization input {index} is invalid: {}", errors.join("\n")))?;
        let payload = &package.package().payload;
        if payload.validation_level != CharacterizationValidationLevel::ProductionValidated {
            return Err(format!(
                "Characterization input {index} ({}) is not ProductionValidated.",
                package.identity().id
            ));
        }

        let pcs = validate_characterization_for_icc_pcs_lab(package).map_err(|error| {
            format!(
                "Characterization input {index} ({}) is not compatible with ICC PCS Lab D50/2°: {error:?}",
                package.identity().id
            )
        })?;
        let pcs_compatibility_content_id = pcs.content_id()?;

        let model = ValidatedLocalForwardModel::build(
            package,
            input.forward_model_config,
            input.forward_model_policy,
        )
        .map_err(|errors| {
            format!(
                "Characterization input {index} ({}) cannot build the validated local forward model: {}",
                package.identity().id,
                errors.join("\n")
            )
        })?;

        entries.push(InverseLutCalibrationCorpusEntry {
            characterization_id: package.identity().id.clone(),
            characterization_revision: payload.revision.clone(),
            validation_level: payload.validation_level,
            pcs_compatibility_content_id,
            output_bit_depth: payload.output_bit_depth,
            channel_names: payload.channel_names.clone(),
            measured_channel_max_coverage: payload.measured_channel_max_coverage.clone(),
            measured_total_ink_limit: payload.measured_total_ink_limit,
            sample_count: u64::try_from(payload.samples.len())
                .map_err(|_| "Characterization sample count does not fit u64.".to_owned())?,
            production_context: payload.production_context.clone(),
            measurement: payload.measurement.clone(),
            forward_model_config: input.forward_model_config,
            forward_model_policy: input.forward_model_policy,
            forward_model_validation: model.validation_report().clone(),
        });
    }

    entries.sort_by(|left, right| left.characterization_id.cmp(&right.characterization_id));
    for pair in entries.windows(2) {
        if pair[0].characterization_id == pair[1].characterization_id {
            return Err(format!(
                "Inverse-LUT measured calibration corpus duplicates characterization {}.",
                pair[0].characterization_id
            ));
        }
    }

    let corpus = InverseLutMeasuredCalibrationCorpus {
        schema_version: INVERSE_LUT_CALIBRATION_CORPUS_SCHEMA_VERSION,
        pcs_method: ProductionPcsCompatibilityMethod::IccPcsLabD50TwoDegreeV1,
        entries,
    };
    corpus.validate().map_err(|errors| errors.join("\n"))?;
    Ok(corpus)
}

fn validate_entry(
    entry: &InverseLutCalibrationCorpusEntry,
    index: usize,
    pcs_method: ProductionPcsCompatibilityMethod,
    errors: &mut Vec<String>,
) {
    if !is_prefixed_sha256(&entry.characterization_id) {
        errors.push(format!(
            "Calibration corpus entry {index} characterization ID must be canonical sha256:<hex>."
        ));
    }
    if entry.characterization_revision.trim().is_empty() {
        errors.push(format!(
            "Calibration corpus entry {index} characterization revision cannot be empty."
        ));
    }
    if entry.validation_level != CharacterizationValidationLevel::ProductionValidated {
        errors.push(format!(
            "Calibration corpus entry {index} must snapshot a ProductionValidated characterization."
        ));
    }
    if !is_prefixed_sha256(&entry.pcs_compatibility_content_id) {
        errors.push(format!(
            "Calibration corpus entry {index} PCS compatibility identity must be canonical sha256:<hex>."
        ));
    } else if is_prefixed_sha256(&entry.characterization_id) {
        let compatibility = ValidatedProductionPcsCompatibility {
            schema_version: PRODUCTION_PCS_COMPATIBILITY_SCHEMA_VERSION,
            method: pcs_method,
            characterization_id: entry.characterization_id.clone(),
            canonical_illuminant: "D50".to_owned(),
            canonical_observer: "2deg".to_owned(),
        };
        match compatibility.content_id() {
            Ok(expected) if expected != entry.pcs_compatibility_content_id => errors.push(format!(
                "Calibration corpus entry {index} PCS compatibility identity mismatch: stored {}, reconstructed {}.",
                entry.pcs_compatibility_content_id, expected
            )),
            Ok(_) => {}
            Err(error) => errors.push(format!(
                "Calibration corpus entry {index} cannot reconstruct PCS compatibility identity: {error}"
            )),
        }
    }

    if !matches!(entry.output_bit_depth, 8 | 16) {
        errors.push(format!(
            "Calibration corpus entry {index} output bit depth must be 8 or 16."
        ));
    }
    if !(4..=12).contains(&entry.channel_names.len()) {
        errors.push(format!(
            "Calibration corpus entry {index} must define 4..=12 channels."
        ));
    }
    if entry.channel_names.len() != entry.measured_channel_max_coverage.len() {
        errors.push(format!(
            "Calibration corpus entry {index} channel topology and measured limits differ in length."
        ));
    }
    let mut names = BTreeSet::new();
    for name in &entry.channel_names {
        let normalized = name.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            errors.push(format!(
                "Calibration corpus entry {index} channel names cannot be empty."
            ));
        } else if !names.insert(normalized) {
            errors.push(format!(
                "Calibration corpus entry {index} contains duplicate channel name {name:?}."
            ));
        }
    }
    for (channel_index, limit) in entry
        .measured_channel_max_coverage
        .iter()
        .copied()
        .enumerate()
    {
        if !limit.is_finite() || !(0.0..=1.0).contains(&limit) || limit == 0.0 {
            errors.push(format!(
                "Calibration corpus entry {index} measured channel limit {} must be finite and in (0, 1].",
                channel_index + 1
            ));
        }
    }
    if !entry.measured_total_ink_limit.is_finite() || entry.measured_total_ink_limit <= 0.0 {
        errors.push(format!(
            "Calibration corpus entry {index} measured total-ink limit must be finite and > 0."
        ));
    } else if entry.measured_total_ink_limit > entry.channel_names.len() as f32 {
        errors.push(format!(
            "Calibration corpus entry {index} measured total-ink limit exceeds normalized channel maximum."
        ));
    }
    if entry.sample_count == 0 {
        errors.push(format!(
            "Calibration corpus entry {index} sample count cannot be zero."
        ));
    }

    validate_context(entry, index, errors);
    validate_model_contract(entry, index, errors);
}

fn validate_context(
    entry: &InverseLutCalibrationCorpusEntry,
    index: usize,
    errors: &mut Vec<String>,
) {
    for (label, value) in [
        ("machine ID", entry.production_context.machine_id.as_str()),
        ("RIP name", entry.production_context.rip_name.as_str()),
        ("RIP version", entry.production_context.rip_version.as_str()),
        (
            "linearization ID",
            entry.production_context.linearization_id.as_str(),
        ),
        ("substrate", entry.production_context.substrate.as_str()),
        ("instrument model", entry.measurement.instrument_model.as_str()),
        ("illuminant", entry.measurement.illuminant.as_str()),
        ("observer", entry.measurement.observer.as_str()),
        (
            "measurement condition",
            entry.measurement.measurement_condition.as_str(),
        ),
    ] {
        if value.trim().is_empty() {
            errors.push(format!(
                "Calibration corpus entry {index} {label} cannot be empty."
            ));
        }
    }
}

fn validate_model_contract(
    entry: &InverseLutCalibrationCorpusEntry,
    index: usize,
    errors: &mut Vec<String>,
) {
    let config = entry.forward_model_config;
    let policy = entry.forward_model_policy;
    if config.neighbor_count < 2 {
        errors.push(format!(
            "Calibration corpus entry {index} forward-model neighbor count must be >= 2."
        ));
    }
    if !config.distance_power.is_finite() || config.distance_power <= 0.0 {
        errors.push(format!(
            "Calibration corpus entry {index} forward-model distance power must be finite and > 0."
        ));
    }
    if !config.max_support_distance.is_finite()
        || config.max_support_distance <= 0.0
        || config.max_support_distance > 1.0
    {
        errors.push(format!(
            "Calibration corpus entry {index} forward-model support distance must be in (0, 1]."
        ));
    }
    if entry.sample_count <= config.neighbor_count as u64 {
        errors.push(format!(
            "Calibration corpus entry {index} needs more measured samples than forward-model neighbors."
        ));
    }

    for (name, value) in [
        ("mean ΔE00", policy.max_mean_delta_e00),
        ("p95 ΔE00", policy.max_p95_delta_e00),
        ("max ΔE00", policy.max_delta_e00),
    ] {
        if !value.is_finite() || value < 0.0 {
            errors.push(format!(
                "Calibration corpus entry {index} forward-model {name} policy must be finite and >= 0."
            ));
        }
    }
    if policy.max_mean_delta_e00 > policy.max_p95_delta_e00
        || policy.max_p95_delta_e00 > policy.max_delta_e00
    {
        errors.push(format!(
            "Calibration corpus entry {index} forward-model policy must satisfy mean <= p95 <= max."
        ));
    }
    if !policy.max_unsupported_fraction.is_finite()
        || !(0.0..=1.0).contains(&policy.max_unsupported_fraction)
    {
        errors.push(format!(
            "Calibration corpus entry {index} forward-model unsupported-fraction policy must be in 0..=1."
        ));
    }

    let report = &entry.forward_model_validation;
    if report.method != LOCAL_FORWARD_MODEL_METHOD_V1 {
        errors.push(format!(
            "Calibration corpus entry {index} forward-model validation method {:?} is not the frozen V1 method.",
            report.method
        ));
    }
    let expected_samples = usize::try_from(entry.sample_count).ok();
    if expected_samples != Some(report.sample_count) {
        errors.push(format!(
            "Calibration corpus entry {index} forward-model report sample count does not match the measured package snapshot."
        ));
    }
    match report.evaluated_count.checked_add(report.unsupported_count) {
        Some(total) if total == report.sample_count => {}
        _ => errors.push(format!(
            "Calibration corpus entry {index} forward-model report counts are inconsistent."
        )),
    }
    let expected_unsupported = if report.sample_count == 0 {
        0.0
    } else {
        report.unsupported_count as f64 / report.sample_count as f64
    };
    if !report.unsupported_fraction.is_finite()
        || !(0.0..=1.0).contains(&report.unsupported_fraction)
        || (report.unsupported_fraction - expected_unsupported).abs() > 1.0e-12
    {
        errors.push(format!(
            "Calibration corpus entry {index} forward-model unsupported fraction is invalid."
        ));
    }
    for (name, value) in [
        ("mean ΔE00", report.mean_delta_e00),
        ("p95 ΔE00", report.p95_delta_e00),
        ("max ΔE00", report.max_delta_e00),
    ] {
        if !value.is_finite() || value < 0.0 {
            errors.push(format!(
                "Calibration corpus entry {index} forward-model report {name} must be finite and >= 0."
            ));
        }
    }
    if report.mean_delta_e00 > report.p95_delta_e00 || report.p95_delta_e00 > report.max_delta_e00 {
        errors.push(format!(
            "Calibration corpus entry {index} forward-model report must satisfy mean <= p95 <= max."
        ));
    }
    if report.mean_delta_e00 > policy.max_mean_delta_e00
        || report.p95_delta_e00 > policy.max_p95_delta_e00
        || report.max_delta_e00 > policy.max_delta_e00
        || report.unsupported_fraction > policy.max_unsupported_fraction
    {
        errors.push(format!(
            "Calibration corpus entry {index} forward-model validation report exceeds its stored policy."
        ));
    }
}

fn is_prefixed_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|bare| {
        bare.len() == 64
            && bare
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_characterization_package::{
        CharacterizationPackage, CharacterizationPayload, CharacterizationSample, MeasuredLabColor,
    };

    fn synthetic_lab(coverages: &[f32]) -> MeasuredLabColor {
        let c = f64::from(coverages[0]);
        let m = f64::from(coverages[1]);
        let y = f64::from(coverages[2]);
        let k = f64::from(coverages[3]);
        MeasuredLabColor {
            l: 96.0 - 18.0 * c - 15.0 * m - 10.0 * y - 45.0 * k,
            a: -4.0 * c + 12.0 * m + 2.0 * y,
            b: -10.0 * c + 2.0 * m + 14.0 * y,
        }
    }

    fn package(
        revision: &str,
        machine: &str,
        illuminant: &str,
        observer: &str,
        validation_level: CharacterizationValidationLevel,
    ) -> ValidatedCharacterizationPackage {
        let mut samples = Vec::new();
        for c in [0.0f32, 0.4, 0.8] {
            for m in [0.0f32, 0.4, 0.8] {
                for y in [0.0f32, 0.4, 0.8] {
                    for k in [0.0f32, 0.4, 0.8] {
                        let coverages = vec![c, m, y, k];
                        samples.push(CharacterizationSample {
                            lab: synthetic_lab(&coverages),
                            coverages,
                        });
                    }
                }
            }
        }
        CharacterizationPackage::new(CharacterizationPayload {
            revision: revision.to_owned(),
            validation_level,
            output_bit_depth: 16,
            channel_names: ["Cyan", "Magenta", "Yellow", "Black"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            measured_channel_max_coverage: vec![0.8; 4],
            measured_total_ink_limit: 3.2,
            production_context: CharacterizationProductionContext {
                machine_id: machine.to_owned(),
                rip_name: "fixture-rip".to_owned(),
                rip_version: "1.0".to_owned(),
                linearization_id: format!("lin-{revision}"),
                substrate: "porcelain-test-tile".to_owned(),
                glaze: Some("fixture-glaze".to_owned()),
                body: Some("fixture-body".to_owned()),
                product_family: Some("fixture-family".to_owned()),
            },
            measurement: CharacterizationMeasurementMetadata {
                instrument_model: "fixture-spectrophotometer".to_owned(),
                instrument_serial: Some(format!("serial-{revision}")),
                illuminant: illuminant.to_owned(),
                observer: observer.to_owned(),
                measurement_condition: "M1".to_owned(),
                measured_at_unix_ms: Some(1_700_000_000_000),
                operator_or_lab: Some("fixture-lab".to_owned()),
            },
            samples,
        })
        .unwrap()
        .validated()
        .unwrap()
    }

    fn config() -> LocalForwardModelConfig {
        LocalForwardModelConfig {
            neighbor_count: 8,
            distance_power: 2.0,
            max_support_distance: 0.5,
        }
    }

    fn policy() -> ForwardModelValidationPolicy {
        ForwardModelValidationPolicy {
            max_mean_delta_e00: 1000.0,
            max_p95_delta_e00: 1000.0,
            max_delta_e00: 1000.0,
            max_unsupported_fraction: 0.0,
        }
    }

    #[test]
    fn corpus_identity_is_deterministic_and_input_order_independent() {
        let first = package(
            "a",
            "machine-a",
            "D50",
            "2deg",
            CharacterizationValidationLevel::ProductionValidated,
        );
        let second = package(
            "b",
            "machine-b",
            "d50",
            "2 degree",
            CharacterizationValidationLevel::ProductionValidated,
        );
        let first_inputs = [
            InverseLutCalibrationCorpusInput {
                characterization: &first,
                forward_model_config: config(),
                forward_model_policy: policy(),
            },
            InverseLutCalibrationCorpusInput {
                characterization: &second,
                forward_model_config: config(),
                forward_model_policy: policy(),
            },
        ];
        let reversed_inputs = [
            InverseLutCalibrationCorpusInput {
                characterization: &second,
                forward_model_config: config(),
                forward_model_policy: policy(),
            },
            InverseLutCalibrationCorpusInput {
                characterization: &first,
                forward_model_config: config(),
                forward_model_policy: policy(),
            },
        ];
        let first_corpus = build_inverse_lut_measured_calibration_corpus(&first_inputs).unwrap();
        let second_corpus = build_inverse_lut_measured_calibration_corpus(&reversed_inputs).unwrap();
        assert_eq!(first_corpus, second_corpus);
        assert_eq!(first_corpus.content_id().unwrap(), second_corpus.content_id().unwrap());
        assert!(first_corpus.validate_bindings(&first_inputs).is_ok());
    }

    #[test]
    fn corpus_rejects_non_d50_characterization() {
        let package = package(
            "d65",
            "machine-d65",
            "D65",
            "2deg",
            CharacterizationValidationLevel::ProductionValidated,
        );
        let inputs = [InverseLutCalibrationCorpusInput {
            characterization: &package,
            forward_model_config: config(),
            forward_model_policy: policy(),
        }];
        assert!(build_inverse_lut_measured_calibration_corpus(&inputs).is_err());
    }

    #[test]
    fn corpus_rejects_non_production_characterization() {
        let package = package(
            "experimental",
            "machine-exp",
            "D50",
            "2deg",
            CharacterizationValidationLevel::Experimental,
        );
        let inputs = [InverseLutCalibrationCorpusInput {
            characterization: &package,
            forward_model_config: config(),
            forward_model_policy: policy(),
        }];
        assert!(build_inverse_lut_measured_calibration_corpus(&inputs).is_err());
    }

    #[test]
    fn corpus_rejects_duplicate_characterization_identity() {
        let package = package(
            "dup",
            "machine-dup",
            "D50",
            "2deg",
            CharacterizationValidationLevel::ProductionValidated,
        );
        let inputs = [
            InverseLutCalibrationCorpusInput {
                characterization: &package,
                forward_model_config: config(),
                forward_model_policy: policy(),
            },
            InverseLutCalibrationCorpusInput {
                characterization: &package,
                forward_model_config: config(),
                forward_model_policy: policy(),
            },
        ];
        assert!(build_inverse_lut_measured_calibration_corpus(&inputs).is_err());
    }

    #[test]
    fn persisted_corpus_revalidates_exact_model_settings() {
        let package = package(
            "binding",
            "machine-binding",
            "D50",
            "2deg",
            CharacterizationValidationLevel::ProductionValidated,
        );
        let inputs = [InverseLutCalibrationCorpusInput {
            characterization: &package,
            forward_model_config: config(),
            forward_model_policy: policy(),
        }];
        let corpus = build_inverse_lut_measured_calibration_corpus(&inputs).unwrap();
        assert!(corpus.validate_bindings(&inputs).is_ok());

        let changed_inputs = [InverseLutCalibrationCorpusInput {
            characterization: &package,
            forward_model_config: LocalForwardModelConfig {
                neighbor_count: 4,
                ..config()
            },
            forward_model_policy: policy(),
        }];
        assert!(corpus.validate_bindings(&changed_inputs).is_err());
    }

    #[test]
    fn self_validation_rejects_tampered_pcs_identity_and_metrics() {
        let package = package(
            "tamper",
            "machine-tamper",
            "D50",
            "2deg",
            CharacterizationValidationLevel::ProductionValidated,
        );
        let inputs = [InverseLutCalibrationCorpusInput {
            characterization: &package,
            forward_model_config: config(),
            forward_model_policy: policy(),
        }];
        let corpus = build_inverse_lut_measured_calibration_corpus(&inputs).unwrap();

        let mut pcs_tamper = corpus.clone();
        pcs_tamper.entries[0].pcs_compatibility_content_id =
            format!("sha256:{}", "f".repeat(64));
        assert!(pcs_tamper.validate().is_err());

        let mut metric_tamper = corpus;
        metric_tamper.entries[0].forward_model_validation.mean_delta_e00 = -0.1;
        assert!(metric_tamper.validate().is_err());
    }
}
