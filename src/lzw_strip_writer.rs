use std::io::{Seek, Write};
use std::time::{Duration, Instant};

use tiff::encoder::compression::{CompressionAlgorithm, Lzw};
use tiff::encoder::{DirectoryEncoder, TiffKind};
use tiff::tags::Tag;

/// Small shared TIFF primitive for writing already-rendered strips as LZW payloads.
///
/// Callers remain responsible for image/topology/ICC/DPI/Photoshop metadata. This writer owns only
/// compression, raw strip publication, StripOffsets/StripByteCounts bookkeeping and the bounded
/// 16-bit native-endian byte scratch needed by image-tiff's LZW compressor.
pub(crate) struct LzwStripWriter<'a, W, K>
where
    W: Write + Seek,
    K: TiffKind,
{
    directory: DirectoryEncoder<'a, W, K>,
    strip_offsets: Vec<K::OffsetType>,
    strip_byte_counts: Vec<K::OffsetType>,
    compressed: Vec<u8>,
    u16_bytes: Vec<u8>,
}

impl<'a, W, K> LzwStripWriter<'a, W, K>
where
    W: Write + Seek,
    K: TiffKind,
{
    pub(crate) fn new(directory: DirectoryEncoder<'a, W, K>) -> Self {
        Self {
            directory,
            strip_offsets: Vec::new(),
            strip_byte_counts: Vec::new(),
            compressed: Vec::new(),
            u16_bytes: Vec::new(),
        }
    }

    pub(crate) fn write_u8_strip(&mut self, samples: &[u8]) -> Result<Duration, String> {
        let started = Instant::now();
        self.compressed.clear();
        Lzw.write_to(&mut self.compressed, samples)
            .map_err(|err| format!("Cannot LZW-compress TIFF strip: {err}"))?;
        self.write_compressed_strip()?;
        Ok(started.elapsed())
    }

    pub(crate) fn write_u16_strip(&mut self, samples: &[u16]) -> Result<Duration, String> {
        let byte_count = samples
            .len()
            .checked_mul(std::mem::size_of::<u16>())
            .ok_or_else(|| "16-bit TIFF strip byte count overflow.".to_owned())?;
        if self.u16_bytes.len() < byte_count {
            self.u16_bytes.resize(byte_count, 0);
        }

        let started = Instant::now();
        for (target, sample) in self.u16_bytes[..byte_count]
            .chunks_exact_mut(2)
            .zip(samples.iter().copied())
        {
            target.copy_from_slice(&sample.to_ne_bytes());
        }
        self.compressed.clear();
        Lzw.write_to(&mut self.compressed, &self.u16_bytes[..byte_count])
            .map_err(|err| format!("Cannot LZW-compress 16-bit TIFF strip: {err}"))?;
        self.write_compressed_strip()?;
        Ok(started.elapsed())
    }

    fn write_compressed_strip(&mut self) -> Result<(), String> {
        let offset = self
            .directory
            .write_data(self.compressed.as_slice())
            .map_err(|err| format!("Cannot write compressed TIFF strip: {err}"))?;
        self.strip_offsets.push(
            K::convert_offset(offset)
                .map_err(|err| format!("TIFF strip offset exceeds selected TIFF kind: {err}"))?,
        );
        self.strip_byte_counts.push(
            K::convert_offset(self.compressed.len() as u64).map_err(|err| {
                format!("TIFF strip byte count exceeds selected TIFF kind: {err}")
            })?,
        );
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<(), String> {
        if self.strip_offsets.len() != self.strip_byte_counts.len() {
            return Err("TIFF strip offset/count table length mismatch.".to_owned());
        }
        self.directory
            .write_tag(Tag::StripOffsets, K::convert_slice(&self.strip_offsets))
            .map_err(|err| format!("Cannot write TIFF StripOffsets: {err}"))?;
        self.directory
            .write_tag(
                Tag::StripByteCounts,
                K::convert_slice(&self.strip_byte_counts),
            )
            .map_err(|err| format!("Cannot write TIFF StripByteCounts: {err}"))?;
        self.directory
            .finish()
            .map_err(|err| format!("Cannot finalize TIFF directory: {err}"))
    }
}
