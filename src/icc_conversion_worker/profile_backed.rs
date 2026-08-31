use crate::profile_backed_inverse_lut_artifact::load_profile_backed_inverse_lut_artifact;
use crate::profile_backed_optimizer_raster_transform::ProfileBackedCustomOptimizerRasterTransform;

/// Execute a profile-backed Custom Optimizer job through the existing bounded
/// source-adjustment spool and N-channel TIFF writer. The measured optimizer
/// core path remains byte-for-byte unchanged in the parent worker.
pub(super) fn render_convert_and_commit_profile_backed(
    backend: &mut FilesystemIccConversionBackend,
    capture: &ConversionJobCapture,
    cancellation: &ConversionCancellation,
    report: &mut dyn FnMut(ConversionProgress),
) -> Result<CommittedConversionOutput, String> {
    cancellation.check_before_commit()?;
    capture.validate()?;
    if capture.custom_optimizer_evidence.is_some() {
        return Err(
            "Profile-backed Custom Optimizer worker cannot execute a capture that also carries measured authority."
                .to_owned(),
        );
    }
    let execution = capture
        .profile_backed_optimizer_execution
        .as_ref()
        .ok_or_else(|| {
            "Profile-backed Custom Optimizer worker requires immutable execution authority."
                .to_owned()
        })?;
    execution
        .validate_for_recipe(&capture.conversion_recipe)
        .map_err(|errors| {
            format!(
                "Profile-backed Custom Optimizer capture rejected at execution: {}",
                errors.join(" ")
            )
        })?;

    backend.replace_existing =
        capture.output_policy == CapturedOutputPolicy::TransactionalReplace;
    if !backend.replace_existing && capture.output_tiff_path.exists() {
        return Err(
            "Queued conversion TIFF destination is no longer free; review route ownership and queue again."
                .to_owned(),
        );
    }

    let perf_enabled = tiff_performance::enabled();
    report(ConversionProgress::new(
        ConversionPhase::CaptureValidation,
        0.02,
        "Revalidating source file identity",
    ));
    let source_identity_bytes = perf_enabled
        .then(|| {
            fs::metadata(&capture.source_face_path)
                .ok()
                .map(|metadata| metadata.len())
        })
        .flatten();
    let source_identity_started = perf_enabled.then(Instant::now);
    verify_file_sha256(
        &capture.source_face_path,
        &capture.source_file_sha256,
        "Source Face",
    )?;
    if let Some(started) = source_identity_started {
        tiff_performance::emit_phase_if_enabled(
            "conversion",
            TiffPerfPhase::SourceIdentity,
            started.elapsed(),
            source_identity_bytes,
        );
    }

    report(ConversionProgress::new(
        ConversionPhase::Decode,
        0.04,
        "Inspecting production source topology",
    ));
    let inspect_started = perf_enabled.then(Instant::now);
    let source = ProductionSourceRaster::load(&capture.source_face_path)?;
    if let Some(started) = inspect_started {
        tiff_performance::emit_phase_if_enabled(
            "conversion",
            TiffPerfPhase::InspectDecode,
            started.elapsed(),
            source_identity_bytes,
        );
    }
    let source_model = source.source_model()?;
    source.validate_transparency_policy(&capture.conversion_recipe)?;

    report(ConversionProgress::new(
        ConversionPhase::CaptureValidation,
        0.05,
        "Reopening and authorizing profile-backed Custom Optimizer inputs",
    ));
    let source_icc = load_verified_source_icc(capture, source.embedded_icc())?;
    let output_profile_path = Path::new(&execution.authority.output_profile_path);
    let output_icc = fs::read(output_profile_path).map_err(|error| {
        format!(
            "Cannot reopen profile-backed Output ICC {}: {error}",
            output_profile_path.display()
        )
    })?;
    verify_bytes_sha256(
        &output_icc,
        &execution.authority.output_profile_sha256,
        "Profile-backed Output ICC",
    )?;

    let artifact = load_profile_backed_inverse_lut_artifact(&execution.lut_artifact_path)
        .map_err(|error| {
            format!(
                "Cannot reopen profile-backed inverse LUT artifact {}: {error}",
                execution.lut_artifact_path.display()
            )
        })?;
    if artifact.identity_content_id != execution.lut_identity_content_id {
        return Err(
            "Profile-backed inverse LUT identity changed after the production job was captured."
                .to_owned(),
        );
    }
    if artifact.payload_sha256 != execution.lut_payload_sha256 {
        return Err(
            "Profile-backed inverse LUT payload changed after the production job was captured."
                .to_owned(),
        );
    }

    let mut transform = ProfileBackedCustomOptimizerRasterTransform::authorize(
        source_model,
        &source_icc,
        &output_icc,
        &execution.authority,
        artifact,
        &capture.conversion_recipe,
    )
    .map_err(|error| {
        format!(
            "Cannot authorize profile-backed Custom Optimizer raster transform: {error:?}"
        )
    })?;
    if transform.authority() != &execution.authority {
        return Err(
            "Profile-backed Custom Optimizer raster authority changed after exact input reload."
                .to_owned(),
        );
    }
    if transform.output_channels() != capture.conversion_recipe.target.channels.len() {
        return Err(
            "Profile-backed Custom Optimizer runtime topology does not match the captured target."
                .to_owned(),
        );
    }
    if transform.target_bit_depth() != capture.conversion_recipe.target.bit_depth {
        return Err(
            "Profile-backed Custom Optimizer runtime bit depth does not match the captured target."
                .to_owned(),
        );
    }

    render_profile_backed_spool_conversion(
        capture,
        cancellation,
        report,
        &source,
        &output_icc,
        &mut transform,
        backend.default_dpi,
    )?;

    let output_identity_bytes = perf_enabled
        .then(|| {
            fs::metadata(&capture.output_tiff_path)
                .ok()
                .map(|metadata| metadata.len())
        })
        .flatten();
    let output_identity_started = perf_enabled.then(Instant::now);
    let output_sha256 = sha256_file(&capture.output_tiff_path)?;
    if let Some(started) = output_identity_started {
        tiff_performance::emit_phase_if_enabled(
            "conversion",
            TiffPerfPhase::OutputIdentity,
            started.elapsed(),
            output_identity_bytes,
        );
    }
    Ok(CommittedConversionOutput {
        path: capture.output_tiff_path.clone(),
        sha256: output_sha256,
        converted_at_unix_ms: unix_time_ms()?,
    })
}

