from pathlib import Path
import re

ROOT = Path(".")
def read(path):
    return (ROOT / path).read_text(encoding="utf-8")
def write(path, text):
    (ROOT / path).write_text(text, encoding="utf-8", newline="\n")
def replace_once(text, old, new, label):
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected 1 match, found {count}")
    return text.replace(old, new, 1)
def regex_once(text, pattern, replacement, label):
    new, count = re.subn(pattern, replacement, text, count=1, flags=re.S)
    if count != 1:
        raise RuntimeError(f"{label}: expected 1 regex match, found {count}")
    return new

cargo = read("Cargo.toml")
cargo = replace_once(cargo, 'version = "0.10.3"', 'version = "0.11.0"', "Cargo version")
write("Cargo.toml", cargo)
lock = read("Cargo.lock")
lock = replace_once(lock, 'name = "windows-shade-editor"\nversion = "0.10.3"', 'name = "windows-shade-editor"\nversion = "0.11.0"', "Cargo.lock version")
write("Cargo.lock", lock)

recovery = r'''use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::ShadeProject;

const RECOVERY_FORMAT_VERSION: u32 = 1;
const RECOVERY_STATE_COUNT: usize = 3;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoveryFile {
    pub format_version: u32,
    pub saved_at_unix_ms: i64,
    pub origin_project_path: Option<String>,
    pub face_paths: Vec<String>,
    pub project: ShadeProject,
}

impl RecoveryFile {
    pub fn new(
        project: ShadeProject,
        face_paths: Vec<PathBuf>,
        origin_project_path: Option<PathBuf>,
    ) -> Self {
        Self {
            format_version: RECOVERY_FORMAT_VERSION,
            saved_at_unix_ms: unix_ms_now(),
            origin_project_path: origin_project_path
                .map(|path| path.to_string_lossy().into_owned()),
            face_paths: face_paths
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            project,
        }
    }

    pub fn origin_path(&self) -> Option<PathBuf> {
        self.origin_project_path.as_deref().map(PathBuf::from)
    }

    pub fn resolved_face_paths(&self) -> Vec<PathBuf> {
        self.face_paths.iter().map(PathBuf::from).collect()
    }
}

pub fn recovery_path() -> PathBuf {
    recovery_paths()[0].clone()
}

fn recovery_paths() -> [PathBuf; RECOVERY_STATE_COUNT] {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ShadeEditor");
    [
        base.join("recovery-v9.json"),
        base.join("recovery-v9-1.json"),
        base.join("recovery-v9-2.json"),
    ]
}

pub fn load() -> Result<Option<RecoveryFile>, String> {
    load_from_paths(&recovery_paths())
}

fn load_from_paths(paths: &[PathBuf]) -> Result<Option<RecoveryFile>, String> {
    let mut errors = Vec::new();
    for path in paths {
        if !path.exists() {
            continue;
        }
        match read_recovery(path) {
            Ok(recovery) => return Ok(Some(recovery)),
            Err(err) => errors.push(err),
        }
    }
    if errors.is_empty() {
        Ok(None)
    } else {
        Err(format!(
            "No valid recovery state was found. {}",
            errors.join(" | ")
        ))
    }
}

fn read_recovery(path: &Path) -> Result<RecoveryFile, String> {
    let text = fs::read_to_string(path)
        .map_err(|err| format!("Cannot read recovery file {}: {err}", path.display()))?;
    let recovery: RecoveryFile = serde_json::from_str(&text)
        .map_err(|err| format!("Invalid recovery file {}: {err}", path.display()))?;
    if recovery.format_version != RECOVERY_FORMAT_VERSION {
        return Err(format!(
            "Unsupported recovery format {} in {} (expected {}).",
            recovery.format_version,
            path.display(),
            RECOVERY_FORMAT_VERSION
        ));
    }
    if recovery.project.schema_version != crate::model::SHADE_SCHEMA_VERSION {
        return Err(format!(
            "Recovery {} uses .shade schema {}, but this build accepts schema {} only.",
            path.display(),
            recovery.project.schema_version,
            crate::model::SHADE_SCHEMA_VERSION
        ));
    }
    Ok(recovery)
}

pub fn write(recovery: &RecoveryFile) -> Result<PathBuf, String> {
    write_to_paths(recovery, &recovery_paths())
}

fn write_to_paths(recovery: &RecoveryFile, paths: &[PathBuf]) -> Result<PathBuf, String> {
    let latest = paths
        .first()
        .ok_or_else(|| "Recovery path list is empty.".to_owned())?;
    if let Some(parent) = latest.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Cannot create recovery folder {}: {err}", parent.display()))?;
    }

    for index in (1..paths.len()).rev() {
        if paths[index].exists() {
            fs::remove_file(&paths[index]).map_err(|err| {
                format!(
                    "Cannot rotate recovery file {}: {err}",
                    paths[index].display()
                )
            })?;
        }
        if paths[index - 1].exists() {
            fs::rename(&paths[index - 1], &paths[index]).map_err(|err| {
                format!(
                    "Cannot rotate recovery file {} to {}: {err}",
                    paths[index - 1].display(),
                    paths[index].display()
                )
            })?;
        }
    }

    let temp = latest.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(recovery)
        .map_err(|err| format!("Cannot serialize recovery state: {err}"))?;
    fs::write(&temp, text)
        .map_err(|err| format!("Cannot write recovery file {}: {err}", temp.display()))?;
    if latest.exists() {
        fs::remove_file(latest)
            .map_err(|err| format!("Cannot replace recovery file {}: {err}", latest.display()))?;
    }
    fs::rename(&temp, latest)
        .map_err(|err| format!("Cannot finalize recovery file {}: {err}", latest.display()))?;
    Ok(latest.clone())
}

pub fn clear() -> Result<(), String> {
    for path in recovery_paths() {
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|err| format!("Cannot remove recovery file {}: {err}", path.display()))?;
        }
    }
    Ok(())
}

pub fn is_recovery_path(path: &Path) -> bool {
    recovery_paths().iter().any(|candidate| candidate == path)
}

fn unix_ms_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths(label: &str) -> (PathBuf, [PathBuf; RECOVERY_STATE_COUNT]) {
        let unique = format!(
            "shade-recovery-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let folder = std::env::temp_dir().join(unique);
        let paths = [
            folder.join("recovery-v9.json"),
            folder.join("recovery-v9-1.json"),
            folder.join("recovery-v9-2.json"),
        ];
        (folder, paths)
    }

    #[test]
    fn recovery_file_keeps_absolute_face_references() {
        let project = ShadeProject::default();
        let recovery = RecoveryFile::new(
            project,
            vec![PathBuf::from(r"C:\tiles\face-1.tif")],
            Some(PathBuf::from(r"C:\tiles\test.shade")),
        );
        assert_eq!(
            recovery.resolved_face_paths()[0],
            PathBuf::from(r"C:\tiles\face-1.tif")
        );
        assert_eq!(
            recovery.origin_path(),
            Some(PathBuf::from(r"C:\tiles\test.shade"))
        );
    }

    #[test]
    fn recovery_rotation_keeps_latest_three_states() {
        let (folder, paths) = temp_paths("rotate");
        for state in 1..=4i64 {
            let mut recovery = RecoveryFile::new(ShadeProject::default(), vec![], None);
            recovery.saved_at_unix_ms = state;
            write_to_paths(&recovery, &paths).unwrap();
        }
        assert_eq!(read_recovery(&paths[0]).unwrap().saved_at_unix_ms, 4);
        assert_eq!(read_recovery(&paths[1]).unwrap().saved_at_unix_ms, 3);
        assert_eq!(read_recovery(&paths[2]).unwrap().saved_at_unix_ms, 2);
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn recovery_load_falls_back_when_latest_state_is_corrupt() {
        let (folder, paths) = temp_paths("fallback");
        let mut older = RecoveryFile::new(ShadeProject::default(), vec![], None);
        older.saved_at_unix_ms = 20;
        write_to_paths(&older, &paths).unwrap();
        let mut latest = RecoveryFile::new(ShadeProject::default(), vec![], None);
        latest.saved_at_unix_ms = 30;
        write_to_paths(&latest, &paths).unwrap();
        fs::write(&paths[0], "{broken json").unwrap();
        let loaded = load_from_paths(&paths).unwrap().unwrap();
        assert_eq!(loaded.saved_at_unix_ms, 20);
        let _ = fs::remove_dir_all(folder);
    }
}
'''
write("src/recovery.rs", recovery)

