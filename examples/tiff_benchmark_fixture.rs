#![cfg(windows)]

use std::env;
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use windows_shade_editor::dpi;
use windows_shade_editor::tiff_io::{self, ChunkStorage};
use windows_shade_editor::tiff_output::{self, TiffLayout};

const HASH_BUFFER_BYTES: usize = 1024 * 1024;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let path = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| "Usage: cargo run --release --example tiff_benchmark_fixture -- <file.tif>".to_owned())?;
    if !path.is_file() {
        return Err(format!("TIFF benchmark fixture does not exist: {}", path.display()));
    }

    let manifest = build_manifest(&path)?;
    let output = serde_json::to_string_pretty(&manifest)
        .map_err(|err| format!("Cannot serialize TIFF benchmark fixture manifest: {err}"))?;
    println!("{output}");
    Ok(())
}

fn build_manifest(path: &Path) -> Result<serde_json::Value, String> {
    let file_bytes = fs::metadata(path)
        .map_err(|err| format!("Cannot stat TIFF benchmark fixture: {err}"))?
        .len();
    let sha256 = sha256_file(path)?;
    let stream = tiff_io::stream_info(path)?;
    let metadata = &stream.metadata;
    let dpi_info = dpi::read_dpi(path, dpi::DEFAULT_DPI);
    let bigtiff = tiff_output::source_is_bigtiff(path)?;
    let storage = match stream.storage {
        ChunkStorage::Strips => "strips",
        ChunkStorage::Tiles => "tiles",
    };
    let raw_logical_bytes = tiff_output::raw_image_bytes(TiffLayout {
        width: metadata.width,
        height: metadata.height,
        channels: metadata.samples_per_pixel,
        bit_depth: metadata.bit_depth,
    });

    Ok(serde_json::json!({
        "schema_version": 1,
        "path": path.to_string_lossy(),
        "file_bytes": file_bytes,
        "sha256": sha256,
        "width": metadata.width,
        "height": metadata.height,
        "bit_depth": metadata.bit_depth,
        "samples_per_pixel": metadata.samples_per_pixel,
        "base_channel_count": metadata.base_channel_count,
        "color_model": metadata.color_model.title(),
        "non_cmyk_separated": metadata.non_cmyk_separated,
        "channel_names": &metadata.channel_names,
        "compression_tag": metadata.compression,
        "predictor_tag": metadata.predictor,
        "orientation_tag": metadata.orientation,
        "icc_profile_bytes": metadata.icc_profile.as_ref().map_or(0, Vec::len),
        "photoshop_resources_bytes": metadata.photoshop_resources.as_ref().map_or(0, Vec::len),
        "photoshop_image_source_data_bytes": metadata.photoshop_image_source_data.as_ref().map_or(0, Vec::len),
        "bigtiff": bigtiff,
        "raw_logical_bytes": raw_logical_bytes,
        "storage": storage,
        "planar_configuration": stream.planar_configuration,
        "rows_per_strip": stream.rows_per_strip,
        "strip_count": stream.strip_count,
        "chunk_width": stream.chunk_width,
        "chunk_height": stream.chunk_height,
        "coding_unit_count": stream.coding_unit_count,
        "streamable": stream.streamable,
        "row_streamable": stream.row_streamable,
        "dpi": {
            "dpi_x": dpi_info.dpi_x,
            "dpi_y": dpi_info.dpi_y,
            "raw_x": dpi_info.raw_x,
            "raw_y": dpi_info.raw_y,
            "resolution_unit": dpi_info.unit,
            "has_physical_resolution": dpi_info.has_physical_resolution,
            "used_default": dpi_info.used_default
        }
    }))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let file = File::open(path).map_err(|err| format!("Cannot hash TIFF benchmark fixture: {err}"))?;
    let mut reader = BufReader::with_capacity(HASH_BUFFER_BYTES, file);
    let mut buffer = vec![0u8; HASH_BUFFER_BYTES];
    let mut hasher = Sha256::new();
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|err| format!("Cannot read TIFF benchmark fixture for SHA-256: {err}"))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufWriter;
    use tiff::encoder::{colortype, Compression, TiffEncoder};

    fn temp_tiff_path() -> PathBuf {
        let unique = format!(
            "shade-benchmark-fixture-{}-{}.tif",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::env::temp_dir().join(unique)
    }

    #[test]
    fn records_stable_identity_and_stream_topology() {
        let path = temp_tiff_path();
        let pixels = [1u8, 2, 3, 4, 5, 6];
        {
            let file = File::create(&path).unwrap();
            let mut encoder = TiffEncoder::new(BufWriter::new(file))
                .unwrap()
                .with_compression(Compression::Lzw);
            let mut image = encoder.new_image::<colortype::RGB8>(2, 1).unwrap();
            image.rows_per_strip(1).unwrap();
            image.write_data(&pixels).unwrap();
        }

        let manifest = build_manifest(&path).unwrap();
        assert_eq!(manifest["schema_version"].as_u64(), Some(1));
        assert_eq!(manifest["width"].as_u64(), Some(2));
        assert_eq!(manifest["height"].as_u64(), Some(1));
        assert_eq!(manifest["bit_depth"].as_u64(), Some(8));
        assert_eq!(manifest["samples_per_pixel"].as_u64(), Some(3));
        assert_eq!(manifest["base_channel_count"].as_u64(), Some(3));
        assert_eq!(manifest["color_model"].as_str(), Some("RGB"));
        assert_eq!(manifest["compression_tag"].as_u64(), Some(5));
        assert_eq!(manifest["storage"].as_str(), Some("strips"));
        assert_eq!(manifest["planar_configuration"].as_u64(), Some(1));
        assert_eq!(manifest["rows_per_strip"].as_u64(), Some(1));
        assert_eq!(manifest["row_streamable"].as_bool(), Some(true));
        assert_eq!(manifest["bigtiff"].as_bool(), Some(false));
        assert_eq!(manifest["raw_logical_bytes"].as_u64(), Some(6));
        assert_eq!(manifest["sha256"].as_str().unwrap().len(), 64);
        assert!(manifest["file_bytes"].as_u64().unwrap() > 0);

        let _ = fs::remove_file(path);
    }
}
