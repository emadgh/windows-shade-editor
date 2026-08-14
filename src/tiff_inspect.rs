use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use tiff::decoder::{Decoder, Limits};
use tiff::tags::Tag;

use crate::color_management;
use crate::dpi;
use crate::tiff_io::{self, ChunkStorage};

const MAX_INSPECTED_IFDS: usize = 1024;
const MAX_PHOTOSHOP_RESOURCE_ENTRIES: usize = 512;
const TIFF_TAG_INK_SET: Tag = Tag::Unknown(332);
const TIFF_TAG_INK_NAMES: Tag = Tag::Unknown(333);
const TIFF_TAG_NUMBER_OF_INKS: Tag = Tag::Unknown(334);

#[derive(Clone, Debug)]
pub struct TiffInspection {
    pub path: PathBuf,
    pub report: String,
}

pub fn inspect(path: &Path, default_dpi: f64) -> Result<TiffInspection, String> {
    let stream = tiff_io::stream_info(path)?;
    let metadata = &stream.metadata;
    let file_size = fs::metadata(path)
        .map_err(|err| format!("Cannot read TIFF file metadata: {err}"))?
        .len();

    let file = File::open(path).map_err(|err| format!("Cannot inspect TIFF tags: {err}"))?;
    let mut decoder = Decoder::new(BufReader::new(file))
        .map_err(|err| format!("Cannot initialize TIFF inspector: {err}"))?
        .with_limits(Limits::default());

    let photometric = tag_u16(&mut decoder, Tag::PhotometricInterpretation);
    let sample_format = tag_u64_vec(&mut decoder, Tag::SampleFormat);
    let fill_order = tag_u16(&mut decoder, Tag::FillOrder);
    let orientation = tag_u16(&mut decoder, Tag::Orientation);
    let rows_per_strip = tag_u64(&mut decoder, Tag::RowsPerStrip);
    let tile_width = tag_u64(&mut decoder, Tag::TileWidth);
    let tile_length = tag_u64(&mut decoder, Tag::TileLength);
    let ink_set = tag_u16(&mut decoder, TIFF_TAG_INK_SET);
    let number_of_inks = tag_u64(&mut decoder, TIFF_TAG_NUMBER_OF_INKS);
    let ink_names = decoder
        .get_tag_ascii_string(TIFF_TAG_INK_NAMES)
        .ok()
        .map(parse_ink_names)
        .unwrap_or_default();
    let extra_samples = tag_u64_vec(&mut decoder, Tag::ExtraSamples)
        .into_iter()
        .map(|value| value as u16)
        .collect::<Vec<_>>();

    let ifd_count = count_ifds(path)?;
    let (container, byte_order) = tiff_container(path)?;
    let dpi = dpi::read_dpi(path, default_dpi);
    let estimated = u128::from(metadata.width)
        .saturating_mul(u128::from(metadata.height))
        .saturating_mul(metadata.samples_per_pixel as u128)
        .saturating_mul(u128::from(metadata.bit_depth))
        / 8;

    let icc = color_management::embedded_profile_description(metadata)
        .unwrap_or_else(|| "None".to_owned());
    let photoshop_bytes = metadata
        .photoshop_resources
        .as_ref()
        .map(Vec::len)
        .unwrap_or(0);
    let image_source_bytes = metadata
        .photoshop_image_source_data
        .as_ref()
        .map(Vec::len)
        .unwrap_or(0);
    let photoshop_resources = metadata
        .photoshop_resources
        .as_deref()
        .map(parse_photoshop_resources)
        .unwrap_or_default();

    let spot_names = metadata
        .channel_names
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            metadata
                .channel_display_info
                .get(*index)
                .and_then(|value| *value)
                .is_some_and(|info| info.is_spot())
        })
        .map(|(_, name)| name.clone())
        .collect::<Vec<_>>();

    let warnings = production_warnings(
        metadata,
        &stream,
        &dpi,
        photometric,
        &extra_samples,
        &spot_names,
        ink_set,
        number_of_inks,
        &ink_names,
        ifd_count,
    );

    let mut report = String::new();
    macro_rules! line {
        ($($arg:tt)*) => {{
            report.push_str(&format!($($arg)*));
            report.push('\n');
        }};
    }

    line!("Shade Editor TIFF Inspection");
    line!("============================");
    line!("File: {}", path.display());
    line!("Container: {container}");
    line!("Byte order: {byte_order}");
    line!("IFD / page count: {ifd_count}");
    line!("File size: {}", format_bytes(file_size as u128));
    line!("Dimensions: {} × {} px", metadata.width, metadata.height);
    line!("Bits per sample: {}", metadata.bit_depth);
    line!("SampleFormat: {}", sample_format_label(&sample_format));
    line!("FillOrder: {}", fill_order_label(fill_order));
    line!("Orientation: {}", orientation_label(orientation));
    line!(
        "PhotometricInterpretation: {}",
        photometric_label(photometric)
    );
    line!(
        "PlanarConfiguration: {}",
        planar_label(stream.planar_configuration)
    );
    line!("Compression: {}", compression_label(metadata.compression));
    line!("Predictor: {}", predictor_label(metadata.predictor));
    line!("SamplesPerPixel: {}", metadata.samples_per_pixel);
    line!("Base color model: {}", metadata.color_model.title());
    line!("Base channel count: {}", metadata.base_channel_count);
    line!(
        "ExtraSamples: {}",
        if extra_samples.is_empty() {
            "None".to_owned()
        } else {
            extra_samples
                .iter()
                .map(|value| format!("{} ({})", value, extra_sample_label(*value)))
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    line!(
        "Storage: {}",
        match stream.storage {
            ChunkStorage::Strips => "Strips",
            ChunkStorage::Tiles => "Tiles",
        }
    );
    line!(
        "Coding unit: {} × {} px · {} unit(s) · streamable={}",
        stream.chunk_width,
        stream.chunk_height,
        stream.coding_unit_count,
        stream.streamable
    );
    line!(
        "RowsPerStrip: {}",
        rows_per_strip
            .map(|value| value.to_string())
            .unwrap_or_else(|| "N/A".into())
    );
    line!(
        "Tile geometry: {}",
        match (tile_width, tile_length) {
            (Some(w), Some(h)) => format!("{w} × {h} px"),
            _ => "N/A".to_owned(),
        }
    );
    line!(
        "DPI: {}",
        if dpi.has_physical_resolution {
            format!(
                "{:.4} × {:.4} dpi (ResolutionUnit={})",
                dpi.dpi_x, dpi.dpi_y, dpi.unit
            )
        } else {
            "No physical source DPI tags".to_owned()
        }
    );
    line!("ICC: {icc}");
    line!(
        "ICC payload: {}",
        metadata
            .icc_profile
            .as_ref()
            .map(|bytes| format_bytes(bytes.len() as u128))
            .unwrap_or_else(|| "None".to_owned())
    );
    line!("InkSet: {}", ink_set_label(ink_set));
    line!(
        "NumberOfInks: {}",
        number_of_inks
            .map(|value| value.to_string())
            .unwrap_or_else(|| "Not present".into())
    );
    line!(
        "InkNames: {}",
        if ink_names.is_empty() {
            "Not present".to_owned()
        } else {
            ink_names.join(" | ")
        }
    );
    line!(
        "Photoshop Image Resources (34377): {}",
        if photoshop_bytes == 0 {
            "None".to_owned()
        } else {
            format_bytes(photoshop_bytes as u128)
        }
    );
    if !photoshop_resources.is_empty() {
        line!("Photoshop resource blocks:");
        for resource in &photoshop_resources {
            line!(
                "  - ID {} (0x{:04X}) · {} · {}",
                resource.id,
                resource.id,
                resource.name.as_deref().unwrap_or("unnamed"),
                format_bytes(resource.data_len as u128)
            );
        }
    }
    line!(
        "Photoshop ImageSourceData (37724): {}",
        if image_source_bytes == 0 {
            "None".to_owned()
        } else {
            format_bytes(image_source_bytes as u128)
        }
    );
    line!(
        "Estimated uncompressed sample data: {}",
        format_bytes(estimated)
    );
    line!("");
    line!("Channel order");
    line!("-------------");
    for (index, name) in metadata.channel_names.iter().enumerate() {
        let role = if index < metadata.base_channel_count {
            "base".to_owned()
        } else {
            match metadata
                .channel_display_info
                .get(index)
                .and_then(|value| *value)
            {
                Some(info) if info.is_spot() => {
                    format!("Spot · Solidity {:.0}%", info.solidity * 100.0)
                }
                Some(_) => "Alpha / auxiliary".to_owned(),
                None => "Extra (type not declared)".to_owned(),
            }
        };
        line!("{:02}. {} · {}", index + 1, name, role);
    }
    line!("");
    line!(
        "Declared Spot order: {}",
        if spot_names.is_empty() {
            "None".to_owned()
        } else {
            spot_names.join(" → ")
        }
    );
    line!("");
    line!("Warnings / RIP risks");
    line!("--------------------");
    if warnings.is_empty() {
        line!("None detected by the static inspector.");
    } else {
        for warning in warnings {
            line!("- {warning}");
        }
    }

    Ok(TiffInspection {
        path: path.to_path_buf(),
        report,
    })
}

fn tag_u16<R: Read + std::io::Seek>(decoder: &mut Decoder<R>, tag: Tag) -> Option<u16> {
    decoder.find_tag_unsigned::<u16>(tag).ok().flatten()
}

fn tag_u64<R: Read + std::io::Seek>(decoder: &mut Decoder<R>, tag: Tag) -> Option<u64> {
    decoder.find_tag_unsigned::<u64>(tag).ok().flatten()
}

fn tag_u64_vec<R: Read + std::io::Seek>(decoder: &mut Decoder<R>, tag: Tag) -> Vec<u64> {
    decoder.get_tag_u64_vec(tag).unwrap_or_default()
}

fn count_ifds(path: &Path) -> Result<usize, String> {
    let file = File::open(path).map_err(|err| format!("Cannot inspect TIFF pages: {err}"))?;
    let mut decoder = Decoder::new(BufReader::new(file))
        .map_err(|err| format!("Cannot initialize TIFF page inspector: {err}"))?
        .with_limits(Limits::default());
    let mut count = 1usize;
    while decoder.more_images() {
        if count >= MAX_INSPECTED_IFDS {
            return Err(format!(
                "TIFF contains more than {MAX_INSPECTED_IFDS} IFDs; inspection stopped by safety limit."
            ));
        }
        decoder
            .next_image()
            .map_err(|err| format!("Cannot advance TIFF IFD: {err}"))?;
        count += 1;
    }
    Ok(count)
}

fn parse_ink_names(value: String) -> Vec<String> {
    value
        .split('\0')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PhotoshopResourceSummary {
    id: u16,
    name: Option<String>,
    data_len: usize,
}

fn parse_photoshop_resources(bytes: &[u8]) -> Vec<PhotoshopResourceSummary> {
    let mut result = Vec::new();
    let mut offset = 0usize;
    while offset + 12 <= bytes.len() && result.len() < MAX_PHOTOSHOP_RESOURCE_ENTRIES {
        if &bytes[offset..offset + 4] != b"8BIM" {
            break;
        }
        let id = u16::from_be_bytes([bytes[offset + 4], bytes[offset + 5]]);
        offset += 6;
        let name_len = bytes[offset] as usize;
        offset += 1;
        if offset + name_len > bytes.len() {
            break;
        }
        let name = if name_len == 0 {
            None
        } else {
            Some(String::from_utf8_lossy(&bytes[offset..offset + name_len]).into_owned())
        };
        offset += name_len;
        if (1 + name_len) % 2 != 0 {
            offset += 1;
        }
        if offset + 4 > bytes.len() {
            break;
        }
        let data_len = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]) as usize;
        offset += 4;
        if offset + data_len > bytes.len() {
            break;
        }
        result.push(PhotoshopResourceSummary { id, name, data_len });
        offset += data_len;
        if data_len % 2 != 0 {
            offset += 1;
        }
    }
    result
}