tiff = read("src/tiff_io.rs")
old_struct = '''#[derive(Clone, Debug)]
pub struct StreamInfo {
    pub metadata: TiffMetadata,
    pub rows_per_strip: u32,
    pub strip_count: u32,
    /// True when the source is chunky/interleaved and strip-based, allowing
    /// bounded-memory incremental decoding. Planar/tiled files use the proven
    /// full-image compatibility path.
    pub streamable: bool,
}
'''
new_struct = '''#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkStorage {
    Strips,
    Tiles,
}

#[derive(Clone, Debug)]
pub struct StreamInfo {
    pub metadata: TiffMetadata,
    /// Output strip height used when Shade Editor rewrites the TIFF. For tiled
    /// sources this is based on TileLength.
    pub rows_per_strip: u32,
    pub strip_count: u32,
    /// True when the source can be decoded one coding region at a time without
    /// allocating the full image.
    pub streamable: bool,
    /// True only for chunky strip TIFFs, where each decoded region is already a
    /// full-width row range and can use the older sequential spool path.
    pub row_streamable: bool,
    pub storage: ChunkStorage,
    pub planar_configuration: u16,
    pub chunk_width: u32,
    pub chunk_height: u32,
    pub coding_unit_count: u32,
}
'''
tiff = replace_once(tiff, old_struct, new_struct, "StreamInfo struct")

