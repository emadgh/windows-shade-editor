use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::inverse_lut_artifact::{
    InverseLutPublishOutcome, load_inverse_lut_artifact, publish_inverse_lut_artifact_if_absent,
    write_inverse_lut_artifact,
};
use crate::inverse_lut_identity::{
    INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION, INVERSE_LUT_IDENTITY_SCHEMA_VERSION,
    InverseLutBuildPolicy, InverseLutContinuityFieldMethod, InverseLutForwardModelIdentity,
    InverseLutForwardModelMethod, InverseLutIdentityRecord, InverseLutInterpolationMethod,
    InverseLutLocalForwardModelConfigIdentity, InverseLutNumericalPrecision,
    InverseLutOutputQuantization, InverseLutValidityEncoding, LabGridSpec,
};

fn identity(bit_depth: u8) -> InverseLutIdentityRecord {
    InverseLutIdentityRecord {
        schema_version: INVERSE_LUT_IDENTITY_SCHEMA_VERSION,
        characterization_id: format!("sha256:{}", "a".repeat(64)),
        forward_model: InverseLutForwardModelIdentity {
            method: InverseLutForwardModelMethod::LocalInverseDistanceWeightedV1,
            config: InverseLutLocalForwardModelConfigIdentity {
                neighbor_count: 8,
                distance_power: 2.0,
                max_support_distance: 0.5,
            },
        },
        recipe_sha256: "b".repeat(64),
        channel_names: ["Blue", "Brown", "Beige", "Black"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        target_bit_depth: bit_depth,
        build_policy: InverseLutBuildPolicy {
            schema_version: INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION,
            grid: LabGridSpec {
                l_min: 0.0,
                l_max: 100.0,
                l_samples: 2,
                a_min: -128.0,
                a_max: 127.0,
                a_samples: 2,
                b_min: -128.0,
                b_max: 127.0,
                b_samples: 2,
            },
            interpolation: InverseLutInterpolationMethod::TrilinearV1,
            validity_encoding: InverseLutValidityEncoding::ExplicitNodeValidityMaskV1,
            numerical_precision: InverseLutNumericalPrecision::NormalizedF32V1,
            output_quantization: InverseLutOutputQuantization::ClampScaleRoundV1,
            continuity_field: InverseLutContinuityFieldMethod::IndependentNodeSolvesV1,
        },
    }
}

fn payload() -> (Vec<bool>, Vec<f32>) {
    let validity = vec![true, true, false, true, true, false, true, true];
    let mut coverages = Vec::new();
    for node in 0..8 {
        for channel in 0..4 {
            coverages.push(((node * 4 + channel) as f32 / 31.0).clamp(0.0, 1.0));
        }
    }
    (validity, coverages)
}

fn temp_folder(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "shade-inverse-lut-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn artifact_round_trip_is_byte_deterministic_for_8_and_16_bit_identity() {
    for bit_depth in [8u8, 16] {
        let folder = temp_folder(&format!("roundtrip-{bit_depth}"));
        fs::create_dir_all(&folder).unwrap();
        let first = folder.join("first.lut");
        let second = folder.join("second.lut");
        let identity = identity(bit_depth);
        let (validity, coverages) = payload();

        write_inverse_lut_artifact(&first, &identity, &validity, &coverages).unwrap();
        write_inverse_lut_artifact(&second, &identity, &validity, &coverages).unwrap();
        assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());

        let loaded = load_inverse_lut_artifact(&first).unwrap();
        assert_eq!(loaded.identity, identity);
        assert_eq!(loaded.identity_content_id, identity.content_id().unwrap());
        assert_eq!(loaded.validity, validity);
        assert_eq!(loaded.coverages, coverages);
        let _ = fs::remove_dir_all(folder);
    }
}

