from pathlib import Path
import re

ROOT = Path('.')

def read(path):
    return (ROOT / path).read_text(encoding='utf-8')

def write(path, text):
    (ROOT / path).write_text(text, encoding='utf-8', newline='\n')

def replace_once(text, old, new, label):
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f'{label}: expected 1 match, found {count}')
    return text.replace(old, new, 1)

def regex_once(text, pattern, replacement, label):
    new, count = re.subn(pattern, replacement, text, count=1, flags=re.S)
    if count != 1:
        raise RuntimeError(f'{label}: expected 1 match, found {count}')
    return new

cargo = read('Cargo.toml')
cargo = replace_once(cargo, 'version = "0.11.1"', 'version = "0.12.0"', 'Cargo version')
write('Cargo.toml', cargo)

lock = read('Cargo.lock')
lock = replace_once(
    lock,
    'name = "windows-shade-editor"\nversion = "0.11.1"',
    'name = "windows-shade-editor"\nversion = "0.12.0"',
    'Cargo.lock version',
)
write('Cargo.lock', lock)

src = read('src/export_v6.rs')
src = replace_once(
    src,
    'use std::io::{BufWriter, Write};',
    'use std::io::{BufWriter, Read, Write};',
    'Read import',
)

full_writer = '''    progress(0.88, "Writing TIFF");
    match decoded.metadata.bit_depth {
        8 => {
            let data = output
                .into_iter()
                .map(|value| (value >> 8) as u8)
                .collect::<Vec<_>>();
            write_tiff_pixels(
                source,
                destination,
                &decoded.metadata,
                dpi_info,
                None,
                OutputPixels::U8(&data),
            )?;
        }
        16 => {
            write_tiff_pixels(
                source,
                destination,
                &decoded.metadata,
                dpi_info,
                None,
                OutputPixels::U16(&output),
            )?;
        }
        depth => {
            return Err(format!(
                "Unsupported export bit depth/color model: {depth}-bit."
            ));
        }
    }

    progress(1.0, "Export complete");'''
src = regex_once(
    src,
    r'    progress\(0\.88, "Writing TIFF"\);\n.*?\n    progress\(1\.0, "Export complete"\);',
    full_writer,
    'full-image TIFF writer',
)

stream_writer = '''        match metadata.bit_depth {
            8 => {
                write_tiff_pixels(
                    source,
                    destination,
                    metadata,
                    dpi_info,
                    Some(stream.rows_per_strip),
                    OutputPixels::U8(&mmap[..]),
                )?;
            }
            16 => {
                let data = mmap_as_u16(&mmap)?;
                write_tiff_pixels(
                    source,
                    destination,
                    metadata,
                    dpi_info,
                    Some(stream.rows_per_strip),
                    OutputPixels::U16(data),
                )?;
            }
            depth => {
                return Err(format!(
                    "Unsupported export bit depth/color model: {depth}-bit."
                ));
            }
        }

        progress(1.0, "Export complete");'''
src = regex_once(
    src,
    r'        let file =\n            File::create\(destination\).*?\n        progress\(1\.0, "Export complete"\);',
    stream_writer,
    'streaming TIFF writer',
)

