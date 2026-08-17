use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use lcms2::{
    ColorSpaceSignature, ColorSpaceSignatureExt, InfoType, Locale, Profile, ProfileClassSignature,
    Tag, TagSignature,
};
use sha2::{Digest, Sha256};

use crate::color_conversion::ConversionEngineMode;
use crate::model::IccProfileIdentity;
use crate::tiff_io::ColorModel;

/// Immutable facts captured from the selected production target profile.
///
/// The ICC payload remains external. The SHA-256 identity is verified again
/// when Target Setup is reviewed and later when a conversion worker starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductionTargetProfileInspection {
    pub path: PathBuf,
    pub identity: IccProfileIdentity,
    pub device_class_label: String,
    pub source_space_label: Option<String>,
    pub output_space_label: String,
    pub output_channel_count: usize,
    pub channel_names: Vec<String>,
    pub channel_names_authoritative: bool,
}

/// Inspect either an Output-class target profile or a DeviceLink.
///
/// Normal ICC output profiles define only the destination device space; the
/// separately selected Source ICC supplies source colorimetry. DeviceLinks
/// define both input and output device spaces, so their input must match the
/// active RGB/CMYK source model.
pub fn inspect_production_target_profile(
    path: &Path,
    mode: ConversionEngineMode,
    source_model: ColorModel,
) -> Result<ProductionTargetProfileInspection, String> {
    if mode == ConversionEngineMode::CustomOptimizer {
        return Err(
            "Custom optimizer targets require characterized target data, not an ICC/DeviceLink file."
                .to_owned(),
        );
    }

    let bytes = fs::read(path).map_err(|err| {
        format!(
            "Cannot read production target profile {}: {err}",
            path.display()
        )
    })?;
    let profile = Profile::new_icc(&bytes).map_err(|err| {
        format!(
            "Cannot open production target profile {}: {err}",
            path.display()
        )
    })?;

    let facts = profile_facts(&profile, mode, source_model)?;
    let (channel_names, channel_names_authoritative) = target_channel_names(
        &profile,
        mode,
        facts.output_space,
        facts.output_channel_count,
    );

    Ok(ProductionTargetProfileInspection {
        path: path.to_path_buf(),
        identity: IccProfileIdentity {
            description: profile_description(&profile),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        },
        device_class_label: profile_class_label(profile.device_class()),
        source_space_label: facts.source_space.map(color_space_label),
        output_space_label: color_space_label(facts.output_space),
        output_channel_count: facts.output_channel_count,
        channel_names,
        channel_names_authoritative,
    })
}

/// Re-inspect an assigned target and reject replacement at the stored path.
pub fn verify_production_target_profile(
    path: &Path,
    expected_identity: &IccProfileIdentity,
    mode: ConversionEngineMode,
    source_model: ColorModel,
) -> Result<ProductionTargetProfileInspection, String> {
    if expected_identity.sha256.trim().is_empty() {
        return Err(
            "Production target profile has no stored SHA-256 identity. Select it again.".to_owned(),
        );
    }
    let inspected = inspect_production_target_profile(path, mode, source_model)?;
    if !inspected
        .identity
        .sha256
        .eq_ignore_ascii_case(expected_identity.sha256.trim())
    {
        return Err(format!(
            "Production target profile at {} no longer matches stored profile '{}'. Select or relink the target profile before conversion.",
            path.display(),
            expected_identity.description
        ));
    }
    Ok(inspected)
}

#[derive(Debug)]
struct ProfileFacts {
    source_space: Option<ColorSpaceSignature>,
    output_space: ColorSpaceSignature,
    output_channel_count: usize,
}

fn profile_facts(
    profile: &Profile,
    mode: ConversionEngineMode,
    source_model: ColorModel,
) -> Result<ProfileFacts, String> {
    match mode {
        ConversionEngineMode::Icc => {
            if profile.device_class() != ProfileClassSignature::OutputClass {
                return Err(format!(
                    "Standard ICC target must be an Output/printer profile; selected profile is {}.",
                    profile_class_label(profile.device_class())
                ));
            }
            let output_space = profile.color_space();
            let output_channel_count = output_space.channels() as usize;
            validate_output_channels(output_space, output_channel_count)?;
            Ok(ProfileFacts {
                source_space: None,
                output_space,
                output_channel_count,
            })
        }
        ConversionEngineMode::DeviceLink => {
            if profile.device_class() != ProfileClassSignature::LinkClass {
                return Err(format!(
                    "DeviceLink mode requires a DeviceLink profile; selected profile is {}.",
                    profile_class_label(profile.device_class())
                ));
            }
            let source_space = profile.color_space();
            let expected_source = expected_source_space(source_model).ok_or_else(|| {
                format!(
                    "{} source data cannot be used as a production DeviceLink input.",
                    source_model.title()
                )
            })?;
            if source_space != expected_source {
                return Err(format!(
                    "DeviceLink input space {} does not match source {}.",
                    color_space_label(source_space),
                    source_model.title()
                ));
            }
            let output_space = profile.pcs();
            let output_channel_count = output_space.channels() as usize;
            validate_output_channels(output_space, output_channel_count)?;
            Ok(ProfileFacts {
                source_space: Some(source_space),
                output_space,
                output_channel_count,
            })
        }
        ConversionEngineMode::CustomOptimizer => unreachable!("handled before profile inspection"),
    }
}

fn validate_output_channels(
    output_space: ColorSpaceSignature,
    output_channel_count: usize,
) -> Result<(), String> {
    if output_channel_count == 4 && output_space == ColorSpaceSignature::CmykData {
        return Ok(());
    }
    if (5..=12).contains(&output_channel_count) {
        return Ok(());
    }
    Err(format!(
        "Production target space {} has {} channels; current production transforms support CMYK (4) or N-channel 5..=12 output.",
        color_space_label(output_space),
        output_channel_count
    ))
}

