use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use tiff::ColorType;
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorModel {
    Gray,
    Rgb,
    Cmyk,
    Other,
}

impl ColorModel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Gray => "Gray",
            Self::Rgb => "RGB",
            Self::Cmyk => "CMYK",
            Self::Other => "Multichannel",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhotoshopChannelDisplay {
    /// Photoshop display color converted to normalized sRGB when the Color
    /// structure uses a color space we understand.
    pub rgb: Option<[f32; 3]>,
    /// Photoshop DisplayInfo opacity/solidity normalized to 0..=1.
    pub solidity: f32,
    /// DisplayInfo kind. Photoshop spot channels use kind 2 in production TIFFs.
    pub kind: u8,
}

impl PhotoshopChannelDisplay {
    pub fn is_spot(self) -> bool {
        self.kind == 2
    }
}

#[derive(Clone, Debug)]
pub struct TiffMetadata {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    /// Actual TIFF SamplesPerPixel. Do not infer this from ColorType: Photoshop
    /// stores spot/alpha separations as ExtraSamples in otherwise RGB/CMYK TIFFs.
    pub samples_per_pixel: usize,
    pub base_channel_count: usize,
    pub color_model: ColorModel,
    pub channel_names: Vec<String>,
    /// Per-channel Photoshop display metadata. Base channels normally contain
    /// None; extra channels may contain Spot/Alpha DisplayInfo resource 1077.
    pub channel_display_info: Vec<Option<PhotoshopChannelDisplay>>,
    pub compression: Option<u16>,
    pub predictor: Option<u16>,
    pub orientation: Option<u16>,
    pub icc_profile: Option<Vec<u8>>,
    pub photoshop_resources: Option<Vec<u8>>,
    pub photoshop_image_source_data: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct DecodedImage {
    pub metadata: TiffMetadata,
    /// Chunky/interleaved samples normalized to 0..=65535 regardless of the
    /// original 8/16-bit sample depth.
    pub samples: Vec<u16>,
}

#[derive(Clone, Debug)]
pub struct PreviewFace {
    pub metadata: TiffMetadata,
    pub width: usize,
    pub height: usize,
    /// One downsampled 16-bit plane per TIFF sample/channel.
    pub channels: Vec<Vec<u16>>,
    pub histograms: Vec<[u32; 256]>,
}

#[derive(Clone, Debug)]
pub struct StreamInfo {
    pub metadata: TiffMetadata,
    pub rows_per_strip: u32,
    pub strip_count: u32,
    /// True when the source is chunky/interleaved and strip-based, allowing
    /// bounded-memory incremental decoding. Planar/tiled files use the proven
    /// full-image compatibility path.
    pub streamable: bool,
}

/// image-tiff 0.11.x intentionally presents RGB/CMYK + ExtraSamples through
/// RGB/CMYK ColorType and therefore drops unspecified extra samples from its
/// decoded output. Photoshop uses exactly those unspecified ExtraSamples for
/// spot channels. For raw shade editing we need every TIFF sample, not only the
/// photometric samples.
///
/// The decoder already supports arbitrary Multiband samples. To reach that
/// path without forking image-tiff, a read-only overlay changes only the
/// PhotometricInterpretation value seen by the decoder from RGB/CMYK to
/// BlackIsZero. Pixel bytes, compression, predictor, channel count and the
/// source file itself remain untouched. The real photometric value is read
/// separately from the original file and retained in TiffMetadata.
#[derive(Clone, Copy, Debug)]
struct PhotometricPatch {
    offset: u64,
    bytes: [u8; 2],
}

struct PatchedReader<R> {
    inner: R,
    position: u64,
    patch: PhotometricPatch,
}

impl<R> PatchedReader<R> {
    fn new(inner: R, patch: PhotometricPatch) -> Self {
        Self {
            inner,
            position: 0,
            patch,
        }
    }
}

impl<R: Read> Read for PatchedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let start = self.position;
        let read = self.inner.read(buffer)?;
        let end = start.saturating_add(read as u64);
        let patch_start = self.patch.offset;
        let patch_end = patch_start.saturating_add(2);

        if start < patch_end && end > patch_start {
            for (index, byte) in buffer[..read].iter_mut().enumerate() {
                let absolute = start + index as u64;
                if absolute == patch_start {
                    *byte = self.patch.bytes[0];
                } else if absolute == patch_start + 1 {
                    *byte = self.patch.bytes[1];
                }
            }
        }

        self.position = end;
        Ok(read)
    }
}

