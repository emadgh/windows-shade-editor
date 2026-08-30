use std::collections::BTreeSet;

use lcms2::{
    ColorSpaceSignature, ColorSpaceSignatureExt, Intent, PixelFormat, Profile,
    ProfileClassSignature, Transform,
};
use sha2::{Digest, Sha256};

use crate::color_conversion::ConversionRenderingIntent;
use crate::device_characterization::{CharacterizationIdentity, DeviceForwardModel, LabColor};
use crate::model::IccProfileIdentity;

pub const OUTPUT_ICC_FORWARD_MODEL_ID_PREFIX: &str = "output-icc-sha256:";

const D50_X: f64 = 0.9642;
const D50_Y: f64 = 1.0;
const D50_Z: f64 = 0.8249;

enum DeviceToXyzTransform {
    C4(Transform<[u16; 4], [f64; 3]>),
    C5(Transform<[u16; 5], [f64; 3]>),
    C6(Transform<[u16; 6], [f64; 3]>),
    C7(Transform<[u16; 7], [f64; 3]>),
    C8(Transform<[u16; 8], [f64; 3]>),
    C9(Transform<[u16; 9], [f64; 3]>),
    C10(Transform<[u16; 10], [f64; 3]>),
    C11(Transform<[u16; 11], [f64; 3]>),
    C12(Transform<[u16; 12], [f64; 3]>),
}

/// Device→D50 Lab adapter backed by the exact Output ICC already used for production.
///
/// This is not measured-characterization evidence and does not grant measured production
/// authority. It lets the inverse separation solver ask the existing Output ICC what PCS
/// color a candidate device vector represents, so Black/per-ink preferences can choose an
/// alternate separation without post-transform channel multiplication.
pub struct OutputIccForwardModel {
    identity: CharacterizationIdentity,
    transform: DeviceToXyzTransform,
}

impl OutputIccForwardModel {
    pub fn from_bytes(
        profile_identity: &IccProfileIdentity,
        channel_names: Vec<String>,
        bytes: &[u8],
        rendering_intent: ConversionRenderingIntent,
    ) -> Result<Self, String> {
        validate_profile_identity(profile_identity, bytes)?;
        validate_channel_names(&channel_names)?;

        let profile = Profile::new_icc(bytes)
            .map_err(|error| format!("Cannot open Output ICC forward model: {error}"))?;
        if profile.device_class() != ProfileClassSignature::OutputClass {
            return Err("ICC-backed optimizer requires an Output-class target profile.".to_owned());
        }
        if !matches!(profile.pcs(), ColorSpaceSignature::LabData | ColorSpaceSignature::XYZData) {
            return Err(format!(
                "ICC-backed optimizer requires Lab or XYZ PCS; profile declares {:?}.",
                profile.pcs()
            ));
        }

        let channels = profile.color_space().channels() as usize;
        if !(4..=12).contains(&channels) {
            return Err(format!(
                "ICC-backed optimizer supports 4..=12 output channels; profile declares {channels}."
            ));
        }
        if channel_names.len() != channels {
            return Err(format!(
                "Output ICC topology mismatch: profile declares {channels} channels but target names declare {}.",
                channel_names.len()
            ));
        }

        let intent = to_lcms_intent(rendering_intent);
        let transform = match channels {
            4 => DeviceToXyzTransform::C4(build_transform::<4>(
                &profile,
                PixelFormat::CMYK_16,
                intent,
            )?),
            5 => DeviceToXyzTransform::C5(build_transform::<5>(
                &profile,
                PixelFormat::CMYK5_16,
                intent,
            )?),
            6 => DeviceToXyzTransform::C6(build_transform::<6>(
                &profile,
                PixelFormat::CMYK6_16,
                intent,
            )?),
            7 => DeviceToXyzTransform::C7(build_transform::<7>(
                &profile,
                PixelFormat::CMYK7_16,
                intent,
            )?),
            8 => DeviceToXyzTransform::C8(build_transform::<8>(
                &profile,
                PixelFormat::CMYK8_16,
                intent,
            )?),
            9 => DeviceToXyzTransform::C9(build_transform::<9>(
                &profile,
                PixelFormat::CMYK9_16,
                intent,
            )?),
            10 => DeviceToXyzTransform::C10(build_transform::<10>(
                &profile,
                PixelFormat::CMYK10_16,
                intent,
            )?),
            11 => DeviceToXyzTransform::C11(build_transform::<11>(
                &profile,
                PixelFormat::CMYK11_16,
                intent,
            )?),
            12 => DeviceToXyzTransform::C12(build_transform::<12>(
                &profile,
                PixelFormat::CMYK12_16,
                intent,
            )?),
            _ => unreachable!("channel range checked above"),
        };

        Ok(Self {
            identity: CharacterizationIdentity {
                id: output_icc_forward_model_id(profile_identity)?,
                channel_names,
            },
            transform,
        })
    }
}

