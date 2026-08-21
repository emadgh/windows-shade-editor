use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use lcms2::{ColorSpaceSignature, Profile, Tag, TagSignature};

use crate::color_conversion::ConversionEngineMode;
use crate::icc_profile_registry::{
    inspect_profile_fresh, IccProfileRecord, IccProfileRegistry, IccProfileRole,
};
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
/// Identity, role and declared input/output topology come from the canonical ICC
/// registry. Production target inspection adds only channel-name/colorant-table policy.
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

    // Selection is an identity-bearing persistence boundary: bypass browse cache and
    // capture facts from a fresh byte read/hash.
    let record = inspect_profile_fresh(path)?;
    inspect_target_record(path, record, mode, source_model)
}

/// Re-inspect an assigned target and reject replacement at the stored path.
pub fn verify_production_target_profile(
    path: &Path,
    expected_identity: &IccProfileIdentity,
    mode: ConversionEngineMode,
    source_model: ColorModel,
) -> Result<ProductionTargetProfileInspection, String> {
    if mode == ConversionEngineMode::CustomOptimizer {
        return Err(
            "Custom optimizer targets require characterized target data, not an ICC/DeviceLink file."
                .to_owned(),
        );
    }
    let record = IccProfileRegistry.verify_identity(path, expected_identity)?;
    inspect_target_record(path, record, mode, source_model)
}

fn inspect_target_record(
    path: &Path,
    record: IccProfileRecord,
    mode: ConversionEngineMode,
    source_model: ColorModel,
) -> Result<ProductionTargetProfileInspection, String> {
    let facts = profile_facts(&record, mode, source_model)?;

    // LittleCMS is reopened only for target ColorantTable/ColorantTableOut tags.
    // Identity, role and topology have already been frozen by the registry record.
    let profile = Profile::new_file(path).map_err(|err| {
        format!(
            "Cannot open production target profile {} for colorant metadata: {err}",
            path.display()
        )
    })?;
    let (channel_names, channel_names_authoritative) = target_channel_names(
        &profile,
        mode,
        &facts.output_space_label,
        facts.output_channel_count,
    );

    // Fail closed if the external profile changed while LittleCMS was reopened to read
    // ColorantTable metadata. The returned channel names must belong to the same exact
    // bytes whose identity/role/topology were frozen above.
    IccProfileRegistry.verify_identity(path, &record.identity)?;

    Ok(ProductionTargetProfileInspection {
        path: path.to_path_buf(),
        identity: record.identity,
        device_class_label: record.role.label().to_owned(),
        source_space_label: facts.source_space_label,
        output_space_label: facts.output_space_label,
        output_channel_count: facts.output_channel_count,
        channel_names,
        channel_names_authoritative,
    })
}

#[derive(Debug)]
struct ProfileFacts {
    source_space_label: Option<String>,
    output_space_label: String,
    output_channel_count: usize,
}

fn profile_facts(
    record: &IccProfileRecord,
    mode: ConversionEngineMode,
    source_model: ColorModel,
) -> Result<ProfileFacts, String> {
    match mode {
        ConversionEngineMode::Icc => {
            if record.role != IccProfileRole::Output {
                return Err(format!(
                    "Standard ICC target must be an Output/printer profile; selected profile is {}.",
                    record.role.label()
                ));
            }
            let output_space_label = record.color_space_label();
            let output_channel_count = record.color_space_channels();
            validate_output_channels(&output_space_label, output_channel_count)?;
            Ok(ProfileFacts {
                source_space_label: None,
                output_space_label,
                output_channel_count,
            })
        }
        ConversionEngineMode::DeviceLink => {
            if record.role != IccProfileRole::DeviceLink {
                return Err(format!(
                    "DeviceLink mode requires a DeviceLink profile; selected profile is {}.",
                    record.role.label()
                ));
            }
            if !matches!(source_model, ColorModel::Rgb | ColorModel::Cmyk) {
                return Err(format!(
                    "{} source data cannot be used as a production DeviceLink input.",
                    source_model.title()
                ));
            }
            if !record.compatible_with_source_model(source_model) {
                return Err(format!(
                    "DeviceLink input space {} does not match source {}.",
                    record.color_space_label(),
                    source_model.title()
                ));
            }
            let output_space_label = record.pcs_space_label();
            let output_channel_count = record.pcs_space_channels();
            validate_output_channels(&output_space_label, output_channel_count)?;
            Ok(ProfileFacts {
                source_space_label: Some(record.color_space_label()),
                output_space_label,
                output_channel_count,
            })
        }
        ConversionEngineMode::CustomOptimizer => unreachable!("handled before profile inspection"),
    }
}

