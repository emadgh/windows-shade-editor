use std::fs;
use std::path::{Path, PathBuf};

use chrono::Local;
use serde::Serialize;

use crate::dpi;
use crate::export;
use crate::model::ShadeProject;
use crate::tiff_io::{self, ColorModel, TiffMetadata};

#[derive(Clone, Debug, Serialize)]
pub struct ValidationCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RoundtripValidationReport {
    pub shade_editor_version: String,
    pub generated_at: String,
    pub source: String,
    pub exported_tiff: String,
    pub passed: bool,
    pub checks: Vec<ValidationCheck>,
}

#[derive(Clone, Debug)]
pub struct ValidationArtifacts {
    pub report: RoundtripValidationReport,
    pub json_path: PathBuf,
    pub markdown_path: PathBuf,
}

pub fn validate_no_adjustment_roundtrip<F>(
    source: &Path,
    output_folder: &Path,
    default_dpi: f64,
    progress: F,
) -> Result<ValidationArtifacts, String>
where
    F: FnMut(f32, &str),
{
    validate_no_adjustment_roundtrip_with_options(
        source,
        output_folder,
        default_dpi,
        true,
        progress,
    )
}

pub fn validate_no_adjustment_roundtrip_with_options<F>(
    source: &Path,
    output_folder: &Path,
    default_dpi: f64,
    force_lzw: bool,
    mut progress: F,
) -> Result<ValidationArtifacts, String>
where
    F: FnMut(f32, &str),
{
    if !source.is_file() {
        return Err(format!(
            "Validation source does not exist: {}",
            source.display()
        ));
    }
    fs::create_dir_all(output_folder)
        .map_err(|err| format!("Cannot create validation folder: {err}"))?;

    progress(0.02, "Decoding source TIFF");
    let source_decoded = tiff_io::decode_full(source)?;
    let source_dpi = dpi::read_dpi(source, default_dpi);

    let mut identity_project = ShadeProject::default();
    identity_project.ensure_channels(&source_decoded.metadata.channel_names);
    identity_project.test_code.enabled = false;

    let stem = source
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "face".to_owned());
    let stamp = Local::now().format("%Y%m%d-%H%M%S");
    let base_name = format!("{stem}-roundtrip-{stamp}");
    let export_path = output_folder.join(format!("{base_name}.tif"));
    let json_path = output_folder.join(format!("{base_name}.validation.json"));
    let markdown_path = output_folder.join(format!("{base_name}.validation.md"));

    progress(0.12, "Exporting through production TIFF backend");
    export::export_face_with_progress_options(
        source,
        &export_path,
        &identity_project,
        default_dpi,
        export::ExportOptions { force_lzw },
        |fraction, detail| progress(0.12 + fraction * 0.58, detail),
    )?;

    progress(0.72, "Decoding exported TIFF");
    let exported_decoded = tiff_io::decode_full(&export_path)?;
    let exported_dpi = dpi::read_dpi(&export_path, default_dpi);

    progress(0.82, "Comparing pixels and production metadata");
    let mut checks = Vec::new();
    push_check(
        &mut checks,
        "Dimensions",
        source_decoded.metadata.width == exported_decoded.metadata.width
            && source_decoded.metadata.height == exported_decoded.metadata.height,
        format!(
            "source={}x{}, export={}x{}",
            source_decoded.metadata.width,
            source_decoded.metadata.height,
            exported_decoded.metadata.width,
            exported_decoded.metadata.height
        ),
    );
    push_check(
        &mut checks,
        "Bit depth",
        source_decoded.metadata.bit_depth == exported_decoded.metadata.bit_depth,
        format!(
            "source={} bit, export={} bit",
            source_decoded.metadata.bit_depth, exported_decoded.metadata.bit_depth
        ),
    );
    push_check(
        &mut checks,
        "Color model",
        source_decoded.metadata.color_model == exported_decoded.metadata.color_model,
        format!(
            "source={}, export={}",
            source_decoded.metadata.color_model.title(),
            exported_decoded.metadata.color_model.title()
        ),
    );
    push_check(
        &mut checks,
        "Channel count / base layout",
        source_decoded.metadata.samples_per_pixel == exported_decoded.metadata.samples_per_pixel
            && source_decoded.metadata.base_channel_count
                == exported_decoded.metadata.base_channel_count,
        format!(
            "source={}/{} base, export={}/{} base",
            source_decoded.metadata.samples_per_pixel,
            source_decoded.metadata.base_channel_count,
            exported_decoded.metadata.samples_per_pixel,
            exported_decoded.metadata.base_channel_count
        ),
    );
    push_check(
        &mut checks,
        "Channel names/order",
        source_decoded.metadata.channel_names == exported_decoded.metadata.channel_names,
        format!(
            "source={:?}, export={:?}",
            source_decoded.metadata.channel_names, exported_decoded.metadata.channel_names
        ),
    );
    push_check(
        &mut checks,
        "Decompressed samples",
        source_decoded.samples == exported_decoded.samples,
        if source_decoded.samples == exported_decoded.samples {
            format!(
                "{} samples are byte-for-byte equivalent after decode",
                source_decoded.samples.len()
            )
        } else {
            sample_difference_detail(&source_decoded.samples, &exported_decoded.samples)
        },
    );

    let expected_compression =
        expected_export_compression(source_decoded.metadata.compression, force_lzw);
    push_check(
        &mut checks,
        "Compression",
        exported_decoded.metadata.compression == expected_compression,
        format!(
            "source={:?}, expected export={:?}, actual export={:?}",
            source_decoded.metadata.compression,
            expected_compression,
            exported_decoded.metadata.compression
        ),
    );
    let expected_predictor = if source_decoded.metadata.predictor == Some(2)
        && source_decoded.metadata.samples_per_pixel == source_decoded.metadata.base_channel_count
    {
        Some(2)
    } else {
        // Horizontal Predictor is a compression transform, not image semantics.
        // The image-tiff encoder's predictor stride excludes appended
        // ExtraSamples, so production exports intentionally normalize it off
        // for Spot/extra-channel TIFFs to guarantee pixel integrity. TIFF
        // Predictor value 1 explicitly means no prediction.
        Some(1)
    };
    push_check(
        &mut checks,
        "Horizontal predictor",
        exported_decoded.metadata.predictor == expected_predictor,
        format!(
            "source={:?}, expected export={:?}, actual export={:?}{}",
            source_decoded.metadata.predictor,
            expected_predictor,
            exported_decoded.metadata.predictor,
            if source_decoded.metadata.predictor == Some(2)
                && source_decoded.metadata.samples_per_pixel
                    > source_decoded.metadata.base_channel_count
            {
                " (Predictor intentionally omitted for extra-channel TIFF pixel safety)"
            } else {
                ""
            }
        ),
    );
    push_check(
        &mut checks,
        "Orientation",
        source_decoded.metadata.orientation == exported_decoded.metadata.orientation,
        format!(
            "source={:?}, export={:?}",
            source_decoded.metadata.orientation, exported_decoded.metadata.orientation
        ),
    );
    push_check(
        &mut checks,
        "ICC profile",
        source_decoded.metadata.icc_profile == exported_decoded.metadata.icc_profile,
        byte_payload_detail(
            source_decoded.metadata.icc_profile.as_deref(),
            exported_decoded.metadata.icc_profile.as_deref(),
        ),
    );
    push_check(
        &mut checks,
        "Photoshop Image Resources 34377",
        source_decoded.metadata.photoshop_resources
            == exported_decoded.metadata.photoshop_resources,
        byte_payload_detail(
            source_decoded.metadata.photoshop_resources.as_deref(),
            exported_decoded.metadata.photoshop_resources.as_deref(),
        ),
    );
    push_check(
        &mut checks,
        "Photoshop ImageSourceData 37724",
        source_decoded.metadata.photoshop_image_source_data
            == exported_decoded.metadata.photoshop_image_source_data,
        byte_payload_detail(
            source_decoded
                .metadata
                .photoshop_image_source_data
                .as_deref(),
            exported_decoded
                .metadata
                .photoshop_image_source_data
                .as_deref(),
        ),
    );
    push_check(
        &mut checks,
        "Photoshop Spot display metadata",
        source_decoded.metadata.channel_display_info
            == exported_decoded.metadata.channel_display_info,
        spot_display_detail(&source_decoded.metadata, &exported_decoded.metadata),
    );

    let dpi_matches = dpi_equivalent(source_dpi, exported_dpi);
    push_check(
        &mut checks,
        "Physical resolution / DPI",
        dpi_matches,
        format!(
            "source={:.4}x{:.4} dpi unit {}, export={:.4}x{:.4} dpi unit {}",
            source_dpi.dpi_x,
            source_dpi.dpi_y,
            source_dpi.unit,
            exported_dpi.dpi_x,
            exported_dpi.dpi_y,
            exported_dpi.unit
        ),
    );

    let passed = checks.iter().all(|check| check.passed);
    let report = RoundtripValidationReport {
        shade_editor_version: env!("CARGO_PKG_VERSION").to_owned(),
        generated_at: Local::now().to_rfc3339(),
        source: source.display().to_string(),
        exported_tiff: export_path.display().to_string(),
        passed,
        checks,
    };

    progress(0.93, "Writing validation report");
    let json = serde_json::to_string_pretty(&report)
        .map_err(|err| format!("Cannot serialize validation report: {err}"))?;
    fs::write(&json_path, json)
        .map_err(|err| format!("Cannot write validation JSON report: {err}"))?;
    fs::write(&markdown_path, markdown_report(&report))
        .map_err(|err| format!("Cannot write validation Markdown report: {err}"))?;
    progress(
        1.0,
        if passed {
            "Validation PASS"
        } else {
            "Validation FAIL"
        },
    );

    Ok(ValidationArtifacts {
        report,
        json_path,
        markdown_path,
    })
}

