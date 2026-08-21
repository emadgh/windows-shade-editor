#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TiffContainerKind {
    ClassicLittleEndian,
    ClassicBigEndian,
    BigTiffLittleEndian,
    BigTiffBigEndian,
}

pub fn classify_tiff_header(header: [u8; 4]) -> Option<TiffContainerKind> {
    match header {
        [b'I', b'I', 42, 0] => Some(TiffContainerKind::ClassicLittleEndian),
        [b'M', b'M', 0, 42] => Some(TiffContainerKind::ClassicBigEndian),
        [b'I', b'I', 43, 0] => Some(TiffContainerKind::BigTiffLittleEndian),
        [b'M', b'M', 0, 43] => Some(TiffContainerKind::BigTiffBigEndian),
        _ => None,
    }
}

/// Checked production-size calculation used by conformance tests without allocating
/// multi-gigabyte image buffers. u128 intentionally keeps >4 GiB layouts exact.
pub fn uncompressed_sample_bytes(
    width: u64,
    height: u64,
    samples_per_pixel: u64,
    bits_per_sample: u64,
) -> Option<u128> {
    if bits_per_sample == 0 || bits_per_sample % 8 != 0 {
        return None;
    }
    u128::from(width)
        .checked_mul(u128::from(height))?
        .checked_mul(u128::from(samples_per_pixel))?
        .checked_mul(u128::from(bits_per_sample / 8))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD_WORKFLOW: &str = include_str!("../.github/workflows/build-windows.yml");
    const COLOR_MANAGEMENT_SRC: &str = include_str!("color_management.rs");
    const PRODUCTION_TARGET_SRC: &str = include_str!("production_target.rs");
    const CONFORMANCE_TESTS: &str = include_str!("tiff_conformance_tests.rs");
    const EXPORT_TESTS: &str = include_str!("export.rs");
    const TIFF_IO_TESTS: &str = include_str!("tiff_io.rs");
    const WORKFLOW_SRC: &str = include_str!("workflow.rs");
    const MAIN_SRC: &str = include_str!("main.rs");

    #[test]
    fn classic_and_bigtiff_headers_are_classified_for_both_byte_orders() {
        assert_eq!(
            classify_tiff_header(*b"II*\0"),
            Some(TiffContainerKind::ClassicLittleEndian)
        );
        assert_eq!(
            classify_tiff_header(*b"MM\0*"),
            Some(TiffContainerKind::ClassicBigEndian)
        );
        assert_eq!(
            classify_tiff_header([b'I', b'I', 43, 0]),
            Some(TiffContainerKind::BigTiffLittleEndian)
        );
        assert_eq!(
            classify_tiff_header([b'M', b'M', 0, 43]),
            Some(TiffContainerKind::BigTiffBigEndian)
        );
    }

    #[test]
    fn greater_than_four_gib_layout_is_reasoned_about_without_allocating_pixels() {
        let bytes = uncompressed_sample_bytes(65_536, 65_536, 6, 16).unwrap();
        assert!(bytes > u128::from(u32::MAX));
        assert_eq!(bytes, 51_539_607_552u128);
    }

    #[test]
    fn production_fixture_matrix_remains_in_the_test_suite() {
        for required in [
            "identity_export_defaults_to_lzw_for_supported_lossless_sources",
            "identity_export_can_preserve_supported_lossless_compressions_when_lzw_is_disabled",
            "identity_export_preserves_horizontal_predictor_for_base_rgb",
            "identity_export_preserves_16bit_cmyk_samples",
            "identity_export_preserves_spot_names_icc_photoshop_resources_and_dpi",
        ] {
            assert!(
                CONFORMANCE_TESTS.contains(required),
                "missing required TIFF conformance test: {required}"
            );
        }
        for required in [
            "streaming_identity_export_preserves_six_channels",
            "large_layout_selects_bigtiff_without_allocating_pixels",
            "identity_export_preserves_bigtiff_container",
        ] {
            assert!(
                EXPORT_TESTS.contains(required),
                "missing required export transport test: {required}"
            );
        }
        for required in [
            "region_stream_compacts_edge_tiles_without_full_decode",
            "region_stream_interleaves_planar_strips_without_full_decode",
        ] {
            assert!(
                TIFF_IO_TESTS.contains(required),
                "missing required tiled/planar streaming coverage: {required}"
            );
        }
    }

    #[test]
    fn color_management_uses_shared_icc_registry_as_profile_authority() {
        for required in [
            "IccProfileRegistry",
            "IccProfileRegistry.inspect(path)",
            "IccProfileRegistry.verify_identity",
            ".installed()?",
            "inspect_registry_profile_fresh",
        ] {
            assert!(
                COLOR_MANAGEMENT_SRC.contains(required),
                "Color Management lost shared ICC registry integration: {required}"
            );
        }
        for forbidden in [
            "EnumColorProfilesW",
            "PROFILE_INSPECTION_CACHE",
            "CachedProfileInspection",
            "registered_profile_names",
            "fn color_directory()",
        ] {
            assert!(
                !COLOR_MANAGEMENT_SRC.contains(forbidden),
                "Color Management reintroduced private ICC registry infrastructure: {forbidden}"
            );
        }
    }

    #[test]
    fn production_target_uses_shared_icc_registry_as_identity_and_role_authority() {
        for required in [
            "inspect_profile_fresh(path)?",
            "IccProfileRegistry.verify_identity",
            "IccProfileRole::Output",
            "IccProfileRole::DeviceLink",
            "record.compatible_with_source_model",
            "record.pcs_space_channels()",
        ] {
            assert!(
                PRODUCTION_TARGET_SRC.contains(required),
                "Production target lost shared ICC registry integration: {required}"
            );
        }
        for forbidden in [
            "Sha256::digest",
            "fn profile_description(",
            "fn profile_class_label(",
            "let bytes = fs::read(path)",
        ] {
            assert!(
                !PRODUCTION_TARGET_SRC.contains(forbidden),
                "Production target reintroduced private ICC identity/role inspection: {forbidden}"
            );
        }
    }

    #[test]
    fn missing_face_relink_safety_paths_remain_wired() {
        for required in [
            "placeholder_loaded_face",
            "verify_relink_metadata",
            "load_relink_candidate",
            "find_named_file_recursive",
            "The active Face source image is missing",
            "Export all requires every Accepted Face source image to be available",
            "TIFF-source-only in this version",
            "TIFF round-trip validation requires a TIFF source Face",
            "status.is_rejected()",
            "excluded {rejected_count} Rejected Face(s)",
        ] {
            assert!(
                WORKFLOW_SRC.contains(required)
                    || EXPORT_TESTS.contains(required)
                    || MAIN_SRC.contains(required),
                "missing required missing-Face/relink guard: {required}"
            );
        }
    }

    #[test]
    fn standard_windows_ci_keeps_shell_and_schema_validation_required() {
        for required in [
            "cargo check --locked --target x86_64-pc-windows-msvc",
            "cargo test --locked --target x86_64-pc-windows-msvc",
            "cargo build --release --locked --target x86_64-pc-windows-msvc",
            "Build and test native Shell extension",
            "Validate Shell property schema XML",
            "actions/upload-artifact@v4",
        ] {
            assert!(
                BUILD_WORKFLOW.contains(required),
                "production Windows CI lost required step: {required}"
            );
        }
    }
}