impl DeviceForwardModel for OutputIccForwardModel {
    fn identity(&self) -> &CharacterizationIdentity {
        &self.identity
    }

    fn predict_lab(&self, coverages: &[f32]) -> Result<LabColor, String> {
        if coverages.len() != self.identity.channel_names.len() {
            return Err(format!(
                "Output ICC forward-model topology mismatch: expected {} channels, found {}.",
                self.identity.channel_names.len(),
                coverages.len()
            ));
        }
        if coverages
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err("Output ICC forward-model coverage must be finite and within 0..=1.".to_owned());
        }

        match &self.transform {
            DeviceToXyzTransform::C4(transform) => predict_with::<4>(transform, coverages),
            DeviceToXyzTransform::C5(transform) => predict_with::<5>(transform, coverages),
            DeviceToXyzTransform::C6(transform) => predict_with::<6>(transform, coverages),
            DeviceToXyzTransform::C7(transform) => predict_with::<7>(transform, coverages),
            DeviceToXyzTransform::C8(transform) => predict_with::<8>(transform, coverages),
            DeviceToXyzTransform::C9(transform) => predict_with::<9>(transform, coverages),
            DeviceToXyzTransform::C10(transform) => predict_with::<10>(transform, coverages),
            DeviceToXyzTransform::C11(transform) => predict_with::<11>(transform, coverages),
            DeviceToXyzTransform::C12(transform) => predict_with::<12>(transform, coverages),
        }
    }
}

pub fn output_icc_forward_model_id(identity: &IccProfileIdentity) -> Result<String, String> {
    let hash = identity.sha256.trim();
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Output ICC forward-model identity requires a 64-digit SHA-256 hash.".to_owned());
    }
    Ok(format!(
        "{OUTPUT_ICC_FORWARD_MODEL_ID_PREFIX}{}",
        hash.to_ascii_lowercase()
    ))
}

fn validate_profile_identity(identity: &IccProfileIdentity, bytes: &[u8]) -> Result<(), String> {
    let expected = identity.sha256.trim();
    output_icc_forward_model_id(identity)?;
    let actual = format!("{:x}", Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(format!(
            "Output ICC identity changed before optimizer use (expected {expected}, found {actual})."
        ));
    }
    Ok(())
}

fn validate_channel_names(channel_names: &[String]) -> Result<(), String> {
    if !(4..=12).contains(&channel_names.len()) {
        return Err(format!(
            "ICC-backed optimizer requires 4..=12 authoritative target channel names; found {}.",
            channel_names.len()
        ));
    }
    let mut seen = BTreeSet::new();
    for name in channel_names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("ICC-backed optimizer target channel names cannot be empty.".to_owned());
        }
        if !seen.insert(trimmed.to_ascii_lowercase()) {
            return Err(format!(
                "ICC-backed optimizer target channel name '{trimmed}' is duplicated."
            ));
        }
    }
    Ok(())
}

fn build_transform<const N: usize>(
    profile: &Profile,
    input_format: PixelFormat,
    intent: Intent,
) -> Result<Transform<[u16; N], [f64; 3]>, String> {
    let xyz = Profile::new_xyz();
    Transform::new(
        profile,
        input_format,
        &xyz,
        PixelFormat::XYZ_DBL,
        intent,
    )
    .map_err(|error| {
        format!(
            "Cannot create Output ICC {N}-channel device→PCS transform for optimizer: {error}"
        )
    })
}

fn predict_with<const N: usize>(
    transform: &Transform<[u16; N], [f64; 3]>,
    coverages: &[f32],
) -> Result<LabColor, String> {
    let input = std::array::from_fn(|index| coverage_to_u16(coverages[index]));
    let mut output = [[0.0f64; 3]; 1];
    transform.transform_pixels(std::slice::from_ref(&input), &mut output);
    xyz_d50_to_lab(output[0])
}

fn coverage_to_u16(value: f32) -> u16 {
    (value.clamp(0.0, 1.0) * f32::from(u16::MAX)).round() as u16
}