new_stream_section = r'''pub fn stream_info(path: &Path) -> Result<StreamInfo, String> {
    let mut decoder = open_decoder(path)?;
    let (metadata, planar_configuration) = read_metadata(&mut decoder)?;
    let tagged_rows_per_strip = decoder
        .find_tag_unsigned::<u32>(Tag::RowsPerStrip)
        .ok()
        .flatten()
        .unwrap_or(metadata.height)
        .max(1)
        .min(metadata.height.max(1));

    let tile_count = decoder.tile_count().ok().filter(|count| *count > 0);
    let strip_count = decoder.strip_count().ok().filter(|count| *count > 0);
    let (storage, total_chunks) = if let Some(count) = tile_count {
        (ChunkStorage::Tiles, count)
    } else if let Some(count) = strip_count {
        (ChunkStorage::Strips, count)
    } else {
        return Ok(StreamInfo {
            metadata,
            rows_per_strip: tagged_rows_per_strip,
            strip_count: 0,
            streamable: false,
            row_streamable: false,
            storage: ChunkStorage::Strips,
            planar_configuration,
            chunk_width: 0,
            chunk_height: 0,
            coding_unit_count: 0,
        });
    };

    let (chunk_width, chunk_height) = decoder.chunk_dimensions();
    let geometric_units = match storage {
        ChunkStorage::Strips => div_ceil_u32(metadata.height, chunk_height.max(1)),
        ChunkStorage::Tiles => div_ceil_u32(metadata.width, chunk_width.max(1))
            .checked_mul(div_ceil_u32(metadata.height, chunk_height.max(1)))
            .ok_or_else(|| "TIFF tile grid is too large.".to_owned())?,
    };
    let expected_chunks = if planar_configuration == 2 {
        geometric_units
            .checked_mul(metadata.samples_per_pixel as u32)
            .ok_or_else(|| "TIFF planar chunk count is too large.".to_owned())?
    } else {
        geometric_units
    };
    let streamable = chunk_width > 0
        && chunk_height > 0
        && geometric_units > 0
        && total_chunks == expected_chunks;
    let rows_per_strip = match storage {
        ChunkStorage::Strips => tagged_rows_per_strip,
        ChunkStorage::Tiles => chunk_height.max(1).min(metadata.height.max(1)),
    };

    Ok(StreamInfo {
        metadata,
        rows_per_strip,
        strip_count: if storage == ChunkStorage::Strips {
            total_chunks
        } else {
            0
        },
        streamable,
        row_streamable: streamable
            && storage == ChunkStorage::Strips
            && planar_configuration == 1,
        storage,
        planar_configuration,
        chunk_width,
        chunk_height,
        coding_unit_count: geometric_units,
    })
}

fn div_ceil_u32(value: u32, divisor: u32) -> u32 {
    if divisor == 0 {
        0
    } else {
        value / divisor + u32::from(value % divisor != 0)
    }
}

pub fn for_each_decoded_strip<F>(
    path: &Path,
    info: &StreamInfo,
    mut callback: F,
) -> Result<(), String>
where
    F: FnMut(u32, u32, &[u16]) -> Result<(), String>,
{
    if !info.row_streamable {
        let decoded = decode_full(path)?;
        callback(0, decoded.metadata.height, &decoded.samples)?;
        return Ok(());
    }
    for_each_decoded_region(path, info, |x, y, width, height, samples| {
        if x != 0 || width != info.metadata.width {
            return Err(format!(
                "TIFF row stream produced region x={x}, width={width}; expected full width {}.",
                info.metadata.width
            ));
        }
        callback(y, height, samples)
    })
}

pub fn for_each_decoded_region<F>(
    path: &Path,
    info: &StreamInfo,
    mut callback: F,
) -> Result<(), String>
where
    F: FnMut(u32, u32, u32, u32, &[u16]) -> Result<(), String>,
{
    if !info.streamable {
        let decoded = decode_full(path)?;
        callback(
            0,
            0,
            decoded.metadata.width,
            decoded.metadata.height,
            &decoded.samples,
        )?;
        return Ok(());
    }

    let needs_multiband_workaround = info.metadata.samples_per_pixel
        > info.metadata.base_channel_count
        && matches!(
            info.metadata.color_model,
            ColorModel::Rgb | ColorModel::Cmyk
        );
    if needs_multiband_workaround {
        let decoder = open_multiband_decoder(path)?;
        stream_decoder_regions(decoder, info, &mut callback)
    } else {
        let decoder = open_decoder(path)?;
        stream_decoder_regions(decoder, info, &mut callback)
    }
}

fn stream_decoder_regions<R, F>(
    mut decoder: Decoder<R>,
    info: &StreamInfo,
    callback: &mut F,
) -> Result<(), String>
where
    R: Read + Seek,
    F: FnMut(u32, u32, u32, u32, &[u16]) -> Result<(), String>,
{
    for unit_index in 0..info.coding_unit_count {
        let (data_width, data_height) = decoder.chunk_data_dimensions(unit_index);
        if data_width == 0 || data_height == 0 {
            continue;
        }
        let samples = decode_coding_unit(&mut decoder, info, unit_index, data_width, data_height)?;
        let (x, y) = coding_unit_origin(info, unit_index)?;
        callback(x, y, data_width, data_height, &samples)?;
    }
    Ok(())
}

fn decode_coding_unit<R: Read + Seek>(
    decoder: &mut Decoder<R>,
    info: &StreamInfo,
    unit_index: u32,
    data_width: u32,
    data_height: u32,
) -> Result<Vec<u16>, String> {
    let channels = info.metadata.samples_per_pixel;
    if info.planar_configuration == 1 {
        let decoded = decoder
            .read_chunk(unit_index)
            .map_err(|err| format!("Cannot decode TIFF chunk {unit_index}: {err}"))?;
        return compact_chunk(
            decoded,
            info.metadata.bit_depth,
            data_width,
            data_height,
            info.chunk_width,
            info.chunk_height,
            channels,
            unit_index,
        );
    }

    let pixels = (data_width as usize)
        .checked_mul(data_height as usize)
        .ok_or_else(|| "TIFF coding unit is too large.".to_owned())?;
    let mut output = vec![0u16; pixels.saturating_mul(channels)];
    for channel in 0..channels {
        let chunk_index = (channel as u32)
            .checked_mul(info.coding_unit_count)
            .and_then(|base| base.checked_add(unit_index))
            .ok_or_else(|| "TIFF planar chunk index overflow.".to_owned())?;
        let decoded = decoder
            .read_chunk(chunk_index)
            .map_err(|err| format!("Cannot decode TIFF planar chunk {chunk_index}: {err}"))?;
        let plane = compact_chunk(
            decoded,
            info.metadata.bit_depth,
            data_width,
            data_height,
            info.chunk_width,
            info.chunk_height,
            1,
            chunk_index,
        )?;
        if plane.len() != pixels {
            return Err(format!(
                "TIFF planar chunk {chunk_index} produced {} samples; expected {pixels}.",
                plane.len()
            ));
        }
        for pixel in 0..pixels {
            output[pixel * channels + channel] = plane[pixel];
        }
    }
    Ok(output)
}

fn compact_chunk(
    decoded: DecodingResult,
    bit_depth: u8,
    data_width: u32,
    data_height: u32,
    full_width: u32,
    full_height: u32,
    channels: usize,
    chunk_index: u32,
) -> Result<Vec<u16>, String> {
    let samples = decoding_result_to_u16(decoded, bit_depth)?;
    let data_width = data_width as usize;
    let data_height = data_height as usize;
    let full_width = full_width as usize;
    let full_height = full_height as usize;
    let data_row = data_width
        .checked_mul(channels)
        .ok_or_else(|| "TIFF chunk row is too large.".to_owned())?;
    let full_row = full_width
        .checked_mul(channels)
        .ok_or_else(|| "TIFF chunk row is too large.".to_owned())?;
    let data_expected = data_row
        .checked_mul(data_height)
        .ok_or_else(|| "TIFF chunk sample count is too large.".to_owned())?;
    let full_expected = full_row
        .checked_mul(full_height)
        .ok_or_else(|| "TIFF chunk sample count is too large.".to_owned())?;

    if samples.len() < data_expected {
        return Err(format!(
            "Decoded TIFF chunk {chunk_index} is incomplete ({} of at least {data_expected} samples).",
            samples.len()
        ));
    }
    if samples.len() < full_expected || (data_width == full_width && data_height == full_height) {
        return Ok(samples[..data_expected].to_vec());
    }

    let mut compact = Vec::with_capacity(data_expected);
    for row in 0..data_height {
        let start = row * full_row;
        compact.extend_from_slice(&samples[start..start + data_row]);
    }
    Ok(compact)
}

fn coding_unit_origin(info: &StreamInfo, unit_index: u32) -> Result<(u32, u32), String> {
    match info.storage {
        ChunkStorage::Strips => Ok((
            0,
            unit_index
                .checked_mul(info.chunk_height)
                .ok_or_else(|| "TIFF strip position overflow.".to_owned())?,
        )),
        ChunkStorage::Tiles => {
            let across = div_ceil_u32(info.metadata.width, info.chunk_width.max(1)).max(1);
            let tile_x = unit_index % across;
            let tile_y = unit_index / across;
            Ok((
                tile_x
                    .checked_mul(info.chunk_width)
                    .ok_or_else(|| "TIFF tile X position overflow.".to_owned())?,
                tile_y
                    .checked_mul(info.chunk_height)
                    .ok_or_else(|| "TIFF tile Y position overflow.".to_owned())?,
            ))
        }
    }
}

pub fn load_preview(path: &Path, max_dimension: u32) -> Result<PreviewFace, String> {
    let info = stream_info(path)?;
    let source_width = info.metadata.width as usize;
    let source_height = info.metadata.height as usize;
    let max_source = source_width.max(source_height).max(1);
    let max_dimension = max_dimension.max(256) as usize;
    let scale = (max_source as f64 / max_dimension as f64).max(1.0);
    let width = ((source_width as f64 / scale).round() as usize).max(1);
    let height = ((source_height as f64 / scale).round() as usize).max(1);
    let channel_count = info.metadata.samples_per_pixel;
    let preview_pixels = width
        .checked_mul(height)
        .ok_or_else(|| "Preview dimensions are too large.".to_owned())?;
    let mut channels = (0..channel_count)
        .map(|_| vec![0u16; preview_pixels])
        .collect::<Vec<_>>();

    let source_x = (0..width)
        .map(|x| {
            ((x as f64 * source_width as f64 / width as f64).floor() as usize)
                .min(source_width.saturating_sub(1))
        })
        .collect::<Vec<_>>();
    let source_y = (0..height)
        .map(|y| {
            ((y as f64 * source_height as f64 / height as f64).floor() as usize)
                .min(source_height.saturating_sub(1))
        })
        .collect::<Vec<_>>();
    let mut filled = vec![false; preview_pixels];
    let mut filled_count = 0usize;

    for_each_decoded_region(path, &info, |region_x, region_y, region_width, region_height, samples| {
        let x0 = region_x as usize;
        let y0 = region_y as usize;
        let rw = region_width as usize;
        let rh = region_height as usize;
        let x1 = x0.saturating_add(rw);
        let y1 = y0.saturating_add(rh);
        let preview_x0 = source_x.partition_point(|value| *value < x0);
        let preview_x1 = source_x.partition_point(|value| *value < x1);
        let preview_y0 = source_y.partition_point(|value| *value < y0);
        let preview_y1 = source_y.partition_point(|value| *value < y1);

        for preview_y in preview_y0..preview_y1 {
            let local_y = source_y[preview_y] - y0;
            for preview_x in preview_x0..preview_x1 {
                let local_x = source_x[preview_x] - x0;
                let source_base = (local_y * rw + local_x) * channel_count;
                let destination = preview_y * width + preview_x;
                for channel in 0..channel_count {
                    channels[channel][destination] = samples[source_base + channel];
                }
                if !filled[destination] {
                    filled[destination] = true;
                    filled_count += 1;
                }
            }
        }
        Ok(())
    })?;

    if filled_count != preview_pixels {
        return Err(format!(
            "Preview region stream filled {filled_count} of {preview_pixels} pixels."
        ));
    }
    let histograms = channels.iter().map(|plane| histogram(plane)).collect();
    Ok(PreviewFace {
        metadata: info.metadata,
        width,
        height,
        channels,
        histograms,
    })
}

'''
tiff = regex_once(
    tiff,
    r'pub fn stream_info\(path: &Path\).*?\nfn read_metadata<',
    new_stream_section + 'fn read_metadata<',
    "streaming section",
)

