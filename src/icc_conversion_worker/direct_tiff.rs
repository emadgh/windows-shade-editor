use std::sync::mpsc::{Receiver, SyncSender, channel, sync_channel};

use super::*;

const DIRECT_TIFF_PIPELINE_DEPTH: usize = 2;

#[derive(Debug)]
struct AdjustedTiffStrip {
    start_row: u32,
    row_count: u32,
    samples: Vec<u16>,
    adjustment_elapsed: Duration,
}

pub(super) fn render_convert_and_commit(
    capture: &ConversionJobCapture,
    cancellation: &ConversionCancellation,
    report: &mut dyn FnMut(ConversionProgress),
    stream: &StreamInfo,
    target_icc: Option<&[u8]>,
    transform: &mut RuntimeProductionTransform,
    default_dpi: f64,
) -> Result<(), String> {
    if !stream.row_streamable {
        return Err("Direct TIFF conversion requires a row-streamable strip source.".to_owned());
    }
    cancellation.check_before_commit()?;

    let metadata = &stream.metadata;
    let width = metadata.width as usize;
    let source_channels = metadata.samples_per_pixel;
    let dpi = dpi::read_dpi(&capture.source_face_path, default_dpi);
    let channel_names = capture
        .conversion_recipe
        .target
        .channels
        .iter()
        .map(|channel| channel.name.clone())
        .collect::<Vec<_>>();
    let spec = ConversionTiffSpec {
        width: metadata.width,
        height: metadata.height,
        channel_names: &channel_names,
        target_icc,
        dpi_x: dpi.dpi_x,
        dpi_y: dpi.dpi_y,
        orientation: metadata.orientation,
        rows_per_strip: stream.rows_per_strip.max(1),
        force_bigtiff: false,
        replace_existing: capture.output_policy == CapturedOutputPolicy::TransactionalReplace,
    };
    let project = capture.source_recipe.materialize_project();
    let perf_enabled = tiff_performance::enabled();
    let logical_source_bytes = u64::from(metadata.width)
        .checked_mul(u64::from(metadata.height))
        .and_then(|pixels| pixels.checked_mul(source_channels as u64))
        .and_then(|samples| samples.checked_mul(std::mem::size_of::<u16>() as u64));
    let height_f32 = metadata.height.max(1) as f32;
    let (strip_sender, strip_receiver) =
        sync_channel::<Result<AdjustedTiffStrip, String>>(DIRECT_TIFF_PIPELINE_DEPTH);
    let (done_sender, done_receiver) = channel::<Result<(), String>>();
    let mut adjustment_elapsed = Duration::ZERO;

    report(ConversionProgress::new(
        ConversionPhase::SourceAdjustments,
        0.08,
        "Streaming saved TIFF source adjustments directly into conversion",
    ));

    let result = std::thread::scope(|scope| -> Result<(), String> {
        let producer = scope.spawn(|| {
            let result = produce_adjusted_strips(
                capture,
                cancellation,
                stream,
                &project,
                strip_sender,
            );
            let _ = done_sender.send(result.clone());
            result
        });

        let write_result = match capture.conversion_recipe.target.bit_depth {
            16 => write_conversion_tiff_u16_atomic_with_precommit(
                &capture.output_tiff_path,
                &spec,
                |start_row, row_count, output| {
                    cancellation.check_before_commit()?;
                    let expected_samples = (row_count as usize)
                        .checked_mul(width)
                        .and_then(|pixels| pixels.checked_mul(source_channels))
                        .ok_or_else(|| "Direct TIFF source strip sample count overflow.".to_owned())?;
                    let strip = receive_adjusted_strip(
                        &strip_receiver,
                        start_row,
                        row_count,
                        expected_samples,
                    )?;
                    adjustment_elapsed += strip.adjustment_elapsed;
                    transform.transform_u16_bounded(&strip.samples, output, source_channels)?;
                    let done = start_row.saturating_add(row_count) as f32 / height_f32;
                    report(ConversionProgress::new(
                        ConversionPhase::ColorConversion,
                        0.10 + 0.76 * done.min(1.0),
                        format!(
                            "Adjusted and converted TIFF rows {}–{}",
                            start_row + 1,
                            start_row + row_count
                        ),
                    ));
                    Ok(())
                },
                || {
                    let producer_result = done_receiver.recv().map_err(|_| {
                        "Direct TIFF source producer ended before the commit boundary.".to_owned()
                    })?;
                    producer_result?;
                    cancellation.check_before_commit()
                },
            ),
            8 => write_conversion_tiff_u8_atomic_with_precommit(
                &capture.output_tiff_path,
                &spec,
                |start_row, row_count, output| {
                    cancellation.check_before_commit()?;
                    let expected_samples = (row_count as usize)
                        .checked_mul(width)
                        .and_then(|pixels| pixels.checked_mul(source_channels))
                        .ok_or_else(|| "Direct TIFF source strip sample count overflow.".to_owned())?;
                    let strip = receive_adjusted_strip(
                        &strip_receiver,
                        start_row,
                        row_count,
                        expected_samples,
                    )?;
                    adjustment_elapsed += strip.adjustment_elapsed;
                    transform.transform_u8_bounded(&strip.samples, output, source_channels)?;
                    let done = start_row.saturating_add(row_count) as f32 / height_f32;
                    report(ConversionProgress::new(
                        ConversionPhase::ColorConversion,
                        0.10 + 0.76 * done.min(1.0),
                        format!(
                            "Adjusted and converted TIFF rows {}–{}",
                            start_row + 1,
                            start_row + row_count
                        ),
                    ));
                    Ok(())
                },
                || {
                    let producer_result = done_receiver.recv().map_err(|_| {
                        "Direct TIFF source producer ended before the commit boundary.".to_owned()
                    })?;
                    producer_result?;
                    cancellation.check_before_commit()
                },
            ),
            depth => Err(format!(
                "Unsupported captured conversion precision: {depth}-bit."
            )),
        };

        drop(strip_receiver);
        let producer_result = producer
            .join()
            .map_err(|_| "Direct TIFF source producer thread panicked.".to_owned())?;
        match write_result {
            Ok(()) => producer_result,
            Err(error) => Err(error),
        }
    });

    if perf_enabled {
        tiff_performance::emit_phase_if_enabled(
            "conversion_direct_tiff",
            TiffPerfPhase::AdjustmentRender,
            adjustment_elapsed,
            logical_source_bytes,
        );
    }
    result?;

    report(ConversionProgress::new(
        ConversionPhase::OutputValidation,
        0.90,
        "Conversion TIFF validated and committed",
    ));
    Ok(())
}

