use std::path::Path;

use crate::model::ShadeProject;
use crate::source_tiff_writer;
use crate::tiff_io::{StreamInfo, for_each_decoded_strip};

use super::{TextOverlay, adjusted_strip, apply_text_overlay_to_rows};

pub(super) fn write_u8<F>(
    source: &Path,
    destination: &Path,
    stream: &StreamInfo,
    project: &ShadeProject,
    overlay: Option<&TextOverlay>,
    dpi_info: crate::dpi::DpiInfo,
    progress: &mut F,
) -> Result<(), String>
where
    F: FnMut(f32, &str),
{
    source_tiff_writer::write_tiff_lzw_strips_u8(
        source,
        destination,
        &stream.metadata,
        dpi_info,
        stream.rows_per_strip,
        |sink| {
            let channels = stream.metadata.samples_per_pixel;
            let width = stream.metadata.width as usize;
            for_each_decoded_strip(source, stream, |row_start, row_count, input| {
                let mut adjusted = adjusted_strip(input, &stream.metadata, project);
                if let Some(overlay) = overlay {
                    apply_text_overlay_to_rows(
                        &mut adjusted,
                        row_start as usize,
                        row_count as usize,
                        width,
                        channels,
                        overlay,
                    );
                }
                let expected = row_count as usize * width * channels;
                if adjusted.len() != expected {
                    return Err(format!(
                        "Output strip sample mismatch: generated {}, expected {expected}.",
                        adjusted.len()
                    ));
                }
                let data = adjusted
                    .into_iter()
                    .map(|value| (value >> 8) as u8)
                    .collect::<Vec<_>>();
                sink(row_start, row_count, &data)?;
                let done = row_start.saturating_add(row_count) as f32
                    / stream.metadata.height.max(1) as f32;
                progress(
                    0.06 + done * 0.84,
                    "Streaming adjustments directly into LZW TIFF",
                );
                Ok(())
            })
        },
    )
}

pub(super) fn write_u16<F>(
    source: &Path,
    destination: &Path,
    stream: &StreamInfo,
    project: &ShadeProject,
    overlay: Option<&TextOverlay>,
    dpi_info: crate::dpi::DpiInfo,
    progress: &mut F,
) -> Result<(), String>
where
    F: FnMut(f32, &str),
{
    source_tiff_writer::write_tiff_lzw_strips_u16(
        source,
        destination,
        &stream.metadata,
        dpi_info,
        stream.rows_per_strip,
        |sink| {
            let channels = stream.metadata.samples_per_pixel;
            let width = stream.metadata.width as usize;
            for_each_decoded_strip(source, stream, |row_start, row_count, input| {
                let mut adjusted = adjusted_strip(input, &stream.metadata, project);
                if let Some(overlay) = overlay {
                    apply_text_overlay_to_rows(
                        &mut adjusted,
                        row_start as usize,
                        row_count as usize,
                        width,
                        channels,
                        overlay,
                    );
                }
                let expected = row_count as usize * width * channels;
                if adjusted.len() != expected {
                    return Err(format!(
                        "Output strip sample mismatch: generated {}, expected {expected}.",
                        adjusted.len()
                    ));
                }
                sink(row_start, row_count, &adjusted)?;
                let done = row_start.saturating_add(row_count) as f32
                    / stream.metadata.height.max(1) as f32;
                progress(
                    0.06 + done * 0.84,
                    "Streaming adjustments directly into LZW TIFF",
                );
                Ok(())
            })
        },
    )
}