encoder_helpers = r'''#[derive(Clone, Copy)]
enum OutputPixels<'a> {
    U8(&'a [u8]),
    U16(&'a [u16]),
}

// Classic TIFF uses 32-bit offsets. Keep a conservative margin below the
// absolute 4 GiB address limit so strip tables, metadata, ICC/Photoshop
// resources and encoder overhead cannot push a nominally-safe image over it.
const CLASSIC_TIFF_SAFE_RAW_BYTES: u64 = 4_000_000_000;

fn source_is_bigtiff(source: &Path) -> Result<bool, String> {
    let mut file = File::open(source)
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

fn raw_image_bytes(width: u32, height: u32, channels: usize, bit_depth: u8) -> Option<u64> {
    let bytes_per_sample = u64::from(bit_depth / 8);
    u64::from(width)
        .checked_mul(u64::from(height))?
        .checked_mul(channels as u64)?
        .checked_mul(bytes_per_sample)
}

fn layout_requires_bigtiff_values(width: u32, height: u32, channels: usize, bit_depth: u8) -> bool {
    raw_image_bytes(width, height, channels, bit_depth)
        .map(|bytes| bytes >= CLASSIC_TIFF_SAFE_RAW_BYTES)
        .unwrap_or(true)
}

fn should_write_bigtiff(source: &Path, metadata: &TiffMetadata) -> Result<bool, String> {
    Ok(source_is_bigtiff(source)?
        || layout_requires_bigtiff_values(
            metadata.width,
            metadata.height,
            metadata.samples_per_pixel,
            metadata.bit_depth,
        ))
}

fn configure_tiff_encoder<W, K>(
    mut encoder: TiffEncoder<W, K>,
    metadata: &TiffMetadata,
) -> TiffEncoder<W, K>
where
    W: std::io::Write + std::io::Seek,
    K: tiff::encoder::TiffKind,
{
    let compression = match metadata.compression {
        Some(1) => Compression::Uncompressed,
        Some(5) => Compression::Lzw,
        Some(8 | 32946) => Compression::Deflate(tiff::encoder::DeflateLevel::Balanced),
        Some(32773) => Compression::Packbits,
        _ => Compression::Lzw,
    };
    encoder = encoder.with_compression(compression);
    if metadata.predictor == Some(2) && metadata.samples_per_pixel == metadata.base_channel_count {
        encoder = encoder.with_predictor(Predictor::Horizontal);
    }
    encoder
}

fn write_tiff_pixels(
    source: &Path,
    destination: &Path,
    metadata: &TiffMetadata,
    dpi_info: DpiInfo,
    rows_per_strip: Option<u32>,
    pixels: OutputPixels<'_>,
) -> Result<(), String> {
    let file = File::create(destination)
        .map_err(|err| format!("Cannot create export TIFF: {err}"))?;
    let writer = BufWriter::new(file);
    if should_write_bigtiff(source, metadata)? {
        let encoder = TiffEncoder::new_big(writer)
            .map_err(|err| format!("Cannot initialize BigTIFF encoder: {err}"))?;
        let mut encoder = configure_tiff_encoder(encoder, metadata);
        write_tiff_with_encoder(&mut encoder, metadata, dpi_info, rows_per_strip, pixels)
    } else {
        let encoder = TiffEncoder::new(writer)
            .map_err(|err| format!("Cannot initialize TIFF encoder: {err}"))?;
        let mut encoder = configure_tiff_encoder(encoder, metadata);
        write_tiff_with_encoder(&mut encoder, metadata, dpi_info, rows_per_strip, pixels)
    }
}

fn write_tiff_with_encoder<W, K>(
    encoder: &mut TiffEncoder<W, K>,
    metadata: &TiffMetadata,
    dpi_info: DpiInfo,
    rows_per_strip: Option<u32>,
    pixels: OutputPixels<'_>,
) -> Result<(), String>
where
    W: std::io::Write + std::io::Seek,
    K: tiff::encoder::TiffKind,
{
    let channels = metadata.samples_per_pixel;
    match (metadata.color_model, metadata.bit_depth, pixels) {
        (ColorModel::Rgb, 8, OutputPixels::U8(data)) => {
            let mut image = encoder
                .new_image::<colortype::RGB8>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create RGB 8-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 3, metadata, dpi_info)?;
            if let Some(rows) = rows_per_strip {
                image.rows_per_strip(rows)
                    .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            }
            image.write_data(data)
                .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (ColorModel::Rgb, 16, OutputPixels::U16(data)) => {
            let mut image = encoder
                .new_image::<colortype::RGB16>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create RGB 16-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 3, metadata, dpi_info)?;
            if let Some(rows) = rows_per_strip {
                image.rows_per_strip(rows)
                    .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            }
            image.write_data(data)
                .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (ColorModel::Cmyk, 8, OutputPixels::U8(data)) => {
            let mut image = encoder
                .new_image::<colortype::CMYK8>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create CMYK 8-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 4, metadata, dpi_info)?;
            if let Some(rows) = rows_per_strip {
                image.rows_per_strip(rows)
                    .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            }
            image.write_data(data)
                .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (ColorModel::Cmyk, 16, OutputPixels::U16(data)) => {
            let mut image = encoder
                .new_image::<colortype::CMYK16>(metadata.width, metadata.height)
                .map_err(|err| format!("Cannot create CMYK 16-bit TIFF image: {err}"))?;
            configure_extras_and_metadata(&mut image, channels, 4, metadata, dpi_info)?;
            if let Some(rows) = rows_per_strip {
                image.rows_per_strip(rows)
                    .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
            }
            image.write_data(data)
                .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
        }
        (_, depth, _) => {
            return Err(format!(
                "Unsupported export bit depth/color model: {depth}-bit."
            ));
        }
    }
    Ok(())
}
'''
src = regex_once(
    src,
    r'fn make_tiff_encoder\(.*?\n}\n\n(?=fn temporary_spool_path)',
    encoder_helpers + '\n',
    'encoder helper block',
)

