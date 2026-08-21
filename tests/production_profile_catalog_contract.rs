#![cfg(windows)]

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use lcms2::{ColorSpaceSignature, Profile};
use windows_shade_editor::color_conversion::ConversionEngineMode;
use windows_shade_editor::icc_profile_registry::IccProfileRegistry;
use windows_shade_editor::production_profile_catalog::inspect_production_profile_candidate;
use windows_shade_editor::production_target::inspect_production_target_profile;
use windows_shade_editor::tiff_io::ColorModel;

fn temp_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "shade-production-profile-contract-{label}-{}-{}.icc",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn display_profile_is_rejected_by_catalog_and_target_validator() {
    let path = temp_path("display");
    let mut profile = Profile::new_srgb();
    profile.save_profile_to_file(&path).unwrap();

    let catalog_error = inspect_production_profile_candidate(
        IccProfileRegistry,
        &path,
        ConversionEngineMode::Icc,
        ColorModel::Rgb,
    )
    .unwrap_err();
    let target_error =
        inspect_production_target_profile(&path, ConversionEngineMode::Icc, ColorModel::Rgb)
            .unwrap_err();

    assert!(catalog_error.contains("profile role"));
    assert!(target_error.contains("Output/printer"));
    let _ = fs::remove_file(path);
}

#[test]
fn cmyk_devicelink_has_same_identity_and_topology_across_catalog_and_target_validator() {
    let path = temp_path("devicelink");
    let mut profile = Profile::ink_limiting(ColorSpaceSignature::CmykData, 240.0).unwrap();
    profile.save_profile_to_file(&path).unwrap();

    let catalog = inspect_production_profile_candidate(
        IccProfileRegistry,
        &path,
        ConversionEngineMode::DeviceLink,
        ColorModel::Cmyk,
    )
    .unwrap();
    let target = inspect_production_target_profile(
        &path,
        ConversionEngineMode::DeviceLink,
        ColorModel::Cmyk,
    )
    .unwrap();

    assert_eq!(catalog.identity, target.identity);
    assert_eq!(catalog.color_space_label(), "CMYK");
    assert_eq!(catalog.pcs_space_label(), "CMYK");
    assert_eq!(target.source_space_label.as_deref(), Some("CMYK"));
    assert_eq!(target.output_space_label, "CMYK");
    assert_eq!(target.output_channel_count, 4);
    let _ = fs::remove_file(path);
}
