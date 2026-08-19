use crate::color_conversion::{ConversionTargetDefinition, SeparationStrategy};
use crate::device_characterization::LabColor;
use crate::gradient_continuity::GradientContinuityReport;

pub const MAX_DIAGNOSTIC_PATH_SAMPLES: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LabPathError {
    InvalidSampleCount {
        actual: usize,
        min: usize,
        max: usize,
    },
    NonFiniteEndpoint,
    LightnessOutOfRange,
}

/// Build a deterministic evenly-spaced straight path in CIE Lab.
///
/// The helper is intentionally simple: the caller owns which neutral/chromatic/
/// saturated endpoints are production-relevant. The sample cap prevents an
/// accidental validation request from becoming an image-sized allocation.
pub fn linear_lab_path(
    start: LabColor,
    end: LabColor,
    sample_count: usize,
) -> Result<Vec<LabColor>, LabPathError> {
    if !(2..=MAX_DIAGNOSTIC_PATH_SAMPLES).contains(&sample_count) {
        return Err(LabPathError::InvalidSampleCount {
            actual: sample_count,
            min: 2,
            max: MAX_DIAGNOSTIC_PATH_SAMPLES,
        });
    }
    if !lab_is_finite(start) || !lab_is_finite(end) {
        return Err(LabPathError::NonFiniteEndpoint);
    }
    if !(0.0..=100.0).contains(&start.l) || !(0.0..=100.0).contains(&end.l) {
        return Err(LabPathError::LightnessOutOfRange);
    }

    let denominator = (sample_count - 1) as f64;
    Ok((0..sample_count)
        .map(|index| {
            let t = index as f64 / denominator;
            LabColor {
                l: lerp(start.l, end.l, t),
                a: lerp(start.a, end.a, t),
                b: lerp(start.b, end.b, t),
            }
        })
        .collect())
}

fn lab_is_finite(value: LabColor) -> bool {
    value.l.is_finite() && value.a.is_finite() && value.b.is_finite()
}

fn lerp(start: f64, end: f64, t: f64) -> f64 {
    start + (end - start) * t
}

#[derive(Clone, Debug, PartialEq)]
pub struct GradientCurvaturePolicy {
    pub max_channel_second_difference: f32,
    pub max_normalized_channel_second_difference: f32,
    pub max_vector_l1_second_difference: f32,
    pub max_vector_l2_second_difference: f32,
    pub max_total_ink_second_difference: f32,
}

