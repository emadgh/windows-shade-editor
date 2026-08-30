use std::collections::BTreeSet;

use crate::characterization_intake::{MeasurementCoverageUnit, MeasurementTableDelimiter};

const MAX_CHANNELS: usize = 12;
const COVERAGE_QUANTIZATION: f64 = 1_000_000.0;
const SINGLE_INK_FRACTIONS: [f32; 4] = [0.25, 0.50, 0.75, 1.00];
const PAIRWISE_FRACTIONS: [f32; 2] = [0.50, 1.00];
const BALANCED_TOTAL_FRACTIONS: [f32; 4] = [0.25, 0.50, 0.75, 1.00];

#[derive(Clone, Debug, PartialEq)]
pub struct CharacterizationAcquisitionPlan {
    pub channel_names: Vec<String>,
    /// Normalized direct-coverage vectors in exact authoritative channel order.
    pub coverage_vectors: Vec<Vec<f32>>,
}

impl CharacterizationAcquisitionPlan {
    pub fn patch_count(&self) -> usize {
        self.coverage_vectors.len()
    }
}

/// Build a deterministic bounded screening plan for collecting real measured
/// characterization data. The plan deliberately contains no Lab values and is
/// not a claim of representativeness or production qualification.
///
/// Coverage families are intentionally generic:
/// - one zero-ink substrate baseline;
/// - four single-ink ramp points per channel;
/// - two pairwise mix points for every channel pair;
/// - four balanced total-ink-envelope points across all channels.
///
/// For 12 channels this is bounded to at most 185 unique patches before
/// de-duplication. Every vector is clamped/scaled to the declared per-channel
/// maxima and total-ink limit.
pub fn build_characterization_acquisition_plan(
    channel_names: &[String],
    channel_max_coverage: &[f32],
    total_ink_limit: f32,
) -> Result<CharacterizationAcquisitionPlan, Vec<String>> {
    let errors = validate_acquisition_inputs(channel_names, channel_max_coverage, total_ink_limit);
    if !errors.is_empty() {
        return Err(errors);
    }

    let channel_count = channel_names.len();
    let mut vectors = Vec::new();
    let mut seen = BTreeSet::new();

    push_unique(
        &mut vectors,
        &mut seen,
        vec![0.0; channel_count],
        total_ink_limit,
    );

    for channel_index in 0..channel_count {
        for fraction in SINGLE_INK_FRACTIONS {
            let mut vector = vec![0.0; channel_count];
            vector[channel_index] =
                (channel_max_coverage[channel_index] * fraction).min(total_ink_limit);
            push_unique(&mut vectors, &mut seen, vector, total_ink_limit);
        }
    }

    for left in 0..channel_count {
        for right in (left + 1)..channel_count {
            for fraction in PAIRWISE_FRACTIONS {
                let mut vector = vec![0.0; channel_count];
                vector[left] = channel_max_coverage[left] * fraction;
                vector[right] = channel_max_coverage[right] * fraction;
                push_unique(&mut vectors, &mut seen, vector, total_ink_limit);
            }
        }
    }

    let max_sum = channel_max_coverage.iter().copied().sum::<f32>();
    let effective_total = total_ink_limit.min(max_sum);
    if max_sum > 0.0 && effective_total > 0.0 {
        for fraction in BALANCED_TOTAL_FRACTIONS {
            let target_total = effective_total * fraction;
            let scale = (target_total / max_sum).min(1.0);
            let vector = channel_max_coverage
                .iter()
                .map(|maximum| maximum * scale)
                .collect::<Vec<_>>();
            push_unique(&mut vectors, &mut seen, vector, total_ink_limit);
        }
    }

    Ok(CharacterizationAcquisitionPlan {
        channel_names: channel_names
            .iter()
            .map(|name| name.trim().to_owned())
            .collect(),
        coverage_vectors: vectors,
    })
}

/// Serialize a plan into the exact measurement-table shape consumed by the
/// existing characterization intake parser. Lab fields are intentionally blank
/// because only a real measurement may populate them.
pub fn acquisition_plan_measurement_table(
    plan: &CharacterizationAcquisitionPlan,
    delimiter: MeasurementTableDelimiter,
    coverage_unit: MeasurementCoverageUnit,
) -> String {
    let delimiter_char = match delimiter {
        MeasurementTableDelimiter::Comma => ',',
        MeasurementTableDelimiter::Tab => '\t',
    };
    let delimiter_text = delimiter_char.to_string();

    let mut rows = Vec::with_capacity(plan.coverage_vectors.len() + 1);
    let mut header = plan
        .channel_names
        .iter()
        .map(|name| escape_field(name, delimiter_char))
        .collect::<Vec<_>>();
    header.extend(["L".to_owned(), "a".to_owned(), "b".to_owned()]);
    rows.push(header.join(&delimiter_text));

    for vector in &plan.coverage_vectors {
        let mut row = vector
            .iter()
            .map(|coverage| {
                let value = match coverage_unit {
                    MeasurementCoverageUnit::Normalized => *coverage,
                    MeasurementCoverageUnit::Percent => *coverage * 100.0,
                };
                format_decimal(value)
            })
            .collect::<Vec<_>>();
        row.extend([String::new(), String::new(), String::new()]);
        rows.push(row.join(&delimiter_text));
    }

    let mut table = rows.join("\n");
    table.push('\n');
    table
}

