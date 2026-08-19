from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    file = Path(path)
    text = file.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one patch marker, found {count}: {old[:100]!r}")
    file.write_text(text.replace(old, new, 1), encoding="utf-8")


worker = r'''use std::fs::File;
use std::path::{Path, PathBuf};

use memmap2::MmapOptions;

use crate::color_conversion::ConversionEngineMode;
use crate::conversion_tiff::{
    ConversionTiffSpec, write_conversion_tiff_u8_atomic, write_conversion_tiff_u16_atomic,
};
use crate::conversion_transaction::{
    CapturedOutputPolicy, CommittedConversionOutput, ConversionCancellation, ConversionJobCapture,
    ConversionPhase, ConversionProgress, ConversionTransactionBackend,
};
use crate::custom_optimizer_evidence::load_and_authorize_custom_optimizer_evidence;
use crate::custom_optimizer_raster_transform::{
    MAX_CUSTOM_OPTIMIZER_RASTER_CHUNK_PIXELS, ProductionCustomOptimizerRasterTransform,
};
use crate::icc_conversion::IccSourceModel;
use crate::icc_conversion_worker::{
    conversion_spool_path, load_verified_source_icc, mmap_as_u16,
    render_adjusted_source_spool, sha256_file, source_rows, unix_time_ms, verify_file_sha256,
};
use crate::model::ShadeProject;
use crate::tiff_io::{self, ColorModel, StreamInfo};
use crate::dpi;

pub struct FilesystemCustomOptimizerConversionBackend {
    default_dpi: f64,
    replace_existing: bool,
}

impl FilesystemCustomOptimizerConversionBackend {
    pub fn new(default_dpi: f64) -> Result<Self, String> {
        if !default_dpi.is_finite() || default_dpi <= 0.0 {
            return Err("Conversion fallback DPI must be finite and positive.".to_owned());
        }
        Ok(Self {
            default_dpi,
            replace_existing: false,
        })
    }
}

impl ConversionTransactionBackend for FilesystemCustomOptimizerConversionBackend {
    fn render_convert_and_commit(
        &mut self,
        capture: &ConversionJobCapture,
        cancellation: &ConversionCancellation,
        report: &mut dyn FnMut(ConversionProgress),
    ) -> Result<CommittedConversionOutput, String> {
        cancellation.check_before_commit()?;
        if capture.conversion_recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
            return Err(
                "Custom Optimizer filesystem backend received a non-Custom-Optimizer recipe."
                    .to_owned(),
            );
        }
        self.replace_existing = capture.output_policy == CapturedOutputPolicy::TransactionalReplace;
        if !self.replace_existing
            && (capture.output_tiff_path.exists() || capture.production_project_path.exists())
        {
            return Err(
                "Queued versioned conversion destination is no longer free; review and queue a new version."
                    .to_owned(),
            );
        }

        report(ConversionProgress::new(
            ConversionPhase::CaptureValidation,
            0.02,
            "Revalidating source file and Custom Optimizer evidence",
        ));
        verify_file_sha256(
            &capture.source_face_path,
            &capture.source_file_sha256,
            "Source Face",
        )?;

        report(ConversionProgress::new(
            ConversionPhase::Decode,
            0.04,
            "Inspecting streamable Custom Optimizer source topology",
        ));
        let stream = tiff_io::stream_info(&capture.source_face_path)?;
        let source_model = custom_optimizer_source_model(&stream)?;
        if !stream.streamable {
            return Err(
                "Production conversion requires a strip/tile-streamable TIFF source; full-image fallback is disabled to preserve bounded memory."
                    .to_owned(),
            );
        }

        let evidence_capture = capture.custom_optimizer_evidence.as_ref().ok_or_else(|| {
            "Custom Optimizer production execution requires captured immutable evidence."
                .to_owned()
        })?;
        let source_icc = load_verified_source_icc(capture, &stream)?;
        let loaded = load_and_authorize_custom_optimizer_evidence(
            evidence_capture,
            &capture.conversion_recipe,
        )
        .map_err(|error| {
            format!("Custom Optimizer production evidence authorization failed: {error:?}")
        })?;

        let mut transform = ProductionCustomOptimizerRasterTransform::authorize(
            source_model,
            &source_icc,
            &loaded.lut,
            &loaded.validation,
            &evidence_capture.threshold_set,
            &evidence_capture.calibration_manifest,
            &evidence_capture.calibration_approval,
            &loaded.pcs_compatibility,
            &capture.conversion_recipe,
            &loaded.model,
        )
        .map_err(|error| {
            format!("Custom Optimizer production raster authorization failed: {error:?}")
        })?;
        if transform.eligibility() != &loaded.eligibility {
            return Err(
                "Custom Optimizer raster authorization did not reproduce the reopened evidence eligibility."
                    .to_owned(),
            );
        }
        if transform.output_channels() != capture.conversion_recipe.target.channels.len() {
            return Err(
                "Custom Optimizer runtime channel topology does not match the captured target."
                    .to_owned(),
            );
        }
        if transform.target_bit_depth() != capture.conversion_recipe.target.bit_depth {
            return Err(
                "Custom Optimizer runtime bit depth does not match the captured target."
                    .to_owned(),
            );
        }

        render_custom_optimizer_and_commit(
            capture,
            cancellation,
            report,
            &stream,
            &mut transform,
            self.default_dpi,
        )?;

        Ok(CommittedConversionOutput {
            path: capture.output_tiff_path.clone(),
            sha256: sha256_file(&capture.output_tiff_path)?,
            converted_at_unix_ms: unix_time_ms()?,
        })
    }

    fn save_production_project(
        &mut self,
        path: &Path,
        project: &ShadeProject,
    ) -> Result<(), String> {
        if project.faces.len() != 1 {
            return Err(
                "A new Production project must contain exactly one converted Face.".to_owned(),
            );
        }
        let output = PathBuf::from(&project.faces[0].path);
        if self.replace_existing {
            project.save(path, &[output])
        } else {
            project.save_new(path, &[output])
        }
    }
}

fn custom_optimizer_source_model(stream: &StreamInfo) -> Result<IccSourceModel, String> {
    let metadata = &stream.metadata;
    if metadata.samples_per_pixel != metadata.base_channel_count {
        return Err(
            "Custom Optimizer production conversion requires a pure RGB or CMYK source without extra/Spot samples."
                .to_owned(),
        );
    }
    match (metadata.color_model, metadata.samples_per_pixel) {
        (ColorModel::Rgb, 3) => Ok(IccSourceModel::Rgb),
        (ColorModel::Cmyk, 4) => Ok(IccSourceModel::Cmyk),
        _ => Err(format!(
            "Custom Optimizer production conversion requires 3-channel RGB or 4-channel CMYK source data; found {} with {} samples.",
            metadata.color_model.title(),
            metadata.samples_per_pixel
        )),
    }
}

fn render_custom_optimizer_and_commit(
    capture: &ConversionJobCapture,
    cancellation: &ConversionCancellation,
    report: &mut dyn FnMut(ConversionProgress),
    stream: &StreamInfo,
    transform: &mut ProductionCustomOptimizerRasterTransform,
    default_dpi: f64,
) -> Result<(), String> {
    let spool_path = conversion_spool_path()?;
    let result = (|| {
        render_adjusted_source_spool(capture, cancellation, report, stream, &spool_path)?;
        cancellation.check_before_commit()?;
        report(ConversionProgress::new(
            ConversionPhase::ColorConversion,
            0.52,
            "Opening bounded adjusted-source spool for Custom Optimizer",
        ));
        let spool_file = File::open(&spool_path)
            .map_err(|err| format!("Cannot reopen conversion source spool: {err}"))?;
        // SAFETY: the source spool is complete and no longer mutable while this
        // read-only mapping is alive.
        let mmap = unsafe {
            MmapOptions::new()
                .map(&spool_file)
                .map_err(|err| format!("Cannot map conversion source spool: {err}"))?
        };
        let source_samples = mmap_as_u16(&mmap)?;
        let metadata = &stream.metadata;
        let dpi = dpi::read_dpi(&capture.source_face_path, default_dpi);
        let channel_names = capture
            .conversion_recipe
            .target
            .channels
            .iter()
            .map(|channel| channel.name.clone())
            .collect::<Vec<_>>();
        let rows_per_strip = bounded_custom_optimizer_rows(metadata.width, stream.rows_per_strip)?;
        let spec = ConversionTiffSpec {
            width: metadata.width,
            height: metadata.height,
            channel_names: &channel_names,
            target_icc: None,
            dpi_x: dpi.dpi_x,
            dpi_y: dpi.dpi_y,
            orientation: metadata.orientation,
            rows_per_strip,
            force_bigtiff: false,
            replace_existing: capture.output_policy == CapturedOutputPolicy::TransactionalReplace,
        };
        let source_channels = metadata.samples_per_pixel;
        let width = usize::try_from(metadata.width)
            .map_err(|_| "Custom Optimizer source width exceeds usize.".to_owned())?;
        let height = metadata.height.max(1) as f32;

        match capture.conversion_recipe.target.bit_depth {
            16 => write_conversion_tiff_u16_atomic(
                &capture.output_tiff_path,
                &spec,
                |start_row, row_count, output| {
                    cancellation.check_before_commit()?;
                    let input =
                        source_rows(source_samples, start_row, row_count, width, source_channels)?;
                    transform
                        .transform_u16_chunk(input, output)
                        .map_err(|error| format!("Custom Optimizer raster conversion failed: {error:?}"))?;
                    report(ConversionProgress::new(
                        ConversionPhase::ColorConversion,
                        0.52 + 0.34 * (start_row.saturating_add(row_count) as f32 / height),
                        format!("Converted rows {}–{}", start_row + 1, start_row + row_count),
                    ));
                    Ok(())
                },
            ),
            8 => write_conversion_tiff_u8_atomic(
                &capture.output_tiff_path,
                &spec,
                |start_row, row_count, output| {
                    cancellation.check_before_commit()?;
                    let input =
                        source_rows(source_samples, start_row, row_count, width, source_channels)?;
                    transform
                        .transform_u8_chunk(input, output)
                        .map_err(|error| format!("Custom Optimizer raster conversion failed: {error:?}"))?;
                    report(ConversionProgress::new(
                        ConversionPhase::ColorConversion,
                        0.52 + 0.34 * (start_row.saturating_add(row_count) as f32 / height),
                        format!("Converted rows {}–{}", start_row + 1, start_row + row_count),
                    ));
                    Ok(())
                },
            ),
            depth => Err(format!(
                "Unsupported captured Custom Optimizer precision: {depth}-bit."
            )),
        }?;
        report(ConversionProgress::new(
            ConversionPhase::OutputValidation,
            0.90,
            "Custom Optimizer TIFF validated and committed",
        ));
        Ok(())
    })();
    let _ = std::fs::remove_file(&spool_path);
    result
}

fn bounded_custom_optimizer_rows(width: u32, preferred_rows: u32) -> Result<u32, String> {
    let width = usize::try_from(width)
        .map_err(|_| "Custom Optimizer source width exceeds usize.".to_owned())?;
    if width == 0 {
        return Err("Custom Optimizer source width cannot be zero.".to_owned());
    }
    if width > MAX_CUSTOM_OPTIMIZER_RASTER_CHUNK_PIXELS {
        return Err(format!(
            "Custom Optimizer source row has {width} pixels; the bounded raster chunk maximum is {MAX_CUSTOM_OPTIMIZER_RASTER_CHUNK_PIXELS}."
        ));
    }
    let max_rows = MAX_CUSTOM_OPTIMIZER_RASTER_CHUNK_PIXELS / width;
    let max_rows = u32::try_from(max_rows).unwrap_or(u32::MAX).max(1);
    Ok(preferred_rows.max(1).min(max_rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_default_dpi_is_rejected() {
        assert!(FilesystemCustomOptimizerConversionBackend::new(f64::NAN).is_err());
        assert!(FilesystemCustomOptimizerConversionBackend::new(0.0).is_err());
        assert!(FilesystemCustomOptimizerConversionBackend::new(220.0).is_ok());
    }

    #[test]
    fn row_window_never_exceeds_raster_chunk_cap() {
        let rows = bounded_custom_optimizer_rows(4096, 512).unwrap();
        assert!(rows >= 1);
        assert!((rows as usize) * 4096 <= MAX_CUSTOM_OPTIMIZER_RASTER_CHUNK_PIXELS);
        assert_eq!(bounded_custom_optimizer_rows(4096, 1).unwrap(), 1);
    }

    #[test]
    fn row_wider_than_raster_chunk_cap_is_rejected() {
        assert!(
            bounded_custom_optimizer_rows(
                (MAX_CUSTOM_OPTIMIZER_RASTER_CHUNK_PIXELS + 1) as u32,
                1,
            )
            .is_err()
        );
    }
}
'''
Path("src/custom_optimizer_conversion_worker.rs").write_text(worker, encoding="utf-8")