fn produce_adjusted_strips(
    capture: &ConversionJobCapture,
    cancellation: &ConversionCancellation,
    stream: &StreamInfo,
    project: &ShadeProject,
    sender: SyncSender<Result<AdjustedTiffStrip, String>>,
) -> Result<(), String> {
    let metadata = &stream.metadata;
    let width = metadata.width as usize;
    let channels = metadata.samples_per_pixel;
    let mut expected_start_row = 0u32;

    let result = tiff_io::for_each_decoded_strip(
        &capture.source_face_path,
        stream,
        |start_row, row_count, input| {
            cancellation.check_before_commit()?;
            if start_row != expected_start_row {
                return Err(format!(
                    "Direct TIFF strip stream is not monotonic: got row {start_row}, expected {expected_start_row}."
                ));
            }
            let end_row = start_row
                .checked_add(row_count)
                .ok_or_else(|| "Direct TIFF row coverage overflow.".to_owned())?;
            if row_count == 0 || end_row > metadata.height {
                return Err(format!(
                    "Direct TIFF strip rows {start_row}..{end_row} exceed source height {}.",
                    metadata.height
                ));
            }
            let expected_samples = (row_count as usize)
                .checked_mul(width)
                .and_then(|pixels| pixels.checked_mul(channels))
                .ok_or_else(|| "Direct TIFF decoded strip sample count overflow.".to_owned())?;
            if input.len() != expected_samples {
                return Err(format!(
                    "Direct TIFF decoded strip contains {} samples; expected {expected_samples}.",
                    input.len()
                ));
            }

            let adjustment_started = Instant::now();
            let adjusted = export::adjusted_strip(input, metadata, project);
            let adjustment_elapsed = adjustment_started.elapsed();
            if adjusted.len() != expected_samples {
                return Err(format!(
                    "Direct TIFF adjusted strip contains {} samples; expected {expected_samples}.",
                    adjusted.len()
                ));
            }

            sender
                .send(Ok(AdjustedTiffStrip {
                    start_row,
                    row_count,
                    samples: adjusted,
                    adjustment_elapsed,
                }))
                .map_err(|_| {
                    "Conversion TIFF writer stopped before the source strip producer completed."
                        .to_owned()
                })?;
            expected_start_row = end_row;
            Ok(())
        },
    );

    let result = result.and_then(|_| {
        if expected_start_row == metadata.height {
            Ok(())
        } else {
            Err(format!(
                "Direct TIFF strip stream covered {expected_start_row} rows; expected {}.",
                metadata.height
            ))
        }
    });

    if let Err(error) = &result {
        let _ = sender.send(Err(error.clone()));
    }
    result
}

fn receive_adjusted_strip(
    receiver: &Receiver<Result<AdjustedTiffStrip, String>>,
    expected_start_row: u32,
    expected_row_count: u32,
    expected_samples: usize,
) -> Result<AdjustedTiffStrip, String> {
    let strip = receiver
        .recv()
        .map_err(|_| "Direct TIFF source strip producer ended unexpectedly.".to_owned())??;
    if strip.start_row != expected_start_row || strip.row_count != expected_row_count {
        return Err(format!(
            "Direct TIFF strip/window mismatch: writer requested rows {}–{}, producer supplied {}–{}.",
            expected_start_row + 1,
            expected_start_row.saturating_add(expected_row_count),
            strip.start_row + 1,
            strip.start_row.saturating_add(strip.row_count)
        ));
    }
    if strip.samples.len() != expected_samples {
        return Err(format!(
            "Direct TIFF adjusted strip contains {} samples; writer expected {expected_samples}.",
            strip.samples.len()
        ));
    }
    Ok(strip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receive_adjusted_strip_rejects_window_mismatch() {
        let (sender, receiver) = sync_channel(1);
        sender
            .send(Ok(AdjustedTiffStrip {
                start_row: 2,
                row_count: 1,
                samples: vec![1, 2, 3],
                adjustment_elapsed: Duration::ZERO,
            }))
            .unwrap();
        let error = receive_adjusted_strip(&receiver, 0, 1, 3).unwrap_err();
        assert!(error.contains("strip/window mismatch"), "{error}");
    }
}