fn production_warnings(
    metadata: &tiff_io::TiffMetadata,
    stream: &tiff_io::StreamInfo,
    dpi: &dpi::DpiInfo,
    photometric: Option<u16>,
    extra_samples: &[u16],
    spot_names: &[String],
    ink_set: Option<u16>,
    number_of_inks: Option<u64>,
    ink_names: &[String],
    ifd_count: usize,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if metadata.icc_profile.is_none() {
        warnings.push(
            "No embedded ICC profile; color interpretation depends on external assumptions."
                .to_owned(),
        );
    }
    if !dpi.has_physical_resolution {
        warnings.push(
            "No physical DPI tags; physical print size depends on a fallback DPI.".to_owned(),
        );
    }
    if !stream.streamable {
        warnings.push("TIFF is not on Shade Editor's bounded streaming path; large exports may require compatibility decode.".to_owned());
    }
    if ifd_count != 1 {
        warnings.push(format!("TIFF contains {ifd_count} IFD/pages; production export edits the active image model, not a multi-page document workflow."));
    }
    if extra_samples.iter().any(|value| *value > 2) {
        warnings.push("Unknown ExtraSamples semantics detected.".to_owned());
    }
    if metadata.samples_per_pixel > metadata.base_channel_count && spot_names.is_empty() {
        warnings.push(
            "Extra channels exist but no Photoshop Spot DisplayInfo was detected.".to_owned(),
        );
    }
    if !spot_names.is_empty() && metadata.photoshop_resources.is_none() {
        warnings.push("Spot channels were inferred without Photoshop Image Resources; verify names/order in Photoshop and RIP.".to_owned());
    }
    if photometric == Some(5)
        && metadata.base_channel_count == 4
        && ink_set.is_some_and(|value| value != 1)
    {
        warnings.push(
            "Separated TIFF declares a non-CMYK InkSet; verify RIP interpretation.".to_owned(),
        );
    }
    if let Some(count) = number_of_inks {
        if !ink_names.is_empty() && count as usize != ink_names.len() {
            warnings.push(format!(
                "NumberOfInks={count} but InkNames contains {} names.",
                ink_names.len()
            ));
        }
    }
    warnings
}