tests = r'''

    #[test]
    fn identity_export_preserves_bigtiff_container() {
        let unique = format!(
            "shade-bigtiff-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let source = std::env::temp_dir().join(format!("{unique}-source.tif"));
        let destination = std::env::temp_dir().join(format!("{unique}-export.tif"));
        let pixels = vec![
            1u8, 2, 3, 4,
            10, 20, 30, 40,
            50, 60, 70, 80,
            90, 100, 110, 120,
        ];
        {
            let file = File::create(&source).unwrap();
            let mut tiff = TiffEncoder::new_big(BufWriter::new(file))
                .unwrap()
                .with_compression(Compression::Lzw);
            let mut image = tiff.new_image::<colortype::CMYK8>(2, 2).unwrap();
            image.write_data(&pixels).unwrap();
        }
        assert!(source_is_bigtiff(&source).unwrap());
        let decoded_source = decode_full(&source).unwrap();
        let mut project = ShadeProject::default();
        project.ensure_channels(&decoded_source.metadata.channel_names);
        export_face_with_progress(&source, &destination, &project, 220.0, |_, _| {}).unwrap();
        assert!(source_is_bigtiff(&destination).unwrap());
        let decoded_output = decode_full(&destination).unwrap();
        assert_eq!(decoded_output.samples, decoded_source.samples);
        assert_eq!(decoded_output.metadata.color_model, ColorModel::Cmyk);
        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_file(destination);
    }

    #[test]
    fn large_layout_selects_bigtiff_without_allocating_pixels() {
        assert!(!layout_requires_bigtiff_values(720, 1280, 6, 8));
        assert!(!layout_requires_bigtiff_values(20_000, 20_000, 4, 8));
        assert!(layout_requires_bigtiff_values(40_000, 40_000, 4, 8));
        assert!(layout_requires_bigtiff_values(30_000, 30_000, 4, 16));
    }
'''
pos = src.rfind('\n}')
if pos < 0:
    raise RuntimeError('Cannot find streaming_tests closing brace')
src = src[:pos] + tests + src[pos:]
write('src/export_v6.rs', src)

notes = read('RELEASE_NOTES.md')
notes = '''# Shade Editor v0.12.0\n\n- Preserves BigTIFF container format when exporting a BigTIFF source.\n- Automatically switches to BigTIFF when the uncompressed output layout approaches the 32-bit offset ceiling of classic TIFF.\n- Uses the same Levels / Curve / Channel Mixer / Test Code and Photoshop metadata preservation pipeline for classic TIFF and BigTIFF outputs.\n- Adds regression coverage for a real BigTIFF identity export plus large-layout format selection without allocating a huge test image.\n- `.shade` schema remains v9.\n\n''' + notes
write('RELEASE_NOTES.md', notes)

roadmap = read('docs/ROADMAP.md')
needle = '## Backend follow-up\n\n'
replacement = '''## Backend follow-up\n\n- Production-test BigTIFF export on >4 GiB ceramic artwork and confirm Photoshop/RIP acceptance.\n'''
roadmap = replace_once(roadmap, needle, replacement, 'roadmap backend heading')
write('docs/ROADMAP.md', roadmap)
