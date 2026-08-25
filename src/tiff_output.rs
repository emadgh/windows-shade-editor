use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

use crate::safe_fs::{self, staging};

/// Conservative ceiling for classic TIFF pixel payloads. The remaining space
/// is reserved for strip tables, metadata, ICC/Photoshop resources and encoder
/// overhead below the 4 GiB offset limit.
pub const CLASSIC_TIFF_SAFE_RAW_BYTES: u64 = 4_000_000_000;

static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[allow(dead_code)]
const MAX_PROCESS_ID_DIGITS: usize = 10;
#[allow(dead_code)]
const MAX_STAGE_SEQUENCE_DIGITS: usize = 20;
const MAX_WINDOWS_COMPONENT_UTF16: usize = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DestinationPolicy {
    ReplaceExisting,
    RequireAbsent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TiffLayout {
    pub width: u32,
    pub height: u32,
    pub channels: usize,
    pub bit_depth: u8,
}

pub fn canonical_destination(path: &Path) -> PathBuf {
    path.with_extension("tif")
}

pub fn staged_path(destination: &Path, suffix: &str) -> PathBuf {
    let suffix = staging::canonical_tiff_suffix(suffix);
    let file_name = destination
        .file_name()
        .map(|value| value.to_os_string())
        .unwrap_or_else(|| "output.tif".into());
    let mut staged_name = file_name;
    staged_name.push(suffix);
    destination.with_file_name(staged_name)
}

/// Maximum UTF-16 code units appended to a destination file name by the normal
/// unique staging convention: `{suffix}.{process_id}-{sequence}`.
///
/// This remains available for diagnostics and compatibility tests. Runtime path
/// validation must not require the descriptive form to fit because the writer
/// has a compact same-directory fallback for long destination components.
#[allow(dead_code)]
pub fn staging_suffix_utf16_reserve(suffix: &str) -> usize {
    let suffix = staging::canonical_tiff_suffix(suffix);
    suffix.encode_utf16().count() + 1 + MAX_PROCESS_ID_DIGITS + 1 + MAX_STAGE_SEQUENCE_DIGITS
}

pub fn layout_requires_bigtiff(layout: TiffLayout) -> bool {
    raw_image_bytes(layout)
        .map(|bytes| bytes >= CLASSIC_TIFF_SAFE_RAW_BYTES)
        .unwrap_or(true)
}

pub fn raw_image_bytes(layout: TiffLayout) -> Option<u64> {
    if layout.bit_depth == 0 {
        return None;
    }
    let bytes_per_sample = u64::from(layout.bit_depth).div_ceil(8);
    u64::from(layout.width)
        .checked_mul(u64::from(layout.height))?
        .checked_mul(layout.channels as u64)?
        .checked_mul(bytes_per_sample)
}

pub fn source_is_bigtiff(source: &Path) -> Result<bool, String> {
    use std::io::Read;

    let mut file = fs::File::open(source)
        .map_err(|err| format!("Cannot inspect source TIFF header: {err}"))?;
    let mut header = [0u8; 4];
    file.read_exact(&mut header)
        .map_err(|err| format!("Cannot read source TIFF header: {err}"))?;
    match header {
        [b'I', b'I', 43, 0] | [b'M', b'M', 0, 43] => Ok(true),
        [b'I', b'I', 42, 0] | [b'M', b'M', 0, 42] => Ok(false),
        _ => Err("Source does not have a valid TIFF/BigTIFF header.".to_owned()),
    }
}

pub fn preserve_source_or_layout_requires_bigtiff(
    source: &Path,
    layout: TiffLayout,
) -> Result<bool, String> {
    Ok(source_is_bigtiff(source)? || layout_requires_bigtiff(layout))
}

/// Stage, validate and atomically publish one TIFF. The staged file is always
/// a unique sibling of the final destination, while large render spools stay
/// under their caller-owned local spool directory.
///
/// Known production suffix strings are normalized through the shared staging
/// registry. If appending the descriptive suffix/process/sequence would exceed
/// the normal Windows 255 UTF-16 component limit, the writer automatically uses
/// a compact process/sequence sibling name instead. The fallback remains beside
/// the final destination, preserving the same-volume atomic-commit invariant.
pub fn write_atomic<F, V>(
    destination: &Path,
    staging_suffix: &str,
    policy: DestinationPolicy,
    write_staged: F,
    verify_staged: V,
) -> Result<(), String>
where
    F: FnOnce(&Path) -> Result<(), String>,
    V: FnOnce(&Path) -> Result<(), String>,
{
    write_atomic_with_precommit(
        destination,
        staging_suffix,
        policy,
        write_staged,
        verify_staged,
        || Ok(()),
    )
}

/// Variant of [`write_atomic`] with a final caller-owned gate that executes
/// after the expensive staged write + validation and immediately before the
/// irreversible publication boundary.
///
/// Conversion uses this to honor cancellation through encode/verification
/// without teaching the generic TIFF storage layer about conversion state. A
/// rejected gate leaves the existing destination untouched and the staged file
/// is removed by `StagedOutput::drop`.
pub fn write_atomic_with_precommit<F, V, C>(
    destination: &Path,
    staging_suffix: &str,
    policy: DestinationPolicy,
    write_staged: F,
    verify_staged: V,
    before_commit: C,
) -> Result<(), String>
where
    F: FnOnce(&Path) -> Result<(), String>,
    V: FnOnce(&Path) -> Result<(), String>,
    C: FnOnce() -> Result<(), String>,
{
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|err| {
        format!(
            "Cannot create TIFF output folder {}: {err}",
            parent.display()
        )
    })?;
    let canonical_suffix = staging::canonical_tiff_suffix(staging_suffix);
    let staged = StagedOutput::new(unique_staged_path(destination, canonical_suffix));

    (|| {
        write_staged(staged.path())?;
        verify_staged(staged.path())?;
        before_commit()?;
        match policy {
            DestinationPolicy::ReplaceExisting => {
                safe_fs::commit_staged_file(staged.path(), destination)
            }
            DestinationPolicy::RequireAbsent => {
                safe_fs::commit_staged_file_if_absent(staged.path(), destination)
            }
        }
    })()
}