fn target_channel_names(
    profile: &Profile,
    mode: ConversionEngineMode,
    output_space: ColorSpaceSignature,
    channel_count: usize,
) -> (Vec<String>, bool) {
    if output_space == ColorSpaceSignature::CmykData {
        return (
            ["Cyan", "Magenta", "Yellow", "Black"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            true,
        );
    }

    let primary_tag = match mode {
        ConversionEngineMode::DeviceLink => TagSignature::ColorantTableOutTag,
        ConversionEngineMode::Icc => TagSignature::ColorantTableTag,
        ConversionEngineMode::CustomOptimizer => unreachable!(),
    };
    for tag in [primary_tag, TagSignature::ColorantTableTag] {
        if let Tag::NamedColorList(list) = profile.read_tag(tag) {
            let names = list
                .colors()
                .into_iter()
                .map(|color| color.name.trim().to_owned())
                .collect::<Vec<_>>();
            if valid_channel_names(&names, channel_count) {
                return (names, true);
            }
        }
    }

    (
        (1..=channel_count)
            .map(|index| format!("Ink {index}"))
            .collect(),
        false,
    )
}

pub fn validate_target_channel_names(
    names: &[String],
    expected_count: usize,
) -> Result<(), String> {
    if names.len() != expected_count {
        return Err(format!(
            "Target topology declares {} channel names but the profile outputs {expected_count} channels.",
            names.len()
        ));
    }
    let mut unique = BTreeSet::new();
    for name in names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("Target channel names cannot be empty.".to_owned());
        }
        if !unique.insert(trimmed.to_ascii_lowercase()) {
            return Err(format!("Duplicate target channel name '{trimmed}'."));
        }
    }
    Ok(())
}

fn valid_channel_names(names: &[String], expected_count: usize) -> bool {
    validate_target_channel_names(names, expected_count).is_ok()
}

fn expected_source_space(model: ColorModel) -> Option<ColorSpaceSignature> {
    match model {
        ColorModel::Rgb => Some(ColorSpaceSignature::RgbData),
        ColorModel::Cmyk => Some(ColorSpaceSignature::CmykData),
        ColorModel::Gray | ColorModel::Other => None,
    }
}

fn profile_description(profile: &Profile) -> String {
    profile
        .info(InfoType::Description, Locale::none())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "ICC profile".to_owned())
}

fn color_space_label(space: ColorSpaceSignature) -> String {
    match space {
        ColorSpaceSignature::RgbData => "RGB".to_owned(),
        ColorSpaceSignature::CmykData => "CMYK".to_owned(),
        ColorSpaceSignature::GrayData => "Gray".to_owned(),
        other => format!("{other:?}"),
    }
}

fn profile_class_label(class: ProfileClassSignature) -> String {
    match class {
        ProfileClassSignature::InputClass => "Input".to_owned(),
        ProfileClassSignature::DisplayClass => "Display".to_owned(),
        ProfileClassSignature::OutputClass => "Output / printer".to_owned(),
        ProfileClassSignature::LinkClass => "DeviceLink".to_owned(),
        ProfileClassSignature::AbstractClass => "Abstract".to_owned(),
        ProfileClassSignature::ColorSpaceClass => "Color space".to_owned(),
        ProfileClassSignature::NamedColorClass => "Named color".to_owned(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_cmyk_topology_is_authoritative() {
        let profile = Profile::new_srgb();
        let (names, authoritative) = target_channel_names(
            &profile,
            ConversionEngineMode::Icc,
            ColorSpaceSignature::CmykData,
            4,
        );
        assert!(authoritative);
        assert_eq!(names, ["Cyan", "Magenta", "Yellow", "Black"]);
    }

    #[test]
    fn missing_nchannel_colorant_table_requires_operator_confirmation() {
        let profile = Profile::new_srgb();
        let (names, authoritative) = target_channel_names(
            &profile,
            ConversionEngineMode::Icc,
            ColorSpaceSignature::Sig7colorData,
            7,
        );
        assert!(!authoritative);
        assert_eq!(names.first().map(String::as_str), Some("Ink 1"));
        assert_eq!(names.last().map(String::as_str), Some("Ink 7"));
    }

    #[test]
    fn target_channel_names_must_match_count_and_be_unique() {
        let valid = ["Blue", "Brown", "Black"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert!(validate_target_channel_names(&valid, 3).is_ok());

        let duplicate = ["Black", " black "]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert!(validate_target_channel_names(&duplicate, 2).is_err());
        assert!(validate_target_channel_names(&valid, 4).is_err());
    }

    #[test]
    fn display_profile_is_rejected_as_production_output_target() {
        let profile = Profile::new_srgb();
        let error = profile_facts(&profile, ConversionEngineMode::Icc, ColorModel::Rgb)
            .expect_err("sRGB display profile is not an output target");
        assert!(error.contains("Output/printer"));
    }

    #[test]
    fn unsupported_output_channel_counts_are_rejected() {
        assert!(validate_output_channels(ColorSpaceSignature::CmykData, 4).is_ok());
        assert!(validate_output_channels(ColorSpaceSignature::Sig4colorData, 4).is_err());
        assert!(validate_output_channels(ColorSpaceSignature::Sig7colorData, 7).is_ok());
        assert!(validate_output_channels(ColorSpaceSignature::RgbData, 3).is_err());
        assert!(validate_output_channels(ColorSpaceSignature::Sig13colorData, 13).is_err());
    }
}
