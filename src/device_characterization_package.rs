use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::color_conversion::ConversionTargetDefinition;
use crate::device_characterization::{CharacterizationIdentity, LabColor};

pub const CHARACTERIZATION_PACKAGE_SCHEMA_VERSION: u32 = 1;
pub const MAX_CHARACTERIZATION_PACKAGE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CharacterizationValidationLevel {
    Experimental,
    ProductionValidated,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CharacterizationProductionContext {
    pub machine_id: String,
    pub rip_name: String,
    pub rip_version: String,
    pub linearization_id: String,
    pub substrate: String,
    #[serde(default)]
    pub glaze: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub product_family: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CharacterizationMeasurementMetadata {
    pub instrument_model: String,
    #[serde(default)]
    pub instrument_serial: Option<String>,
    pub illuminant: String,
    pub observer: String,
    pub measurement_condition: String,
    #[serde(default)]
    pub measured_at_unix_ms: Option<i64>,
    #[serde(default)]
    pub operator_or_lab: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct MeasuredLabColor {
    pub l: f64,
    pub a: f64,
    pub b: f64,
}

impl From<MeasuredLabColor> for LabColor {
    fn from(value: MeasuredLabColor) -> Self {
        Self {
            l: value.l,
            a: value.a,
            b: value.b,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CharacterizationSample {
    /// Normalized direct-coverage values in exact authoritative channel order.
    pub coverages: Vec<f32>,
    pub lab: MeasuredLabColor,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CharacterizationPayload {
    /// Human-readable dataset revision. The cryptographic package ID is separate.
    pub revision: String,
    pub validation_level: CharacterizationValidationLevel,
    pub output_bit_depth: u8,
    pub channel_names: Vec<String>,
    /// Maximum normalized coverage that was validated/measured for each channel.
    pub measured_channel_max_coverage: Vec<f32>,
    /// Maximum normalized sum of all channels covered by this dataset.
    pub measured_total_ink_limit: f32,
    pub production_context: CharacterizationProductionContext,
    pub measurement: CharacterizationMeasurementMetadata,
    pub samples: Vec<CharacterizationSample>,
}

/// Portable, content-addressed measured characterization package.
///
/// `id` is always `sha256:<hex>` over the serialized payload only. Therefore a
/// same-named file or revised dataset cannot silently keep an old production
/// identity. `ConversionTargetDefinition::characterization_id` stores this exact
/// content address.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CharacterizationPackage {
    pub schema_version: u32,
    pub id: String,
    pub payload: CharacterizationPayload,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedCharacterizationPackage {
    package: CharacterizationPackage,
    identity: CharacterizationIdentity,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CharacterizationTargetError {
    InvalidPackage(Vec<String>),
    MissingTargetCharacterization,
    IdentityMismatch { target: String, package: String },
    ChannelTopologyMismatch { target: Vec<String>, package: Vec<String> },
    BitDepthMismatch { target: u8, package: u8 },
    MissingChannelLimit(String),
    ChannelLimitExceedsMeasured {
        channel: String,
        target: f32,
        measured: f32,
    },
    MissingTotalInkLimit,
    TotalInkLimitExceedsMeasured { target: f32, measured: f32 },
    PackageNotProductionValidated,
}

impl CharacterizationPackage {
    pub fn new(payload: CharacterizationPayload) -> Result<Self, Vec<String>> {
        let id = payload_content_id(&payload)
            .map_err(|err| vec![format!("Cannot fingerprint characterization payload: {err}")])?;
        let package = Self {
            schema_version: CHARACTERIZATION_PACKAGE_SCHEMA_VERSION,
            id,
            payload,
        };
        package.validate()?;
        Ok(package)
    }

    pub fn expected_content_id(&self) -> Result<String, String> {
        payload_content_id(&self.payload)
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.schema_version != CHARACTERIZATION_PACKAGE_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported characterization package schema {} (expected {}).",
                self.schema_version, CHARACTERIZATION_PACKAGE_SCHEMA_VERSION
            ));
        }

        match self.expected_content_id() {
            Ok(expected) if expected != self.id => errors.push(format!(
                "Characterization content identity mismatch: package declares '{}', payload is '{}'.",
                self.id, expected
            )),
            Err(err) => errors.push(format!("Cannot fingerprint characterization payload: {err}")),
            Ok(_) => {}
        }

        validate_payload(&self.payload, &mut errors);

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn validated(self) -> Result<ValidatedCharacterizationPackage, Vec<String>> {
        self.validate()?;
        let identity = CharacterizationIdentity {
            id: self.id.clone(),
            channel_names: self.payload.channel_names.clone(),
        };
        Ok(ValidatedCharacterizationPackage {
            package: self,
            identity,
        })
    }
}

impl ValidatedCharacterizationPackage {
    pub fn package(&self) -> &CharacterizationPackage {
        &self.package
    }

    pub fn identity(&self) -> &CharacterizationIdentity {
        &self.identity
    }

    pub fn into_package(self) -> CharacterizationPackage {
        self.package
    }
}

pub fn load_characterization_package(
    path: &Path,
) -> Result<ValidatedCharacterizationPackage, String> {
    let metadata = fs::metadata(path).map_err(|err| {
        format!(
            "Cannot inspect characterization package {}: {err}",
            path.display()
        )
    })?;
    if metadata.len() > MAX_CHARACTERIZATION_PACKAGE_BYTES {
        return Err(format!(
            "Characterization package {} is {} bytes; maximum accepted size is {} bytes.",
            path.display(),
            metadata.len(),
            MAX_CHARACTERIZATION_PACKAGE_BYTES
        ));
    }

    let bytes = fs::read(path).map_err(|err| {
        format!(
            "Cannot read characterization package {}: {err}",
            path.display()
        )
    })?;
    let package: CharacterizationPackage = serde_json::from_slice(&bytes).map_err(|err| {
        format!(
            "Cannot parse characterization package {}: {err}",
            path.display()
        )
    })?;
    package
        .validated()
        .map_err(|errors| errors.join("\n"))
}

/// Validate that a content-addressed measured package is safe for the selected
/// production target. This deliberately does not create an interpolation model;
/// the next optimizer slice must define and validate that model separately.
pub fn validate_characterization_for_target(
    package: &CharacterizationPackage,
    target: &ConversionTargetDefinition,
    require_production_validation: bool,
) -> Result<(), CharacterizationTargetError> {
    package
        .validate()
        .map_err(CharacterizationTargetError::InvalidPackage)?;

    let target_id = target
        .characterization_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(CharacterizationTargetError::MissingTargetCharacterization)?;
    if target_id != package.id {
        return Err(CharacterizationTargetError::IdentityMismatch {
            target: target_id.to_owned(),
            package: package.id.clone(),
        });
    }

    let target_channels = target
        .channels
        .iter()
        .map(|channel| channel.name.clone())
        .collect::<Vec<_>>();
    if target_channels != package.payload.channel_names {
        return Err(CharacterizationTargetError::ChannelTopologyMismatch {
            target: target_channels,
            package: package.payload.channel_names.clone(),
        });
    }

    if target.bit_depth != package.payload.output_bit_depth {
        return Err(CharacterizationTargetError::BitDepthMismatch {
            target: target.bit_depth,
            package: package.payload.output_bit_depth,
        });
    }

    for ((channel, target_channel), measured_limit) in package
        .payload
        .channel_names
        .iter()
        .zip(&target.channels)
        .zip(&package.payload.measured_channel_max_coverage)
    {
        let target_limit = target_channel
            .max_coverage
            .ok_or_else(|| CharacterizationTargetError::MissingChannelLimit(channel.clone()))?;
        if target_limit > *measured_limit {
            return Err(CharacterizationTargetError::ChannelLimitExceedsMeasured {
                channel: channel.clone(),
                target: target_limit,
                measured: *measured_limit,
            });
        }
    }

    let target_total = target
        .total_ink_limit
        .ok_or(CharacterizationTargetError::MissingTotalInkLimit)?;
    if target_total > package.payload.measured_total_ink_limit {
        return Err(CharacterizationTargetError::TotalInkLimitExceedsMeasured {
            target: target_total,
            measured: package.payload.measured_total_ink_limit,
        });
    }

    if require_production_validation
        && package.payload.validation_level != CharacterizationValidationLevel::ProductionValidated
    {
        return Err(CharacterizationTargetError::PackageNotProductionValidated);
    }

    Ok(())
}

fn payload_content_id(payload: &CharacterizationPayload) -> Result<String, String> {
    let bytes = serde_json::to_vec(payload).map_err(|err| err.to_string())?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn validate_payload(payload: &CharacterizationPayload, errors: &mut Vec<String>) {
    if payload.revision.trim().is_empty() {
        errors.push("Characterization revision cannot be empty.".to_owned());
    }
    if !matches!(payload.output_bit_depth, 8 | 16) {
        errors.push(format!(
            "Characterization output bit depth must be 8 or 16, got {}.",
            payload.output_bit_depth
        ));
    }
    if !(4..=12).contains(&payload.channel_names.len()) {
        errors.push(format!(
            "Characterization must define 4..=12 production channels, got {}.",
            payload.channel_names.len()
        ));
    }
    if payload.channel_names.len() != payload.measured_channel_max_coverage.len() {
        errors.push(format!(
            "Characterization defines {} channel names but {} measured channel limits.",
            payload.channel_names.len(),
            payload.measured_channel_max_coverage.len()
        ));
    }

    let mut unique_names = BTreeSet::new();
    for name in &payload.channel_names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            errors.push("Characterization channel names cannot be empty.".to_owned());
            continue;
        }
        if !unique_names.insert(trimmed.to_ascii_lowercase()) {
            errors.push(format!("Duplicate characterization channel '{trimmed}'."));
        }
    }

    for (index, limit) in payload.measured_channel_max_coverage.iter().copied().enumerate() {
        if !limit.is_finite() || !(0.0..=1.0).contains(&limit) || limit == 0.0 {
            errors.push(format!(
                "Measured channel limit {} must be finite and in (0, 1].",
                index + 1
            ));
        }
    }
    if !payload.measured_total_ink_limit.is_finite() || payload.measured_total_ink_limit <= 0.0 {
        errors.push("Measured total-ink limit must be finite and greater than zero.".to_owned());
    } else if payload.measured_total_ink_limit > payload.channel_names.len() as f32 {
        errors.push(format!(
            "Measured total-ink limit {} exceeds the normalized maximum for {} channels.",
            payload.measured_total_ink_limit,
            payload.channel_names.len()
        ));
    }

    validate_required_context(&payload.production_context, errors);
    validate_measurement_metadata(&payload.measurement, errors);
    validate_samples(payload, errors);
}

fn validate_required_context(
    context: &CharacterizationProductionContext,
    errors: &mut Vec<String>,
) {
    for (label, value) in [
        ("machine ID", context.machine_id.as_str()),
        ("RIP name", context.rip_name.as_str()),
        ("RIP version", context.rip_version.as_str()),
        ("linearization/calibration ID", context.linearization_id.as_str()),
        ("substrate", context.substrate.as_str()),
    ] {
        if value.trim().is_empty() {
            errors.push(format!("Characterization {label} cannot be empty."));
        }
    }
}

fn validate_measurement_metadata(
    metadata: &CharacterizationMeasurementMetadata,
    errors: &mut Vec<String>,
) {
    for (label, value) in [
        ("instrument model", metadata.instrument_model.as_str()),
        ("illuminant", metadata.illuminant.as_str()),
        ("observer", metadata.observer.as_str()),
        ("measurement condition", metadata.measurement_condition.as_str()),
    ] {
        if value.trim().is_empty() {
            errors.push(format!("Characterization {label} cannot be empty."));
        }
    }
}

fn validate_samples(payload: &CharacterizationPayload, errors: &mut Vec<String>) {
    // The topology mismatch is reported by `validate_payload`. Do not index a
    // malformed measured-limit vector while collecting additional sample errors.
    if payload.channel_names.len() != payload.measured_channel_max_coverage.len() {
        return;
    }
    if payload.samples.is_empty() {
        errors.push("Characterization must contain measured response samples.".to_owned());
        return;
    }

    let mut coverage_keys = BTreeSet::new();
    let mut has_zero_sample = false;
    let mut channel_exercised = vec![false; payload.channel_names.len()];

    for (sample_index, sample) in payload.samples.iter().enumerate() {
        if sample.coverages.len() != payload.channel_names.len() {
            errors.push(format!(
                "Characterization sample {} has {} coverages; expected {}.",
                sample_index,
                sample.coverages.len(),
                payload.channel_names.len()
            ));
            continue;
        }

        let mut total = 0.0f32;
        let mut key = Vec::with_capacity(sample.coverages.len());
        let mut all_zero = true;
        for (channel_index, coverage) in sample.coverages.iter().copied().enumerate() {
            key.push(coverage.to_bits());
            let measured_limit = payload.measured_channel_max_coverage[channel_index];
            if !coverage.is_finite() || coverage < 0.0 || coverage > measured_limit {
                errors.push(format!(
                    "Characterization sample {} channel '{}' coverage {} is outside measured 0..={}.",
                    sample_index,
                    payload.channel_names[channel_index],
                    coverage,
                    measured_limit
                ));
            }
            if coverage > 0.0 {
                all_zero = false;
                channel_exercised[channel_index] = true;
            }
            total += coverage;
        }
        if total > payload.measured_total_ink_limit {
            errors.push(format!(
                "Characterization sample {} total coverage {} exceeds measured total-ink limit {}.",
                sample_index, total, payload.measured_total_ink_limit
            ));
        }
        if all_zero {
            has_zero_sample = true;
        }
        if !coverage_keys.insert(key) {
            errors.push(format!(
                "Characterization contains duplicate coverage vector at sample {}.",
                sample_index
            ));
        }

        if !sample.lab.l.is_finite()
            || !sample.lab.a.is_finite()
            || !sample.lab.b.is_finite()
            || !(0.0..=100.0).contains(&sample.lab.l)
        {
            errors.push(format!(
                "Characterization sample {} contains invalid CIE Lab values.",
                sample_index
            ));
        }
    }

    if payload.validation_level == CharacterizationValidationLevel::ProductionValidated {
        if !has_zero_sample {
            errors.push(
                "Production-validated characterization must include a zero-ink substrate baseline sample."
                    .to_owned(),
            );
        }
        for (index, exercised) in channel_exercised.into_iter().enumerate() {
            if !exercised {
                errors.push(format!(
                    "Production-validated characterization never exercises channel '{}'.",
                    payload.channel_names[index]
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::TargetChannelDefinition;

    fn payload(level: CharacterizationValidationLevel) -> CharacterizationPayload {
        CharacterizationPayload {
            revision: "line105-2026-08-a".to_owned(),
            validation_level: level,
            output_bit_depth: 16,
            channel_names: ["Blue", "Brown", "Beige", "Black"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            measured_channel_max_coverage: vec![0.8, 0.8, 0.8, 0.7],
            measured_total_ink_limit: 1.8,
            production_context: CharacterizationProductionContext {
                machine_id: "Durst-Line105".to_owned(),
                rip_name: "Production RIP".to_owned(),
                rip_version: "5.4".to_owned(),
                linearization_id: "lin-2026-08-01".to_owned(),
                substrate: "porcelain-body-A".to_owned(),
                glaze: Some("matte-01".to_owned()),
                body: Some("body-A".to_owned()),
                product_family: Some("60x120-matte".to_owned()),
            },
            measurement: CharacterizationMeasurementMetadata {
                instrument_model: "spectrophotometer".to_owned(),
                instrument_serial: Some("fixture-serial".to_owned()),
                illuminant: "D50".to_owned(),
                observer: "2deg".to_owned(),
                measurement_condition: "M1".to_owned(),
                measured_at_unix_ms: Some(1_700_000_000_000),
                operator_or_lab: Some("QA Lab".to_owned()),
            },
            samples: vec![
                CharacterizationSample {
                    coverages: vec![0.0, 0.0, 0.0, 0.0],
                    lab: MeasuredLabColor {
                        l: 94.0,
                        a: 0.2,
                        b: 1.1,
                    },
                },
                CharacterizationSample {
                    coverages: vec![0.4, 0.0, 0.0, 0.0],
                    lab: MeasuredLabColor {
                        l: 70.0,
                        a: -2.0,
                        b: -20.0,
                    },
                },
                CharacterizationSample {
                    coverages: vec![0.0, 0.4, 0.0, 0.0],
                    lab: MeasuredLabColor {
                        l: 68.0,
                        a: 8.0,
                        b: 10.0,
                    },
                },
                CharacterizationSample {
                    coverages: vec![0.0, 0.0, 0.4, 0.0],
                    lab: MeasuredLabColor {
                        l: 80.0,
                        a: 2.0,
                        b: 8.0,
                    },
                },
                CharacterizationSample {
                    coverages: vec![0.0, 0.0, 0.0, 0.4],
                    lab: MeasuredLabColor {
                        l: 50.0,
                        a: 0.5,
                        b: 0.7,
                    },
                },
            ],
        }
    }

    fn target(id: String) -> ConversionTargetDefinition {
        ConversionTargetDefinition {
            name: "Line 105 characterized target".to_owned(),
            channels: ["Blue", "Brown", "Beige", "Black"]
                .into_iter()
                .map(|name| TargetChannelDefinition {
                    name: name.to_owned(),
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: Some(if name == "Black" { 0.65 } else { 0.75 }),
                })
                .collect(),
            bit_depth: 16,
            output_profile_identity: None,
            output_profile_path: None,
            device_link_identity: None,
            device_link_path: None,
            characterization_id: Some(id),
            total_ink_limit: Some(1.6),
        }
    }

    #[test]
    fn malformed_limit_topology_fails_without_indexing_samples() {
        let mut data = payload(CharacterizationValidationLevel::ProductionValidated);
        data.measured_channel_max_coverage.pop();
        let errors = CharacterizationPackage::new(data).unwrap_err().join("\n");
        assert!(errors.contains("measured channel limits"));
    }

    #[test]
    fn package_id_is_content_addressed_and_payload_change_is_detected() {
        let mut package = CharacterizationPackage::new(payload(
            CharacterizationValidationLevel::ProductionValidated,
        ))
        .unwrap();
        assert!(package.id.starts_with("sha256:"));
        assert!(package.validate().is_ok());

        package.payload.production_context.rip_version = "5.5".to_owned();
        let errors = package.validate().unwrap_err().join("\n");
        assert!(errors.contains("content identity mismatch"));
    }

    #[test]
    fn production_package_requires_baseline_and_every_channel_to_be_measured() {
        let mut data = payload(CharacterizationValidationLevel::ProductionValidated);
        data.samples.remove(0);
        data.samples.retain(|sample| sample.coverages[2] == 0.0);
        let errors = CharacterizationPackage::new(data).unwrap_err().join("\n");
        assert!(errors.contains("zero-ink substrate baseline"));
        assert!(errors.contains("never exercises channel 'Beige'"));
    }

    #[test]
    fn target_must_use_exact_content_id_topology_depth_and_bounded_limits() {
        let package = CharacterizationPackage::new(payload(
            CharacterizationValidationLevel::ProductionValidated,
        ))
        .unwrap();
        let matching = target(package.id.clone());
        assert_eq!(
            validate_characterization_for_target(&package, &matching, true),
            Ok(())
        );

        let mut stale = matching.clone();
        stale.characterization_id = Some("sha256:old".to_owned());
        assert!(matches!(
            validate_characterization_for_target(&package, &stale, true),
            Err(CharacterizationTargetError::IdentityMismatch { .. })
        ));

        let mut reordered = matching.clone();
        reordered.target_channels_for_test_swap();
        assert!(matches!(
            validate_characterization_for_target(&package, &reordered, true),
            Err(CharacterizationTargetError::ChannelTopologyMismatch { .. })
        ));

        let mut unsafe_limit = matching;
        unsafe_limit.channels[0].max_coverage = Some(0.9);
        assert!(matches!(
            validate_characterization_for_target(&package, &unsafe_limit, true),
            Err(CharacterizationTargetError::ChannelLimitExceedsMeasured { .. })
        ));
    }

    #[test]
    fn experimental_package_is_rejected_for_production_but_can_be_preflighted() {
        let package = CharacterizationPackage::new(payload(
            CharacterizationValidationLevel::Experimental,
        ))
        .unwrap();
        let target = target(package.id.clone());
        assert_eq!(
            validate_characterization_for_target(&package, &target, true),
            Err(CharacterizationTargetError::PackageNotProductionValidated)
        );
        assert_eq!(
            validate_characterization_for_target(&package, &target, false),
            Ok(())
        );
    }

    #[test]
    fn validated_wrapper_exposes_existing_forward_model_identity_shape() {
        let package = CharacterizationPackage::new(payload(
            CharacterizationValidationLevel::ProductionValidated,
        ))
        .unwrap();
        let expected_id = package.id.clone();
        let validated = package.validated().unwrap();
        assert_eq!(validated.identity().id, expected_id);
        assert_eq!(
            validated.identity().channel_names,
            ["Blue", "Brown", "Beige", "Black"]
        );
    }

    trait TargetTestExt {
        fn target_channels_for_test_swap(&mut self);
    }

    impl TargetTestExt for ConversionTargetDefinition {
        fn target_channels_for_test_swap(&mut self) {
            self.channels.swap(0, 1);
        }
    }
}