fn validate_acquisition_inputs(
    channel_names: &[String],
    channel_max_coverage: &[f32],
    total_ink_limit: f32,
) -> Vec<String> {
    let mut errors = Vec::new();
    if channel_names.is_empty() || channel_names.len() > MAX_CHANNELS {
        errors.push(format!(
            "Acquisition template requires 1..={MAX_CHANNELS} production channels."
        ));
    }
    if channel_names.len() != channel_max_coverage.len() {
        errors.push(
            "Acquisition template channel names and maximum-coverage vectors must have identical lengths."
                .to_owned(),
        );
    }

    let mut normalized_names = BTreeSet::new();
    for (index, name) in channel_names.iter().enumerate() {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            errors.push(format!(
                "Acquisition template channel {} requires a non-empty authoritative name.",
                index + 1
            ));
            continue;
        }
        let normalized = trimmed.to_ascii_lowercase();
        if !normalized_names.insert(normalized) {
            errors.push(format!(
                "Acquisition template channel name '{trimmed}' is duplicated."
            ));
        }
    }

    for (index, maximum) in channel_max_coverage.iter().enumerate() {
        if !maximum.is_finite() || *maximum <= 0.0 || *maximum > 1.0 {
            errors.push(format!(
                "Acquisition template maximum coverage for channel {} must be finite in (0, 1].",
                index + 1
            ));
        }
    }
    if !total_ink_limit.is_finite()
        || total_ink_limit <= 0.0
        || total_ink_limit > channel_names.len().max(1) as f32
    {
        errors.push(format!(
            "Acquisition template total-ink limit must be finite in (0, {}].",
            channel_names.len().max(1)
        ));
    }
    errors
}

fn scale_to_total_limit(vector: &mut [f32], total_ink_limit: f32) {
    let total = vector.iter().copied().sum::<f32>();
    if total > total_ink_limit && total > 0.0 {
        let scale = total_ink_limit / total;
        for value in vector {
            *value *= scale;
        }
    }
}

fn push_unique(
    vectors: &mut Vec<Vec<f32>>,
    seen: &mut BTreeSet<Vec<u32>>,
    mut vector: Vec<f32>,
    total_ink_limit: f32,
) {
    // Bound first, then quantize downward. This order guarantees the six-decimal
    // serialized representation cannot round a boundary sample back above the
    // declared total-ink limit.
    scale_to_total_limit(&mut vector, total_ink_limit);
    let quantized = vector
        .into_iter()
        .map(quantize_down)
        .collect::<Vec<_>>();
    let key = quantized
        .iter()
        .map(|value| ((*value as f64) * COVERAGE_QUANTIZATION).round() as u32)
        .collect::<Vec<_>>();
    if seen.insert(key) {
        vectors.push(quantized);
    }
}

fn quantize_down(value: f32) -> f32 {
    let value = value.max(0.0) as f64;
    ((value * COVERAGE_QUANTIZATION + 1.0e-9).floor() / COVERAGE_QUANTIZATION) as f32
}