impl<R: Seek> Seek for PatchedReader<R> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        let absolute = self.inner.seek(position)?;
        self.position = absolute;
        Ok(absolute)
    }
}

#[derive(Clone, Copy)]
enum TiffEndian {
    Little,
    Big,
}

impl TiffEndian {
    fn u16(self, bytes: [u8; 2]) -> u16 {
        match self {
            Self::Little => u16::from_le_bytes(bytes),
            Self::Big => u16::from_be_bytes(bytes),
        }
    }

    fn u32(self, bytes: [u8; 4]) -> u32 {
        match self {
            Self::Little => u32::from_le_bytes(bytes),
            Self::Big => u32::from_be_bytes(bytes),
        }
    }

    fn u64(self, bytes: [u8; 8]) -> u64 {
        match self {
            Self::Little => u64::from_le_bytes(bytes),
            Self::Big => u64::from_be_bytes(bytes),
        }
    }

    fn short_bytes(self, value: u16) -> [u8; 2] {
        match self {
            Self::Little => value.to_le_bytes(),
            Self::Big => value.to_be_bytes(),
        }
    }
}

fn open_decoder(path: &Path) -> Result<Decoder<BufReader<File>>, String> {
    let file = File::open(path).map_err(|err| format!("Cannot open TIFF: {err}"))?;
    let reader = BufReader::new(file);
    let decoder = Decoder::new(reader)
        .map_err(|err| format!("Invalid/unsupported TIFF: {err}"))?
        .with_limits(Limits::unlimited());
    Ok(decoder)
}

fn open_multiband_decoder(path: &Path) -> Result<Decoder<PatchedReader<BufReader<File>>>, String> {
    let patch = locate_photometric_patch(path)?;
    let file = File::open(path).map_err(|err| format!("Cannot reopen TIFF: {err}"))?;
    let reader = PatchedReader::new(BufReader::new(file), patch);
    Decoder::new(reader)
        .map_err(|err| format!("Cannot initialize full-channel TIFF decoder: {err}"))
        .map(|decoder| decoder.with_limits(Limits::unlimited()))
}

pub fn decode_full(path: &Path) -> Result<DecodedImage, String> {
    let mut metadata_decoder = open_decoder(path)?;
    let (metadata, planar_configuration) = read_metadata(&mut metadata_decoder)?;

    let needs_multiband_workaround = metadata.samples_per_pixel > metadata.base_channel_count
        && matches!(metadata.color_model, ColorModel::Rgb | ColorModel::Cmyk);

    let samples = if needs_multiband_workaround {
        // Do not let image-tiff's RGB/CMYK readout discard Photoshop spot
        // ExtraSamples. Reopen through the metadata-only photometric overlay.
        drop(metadata_decoder);
        let decoder = open_multiband_decoder(path)?;
        decode_samples(
            decoder,
            metadata.bit_depth,
            metadata.width,
            metadata.height,
            metadata.samples_per_pixel,
            planar_configuration,
        )?
    } else {
        decode_samples(
            metadata_decoder,
            metadata.bit_depth,
            metadata.width,
            metadata.height,
            metadata.samples_per_pixel,
            planar_configuration,
        )?
    };

    Ok(DecodedImage { metadata, samples })
}

fn decode_samples<R: Read + Seek>(
    mut decoder: Decoder<R>,
    bit_depth: u8,
    width: u32,
    height: u32,
    samples_per_pixel: usize,
    planar_configuration: u16,
) -> Result<Vec<u16>, String> {
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| "TIFF dimensions are too large.".to_owned())?;
    let expected = pixel_count
        .checked_mul(samples_per_pixel)
        .ok_or_else(|| "TIFF sample count is too large.".to_owned())?;
    if expected > 32_000_000_000usize {
        return Err("TIFF declares an unreasonable number of samples.".to_owned());
    }

    let mut decoded = DecodingResult::U8(Vec::new());
    let layout = decoder
        .read_image_to_buffer(&mut decoded)
        .map_err(|err| format!("Cannot decode TIFF pixels: {err}"))?;

    let mut samples = decoding_result_to_u16(decoded, bit_depth)?;
    if samples.len() < expected {
        return Err(format!(
            "Decoded TIFF data is incomplete ({} of {} samples). The decoder returned fewer samples than TIFF SamplesPerPixel declares.",
            samples.len(),
            expected
        ));
    }
    if samples.len() > expected {
        samples.truncate(expected);
    }

    if planar_configuration == 2 || layout.planes > 1 {
        samples = planar_to_interleaved(&samples, pixel_count, samples_per_pixel);
    }

    Ok(samples)
}

