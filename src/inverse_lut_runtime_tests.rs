use sha2::{Digest, Sha256};

use crate::device_characterization::LabColor;
use crate::inverse_lut_artifact::VerifiedInverseLutArtifact;
use crate::inverse_lut_identity::{
    INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION, INVERSE_LUT_IDENTITY_SCHEMA_VERSION,
    InverseLutBuildPolicy, InverseLutContinuityFieldMethod, InverseLutForwardModelIdentity,
    InverseLutForwardModelMethod, InverseLutInterpolationMethod,
    InverseLutLocalForwardModelConfigIdentity, InverseLutNumericalPrecision,
    InverseLutOutputQuantization, InverseLutValidityEncoding, LabGridSpec,
};
use crate::inverse_lut_runtime::{InverseLutLookupError, InverseLutRuntime};

const CHANNEL_COUNT: usize = 4;
const NODE_COUNT: usize = 8;

fn identity() -> crate::inverse_lut_identity::InverseLutIdentityRecord {
    crate::inverse_lut_identity::InverseLutIdentityRecord {
        schema_version: INVERSE_LUT_IDENTITY_SCHEMA_VERSION,
        characterization_id: format!("sha256:{}", "1".repeat(64)),
        forward_model: InverseLutForwardModelIdentity {
            method: InverseLutForwardModelMethod::LocalInverseDistanceWeightedV1,
            config: InverseLutLocalForwardModelConfigIdentity {
                neighbor_count: 2,
                distance_power: 2.0,
                max_support_distance: 0.5,
            },
        },
        recipe_sha256: "2".repeat(64),
        channel_names: ["A", "B", "C", "D"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        target_bit_depth: 16,
        build_policy: InverseLutBuildPolicy {
            schema_version: INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION,
            grid: LabGridSpec {
                l_min: 0.0,
                l_max: 100.0,
                l_samples: 2,
                a_min: -10.0,
                a_max: 10.0,
                a_samples: 2,
                b_min: -20.0,
                b_max: 20.0,
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

fn payload_sha256(validity: &[bool], coverages: &[f32]) -> String {
    let mut hasher = Sha256::new();
    for valid in validity {
        hasher.update([u8::from(*valid)]);
    }
    for value in coverages.iter().copied() {
        let canonical = if value == 0.0 { 0.0 } else { value };
        hasher.update(canonical.to_bits().to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn artifact(validity: Vec<bool>, coverages: Vec<f32>) -> VerifiedInverseLutArtifact {
    let identity = identity();
    let payload_sha256 = payload_sha256(&validity, &coverages);
    VerifiedInverseLutArtifact {
        identity_content_id: identity.content_id().unwrap(),
        identity,
        payload_sha256,
        validity,
        coverages,
    }
}

#[test]
fn exact_grid_node_returns_exact_stored_coverage() {
    let mut coverages = Vec::new();
    for node in 0..NODE_COUNT {
        let value = node as f32 / 10.0;
        coverages.extend([value, 1.0 - value, 0.2, 0.1]);
    }
    let runtime = InverseLutRuntime::from_verified(artifact(vec![true; NODE_COUNT], coverages)).unwrap();
    assert_eq!(
        runtime
            .lookup(LabColor {
                l: 100.0,
                a: 10.0,
                b: 20.0,
            })
            .unwrap(),
        vec![0.7, 0.3, 0.2, 0.1]
    );
}

#[test]
fn center_lookup_is_trilinear_average_of_eight_corners() {
    let mut coverages = Vec::new();
    for node in 0..NODE_COUNT {
        let value = node as f32 / 7.0;
        coverages.extend([value, 1.0 - value, 0.2, 0.1]);
    }
    let runtime = InverseLutRuntime::from_verified(artifact(vec![true; NODE_COUNT], coverages)).unwrap();
    let value = runtime
        .lookup(LabColor {
            l: 50.0,
            a: 0.0,
            b: 0.0,
        })
        .unwrap();
    assert!((value[0] - 0.5).abs() < 1.0e-6);
    assert!((value[1] - 0.5).abs() < 1.0e-6);
    assert!((value[2] - 0.2).abs() < 1.0e-6);
    assert!((value[3] - 0.1).abs() < 1.0e-6);
}

#[test]
fn invalid_required_corner_fails_closed_but_exact_valid_node_remains_usable() {
    let mut validity = vec![true; NODE_COUNT];
    validity[7] = false;
    let mut coverages = vec![0.25; NODE_COUNT * CHANNEL_COUNT];
    coverages[7 * CHANNEL_COUNT..8 * CHANNEL_COUNT].fill(0.0);
    let runtime = InverseLutRuntime::from_verified(artifact(validity, coverages)).unwrap();
    assert!(matches!(
        runtime.lookup(LabColor {
            l: 50.0,
            a: 0.0,
            b: 0.0,
        }),
        Err(InverseLutLookupError::UnsupportedCorner { node_index: 7 })
    ));
    assert_eq!(
        runtime
            .lookup(LabColor {
                l: 0.0,
                a: -10.0,
                b: -20.0,
            })
            .unwrap(),
        vec![0.25; CHANNEL_COUNT]
    );
}

#[test]
fn lookup_rejects_out_of_domain_non_finite_and_wrong_output_topology() {
    let runtime = InverseLutRuntime::from_verified(artifact(
        vec![true; NODE_COUNT],
        vec![0.0; NODE_COUNT * CHANNEL_COUNT],
    ))
    .unwrap();
    assert!(matches!(
        runtime.lookup(LabColor {
            l: 101.0,
            a: 0.0,
            b: 0.0,
        }),
        Err(InverseLutLookupError::OutOfDomain { axis: "L*", .. })
    ));
    assert_eq!(
        runtime.lookup(LabColor {
            l: f64::NAN,
            a: 0.0,
            b: 0.0,
        }),
        Err(InverseLutLookupError::NonFiniteLab)
    );
    let mut wrong = [0.0f32; 1];
    assert!(matches!(
        runtime.lookup_into(
            LabColor {
                l: 50.0,
                a: 0.0,
                b: 0.0,
            },
            &mut wrong,
        ),
        Err(InverseLutLookupError::InvalidArtifact(_))
    ));
}

#[test]
fn quantized_lookup_reuses_versioned_identity_quantization() {
    let runtime = InverseLutRuntime::from_verified(artifact(
        vec![true; NODE_COUNT],
        vec![0.5; NODE_COUNT * CHANNEL_COUNT],
    ))
    .unwrap();
    assert_eq!(
        runtime
            .lookup_quantized(LabColor {
                l: 50.0,
                a: 0.0,
                b: 0.0,
            })
            .unwrap(),
        vec![32_768; CHANNEL_COUNT]
    );
}

#[test]
fn runtime_rechecks_payload_digest_and_canonical_zero() {
    let mut bad_digest = artifact(
        vec![true; NODE_COUNT],
        vec![0.25; NODE_COUNT * CHANNEL_COUNT],
    );
    bad_digest.payload_sha256 = "0".repeat(64);
    assert!(matches!(
        InverseLutRuntime::from_verified(bad_digest),
        Err(InverseLutLookupError::InvalidArtifact(_))
    ));

    let mut validity = vec![true; NODE_COUNT];
    validity[3] = false;
    let mut coverages = vec![0.0; NODE_COUNT * CHANNEL_COUNT];
    coverages[3 * CHANNEL_COUNT] = -0.0;
    let forged = VerifiedInverseLutArtifact {
        identity_content_id: identity().content_id().unwrap(),
        identity: identity(),
        payload_sha256: payload_sha256(&validity, &coverages),
        validity,
        coverages,
    };
    assert!(matches!(
        InverseLutRuntime::from_verified(forged),
        Err(InverseLutLookupError::InvalidArtifact(_))
    ));
}
