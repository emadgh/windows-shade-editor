use std::path::Path;

use crate::color_conversion::ConversionEngineMode;
use crate::icc_profile_registry::{IccProfileRecord, IccProfileRegistry, IccProfileRole};
use crate::model::IccProfileIdentity;
use crate::tiff_io::ColorModel;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProductionProfileRejection {
    WrongRole,
    UnsupportedSource,
    UnsupportedOutputTopology,
}

impl ProductionProfileRejection {
    pub fn label(self) -> &'static str {
        match self {
            Self::WrongRole => "profile role does not match the selected conversion engine",
            Self::UnsupportedSource => "profile input space does not match the active source",
            Self::UnsupportedOutputTopology => {
                "profile output topology is outside supported CMYK/N-channel production output"
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProductionProfileCandidate {
    pub profile: IccProfileRecord,
    pub rejection: Option<ProductionProfileRejection>,
}

impl ProductionProfileCandidate {
    pub fn selectable(&self) -> bool {
        self.rejection.is_none()
    }
}

/// Installed production-profile catalog used by Converter UI.
///
/// Discovery, identity, hashing, cache and changed-profile detection all come from the
/// canonical `IccProfileRegistry`. This layer adds only production-engine eligibility.
pub fn installed_production_profiles(
    registry: IccProfileRegistry,
    mode: ConversionEngineMode,
    source_model: ColorModel,
    query: &str,
    include_incompatible: bool,
) -> Result<Vec<ProductionProfileCandidate>, String> {
    let mut rows = registry
        .installed()?
        .into_iter()
        .filter(|profile| profile.matches_query(query))
        .map(|profile| {
            let rejection = production_profile_rejection(&profile, mode, source_model);
            ProductionProfileCandidate { profile, rejection }
        })
        .filter(|candidate| include_incompatible || candidate.selectable())
        .collect::<Vec<_>>();

    rows.sort_by(|left, right| {
        left.profile
            .description
            .to_lowercase()
            .cmp(&right.profile.description.to_lowercase())
            .then_with(|| {
                left.profile
                    .filename()
                    .to_lowercase()
                    .cmp(&right.profile.filename().to_lowercase())
            })
    });
    Ok(rows)
}

/// Explicit file-picker path for Converter. The file is inspected by the same registry as
/// installed-profile browsing, then production eligibility is applied on top.
pub fn inspect_production_profile_candidate(
    registry: IccProfileRegistry,
    path: &Path,
    mode: ConversionEngineMode,
    source_model: ColorModel,
) -> Result<IccProfileRecord, String> {
    let profile = registry.inspect(path)?;
    ensure_eligible(profile, mode, source_model)
}

/// Reuse boundary for a previously selected profile. This must not trust browse-cache facts:
/// the registry re-reads and re-hashes the bytes first, then production eligibility is checked.
pub fn verify_production_profile_candidate(
    registry: IccProfileRegistry,
    path: &Path,
    expected_identity: &IccProfileIdentity,
    mode: ConversionEngineMode,
    source_model: ColorModel,
) -> Result<IccProfileRecord, String> {
    let profile = registry.verify_identity(path, expected_identity)?;
    ensure_eligible(profile, mode, source_model)
}

fn ensure_eligible(
    profile: IccProfileRecord,
    mode: ConversionEngineMode,
    source_model: ColorModel,
) -> Result<IccProfileRecord, String> {
    if let Some(rejection) = production_profile_rejection(&profile, mode, source_model) {
        return Err(format!(
            "Production profile '{}' cannot be used: {}.",
            profile.description,
            rejection.label()
        ));
    }
    Ok(profile)
}

pub fn production_profile_rejection(
    profile: &IccProfileRecord,
    mode: ConversionEngineMode,
    source_model: ColorModel,
) -> Option<ProductionProfileRejection> {
    match mode {
        ConversionEngineMode::Icc => {
            if profile.role != IccProfileRole::Output {
                return Some(ProductionProfileRejection::WrongRole);
            }
            if !supported_output_topology(
                profile.color_space_label().as_str(),
                profile.color_space_channels(),
            ) {
                return Some(ProductionProfileRejection::UnsupportedOutputTopology);
            }
            None
        }
        ConversionEngineMode::DeviceLink => {
            if profile.role != IccProfileRole::DeviceLink {
                return Some(ProductionProfileRejection::WrongRole);
            }
            if !matches!(source_model, ColorModel::Rgb | ColorModel::Cmyk)
                || !profile.compatible_with_source_model(source_model)
            {
                return Some(ProductionProfileRejection::UnsupportedSource);
            }
            if !supported_output_topology(
                profile.pcs_space_label().as_str(),
                profile.pcs_space_channels(),
            ) {
                return Some(ProductionProfileRejection::UnsupportedOutputTopology);
            }
            None
        }
        ConversionEngineMode::CustomOptimizer => Some(ProductionProfileRejection::WrongRole),
    }
}

fn supported_output_topology(space_label: &str, channels: usize) -> bool {
    (channels == 4 && space_label.eq_ignore_ascii_case("CMYK")) || (5..=12).contains(&channels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lcms2::{ColorSpaceSignature, Profile};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shade-production-profile-{label}-{}-{}.icc",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn display_profile_is_rejected_for_standard_output_mode() {
        let path = temp_path("display");
        let mut profile = Profile::new_srgb();
        profile.save_profile_to_file(&path).unwrap();
        let record = IccProfileRegistry.inspect(&path).unwrap();
        assert_eq!(
            production_profile_rejection(&record, ConversionEngineMode::Icc, ColorModel::Rgb),
            Some(ProductionProfileRejection::WrongRole)
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn cmyk_devicelink_is_selectable_only_for_matching_cmyk_source() {
        let path = temp_path("devicelink");
        let mut profile = Profile::ink_limiting(ColorSpaceSignature::CmykData, 240.0).unwrap();
        profile.save_profile_to_file(&path).unwrap();
        let record = IccProfileRegistry.inspect(&path).unwrap();
        assert_eq!(record.role, IccProfileRole::DeviceLink);
        assert_eq!(
            production_profile_rejection(
                &record,
                ConversionEngineMode::DeviceLink,
                ColorModel::Cmyk,
            ),
            None
        );
        assert_eq!(
            production_profile_rejection(
                &record,
                ConversionEngineMode::DeviceLink,
                ColorModel::Rgb,
            ),
            Some(ProductionProfileRejection::UnsupportedSource)
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn explicit_selection_uses_registry_identity_and_same_eligibility_rules() {
        let path = temp_path("explicit");
        let mut profile = Profile::new_srgb();
        profile.save_profile_to_file(&path).unwrap();
        let error = inspect_production_profile_candidate(
            IccProfileRegistry,
            &path,
            ConversionEngineMode::Icc,
            ColorModel::Rgb,
        )
        .unwrap_err();
        assert!(error.contains("profile role"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn saved_selection_is_freshly_rehashed_before_reuse() {
        let path = temp_path("verify");
        let mut profile = Profile::ink_limiting(ColorSpaceSignature::CmykData, 240.0).unwrap();
        profile.save_profile_to_file(&path).unwrap();
        let selected = inspect_production_profile_candidate(
            IccProfileRegistry,
            &path,
            ConversionEngineMode::DeviceLink,
            ColorModel::Cmyk,
        )
        .unwrap();
        assert!(
            verify_production_profile_candidate(
                IccProfileRegistry,
                &path,
                &selected.identity,
                ConversionEngineMode::DeviceLink,
                ColorModel::Cmyk,
            )
            .is_ok()
        );

        let mut replacement = Profile::new_srgb();
        replacement.save_profile_to_file(&path).unwrap();
        let error = verify_production_profile_candidate(
            IccProfileRegistry,
            &path,
            &selected.identity,
            ConversionEngineMode::DeviceLink,
            ColorModel::Cmyk,
        )
        .unwrap_err();
        assert!(error.contains("no longer matches"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn custom_optimizer_never_accepts_icc_catalog_entries() {
        let path = temp_path("custom");
        let mut profile = Profile::new_srgb();
        profile.save_profile_to_file(&path).unwrap();
        let record = IccProfileRegistry.inspect(&path).unwrap();
        assert_eq!(
            production_profile_rejection(
                &record,
                ConversionEngineMode::CustomOptimizer,
                ColorModel::Rgb,
            ),
            Some(ProductionProfileRejection::WrongRole)
        );
        let _ = fs::remove_file(path);
    }
}
