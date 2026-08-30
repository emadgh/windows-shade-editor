use lcms2::{
    CIExyY, ColorSpaceSignature, ColorSpaceSignatureExt, Flags, GlobalContext, Intent, PixelFormat,
    Profile, ProfileClassSignature, Transform,
};
use sha2::{Digest, Sha256};

use crate::color_conversion::{ConversionRenderingIntent, ConversionTargetDefinition};
use crate::device_characterization::{CharacterizationIdentity, DeviceForwardModel, LabColor};
use crate::icc_conversion::to_lcms_intent;

pub const ICC_FORWARD_MODEL_MIN_CHANNELS: usize = 4;
pub const ICC_FORWARD_MODEL_MAX_CHANNELS: usize = 12;

type LabPixel = [f64; 3];

enum DeviceToLabTransform {
    C4(Transform<[u16; 4], LabPixel>),
    C5(Transform<[u16; 5], LabPixel>),
    C6(Transform<[u16; 6], LabPixel>),
    C7(Transform<[u16; 7], LabPixel>),
    C8(Transform<[u16; 8], LabPixel>),
    C9(Transform<[u16; 9], LabPixel>),
    C10(Transform<[u16; 10], LabPixel>),
    C11(Transform<[u16; 11], LabPixel>),
    C12(Transform<[u16; 12], LabPixel>),
}

/// Identity-bound device→PCS model backed by an existing printer/output ICC.
///
/// This is intentionally a forward color model, not a second ICC separation
/// path. The Custom Optimizer asks this model what Lab color a candidate ink
/// vector represents, then the existing inverse solver chooses between
/// colorimetrically acceptable candidates using Black/per-ink/total-ink policy.
pub struct IccDeviceForwardModel {
    identity: CharacterizationIdentity,
    transform: DeviceToLabTransform,
    rendering_intent: ConversionRenderingIntent,
    black_point_compensation: bool,
}

impl IccDeviceForwardModel {
    pub fn from_output_icc_bytes(
        profile_bytes: &[u8],
        expected_sha256: &str,
        channel_names: Vec<String>,
        rendering_intent: ConversionRenderingIntent,
        black_point_compensation: bool,
    ) -> Result<Self, String> {
        validate_profile_hash(profile_bytes, expected_sha256)?;
        validate_channel_names(&channel_names)?;

        let profile = Profile::new_icc(profile_bytes)
            .map_err(|error| format!("Cannot open target Output ICC for optimizer modeling: {error}"))?;
        if profile.device_class() != ProfileClassSignature::OutputClass {
            return Err("Custom Optimizer ICC forward model requires an Output-class ICC profile.".to_owned());
        }
        let channel_count = profile.color_space().channels() as usize;
        if channel_count != channel_names.len() {
            return Err(format!(
                "Output ICC channel topology mismatch: profile declares {channel_count} channels but target declares {}.",
                channel_names.len()
            ));
        }
        if !(ICC_FORWARD_MODEL_MIN_CHANNELS..=ICC_FORWARD_MODEL_MAX_CHANNELS)
            .contains(&channel_count)
        {
            return Err(format!(
                "Output ICC forward model supports {ICC_FORWARD_MODEL_MIN_CHANNELS}..={ICC_FORWARD_MODEL_MAX_CHANNELS} channels; profile declares {channel_count}."
            ));
        }
        if !matches!(profile.pcs(), ColorSpaceSignature::LabData | ColorSpaceSignature::XYZData) {
            return Err(format!(
                "Output ICC forward model requires Lab or XYZ PCS; profile declares {:?}.",
                profile.pcs()
            ));
        }

        let lab = Profile::new_lab4_context(GlobalContext::new(), CIExyY::d50())
            .map_err(|error| format!("Cannot create D50 Lab identity profile: {error}"))?;
        let intent = to_lcms_intent(rendering_intent);
        let transform = create_transform(channel_count, &profile, &lab, intent, black_point_compensation)?;

        Ok(Self {
            identity: CharacterizationIdentity {
                id: expected_sha256.trim().to_ascii_lowercase(),
                channel_names,
            },
            transform,
            rendering_intent,
            black_point_compensation,
        })
    }

    pub fn rendering_intent(&self) -> ConversionRenderingIntent {
        self.rendering_intent
    }

    pub fn black_point_compensation(&self) -> bool {
        self.black_point_compensation
    }
}

impl DeviceForwardModel for IccDeviceForwardModel {
    fn identity(&self) -> &CharacterizationIdentity {
        &self.identity
    }

