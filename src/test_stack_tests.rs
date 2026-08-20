use std::fs::File;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use tiff::encoder::{TiffEncoder, colortype};

use crate::export::ExportOptions;
use crate::model::{AdjustmentSnapshot, ShadeProject};
use crate::test_stack::{
    TestStackAnchor, TestStackLayout, export_test_stack_with_progress,
};
use crate::tiff_io::decode_full;

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

fn snapshot(id: u64, name: &str) -> AdjustmentSnapshot {
    AdjustmentSnapshot {
        id,
        name: name.to_owned(),
        created_at_unix_ms: id as i64,
        adjustments: Default::default(),
        exports: Vec::new(),
        history: Default::default(),
    }
}

#[test]
fn two_by_two_pipeline_writes_same_size_corner_cropped_tiff() {
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
        snapshot(1, "Test 1"),
        snapshot(2, "Test 2"),
        snapshot(3, "Test 3"),
        snapshot(4, "Test 4"),
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
    assert_eq!(decoded.metadata.width, 4);
    assert_eq!(decoded.metadata.height, 4);
    let bytes = decoded
        .samples
        .iter()
        .map(|value| (value >> 8) as u8)
        .collect::<Vec<_>>();
    assert_eq!(
        bytes,
        vec![
            0, 1, 0, 1,
            4, 5, 4, 5,
            0, 1, 0, 1,
            4, 5, 4, 5,
        ]
    );
    assert!(output.is_file());
    assert!(!output.with_file_name("stack.tif.test-stack.tmp").exists());

    let _ = std::fs::remove_dir_all(folder);
}