pub fn stream_info(path: &Path) -> Result<StreamInfo, String> {
    let mut decoder = open_decoder(path)?;
    let (metadata, planar_configuration) = read_metadata(&mut decoder)?;
    let rows_per_strip = decoder
        .find_tag_unsigned::<u32>(Tag::RowsPerStrip)
        .ok()
        .flatten()
        .unwrap_or(metadata.height)
        .max(1)
        .min(metadata.height.max(1));
    let strip_count = decoder.strip_count().ok();
    Ok(StreamInfo {
        metadata,
        rows_per_strip,
        strip_count: strip_count.unwrap_or(1),
        streamable: planar_configuration == 1 && strip_count.is_some(),
    })
}

pub fn for_each_decoded_strip<F>(
    path: &Path,
    info: &StreamInfo,
    mut callback: F,
) -> Result<(), String>
where
    F: FnMut(u32, u32, &[u16]) -> Result<(), String>,
{
    if !info.streamable {
        let decoded = decode_full(path)?;
        callback(0, decoded.metadata.height, &decoded.samples)?;
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
        stream_decoder_strips(decoder, info, &mut callback)
    } else {
        let decoder = open_decoder(path)?;
        stream_decoder_strips(decoder, info, &mut callback)
    }
}

fn stream_decoder_strips<R, F>(
    mut decoder: Decoder<R>,
    info: &StreamInfo,
    callback: &mut F,
) -> Result<(), String>
where
    R: Read + Seek,
    F: FnMut(u32, u32, &[u16]) -> Result<(), String>,
{
    let strip_count = decoder
        .strip_count()
        .map_err(|err| format!("Cannot read TIFF strip count: {err}"))?;
    let width = info.metadata.width as usize;
    let channels = info.metadata.samples_per_pixel;
    let mut row_start = 0u32;

    for strip_index in 0..strip_count {
        let (chunk_width, row_count) = decoder.chunk_data_dimensions(strip_index);
        if chunk_width != info.metadata.width {
            return Err(format!(
                "Unexpected TIFF strip width {chunk_width}; expected {}.",
                info.metadata.width
            ));
        }
        let decoded = decoder
            .read_chunk(strip_index)
            .map_err(|err| format!("Cannot decode TIFF strip {strip_index}: {err}"))?;
        let mut samples = decoding_result_to_u16(decoded, info.metadata.bit_depth)?;
        let expected = width
            .checked_mul(row_count as usize)
            .and_then(|pixels| pixels.checked_mul(channels))
            .ok_or_else(|| "TIFF strip sample count is too large.".to_owned())?;
        if samples.len() < expected {
            return Err(format!(
                "Decoded TIFF strip {strip_index} is incomplete ({} of {expected} samples).",
                samples.len()
            ));
        }
        samples.truncate(expected);
        callback(row_start, row_count, &samples)?;
        row_start = row_start.saturating_add(row_count);
    }

    if row_start < info.metadata.height {
        return Err(format!(
            "TIFF strip stream ended at row {row_start} of {}.",
            info.metadata.height
        ));
    }
    Ok(())
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
    let mut next_preview_y = 0usize;

    for_each_decoded_strip(path, &info, |row_start, row_count, samples| {
        let row_end = row_start.saturating_add(row_count) as usize;
        while next_preview_y < height && source_y[next_preview_y] < row_end {
            let src_y = source_y[next_preview_y];
            if src_y < row_start as usize {
                next_preview_y += 1;
                continue;
            }
            let local_y = src_y - row_start as usize;
            for (preview_x, &src_x) in source_x.iter().enumerate() {
                let source_base = (local_y * source_width + src_x) * channel_count;
                let destination = next_preview_y * width + preview_x;
                for channel in 0..channel_count {
                    channels[channel][destination] = samples[source_base + channel];
                }
            }
            next_preview_y += 1;
        }
        Ok(())
    })?;

    if next_preview_y != height {
        return Err(format!(
            "Preview stream filled {next_preview_y} of {height} rows."
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

fn read_metadata<R: Read + Seek>(decoder: &mut Decoder<R>) -> Result<(TiffMetadata, u16), String> {
    let (width, height) = decoder
        .dimensions()
        .map_err(|err| format!("Cannot read TIFF dimensions: {err}"))?;
    let color_type = decoder
        .colortype()
        .map_err(|err| format!("Cannot read TIFF color type: {err}"))?;
    let bit_depth = color_type.bit_depth();
    if !matches!(bit_depth, 8 | 16) {
        return Err(format!(
            "Shade Editor currently supports 8-bit and 16-bit TIFF only; file is {bit_depth}-bit."
        ));
    }

    let samples_per_pixel = decoder
        .find_tag_unsigned::<u16>(Tag::SamplesPerPixel)
        .map_err(|err| format!("Cannot read TIFF SamplesPerPixel: {err}"))?
        .map(usize::from)
        .unwrap_or_else(|| usize::from(color_type.num_samples()));
    if samples_per_pixel == 0 || samples_per_pixel > 56 {
        return Err(format!(
            "Unsupported TIFF channel count: {samples_per_pixel}."
        ));
    }

    let photometric = decoder
        .find_tag_unsigned::<u16>(Tag::PhotometricInterpretation)
        .ok()
        .flatten();
    let (color_model, base_channel_count) = infer_color_model(&color_type, photometric);
    if samples_per_pixel < base_channel_count {
        return Err(format!(
            "TIFF SamplesPerPixel ({samples_per_pixel}) is smaller than its {} base channel count ({base_channel_count}).",
            color_model.title()
        ));
    }

    let planar_configuration = decoder
        .find_tag_unsigned::<u16>(Tag::PlanarConfiguration)
        .ok()
        .flatten()
        .unwrap_or(1);
    if !matches!(planar_configuration, 1 | 2) {
        return Err(format!(
            "Unsupported TIFF PlanarConfiguration: {planar_configuration}."
        ));
    }

    let compression = decoder
        .find_tag_unsigned::<u16>(Tag::Compression)
        .ok()
        .flatten();
    let predictor = decoder
        .find_tag_unsigned::<u16>(Tag::Predictor)
        .ok()
        .flatten();
    let orientation = decoder
        .find_tag_unsigned::<u16>(Tag::Orientation)
        .ok()
        .flatten();
    let icc_profile = decoder.get_tag_u8_vec(Tag::IccProfile).ok();
    let photoshop_resources = decoder.get_tag_u8_vec(Tag::Unknown(34377)).ok();
    let photoshop_image_source_data = decoder.get_tag_u8_vec(Tag::Unknown(37724)).ok();
    let channel_names = channel_names(
        color_model,
        base_channel_count,
        samples_per_pixel,
        photoshop_resources.as_deref(),
    );
    let channel_display_info = photoshop_resources
        .as_deref()
        .map(|resources| {
            photoshop_channel_display_info(base_channel_count, samples_per_pixel, resources)
        })
        .unwrap_or_else(|| vec![None; samples_per_pixel]);

    Ok((
        TiffMetadata {
            width,
            height,
            bit_depth,
            samples_per_pixel,
            base_channel_count,
            color_model,
            channel_names,
            channel_display_info,
            compression,
            predictor,
            orientation,
            icc_profile,
            photoshop_resources,
            photoshop_image_source_data,
        },
        planar_configuration,
    ))
}

fn infer_color_model(color_type: &ColorType, photometric: Option<u16>) -> (ColorModel, usize) {
    match photometric {
        Some(2) => return (ColorModel::Rgb, 3),
        Some(5) => return (ColorModel::Cmyk, 4),
        Some(0 | 1) => return (ColorModel::Gray, 1),
        _ => {}
    }
    match color_type {
        ColorType::RGB(_) | ColorType::RGBA(_) => (ColorModel::Rgb, 3),
        ColorType::CMYK(_) | ColorType::CMYKA(_) => (ColorModel::Cmyk, 4),
        ColorType::Gray(_) | ColorType::GrayA(_) => (ColorModel::Gray, 1),
        _ => (
            ColorModel::Other,
            usize::from(color_type.num_samples()).max(1),
        ),
    }
}

fn locate_photometric_patch(path: &Path) -> Result<PhotometricPatch, String> {
    let file = File::open(path).map_err(|err| format!("Cannot inspect TIFF IFD: {err}"))?;
    let mut reader = BufReader::new(file);
    locate_photometric_patch_in(&mut reader)
}

fn locate_photometric_patch_in<R: Read + Seek>(reader: &mut R) -> Result<PhotometricPatch, String> {
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|err| format!("Cannot seek TIFF header: {err}"))?;

    let mut signature = [0u8; 4];
    reader
        .read_exact(&mut signature)
        .map_err(|err| format!("Cannot read TIFF header: {err}"))?;
    let endian = match &signature[..2] {
        b"II" => TiffEndian::Little,
        b"MM" => TiffEndian::Big,
        _ => return Err("Cannot patch TIFF: invalid byte-order signature.".to_owned()),
    };
    let magic = endian.u16([signature[2], signature[3]]);

    let (first_ifd, big_tiff) = match magic {
        42 => {
            let mut bytes = [0u8; 4];
            reader
                .read_exact(&mut bytes)
                .map_err(|err| format!("Cannot read TIFF first IFD offset: {err}"))?;
            (u64::from(endian.u32(bytes)), false)
        }
        43 => {
            let mut rest = [0u8; 12];
            reader
                .read_exact(&mut rest)
                .map_err(|err| format!("Cannot read BigTIFF header: {err}"))?;
            let offset_size = endian.u16([rest[0], rest[1]]);
            let reserved = endian.u16([rest[2], rest[3]]);
            if offset_size != 8 || reserved != 0 {
                return Err("Unsupported BigTIFF header layout.".to_owned());
            }
            let mut ifd_bytes = [0u8; 8];
            ifd_bytes.copy_from_slice(&rest[4..12]);
            (endian.u64(ifd_bytes), true)
        }
        _ => {
            return Err(format!(
                "Cannot patch TIFF: unexpected magic value {magic}."
            ));
        }
    };

    reader
        .seek(SeekFrom::Start(first_ifd))
        .map_err(|err| format!("Cannot seek TIFF first IFD: {err}"))?;

    if big_tiff {
        let mut count_bytes = [0u8; 8];
        reader
            .read_exact(&mut count_bytes)
            .map_err(|err| format!("Cannot read BigTIFF IFD entry count: {err}"))?;
        let count = endian.u64(count_bytes);
        if count > 65_535 {
            return Err("BigTIFF IFD contains an unreasonable number of entries.".to_owned());
        }

        for _ in 0..count {
            let entry_start = reader
                .stream_position()
                .map_err(|err| format!("Cannot inspect BigTIFF IFD position: {err}"))?;
            let mut entry = [0u8; 20];
            reader
                .read_exact(&mut entry)
                .map_err(|err| format!("Cannot read BigTIFF IFD entry: {err}"))?;
            let tag = endian.u16([entry[0], entry[1]]);
            if tag != 262 {
                continue;
            }
            let field_type = endian.u16([entry[2], entry[3]]);
            let mut item_count_bytes = [0u8; 8];
            item_count_bytes.copy_from_slice(&entry[4..12]);
            let item_count = endian.u64(item_count_bytes);
            if field_type != 3 || item_count != 1 {
                return Err(
                    "Unsupported BigTIFF PhotometricInterpretation tag representation.".to_owned(),
                );
            }
            return Ok(PhotometricPatch {
                offset: entry_start + 12,
                bytes: endian.short_bytes(1),
            });
        }
    } else {
        let mut count_bytes = [0u8; 2];
        reader
            .read_exact(&mut count_bytes)
            .map_err(|err| format!("Cannot read TIFF IFD entry count: {err}"))?;
        let count = endian.u16(count_bytes);

        for _ in 0..count {
            let entry_start = reader
                .stream_position()
                .map_err(|err| format!("Cannot inspect TIFF IFD position: {err}"))?;
            let mut entry = [0u8; 12];
            reader
                .read_exact(&mut entry)
                .map_err(|err| format!("Cannot read TIFF IFD entry: {err}"))?;
            let tag = endian.u16([entry[0], entry[1]]);
            if tag != 262 {
                continue;
            }
            let field_type = endian.u16([entry[2], entry[3]]);
            let item_count = endian.u32([entry[4], entry[5], entry[6], entry[7]]);
            if field_type != 3 || item_count != 1 {
                return Err(
                    "Unsupported TIFF PhotometricInterpretation tag representation.".to_owned(),
                );
            }
            return Ok(PhotometricPatch {
                offset: entry_start + 8,
                bytes: endian.short_bytes(1),
            });
        }
    }

    Err(
        "Cannot decode Photoshop extra channels: TIFF PhotometricInterpretation tag was not found."
            .to_owned(),
    )
}

fn decoding_result_to_u16(decoded: DecodingResult, bit_depth: u8) -> Result<Vec<u16>, String> {
    match decoded {
        DecodingResult::U8(values) if bit_depth == 8 => Ok(values
            .into_iter()
            .map(|value| u16::from(value) * 257)
            .collect()),
        DecodingResult::U16(values) if bit_depth == 16 => Ok(values),
        DecodingResult::U8(values) => Ok(values
            .into_iter()
            .map(|value| u16::from(value) * 257)
            .collect()),
        DecodingResult::U16(values) => Ok(values),
        _ => Err("This TIFF sample type is not supported by Shade Editor.".to_owned()),
    }
}

fn planar_to_interleaved(planar: &[u16], pixel_count: usize, channels: usize) -> Vec<u16> {
    let mut interleaved = vec![0u16; pixel_count.saturating_mul(channels)];
    for channel in 0..channels {
        let plane_start = channel.saturating_mul(pixel_count);
        if plane_start + pixel_count > planar.len() {
            break;
        }
        for pixel in 0..pixel_count {
            interleaved[pixel * channels + channel] = planar[plane_start + pixel];
        }
    }
    interleaved
}

fn histogram(values: &[u16]) -> [u32; 256] {
    let mut bins = [0u32; 256];
    for value in values {
        let index = usize::from(*value >> 8);
        bins[index] = bins[index].saturating_add(1);
    }
    bins
}

fn channel_names(
    model: ColorModel,
    base_count: usize,
    total_count: usize,
    photoshop: Option<&[u8]>,
) -> Vec<String> {
    let mut names: Vec<String> = match model {
        ColorModel::Rgb => ["Red", "Green", "Blue"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        ColorModel::Cmyk => ["Cyan", "Magenta", "Yellow", "Black"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        ColorModel::Gray => vec!["Gray".to_owned()],
        ColorModel::Other => (0..base_count)
            .map(|index| format!("Channel {}", index + 1))
            .collect(),
    };
    names.truncate(base_count);
    while names.len() < base_count {
        names.push(format!("Channel {}", names.len() + 1));
    }

    let extra_count = total_count.saturating_sub(base_count);
    let photoshop_names = photoshop
        .map(parse_photoshop_channel_names)
        .unwrap_or_default();
    for index in 0..extra_count {
        let name = photoshop_names
            .get(index)
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| format!("Spot/Alpha {}", index + 1));
        names.push(unique_name(name, &names));
    }
    while names.len() < total_count {
        names.push(format!("Channel {}", names.len() + 1));
    }
    names.truncate(total_count);
    names
}

fn photoshop_channel_display_info(
    base_count: usize,
    total_count: usize,
    resources: &[u8],
) -> Vec<Option<PhotoshopChannelDisplay>> {
    let mut result = vec![None; total_count];
    let Some(payload) = find_photoshop_resource(resources, 1077) else {
        return result;
    };
    let extra_count = total_count.saturating_sub(base_count);
    for (index, display) in parse_photoshop_display_info(payload)
        .into_iter()
        .take(extra_count)
        .enumerate()
    {
        result[base_count + index] = Some(display);
    }
    result
}

fn find_photoshop_resource(resources: &[u8], wanted_id: u16) -> Option<&[u8]> {
    let mut offset = 0usize;
    while offset + 12 <= resources.len() {
        if &resources[offset..offset + 4] != b"8BIM" {
            offset += 1;
            continue;
        }
        offset += 4;
        let id = u16::from_be_bytes([resources[offset], resources[offset + 1]]);
        offset += 2;
        if offset >= resources.len() {
            return None;
        }
        let name_len = resources[offset] as usize;
        offset += 1;
        if offset + name_len > resources.len() {
            return None;
        }
        offset += name_len;
        if (1 + name_len) % 2 != 0 {
            offset = offset.saturating_add(1);
        }
        if offset + 4 > resources.len() {
            return None;
        }
        let size = u32::from_be_bytes([
            resources[offset],
            resources[offset + 1],
            resources[offset + 2],
            resources[offset + 3],
        ]) as usize;
        offset += 4;
        if offset + size > resources.len() {
            return None;
        }
        let payload = &resources[offset..offset + size];
        if id == wanted_id {
            return Some(payload);
        }
        offset += size;
        if size % 2 != 0 {
            offset = offset.saturating_add(1);
        }
    }
    None
}

fn parse_photoshop_display_info(payload: &[u8]) -> Vec<PhotoshopChannelDisplay> {
    // Resource 1077 starts with a big-endian u32 version followed by 13-byte
    // DisplayInfo records: Color(10), opacity/solidity u16, kind u8.
    if payload.len() < 4 || u32::from_be_bytes(payload[0..4].try_into().unwrap()) != 1 {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut offset = 4usize;
    while offset + 13 <= payload.len() {
        let color_space = u16::from_be_bytes([payload[offset], payload[offset + 1]]);
        let mut components = [0u16; 4];
        for (component, slot) in components.iter_mut().enumerate() {
            let start = offset + 2 + component * 2;
            *slot = u16::from_be_bytes([payload[start], payload[start + 1]]);
        }
        let solidity_raw = u16::from_be_bytes([payload[offset + 10], payload[offset + 11]]);
        let kind = payload[offset + 12];
        result.push(PhotoshopChannelDisplay {
            rgb: photoshop_color_to_rgb(color_space, components),
            solidity: (solidity_raw as f32 / 100.0).clamp(0.0, 1.0),
            kind,
        });
        offset += 13;
    }
    result
}

fn photoshop_color_to_rgb(color_space: u16, c: [u16; 4]) -> Option<[f32; 3]> {
    let unit = |value: u16| value as f32 / 65535.0;
    match color_space {
        0 => Some([unit(c[0]), unit(c[1]), unit(c[2])]),
        1 => Some(hsb_to_rgb(unit(c[0]), unit(c[1]), unit(c[2]))),
        2 => {
            // Adobe Color structure CMYK uses 0 = 100% ink and 65535 = 0% ink.
            let cyan = 1.0 - unit(c[0]);
            let magenta = 1.0 - unit(c[1]);
            let yellow = 1.0 - unit(c[2]);
            let black = 1.0 - unit(c[3]);
            Some([
                1.0 - (cyan + black).min(1.0),
                1.0 - (magenta + black).min(1.0),
                1.0 - (yellow + black).min(1.0),
            ])
        }
        8 => {
            let gray = (c[0] as f32 / 10000.0).clamp(0.0, 1.0);
            Some([gray, gray, gray])
        }
        _ => None,
    }
}

fn hsb_to_rgb(hue: f32, saturation: f32, brightness: f32) -> [f32; 3] {
    let h = hue.rem_euclid(1.0) * 6.0;
    let sector = h.floor() as i32;
    let f = h - sector as f32;
    let p = brightness * (1.0 - saturation);
    let q = brightness * (1.0 - saturation * f);
    let t = brightness * (1.0 - saturation * (1.0 - f));
    match sector.rem_euclid(6) {
        0 => [brightness, t, p],
        1 => [q, brightness, p],
        2 => [p, brightness, t],
        3 => [p, q, brightness],
        4 => [t, p, brightness],
        _ => [brightness, p, q],
    }
}

fn unique_name(mut name: String, existing: &[String]) -> String {
    if !existing.iter().any(|item| item == &name) {
        return name;
    }
    let root = name.clone();
    let mut suffix = 2;
    while existing.iter().any(|item| item == &name) {
        name = format!("{root} ({suffix})");
        suffix += 1;
    }
    name
}

fn parse_photoshop_channel_names(resources: &[u8]) -> Vec<String> {
    let mut offset = 0usize;
    let mut pascal_names = Vec::new();
    let mut unicode_names = Vec::new();

    while offset + 12 <= resources.len() {
        if &resources[offset..offset + 4] != b"8BIM" {
            offset += 1;
            continue;
        }
        offset += 4;
        if offset + 2 > resources.len() {
            break;
        }
        let id = u16::from_be_bytes([resources[offset], resources[offset + 1]]);
        offset += 2;

        if offset >= resources.len() {
            break;
        }
        let name_len = resources[offset] as usize;
        offset += 1;
        if offset + name_len > resources.len() {
            break;
        }
        offset += name_len;
        if (1 + name_len) % 2 != 0 {
            offset = offset.saturating_add(1);
        }
        if offset + 4 > resources.len() {
            break;
        }
        let size = u32::from_be_bytes([
            resources[offset],
            resources[offset + 1],
            resources[offset + 2],
            resources[offset + 3],
        ]) as usize;
        offset += 4;
        if offset + size > resources.len() {
            break;
        }
        let data = &resources[offset..offset + size];

        match id {
            1006 => pascal_names.extend(parse_pascal_names(data)),
            1045 => unicode_names.extend(parse_unicode_names(data)),
            _ => {}
        }

        offset += size;
        if size % 2 != 0 {
            offset = offset.saturating_add(1);
        }
    }

    if !unicode_names.is_empty() {
        unicode_names
    } else {
        pascal_names
    }
}

fn parse_pascal_names(data: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut offset = 0usize;
    while offset < data.len() {
        let len = data[offset] as usize;
        offset += 1;
        if offset + len > data.len() {
            break;
        }
        names.push(String::from_utf8_lossy(&data[offset..offset + len]).into_owned());
        offset += len;
    }
    names
}

fn parse_unicode_names(data: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut offset = 0usize;
    while offset + 4 <= data.len() {
        let units = u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        offset += 4;
        let byte_len = match units.checked_mul(2) {
            Some(value) => value,
            None => break,
        };
        if offset + byte_len > data.len() {
            break;
        }
        let mut utf16 = Vec::with_capacity(units);
        for pair in data[offset..offset + byte_len].chunks_exact(2) {
            utf16.push(u16::from_be_bytes([pair[0], pair[1]]));
        }
        names.push(
            String::from_utf16_lossy(&utf16)
                .trim_end_matches('\0')
                .to_owned(),
        );
        offset += byte_len;
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    use tiff::encoder::{TiffEncoder, colortype};
    use tiff::tags::ExtraSamples;

    #[test]
    fn interleaves_planar_data() {
        let planar = vec![1, 2, 3, 10, 20, 30];
        assert_eq!(
            planar_to_interleaved(&planar, 3, 2),
            vec![1, 10, 2, 20, 3, 30]
        );
    }

    #[test]
    fn names_all_photoshop_extra_channels() {
        let names = channel_names(ColorModel::Cmyk, 4, 6, None);
        assert_eq!(names.len(), 6);
        assert_eq!(names[0], "Cyan");
        assert_eq!(names[4], "Spot/Alpha 1");
        assert_eq!(names[5], "Spot/Alpha 2");
    }

    #[test]
    fn photometric_overrides_multiband_shape() {
        let fake = ColorType::Multiband {
            bit_depth: 8,
            num_samples: 6,
        };
        assert_eq!(infer_color_model(&fake, Some(2)), (ColorModel::Rgb, 3));
        assert_eq!(infer_color_model(&fake, Some(5)), (ColorModel::Cmyk, 4));
    }

    #[test]
    fn patched_reader_exposes_all_cmyk_extra_samples() {
        let pixels = vec![1u8, 2, 3, 4, 5, 6, 10, 20, 30, 40, 50, 60];

        let mut encoded = Cursor::new(Vec::new());
        {
            let mut tiff = TiffEncoder::new(&mut encoded).unwrap();
            let mut image = tiff.new_image::<colortype::CMYK8>(2, 1).unwrap();
            image
                .extra_samples(&[ExtraSamples::Unspecified, ExtraSamples::Unspecified])
                .unwrap();
            image.write_data(&pixels).unwrap();
        }

        let bytes = encoded.into_inner();
        let mut inspector = Cursor::new(bytes.clone());
        let patch = locate_photometric_patch_in(&mut inspector).unwrap();

        let reader = PatchedReader::new(Cursor::new(bytes), patch);
        let mut decoder = Decoder::new(reader)
            .unwrap()
            .with_limits(Limits::unlimited());
        assert!(matches!(
            decoder.colortype().unwrap(),
            ColorType::Multiband {
                bit_depth: 8,
                num_samples: 6
            }
        ));

        let mut decoded = DecodingResult::U8(Vec::new());
        decoder.read_image_to_buffer(&mut decoded).unwrap();
        let DecodingResult::U8(values) = decoded else {
            panic!("Expected 8-bit decoded values");
        };
        assert_eq!(values, pixels);
    }

    #[test]
    fn patch_reader_only_changes_photometric_value_bytes() {
        let original = vec![10u8, 11, 12, 13, 14, 15];
        let patch = PhotometricPatch {
            offset: 2,
            bytes: [1, 0],
        };
        let mut reader = PatchedReader::new(Cursor::new(original), patch);
        let mut result = Vec::new();
        reader.read_to_end(&mut result).unwrap();
        assert_eq!(result, vec![10, 11, 1, 0, 14, 15]);
    }

    #[test]
    fn parses_photoshop_spot_display_info_from_production_resource_shape() {
        // The two 13-byte records mirror the resource 1077 layout seen in a
        // production CMYK + 2 Spot TIFF: HSB purple and HSB green, kind = 2.
        let payload = [
            0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0xd4, 0xd4, 0xc7, 0xc7, 0xff, 0xff, 0x00, 0x00,
            0x00, 0x00, 0x02, 0x00, 0x01, 0x6e, 0x6e, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00,
            0x00, 0x02,
        ];
        let display = parse_photoshop_display_info(&payload);
        assert_eq!(display.len(), 2);
        assert!(display[0].is_spot());
        assert!(display[1].is_spot());
        assert_eq!(display[0].solidity, 0.0);
        let purple = display[0].rgb.unwrap();
        let green = display[1].rgb.unwrap();
        assert!(purple[0] > 0.95 && purple[2] > 0.95 && purple[1] < 0.30);
        assert!(green[1] > 0.95 && green[0] < 0.05 && green[2] > 0.50);
    }
}