fn escape_field(value: &str, delimiter: char) -> String {
    if value.contains(delimiter) || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn format_decimal(value: f32) -> String {
    let mut text = format!("{value:.6}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    if text.is_empty() || text == "-0" {
        "0".to_owned()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::characterization_intake::parse_measurement_table;

    fn four_channel_input() -> (Vec<String>, Vec<f32>, f32) {
        (
            ["Blue", "Brown", "Beige", "Black"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            vec![0.8, 0.8, 0.8, 0.7],
            1.8,
        )
    }

    fn populate_lab(table: &str, delimiter: char) -> String {
        let suffix = delimiter.to_string().repeat(3);
        let lab = format!("{delimiter}50{delimiter}0{delimiter}0");
        let mut rows = table.lines();
        let mut measured = String::new();
        measured.push_str(rows.next().unwrap());
        measured.push('\n');
        for row in rows {
            let coverage = row.strip_suffix(&suffix).unwrap();
            measured.push_str(coverage);
            measured.push_str(&lab);
            measured.push('\n');
        }
        measured
    }

    #[test]
    fn plan_is_deterministic_bounded_and_starts_with_substrate_baseline() {
        let (names, maxima, total) = four_channel_input();
        let first = build_characterization_acquisition_plan(&names, &maxima, total).unwrap();
        let second = build_characterization_acquisition_plan(&names, &maxima, total).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.coverage_vectors[0], vec![0.0; 4]);
        assert!(first.patch_count() <= 1 + 4 * 4 + 2 * 6 + 4);

        let names = (0..12)
            .map(|index| format!("Ink-{index}"))
            .collect::<Vec<_>>();
        let maxima = vec![0.8; 12];
        let large = build_characterization_acquisition_plan(&names, &maxima, 2.4).unwrap();
        assert!(large.patch_count() <= 185);
    }

    #[test]
    fn every_channel_is_exercised_and_all_vectors_respect_declared_limits() {
        let (names, maxima, total) = four_channel_input();
        let plan = build_characterization_acquisition_plan(&names, &maxima, total).unwrap();

        for channel in 0..names.len() {
            assert!(
                plan.coverage_vectors.iter().any(|vector| {
                    vector[channel] > 0.0
                        && vector
                            .iter()
                            .enumerate()
                            .all(|(index, value)| index == channel || *value == 0.0)
                }),
                "channel {channel} is missing a single-ink acquisition sample"
            );
        }

        for vector in &plan.coverage_vectors {
            assert_eq!(vector.len(), maxima.len());
            for (coverage, maximum) in vector.iter().zip(maxima.iter()) {
                assert!(*coverage >= 0.0);
                assert!(*coverage <= *maximum + 1.0e-6);
            }
            assert!(vector.iter().sum::<f32>() <= total);
        }

        let unique = plan
            .coverage_vectors
            .iter()
            .map(|vector| {
                vector
                    .iter()
                    .map(|value| format!("{value:.6}"))
                    .collect::<Vec<_>>()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(unique.len(), plan.coverage_vectors.len());
    }

    #[test]
    fn exported_template_has_blank_lab_and_round_trips_after_measurement_population() {
        let (names, maxima, total) = four_channel_input();
        let plan = build_characterization_acquisition_plan(&names, &maxima, total).unwrap();
        let table = acquisition_plan_measurement_table(
            &plan,
            MeasurementTableDelimiter::Comma,
            MeasurementCoverageUnit::Normalized,
        );
        assert!(table.starts_with("Blue,Brown,Beige,Black,L,a,b\n"));
        assert!(table.lines().skip(1).all(|line| line.ends_with(",,,")));

        let measured = populate_lab(&table, ',');
        let parsed = parse_measurement_table(
            &measured,
            MeasurementTableDelimiter::Comma,
            MeasurementCoverageUnit::Normalized,
            &names,
        )
        .unwrap();
        assert_eq!(parsed.len(), plan.patch_count());
        assert!(
            parsed
                .iter()
                .all(|sample| sample.coverages.iter().sum::<f32>() <= total)
        );
    }

    #[test]
    fn percent_export_scales_coverages_and_round_trips_inside_limits() {
        let (names, maxima, total) = four_channel_input();
        let plan = build_characterization_acquisition_plan(&names, &maxima, total).unwrap();
        let table = acquisition_plan_measurement_table(
            &plan,
            MeasurementTableDelimiter::Tab,
            MeasurementCoverageUnit::Percent,
        );
        assert!(table.starts_with("Blue\tBrown\tBeige\tBlack\tL\ta\tb\n"));
        assert!(table.lines().skip(1).all(|line| line.ends_with("\t\t\t")));
        assert!(table.contains("80"));

        let measured = populate_lab(&table, '\t');
        let parsed = parse_measurement_table(
            &measured,
            MeasurementTableDelimiter::Tab,
            MeasurementCoverageUnit::Percent,
            &names,
        )
        .unwrap();
        assert_eq!(parsed.len(), plan.patch_count());
        assert!(
            parsed
                .iter()
                .all(|sample| sample.coverages.iter().sum::<f32>() <= total)
        );
    }

    #[test]
    fn invalid_topology_or_limits_fail_closed() {
        let duplicate = vec!["Blue".to_owned(), "blue".to_owned()];
        assert!(build_characterization_acquisition_plan(&duplicate, &[0.8, 0.8], 1.2).is_err());
        assert!(build_characterization_acquisition_plan(&["Blue".to_owned()], &[1.1], 1.0).is_err());
        assert!(build_characterization_acquisition_plan(&["Blue".to_owned()], &[0.8], 0.0).is_err());
    }
}
