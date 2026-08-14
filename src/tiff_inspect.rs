use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use tiff::decoder::{Decoder, Limits};
use tiff::tags::Tag;

use crate::color_management;
use crate::dpi;
use crate::tiff_io::{self, ChunkStorage};

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
        .with_limits(Limits::unlimited());

    let photometric = decoder
        .find_tag_unsigned::<u16>(Tag::PhotometricInterpretation)
        .ok()
        .flatten();
    let extra_samples = decoder
        .get_tag_u64_vec(Tag::ExtraSamples)
        .unwrap_or_default()
        .into_iter()
        .map(|value| value as u16)
        .collect::<Vec<_>>();

    let container = tiff_container(path)?;
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
    line!("File size: {}", format_bytes(file_size as u128));
    line!("Dimensions: {} × {} px", metadata.width, metadata.height);
    line!("Bits per sample: {}", metadata.bit_depth);
    line!(
        "PhotometricInterpretation: {}",
        photometric_label(photometric)
    );
    line!(
        "PlanarConfiguration: {}",
        planar_label(stream.planar_configuration)
    );
    line!(
        "Compression: {}",
        compression_label(metadata.compression)
    );
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
    line!(
        "Photoshop Image Resources (34377): {}",
        if photoshop_bytes == 0 {
            "None".to_owned()
        } else {
            format_bytes(photoshop_bytes as u128)
        }
    );
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
    line!("");
    line!(
        "Declared Spot order: {}",
        if spot_names.is_empty() {
            "None".to_owned()
        } else {
            spot_names.join(" → ")
        }
    );

    Ok(TiffInspection {
        path: path.to_path_buf(),
        report,
    })
}

fn tiff_container(path: &Path) -> Result<&'static str, String> {
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
    match magic {
        42 => Ok("Classic TIFF"),
        43 => Ok("BigTIFF"),
        other => Err(format!("Unknown TIFF magic value {other}.")),
    }
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
    }
}
