use serde::{Deserialize, Serialize};

use crate::device_characterization::{
    delta_e_2000, CharacterizationIdentity, DeviceForwardModel, LabColor,
};
use crate::device_characterization_package::{
    CharacterizationValidationLevel, MeasuredLabColor, ValidatedCharacterizationPackage,
};

const EXACT_SAMPLE_EPSILON: f64 = 1.0e-10;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct LocalForwardModelConfig {
    /// Number of nearest measured samples blended for each prediction.
    pub neighbor_count: usize,
    /// Inverse-distance exponent. Higher values favor the closest samples.
    pub distance_power: f64,
    /// Maximum normalized RMS device-space distance allowed for the farthest
    /// selected neighbor. Queries without this amount of local measured support
    /// are rejected instead of silently extrapolated.
    pub max_support_distance: f64,
}

impl Default for LocalForwardModelConfig {
    fn default() -> Self {
        Self {
            neighbor_count: 8,
            distance_power: 2.0,
            max_support_distance: 0.45,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct ForwardModelValidationPolicy {
    pub max_mean_delta_e00: f64,
    pub max_p95_delta_e00: f64,
    pub max_delta_e00: f64,
    /// Maximum fraction of leave-one-out samples that may be unsupported by the
    /// configured local neighborhood. Production defaults to zero.
    pub max_unsupported_fraction: f64,
}

impl Default for ForwardModelValidationPolicy {
    fn default() -> Self {
        Self {
            max_mean_delta_e00: 2.0,
            max_p95_delta_e00: 4.0,
            max_delta_e00: 8.0,
            max_unsupported_fraction: 0.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ForwardModelValidationReport {
    pub method: String,
    pub sample_count: usize,
    pub evaluated_count: usize,
    pub unsupported_count: usize,
    pub unsupported_fraction: f64,
    pub mean_delta_e00: f64,
    pub p95_delta_e00: f64,
    pub max_delta_e00: f64,
}

#[derive(Clone, Debug)]
struct ModelSample {
    normalized_coverages: Vec<f64>,
    lab: LabColor,
}

/// Local measured forward model for the Custom Optimizer path.
///
/// The model blends nearby measured samples in normalized device space using
/// inverse-distance weighting. It is only constructed from a production-
/// validated, content-addressed characterization package and must pass a
/// leave-one-out color-error policy before it can implement `DeviceForwardModel`.
///
/// This is intentionally conservative: it rejects coverage vectors outside the
/// package's measured per-channel/total-ink domain or without enough nearby
/// measured support. It does not claim that a sparse sample cloud covers every
/// point inside its rectangular bounds.
#[derive(Clone, Debug)]
pub struct ValidatedLocalForwardModel {
    identity: CharacterizationIdentity,
    measured_channel_max_coverage: Vec<f32>,
    measured_total_ink_limit: f32,
    samples: Vec<ModelSample>,
    config: LocalForwardModelConfig,
    validation: ForwardModelValidationReport,
}

impl ValidatedLocalForwardModel {
    pub fn build(
        package: &ValidatedCharacterizationPackage,
        config: LocalForwardModelConfig,
        policy: ForwardModelValidationPolicy,
    ) -> Result<Self, Vec<String>> {
        let payload = &package.package().payload;
        let mut errors = validate_model_settings(config, policy);
        if payload.validation_level != CharacterizationValidationLevel::ProductionValidated {
            errors.push(
                "A production forward model requires a ProductionValidated characterization package."
                    .to_owned(),
            );
        }
        if payload.samples.len() <= config.neighbor_count {
            errors.push(format!(
                "Characterization has {} samples but local validation with {} neighbors requires at least {} samples.",
                payload.samples.len(),
                config.neighbor_count,
                config.neighbor_count + 1
            ));
        }
        if !errors.is_empty() {
            return Err(errors);
        }

        let scales = payload
            .measured_channel_max_coverage
            .iter()
            .map(|value| f64::from(*value))
            .collect::<Vec<_>>();
        let samples = payload
            .samples
            .iter()
            .map(|sample| ModelSample {
                normalized_coverages: normalize_coverages(&sample.coverages, &scales),
                lab: measured_to_lab(sample.lab),
            })
            .collect::<Vec<_>>();

        let validation = leave_one_out_validation(&samples, config);
        let unsupported_fraction = validation.unsupported_fraction;
        if unsupported_fraction > policy.max_unsupported_fraction
            || validation.mean_delta_e00 > policy.max_mean_delta_e00
            || validation.p95_delta_e00 > policy.max_p95_delta_e00
            || validation.max_delta_e00 > policy.max_delta_e00
        {
            return Err(vec![format!(
                "Characterization forward-model validation failed: mean ΔE00 {:.3} (limit {:.3}), p95 {:.3} (limit {:.3}), max {:.3} (limit {:.3}), unsupported {:.2}% (limit {:.2}%).",
                validation.mean_delta_e00,
                policy.max_mean_delta_e00,
                validation.p95_delta_e00,
                policy.max_p95_delta_e00,
                validation.max_delta_e00,
                policy.max_delta_e00,
                unsupported_fraction * 100.0,
                policy.max_unsupported_fraction * 100.0,
            )]);
        }

        Ok(Self {
            identity: package.identity().clone(),
            measured_channel_max_coverage: payload.measured_channel_max_coverage.clone(),
            measured_total_ink_limit: payload.measured_total_ink_limit,
            samples,
            config,
            validation,
        })
    }

    pub fn config(&self) -> LocalForwardModelConfig {
        self.config
    }

    pub fn validation_report(&self) -> &ForwardModelValidationReport {
        &self.validation
    }

    fn validate_query(&self, coverages: &[f32]) -> Result<Vec<f64>, String> {
        if coverages.len() != self.identity.channel_names.len() {
            return Err(format!(
                "Forward-model topology mismatch: expected {} coverages, got {}.",
                self.identity.channel_names.len(),
                coverages.len()
            ));
        }

        let mut total = 0.0f32;
        for (index, coverage) in coverages.iter().copied().enumerate() {
            let max = self.measured_channel_max_coverage[index];
            if !coverage.is_finite() || coverage < 0.0 || coverage > max {
                return Err(format!(
                    "Coverage for channel '{}' is {} but the measured model domain is 0..={}.",
                    self.identity.channel_names[index], coverage, max
                ));
            }
            total += coverage;
        }
        if total > self.measured_total_ink_limit {
            return Err(format!(
                "Coverage vector total {} exceeds measured model total-ink domain {}.",
                total, self.measured_total_ink_limit
            ));
        }

        let scales = self
            .measured_channel_max_coverage
            .iter()
            .map(|value| f64::from(*value))
            .collect::<Vec<_>>();
        Ok(normalize_coverages(coverages, &scales))
    }
}

impl DeviceForwardModel for ValidatedLocalForwardModel {
    fn identity(&self) -> &CharacterizationIdentity {
        &self.identity
    }

    fn predict_lab(&self, coverages: &[f32]) -> Result<LabColor, String> {
        let normalized = self.validate_query(coverages)?;
        predict_from_samples(&self.samples, &normalized, self.config, None)
    }
}

fn validate_model_settings(
    config: LocalForwardModelConfig,
    policy: ForwardModelValidationPolicy,
) -> Vec<String> {
    let mut errors = Vec::new();
    if config.neighbor_count < 2 {
        errors.push("Local forward model requires at least two neighbors.".to_owned());
    }
    if !config.distance_power.is_finite() || config.distance_power <= 0.0 {
        errors.push("Local forward-model distance power must be finite and > 0.".to_owned());
    }
    if !config.max_support_distance.is_finite()
        || config.max_support_distance <= 0.0
        || config.max_support_distance > 1.0
    {
        errors.push(
            "Local forward-model support distance must be finite and in (0, 1].".to_owned(),
        );
    }
    for (name, value) in [
        ("mean ΔE00", policy.max_mean_delta_e00),
        ("p95 ΔE00", policy.max_p95_delta_e00),
        ("max ΔE00", policy.max_delta_e00),
    ] {
        if !value.is_finite() || value < 0.0 {
            errors.push(format!("Forward-model {name} limit must be finite and non-negative."));
        }
    }
    if !policy.max_unsupported_fraction.is_finite()
        || !(0.0..=1.0).contains(&policy.max_unsupported_fraction)
    {
        errors.push(
            "Forward-model unsupported fraction limit must be finite and in 0..=1.".to_owned(),
        );
    }
    errors
}

fn normalize_coverages(coverages: &[f32], scales: &[f64]) -> Vec<f64> {
    coverages
        .iter()
        .zip(scales)
        .map(|(coverage, scale)| f64::from(*coverage) / *scale)
        .collect()
}

fn measured_to_lab(value: MeasuredLabColor) -> LabColor {
    LabColor {
        l: value.l,
        a: value.a,
        b: value.b,
    }
}

fn normalized_rms_distance(first: &[f64], second: &[f64]) -> f64 {
    let sum = first
        .iter()
        .zip(second)
        .map(|(a, b)| {
            let delta = a - b;
            delta * delta
        })
        .sum::<f64>();
    (sum / first.len() as f64).sqrt()
}

fn predict_from_samples(
    samples: &[ModelSample],
    query: &[f64],
    config: LocalForwardModelConfig,
    excluded_index: Option<usize>,
) -> Result<LabColor, String> {
    let mut distances = samples
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != excluded_index)
        .map(|(index, sample)| {
            (
                normalized_rms_distance(&sample.normalized_coverages, query),
                index,
            )
        })
        .collect::<Vec<_>>();
    distances.sort_by(|left, right| left.0.total_cmp(&right.0));

    if let Some((distance, index)) = distances.first().copied() {
        if distance <= EXACT_SAMPLE_EPSILON {
            return Ok(samples[index].lab);
        }
    } else {
        return Err("Forward model has no usable measured samples.".to_owned());
    }

    if distances.len() < config.neighbor_count {
        return Err(format!(
            "Forward model has only {} usable samples; {} neighbors are required.",
            distances.len(), config.neighbor_count
        ));
    }

    let selected = &distances[..config.neighbor_count];
    let farthest = selected.last().expect("neighbor count is validated").0;
    if farthest > config.max_support_distance {
        return Err(format!(
            "Coverage vector is outside validated local measured support: farthest required neighbor distance {:.4} exceeds {:.4}.",
            farthest, config.max_support_distance
        ));
    }

    let mut weight_sum = 0.0f64;
    let mut l = 0.0f64;
    let mut a = 0.0f64;
    let mut b = 0.0f64;
    for (distance, index) in selected.iter().copied() {
        let weight = 1.0 / distance.powf(config.distance_power);
        if !weight.is_finite() || weight <= 0.0 {
            return Err("Forward-model interpolation produced an invalid sample weight.".to_owned());
        }
        let lab = samples[index].lab;
        weight_sum += weight;
        l += weight * lab.l;
        a += weight * lab.a;
        b += weight * lab.b;
    }
    if !weight_sum.is_finite() || weight_sum <= 0.0 {
        return Err("Forward-model interpolation produced an invalid weight sum.".to_owned());
    }

    let predicted = LabColor {
        l: l / weight_sum,
        a: a / weight_sum,
        b: b / weight_sum,
    };
    if !predicted.l.is_finite() || !predicted.a.is_finite() || !predicted.b.is_finite() {
        return Err("Forward-model interpolation produced non-finite Lab values.".to_owned());
    }
    Ok(predicted)
}

fn leave_one_out_validation(
    samples: &[ModelSample],
    config: LocalForwardModelConfig,
) -> ForwardModelValidationReport {
    let mut errors = Vec::with_capacity(samples.len());
    let mut unsupported_count = 0usize;

    for (index, sample) in samples.iter().enumerate() {
        match predict_from_samples(
            samples,
            &sample.normalized_coverages,
            config,
            Some(index),
        ) {
            Ok(predicted) => errors.push(delta_e_2000(sample.lab, predicted)),
            Err(_) => unsupported_count += 1,
        }
    }

    errors.sort_by(f64::total_cmp);
    let evaluated_count = errors.len();
    let mean = if errors.is_empty() {
        f64::INFINITY
    } else {
        errors.iter().sum::<f64>() / errors.len() as f64
    };
    let p95 = percentile_nearest_rank(&errors, 0.95).unwrap_or(f64::INFINITY);
    let max = errors.last().copied().unwrap_or(f64::INFINITY);
    let unsupported_fraction = unsupported_count as f64 / samples.len() as f64;

    ForwardModelValidationReport {
        method: "leave_one_out_local_idw_v1".to_owned(),
        sample_count: samples.len(),
        evaluated_count,
        unsupported_count,
        unsupported_fraction,
        mean_delta_e00: mean,
        p95_delta_e00: p95,
        max_delta_e00: max,
    }
}

fn percentile_nearest_rank(sorted: &[f64], quantile: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (quantile * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.saturating_sub(1).min(sorted.len() - 1)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_characterization_package::{
        CharacterizationMeasurementMetadata, CharacterizationPackage,
        CharacterizationPayload, CharacterizationProductionContext, CharacterizationSample,
    };

    fn synthetic_lab(coverages: &[f32]) -> MeasuredLabColor {
        let blue = f64::from(coverages[0]);
        let brown = f64::from(coverages[1]);
        let beige = f64::from(coverages[2]);
        let black = f64::from(coverages[3]);
        MeasuredLabColor {
            l: 95.0 - 18.0 * blue - 14.0 * brown - 8.0 * beige - 45.0 * black,
            a: -2.0 * blue + 6.0 * brown + 2.0 * beige,
            b: -14.0 * blue + 8.0 * brown + 5.0 * beige,
        }
    }

    fn package_with_grid() -> ValidatedCharacterizationPackage {
        let mut samples = Vec::new();
        for blue in [0.0f32, 0.4, 0.8] {
            for brown in [0.0f32, 0.4, 0.8] {
                for beige in [0.0f32, 0.4, 0.8] {
                    for black in [0.0f32, 0.35, 0.7] {
                        let coverages = vec![blue, brown, beige, black];
                        if coverages.iter().sum::<f32>() <= 2.0 {
                            samples.push(CharacterizationSample {
                                lab: synthetic_lab(&coverages),
                                coverages,
                            });
                        }
                    }
                }
            }
        }

        CharacterizationPackage::new(CharacterizationPayload {
            revision: "synthetic-grid-v1".to_owned(),
            validation_level: CharacterizationValidationLevel::ProductionValidated,
            output_bit_depth: 16,
            channel_names: ["Blue", "Brown", "Beige", "Black"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            measured_channel_max_coverage: vec![0.8, 0.8, 0.8, 0.7],
            measured_total_ink_limit: 2.0,
            production_context: CharacterizationProductionContext {
                machine_id: "synthetic-machine".to_owned(),
                rip_name: "synthetic-rip".to_owned(),
                rip_version: "1".to_owned(),
                linearization_id: "synthetic-linearization".to_owned(),
                substrate: "synthetic-substrate".to_owned(),
                glaze: None,
                body: None,
                product_family: None,
            },
            measurement: CharacterizationMeasurementMetadata {
                instrument_model: "synthetic-instrument".to_owned(),
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

    fn permissive_policy() -> ForwardModelValidationPolicy {
        ForwardModelValidationPolicy {
            max_mean_delta_e00: 10.0,
            max_p95_delta_e00: 15.0,
            max_delta_e00: 25.0,
            max_unsupported_fraction: 0.0,
        }
    }

    #[test]
    fn production_model_builds_only_after_leave_one_out_validation() {
        let package = package_with_grid();
        let model = ValidatedLocalForwardModel::build(
            &package,
            LocalForwardModelConfig {
                neighbor_count: 6,
                distance_power: 2.0,
                max_support_distance: 0.6,
            },
            permissive_policy(),
        )
        .unwrap();
        let report = model.validation_report();
        assert_eq!(report.sample_count, package.package().payload.samples.len());
        assert_eq!(report.unsupported_count, 0);
        assert!(report.mean_delta_e00.is_finite());
    }

    #[test]
    fn exact_measured_sample_round_trips_exact_lab() {
        let package = package_with_grid();
        let model = ValidatedLocalForwardModel::build(
            &package,
            LocalForwardModelConfig {
                neighbor_count: 6,
                distance_power: 2.0,
                max_support_distance: 0.6,
            },
            permissive_policy(),
        )
        .unwrap();
        let sample = &package.package().payload.samples[5];
        let predicted = model.predict_lab(&sample.coverages).unwrap();
        let expected = measured_to_lab(sample.lab);
        assert_eq!(predicted, expected);
    }

    #[test]
    fn query_outside_measured_channel_or_total_ink_domain_is_rejected() {
        let package = package_with_grid();
        let model = ValidatedLocalForwardModel::build(
            &package,
            LocalForwardModelConfig {
                neighbor_count: 6,
                distance_power: 2.0,
                max_support_distance: 0.6,
            },
            permissive_policy(),
        )
        .unwrap();
        assert!(model.predict_lab(&[0.81, 0.0, 0.0, 0.0]).is_err());
        assert!(model.predict_lab(&[0.6, 0.6, 0.6, 0.6]).is_err());
    }

    #[test]
    fn insufficient_local_support_is_rejected_instead_of_extrapolated() {
        let package = package_with_grid();
        let model = ValidatedLocalForwardModel::build(
            &package,
            LocalForwardModelConfig {
                neighbor_count: 6,
                distance_power: 2.0,
                max_support_distance: 0.6,
            },
            permissive_policy(),
        )
        .unwrap();
        let mut far = model.clone();
        far.config.max_support_distance = 0.01;
        assert!(far.predict_lab(&[0.2, 0.2, 0.2, 0.2]).is_err());
    }

    #[test]
    fn strict_color_error_policy_rejects_corrupted_measurement_cloud() {
        let mut package = package_with_grid().into_package();
        package.payload.samples[10].lab = MeasuredLabColor {
            l: 5.0,
            a: 90.0,
            b: -90.0,
        };
        // Re-content-address the intentionally corrupted fixture so this test is
        // about model validation rather than package digest validation.
        package = CharacterizationPackage::new(package.payload).unwrap();
        let validated = package.validated().unwrap();
        let error = ValidatedLocalForwardModel::build(
            &validated,
            LocalForwardModelConfig {
                neighbor_count: 6,
                distance_power: 2.0,
                max_support_distance: 0.6,
            },
            ForwardModelValidationPolicy {
                max_mean_delta_e00: 0.5,
                max_p95_delta_e00: 1.0,
                max_delta_e00: 2.0,
                max_unsupported_fraction: 0.0,
            },
        )
        .unwrap_err()
        .join("\n");
        assert!(error.contains("validation failed"));
    }

    #[test]
    fn experimental_characterization_cannot_construct_production_model() {
        let mut package = package_with_grid().into_package();
        package.payload.validation_level = CharacterizationValidationLevel::Experimental;
        let package = CharacterizationPackage::new(package.payload)
            .unwrap()
            .validated()
            .unwrap();
        let errors = ValidatedLocalForwardModel::build(
            &package,
            LocalForwardModelConfig::default(),
            permissive_policy(),
        )
        .unwrap_err()
        .join("\n");
        assert!(errors.contains("ProductionValidated"));
    }
}
