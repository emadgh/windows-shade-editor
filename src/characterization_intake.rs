use std::path::Path;

use crate::device_characterization_package::{
    CharacterizationMeasurementMetadata, CharacterizationPackage, CharacterizationPayload,
    CharacterizationProductionContext, CharacterizationSample, CharacterizationValidationLevel,
    MeasuredLabColor,
};
use crate::production_colorimetry::{
    ProductionPcsCompatibilityError, validate_characterization_for_icc_pcs_lab,
};
use crate::safe_fs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasurementTableDelimiter {
    Comma,
    Tab,
}

impl MeasurementTableDelimiter {
    fn character(self) -> char {
        match self {
            Self::Comma => ',',
            Self::Tab => '\t',
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasurementCoverageUnit {
    Normalized,
    Percent,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CharacterizationIntakeMetadata {
    pub revision: String,
    pub validation_level: CharacterizationValidationLevel,
    pub output_bit_depth: u8,
    pub channel_names: Vec<String>,
    /// Normalized production limits in exact `channel_names` order.
    pub measured_channel_max_coverage: Vec<f32>,
    /// Normalized sum of direct-coverage channels.
    pub measured_total_ink_limit: f32,
    pub production_context: CharacterizationProductionContext,
    pub measurement: CharacterizationMeasurementMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CharacterizationQualificationWarning {
    NotProductionValidated,
    UnsupportedIlluminant {
        declared: String,
        required: &'static str,
    },
    UnsupportedObserver {
        declared: String,
        required: &'static str,
    },
}

impl std::fmt::Display for CharacterizationQualificationWarning {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotProductionValidated => formatter.write_str(
                "Package is structurally valid but declares experimental validation; it cannot enter #205 production qualification as approved evidence.",
            ),
            Self::UnsupportedIlluminant { declared, required } => write!(
                formatter,
                "Measured illuminant '{declared}' is not compatible with the current Custom Optimizer production PCS; #205 requires {required}."
            ),
            Self::UnsupportedObserver { declared, required } => write!(
                formatter,
                "Measured observer '{declared}' is not compatible with the current Custom Optimizer production PCS; #205 requires {required}."
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CharacterizationIntakeResult {
    pub package: CharacterizationPackage,
    pub qualification_warnings: Vec<CharacterizationQualificationWarning>,
}

/// Parse an explicit measurement table and build the same content-addressed
/// package consumed by the production characterization loader.
///
/// The first row must be exactly the authoritative channel order followed by
/// `L,a,b`. Coverage units are never guessed: callers choose normalized `0..1`
/// or percent `0..100` explicitly.
pub fn build_characterization_package_from_table(
    table: &str,
    delimiter: MeasurementTableDelimiter,
    coverage_unit: MeasurementCoverageUnit,
    metadata: CharacterizationIntakeMetadata,
) -> Result<CharacterizationIntakeResult, Vec<String>> {
    let samples = parse_measurement_table(
        table,
        delimiter,
        coverage_unit,
        &metadata.channel_names,
    )?;

    let payload = CharacterizationPayload {
        revision: metadata.revision,
        validation_level: metadata.validation_level,
        output_bit_depth: metadata.output_bit_depth,
        channel_names: metadata.channel_names,
        measured_channel_max_coverage: metadata.measured_channel_max_coverage,
        measured_total_ink_limit: metadata.measured_total_ink_limit,
        production_context: metadata.production_context,
        measurement: metadata.measurement,
        samples,
    };
    let package = CharacterizationPackage::new(payload)?;
    let qualification_warnings = qualification_warnings(&package);
    Ok(CharacterizationIntakeResult {
        package,
        qualification_warnings,
    })
}

pub fn parse_measurement_table(
    table: &str,
    delimiter: MeasurementTableDelimiter,
    coverage_unit: MeasurementCoverageUnit,
    channel_names: &[String],
) -> Result<Vec<CharacterizationSample>, Vec<String>> {
    let mut rows = table
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty());
    let Some((header_index, raw_header)) = rows.next() else {
        return Err(vec!["Measurement table is empty.".to_owned()]);
    };
    let raw_header = raw_header.trim_start_matches('\u{feff}');
    let header = parse_delimited_row(raw_header, delimiter.character(), header_index + 1)
        .map_err(|error| vec![error])?;

    let expected_columns = channel_names.len() + 3;
    if header.len() != expected_columns {
        return Err(vec![format!(
            "Measurement header has {} columns; expected {} production channels plus L, a, b ({} columns total).",
            header.len(),
            channel_names.len(),
            expected_columns
        )]);
    }

    let mut errors = Vec::new();
    for (index, channel_name) in channel_names.iter().enumerate() {
        if header[index].trim() != channel_name.trim() {
            errors.push(format!(
                "Measurement header column {} is {:?}; expected production channel {:?}. Channel order is authoritative and is never reordered implicitly.",
                index + 1,
                header[index],
                channel_name
            ));
        }
    }
    for (offset, expected) in ["L", "a", "b"].into_iter().enumerate() {
        let index = channel_names.len() + offset;
        if !header[index].trim().eq_ignore_ascii_case(expected) {
            errors.push(format!(
                "Measurement header column {} is {:?}; expected {expected}.",
                index + 1,
                header[index]
            ));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut samples = Vec::new();
    for (line_index, raw_line) in rows {
        let row_number = line_index + 1;
        let cells = match parse_delimited_row(raw_line, delimiter.character(), row_number) {
            Ok(cells) => cells,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        if cells.len() != expected_columns {
            errors.push(format!(
                "Measurement row {row_number} has {} columns; expected {expected_columns}.",
                cells.len()
            ));
            continue;
        }

        let mut coverages = Vec::with_capacity(channel_names.len());
        let mut row_failed = false;
        for (channel_index, cell) in cells.iter().take(channel_names.len()).enumerate() {
            match parse_coverage(cell, coverage_unit, row_number, &channel_names[channel_index]) {
                Ok(value) => coverages.push(value),
                Err(error) => {
                    errors.push(error);
                    row_failed = true;
                }
            }
        }

        let lab_start = channel_names.len();
        let l = parse_number(&cells[lab_start], row_number, "L*");
        let a = parse_number(&cells[lab_start + 1], row_number, "a*");
        let b = parse_number(&cells[lab_start + 2], row_number, "b*");
        for result in [&l, &a, &b] {
            if let Err(error) = result {
                errors.push(error.clone());
                row_failed = true;
            }
        }

        if !row_failed {
            samples.push(CharacterizationSample {
                coverages,
                lab: MeasuredLabColor {
                    l: l.unwrap(),
                    a: a.unwrap(),
                    b: b.unwrap(),
                },
            });
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(samples)
}

/// Persist only a package that still validates under the production domain
/// authority. The atomic writer prevents a failed save from publishing a partial
/// measured-evidence file.
pub fn save_characterization_package(
    path: &Path,
    package: &CharacterizationPackage,
) -> Result<(), String> {
    package.validate().map_err(|errors| errors.join("\n"))?;
    let json = serde_json::to_vec_pretty(package).map_err(|error| error.to_string())?;
    safe_fs::atomic_write(path, &json, None).map_err(|error| {
        format!(
            "Cannot persist measured characterization package {}: {error}",
            path.display()
        )
    })
}

fn qualification_warnings(
    package: &CharacterizationPackage,
) -> Vec<CharacterizationQualificationWarning> {
    let mut warnings = Vec::new();
    if package.payload.validation_level != CharacterizationValidationLevel::ProductionValidated {
        warnings.push(CharacterizationQualificationWarning::NotProductionValidated);
    }

    // `CharacterizationPackage::new` already validated the package. Rebuilding
    // the validated wrapper here lets intake use the exact production PCS gate
    // rather than duplicating illuminant/observer aliases.
    if let Ok(validated) = package.clone().validated() {
        match validate_characterization_for_icc_pcs_lab(&validated) {
            Ok(_) => {}
            Err(ProductionPcsCompatibilityError::UnsupportedIlluminant {
                declared,
                required,
            }) => warnings.push(CharacterizationQualificationWarning::UnsupportedIlluminant {
                declared,
                required,
            }),
            Err(ProductionPcsCompatibilityError::UnsupportedObserver {
                declared,
                required,
            }) => warnings.push(CharacterizationQualificationWarning::UnsupportedObserver {
                declared,
                required,
            }),
        }
    }
    warnings
}

fn parse_coverage(
    cell: &str,
    coverage_unit: MeasurementCoverageUnit,
    row_number: usize,
    channel_name: &str,
) -> Result<f32, String> {
    let value = parse_number(cell, row_number, channel_name)?;
    let (min, max, normalized) = match coverage_unit {
        MeasurementCoverageUnit::Normalized => (0.0, 1.0, value),
        MeasurementCoverageUnit::Percent => (0.0, 100.0, value / 100.0),
    };
    if !(min..=max).contains(&value) {
        return Err(format!(
            "Measurement row {row_number} channel '{channel_name}' coverage {value} is outside the selected {:?} range {min}..={max}.",
            coverage_unit
        ));
    }
    Ok(normalized as f32)
}

fn parse_number(cell: &str, row_number: usize, label: &str) -> Result<f64, String> {
    let value = cell.trim().parse::<f64>().map_err(|_| {
        format!(
            "Measurement row {row_number} field '{label}' is not a valid number: {:?}.",
            cell.trim()
        )
    })?;
    if !value.is_finite() {
        return Err(format!(
            "Measurement row {row_number} field '{label}' must be finite."
        ));
    }
    Ok(value)
}

/// Minimal RFC-4180-style row parser sufficient for numeric measurement tables:
/// delimiters inside quoted fields and escaped `""` quotes are supported.
fn parse_delimited_row(line: &str, delimiter: char, row_number: usize) -> Result<Vec<String>, String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut chars = line.chars().peekable();
    let mut in_quotes = false;
    let mut after_quote = false;

    while let Some(character) = chars.next() {
        if in_quotes {
            if character == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                    after_quote = true;
                }
            } else {
                field.push(character);
            }
            continue;
        }

        if after_quote {
            if character == delimiter {
                fields.push(field.trim().to_owned());
                field.clear();
                after_quote = false;
            } else if !character.is_whitespace() {
                return Err(format!(
                    "Measurement row {row_number} has unexpected character {character:?} after a closing quote."
                ));
            }
            continue;
        }

        if character == delimiter {
            fields.push(field.trim().to_owned());
            field.clear();
        } else if character == '"' {
            if field.trim().is_empty() {
                field.clear();
                in_quotes = true;
            } else {
                return Err(format!(
                    "Measurement row {row_number} contains a quote inside an unquoted field."
                ));
            }
        } else {
            field.push(character);
        }
    }

    if in_quotes {
        return Err(format!(
            "Measurement row {row_number} has an unterminated quoted field."
        ));
    }
    fields.push(field.trim().to_owned());
    Ok(fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_characterization_package::{
        CharacterizationValidationLevel, load_characterization_package,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn metadata(level: CharacterizationValidationLevel) -> CharacterizationIntakeMetadata {
        CharacterizationIntakeMetadata {
            revision: "line105-intake-v1".to_owned(),
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
        }
    }

    fn normalized_csv() -> &'static str {
        "Blue,Brown,Beige,Black,L,a,b\n\
         0,0,0,0,94,0.2,1.1\n\
         0.4,0,0,0,70,-2,-20\n\
         0,0.4,0,0,68,8,10\n\
         0,0,0.4,0,80,2,8\n\
         0,0,0,0.4,50,0.5,0.7\n"
    }

    fn temp_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("shade-editor-characterization-intake-{}-{nonce}", std::process::id()))
            .join(name)
    }

    #[test]
    fn normalized_csv_builds_stable_content_addressed_package() {
        let first = build_characterization_package_from_table(
            normalized_csv(),
            MeasurementTableDelimiter::Comma,
            MeasurementCoverageUnit::Normalized,
            metadata(CharacterizationValidationLevel::ProductionValidated),
        )
        .unwrap();
        let second = build_characterization_package_from_table(
            normalized_csv(),
            MeasurementTableDelimiter::Comma,
            MeasurementCoverageUnit::Normalized,
            metadata(CharacterizationValidationLevel::ProductionValidated),
        )
        .unwrap();
        assert!(first.package.id.starts_with("sha256:"));
        assert_eq!(first.package.id, second.package.id);
        assert_eq!(first.package.payload.samples.len(), 5);
        assert!(first.qualification_warnings.is_empty());
    }

    #[test]
    fn percent_tsv_is_explicitly_normalized() {
        let table = "Blue\tBrown\tBeige\tBlack\tL\ta\tb\n\
                     0\t0\t0\t0\t94\t0.2\t1.1\n\
                     40\t0\t0\t0\t70\t-2\t-20\n\
                     0\t40\t0\t0\t68\t8\t10\n\
                     0\t0\t40\t0\t80\t2\t8\n\
                     0\t0\t0\t40\t50\t0.5\t0.7\n";
        let result = build_characterization_package_from_table(
            table,
            MeasurementTableDelimiter::Tab,
            MeasurementCoverageUnit::Percent,
            metadata(CharacterizationValidationLevel::ProductionValidated),
        )
        .unwrap();
        assert!((result.package.payload.samples[1].coverages[0] - 0.4).abs() < 1e-6);
    }

    #[test]
    fn authoritative_channel_order_is_not_reordered_implicitly() {
        let table = normalized_csv().replacen(
            "Blue,Brown,Beige,Black",
            "Brown,Blue,Beige,Black",
            1,
        );
        let errors = build_characterization_package_from_table(
            &table,
            MeasurementTableDelimiter::Comma,
            MeasurementCoverageUnit::Normalized,
            metadata(CharacterizationValidationLevel::ProductionValidated),
        )
        .unwrap_err()
        .join("\n");
        assert!(errors.contains("Channel order is authoritative"));
    }

    #[test]
    fn quoted_channel_header_can_contain_delimiter_without_implicit_renaming() {
        let mut metadata = metadata(CharacterizationValidationLevel::ProductionValidated);
        metadata.channel_names[0] = "Light,Blue".to_owned();
        let table = normalized_csv().replacen("Blue,", "\"Light,Blue\",", 1);
        let result = build_characterization_package_from_table(
            &table,
            MeasurementTableDelimiter::Comma,
            MeasurementCoverageUnit::Normalized,
            metadata,
        )
        .unwrap();
        assert_eq!(result.package.payload.channel_names[0], "Light,Blue");
    }

    #[test]
    fn production_validated_import_must_retain_zero_ink_baseline() {
        let table = normalized_csv().replacen("0,0,0,0,94,0.2,1.1\n", "", 1);
        let errors = build_characterization_package_from_table(
            &table,
            MeasurementTableDelimiter::Comma,
            MeasurementCoverageUnit::Normalized,
            metadata(CharacterizationValidationLevel::ProductionValidated),
        )
        .unwrap_err()
        .join("\n");
        assert!(errors.contains("zero-ink substrate baseline"));
    }

    #[test]
    fn duplicate_coverage_vectors_are_rejected_by_domain_validation() {
        let table = format!("{}0,0,0,0,90,0,0\n", normalized_csv());
        let errors = build_characterization_package_from_table(
            &table,
            MeasurementTableDelimiter::Comma,
            MeasurementCoverageUnit::Normalized,
            metadata(CharacterizationValidationLevel::ProductionValidated),
        )
        .unwrap_err()
        .join("\n");
        assert!(errors.contains("duplicate coverage vector"));
    }

    #[test]
    fn d65_is_structurally_valid_but_explicitly_not_production_pcs_ready() {
        let mut metadata = metadata(CharacterizationValidationLevel::ProductionValidated);
        metadata.measurement.illuminant = "D65".to_owned();
        let result = build_characterization_package_from_table(
            normalized_csv(),
            MeasurementTableDelimiter::Comma,
            MeasurementCoverageUnit::Normalized,
            metadata,
        )
        .unwrap();
        assert!(matches!(
            result.qualification_warnings.as_slice(),
            [CharacterizationQualificationWarning::UnsupportedIlluminant { declared, required }]
                if declared == "D65" && *required == "D50"
        ));
    }

    #[test]
    fn experimental_package_is_valid_but_not_claimed_as_production_qualified() {
        let result = build_characterization_package_from_table(
            normalized_csv(),
            MeasurementTableDelimiter::Comma,
            MeasurementCoverageUnit::Normalized,
            metadata(CharacterizationValidationLevel::Experimental),
        )
        .unwrap();
        assert!(result
            .qualification_warnings
            .contains(&CharacterizationQualificationWarning::NotProductionValidated));
    }

    #[test]
    fn invalid_selected_coverage_unit_fails_before_domain_package_creation() {
        let table = normalized_csv().replacen("0.4,0,0,0", "40,0,0,0", 1);
        let errors = build_characterization_package_from_table(
            &table,
            MeasurementTableDelimiter::Comma,
            MeasurementCoverageUnit::Normalized,
            metadata(CharacterizationValidationLevel::ProductionValidated),
        )
        .unwrap_err()
        .join("\n");
        assert!(errors.contains("Normalized range"));
    }

    #[test]
    fn atomic_save_round_trips_through_production_loader() {
        let result = build_characterization_package_from_table(
            normalized_csv(),
            MeasurementTableDelimiter::Comma,
            MeasurementCoverageUnit::Normalized,
            metadata(CharacterizationValidationLevel::ProductionValidated),
        )
        .unwrap();
        let path = temp_path("measured-characterization.json");
        save_characterization_package(&path, &result.package).unwrap();
        let restored = load_characterization_package(&path).unwrap();
        assert_eq!(restored.package(), &result.package);
        assert!(!safe_fs::temp_path(&path).exists());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
