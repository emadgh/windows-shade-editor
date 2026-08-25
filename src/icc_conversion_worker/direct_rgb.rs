use super::*;

/// Convert an already-decoded RGB PNG/JPEG source directly into the staged
/// production TIFF without materializing the adjusted u16 raster to a disk
/// spool first.
///
/// The caller has already validated the decoded source model, transparency
/// policy, profile identities and output ownership. This function preserves
/// the existing `conversion_tiff` render/pre-commit contract so final TIFF
/// metadata, cancellation, staged verification and durable publication remain
/// unchanged.
pub(super) fn render_convert_and_commit(
    capture: &ConversionJobCapture,
    cancellation: &ConversionCancellation,
    report: &mut dyn FnMut(ConversionProgress),
    width: u32,
    height: u32,
    samples: &[u16],
    alpha: Option<&[u16]>,
    target_icc: Option<&[u8]>,
    transform: &mut RuntimeProductionTransform,
    default_dpi: f64,
) -> Result<(), String> {
    const CHANNELS: usize = 3;

    cancellation.check_before_commit()?;
    if width == 0 || height == 0 {
        return Err("Decoded RGB design source dimensions must be non-zero.".to_owned());
    }

    let width_usize = width as usize;
    let height_usize = height as usize;
    let pixel_count = width_usize
        .checked_mul(height_usize)
        .ok_or_else(|| "RGB production source pixel count overflow.".to_owned())?;
    let expected_samples = pixel_count
        .checked_mul(CHANNELS)
        .ok_or_else(|| "RGB production source sample count overflow.".to_owned())?;
    if samples.len() != expected_samples {
        return Err(format!(
            "Decoded RGB production source contains {} samples; expected {expected_samples}.",
            samples.len()
        ));
    }
    if let Some(alpha) = alpha {
        if alpha.len() != pixel_count {
            return Err(format!(
                "Decoded PNG alpha contains {} samples; expected {pixel_count}.",
                alpha.len()
            ));
        }
    }

    let flatten_policy = match (alpha.is_some(), capture.conversion_recipe.source_transparency_policy)
    {
        (true, Some(policy)) => Some(policy),
        (true, None) => {
            return Err(
                "PNG alpha requires the explicit flatten policy captured in the conversion recipe."
                    .to_owned(),
            );
        }
        (false, Some(_)) => {
            return Err(
                "Conversion recipe contains an alpha-flatten policy for a source without alpha."
                    .to_owned(),
            );
        }
        (false, None) => None,
    };

    let project = capture.source_recipe.materialize_project();
    let channel_names = capture
        .conversion_recipe
        .target
        .channels
        .iter()
        .map(|channel| channel.name.clone())
        .collect::<Vec<_>>();
    let dpi = dpi::DpiInfo::with_default(default_dpi);
    let spec = ConversionTiffSpec {
        width,
        height,
        channel_names: &channel_names,
        target_icc,
        dpi_x: dpi.dpi_x,
        dpi_y: dpi.dpi_y,
        orientation: None,
        rows_per_strip: height.min(DESIGN_SOURCE_ROWS_PER_STRIP).max(1),
        force_bigtiff: false,
        replace_existing: capture.output_policy == CapturedOutputPolicy::TransactionalReplace,
    };
    let height_f32 = height.max(1) as f32;
    let perf_enabled = tiff_performance::enabled();
    let logical_bytes = u64::try_from(expected_samples)
        .ok()
        .and_then(|samples| samples.checked_mul(std::mem::size_of::<u16>() as u64));
    let mut adjustment_elapsed = Duration::ZERO;

    report(ConversionProgress::new(
        ConversionPhase::SourceAdjustments,
        0.08,
        "Streaming saved RGB source adjustments directly into conversion",
    ));

    let write_result = match capture.conversion_recipe.target.bit_depth {
        16 => write_conversion_tiff_u16_atomic_with_precommit(
            &capture.output_tiff_path,
            &spec,
            |start_row, row_count, output| {
                cancellation.check_before_commit()?;
                let input = source_rows(samples, start_row, row_count, width_usize, CHANNELS)?;
                let adjustment_started = perf_enabled.then(Instant::now);
                let mut adjusted = adjust_working_rgb(input, &project)?;
                if let (Some(alpha), Some(policy)) = (alpha, flatten_policy) {
                    let start_pixel = (start_row as usize)
                        .checked_mul(width_usize)
                        .ok_or_else(|| "PNG alpha row offset overflow.".to_owned())?;
                    let row_pixels = (row_count as usize)
                        .checked_mul(width_usize)
                        .ok_or_else(|| "PNG alpha row length overflow.".to_owned())?;
                    let alpha_end = start_pixel
                        .checked_add(row_pixels)
                        .ok_or_else(|| "PNG alpha row range overflow.".to_owned())?;
                    flatten_adjusted_rgb_in_place(
                        &mut adjusted,
                        alpha
                            .get(start_pixel..alpha_end)
                            .ok_or_else(|| "PNG alpha does not contain requested rows.".to_owned())?,
                        policy,
                    )?;
                }
                if let Some(started) = adjustment_started {
                    adjustment_elapsed += started.elapsed();
                }
                transform.transform_u16_bounded(&adjusted, output, CHANNELS)?;
                let done = start_row.saturating_add(row_count) as f32 / height_f32;
                report(ConversionProgress::new(
                    ConversionPhase::ColorConversion,
                    0.10 + 0.76 * done.min(1.0),
                    format!("Adjusted and converted rows {}–{}", start_row + 1, start_row + row_count),
                ));
                Ok(())
            },
            || cancellation.check_before_commit(),
        ),
        8 => write_conversion_tiff_u8_atomic_with_precommit(
            &capture.output_tiff_path,
            &spec,
            |start_row, row_count, output| {
                cancellation.check_before_commit()?;
                let input = source_rows(samples, start_row, row_count, width_usize, CHANNELS)?;
                let adjustment_started = perf_enabled.then(Instant::now);
                let mut adjusted = adjust_working_rgb(input, &project)?;
                if let (Some(alpha), Some(policy)) = (alpha, flatten_policy) {
                    let start_pixel = (start_row as usize)
                        .checked_mul(width_usize)
                        .ok_or_else(|| "PNG alpha row offset overflow.".to_owned())?;
                    let row_pixels = (row_count as usize)
                        .checked_mul(width_usize)
                        .ok_or_else(|| "PNG alpha row length overflow.".to_owned())?;
                    let alpha_end = start_pixel
                        .checked_add(row_pixels)
                        .ok_or_else(|| "PNG alpha row range overflow.".to_owned())?;
                    flatten_adjusted_rgb_in_place(
                        &mut adjusted,
                        alpha
                            .get(start_pixel..alpha_end)
                            .ok_or_else(|| "PNG alpha does not contain requested rows.".to_owned())?,
                        policy,
                    )?;
                }
                if let Some(started) = adjustment_started {
                    adjustment_elapsed += started.elapsed();
                }
                transform.transform_u8_bounded(&adjusted, output, CHANNELS)?;
                let done = start_row.saturating_add(row_count) as f32 / height_f32;
                report(ConversionProgress::new(
                    ConversionPhase::ColorConversion,
                    0.10 + 0.76 * done.min(1.0),
                    format!("Adjusted and converted rows {}–{}", start_row + 1, start_row + row_count),
                ));
                Ok(())
            },
            || cancellation.check_before_commit(),
        ),
        depth => Err(format!(
            "Unsupported captured conversion precision: {depth}-bit."
        )),
    };

    if perf_enabled {
        tiff_performance::emit_phase_if_enabled(
            "conversion_direct_rgb",
            TiffPerfPhase::AdjustmentRender,
            adjustment_elapsed,
            logical_bytes,
        );
    }
    write_result?;

    report(ConversionProgress::new(
        ConversionPhase::OutputValidation,
        0.90,
        "Conversion TIFF validated and committed",
    ));
    Ok(())
}