struct StagedOutput {
    path: PathBuf,
}

impl StagedOutput {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagedOutput {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn unique_staged_path(destination: &Path, suffix: &str) -> PathBuf {
    let suffix = staging::canonical_tiff_suffix(suffix);
    let file_name = destination
        .file_name()
        .map(|value| value.to_os_string())
        .unwrap_or_else(|| "output.tif".into());
    let process = std::process::id();

    for _ in 0..64 {
        let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);

        let mut descriptive_name = file_name.clone();
        descriptive_name.push(format!("{suffix}.{process}-{sequence}"));
        let descriptive = destination.with_file_name(descriptive_name);
        if component_fits_windows_limit(&descriptive) && !descriptive.exists() {
            return descriptive;
        }

        // A destination component can itself be valid while leaving no room
        // for a descriptive suffix. Keep the stage on the same volume but use
        // a compact unique filename independent of the destination stem.
        let compact = destination.with_file_name(compact_stage_name(process, sequence, suffix));
        if component_fits_windows_limit(&compact) && !compact.exists() {
            return compact;
        }
    }

    // The process-local sequence makes exhaustion effectively impossible. Keep
    // a compact deterministic fallback so pathological collision storms fail at
    // file creation rather than panicking or constructing an oversized name.
    let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    destination.with_file_name(compact_stage_name(process, sequence, suffix))
}

fn compact_stage_name(process: u32, sequence: u64, suffix: &str) -> String {
    let preferred = format!("shade-stage-{process}-{sequence}{suffix}");
    if preferred.encode_utf16().count() <= MAX_WINDOWS_COMPONENT_UTF16 {
        preferred
    } else {
        format!("shade-stage-{process}-{sequence}{}", staging::SAFE_FS_TEMP_SUFFIX)
    }
}

fn component_fits_windows_limit(path: &Path) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    component_utf16_len(name) <= MAX_WINDOWS_COMPONENT_UTF16
}

#[cfg(windows)]
fn component_utf16_len(value: &std::ffi::OsStr) -> usize {
    value.encode_wide().count()
}