test_insert = r'''
    fn temp_tiff_path(label: &str) -> PathBuf {
        let unique = format!(
            "shade-{label}-{}-{}.tif",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::env::temp_dir().join(unique)
    }

    fn push_u16(bytes: &mut Vec<u8>, value: u16) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_ifd_entry(bytes: &mut Vec<u8>, tag: u16, field_type: u16, count: u32, value: u32) {
        push_u16(bytes, tag);
        push_u16(bytes, field_type);
        push_u32(bytes, count);
        push_u32(bytes, value);
    }

    fn build_planar_rgb8_strip_tiff() -> Vec<u8> {
        const ENTRY_COUNT: u16 = 10;
        let ifd_size = 2 + ENTRY_COUNT as usize * 12 + 4;
        let values_start = 8 + ifd_size;
        let bits_offset = values_start as u32;
        let strip_offsets_offset = bits_offset + 6;
        let strip_byte_counts_offset = strip_offsets_offset + 12;
        let pixels_offset = strip_byte_counts_offset + 12;
        let strip_offsets = [pixels_offset, pixels_offset + 4, pixels_offset + 8];

        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"II");
        push_u16(&mut bytes, 42);
        push_u32(&mut bytes, 8);
        push_u16(&mut bytes, ENTRY_COUNT);
        push_ifd_entry(&mut bytes, 256, 3, 1, 2);
        push_ifd_entry(&mut bytes, 257, 3, 1, 2);
        push_ifd_entry(&mut bytes, 258, 3, 3, bits_offset);
        push_ifd_entry(&mut bytes, 259, 3, 1, 1);
        push_ifd_entry(&mut bytes, 262, 3, 1, 2);
        push_ifd_entry(&mut bytes, 273, 4, 3, strip_offsets_offset);
        push_ifd_entry(&mut bytes, 277, 3, 1, 3);
        push_ifd_entry(&mut bytes, 278, 4, 1, 2);
        push_ifd_entry(&mut bytes, 279, 4, 3, strip_byte_counts_offset);
        push_ifd_entry(&mut bytes, 284, 3, 1, 2);
        push_u32(&mut bytes, 0);
        for _ in 0..3 {
            push_u16(&mut bytes, 8);
        }
        for offset in strip_offsets {
            push_u32(&mut bytes, offset);
        }
        for _ in 0..3 {
            push_u32(&mut bytes, 4);
        }
        bytes.extend_from_slice(&[1, 2, 3, 4]);
        bytes.extend_from_slice(&[11, 12, 13, 14]);
        bytes.extend_from_slice(&[21, 22, 23, 24]);
        bytes
    }

    fn build_tiled_rgb8_tiff() -> Vec<u8> {
        const WIDTH: u32 = 17;
        const HEIGHT: u32 = 2;
        const TILE: u32 = 16;
        const ENTRY_COUNT: u16 = 11;
        let ifd_size = 2 + ENTRY_COUNT as usize * 12 + 4;
        let values_start = 8 + ifd_size;
        let bits_offset = values_start as u32;
        let tile_offsets_offset = bits_offset + 6;
        let tile_byte_counts_offset = tile_offsets_offset + 8;
        let pixels_offset = tile_byte_counts_offset + 8;
        let tile_bytes = TILE * TILE * 3;
        let tile_offsets = [pixels_offset, pixels_offset + tile_bytes];

        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"II");
        push_u16(&mut bytes, 42);
        push_u32(&mut bytes, 8);
        push_u16(&mut bytes, ENTRY_COUNT);
        push_ifd_entry(&mut bytes, 256, 4, 1, WIDTH);
        push_ifd_entry(&mut bytes, 257, 4, 1, HEIGHT);
        push_ifd_entry(&mut bytes, 258, 3, 3, bits_offset);
        push_ifd_entry(&mut bytes, 259, 3, 1, 1);
        push_ifd_entry(&mut bytes, 262, 3, 1, 2);
        push_ifd_entry(&mut bytes, 277, 3, 1, 3);
        push_ifd_entry(&mut bytes, 284, 3, 1, 1);
        push_ifd_entry(&mut bytes, 322, 4, 1, TILE);
        push_ifd_entry(&mut bytes, 323, 4, 1, TILE);
        push_ifd_entry(&mut bytes, 324, 4, 2, tile_offsets_offset);
        push_ifd_entry(&mut bytes, 325, 4, 2, tile_byte_counts_offset);
        push_u32(&mut bytes, 0);
        for _ in 0..3 {
            push_u16(&mut bytes, 8);
        }
        for offset in tile_offsets {
            push_u32(&mut bytes, offset);
        }
        for _ in 0..2 {
            push_u32(&mut bytes, tile_bytes);
        }

        for tile_x in 0..2u32 {
            for local_y in 0..TILE {
                for local_x in 0..TILE {
                    let x = tile_x * TILE + local_x;
                    let y = local_y;
                    if x < WIDTH && y < HEIGHT {
                        bytes.push((x + 1) as u8);
                        bytes.push((40 + x) as u8);
                        bytes.push((80 + x) as u8);
                    } else {
                        bytes.extend_from_slice(&[0, 0, 0]);
                    }
                }
            }
        }
        bytes
    }

    fn collect_regions(path: &Path) -> (StreamInfo, Vec<u16>) {
        let info = stream_info(path).unwrap();
        let width = info.metadata.width as usize;
        let height = info.metadata.height as usize;
        let channels = info.metadata.samples_per_pixel;
        let mut canvas = vec![0u16; width * height * channels];
        for_each_decoded_region(path, &info, |x, y, w, h, samples| {
            for local_y in 0..h as usize {
                let source = local_y * w as usize * channels;
                let destination =
                    ((y as usize + local_y) * width + x as usize) * channels;
                let count = w as usize * channels;
                canvas[destination..destination + count]
                    .copy_from_slice(&samples[source..source + count]);
            }
            Ok(())
        })
        .unwrap();
        (info, canvas)
    }

    #[test]
    fn region_stream_interleaves_planar_strips_without_full_decode() {
        let path = temp_tiff_path("planar");
        fs::write(&path, build_planar_rgb8_strip_tiff()).unwrap();
        let (info, canvas) = collect_regions(&path);
        assert!(info.streamable);
        assert!(!info.row_streamable);
        assert_eq!(info.storage, ChunkStorage::Strips);
        assert_eq!(info.planar_configuration, 2);
        let expected = vec![
            257, 2827, 5397, 514, 3084, 5654, 771, 3341, 5911, 1028, 3598, 6168,
        ];
        assert_eq!(canvas, expected);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn region_stream_compacts_edge_tiles_without_full_decode() {
        let path = temp_tiff_path("tiles");
        fs::write(&path, build_tiled_rgb8_tiff()).unwrap();
        let (info, canvas) = collect_regions(&path);
        assert!(info.streamable);
        assert!(!info.row_streamable);
        assert_eq!(info.storage, ChunkStorage::Tiles);
        assert_eq!(info.chunk_width, 16);
        assert_eq!(info.chunk_height, 16);
        let channels = 3usize;
        for y in 0..2usize {
            for x in 0..17usize {
                let base = (y * 17 + x) * channels;
                assert_eq!(canvas[base], ((x + 1) as u16) * 257);
                assert_eq!(canvas[base + 1], ((40 + x) as u16) * 257);
                assert_eq!(canvas[base + 2], ((80 + x) as u16) * 257);
            }
        }
        let _ = fs::remove_file(path);
    }

'''
anchor = '''    #[test]
    fn names_all_photoshop_extra_channels() {
'''
tiff = replace_once(tiff, anchor, test_insert + anchor, "tiff region tests insertion")
write("src/tiff_io.rs", tiff)