fn tiff_container(path: &Path) -> Result<(&'static str, &'static str), String> {
    let mut file = File::open(path).map_err(|err| format!("Cannot read TIFF header: {err}"))?;
    let mut header = [0u8; 4];
    file.read_exact(&mut header)
        .map_err(|err| format!("Cannot read TIFF header: {err}"))?;
    let little = &header[..2] == b"II";
    let big = &header[..2] == b"MM";
    if !little && !big {
        return Err("Invalid TIFF byte-order signature.".to_owned());
    }
    let magic = if little {
        u16::from_le_bytes([header[2], header[3]])
    } else {
        u16::from_be_bytes([header[2], header[3]])
    };
    let container = match magic {
        42 => "Classic TIFF",
        43 => "BigTIFF",
        other => return Err(format!("Unknown TIFF magic value {other}.")),
    };
    Ok((
        container,
        if little {
            "Little-endian (II)"
        } else {
            "Big-endian (MM)"
        },
    ))
}

fn photometric_label(value: Option<u16>) -> String {
    match value {
        Some(0) => "0 · WhiteIsZero".to_owned(),
        Some(1) => "1 · BlackIsZero".to_owned(),
        Some(2) => "2 · RGB".to_owned(),
        Some(3) => "3 · Palette".to_owned(),
        Some(4) => "4 · Transparency mask".to_owned(),
        Some(5) => "5 · Separated / CMYK".to_owned(),
        Some(6) => "6 · YCbCr".to_owned(),
        Some(8) => "8 · CIELab".to_owned(),
        Some(value) => format!("{value}"),
        None => "Not present".to_owned(),
    }
}