pub fn validate_export_transport(source: &Path, exported: &Path) -> Result<String, String> {
    validate_export_transport_with_options(source, exported, true)
}

pub fn validate_export_transport_with_options(
    source: &Path,
    exported: &Path,
    force_lzw: bool,
) -> Result<String, String> {
    let source_info = tiff_io::stream_info(source)
        .map_err(|err| format!("Cannot inspect source TIFF for post-export validation: {err}"))?;
    let output_info = tiff_io::stream_info(exported)
        .map_err(|err| format!("Post-export TIFF validation failed while opening output: {err}"))?;
    let source_meta = &source_info.metadata;
    let output_meta = &output_info.metadata;
    let mut mismatches = Vec::new();

    if (source_meta.width, source_meta.height) != (output_meta.width, output_meta.height) {
        mismatches.push("dimensions changed".to_owned());
    }
    if source_meta.bit_depth != output_meta.bit_depth {
        mismatches.push("bit depth changed".to_owned());
    }
    if source_meta.color_model != output_meta.color_model {
        mismatches.push("color model changed".to_owned());
    }
    if source_meta.samples_per_pixel != output_meta.samples_per_pixel
        || source_meta.base_channel_count != output_meta.base_channel_count
    {
        mismatches.push("channel layout changed".to_owned());
    }
    if source_meta.channel_names != output_meta.channel_names {
        mismatches.push("channel names/order changed".to_owned());
    }
    if source_meta.icc_profile != output_meta.icc_profile {
        mismatches.push("ICC profile changed".to_owned());
    }
    if source_meta.photoshop_resources != output_meta.photoshop_resources {
        mismatches.push("Photoshop Image Resources 34377 changed".to_owned());
    }
    if source_meta.photoshop_image_source_data != output_meta.photoshop_image_source_data {
        mismatches.push("Photoshop ImageSourceData 37724 changed".to_owned());
    }
    if source_meta.orientation != output_meta.orientation {
        mismatches.push("orientation changed".to_owned());
    }
    let expected_compression = expected_export_compression(source_meta.compression, force_lzw);
    if output_meta.compression != expected_compression {
        mismatches.push(format!(
            "compression expected {:?}, got {:?}",
            expected_compression, output_meta.compression
        ));
    }
    if source_meta.predictor == Some(2)
        && source_meta.samples_per_pixel == source_meta.base_channel_count
    {
        if output_meta.predictor != Some(2) {
            mismatches.push(format!(
                "horizontal predictor expected Some(2), got {:?}",
                output_meta.predictor
            ));
        }
    } else if output_meta.predictor == Some(2)
        && output_meta.samples_per_pixel > output_meta.base_channel_count
    {
        mismatches
            .push("unsafe horizontal predictor remained enabled with ExtraSamples".to_owned());
    }
    if !mismatches.is_empty() {
        return Err(format!(
            "Post-export TIFF metadata validation failed: {}",
            mismatches.join("; ")
        ));
    }

    let mut next_row = 0u32;
    let mut decoded_samples = 0u64;
    tiff_io::for_each_decoded_strip(exported, &output_info, |row_start, row_count, samples| {
        if row_start != next_row {
            return Err(format!(
                "Post-export TIFF strip order is invalid: expected row {next_row}, got {row_start}."
            ));
        }
        let expected = u64::from(output_meta.width)
            .checked_mul(u64::from(row_count))
            .and_then(|value| value.checked_mul(output_meta.samples_per_pixel as u64))
            .ok_or_else(|| "Post-export TIFF sample count overflow.".to_owned())?;
        if samples.len() as u64 != expected {
            return Err(format!(
                "Post-export TIFF strip sample count mismatch: decoded {}, expected {expected}.",
                samples.len()
            ));
        }
        decoded_samples = decoded_samples.saturating_add(expected);
        next_row = next_row.saturating_add(row_count);
        Ok(())
    })?;
    let expected_samples = u64::from(output_meta.width)
        .checked_mul(u64::from(output_meta.height))
        .and_then(|value| value.checked_mul(output_meta.samples_per_pixel as u64))
        .ok_or_else(|| "Post-export TIFF sample count overflow.".to_owned())?;
    if next_row != output_meta.height || decoded_samples != expected_samples {
        return Err(format!(
            "Post-export TIFF decode incomplete: rows {next_row}/{}, samples {decoded_samples}/{expected_samples}.",
            output_meta.height
        ));
    }

    Ok(format!(
        "validation PASS · {} channel(s) · compression {:?} · predictor {:?}",
        output_meta.samples_per_pixel, output_meta.compression, output_meta.predictor
    ))
}

