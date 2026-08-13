use std::fs::{self, File};
use std::io::BufWriter;
use std::path::PathBuf;

use tiff::encoder::{Compression, Predictor, TiffEncoder, colortype};
use tiff::tags::{ExtraSamples, Tag};

use crate::export;
use crate::model::ShadeProject;
use crate::tiff_io::{self, ColorModel};

fn temp_paths(label: &str) -> (PathBuf, PathBuf) {
    let unique = format!(
        "shade-conformance-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    (
        std::env::temp_dir().join(format!("{unique}-source.tif")),
        std::env::temp_dir().join(format!("{unique}-export.tif")),
    )
}

fn identity_project(path: &std::path::Path) -> ShadeProject {
    let decoded = tiff_io::decode_full(path).unwrap();
    let mut project = ShadeProject::default();
    project.ensure_channels(&decoded.metadata.channel_names);
    project
}

fn cleanup(source: PathBuf, output: PathBuf) {
    let _ = fs::remove_file(source);
    let _ = fs::remove_file(output);
}

fn export_identity(source: &std::path::Path, output: &std::path::Path) {
    let project = identity_project(source);
    export::export_face_with_progress(source, output, &project, 220.0, |_, _| {}).unwrap();
}

fn write_rgb8_source(path: &std::path::Path, compression: Compression, predictor: bool) -> Vec<u8> {
    let pixels = vec![
        1u8, 2, 3, 10, 20, 30, 40, 50, 60, 70, 80, 90, 120, 130, 140, 200, 210, 220,
    ];
    let file = File::create(path).unwrap();
    let writer = BufWriter::new(file);
    let mut encoder = TiffEncoder::new(writer).unwrap().with_compression(compression);
    if predictor {
        encoder = encoder.with_predictor(Predictor::Horizontal);
    }
    let mut image = encoder.new_image::<colortype::RGB8>(3, 2).unwrap();
    image.rows_per_strip(1).unwrap();
    image.write_data(&pixels).unwrap();
    pixels
}

fn assert_rgb8_compression(compression: Compression, expected_tag: u16, label: &str) {
    let (source, output) = temp_paths(label);
    write_rgb8_source(&source, compression, false);
    let source_decoded = tiff_io::decode_full(&source).unwrap();
    export_identity(&source, &output);
    let exported = tiff_io::decode_full(&output).unwrap();
    assert_eq!(exported.metadata.color_model, ColorModel::Rgb);
    assert_eq!(exported.metadata.bit_depth, 8);
    assert_eq!(exported.metadata.compression, Some(expected_tag));
    assert_eq!(exported.samples, source_decoded.samples);
    cleanup(source, output);
}

#[test]
fn identity_export_preserves_supported_lossless_compressions() {
    assert_rgb8_compression(Compression::Uncompressed, 1, "uncompressed");
    assert_rgb8_compression(Compression::Lzw, 5, "lzw");
    assert_rgb8_compression(Compression::Packbits, 32773, "packbits");
    assert_rgb8_compression(
        Compression::Deflate(tiff::encoder::DeflateLevel::Balanced),
        8,
        "deflate",
    );
}

#[test]
fn identity_export_preserves_horizontal_predictor_for_base_rgb() {
    let (source, output) = temp_paths("rgb-predictor");
    write_rgb8_source(&source, Compression::Lzw, true);
    let source_decoded = tiff_io::decode_full(&source).unwrap();
    assert_eq!(source_decoded.metadata.predictor, Some(2));
    export_identity(&source, &output);
    let exported = tiff_io::decode_full(&output).unwrap();
    assert_eq!(exported.metadata.compression, Some(5));
    assert_eq!(exported.metadata.predictor, Some(2));
    assert_eq!(exported.samples, source_decoded.samples);
    cleanup(source, output);
}

#[test]
fn identity_export_preserves_16bit_cmyk_samples() {
    let (source, output) = temp_paths("cmyk16");
    let pixels = vec![
        0u16, 1000, 2000, 3000, 4000, 5000, 6000, 7000,
        12000, 24000, 36000, 48000, 65535, 50000, 32000, 16000,
    ];
    {
        let file = File::create(&source).unwrap();
        let mut encoder = TiffEncoder::new(BufWriter::new(file))
            .unwrap()
            .with_compression(Compression::Lzw);
        let mut image = encoder.new_image::<colortype::CMYK16>(2, 2).unwrap();
        image.rows_per_strip(1).unwrap();
        image.write_data(&pixels).unwrap();
    }
    let source_decoded = tiff_io::decode_full(&source).unwrap();
    assert_eq!(source_decoded.metadata.bit_depth, 16);
    assert_eq!(source_decoded.metadata.color_model, ColorModel::Cmyk);
    export_identity(&source, &output);
    let exported = tiff_io::decode_full(&output).unwrap();
    assert_eq!(exported.metadata.bit_depth, 16);
    assert_eq!(exported.samples, source_decoded.samples);
    cleanup(source, output);
}

