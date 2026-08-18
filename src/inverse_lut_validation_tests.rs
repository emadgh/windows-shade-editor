use crate::inverse_lut_validation::{
    INVERSE_LUT_VALIDATION_REPORT_SCHEMA_VERSION, InverseLutValidationPolicy,
    InverseLutValidationSample, summarize_validation_samples,
};

fn id(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn bare(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn supported(delta: f64, ink_l1: f64) -> InverseLutValidationSample {
    InverseLutValidationSample {
        supported: true,
        lut_delta_e00: Some(delta),
        reference_delta_e00: Some(delta * 0.8),
        ink_l1: Some(ink_l1),
        ink_l2: Some(ink_l1 * 0.6),
        max_channel_deviation: Some(ink_l1 * 0.4),
        u8_quantization_l1: Some(0.01),
        u16_quantization_l1: Some(0.0001),
        constraints_preserved: true,
    }
}

fn unsupported() -> InverseLutValidationSample {
    InverseLutValidationSample {
        supported: false,
        lut_delta_e00: None,
        reference_delta_e00: None,
        ink_l1: None,
        ink_l2: None,
        max_channel_deviation: None,
        u8_quantization_l1: None,
        u16_quantization_l1: None,
        constraints_preserved: true,
    }
}

fn report_with_inputs(
    lut_identity_content_id: String,
    lut_payload_sha256: String,
    recipe_sha256: String,
    characterization_id: String,
    policy: InverseLutValidationPolicy,
    samples: &[InverseLutValidationSample],
) -> crate::inverse_lut_validation::InverseLutValidationReport {
    summarize_validation_samples(
        lut_identity_content_id,
        lut_payload_sha256,
        recipe_sha256,
        characterization_id,
        policy,
        samples,
    )
    .unwrap()
}

fn report(
    policy: InverseLutValidationPolicy,
    samples: &[InverseLutValidationSample],
) -> crate::inverse_lut_validation::InverseLutValidationReport {
    report_with_inputs(id('a'), bare('b'), bare('c'), id('d'), policy, samples)
}

#[test]
fn deterministic_summary_uses_nearest_rank_p95() {
    let policy = InverseLutValidationPolicy {
        max_mean_delta_e00: 100.0,
        max_p95_delta_e00: 100.0,
        max_delta_e00: 100.0,
        max_mean_ink_l1: 100.0,
        max_p95_ink_l1: 100.0,
        max_ink_l1: 100.0,
        max_ink_l2: 100.0,
        max_channel_deviation: 100.0,
        max_unsupported_fraction: 1.0,
        max_u8_quantization_l1: 100.0,
        max_u16_quantization_l1: 100.0,
        ..InverseLutValidationPolicy::default()
    };
    let samples = (1..=20)
        .map(|value| supported(value as f64, value as f64 / 100.0))
        .collect::<Vec<_>>();
    let first = report(policy, &samples);
    let second = report(policy, &samples);
    assert_eq!(first, second);
    assert_eq!(first.summary.lut_delta_e00.p95, 19.0);
    assert_eq!(first.summary.lut_delta_e00.max, 20.0);
    assert_eq!(first.content_id().unwrap(), second.content_id().unwrap());
}

#[test]
fn each_bound_input_changes_report_content_identity() {
    let policy = InverseLutValidationPolicy::default();
    let samples = [supported(0.4, 0.05)];
    let base = report(policy, &samples);
    let base_id = base.content_id().unwrap();

    let changed = report_with_inputs(
        id('e'),
        bare('b'),
        bare('c'),
        id('d'),
        policy,
        &samples,
    );
    assert_ne!(changed.content_id().unwrap(), base_id);

    let changed = report_with_inputs(
        id('a'),
        bare('e'),
        bare('c'),
        id('d'),
        policy,
        &samples,
    );
    assert_ne!(changed.content_id().unwrap(), base_id);

    let changed = report_with_inputs(
        id('a'),
        bare('b'),
        bare('e'),
        id('d'),
        policy,
        &samples,
    );
    assert_ne!(changed.content_id().unwrap(), base_id);

    let changed = report_with_inputs(
        id('a'),
        bare('b'),
        bare('c'),
        id('e'),
        policy,
        &samples,
    );
    assert_ne!(changed.content_id().unwrap(), base_id);

    let mut changed_policy = policy;
    changed_policy.max_delta_e00 += 0.5;
    let changed = report(changed_policy, &samples);
    assert_ne!(changed.content_id().unwrap(), base_id);
}

#[test]
fn unsupported_fraction_and_constraint_failures_are_explicit() {
    let mut policy = InverseLutValidationPolicy::default();
    policy.max_unsupported_fraction = 0.20;
    let passing = report(policy, &[supported(0.5, 0.05), supported(0.6, 0.06)]);
    assert!(passing.passed);

    let unsupported_failure = report(
        policy,
        &[supported(0.5, 0.05), unsupported(), unsupported()],
    );
    assert!(!unsupported_failure.passed);
    assert_eq!(unsupported_failure.summary.unsupported_samples, 2);

    let mut bad_constraint = supported(0.5, 0.05);
    bad_constraint.constraints_preserved = false;
    let constraint_failure = report(policy, &[bad_constraint]);
    assert!(!constraint_failure.passed);
    assert_eq!(constraint_failure.summary.constraint_violation_count, 1);
}

#[test]
fn unsupported_samples_cannot_smuggle_numeric_metrics() {
    let mut sample = unsupported();
    sample.ink_l1 = Some(0.0);
    let error = summarize_validation_samples(
        id('a'),
        bare('b'),
        bare('c'),
        id('d'),
        InverseLutValidationPolicy::default(),
        &[sample],
    )
    .unwrap_err();
    assert!(error.contains("must not carry numeric metrics"));
}

#[test]
fn report_rejects_tampered_pass_flag_and_schema() {
    let mut tampered_pass = report(
        InverseLutValidationPolicy::default(),
        &[supported(0.5, 0.05)],
    );
    tampered_pass.passed = !tampered_pass.passed;
    assert!(tampered_pass.validate().is_err());

    let mut tampered_schema = report(
        InverseLutValidationPolicy::default(),
        &[supported(0.5, 0.05)],
    );
    tampered_schema.schema_version = INVERSE_LUT_VALIDATION_REPORT_SCHEMA_VERSION + 1;
    assert!(tampered_schema.validate().is_err());
}

#[test]
fn policy_rejects_nonfinite_negative_and_nonmonotonic_thresholds() {
    let mut policy = InverseLutValidationPolicy::default();
    policy.max_mean_delta_e00 = f64::NAN;
    policy.max_unsupported_fraction = 1.1;
    policy.max_mean_ink_l1 = -1.0;
    assert!(policy.validate().is_err());

    let mut policy = InverseLutValidationPolicy::default();
    policy.max_mean_delta_e00 = 2.0;
    policy.max_p95_delta_e00 = 1.0;
    assert!(policy.validate().is_err());
}
