//! Canonical temporary/staging suffixes used by Shade Editor storage pipelines.
//!
//! Keep transient-file naming here so Export, Test Stack, Converter and low-level
//! atomic persistence do not silently drift to incompatible cleanup conventions.

/// Fully rendered conversion TIFF staged beside its final destination.
pub const CONVERSION_STAGED_SUFFIX: &str = ".conversion.tmp";
/// Test Stack TIFF staged beside its final destination.
pub const TEST_STACK_STAGED_SUFFIX: &str = ".test-stack.tmp";
/// Normal Face export TIFF staged beside its final destination.
pub const EXPORT_TEMP_SUFFIX: &str = ".export.tmp";
/// Local disk-backed Export processing spool suffix. Spools are not committed directly.
pub const EXPORT_SPOOL_SUFFIX: &str = ".spool.tmp";
/// Generic low-level atomic-write sibling suffix used by `safe_fs`.
pub const SAFE_FS_TEMP_SUFFIX: &str = ".tmp";
/// Backup suffix used by project/settings persistence.
pub const BACKUP_SUFFIX: &str = ".bak";

/// Canonical transient suffixes that may identify incomplete work.
pub const ALL_STAGING_SUFFIXES: [&str; 5] = [
    CONVERSION_STAGED_SUFFIX,
    TEST_STACK_STAGED_SUFFIX,
    EXPORT_TEMP_SUFFIX,
    EXPORT_SPOOL_SUFFIX,
    SAFE_FS_TEMP_SUFFIX,
];

/// Normalize legacy/string API input at the shared TIFF writer boundary.
///
/// `tiff_output::write_atomic` intentionally keeps accepting `&str` so external
/// and older internal callers do not need a breaking API migration. Known
/// production suffix values are nevertheless resolved through this registry,
/// making these constants the canonical source of truth.
pub fn canonical_tiff_suffix(requested: &str) -> &str {
    match requested {
        ".conversion.tmp" => CONVERSION_STAGED_SUFFIX,
        ".test-stack.tmp" => TEST_STACK_STAGED_SUFFIX,
        ".export.tmp" => EXPORT_TEMP_SUFFIX,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn staging_suffixes_are_non_empty_distinct_and_temporary() {
        let mut unique = BTreeSet::new();
        for suffix in ALL_STAGING_SUFFIXES {
            assert!(!suffix.is_empty());
            assert!(suffix.starts_with('.'));
            assert!(suffix.ends_with("tmp"));
            assert!(unique.insert(suffix), "duplicate staging suffix: {suffix}");
        }
    }

    #[test]
    fn legacy_tiff_suffix_values_resolve_to_canonical_constants() {
        assert!(std::ptr::eq(
            canonical_tiff_suffix(".conversion.tmp"),
            CONVERSION_STAGED_SUFFIX
        ));
        assert!(std::ptr::eq(
            canonical_tiff_suffix(".test-stack.tmp"),
            TEST_STACK_STAGED_SUFFIX
        ));
        assert!(std::ptr::eq(
            canonical_tiff_suffix(".export.tmp"),
            EXPORT_TEMP_SUFFIX
        ));
        assert_eq!(canonical_tiff_suffix(".custom.tmp"), ".custom.tmp");
    }
}
