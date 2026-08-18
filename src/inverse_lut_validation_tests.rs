use crate::inverse_lut_path_validation::{
    InverseLutPathDiagnostic, InverseLutValidationPathKind,
};
use crate::inverse_lut_validation::{
    INVERSE_LUT_VALIDATION_REPORT_SCHEMA_VERSION, InverseLutValidationPolicy,
    InverseLutValidationSample, summarize_validation_samples,
};
use crate::inverse_lut_validation_reference::InverseLutValidationReferenceMethod;

fn id(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn bare(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn independent_reference_method() -> InverseLutValidationReferenceMethod {
    InverseLutValidationReferenceMethod::IndependentPointSolveV1
}

fn supported(delta: f64, ink_l1: f64) -> InverseLutValidationSample {
    InverseLutValidationSample {
        supported: true,
        lut_delta_e00: Some(delta),
        reference_delta_e00: Some(delta * 0.8),
        lut_vs_reference_delta_e00: Some(delta * 0.2),
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
        lut_vs_reference_delta_e00: None,
        ink_l1: None,
        ink_l2: None,
        max_channel_deviation: None,
        u8_quantization_l1: None,
        u16_quantization_l1: None,
        constraints_preserved: true,
    }
}

fn path(kind: InverseLutValidationPathKind) -> InverseLutPathDiagnostic {
    InverseLutPathDiagnostic {
        kind,
        sample_count: 5,
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

fn passing_paths() -> Vec<InverseLutPathDiagnostic> {
    [
        InverseLutValidationPathKind::NeutralAxis,
        InverseLutValidationPathKind::NearNeutralWarm,
        InverseLutValidationPathKind::NearNeutralCool,
        InverseLutValidationPathKind::AAxis,
        InverseLutValidationPathKind::BAxis,
        InverseLutValidationPathKind::AbDiagonal,
        InverseLutValidationPathKind::AbOpposedDiagonal,
    ]
    .into_iter()
    .map(path)
    .collect()
}

fn report_with_inputs_and_paths(
    lut_identity_content_id: String,
    lut_payload_sha256: String,
    recipe_sha256: String,
    characterization_id: String,
    policy: InverseLutValidationPolicy,
    paths: Vec<InverseLutPathDiagnostic>,
    samples: &[InverseLutValidationSample],
) -> crate::inverse_lut_validation::InverseLutValidationReport {
    summarize_validation_samples(
        lut_identity_content_id,
        lut_payload_sha256,
        recipe_sha256,
        characterization_id,
        policy,
        independent_reference_method(),
        paths,
        samples,
    )
    .unwrap()
}

fn report_with_inputs(
    lut_identity_content_id: String,
    lut_payload_sha256: String,
    recipe_sha256: String,
    characterization_id: String,
    policy: InverseLutValidationPolicy,
    samples: &[InverseLutValidationSample],
) -> crate::inverse_lut_validation::InverseLutValidationReport {
    report_with_inputs_and_paths(
        lut_identity_content_id,
        lut_payload_sha256,
        recipe_sha256,
        characterization_id,
        policy,
        passing_paths(),
        samples,
    )
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
        max_mean_lut_vs_reference_delta_e00: 100.0,
        max_p95_lut_vs_reference_delta_e00: 100.0,
        max_lut_vs_reference_delta_e00: 100.0,
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
    assert!((first.summary.lut_vs_reference_delta_e00.p95 - 3.8).abs() < 1.0e-12);
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

    let mut changed_paths = passing_paths();
    changed_paths[0].max_channel_jump = Some(0.01);
    let changed = report_with_inputs_and_paths(
        id('a'),
        bare('b'),
        bare('c'),
        id('d'),
        policy,
        changed_paths,
        &samples,
    );
    assert_ne!(changed.content_id().unwrap(), base_id);

    let mut changed_reference_method = base.clone();
    changed_reference_method.reference_method =
        InverseLutValidationReferenceMethod::FrozenJacobiTrilinearThenV2SolveV1;
    assert_ne!(changed_reference_method.content_id().unwrap(), base_id);
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
fn ordered_path_gate_fails_closed_and_cannot_be_omitted() {
    let policy = InverseLutValidationPolicy::default();
    let samples = [supported(0.4, 0.05)];

    let mut unsupported_path = passing_paths();
    unsupported_path[0] = InverseLutPathDiagnostic {
        kind: InverseLutValidationPathKind::NeutralAxis,
        sample_count: 5,
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
    let failed = report_with_inputs_and_paths(
        id('a'),
        bare('b'),
        bare('c'),
        id('d'),
        policy,
        unsupported_path,
        &samples,
    );
    assert!(!failed.passed);

    let missing = summarize_validation_samples(
        id('a'),
        bare('b'),
        bare('c'),
        id('d'),
        policy,
        independent_reference_method(),
        passing_paths()[..6].to_vec(),
        &samples,
    );
    assert!(missing.is_err());

    let mut reordered = passing_paths();
    reordered.swap(0, 1);
    let reordered = summarize_validation_samples(
        id('a'),
        bare('b'),
        bare('c'),
        id('d'),
        policy,
        independent_reference_method(),
        reordered,
        &samples,
    );
    assert!(reordered.is_err());
}

#[test]
fn dominant_channel_switches_are_a_separate_path_gate() {
    let mut policy = InverseLutValidationPolicy::default();
    policy.path_policy.max_dominant_channel_switches_per_path = 1;
    let mut paths = passing_paths();
    paths[3].dominant_channel_switches = Some(2);
    let failed = report_with_inputs_and_paths(
        id('a'),
        bare('b'),
        bare('c'),
        id('d'),
        policy,
        paths,
        &[supported(0.4, 0.05)],
    );
    assert!(!failed.passed);
}

#[test]
fn direct_lut_reference_delta_has_an_independent_gate() {
    let mut policy = InverseLutValidationPolicy::default();
    policy.max_mean_lut_vs_reference_delta_e00 = 0.05;
    policy.max_p95_lut_vs_reference_delta_e00 = 0.05;
    policy.max_lut_vs_reference_delta_e00 = 0.05;
    let result = report(policy, &[supported(0.5, 0.05)]);
    assert!(!result.passed);
    assert!((result.summary.lut_vs_reference_delta_e00.max - 0.1).abs() < 1.0e-12);
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
        independent_reference_method(),
        passing_paths(),
        &[sample],
    )
    .unwrap_err();
    assert!(error.contains("must not carry numeric metrics"));
}

#[test]
fn report_rejects_tampered_pass_flag_schema_and_path_order() {
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

    let mut tampered_path_order = report(
        InverseLutValidationPolicy::default(),
        &[supported(0.5, 0.05)],
    );
    tampered_path_order.path_diagnostics.swap(0, 1);
    assert!(tampered_path_order.validate().is_err());
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

    let mut policy = InverseLutValidationPolicy::default();
    policy.max_mean_lut_vs_reference_delta_e00 = 1.0;
    policy.max_p95_lut_vs_reference_delta_e00 = 0.5;
    assert!(policy.validate().is_err());

    let mut policy = InverseLutValidationPolicy::default();
    policy.path_policy.max_vector_l1_second_difference = -0.1;
    assert!(policy.validate().is_err());
}