export = read("src/export_v6.rs")
export = replace_once(export, "use std::fs::{self, File};", "use std::fs::{self, File, OpenOptions};", "OpenOptions import")
export = replace_once(
    export,
    "    ColorModel, StreamInfo, TiffMetadata, decode_full, for_each_decoded_strip, stream_info,\n",
    "    ColorModel, StreamInfo, TiffMetadata, decode_full, for_each_decoded_region,\n    for_each_decoded_strip, stream_info,\n",
    "region import",
)
old_spool_block = '''        {
            let spool_file = File::create(&spool_path)
                .map_err(|err| format!("Cannot create export spool: {err}"))?;
            let mut spool = BufWriter::new(spool_file);
            match metadata.bit_depth {
                8 => stream_spool_u8(
                    source,
                    stream,
                    project,
                    overlay.as_ref(),
                    &mut spool,
                    progress,
                )?,
                16 => stream_spool_u16(
                    source,
                    stream,
                    project,
                    overlay.as_ref(),
                    &mut spool,
                    progress,
                )?,
                _ => unreachable!(),
            }
            spool
                .flush()
                .map_err(|err| format!("Cannot flush export spool: {err}"))?;
        }
'''
new_spool_block = '''        if stream.row_streamable {
            let spool_file = File::create(&spool_path)
                .map_err(|err| format!("Cannot create export spool: {err}"))?;
            let mut spool = BufWriter::new(spool_file);
            match metadata.bit_depth {
                8 => stream_spool_u8(
                    source,
                    stream,
                    project,
                    overlay.as_ref(),
                    &mut spool,
                    progress,
                )?,
                16 => stream_spool_u16(
                    source,
                    stream,
                    project,
                    overlay.as_ref(),
                    &mut spool,
                    progress,
                )?,
                _ => unreachable!(),
            }
            spool
                .flush()
                .map_err(|err| format!("Cannot flush export spool: {err}"))?;
        } else {
            stream_spool_regions(
                source,
                stream,
                project,
                overlay.as_ref(),
                &spool_path,
                progress,
            )?;
        }
'''
export = replace_once(export, old_spool_block, new_spool_block, "stream spool branch")

