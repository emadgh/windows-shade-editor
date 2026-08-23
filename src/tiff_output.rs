use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::safe_fs;

/// Conservative ceiling for classic TIFF pixel payloads. The remaining space
/// is reserved for strip tables, metadata, ICC/Photoshop resources and encoder
/// overhead below the 4 GiB offset limit.
pub const CLASSIC_TIFF_SAFE_RAW_BYTES: u64 = 4_000_000_000;

static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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
    let file_name = destination
        .file_name()
        .map(|value| value.to_os_string())
        .unwrap_or_else(|| "output.tif".into());
    let mut staged_name = file_name;
    staged_name.push(suffix);
    destination.with_file_name(staged_name)
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
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|err| {
        format!(
            "Cannot create TIFF output folder {}: {err}",
            parent.display()
        )
    })?;
    let staged = unique_staged_path(destination, staging_suffix);

    let result = (|| {
        write_staged(&staged)?;
        verify_staged(&staged)?;
        match policy {
            DestinationPolicy::ReplaceExisting => safe_fs::commit_staged_file(&staged, destination),
            DestinationPolicy::RequireAbsent => {
                safe_fs::commit_staged_file_if_absent(&staged, destination)
            }
        }
    })();
    if result.is_err() && staged.exists() {
        let _ = fs::remove_file(&staged);
    }
    result
}

fn unique_staged_path(destination: &Path, suffix: &str) -> PathBuf {
    let file_name = destination
        .file_name()
        .map(|value| value.to_os_string())
        .unwrap_or_else(|| "output.tif".into());
    let process = std::process::id();
    for _ in 0..64 {
        let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut staged_name = file_name.clone();
        staged_name.push(format!("{suffix}.{process}-{sequence}"));
        let candidate = destination.with_file_name(staged_name);
        if !candidate.exists() {
            return candidate;
        }
    }
    // The sequence space is process-local and the loop above is defensive;
    // retaining a deterministic fallback keeps the error at file creation
    // rather than panicking on an exhausted name search.
    let mut staged_name = file_name;
    staged_name.push(format!(
        "{suffix}.{}",
        STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    destination.with_file_name(staged_name)
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
            ".stage.tmp",
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
    fn successful_write_replaces_destination() {
        let folder = temp_folder("success");
        fs::create_dir_all(&folder).unwrap();
        let destination = folder.join("output.tif");
        fs::write(&destination, b"previous").unwrap();

        write_atomic(
            &destination,
            ".stage.tmp",
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
            ".stage.tmp",
            DestinationPolicy::RequireAbsent,
            |staged| fs::write(staged, b"new").map_err(|err| err.to_string()),
            |_| Ok(()),
        )
        .unwrap_err();

        assert!(error.contains("exists") || error.contains("created"));
        assert_eq!(fs::read(&destination).unwrap(), b"previous");
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
        let _ = fs::remove_dir_all(folder);
    }
}
