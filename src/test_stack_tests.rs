use std::collections::BTreeMap;
use std::fs::File;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use tiff::encoder::{TiffEncoder, colortype};

use crate::export::{self, ExportCropRect, ExportOptions};
use crate::model::{
    AdjustmentSnapshot, ChannelAdjustment, MASTER_ADJUSTMENT_KEY, ShadeProject, TestCodePosition,
};
use crate::test_stack::{
    TestStackAnchor, TestStackLayout, export_test_stack_with_progress,
};
use crate::tiff_io::{self, decode_full};
use crate::tiff_output::source_is_bigtiff;

fn temp_folder(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "shade-test-stack-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn snapshot(id: u64, name: &str, output_level: f32) -> AdjustmentSnapshot {
    let mut master = ChannelAdjustment::default();
    master.levels.output_black = output_level;
    master.levels.output_white = output_level;
    let mut adjustments = BTreeMap::new();
    adjustments.insert(MASTER_ADJUSTMENT_KEY.to_owned(), master);

    AdjustmentSnapshot {
        id,
        name: name.to_owned(),
        created_at_unix_ms: id as i64,
        adjustments,
        exports: Vec::new(),
        history: Default::default(),
    }
}

#[test]
fn two_by_two_pipeline_renders_each_saved_snapshot_state_and_writes_same_size_tiff() {
    let folder = temp_folder("2x2");
    std::fs::create_dir_all(&folder).unwrap();
    let source = folder.join("source.tif");
    let output = folder.join("stack.tif");

    let file = File::create(&source).unwrap();
    let mut encoder = TiffEncoder::new(file).unwrap();
    let image = encoder.new_image::<colortype::Gray8>(4, 4).unwrap();
    image
        .write_data(&[
            0, 1, 2, 3,
            4, 5, 6, 7,
            8, 9, 10, 11,
            12, 13, 14, 15,
        ])
        .unwrap();

    let mut project = ShadeProject::default();
    project.test_code.enabled = false;
    project.snapshots = vec![
        snapshot(1, "Test 1", 0.0),
        snapshot(2, "Test 2", 0.25),
        snapshot(3, "Test 3", 0.50),
        snapshot(4, "Test 4", 0.75),
    ];

    export_test_stack_with_progress(
        &source,
        &output,
        &project,
        &[1, 2, 3, 4],
        TestStackLayout::TWO_BY_TWO,
        TestStackAnchor::TopLeft,
        220.0,
        ExportOptions { force_lzw: false },
        |_, _| {},
    )
    .unwrap();

    let decoded = decode_full(&output).unwrap();
    let source_info = tiff_io::stream_info(&source).unwrap();
    let output_info = tiff_io::stream_info(&output).unwrap();
    assert_eq!(decoded.metadata.width, 4);
    assert_eq!(decoded.metadata.height, 4);
    assert_eq!(output_info.metadata.width, source_info.metadata.width);
    assert_eq!(output_info.metadata.height, source_info.metadata.height);
    assert_eq!(output_info.metadata.bit_depth, source_info.metadata.bit_depth);
    assert_eq!(
        output_info.metadata.samples_per_pixel,
        source_info.metadata.samples_per_pixel
    );
    assert_eq!(output_info.metadata.color_model, source_info.metadata.color_model);
    assert_eq!(output_info.metadata.channel_names, source_info.metadata.channel_names);
    let bytes = decoded
        .samples
        .iter()
        .map(|value| (value >> 8) as u8)
        .collect::<Vec<_>>();

    // Each quadrant must come from that Snapshot's saved Master adjustment,
    // not from the currently mutable project adjustment state.
    assert_eq!(
        bytes,
        vec![
            0, 0, 64, 64,
            0, 0, 64, 64,
            128, 128, 191, 191,
            128, 128, 191, 191,
        ]
    );
    assert!(output.is_file());
    assert!(!output.with_file_name("stack.tif.test-stack.tmp").exists());

    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn direct_crop_renderer_matches_full_export_with_adjustments_and_test_code() {
    const WIDTH: usize = 64;
    const HEIGHT: usize = 64;
    const CROP: usize = 32;

    let folder = temp_folder("direct-crop-equivalence");
    std::fs::create_dir_all(&folder).unwrap();
    let source = folder.join("source.tif");
    let full_export = folder.join("full-export.tif");

    let file = File::create(&source).unwrap();
    let mut encoder = TiffEncoder::new(file).unwrap();
    let image = encoder
        .new_image::<colortype::Gray8>(WIDTH as u32, HEIGHT as u32)
        .unwrap();
    let source_pixels = (0..WIDTH * HEIGHT)
        .map(|index| 96u8.saturating_add((index % 128) as u8))
        .collect::<Vec<_>>();
    image.write_data(&source_pixels).unwrap();

    let mut project = ShadeProject::default();
    let mut master = ChannelAdjustment::default();
    master.levels.output_black = 0.15;
    master.levels.output_white = 0.85;
    project
        .adjustments
        .insert(MASTER_ADJUSTMENT_KEY.to_owned(), master);
    project.test_code.enabled = true;
    project.test_code.text = "A1".to_owned();
    project.test_code.font_size_pt = 6.0;
    project.test_code.margin_cm = 0.0;
    project.test_code.position = TestCodePosition::TopLeft;

    export::export_face_with_progress_options(
        &source,
        &full_export,
        &project,
        220.0,
        ExportOptions { force_lzw: false },
        |_, _| {},
    )
    .unwrap();

    let stream = tiff_io::stream_info(&source).unwrap();
    let direct = export::render_adjusted_crop_u16(
        &source,
        &stream,
        &project,
        220.0,
        ExportCropRect {
            x: 0,
            y: 0,
            width: CROP,
            height: CROP,
        },
        |_, _| {},
    )
    .unwrap();

    let full = decode_full(&full_export).unwrap();
    let mut expected = Vec::with_capacity(CROP * CROP);
    for y in 0..CROP {
        let start = y * WIDTH;
        expected.extend_from_slice(&full.samples[start..start + CROP]);
    }

    // The direct renderer deliberately keeps full u16 working precision until
    // the final Test Stack writer. A Gray8 source/export quantizes each sample
    // with `>> 8` and the decoder expands the stored byte back to u16 via *257.
    // Compare the direct path after the identical write/decode quantization so
    // this test checks exported TIFF equivalence rather than internal precision.
    let direct_as_written = direct
        .into_iter()
        .map(|value| u16::from((value >> 8) as u8) * 257)
        .collect::<Vec<_>>();
    assert_eq!(direct_as_written, expected);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn test_stack_preserves_bigtiff_container_choice() {
    let folder = temp_folder("bigtiff");
    std::fs::create_dir_all(&folder).unwrap();
    let source = folder.join("source.tif");
    let output = folder.join("stack.tif");

    let file = File::create(&source).unwrap();
    let mut encoder = TiffEncoder::new_big(file).unwrap();
    let image = encoder.new_image::<colortype::Gray8>(2, 2).unwrap();
    image.write_data(&[0, 1, 2, 3]).unwrap();

    let mut project = ShadeProject::default();
    project.snapshots = vec![snapshot(1, "Test 1", 0.0)];
    export_test_stack_with_progress(
        &source,
        &output,
        &project,
        &[1],
        TestStackLayout::new(1, 1).unwrap(),
        TestStackAnchor::TopLeft,
        220.0,
        ExportOptions { force_lzw: false },
        |_, _| {},
    )
    .unwrap();

    assert!(source_is_bigtiff(&source).unwrap());
    assert!(source_is_bigtiff(&output).unwrap());
    let _ = std::fs::remove_dir_all(folder);
}