replace_once(
    "src/lib.rs",
    "pub mod custom_optimizer_config;\npub mod custom_optimizer_evidence;",
    "pub mod custom_optimizer_config;\npub mod custom_optimizer_conversion_worker;\npub mod custom_optimizer_evidence;",
)

icc_helper = r'''pub(crate) fn load_verified_source_icc(
    capture: &ConversionJobCapture,
    stream: &StreamInfo,
) -> Result<Vec<u8>, String> {
    let source_icc = match &capture.source_profile {
        CapturedSourceProfile::Embedded => {
            stream.metadata.icc_profile.clone().ok_or_else(|| {
                "Captured source expects an embedded ICC, but the TIFF has none.".to_owned()
            })?
        }
        CapturedSourceProfile::External { path } => fs::read(path).map_err(|err| {
            format!(
                "Cannot reopen assigned production Source ICC {}: {err}",
                path.display()
            )
        })?,
    };
    verify_bytes_sha256(
        &source_icc,
        &capture.conversion_recipe.source_profile_identity.sha256,
        "Source ICC",
    )?;
    Ok(source_icc)
}

'''
replace_once(
    "src/icc_conversion_worker.rs",
    "struct VerifiedConversionProfiles {\n",
    icc_helper + "struct VerifiedConversionProfiles {\n",
)
old_source_loader = r'''    let source_icc = match &capture.source_profile {
        CapturedSourceProfile::Embedded => {
            stream.metadata.icc_profile.clone().ok_or_else(|| {
                "Captured source expects an embedded ICC, but the TIFF has none.".to_owned()
            })?
        }
        CapturedSourceProfile::External { path } => fs::read(path).map_err(|err| {
            format!(
                "Cannot reopen assigned production Source ICC {}: {err}",
                path.display()
            )
        })?,
    };
    verify_bytes_sha256(
        &source_icc,
        &capture.conversion_recipe.source_profile_identity.sha256,
        "Source ICC",
    )?;
'''
replace_once(
    "src/icc_conversion_worker.rs",
    old_source_loader,
    "    let source_icc = load_verified_source_icc(capture, stream)?;\n",
)
for old, new in [
    ("fn render_adjusted_source_spool(\n", "pub(crate) fn render_adjusted_source_spool(\n"),
    ("fn source_rows<'a>(\n", "pub(crate) fn source_rows<'a>(\n"),
    ("fn conversion_spool_path() -> Result<PathBuf, String> {\n", "pub(crate) fn conversion_spool_path() -> Result<PathBuf, String> {\n"),
    ("fn mmap_as_u16(mmap: &memmap2::Mmap) -> Result<&[u16], String> {\n", "pub(crate) fn mmap_as_u16(mmap: &memmap2::Mmap) -> Result<&[u16], String> {\n"),
    ("fn verify_file_sha256(path: &Path, expected: &str, label: &str) -> Result<(), String> {\n", "pub(crate) fn verify_file_sha256(path: &Path, expected: &str, label: &str) -> Result<(), String> {\n"),
    ("fn unix_time_ms() -> Result<i64, String> {\n", "pub(crate) fn unix_time_ms() -> Result<i64, String> {\n"),
]:
    replace_once("src/icc_conversion_worker.rs", old, new)