    fn predict_lab(&self, coverages: &[f32]) -> Result<LabColor, String> {
        if coverages.len() != self.identity.channel_names.len() {
            return Err(format!(
                "Output ICC candidate topology mismatch: model has {} channels, candidate has {}.",
                self.identity.channel_names.len(),
                coverages.len()
            ));
        }
        let encoded = quantize_coverages(coverages)?;
        let [l, a, b] = self.transform.predict(&encoded)?;
        if !l.is_finite() || !a.is_finite() || !b.is_finite() {
            return Err("Output ICC device→PCS transform produced non-finite Lab.".to_owned());
        }
        if !(-1.0e-9..=100.0 + 1.0e-9).contains(&l) {
            return Err(format!(
                "Output ICC device→PCS transform produced out-of-range L* {l}."
            ));
        }
        Ok(LabColor { l, a, b })
    }
}

impl DeviceToLabTransform {
    fn predict(&self, samples: &[u16]) -> Result<LabPixel, String> {
        match self {
            Self::C4(transform) => transform_one::<4>(transform, samples),
            Self::C5(transform) => transform_one::<5>(transform, samples),
            Self::C6(transform) => transform_one::<6>(transform, samples),
            Self::C7(transform) => transform_one::<7>(transform, samples),
            Self::C8(transform) => transform_one::<8>(transform, samples),
            Self::C9(transform) => transform_one::<9>(transform, samples),
            Self::C10(transform) => transform_one::<10>(transform, samples),
            Self::C11(transform) => transform_one::<11>(transform, samples),
            Self::C12(transform) => transform_one::<12>(transform, samples),
        }
    }
}

fn transform_one<const N: usize>(
    transform: &Transform<[u16; N], LabPixel>,
    samples: &[u16],
) -> Result<LabPixel, String> {
    if samples.len() != N {
        return Err(format!(
            "Output ICC transform expected {N} device channels, got {}.",
            samples.len()
        ));
    }
    let input = [std::array::from_fn(|index| samples[index])];
    let mut output = [[0.0f64; 3]; 1];
    transform.transform_pixels(&input, &mut output);
    Ok(output[0])
}

fn create_transform(
    channel_count: usize,
    profile: &Profile,
    lab: &Profile,
    intent: Intent,
    black_point_compensation: bool,
) -> Result<DeviceToLabTransform, String> {
    macro_rules! build {
        ($n:literal, $format:expr, $variant:ident) => {{
            let result: lcms2::LCMSResult<Transform<[u16; $n], LabPixel>> =
                if black_point_compensation {
                    Transform::new_flags(
                        profile,
                        $format,
                        lab,
                        PixelFormat::Lab_DBL,
                        intent,
                        Flags::BLACKPOINT_COMPENSATION,
                    )
                } else {
                    Transform::new(profile, $format, lab, PixelFormat::Lab_DBL, intent)
                };
            result
                .map(DeviceToLabTransform::$variant)
                .map_err(|error| format!(
                    "Output ICC does not expose a usable {0}-channel device→PCS transform for the selected rendering intent: {error}",
                    $n
                ))
        }};
    }

    match channel_count {
        4 => build!(4, PixelFormat::CMYK_16, C4),
        5 => build!(5, PixelFormat::CMYK5_16, C5),
        6 => build!(6, PixelFormat::CMYK6_16, C6),
        7 => build!(7, PixelFormat::CMYK7_16, C7),
        8 => build!(8, PixelFormat::CMYK8_16, C8),
        9 => build!(9, PixelFormat::CMYK9_16, C9),
        10 => build!(10, PixelFormat::CMYK10_16, C10),
        11 => build!(11, PixelFormat::CMYK11_16, C11),
        12 => build!(12, PixelFormat::CMYK12_16, C12),
        _ => Err(format!(
            "Output ICC forward model supports 4..=12 channels; requested {channel_count}."
        )),
    }
}

fn validate_profile_hash(profile_bytes: &[u8], expected_sha256: &str) -> Result<(), String> {
    let expected = expected_sha256.trim();
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Target Output ICC identity must be a full SHA-256 hash.".to_owned());
    }
    let actual = format!("{:x}", Sha256::digest(profile_bytes));
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "Target Output ICC identity mismatch: expected {expected}, actual {actual}."
        ))
    }
}

