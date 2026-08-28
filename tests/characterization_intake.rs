#![cfg(windows)]

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use windows_shade_editor::characterization_intake::{
    CharacterizationIntakeMetadata, MeasurementCoverageUnit, MeasurementTableDelimiter,
    build_characterization_package_from_table, save_characterization_package,
};
use windows_shade_editor::device_characterization_package::{
    CharacterizationMeasurementMetadata, CharacterizationProductionContext,
    CharacterizationValidationLevel,
};

fn metadata() -> CharacterizationIntakeMetadata {
    CharacterizationIntakeMetadata {
        revision: "line105-intake-contract-v1".to_owned(),
        validation_level: CharacterizationValidationLevel::ProductionValidated,
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

fn table() -> &'static str {
    "Blue,Brown,Beige,Black,L,a,b\n\
     0,0,0,0,94,0.2,1.1\n\
     0.4,0,0,0,70,-2,-20\n\
     0,0.4,0,0,68,8,10\n\
     0,0,0.4,0,80,2,8\n\
     0,0,0,0.4,50,0.5,0.7\n"
}

fn build(metadata: CharacterizationIntakeMetadata) -> windows_shade_editor::characterization_intake::CharacterizationIntakeResult {
    build_characterization_package_from_table(
        table(),
        MeasurementTableDelimiter::Comma,
        MeasurementCoverageUnit::Normalized,
        metadata,
    )
    .unwrap()
}

fn temp_path(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir()
        .join(format!("shade-editor-characterization-intake-contract-{}-{nonce}", std::process::id()))
        .join(name)
}

#[test]
fn content_identity_changes_when_production_context_changes() {
    let first = build(metadata());
    let mut changed = metadata();
    changed.production_context.rip_version = "5.5".to_owned();
    let second = build(changed);

    assert_ne!(first.package.id, second.package.id);
}

#[test]
fn coverage_above_declared_measured_channel_limit_is_rejected() {
    let mut constrained = metadata();
    constrained.measured_channel_max_coverage[0] = 0.3;

    let errors = build_characterization_package_from_table(
        table(),
        MeasurementTableDelimiter::Comma,
        MeasurementCoverageUnit::Normalized,
        constrained,
    )
    .unwrap_err()
    .join("\n");

    assert!(errors.contains("outside measured 0..=0.3"), "{errors}");
}

#[test]
fn non_finite_lab_value_is_rejected_before_package_creation() {
    let invalid = table().replacen("94,0.2,1.1", "NaN,0.2,1.1", 1);
    let errors = build_characterization_package_from_table(
        &invalid,
        MeasurementTableDelimiter::Comma,
        MeasurementCoverageUnit::Normalized,
        metadata(),
    )
    .unwrap_err()
    .join("\n");

    assert!(errors.contains("field 'L*' must be finite"), "{errors}");
}

#[test]
fn invalid_package_is_never_published_by_atomic_save() {
    let mut result = build(metadata());
    result.package.payload.production_context.rip_version = "tampered-after-id".to_owned();
    let path = temp_path("must-not-exist.json");

    let error = save_characterization_package(&path, &result.package).unwrap_err();

    assert!(error.contains("content identity mismatch"), "{error}");
    assert!(!path.exists());
    assert!(!windows_shade_editor::safe_fs::temp_path(&path).exists());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