fn photoshop_resource(id: u16, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"8BIM");
    out.extend_from_slice(&id.to_be_bytes());
    // Empty Pascal resource name, padded to an even byte count.
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 != 0 {
        out.push(0);
    }
    out
}

fn production_shaped_photoshop_resources() -> Vec<u8> {
    // Resource 1006: two Pascal alpha/spot names.
    let alpha_names = [
        6u8, b'p', b'u', b'r', b'p', b'o', b'l',
        6u8, b'b', b'g', b'r', b'e', b'e', b'n',
    ];
    // Resource 1077: same two 13-byte DisplayInfo records used by our production fixture.
    let display_info = [
        0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0xd4, 0xd4, 0xc7, 0xc7, 0xff, 0xff, 0x00,
        0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0x6e, 0x6e, 0xff, 0xff, 0xff, 0xff, 0x00,
        0x00, 0x00, 0x00, 0x02,
    ];
    let mut resources = photoshop_resource(1006, &alpha_names);
    resources.extend_from_slice(&photoshop_resource(1077, &display_info));
    resources
}

#[test]
fn identity_export_preserves_spot_names_icc_photoshop_resources_and_dpi() {
    let (source, output) = temp_paths("spot-metadata");
    let pixels = vec![
        1u8, 2, 3, 4, 5, 6, 10, 20, 30, 40, 50, 60,
        70, 80, 90, 100, 110, 120, 130, 140, 150, 160, 170, 180,
    ];
    let icc = (0u16..256).map(|value| value as u8).collect::<Vec<_>>();
    let resources = production_shaped_photoshop_resources();
    let image_source_data = b"Photoshop ImageSourceData regression payload".to_vec();
    {
        let file = File::create(&source).unwrap();
        let mut encoder = TiffEncoder::new(BufWriter::new(file))
            .unwrap()
            .with_compression(Compression::Lzw);
        let mut image = encoder.new_image::<colortype::CMYK8>(2, 2).unwrap();
        image
            .extra_samples(&[ExtraSamples::Unspecified, ExtraSamples::Unspecified])
            .unwrap();
        image.rows_per_strip(1).unwrap();
        image.x_resolution(crate::dpi::rational(220.0));
        image.y_resolution(crate::dpi::rational(220.0));
        image.encoder().write_tag(Tag::ResolutionUnit, 2u16).unwrap();
        image.encoder().write_tag(Tag::Orientation, 1u16).unwrap();
        image.encoder().write_tag(Tag::IccProfile, icc.as_slice()).unwrap();
        image
            .encoder()
            .write_tag(Tag::Unknown(34377), resources.as_slice())
            .unwrap();
        image
            .encoder()
            .write_tag(Tag::Unknown(37724), image_source_data.as_slice())
            .unwrap();
        image.write_data(&pixels).unwrap();
    }

    let source_decoded = tiff_io::decode_full(&source).unwrap();
    assert_eq!(source_decoded.metadata.samples_per_pixel, 6);
    assert_eq!(source_decoded.metadata.channel_names[4], "purpol");
    assert_eq!(source_decoded.metadata.channel_names[5], "bgreen");
    export_identity(&source, &output);
    let exported = tiff_io::decode_full(&output).unwrap();

    assert_eq!(exported.samples, source_decoded.samples);
    assert_eq!(exported.metadata.samples_per_pixel, 6);
    assert_eq!(exported.metadata.channel_names, source_decoded.metadata.channel_names);
    assert_eq!(exported.metadata.icc_profile.as_deref(), Some(icc.as_slice()));
    assert_eq!(
        exported.metadata.photoshop_resources.as_deref(),
        Some(resources.as_slice())
    );
    assert_eq!(
        exported.metadata.photoshop_image_source_data.as_deref(),
        Some(image_source_data.as_slice())
    );
    assert_eq!(exported.metadata.orientation, Some(1));
    assert_eq!(exported.metadata.compression, Some(5));

    let source_dpi = crate::dpi::read_dpi(&source, 220.0);
    let output_dpi = crate::dpi::read_dpi(&output, 220.0);
    assert!((source_dpi.dpi_x - output_dpi.dpi_x).abs() < 0.01);
    assert!((source_dpi.dpi_y - output_dpi.dpi_y).abs() < 0.01);
    assert!(!output_dpi.used_default);
    cleanup(source, output);
}