fn validate_channel_names(channel_names: &[String]) -> Result<(), String> {
    if !(ICC_FORWARD_MODEL_MIN_CHANNELS..=ICC_FORWARD_MODEL_MAX_CHANNELS)
        .contains(&channel_names.len())
    {
        return Err(format!(
            "Output ICC forward model requires 4..=12 target channels; got {}.",
            channel_names.len()
        ));
    }
    if channel_names.iter().any(|name| name.trim().is_empty()) {
        return Err("Output ICC forward model channel names cannot be empty.".to_owned());
    }
    let mut normalized = channel_names
        .iter()
        .map(|name| name.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    if normalized.len() != channel_names.len() {
        return Err("Output ICC forward model channel names must be unique.".to_owned());
    }
    Ok(())
}

fn quantize_coverages(coverages: &[f32]) -> Result<Vec<u16>, String> {
    coverages
        .iter()
        .enumerate()
        .map(|(index, value)| {
            if !value.is_finite() || !(0.0..=1.0).contains(value) {
                return Err(format!(
                    "Output ICC candidate channel {index} has coverage {value}; expected 0..=1."
                ));
            }
            Ok((value * 65_535.0).round() as u16)
        })
        .collect()
}

/// A target may authorize the inverse solver from either its measured
/// characterization identity or the exact SHA-256 of its Output ICC forward
/// model. Measured characterization remains valid and takes no semantic hit.
pub fn target_accepts_forward_model_identity(
    target: &ConversionTargetDefinition,
    model_id: &str,
) -> bool {
    let model_id = model_id.trim();
    target
        .characterization_id
        .as_deref()
        .is_some_and(|id| !id.trim().is_empty() && id.trim() == model_id)
        || target.output_profile_identity.as_ref().is_some_and(|identity| {
            !identity.sha256.trim().is_empty()
                && identity.sha256.trim().eq_ignore_ascii_case(model_id)
        })
}

pub fn target_forward_model_authorities(target: &ConversionTargetDefinition) -> Vec<String> {
    let mut authorities = Vec::new();
    if let Some(id) = target
        .characterization_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
    {
        authorities.push(id.trim().to_owned());
    }
    if let Some(identity) = target
        .output_profile_identity
        .as_ref()
        .filter(|identity| !identity.sha256.trim().is_empty())
    {
        authorities.push(identity.sha256.trim().to_ascii_lowercase());
    }
    authorities
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::TargetChannelDefinition;
    use crate::model::IccProfileIdentity;

    fn target() -> ConversionTargetDefinition {
        ConversionTargetDefinition {
            name: "Profile-backed 4C".to_owned(),
            channels: ["Blue", "Brown", "Beige", "Black"]
                .into_iter()
                .map(|name| TargetChannelDefinition {
                    name: name.to_owned(),
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: Some(1.0),
                })
                .collect(),
            bit_depth: 16,
            output_profile_identity: Some(IccProfileIdentity {
                description: "Existing ceramic profile".to_owned(),
                sha256: "a".repeat(64),
            }),
            output_profile_path: Some("existing.icc".to_owned()),
            device_link_identity: None,
            device_link_path: None,
            characterization_id: None,
            total_ink_limit: Some(2.4),
        }
    }

    #[test]
    fn profile_identity_can_authorize_forward_model_without_measurement_id() {
        assert!(target_accepts_forward_model_identity(&target(), &"A".repeat(64)));
        assert!(!target_accepts_forward_model_identity(&target(), &"b".repeat(64)));
    }

    #[test]
    fn measured_identity_remains_supported() {
        let mut target = target();
        target.characterization_id = Some("measured-v1".to_owned());
        assert!(target_accepts_forward_model_identity(&target, "measured-v1"));
    }

    #[test]
    fn coverage_quantization_is_bounded_and_rejects_invalid_values() {
        assert_eq!(quantize_coverages(&[0.0, 0.5, 1.0]).unwrap(), [0, 32768, 65535]);
        assert!(quantize_coverages(&[-0.1]).is_err());
        assert!(quantize_coverages(&[1.1]).is_err());
        assert!(quantize_coverages(&[f32::NAN]).is_err());
    }

    #[test]
    fn output_profile_hash_is_exact() {
        let bytes = b"icc-forward-model-hash-fixture";
        let hash = format!("{:x}", Sha256::digest(bytes));
        assert!(validate_profile_hash(bytes, &hash).is_ok());
        assert!(validate_profile_hash(bytes, &"0".repeat(64)).is_err());
        assert!(validate_profile_hash(bytes, "short").is_err());
    }

    #[test]
    fn non_output_profile_fails_before_transform_use() {
        let bytes = Profile::new_srgb().icc().unwrap();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let error = IccDeviceForwardModel::from_output_icc_bytes(
            &bytes,
            &hash,
            ["C", "M", "Y", "K"].into_iter().map(str::to_owned).collect(),
            ConversionRenderingIntent::RelativeColorimetric,
            false,
        )
        .err()
        .expect("sRGB must not be accepted as an output printer model");
        assert!(error.contains("Output-class"));
    }

    #[test]
    fn unsupported_channel_count_fails_closed() {
        let error = validate_channel_names(&["A", "B", "C"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>())
        .unwrap_err();
        assert!(error.contains("4..=12"));
    }
}