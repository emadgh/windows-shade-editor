#![cfg(windows)]

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use png::{BitDepth, ColorType, Encoder};

use windows_shade_editor::color_conversion::{
    CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
    ConversionRenderingIntent, ConversionTargetDefinition, SeparationStrategy,
    TargetChannelDefinition,
};
use windows_shade_editor::conversion_transaction::{
    CapturedOutputPolicy, CapturedSourceColorModel, CapturedSourceFormat, CapturedSourceProfile,
    CapturedSourceRasterFacts, ConversionCancellation, ConversionJobCapture,
    ConversionTransactionBackend,
};
use windows_shade_editor::export_recipe::ExportRecipe;
use windows_shade_editor::icc_conversion_worker::{FilesystemIccConversionBackend, sha256_file};
use windows_shade_editor::model::{IccProfileIdentity, ShadeProject};
use windows_shade_editor::source_transparency::SourceTransparencyPolicy;

const RGB_BLACK_JPEG: &str = "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQH/2wBDAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQH/wAARCAABAAEDAREAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwD/AD/6AP/Z";

fn temp_path(label: &str, extension: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "shade-nontiff-production-{label}-{}-{}.{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos(),
        extension
    ))
}

fn write_rgba16_png(path: &Path) {
    let file = File::create(path).expect("create PNG fixture");
    let mut encoder = Encoder::new(file, 1, 1);
    encoder.set_color(ColorType::Rgba);
    encoder.set_depth(BitDepth::Sixteen);
    let mut writer = encoder.write_header().expect("PNG header");
    let samples = [0x1234u16, 0x4567, 0x89ab, 0xcdef];
    let bytes = samples
        .into_iter()
        .flat_map(u16::to_be_bytes)
        .collect::<Vec<_>>();
    writer.write_image_data(&bytes).expect("PNG pixels");
}

fn recipe(policy: Option<SourceTransparencyPolicy>) -> ConversionRecipe {
    ConversionRecipe {
        source_transparency_policy: policy,
        schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
        engine_mode: ConversionEngineMode::Icc,
        source_profile_identity: IccProfileIdentity {
            description: "Fixture source ICC".to_owned(),
            sha256: "a".repeat(64),
        },
        target: ConversionTargetDefinition {
            name: "Fixture CMYK".to_owned(),
            channels: ["Cyan", "Magenta", "Yellow", "Black"]
                .into_iter()
                .map(|name| TargetChannelDefinition {
                    name: name.to_owned(),
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: None,
                })
                .collect(),
            bit_depth: 16,
            output_profile_identity: Some(IccProfileIdentity {
                description: "Unreached fixture target".to_owned(),
                sha256: "b".repeat(64),
            }),
            output_profile_path: Some("unreached-fixture-target.icc".to_owned()),
            device_link_identity: None,
            device_link_path: None,
            characterization_id: None,
            total_ink_limit: None,
        },
        rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
        black_point_compensation: true,
        strategy: SeparationStrategy::default(),
        custom_optimizer_solver: None,
    }
}

fn capture(
    source: &Path,
    policy: Option<SourceTransparencyPolicy>,
    source_raster: CapturedSourceRasterFacts,
) -> ConversionJobCapture {
    let project = ShadeProject::default();
    let base = temp_path("capture", "tmp");
    ConversionJobCapture {
        source_project_path: base.with_extension("shade"),
        source_project_file_sha256: "c".repeat(64),
        source_face_path: source.to_path_buf(),
        source_snapshot_id: None,
        source_file_sha256: sha256_file(source).expect("source hash"),
        source_profile: CapturedSourceProfile::Embedded,
        source_raster: Some(source_raster),
        source_recipe: ExportRecipe::from_project(&project),
        conversion_recipe: recipe(policy),
        conversion_recipe_sha256: "d".repeat(64),
        custom_optimizer_evidence: None,
        audit_findings: Vec::new(),
        output_policy: CapturedOutputPolicy::MustNotExist,
        output_tiff_path: base.with_extension("tif"),
        production_project_path: base.with_extension("production.shade"),
        production_project_name: "Fixture production".to_owned(),
        output_face_label: "Fixture output".to_owned(),
    }
}

fn backend_error(capture: &ConversionJobCapture) -> String {
    let mut backend = FilesystemIccConversionBackend::new(220.0).expect("backend");
    backend
        .render_convert_and_commit(
            capture,
            &ConversionCancellation::default(),
            &mut |_| {},
        )
        .expect_err("fixture intentionally omits embedded ICC")
}

#[test]
fn rgba16_png_file_reaches_production_source_adapter_and_requires_explicit_alpha_policy() {
    let path = temp_path("rgba16", "png");
    write_rgba16_png(&path);
    let raster = CapturedSourceRasterFacts::new(
        CapturedSourceFormat::Png,
        CapturedSourceColorModel::Rgb,
        16,
        3,
    );

    let missing = backend_error(&capture(&path, None, raster));
    assert!(
        missing.contains("no explicit flatten background policy"),
        "{missing}"
    );

    let policy = SourceTransparencyPolicy::FlattenSolidRgb16 {
        background_rgb: [u16::MAX; 3],
    };
    let after_policy = backend_error(&capture(&path, Some(policy), raster));
    assert!(
        after_policy.contains("decoded source has none"),
        "expected real PNG decode to pass alpha policy and reach embedded ICC verification, got: {after_policy}"
    );

    let _ = fs::remove_file(path);
}

#[test]
fn rgb_jpeg_file_reaches_production_source_adapter_before_embedded_icc_verification() {
    let path = temp_path("rgb", "jpg");
    fs::write(&path, STANDARD.decode(RGB_BLACK_JPEG).expect("JPEG fixture"))
        .expect("write JPEG fixture");
    let raster = CapturedSourceRasterFacts::new(
        CapturedSourceFormat::Jpeg,
        CapturedSourceColorModel::Rgb,
        8,
        3,
    );

    let error = backend_error(&capture(&path, None, raster));
    assert!(
        error.contains("decoded source has none"),
        "expected real JPEG decode to reach embedded ICC verification, got: {error}"
    );

    let _ = fs::remove_file(path);
}