fn validate_output_channels(
    output_space_label: &str,
    output_channel_count: usize,
) -> Result<(), String> {
    if output_channel_count == 4 && output_space_label.eq_ignore_ascii_case("CMYK") {
        return Ok(());
    }
    if (5..=12).contains(&output_channel_count) {
        return Ok(());
    }
    Err(format!(
        "Production target space {output_space_label} has {output_channel_count} channels; current production transforms support CMYK (4) or N-channel 5..=12 output."
    ))
}

fn target_channel_names(
    profile: &Profile,
    mode: ConversionEngineMode,
    output_space_label: &str,
    channel_count: usize,
) -> (Vec<String>, bool) {
    if output_space_label.eq_ignore_ascii_case("CMYK") && channel_count == 4 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_profile_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shade-production-target-{label}-{}-{}.icc",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn save_profile(profile: &mut Profile, label: &str) -> PathBuf {
        let path = temp_profile_path(label);
        profile.save_profile_to_file(&path).unwrap();
        path
    }

    #[test]
    fn standard_cmyk_topology_is_authoritative() {
        let profile = Profile::new_srgb();
        let (names, authoritative) =
            target_channel_names(&profile, ConversionEngineMode::Icc, "CMYK", 4);
        assert!(authoritative);
        assert_eq!(names, ["Cyan", "Magenta", "Yellow", "Black"]);
    }

    #[test]
    fn missing_nchannel_colorant_table_requires_operator_confirmation() {
        let profile = Profile::new_srgb();
        let (names, authoritative) =
            target_channel_names(&profile, ConversionEngineMode::Icc, "Sig7colorData", 7);
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
        let path = save_profile(&mut Profile::new_srgb(), "display");
        let record = inspect_profile_fresh(&path).unwrap();
        let error = profile_facts(&record, ConversionEngineMode::Icc, ColorModel::Rgb)
            .expect_err("sRGB display profile is not an output target");
        assert!(error.contains("Output/printer"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn real_cmyk_devicelink_declares_direct_input_and_output_topology() {
        let mut profile = Profile::ink_limiting(ColorSpaceSignature::CmykData, 240.0).unwrap();
        let path = save_profile(&mut profile, "devicelink");
        let record = inspect_profile_fresh(&path).unwrap();
        let facts =
            profile_facts(&record, ConversionEngineMode::DeviceLink, ColorModel::Cmyk).unwrap();
        assert_eq!(facts.source_space_label.as_deref(), Some("CMYK"));
        assert_eq!(facts.output_space_label, "CMYK");
        assert_eq!(facts.output_channel_count, 4);

        let error = profile_facts(&record, ConversionEngineMode::DeviceLink, ColorModel::Rgb)
            .expect_err("CMYK DeviceLink must reject RGB source topology");
        assert!(error.contains("does not match"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn unsupported_output_channel_counts_are_rejected() {
        assert!(validate_output_channels("CMYK", 4).is_ok());
        assert!(validate_output_channels("Sig4colorData", 4).is_err());
        assert!(validate_output_channels("Sig7colorData", 7).is_ok());
        assert!(validate_output_channels("RGB", 3).is_err());
        assert!(validate_output_channels("Sig13colorData", 13).is_err());
    }

    #[test]
    fn verification_reuses_registry_fresh_identity_boundary() {
        let path = save_profile(&mut Profile::new_srgb(), "verify");
        let selected = inspect_profile_fresh(&path).unwrap();
        let original = fs::read(&path).unwrap();
        fs::write(&path, vec![0u8; original.len()]).unwrap();
        let error = verify_production_target_profile(
            &path,
            &selected.identity,
            ConversionEngineMode::Icc,
            ColorModel::Rgb,
        )
        .unwrap_err();
        assert!(error.contains("no longer matches") || error.contains("Cannot open ICC profile"));
        let _ = fs::remove_file(path);
    }
}