region_spool = r'''
fn stream_spool_regions<F>(
    source: &Path,
    stream: &StreamInfo,
    project: &ShadeProject,
    overlay: Option<&TextOverlay>,
    spool_path: &Path,
    progress: &mut F,
) -> Result<(), String>
where
    F: FnMut(f32, &str),
{
    let metadata = &stream.metadata;
    let channels = metadata.samples_per_pixel;
    let names = &metadata.channel_names;
    let full_width = metadata.width as usize;
    let bytes_per_sample = usize::from(metadata.bit_depth / 8);
    let total_samples = (metadata.width as usize)
        .checked_mul(metadata.height as usize)
        .and_then(|value| value.checked_mul(channels))
        .ok_or_else(|| "Export spool sample count overflow.".to_owned())?;
    let total_bytes = total_samples
        .checked_mul(bytes_per_sample)
        .ok_or_else(|| "Export spool size overflow.".to_owned())?;

    let spool_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(spool_path)
        .map_err(|err| format!("Cannot create random-access export spool: {err}"))?;
    spool_file
        .set_len(total_bytes as u64)
        .map_err(|err| format!("Cannot size export spool: {err}"))?;
    let mut mmap = unsafe {
        MmapOptions::new()
            .map_mut(&spool_file)
            .map_err(|err| format!("Cannot map export spool for writing: {err}"))?
    };
    let total_pixels = u64::from(metadata.width).saturating_mul(u64::from(metadata.height));
    let mut processed_pixels = 0u64;

    for_each_decoded_region(source, stream, |x, y, width, height, input| {
        let region_width = width as usize;
        let region_height = height as usize;
        let mut adjusted = adjusted_strip(input, channels, names, project);
        if let Some(overlay) = overlay {
            apply_text_overlay_to_region(
                &mut adjusted,
                x as usize,
                y as usize,
                region_width,
                region_height,
                channels,
                overlay,
            );
        }
        let expected = region_width
            .checked_mul(region_height)
            .and_then(|value| value.checked_mul(channels))
            .ok_or_else(|| "Output region sample count overflow.".to_owned())?;
        if adjusted.len() != expected {
            return Err(format!(
                "Output region sample mismatch: generated {}, expected {expected}.",
                adjusted.len()
            ));
        }

        for local_y in 0..region_height {
            let source_sample = local_y * region_width * channels;
            let destination_sample =
                ((y as usize + local_y) * full_width + x as usize) * channels;
            let row_samples = region_width * channels;
            match metadata.bit_depth {
                8 => {
                    let destination = destination_sample;
                    for offset in 0..row_samples {
                        mmap[destination + offset] =
                            (adjusted[source_sample + offset] >> 8) as u8;
                    }
                }
                16 => {
                    let destination = destination_sample * 2;
                    for offset in 0..row_samples {
                        let bytes = adjusted[source_sample + offset].to_ne_bytes();
                        let index = destination + offset * 2;
                        mmap[index] = bytes[0];
                        mmap[index + 1] = bytes[1];
                    }
                }
                depth => {
                    return Err(format!(
                        "Unsupported streaming spool bit depth: {depth}-bit."
                    ));
                }
            }
        }

        processed_pixels = processed_pixels
            .saturating_add(u64::from(width).saturating_mul(u64::from(height)));
        let done = processed_pixels as f32 / total_pixels.max(1) as f32;
        progress(0.06 + done.min(1.0) * 0.60, "Streaming TIFF regions to disk spool");
        Ok(())
    })?;
    mmap.flush()
        .map_err(|err| format!("Cannot flush random-access export spool: {err}"))?;
    Ok(())
}

'''
export = replace_once(export, "fn mmap_as_u16(", region_spool + "fn mmap_as_u16(", "region spool insertion")