fn sample_format_label(values: &[u64]) -> String {
    if values.is_empty() {
        return "1 · Unsigned integer (default)".to_owned();
    }
    values
        .iter()
        .map(|value| match value {
            1 => "1 · Unsigned integer".to_owned(),
            2 => "2 · Signed integer".to_owned(),
            3 => "3 · IEEE floating point".to_owned(),
            4 => "4 · Undefined".to_owned(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn fill_order_label(value: Option<u16>) -> String {
    match value {
        Some(1) | None => "1 · Most-significant bit first".to_owned(),
        Some(2) => "2 · Least-significant bit first".to_owned(),
        Some(value) => value.to_string(),
    }
}

fn orientation_label(value: Option<u16>) -> String {
    match value {
        None | Some(1) => "1 · Top-left".to_owned(),
        Some(2) => "2 · Top-right".to_owned(),
        Some(3) => "3 · Bottom-right".to_owned(),
        Some(4) => "4 · Bottom-left".to_owned(),
        Some(5) => "5 · Left-top".to_owned(),
        Some(6) => "6 · Right-top".to_owned(),
        Some(7) => "7 · Right-bottom".to_owned(),
        Some(8) => "8 · Left-bottom".to_owned(),
        Some(value) => value.to_string(),
    }
}

fn ink_set_label(value: Option<u16>) -> String {
    match value {
        Some(1) => "1 · CMYK".to_owned(),
        Some(2) => "2 · Not CMYK".to_owned(),
        Some(value) => value.to_string(),
        None => "Not present".to_owned(),
    }
}

fn planar_label(value: u16) -> String {
    match value {
        1 => "1 · Chunky / contiguous".to_owned(),
        2 => "2 · Planar / separate".to_owned(),
        other => format!("{other}"),
    }
}

fn compression_label(value: Option<u16>) -> String {
    match value {
        Some(1) => "1 · None".to_owned(),
        Some(5) => "5 · LZW".to_owned(),
        Some(7) => "7 · JPEG".to_owned(),
        Some(8) => "8 · Deflate".to_owned(),
        Some(32773) => "32773 · PackBits".to_owned(),
        Some(32946) => "32946 · Deflate".to_owned(),
        Some(value) => format!("{value}"),
        None => "Not present".to_owned(),
    }
}

fn predictor_label(value: Option<u16>) -> String {
    match value {
        Some(1) => "1 · None".to_owned(),
        Some(2) => "2 · Horizontal differencing".to_owned(),
        Some(3) => "3 · Floating point".to_owned(),
        Some(value) => format!("{value}"),
        None => "Not present".to_owned(),
    }
}

fn extra_sample_label(value: u16) -> &'static str {
    match value {
        0 => "Unspecified",
        1 => "Associated alpha",
        2 => "Unassociated alpha",
        _ => "Unknown",
    }
}

fn format_bytes(bytes: u128) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    const TIB: f64 = GIB * 1024.0;
    let value = bytes as f64;
    if value >= TIB {
        format!("{:.2} TiB ({} bytes)", value / TIB, bytes)
    } else if value >= GIB {
        format!("{:.2} GiB ({} bytes)", value / GIB, bytes)
    } else if value >= MIB {
        format!("{:.2} MiB ({} bytes)", value / MIB, bytes)
    } else if value >= KIB {
        format!("{:.2} KiB ({} bytes)", value / KIB, bytes)
    } else {
        format!("{bytes} bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_tag_labels_are_human_readable() {
        assert!(photometric_label(Some(5)).contains("CMYK"));
        assert!(compression_label(Some(5)).contains("LZW"));
        assert!(planar_label(2).contains("Planar"));
        assert!(sample_format_label(&[1]).contains("Unsigned"));
    }

    #[test]
    fn photoshop_resource_parser_is_bounded_and_reads_valid_block() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"8BIM");
        bytes.extend_from_slice(&0x040Fu16.to_be_bytes());
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(&4u32.to_be_bytes());
        bytes.extend_from_slice(&[1, 2, 3, 4]);
        let resources = parse_photoshop_resources(&bytes);
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].id, 0x040F);
        assert_eq!(resources[0].data_len, 4);
    }

    #[test]
    fn ink_names_split_nul_separated_values() {
        assert_eq!(
            parse_ink_names("Cyan\0Spot Red\0".into()),
            vec!["Cyan", "Spot Red"]
        );
    }
}