fn xyz_d50_to_lab([x, y, z]: [f64; 3]) -> Result<LabColor, String> {
    if !x.is_finite() || !y.is_finite() || !z.is_finite() || x < 0.0 || y < 0.0 || z < 0.0 {
        return Err("Output ICC device→PCS transform produced invalid XYZ values.".to_owned());
    }
    let fx = lab_f(x / D50_X);
    let fy = lab_f(y / D50_Y);
    let fz = lab_f(z / D50_Z);
    let lab = LabColor {
        l: 116.0 * fy - 16.0,
        a: 500.0 * (fx - fy),
        b: 200.0 * (fy - fz),
    };
    if lab.l.is_finite() && lab.a.is_finite() && lab.b.is_finite() {
        Ok(lab)
    } else {
        Err("Output ICC device→PCS transform produced invalid Lab values.".to_owned())
    }
}

fn lab_f(value: f64) -> f64 {
    const DELTA: f64 = 6.0 / 29.0;
    const DELTA_CUBED: f64 = DELTA * DELTA * DELTA;
    if value > DELTA_CUBED {
        value.cbrt()
    } else {
        value / (3.0 * DELTA * DELTA) + 4.0 / 29.0
    }
}

fn to_lcms_intent(intent: ConversionRenderingIntent) -> Intent {
    match intent {
        ConversionRenderingIntent::Perceptual => Intent::Perceptual,
        ConversionRenderingIntent::RelativeColorimetric => Intent::RelativeColorimetric,
        ConversionRenderingIntent::Saturation => Intent::Saturation,
        ConversionRenderingIntent::AbsoluteColorimetric => Intent::AbsoluteColorimetric,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(hash: &str) -> IccProfileIdentity {
        IccProfileIdentity {
            description: "Output".to_owned(),
            sha256: hash.to_owned(),
        }
    }

    #[test]
    fn forward_model_identity_is_exact_sha_bound() {
        let hash = "A".repeat(64);
        assert_eq!(
            output_icc_forward_model_id(&identity(&hash)).unwrap(),
            format!("{OUTPUT_ICC_FORWARD_MODEL_ID_PREFIX}{}", "a".repeat(64))
        );
        assert!(output_icc_forward_model_id(&identity("short")).is_err());
    }

    #[test]
    fn d50_white_and_black_convert_to_expected_lab() {
        let white = xyz_d50_to_lab([D50_X, D50_Y, D50_Z]).unwrap();
        assert!((white.l - 100.0).abs() < 1.0e-9);
        assert!(white.a.abs() < 1.0e-9);
        assert!(white.b.abs() < 1.0e-9);

        let black = xyz_d50_to_lab([0.0, 0.0, 0.0]).unwrap();
        assert!(black.l.abs() < 1.0e-9);
        assert!(black.a.abs() < 1.0e-9);
        assert!(black.b.abs() < 1.0e-9);
    }

    #[test]
    fn coverage_quantization_preserves_endpoints() {
        assert_eq!(coverage_to_u16(0.0), 0);
        assert_eq!(coverage_to_u16(1.0), u16::MAX);
        assert_eq!(coverage_to_u16(-1.0), 0);
        assert_eq!(coverage_to_u16(2.0), u16::MAX);
    }

    #[test]
    fn channel_names_are_bounded_explicit_and_unique() {
        assert!(validate_channel_names(&["Black".to_owned(); 3]).is_err());
        assert!(validate_channel_names(&["Black".to_owned(); 13]).is_err());
        assert!(validate_channel_names(&[
            "Blue".to_owned(),
            "Brown".to_owned(),
            "Beige".to_owned(),
            "Black".to_owned(),
        ])
        .is_ok());
        assert!(validate_channel_names(&[
            "Blue".to_owned(),
            "blue".to_owned(),
            "Beige".to_owned(),
            "Black".to_owned(),
        ])
        .is_err());
    }

    #[test]
    fn display_profile_is_rejected_before_optimizer_transform() {
        let profile = Profile::new_srgb();
        let bytes = profile.icc().unwrap();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let error = OutputIccForwardModel::from_bytes(
            &identity(&hash),
            vec![
                "Blue".to_owned(),
                "Brown".to_owned(),
                "Beige".to_owned(),
                "Black".to_owned(),
            ],
            &bytes,
            ConversionRenderingIntent::RelativeColorimetric,
        )
        .err()
        .expect("sRGB display profile must not become optimizer authority");
        assert!(error.contains("Output-class"));
    }

    #[test]
    fn profile_bytes_must_match_captured_sha_before_opening() {
        let profile = Profile::new_srgb();
        let bytes = profile.icc().unwrap();
        let error = OutputIccForwardModel::from_bytes(
            &identity(&"0".repeat(64)),
            vec![
                "Blue".to_owned(),
                "Brown".to_owned(),
                "Beige".to_owned(),
                "Black".to_owned(),
            ],
            &bytes,
            ConversionRenderingIntent::RelativeColorimetric,
        )
        .err()
        .expect("wrong SHA must fail closed");
        assert!(error.contains("identity changed"));
    }
}