region_overlay = r'''fn apply_text_overlay_to_region(
    samples: &mut [u16],
    region_x: usize,
    region_y: usize,
    region_width: usize,
    region_height: usize,
    channels: usize,
    overlay: &TextOverlay,
) {
    if overlay.bitmap.width == 0 || overlay.bitmap.height == 0 {
        return;
    }
    let region_x1 = region_x.saturating_add(region_width);
    let region_y1 = region_y.saturating_add(region_height);
    let text_x1 = overlay.x0.saturating_add(overlay.bitmap.width);
    let text_y1 = overlay.y0.saturating_add(overlay.bitmap.height);
    let x_begin = region_x.max(overlay.x0);
    let x_end = region_x1.min(text_x1);
    let y_begin = region_y.max(overlay.y0);
    let y_end = region_y1.min(text_y1);
    if x_begin >= x_end || y_begin >= y_end {
        return;
    }

    for image_y in y_begin..y_end {
        let bitmap_y = image_y - overlay.y0;
        let local_y = image_y - region_y;
        for image_x in x_begin..x_end {
            let bitmap_x = image_x - overlay.x0;
            let alpha =
                overlay.bitmap.alpha[bitmap_y * overlay.bitmap.width + bitmap_x];
            if alpha == 0 {
                continue;
            }
            let local_x = image_x - region_x;
            for &(target_channel, target_value) in &overlay.targets {
                let index = (local_y * region_width + local_x) * channels + target_channel;
                if index >= samples.len() {
                    continue;
                }
                let a = f32::from(alpha) / 255.0;
                let current = samples[index] as f32;
                samples[index] =
                    (current * (1.0 - a) + target_value as f32 * a).round() as u16;
            }
        }
    }
}

'''
export = replace_once(
    export,
    "fn apply_text_overlay_to_rows(",
    region_overlay + "fn apply_text_overlay_to_rows(",
    "region overlay insertion",
)
write("src/export_v6.rs", export)

