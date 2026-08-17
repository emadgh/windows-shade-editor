use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use tiff::encoder::{colortype, Compression, TiffEncoder, TiffValue};
use tiff::tags::{ExtraSamples, Tag, Type};

use crate::{dpi, safe_fs, tiff_io};

const CLASSIC_TIFF_SAFE_RAW_BYTES: u64 = 4_000_000_000;

struct TiffAsciiBytes<'a>(&'a [u8]);

impl TiffValue for TiffAsciiBytes<'_> {
    const BYTE_LEN: u8 = 1;
    const FIELD_TYPE: Type = Type::ASCII;

    fn count(&self) -> usize {
        self.0.len()
    }

    fn data(&self) -> Cow<'_, [u8]> {
        Cow::Borrowed(self.0)
    }
}

#[derive(Clone, Debug)]
pub struct ConversionTiffSpec<'a> {
    pub width: u32,
    pub height: u32,
    pub channel_names: &'a [String],
    pub target_icc: &'a [u8],
    pub dpi_x: f64,
    pub dpi_y: f64,
    pub orientation: Option<u16>,
    pub rows_per_strip: u32,
    pub force_bigtiff: bool,
}

pub fn write_conversion_tiff_u8_atomic<F>(
    destination: &Path,
    spec: &ConversionTiffSpec<'_>,
    render_strip: F,
) -> Result<(), String>
where
    F: FnMut(u32, u32, &mut [u8]) -> Result<(), String>,
{
    validate_spec(destination, spec, 8)?;
    write_atomic(destination, spec, 8, |staged| {
        write_u8(staged, spec, render_strip)
    })
}

pub fn write_conversion_tiff_u16_atomic<F>(
    destination: &Path,
    spec: &ConversionTiffSpec<'_>,
    render_strip: F,
) -> Result<(), String>
where
    F: FnMut(u32, u32, &mut [u16]) -> Result<(), String>,
{
    validate_spec(destination, spec, 16)?;
    write_atomic(destination, spec, 16, |staged| {
        write_u16(staged, spec, render_strip)
    })
}

fn write_atomic<F>(
    destination: &Path,
    spec: &ConversionTiffSpec<'_>,
    bit_depth: u8,
    write_staged: F,
) -> Result<(), String>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|err| format!("Cannot create conversion output folder: {err}"))?;
    let staged = staged_path(destination)?;
    if staged.exists() {
        fs::remove_file(&staged)
            .map_err(|err| format!("Cannot remove stale conversion temp file: {err}"))?;
    }

    let result = (|| {
        write_staged(&staged)?;
        verify_staged(&staged, spec, bit_depth)?;
        safe_fs::commit_staged_file(&staged, destination)
    })();
    if result.is_err() && staged.exists() {
        let _ = fs::remove_file(&staged);
    }
    result
}