fn push_check(checks: &mut Vec<ValidationCheck>, name: &str, passed: bool, detail: String) {
    checks.push(ValidationCheck {
        name: name.to_owned(),
        passed,
        detail,
    });
}

fn expected_export_compression(source: Option<u16>, force_lzw: bool) -> Option<u16> {
    if force_lzw {
        return Some(5);
    }
    match source {
        Some(1 | 5 | 8 | 32946 | 32773) => source,
        _ => Some(5),
    }
}

fn dpi_equivalent(source: dpi::DpiInfo, output: dpi::DpiInfo) -> bool {
    if !source.has_physical_resolution {
        return output.dpi_x.is_finite() && output.dpi_y.is_finite();
    }
    source.unit == output.unit
        && (source.dpi_x - output.dpi_x).abs() <= 0.02
        && (source.dpi_y - output.dpi_y).abs() <= 0.02
}

fn sample_difference_detail(source: &[u16], output: &[u16]) -> String {
    if source.len() != output.len() {
        return format!(
            "sample count differs: source={}, export={}",
            source.len(),
            output.len()
        );
    }
    let mut differences = 0usize;
    let mut first = None;
    let mut max_delta = 0u16;
    for (index, (&a, &b)) in source.iter().zip(output).enumerate() {
        if a != b {
            differences += 1;
            first.get_or_insert((index, a, b));
            max_delta = max_delta.max(a.abs_diff(b));
        }
    }
    match first {
        Some((index, a, b)) => format!(
            "{differences} samples differ; first difference at sample {index}: {a} vs {b}; max delta={max_delta}"
        ),
        None => "samples differ for an unknown reason".to_owned(),
    }
}