replace_once(
    "src/conversion_queue.rs",
    "use crate::conversion_transaction::{\n    CommittedConversionOutput, CompletedConversionTransaction, ConversionCancellation,\n    ConversionJobCapture, ConversionPhase, ConversionTransactionOutcome,\n    run_conversion_transaction,\n};\nuse crate::icc_conversion_worker::FilesystemIccConversionBackend;\n",
    "use crate::color_conversion::ConversionEngineMode;\nuse crate::conversion_transaction::{\n    CommittedConversionOutput, CompletedConversionTransaction, ConversionCancellation,\n    ConversionJobCapture, ConversionPhase, ConversionProgress, ConversionTransactionOutcome,\n    run_conversion_transaction,\n};\nuse crate::custom_optimizer_conversion_worker::FilesystemCustomOptimizerConversionBackend;\nuse crate::icc_conversion_worker::FilesystemIccConversionBackend;\n",
)
old_dispatch = r'''                let mut backend = match FilesystemIccConversionBackend::new(spec.default_dpi) {
                    Ok(backend) => backend,
                    Err(error) => {
                        return ConversionTransactionOutcome::FailedBeforeCommit {
                            phase: ConversionPhase::CaptureValidation,
                            error,
                        };
                    }
                };
                run_conversion_transaction(&capture, &cancellation, &mut backend, |progress| {
                    let _ = worker_tx.send(ConversionQueueEvent::Progress {
                        id,
                        phase: progress.phase.label().to_owned(),
                        fraction: progress.fraction,
                        detail: progress.detail,
                    });
                })
'''
new_dispatch = r'''                let mut report_progress = |progress: ConversionProgress| {
                    let _ = worker_tx.send(ConversionQueueEvent::Progress {
                        id,
                        phase: progress.phase.label().to_owned(),
                        fraction: progress.fraction,
                        detail: progress.detail,
                    });
                };
                if capture.conversion_recipe.engine_mode == ConversionEngineMode::CustomOptimizer {
                    let mut backend =
                        match FilesystemCustomOptimizerConversionBackend::new(spec.default_dpi) {
                            Ok(backend) => backend,
                            Err(error) => {
                                return ConversionTransactionOutcome::FailedBeforeCommit {
                                    phase: ConversionPhase::CaptureValidation,
                                    error,
                                };
                            }
                        };
                    run_conversion_transaction(
                        &capture,
                        &cancellation,
                        &mut backend,
                        &mut report_progress,
                    )
                } else {
                    let mut backend = match FilesystemIccConversionBackend::new(spec.default_dpi) {
                        Ok(backend) => backend,
                        Err(error) => {
                            return ConversionTransactionOutcome::FailedBeforeCommit {
                                phase: ConversionPhase::CaptureValidation,
                                error,
                            };
                        }
                    };
                    run_conversion_transaction(
                        &capture,
                        &cancellation,
                        &mut backend,
                        &mut report_progress,
                    )
                }
'''
replace_once("src/conversion_queue.rs", old_dispatch, new_dispatch)