#[test]
fn truncation_trailing_data_and_payload_corruption_fail_closed() {
    let folder = temp_folder("corruption");
    fs::create_dir_all(&folder).unwrap();
    let clean = folder.join("clean.lut");
    let identity = identity(16);
    let (validity, coverages) = payload();
    write_inverse_lut_artifact(&clean, &identity, &validity, &coverages).unwrap();
    let bytes = fs::read(&clean).unwrap();

    let truncated = folder.join("truncated.lut");
    fs::write(&truncated, &bytes[..bytes.len() - 1]).unwrap();
    assert!(load_inverse_lut_artifact(&truncated).is_err());

    let trailing = folder.join("trailing.lut");
    fs::write(&trailing, &bytes).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&trailing)
        .unwrap()
        .write_all(&[0x55])
        .unwrap();
    assert!(load_inverse_lut_artifact(&trailing).is_err());

    let corrupted = folder.join("corrupted.lut");
    let mut damaged = bytes.clone();
    let last = damaged.len() - 1;
    damaged[last] ^= 0x01;
    fs::write(&corrupted, damaged).unwrap();
    assert!(load_inverse_lut_artifact(&corrupted).is_err());
    let _ = fs::remove_dir_all(folder);
}

#[test]
fn header_identity_content_id_tampering_fails_closed() {
    let folder = temp_folder("identity-tamper");
    fs::create_dir_all(&folder).unwrap();
    let path = folder.join("identity.lut");
    let identity = identity(16);
    let (validity, coverages) = payload();
    write_inverse_lut_artifact(&path, &identity, &validity, &coverages).unwrap();

    let mut bytes = fs::read(&path).unwrap();
    let needle = b"\"identity_content_id\":\"sha256:";
    let start = bytes
        .windows(needle.len())
        .position(|window| window == needle)
        .unwrap();
    let hex = start + needle.len();
    bytes[hex] = if bytes[hex] == b'0' { b'1' } else { b'0' };
    fs::write(&path, bytes).unwrap();
    assert!(load_inverse_lut_artifact(&path).is_err());
    let _ = fs::remove_dir_all(folder);
}

#[test]
fn publication_reuses_only_exact_existing_object() {
    let folder = temp_folder("publish");
    fs::create_dir_all(&folder).unwrap();
    let destination = folder.join("object.lut");
    let identity = identity(16);
    let (validity, coverages) = payload();

    assert_eq!(
        publish_inverse_lut_artifact_if_absent(&destination, &identity, &validity, &coverages)
            .unwrap(),
        InverseLutPublishOutcome::Published
    );
    assert_eq!(
        publish_inverse_lut_artifact_if_absent(&destination, &identity, &validity, &coverages)
            .unwrap(),
        InverseLutPublishOutcome::ReusedExisting
    );

    let mut changed = coverages.clone();
    changed[0] = 0.75;
    assert!(
        publish_inverse_lut_artifact_if_absent(&destination, &identity, &validity, &changed)
            .is_err()
    );
    let loaded = load_inverse_lut_artifact(&destination).unwrap();
    assert_eq!(loaded.coverages, coverages);
    let _ = fs::remove_dir_all(folder);
}

#[test]
fn lengths_non_finite_values_and_negative_zero_are_canonicalized_or_rejected() {
    let folder = temp_folder("input-validation");
    fs::create_dir_all(&folder).unwrap();
    let identity = identity(16);
    let (validity, mut coverages) = payload();
    let path = folder.join("canonical.lut");

    assert!(write_inverse_lut_artifact(&path, &identity, &validity[..7], &coverages).is_err());
    coverages[0] = f32::NAN;
    assert!(write_inverse_lut_artifact(&path, &identity, &validity, &coverages).is_err());

    let (_, mut positive_zero) = payload();
    let mut negative_zero = positive_zero.clone();
    positive_zero[0] = 0.0;
    negative_zero[0] = -0.0;
    let first = folder.join("positive-zero.lut");
    let second = folder.join("negative-zero.lut");
    write_inverse_lut_artifact(&first, &identity, &validity, &positive_zero).unwrap();
    write_inverse_lut_artifact(&second, &identity, &validity, &negative_zero).unwrap();
    assert_eq!(fs::read(first).unwrap(), fs::read(second).unwrap());
    let _ = fs::remove_dir_all(folder);
}