fn write_u8<F>(staged: &Path, spec: &ConversionTiffSpec<'_>, render_strip: F) -> Result<(), String>
where
    F: FnMut(u32, u32, &mut [u8]) -> Result<(), String>,
{
    let file = File::create(staged)
        .map_err(|err| format!("Cannot create staged conversion TIFF: {err}"))?;
    let writer = BufWriter::new(file);
    if should_write_bigtiff(spec, 8) {
        let encoder = TiffEncoder::new_big(writer)
            .map_err(|err| format!("Cannot initialize conversion BigTIFF: {err}"))?;
        write_u8_with_encoder(encoder, spec, render_strip)
    } else {
        let encoder = TiffEncoder::new(writer)
            .map_err(|err| format!("Cannot initialize conversion TIFF: {err}"))?;
        write_u8_with_encoder(encoder, spec, render_strip)
    }
}

fn write_u16<F>(staged: &Path, spec: &ConversionTiffSpec<'_>, render_strip: F) -> Result<(), String>
where
    F: FnMut(u32, u32, &mut [u16]) -> Result<(), String>,
{
    let file = File::create(staged)
        .map_err(|err| format!("Cannot create staged conversion TIFF: {err}"))?;
    let writer = BufWriter::new(file);
    if should_write_bigtiff(spec, 16) {
        let encoder = TiffEncoder::new_big(writer)
            .map_err(|err| format!("Cannot initialize conversion BigTIFF: {err}"))?;
        write_u16_with_encoder(encoder, spec, render_strip)
    } else {
        let encoder = TiffEncoder::new(writer)
            .map_err(|err| format!("Cannot initialize conversion TIFF: {err}"))?;
        write_u16_with_encoder(encoder, spec, render_strip)
    }
}

fn write_u8_with_encoder<W, K, F>(
    encoder: TiffEncoder<W, K>,
    spec: &ConversionTiffSpec<'_>,
    mut render_strip: F,
) -> Result<(), String>
where
    W: std::io::Write + std::io::Seek,
    K: tiff::encoder::TiffKind,
    F: FnMut(u32, u32, &mut [u8]) -> Result<(), String>,
{
    // image-tiff only activates compressed writers through `write_data`, which
    // requires the complete sample slice. The conversion path deliberately uses
    // bounded strip callbacks, so this first writer uses uncompressed strips.
    // A later local mmap spool may add LZW without regressing bounded RAM.
    let mut encoder = encoder.with_compression(Compression::Uncompressed);
    let mut image = encoder
        .new_image::<colortype::CMYK8>(spec.width, spec.height)
        .map_err(|err| format!("Cannot create 8-bit separated TIFF image: {err}"))?;
    configure_image(&mut image, spec)?;
    write_strips(&mut image, spec, &mut render_strip)?;
    image
        .finish()
        .map_err(|err| format!("Cannot finish 8-bit conversion TIFF: {err}"))
}

fn write_u16_with_encoder<W, K, F>(
    encoder: TiffEncoder<W, K>,
    spec: &ConversionTiffSpec<'_>,
    mut render_strip: F,
) -> Result<(), String>
where
    W: std::io::Write + std::io::Seek,
    K: tiff::encoder::TiffKind,
    F: FnMut(u32, u32, &mut [u16]) -> Result<(), String>,
{
    let mut encoder = encoder.with_compression(Compression::Uncompressed);
    let mut image = encoder
        .new_image::<colortype::CMYK16>(spec.width, spec.height)
        .map_err(|err| format!("Cannot create 16-bit separated TIFF image: {err}"))?;
    configure_image(&mut image, spec)?;
    write_strips(&mut image, spec, &mut render_strip)?;
    image
        .finish()
        .map_err(|err| format!("Cannot finish 16-bit conversion TIFF: {err}"))
}

fn configure_image<W, C, K>(
    image: &mut tiff::encoder::ImageEncoder<'_, W, C, K>,
    spec: &ConversionTiffSpec<'_>,
) -> Result<(), String>
where
    W: std::io::Write + std::io::Seek,
    C: tiff::encoder::colortype::ColorType,
    K: tiff::encoder::TiffKind,
{
    let channel_count = spec.channel_names.len();
    if channel_count > 4 {
        let extras = vec![ExtraSamples::Unspecified; channel_count - 4];
        image
            .extra_samples(&extras)
            .map_err(|err| format!("Cannot configure N-channel samples: {err}"))?;
    }
    image
        .rows_per_strip(spec.rows_per_strip.min(spec.height).max(1))
        .map_err(|err| format!("Cannot configure conversion strip size: {err}"))?;
    image.x_resolution(dpi::rational(spec.dpi_x));
    image.y_resolution(dpi::rational(spec.dpi_y));
    image
        .encoder()
        .write_tag(Tag::ResolutionUnit, 2u16)
        .map_err(|err| format!("Cannot write conversion resolution unit: {err}"))?;
    if let Some(orientation) = spec.orientation {
        image
            .encoder()
            .write_tag(Tag::Orientation, orientation)
            .map_err(|err| format!("Cannot write conversion orientation: {err}"))?;
    }
    image
        .encoder()
        .write_tag(Tag::IccProfile, spec.target_icc)
        .map_err(|err| format!("Cannot embed target ICC: {err}"))?;
    image
        .encoder()
        .write_tag(
            Tag::Unknown(332),
            if channel_count == 4 { 1u16 } else { 2u16 },
        )
        .map_err(|err| format!("Cannot write TIFF InkSet: {err}"))?;
    image
        .encoder()
        .write_tag(Tag::Unknown(334), channel_count as u16)
        .map_err(|err| format!("Cannot write TIFF NumberOfInks: {err}"))?;
    let ink_names = format!(
        "{}\0",
        spec.channel_names
            .iter()
            .map(|name| name.trim())
            .collect::<Vec<_>>()
            .join("\0")
    )
    .into_bytes();
    image
        .encoder()
        .write_tag(Tag::Unknown(333), TiffAsciiBytes(&ink_names))
        .map_err(|err| format!("Cannot write TIFF InkNames: {err}"))?;
    image
        .encoder()
        .write_tag(Tag::Software, "Shade Editor Color Conversion")
        .map_err(|err| format!("Cannot write conversion Software tag: {err}"))?;
    Ok(())
}

fn write_strips<W, C, K, F>(
    image: &mut tiff::encoder::ImageEncoder<'_, W, C, K>,
    spec: &ConversionTiffSpec<'_>,
    render_strip: &mut F,
) -> Result<(), String>
where
    W: std::io::Write + std::io::Seek,
    C: tiff::encoder::colortype::ColorType,
    [C::Inner]: tiff::encoder::TiffValue,
    K: tiff::encoder::TiffKind,
    F: FnMut(u32, u32, &mut [C::Inner]) -> Result<(), String>,
    C::Inner: Default + Clone,
{
    let row_samples = usize::try_from(spec.width)
        .ok()
        .and_then(|width| width.checked_mul(spec.channel_names.len()))
        .ok_or_else(|| "Conversion TIFF row is too large.".to_owned())?;
    let mut start_row = 0u32;
    while image.next_strip_sample_count() > 0 {
        let sample_count = usize::try_from(image.next_strip_sample_count())
            .map_err(|_| "Conversion strip sample count is too large.".to_owned())?;
        if sample_count % row_samples != 0 {
            return Err("Conversion strip does not contain complete rows.".to_owned());
        }
        let row_count = u32::try_from(sample_count / row_samples)
            .map_err(|_| "Conversion strip row count is too large.".to_owned())?;
        let mut samples = vec![C::Inner::default(); sample_count];
        render_strip(start_row, row_count, &mut samples)?;
        image
            .write_strip(&samples)
            .map_err(|err| format!("Cannot write conversion TIFF strip: {err}"))?;
        start_row = start_row.saturating_add(row_count);
    }
    if start_row != spec.height {
        return Err(format!(
            "Conversion TIFF wrote {start_row} rows; expected {}.",
            spec.height
        ));
    }
    Ok(())
}

fn validate_spec(
    destination: &Path,
    spec: &ConversionTiffSpec<'_>,
    bit_depth: u8,
) -> Result<(), String> {
    if spec.width == 0 || spec.height == 0 {
        return Err("Conversion TIFF dimensions must be non-zero.".to_owned());
    }
    if !(4..=12).contains(&spec.channel_names.len()) {
        return Err("Conversion TIFF supports CMYK or 5–12 channels.".to_owned());
    }
    let mut unique = BTreeSet::new();
    for name in spec.channel_names {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.contains('\0') || !trimmed.is_ascii() {
            return Err(
                "Conversion channel names must be non-empty TIFF ASCII and contain no NUL."
                    .to_owned(),
            );
        }
        if !unique.insert(trimmed.to_ascii_lowercase()) {
            return Err(format!("Duplicate conversion channel name '{trimmed}'."));
        }
    }
    if spec.target_icc.is_empty() {
        return Err("Conversion TIFF requires target ICC payload bytes.".to_owned());
    }
    if !spec.dpi_x.is_finite() || !spec.dpi_y.is_finite() || spec.dpi_x <= 0.0 || spec.dpi_y <= 0.0
    {
        return Err("Conversion TIFF DPI must be finite and positive.".to_owned());
    }
    if spec.rows_per_strip == 0 {
        return Err("Conversion TIFF rows-per-strip must be non-zero.".to_owned());
    }
    if spec
        .orientation
        .is_some_and(|value| !(1..=8).contains(&value))
    {
        return Err("Conversion TIFF orientation must be in 1..=8.".to_owned());
    }
    if !matches!(bit_depth, 8 | 16) {
        return Err("Conversion TIFF bit depth must be 8 or 16.".to_owned());
    }
    let extension = destination
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !extension.eq_ignore_ascii_case("tif") && !extension.eq_ignore_ascii_case("tiff") {
        return Err("Conversion output must use .tif or .tiff.".to_owned());
    }
    Ok(())
}

fn verify_staged(
    staged: &Path,
    spec: &ConversionTiffSpec<'_>,
    bit_depth: u8,
) -> Result<(), String> {
    let stream = tiff_io::stream_info(staged)?;
    let metadata = stream.metadata;
    if metadata.width != spec.width
        || metadata.height != spec.height
        || metadata.samples_per_pixel != spec.channel_names.len()
        || metadata.bit_depth != bit_depth
    {
        return Err("Staged conversion TIFF topology verification failed.".to_owned());
    }
    if metadata.icc_profile.as_deref() != Some(spec.target_icc) {
        return Err("Staged conversion TIFF target ICC verification failed.".to_owned());
    }
    let file = File::open(staged)
        .map_err(|err| format!("Cannot verify staged conversion TIFF tags: {err}"))?;
    let mut decoder = tiff::decoder::Decoder::new(file)
        .map_err(|err| format!("Cannot decode staged conversion TIFF tags: {err}"))?;
    let expected_ink_set = if spec.channel_names.len() == 4 { 1 } else { 2 };
    if decoder.get_tag_unsigned::<u16>(Tag::Unknown(332)).ok() != Some(expected_ink_set)
        || decoder.get_tag_unsigned::<u16>(Tag::Unknown(334)).ok()
            != Some(spec.channel_names.len() as u16)
    {
        return Err("Staged conversion TIFF ink-topology tag verification failed.".to_owned());
    }
    let expected_names = format!(
        "{}\0",
        spec.channel_names
            .iter()
            .map(|name| name.trim())
            .collect::<Vec<_>>()
            .join("\0")
    );
    if read_ascii_tag_bytes(staged, 333, expected_names.len())? != expected_names.as_bytes() {
        return Err("Staged conversion TIFF InkNames verification failed.".to_owned());
    }
    Ok(())
}

fn read_ascii_tag_bytes(
    path: &Path,
    wanted_tag: u16,
    expected_len: usize,
) -> Result<Vec<u8>, String> {
    let mut file = File::open(path)
        .map_err(|err| format!("Cannot verify staged conversion TIFF tags: {err}"))?;
    let mut signature = [0u8; 4];
    file.read_exact(&mut signature)
        .map_err(|err| format!("Cannot read staged TIFF header: {err}"))?;
    let little_endian = match &signature[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Err("Staged conversion output has an invalid TIFF byte order.".to_owned()),
    };
    let magic = read_u16([signature[2], signature[3]], little_endian);

    let (first_ifd, bigtiff) = match magic {
        42 => {
            let mut offset = [0u8; 4];
            file.read_exact(&mut offset)
                .map_err(|err| format!("Cannot read staged TIFF IFD offset: {err}"))?;
            (u64::from(read_u32(offset, little_endian)), false)
        }
        43 => {
            let mut header = [0u8; 12];
            file.read_exact(&mut header)
                .map_err(|err| format!("Cannot read staged BigTIFF header: {err}"))?;
            if read_u16([header[0], header[1]], little_endian) != 8
                || read_u16([header[2], header[3]], little_endian) != 0
            {
                return Err(
                    "Staged conversion output has an unsupported BigTIFF header.".to_owned(),
                );
            }
            let mut offset = [0u8; 8];
            offset.copy_from_slice(&header[4..12]);
            (read_u64(offset, little_endian), true)
        }
        _ => return Err(format!("Staged conversion output has TIFF magic {magic}.")),
    };

    file.seek(SeekFrom::Start(first_ifd))
        .map_err(|err| format!("Cannot seek staged conversion TIFF IFD: {err}"))?;
    if bigtiff {
        let mut count = [0u8; 8];
        file.read_exact(&mut count)
            .map_err(|err| format!("Cannot read staged BigTIFF IFD count: {err}"))?;
        let entry_count = read_u64(count, little_endian);
        if entry_count > 65_535 {
            return Err("Staged BigTIFF IFD contains too many entries.".to_owned());
        }
        for _ in 0..entry_count {
            let mut entry = [0u8; 20];
            file.read_exact(&mut entry)
                .map_err(|err| format!("Cannot read staged BigTIFF IFD entry: {err}"))?;
            if read_u16([entry[0], entry[1]], little_endian) == wanted_tag {
                let mut count = [0u8; 8];
                count.copy_from_slice(&entry[4..12]);
                return read_ascii_entry_payload(
                    &mut file,
                    read_u16([entry[2], entry[3]], little_endian),
                    read_u64(count, little_endian),
                    &entry[12..20],
                    little_endian,
                    expected_len,
                );
            }
        }
    } else {
        let mut count = [0u8; 2];
        file.read_exact(&mut count)
            .map_err(|err| format!("Cannot read staged TIFF IFD count: {err}"))?;
        for _ in 0..read_u16(count, little_endian) {
            let mut entry = [0u8; 12];
            file.read_exact(&mut entry)
                .map_err(|err| format!("Cannot read staged TIFF IFD entry: {err}"))?;
            if read_u16([entry[0], entry[1]], little_endian) == wanted_tag {
                let mut count = [0u8; 4];
                count.copy_from_slice(&entry[4..8]);
                return read_ascii_entry_payload(
                    &mut file,
                    read_u16([entry[2], entry[3]], little_endian),
                    u64::from(read_u32(count, little_endian)),
                    &entry[8..12],
                    little_endian,
                    expected_len,
                );
            }
        }
    }
    Err(format!(
        "Staged conversion TIFF tag {wanted_tag} is missing."
    ))
}

fn read_ascii_entry_payload(
    file: &mut File,
    field_type: u16,
    count: u64,
    inline_or_offset: &[u8],
    little_endian: bool,
    expected_len: usize,
) -> Result<Vec<u8>, String> {
    if field_type != 2 || count != expected_len as u64 {
        return Err("Staged conversion TIFF InkNames type/count verification failed.".to_owned());
    }
    if expected_len <= inline_or_offset.len() {
        return Ok(inline_or_offset[..expected_len].to_vec());
    }
    let offset = if inline_or_offset.len() == 4 {
        let mut bytes = [0u8; 4];
        bytes.copy_from_slice(inline_or_offset);
        u64::from(read_u32(bytes, little_endian))
    } else {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(inline_or_offset);
        read_u64(bytes, little_endian)
    };
    file.seek(SeekFrom::Start(offset))
        .map_err(|err| format!("Cannot seek staged TIFF InkNames: {err}"))?;
    let mut payload = vec![0u8; expected_len];
    file.read_exact(&mut payload)
        .map_err(|err| format!("Cannot read staged TIFF InkNames: {err}"))?;
    Ok(payload)
}

fn read_u16(bytes: [u8; 2], little_endian: bool) -> u16 {
    if little_endian {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    }
}

fn read_u32(bytes: [u8; 4], little_endian: bool) -> u32 {
    if little_endian {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    }
}

fn read_u64(bytes: [u8; 8], little_endian: bool) -> u64 {
    if little_endian {
        u64::from_le_bytes(bytes)
    } else {
        u64::from_be_bytes(bytes)
    }
}

fn should_write_bigtiff(spec: &ConversionTiffSpec<'_>, bit_depth: u8) -> bool {
    spec.force_bigtiff
        || u64::from(spec.width)
            .checked_mul(u64::from(spec.height))
            .and_then(|value| value.checked_mul(spec.channel_names.len() as u64))
            .and_then(|value| value.checked_mul(u64::from(bit_depth / 8)))
            .is_none_or(|bytes| bytes >= CLASSIC_TIFF_SAFE_RAW_BYTES)
}

fn staged_path(destination: &Path) -> Result<PathBuf, String> {
    let file_name = destination
        .file_name()
        .ok_or_else(|| "Conversion destination must include a file name.".to_owned())?;
    let mut staged_name = file_name.to_os_string();
    staged_name.push(".conversion.tmp");
    Ok(destination.with_file_name(staged_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiff::decoder::Decoder;

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shade-conversion-{label}-{}-{}.tif",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn profile_bytes() -> Vec<u8> {
        lcms2::Profile::new_srgb().icc().unwrap()
    }

    #[test]
    fn seven_channel_tiff_round_trips_topology_names_icc_and_samples() {
        let destination = temp_path("7c");
        let names =
            ["Blue", "Brown", "Beige", "Black", "Yellow", "Pink", "Green"].map(str::to_owned);
        let profile = profile_bytes();
        let spec = ConversionTiffSpec {
            width: 3,
            height: 2,
            channel_names: &names,
            target_icc: &profile,
            dpi_x: 220.0,
            dpi_y: 220.0,
            orientation: Some(1),
            rows_per_strip: 1,
            force_bigtiff: false,
        };

        write_conversion_tiff_u16_atomic(&destination, &spec, |start, _, samples| {
            for (index, sample) in samples.iter_mut().enumerate() {
                *sample = (start as u16) * 1000 + index as u16;
            }
            Ok(())
        })
        .unwrap();

        let stream = tiff_io::stream_info(&destination).unwrap();
        assert_eq!(stream.metadata.samples_per_pixel, 7);
        assert_eq!(stream.metadata.bit_depth, 16);
        assert_eq!(
            stream.metadata.icc_profile.as_deref(),
            Some(profile.as_slice())
        );
        let decoded = tiff_io::decode_full(&destination).unwrap();
        assert_eq!(decoded.samples.len(), 3 * 2 * 7);
        assert_eq!(decoded.samples[0], 0);
        assert_eq!(decoded.samples[3 * 7], 1000);

        let file = File::open(&destination).unwrap();
        let mut decoder = Decoder::new(file).unwrap();
        assert_eq!(
            decoder.get_tag_unsigned::<u16>(Tag::Unknown(332)).unwrap(),
            2
        );
        assert_eq!(
            decoder.get_tag_unsigned::<u16>(Tag::Unknown(334)).unwrap(),
            7
        );
        let raw = fs::read(&destination).unwrap();
        let expected_ink_names = b"Blue\0Brown\0Beige\0Black\0Yellow\0Pink\0Green\0";
        assert!(raw
            .windows(expected_ink_names.len())
            .any(|window| window == expected_ink_names));
        let _ = fs::remove_file(destination);
    }

    #[test]
    fn cmyk_u8_tiff_round_trips_standard_ink_set_and_samples() {
        let destination = temp_path("cmyk");
        let names = ["Cyan", "Magenta", "Yellow", "Black"].map(str::to_owned);
        let profile = profile_bytes();
        let spec = ConversionTiffSpec {
            width: 2,
            height: 1,
            channel_names: &names,
            target_icc: &profile,
            dpi_x: 300.0,
            dpi_y: 300.0,
            orientation: Some(1),
            rows_per_strip: 1,
            force_bigtiff: false,
        };

        write_conversion_tiff_u8_atomic(&destination, &spec, |_, _, samples| {
            samples.copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
            Ok(())
        })
        .unwrap();

        let decoded = tiff_io::decode_full(&destination).unwrap();
        assert_eq!(decoded.metadata.samples_per_pixel, 4);
        assert_eq!(decoded.metadata.bit_depth, 8);
        assert_eq!(
            decoded.samples,
            vec![257, 514, 771, 1028, 1285, 1542, 1799, 2056]
        );
        let file = File::open(&destination).unwrap();
        let mut decoder = Decoder::new(file).unwrap();
        assert_eq!(
            decoder.get_tag_unsigned::<u16>(Tag::Unknown(332)).unwrap(),
            1
        );
        let _ = fs::remove_file(destination);
    }

    #[test]
    fn failed_render_never_replaces_existing_destination() {
        let destination = temp_path("failure");
        fs::write(&destination, b"existing-production").unwrap();
        let names = ["Cyan", "Magenta", "Yellow", "Black"].map(str::to_owned);
        let profile = profile_bytes();
        let spec = ConversionTiffSpec {
            width: 2,
            height: 2,
            channel_names: &names,
            target_icc: &profile,
            dpi_x: 220.0,
            dpi_y: 220.0,
            orientation: None,
            rows_per_strip: 1,
            force_bigtiff: false,
        };

        let error = write_conversion_tiff_u8_atomic(&destination, &spec, |_, _, _| {
            Err("simulated conversion failure".to_owned())
        })
        .expect_err("render failure must abort");

        assert!(error.contains("simulated"));
        assert_eq!(fs::read(&destination).unwrap(), b"existing-production");
        assert!(!staged_path(&destination).unwrap().exists());
        let _ = fs::remove_file(destination);
    }

    #[test]
    fn invalid_channel_topology_is_rejected_before_writing() {
        let destination = temp_path("invalid");
        let names = ["C", "M", "Y"].map(str::to_owned);
        let profile = profile_bytes();
        let spec = ConversionTiffSpec {
            width: 1,
            height: 1,
            channel_names: &names,
            target_icc: &profile,
            dpi_x: 220.0,
            dpi_y: 220.0,
            orientation: None,
            rows_per_strip: 1,
            force_bigtiff: false,
        };
        assert!(write_conversion_tiff_u8_atomic(&destination, &spec, |_, _, _| Ok(())).is_err());
        assert!(!destination.exists());
    }

    #[test]
    fn huge_layout_selects_bigtiff_without_allocating_pixels() {
        let names = ["Cyan", "Magenta", "Yellow", "Black"].map(str::to_owned);
        let profile = profile_bytes();
        let spec = ConversionTiffSpec {
            width: 100_000,
            height: 100_000,
            channel_names: &names,
            target_icc: &profile,
            dpi_x: 220.0,
            dpi_y: 220.0,
            orientation: None,
            rows_per_strip: 32,
            force_bigtiff: false,
        };
        assert!(should_write_bigtiff(&spec, 16));
    }

    #[test]
    fn forced_bigtiff_round_trips_ink_names_without_large_allocation() {
        let destination = temp_path("forced-bigtiff");
        let names = ["Cyan", "Magenta", "Yellow", "Black"].map(str::to_owned);
        let profile = profile_bytes();
        let spec = ConversionTiffSpec {
            width: 1,
            height: 1,
            channel_names: &names,
            target_icc: &profile,
            dpi_x: 220.0,
            dpi_y: 220.0,
            orientation: None,
            rows_per_strip: 1,
            force_bigtiff: true,
        };

        write_conversion_tiff_u8_atomic(&destination, &spec, |_, _, samples| {
            samples.copy_from_slice(&[1, 2, 3, 4]);
            Ok(())
        })
        .unwrap();

        assert_eq!(&fs::read(&destination).unwrap()[2..4], &[43, 0]);
        assert_eq!(
            read_ascii_tag_bytes(&destination, 333, 26).unwrap(),
            b"Cyan\0Magenta\0Yellow\0Black\0"
        );
        let _ = fs::remove_file(destination);
    }
}