#[cfg(not(windows))]
fn component_utf16_len(value: &std::ffi::OsStr) -> usize {
    value.to_string_lossy().encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_folder(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shade-tiff-output-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn canonical_destination_always_uses_tif() {
        assert_eq!(
            canonical_destination(Path::new(r"C:\Output\face.TIFF")),
            PathBuf::from(r"C:\Output\face.tif")
        );
        assert_eq!(
            canonical_destination(Path::new(r"C:\Output\face")),
            PathBuf::from(r"C:\Output\face.tif")
        );
    }

    #[test]
    fn staging_reserve_covers_the_longest_unique_suffix() {
        let suffix = staging::CONVERSION_STAGED_SUFFIX;
        let staged_suffix = format!("{suffix}.{}-{}", u32::MAX, u64::MAX);
        assert_eq!(
            staging_suffix_utf16_reserve(suffix),
            staged_suffix.encode_utf16().count()
        );
    }

    #[test]
    fn long_destination_component_uses_compact_sibling_stage() {
        let destination = PathBuf::from(r"C:\Output")
            .join(format!("{}.tif", "x".repeat(245)));
        assert!(component_utf16_len(destination.file_name().unwrap()) <= MAX_WINDOWS_COMPONENT_UTF16);

        let staged = unique_staged_path(&destination, staging::CONVERSION_STAGED_SUFFIX);

        assert_eq!(staged.parent(), destination.parent());
        assert!(component_utf16_len(staged.file_name().unwrap()) <= MAX_WINDOWS_COMPONENT_UTF16);
        assert!(staged
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("shade-stage-"));
        assert!(!staged
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(&"x".repeat(32)));
    }

    #[test]
    fn bigtiff_policy_is_conservative_on_overflow() {
        assert!(!layout_requires_bigtiff(TiffLayout {
            width: 1,
            height: 1,
            channels: 4,
            bit_depth: 16,
        }));
        assert!(layout_requires_bigtiff(TiffLayout {
            width: u32::MAX,
            height: u32::MAX,
            channels: usize::MAX,
            bit_depth: 16,
        }));
    }

    #[test]
    fn failed_validation_preserves_destination_and_cleans_stage() {
        let folder = temp_folder("validation-failure");
        fs::create_dir_all(&folder).unwrap();
        let destination = folder.join("output.tif");
        fs::write(&destination, b"previous").unwrap();

        let error = write_atomic(
            &destination,
            staging::EXPORT_TEMP_SUFFIX,
            DestinationPolicy::ReplaceExisting,
            |staged| fs::write(staged, b"partial").map_err(|err| err.to_string()),
            |_| Err("verification failed".to_owned()),
        )
        .unwrap_err();

        assert!(error.contains("verification failed"));
        assert_eq!(fs::read(&destination).unwrap(), b"previous");
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn precommit_rejection_preserves_destination_and_cleans_verified_stage() {
        let folder = temp_folder("precommit-rejection");
        fs::create_dir_all(&folder).unwrap();
        let destination = folder.join("output.tif");
        fs::write(&destination, b"previous").unwrap();
        let mut verified = false;

        let error = write_atomic_with_precommit(
            &destination,
            staging::CONVERSION_STAGED_SUFFIX,
            DestinationPolicy::ReplaceExisting,
            |staged| fs::write(staged, b"complete").map_err(|err| err.to_string()),
            |staged| {
                verified = fs::read(staged).map_err(|err| err.to_string())? == b"complete";
                verified.then_some(()).ok_or_else(|| "verification failed".to_owned())
            },
            || Err("cancelled immediately before publication".to_owned()),
        )
        .unwrap_err();

        assert!(verified, "pre-commit gate must run after staged verification");
        assert!(error.contains("cancelled immediately before publication"));
        assert_eq!(fs::read(&destination).unwrap(), b"previous");
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn failed_write_preserves_destination_and_cleans_stage() {
        let folder = temp_folder("write-failure");
        fs::create_dir_all(&folder).unwrap();
        let destination = folder.join("output.tif");
        fs::write(&destination, b"previous").unwrap();

        let error = write_atomic(
            &destination,
            staging::TEST_STACK_STAGED_SUFFIX,
            DestinationPolicy::ReplaceExisting,
            |staged| {
                fs::write(staged, b"partial").unwrap();
                Err("writer cancelled".to_owned())
            },
            |_| Ok(()),
        )
        .unwrap_err();

        assert!(error.contains("cancelled"));
        assert_eq!(fs::read(&destination).unwrap(), b"previous");
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn writer_panic_preserves_destination_and_cleans_stage() {
        let folder = temp_folder("writer-panic");
        fs::create_dir_all(&folder).unwrap();
        let destination = folder.join("output.tif");
        fs::write(&destination, b"previous").unwrap();

        let result = std::panic::catch_unwind(|| {
            let _ = write_atomic(
                &destination,
                staging::EXPORT_TEMP_SUFFIX,
                DestinationPolicy::ReplaceExisting,
                |staged| {
                    fs::write(staged, b"partial").unwrap();
                    panic!("simulated writer panic");
                },
                |_| Ok(()),
            );
        });

        assert!(result.is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"previous");
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn successful_write_replaces_destination() {
        let folder = temp_folder("success");
        fs::create_dir_all(&folder).unwrap();
        let destination = folder.join("output.tif");
        fs::write(&destination, b"previous").unwrap();

        write_atomic(
            &destination,
            staging::EXPORT_TEMP_SUFFIX,
            DestinationPolicy::ReplaceExisting,
            |staged| fs::write(staged, b"complete").map_err(|err| err.to_string()),
            |staged| {
                (fs::read(staged).map_err(|err| err.to_string())? == b"complete")
                    .then_some(())
                    .ok_or_else(|| "verification failed".to_owned())
            },
        )
        .unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"complete");
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn require_absent_never_replaces_existing_destination() {
        let folder = temp_folder("require-absent");
        fs::create_dir_all(&folder).unwrap();
        let destination = folder.join("output.tif");
        fs::write(&destination, b"previous").unwrap();

        let error = write_atomic(
            &destination,
            staging::CONVERSION_STAGED_SUFFIX,
            DestinationPolicy::RequireAbsent,
            |staged| fs::write(staged, b"new").map_err(|err| err.to_string()),
            |_| Ok(()),
        )
        .unwrap_err();

        assert!(error.contains("commit new destination") || error.contains("exists") || error.contains("created"));
        assert_eq!(fs::read(&destination).unwrap(), b"previous");
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
        let _ = fs::remove_dir_all(folder);
    }
}