fn render_profile_backed_spool_conversion(
    capture: &ConversionJobCapture,
    cancellation: &ConversionCancellation,
    report: &mut dyn FnMut(ConversionProgress),
    source: &ProductionSourceRaster,
    output_icc: &[u8],
    transform: &mut ProfileBackedCustomOptimizerRasterTransform,
    default_dpi: f64,
) -> Result<(), String> {
    let spool_path = conversion_spool_path()?;
    let result = (|| {
        render_adjusted_source_spool(capture, cancellation, report, source, &spool_path)?;
        cancellation.check_before_commit()?;
        report(ConversionProgress::new(
            ConversionPhase::ColorConversion,
            0.52,
            "Opening bounded adjusted-source spool for profile-backed optimization",
        ));
        let spool_file = File::open(&spool_path)
            .map_err(|error| format!("Cannot reopen conversion source spool: {error}"))?;
        // SAFETY: the source spool is complete and immutable for this read mapping.
        let mmap = unsafe {
            MmapOptions::new()
                .map(&spool_file)
                .map_err(|error| format!("Cannot map conversion source spool: {error}"))?
        };
        let source_samples = mmap_as_u16(&mmap)?;
        let (source_width, source_height) = source.dimensions();
        let dpi = source.dpi(&capture.source_face_path, default_dpi);
        let channel_names = capture
            .conversion_recipe
            .target
            .channels
            .iter()
            .map(|channel| channel.name.clone())
            .collect::<Vec<_>>();
        let spec = ConversionTiffSpec {
            width: source_width,
            height: source_height,
            channel_names: &channel_names,
            target_icc: Some(output_icc),
            dpi_x: dpi.dpi_x,
            dpi_y: dpi.dpi_y,
            orientation: source.orientation(),
            rows_per_strip: source.rows_per_strip(),
            force_bigtiff: false,
            replace_existing: capture.output_policy == CapturedOutputPolicy::TransactionalReplace,
        };
        let source_channels = source.source_channels();
        let width = source_width as usize;
        let height = source_height.max(1) as f32;
        let target_channels = transform.output_channels();

        match capture.conversion_recipe.target.bit_depth {
            16 => write_conversion_tiff_u16_atomic_with_precommit(
                &capture.output_tiff_path,
                &spec,
                |start_row, row_count, output| {
                    cancellation.check_before_commit()?;
                    let input =
                        source_rows(source_samples, start_row, row_count, width, source_channels)?;
                    transform_custom_optimizer_bounded(
                        input,
                        output,
                        source_channels,
                        target_channels,
                        |source_chunk, destination_chunk| {
                            transform
                                .transform_u16_chunk(source_chunk, destination_chunk)
                                .map_err(|error| {
                                    format!(
                                        "Profile-backed Custom Optimizer u16 raster chunk failed: {error:?}"
                                    )
                                })
                        },
                    )?;
                    report(ConversionProgress::new(
                        ConversionPhase::ColorConversion,
                        0.52 + 0.34 * (start_row.saturating_add(row_count) as f32 / height),
                        format!("Converted rows {}–{}", start_row + 1, start_row + row_count),
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
                    let input =
                        source_rows(source_samples, start_row, row_count, width, source_channels)?;
                    transform_custom_optimizer_bounded(
                        input,
                        output,
                        source_channels,
                        target_channels,
                        |source_chunk, destination_chunk| {
                            transform
                                .transform_u8_chunk(source_chunk, destination_chunk)
                                .map_err(|error| {
                                    format!(
                                        "Profile-backed Custom Optimizer u8 raster chunk failed: {error:?}"
                                    )
                                })
                        },
                    )?;
                    report(ConversionProgress::new(
                        ConversionPhase::ColorConversion,
                        0.52 + 0.34 * (start_row.saturating_add(row_count) as f32 / height),
                        format!("Converted rows {}–{}", start_row + 1, start_row + row_count),
                    ));
                    Ok(())
                },
                || cancellation.check_before_commit(),
            ),
            depth => Err(format!(
                "Unsupported captured conversion precision: {depth}-bit."
            )),
        }?;
        report(ConversionProgress::new(
            ConversionPhase::OutputValidation,
            0.90,
            "Profile-backed conversion TIFF validated and committed",
        ));
        Ok(())
    })();
    let _ = fs::remove_file(&spool_path);
    result
}