impl GradientCurvaturePolicy {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        validate_threshold(
            "max_channel_second_difference",
            self.max_channel_second_difference,
            &mut errors,
        );
        validate_threshold(
            "max_normalized_channel_second_difference",
            self.max_normalized_channel_second_difference,
            &mut errors,
        );
        validate_threshold(
            "max_vector_l1_second_difference",
            self.max_vector_l1_second_difference,
            &mut errors,
        );
        validate_threshold(
            "max_vector_l2_second_difference",
            self.max_vector_l2_second_difference,
            &mut errors,
        );
        validate_threshold(
            "max_total_ink_second_difference",
            self.max_total_ink_second_difference,
            &mut errors,
        );
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

fn validate_threshold(name: &str, value: f32, errors: &mut Vec<String>) {
    if !value.is_finite() || value < 0.0 {
        errors.push(format!("{name} must be finite and >= 0."));
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum GradientCurvatureViolation {
    ChannelSecondDifference {
        channel_index: usize,
        value: f32,
        limit: f32,
    },
    NormalizedChannelSecondDifference {
        channel_index: usize,
        value: f32,
        limit: f32,
    },
    VectorL1SecondDifference {
        value: f32,
        limit: f32,
    },
    VectorL2SecondDifference {
        value: f32,
        limit: f32,
    },
    TotalInkSecondDifference {
        value: f32,
        limit: f32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct GradientCurvatureDiagnostic {
    pub previous_index: usize,
    pub center_index: usize,
    pub next_index: usize,
    pub per_channel_abs_second_difference: Vec<f32>,
    pub per_channel_normalized_second_difference: Vec<f32>,
    pub max_channel_second_difference: f32,
    pub max_normalized_channel_second_difference: f32,
    pub vector_l1_second_difference: f32,
    pub vector_l2_second_difference: f32,
    pub total_ink_second_difference: f32,
    pub violations: Vec<GradientCurvatureViolation>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GradientCurvatureReport {
    pub diagnostics: Vec<GradientCurvatureDiagnostic>,
    pub max_channel_second_difference: f32,
    pub max_normalized_channel_second_difference: f32,
    pub max_vector_l1_second_difference: f32,
    pub max_vector_l2_second_difference: f32,
    pub max_total_ink_second_difference: f32,
    pub violation_count: usize,
}

impl GradientCurvatureReport {
    pub fn passes(&self) -> bool {
        self.violation_count == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GradientCurvatureError {
    InvalidPolicy(Vec<String>),
    SampleTopologyMismatch {
        index: usize,
        expected: usize,
        actual: usize,
    },
}

/// Measure second differences of already-solved separation vectors.
///
/// First-order jumps catch abrupt steps; this second-order pass catches kinks
/// where adjacent steps are individually modest but change slope sharply. It is
/// kept separate from reference solving so quantization/raster effects can be
/// added as later validation layers rather than contaminating the solver metric.
pub fn analyze_gradient_curvature(
    target: &ConversionTargetDefinition,
    strategy: &SeparationStrategy,
    continuity: &GradientContinuityReport,
    policy: &GradientCurvaturePolicy,
) -> Result<GradientCurvatureReport, GradientCurvatureError> {
    policy
        .validate()
        .map_err(GradientCurvatureError::InvalidPolicy)?;

    let channel_count = target.channels.len();
    for sample in &continuity.samples {
        if sample.coverages.len() != channel_count {
            return Err(GradientCurvatureError::SampleTopologyMismatch {
                index: sample.index,
                expected: channel_count,
                actual: sample.coverages.len(),
            });
        }
    }

    let maxima = effective_channel_maxima(target, strategy);
    let mut diagnostics = Vec::with_capacity(continuity.samples.len().saturating_sub(2));
    for triple in continuity.samples.windows(3) {
        diagnostics.push(diagnose_curvature(
            &triple[0], &triple[1], &triple[2], &maxima, policy,
        ));
    }

    let max_channel_second_difference = diagnostics
        .iter()
        .map(|item| item.max_channel_second_difference)
        .max_by(f32::total_cmp)
        .unwrap_or(0.0);
    let max_normalized_channel_second_difference = diagnostics
        .iter()
        .map(|item| item.max_normalized_channel_second_difference)
        .max_by(f32::total_cmp)
        .unwrap_or(0.0);
    let max_vector_l1_second_difference = diagnostics
        .iter()
        .map(|item| item.vector_l1_second_difference)
        .max_by(f32::total_cmp)
        .unwrap_or(0.0);
    let max_vector_l2_second_difference = diagnostics
        .iter()
        .map(|item| item.vector_l2_second_difference)
        .max_by(f32::total_cmp)
        .unwrap_or(0.0);
    let max_total_ink_second_difference = diagnostics
        .iter()
        .map(|item| item.total_ink_second_difference)
        .max_by(f32::total_cmp)
        .unwrap_or(0.0);
    let violation_count = diagnostics.iter().map(|item| item.violations.len()).sum();

    Ok(GradientCurvatureReport {
        diagnostics,
        max_channel_second_difference,
        max_normalized_channel_second_difference,
        max_vector_l1_second_difference,
        max_vector_l2_second_difference,
        max_total_ink_second_difference,
        violation_count,
    })
}

fn diagnose_curvature(
    previous: &crate::gradient_continuity::GradientSampleDiagnostic,
    center: &crate::gradient_continuity::GradientSampleDiagnostic,
    next: &crate::gradient_continuity::GradientSampleDiagnostic,
    maxima: &[f32],
    policy: &GradientCurvaturePolicy,
) -> GradientCurvatureDiagnostic {
    let per_channel_abs_second_difference = previous
        .coverages
        .iter()
        .zip(&center.coverages)
        .zip(&next.coverages)
        .map(|((previous, center), next)| (next - 2.0 * center + previous).abs())
        .collect::<Vec<_>>();
    let per_channel_normalized_second_difference = per_channel_abs_second_difference
        .iter()
        .copied()
        .zip(maxima.iter().copied())
        .map(|(difference, maximum)| {
            if maximum > f32::EPSILON {
                difference / maximum
            } else {
                0.0
            }
        })
        .collect::<Vec<_>>();

    let max_channel_second_difference = per_channel_abs_second_difference
        .iter()
        .copied()
        .max_by(f32::total_cmp)
        .unwrap_or(0.0);
    let max_normalized_channel_second_difference = per_channel_normalized_second_difference
        .iter()
        .copied()
        .max_by(f32::total_cmp)
        .unwrap_or(0.0);
    let vector_l1_second_difference = per_channel_abs_second_difference.iter().sum::<f32>();
    let vector_l2_second_difference = per_channel_abs_second_difference
        .iter()
        .map(|difference| difference * difference)
        .sum::<f32>()
        .sqrt();
    let total_ink_second_difference =
        (next.total_ink - 2.0 * center.total_ink + previous.total_ink).abs();

    let mut violations = Vec::new();
    for (channel_index, value) in per_channel_abs_second_difference
        .iter()
        .copied()
        .enumerate()
    {
        if value > policy.max_channel_second_difference {
            violations.push(GradientCurvatureViolation::ChannelSecondDifference {
                channel_index,
                value,
                limit: policy.max_channel_second_difference,
            });
        }
    }
    for (channel_index, value) in per_channel_normalized_second_difference
        .iter()
        .copied()
        .enumerate()
    {
        if value > policy.max_normalized_channel_second_difference {
            violations.push(
                GradientCurvatureViolation::NormalizedChannelSecondDifference {
                    channel_index,
                    value,
                    limit: policy.max_normalized_channel_second_difference,
                },
            );
        }
    }
    if vector_l1_second_difference > policy.max_vector_l1_second_difference {
        violations.push(GradientCurvatureViolation::VectorL1SecondDifference {
            value: vector_l1_second_difference,
            limit: policy.max_vector_l1_second_difference,
        });
    }
    if vector_l2_second_difference > policy.max_vector_l2_second_difference {
        violations.push(GradientCurvatureViolation::VectorL2SecondDifference {
            value: vector_l2_second_difference,
            limit: policy.max_vector_l2_second_difference,
        });
    }
    if total_ink_second_difference > policy.max_total_ink_second_difference {
        violations.push(GradientCurvatureViolation::TotalInkSecondDifference {
            value: total_ink_second_difference,
            limit: policy.max_total_ink_second_difference,
        });
    }

    GradientCurvatureDiagnostic {
        previous_index: previous.index,
        center_index: center.index,
        next_index: next.index,
        per_channel_abs_second_difference,
        per_channel_normalized_second_difference,
        max_channel_second_difference,
        max_normalized_channel_second_difference,
        vector_l1_second_difference,
        vector_l2_second_difference,
        total_ink_second_difference,
        violations,
    }
}

fn effective_channel_maxima(
    target: &ConversionTargetDefinition,
    strategy: &SeparationStrategy,
) -> Vec<f32> {
    target
        .channels
        .iter()
        .map(|channel| {
            let mut maximum = channel.max_coverage.unwrap_or(1.0).clamp(0.0, 1.0);
            if strategy.black_channel.as_deref() == Some(channel.name.as_str()) {
                maximum = maximum.min(strategy.black_max.clamp(0.0, 1.0));
            }
            maximum
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::TargetChannelDefinition;
    use crate::gradient_continuity::{GradientContinuityReport, GradientSampleDiagnostic};
    use crate::inverse_separation_solver::InverseSolverStats;

    fn target() -> ConversionTargetDefinition {
        ConversionTargetDefinition {
            name: "Curvature fixture".to_owned(),
            channels: vec![TargetChannelDefinition {
                name: "Ink".to_owned(),
                display_rgb: None,
                solidity: 1.0,
                max_coverage: Some(1.0),
            }],
            bit_depth: 16,
            output_profile_identity: None,
            output_profile_path: None,
            device_link_identity: None,
            device_link_path: None,
            characterization_id: Some("curvature-fixture".to_owned()),
            total_ink_limit: None,
        }
    }

    fn policy(limit: f32) -> GradientCurvaturePolicy {
        GradientCurvaturePolicy {
            max_channel_second_difference: limit,
            max_normalized_channel_second_difference: limit,
            max_vector_l1_second_difference: limit,
            max_vector_l2_second_difference: limit,
            max_total_ink_second_difference: limit,
        }
    }

    fn sample(index: usize, coverage: f32) -> GradientSampleDiagnostic {
        GradientSampleDiagnostic {
            index,
            target_lab: LabColor {
                l: 50.0,
                a: 0.0,
                b: 0.0,
            },
            coverages: vec![coverage],
            delta_e00: 0.0,
            total_ink: coverage,
            solver_stats: InverseSolverStats::default(),
        }
    }

    fn continuity(samples: Vec<GradientSampleDiagnostic>) -> GradientContinuityReport {
        GradientContinuityReport {
            samples,
            transitions: Vec::new(),
            max_channel_jump: 0.0,
            max_normalized_channel_jump: 0.0,
            max_vector_l1_jump: 0.0,
            max_vector_l2_jump: 0.0,
            max_total_ink_jump: 0.0,
            dominant_channel_switches: 0,
            violation_count: 0,
        }
    }

    #[test]
    fn linear_lab_path_is_bounded_and_deterministic() {
        let start = LabColor {
            l: 100.0,
            a: 0.0,
            b: 0.0,
        };
        let end = LabColor {
            l: 0.0,
            a: 50.0,
            b: -50.0,
        };
        let path = linear_lab_path(start, end, 3).expect("valid path");
        assert_eq!(path[0], start);
        assert_eq!(
            path[1],
            LabColor {
                l: 50.0,
                a: 25.0,
                b: -25.0
            }
        );
        assert_eq!(path[2], end);
        assert_eq!(linear_lab_path(start, end, 3).unwrap(), path);

        assert!(matches!(
            linear_lab_path(start, end, 1),
            Err(LabPathError::InvalidSampleCount { .. })
        ));
        assert!(matches!(
            linear_lab_path(start, end, MAX_DIAGNOSTIC_PATH_SAMPLES + 1),
            Err(LabPathError::InvalidSampleCount { .. })
        ));
        assert_eq!(
            linear_lab_path(
                LabColor {
                    l: f64::NAN,
                    ..start
                },
                end,
                3
            ),
            Err(LabPathError::NonFiniteEndpoint)
        );
        assert_eq!(
            linear_lab_path(LabColor { l: 101.0, ..start }, end, 3),
            Err(LabPathError::LightnessOutOfRange)
        );
    }

    #[test]
    fn linear_separation_has_zero_second_difference() {
        let report = continuity(vec![sample(0, 0.1), sample(1, 0.2), sample(2, 0.3)]);
        let result = analyze_gradient_curvature(
            &target(),
            &SeparationStrategy::default(),
            &report,
            &policy(1e-5),
        )
        .expect("curvature report");
        assert!(result.passes());
        assert!(result.max_channel_second_difference < 1e-5);
    }

    #[test]
    fn modest_adjacent_steps_with_a_kink_are_detected() {
        let report = continuity(vec![sample(0, 0.0), sample(1, 0.2), sample(2, 0.6)]);
        let result = analyze_gradient_curvature(
            &target(),
            &SeparationStrategy::default(),
            &report,
            &policy(0.1),
        )
        .expect("curvature report");

        assert!(!result.passes());
        assert!((result.max_channel_second_difference - 0.2).abs() < 1e-6);
        assert!((result.max_total_ink_second_difference - 0.2).abs() < 1e-6);
        assert!(result.violation_count >= 1);
    }
}