fn byte_payload_detail(source: Option<&[u8]>, output: Option<&[u8]>) -> String {
    match (source, output) {
        (None, None) => "absent in both source and export".to_owned(),
        (Some(a), Some(b)) if a == b => format!("preserved exactly ({} bytes)", a.len()),
        (Some(a), Some(b)) => format!(
            "payload differs: source={} bytes, export={} bytes",
            a.len(),
            b.len()
        ),
        (Some(a), None) => format!("lost on export (source={} bytes)", a.len()),
        (None, Some(b)) => format!("unexpected payload added on export ({} bytes)", b.len()),
    }
}

fn spot_display_detail(source: &TiffMetadata, output: &TiffMetadata) -> String {
    let summarize = |metadata: &TiffMetadata| {
        metadata
            .channel_display_info
            .iter()
            .enumerate()
            .filter_map(|(index, value)| {
                let info = value.as_ref()?;
                Some(format!(
                    "{}:{} kind={} solidity={:.0}% rgb={:?}",
                    index,
                    metadata
                        .channel_names
                        .get(index)
                        .map(String::as_str)
                        .unwrap_or("?"),
                    info.kind,
                    info.solidity * 100.0,
                    info.rgb
                ))
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    format!(
        "source=[{}], export=[{}]",
        summarize(source),
        summarize(output)
    )
}

fn markdown_report(report: &RoundtripValidationReport) -> String {
    let mut text = String::new();
    text.push_str("# Shade Editor TIFF round-trip validation\n\n");
    text.push_str(&format!(
        "Overall: **{}**  \nShade Editor: `{}`  \nSource: `{}`  \nExport: `{}`  \nGenerated: `{}`\n\n",
        if report.passed { "PASS" } else { "FAIL" },
        report.shade_editor_version,
        report.source,
        report.exported_tiff,
        report.generated_at
    ));
    text.push_str("| Check | Result | Detail |\n|---|---|---|\n");
    for check in &report.checks {
        let detail = check.detail.replace('|', "\\|").replace('\n', " ");
        text.push_str(&format!(
            "| {} | {} | {} |\n",
            check.name,
            if check.passed { "PASS" } else { "FAIL" },
            detail
        ));
    }
    text.push_str("\nThis report verifies Shade Editor decode/export/redecode parity and TIFF/Photoshop metadata preservation. Photoshop and RIP application-level interpretation still require the external production check documented in `docs/PRODUCTION_VALIDATION.md`.\n");
    text
}

#[allow(dead_code)]
fn _assert_supported_model(model: ColorModel) -> bool {
    matches!(model, ColorModel::Rgb | ColorModel::Cmyk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::BufWriter;

    use tiff::encoder::{TiffEncoder, colortype};
    use tiff::tags::ExtraSamples;

    #[test]
    fn validator_passes_real_export_backend_identity_roundtrip() {
        let unique = format!(
            "shade-validator-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let folder = std::env::temp_dir().join(unique);
        fs::create_dir_all(&folder).unwrap();
        let source = folder.join("source.tif");
        let pixels = vec![
            1u8, 2, 3, 4, 5, 6, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150,
            160, 170, 180,
        ];
        {
            let file = File::create(&source).unwrap();
            let mut tiff = TiffEncoder::new(BufWriter::new(file)).unwrap();
            let mut image = tiff.new_image::<colortype::CMYK8>(2, 2).unwrap();
            image
                .extra_samples(&[ExtraSamples::Unspecified, ExtraSamples::Unspecified])
                .unwrap();
            image.rows_per_strip(1).unwrap();
            image.write_data(&pixels).unwrap();
        }

        let artifacts = validate_no_adjustment_roundtrip(&source, &folder, 220.0, |_, _| {})
            .expect("validator should run");
        assert!(artifacts.report.passed, "{:#?}", artifacts.report.checks);
        assert!(artifacts.json_path.is_file());
        assert!(artifacts.markdown_path.is_file());
        let exported = PathBuf::from(&artifacts.report.exported_tiff);
        let transport = validate_export_transport(&source, &exported)
            .expect("post-export transport validation should pass");
        assert!(transport.contains("validation PASS"));

        let _ = fs::remove_dir_all(folder);
    }
}