notes = read("RELEASE_NOTES.md")
v011 = '''# Shade Editor v0.11.0

- Extended bounded-memory TIFF decoding to tiled and planar 8/16-bit RGB/CMYK layouts.
- Preview now samples arbitrary TIFF coding regions instead of requiring full-width strips.
- Export uses a random-access disk-backed spool for tiled/planar sources while keeping the proven sequential strip path for normal Photoshop TIFFs.
- Crash recovery now rotates the latest three states and automatically falls back to an older valid state if the newest recovery JSON is damaged.
- Added planar-strip and edge-tile regression fixtures for the new region decoder.

'''
if not notes.startswith("# Shade Editor v0.11.0"):
    notes = v011 + notes
write("RELEASE_NOTES.md", notes)

roadmap = read("docs/ROADMAP.md")
roadmap = roadmap.replace(
    "- Extend bounded streaming to tiled and planar TIFF layouts. Normal chunky strip TIFFs already use the streaming pipeline.\n"
    "- Rotate crash recovery through the latest three recovery states instead of keeping only one recovery file.\n",
    "- Production-test bounded tiled/planar TIFF streaming against real Photoshop/RIP assets; synthetic planar-strip and tiled-edge fixtures are covered in CI.\n"
    "- Production-test the three-state recovery rotation and corrupted-latest fallback on Windows.\n",
)
write("docs/ROADMAP.md", roadmap)
