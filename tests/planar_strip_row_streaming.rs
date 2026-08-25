use std::fs;
use std::path::PathBuf;

use windows_shade_editor::tiff_io::{self, ChunkStorage};

fn temp_tiff_path() -> PathBuf {
    let unique = format!(
        "shade-planar-row-stream-{}-{}.tif",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    std::env::temp_dir().join(unique)
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_ifd_entry(bytes: &mut Vec<u8>, tag: u16, field_type: u16, count: u32, value: u32) {
    push_u16(bytes, tag);
    push_u16(bytes, field_type);
    push_u32(bytes, count);
    push_u32(bytes, value);
}

fn build_planar_rgb8_strip_tiff() -> Vec<u8> {
    const ENTRY_COUNT: u16 = 10;
    const STRIP_COUNT: u32 = 9;
    let ifd_size = 2 + ENTRY_COUNT as usize * 12 + 4;
    let values_start = 8 + ifd_size;
    let bits_offset = values_start as u32;
    let strip_offsets_offset = bits_offset + 6;
    let strip_byte_counts_offset = strip_offsets_offset + STRIP_COUNT * 4;
    let pixels_offset = strip_byte_counts_offset + STRIP_COUNT * 4;
    let strip_offsets = (0..STRIP_COUNT)
        .map(|index| pixels_offset + index * 2)
        .collect::<Vec<_>>();

    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"II");
    push_u16(&mut bytes, 42);
    push_u32(&mut bytes, 8);
    push_u16(&mut bytes, ENTRY_COUNT);
    push_ifd_entry(&mut bytes, 256, 3, 1, 2);
    push_ifd_entry(&mut bytes, 257, 3, 1, 3);
    push_ifd_entry(&mut bytes, 258, 3, 3, bits_offset);
    push_ifd_entry(&mut bytes, 259, 3, 1, 1);
    push_ifd_entry(&mut bytes, 262, 3, 1, 2);
    push_ifd_entry(&mut bytes, 273, 4, STRIP_COUNT, strip_offsets_offset);
    push_ifd_entry(&mut bytes, 277, 3, 1, 3);
    push_ifd_entry(&mut bytes, 278, 4, 1, 1);
    push_ifd_entry(&mut bytes, 279, 4, STRIP_COUNT, strip_byte_counts_offset);
    push_ifd_entry(&mut bytes, 284, 3, 1, 2);
    push_u32(&mut bytes, 0);
    for _ in 0..3 {
        push_u16(&mut bytes, 8);
    }
    for offset in strip_offsets {
        push_u32(&mut bytes, offset);
    }
    for _ in 0..STRIP_COUNT {
        push_u32(&mut bytes, 2);
    }

    // Three one-row strips for Red, then Green, then Blue.
    bytes.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
    bytes.extend_from_slice(&[11, 12, 13, 14, 15, 16]);
    bytes.extend_from_slice(&[21, 22, 23, 24, 25, 26]);
    bytes
}

#[test]
fn planar_strip_decoder_produces_ordered_full_width_interleaved_rows() {
    let path = temp_tiff_path();
    fs::write(&path, build_planar_rgb8_strip_tiff()).unwrap();

    let info = tiff_io::stream_info(&path).unwrap();
    assert!(info.streamable);
    assert!(info.row_streamable);
    assert_eq!(info.storage, ChunkStorage::Strips);
    assert_eq!(info.planar_configuration, 2);
    assert_eq!(info.rows_per_strip, 1);
    assert_eq!(info.coding_unit_count, 3);

    let mut callbacks = Vec::new();
    tiff_io::for_each_decoded_strip(&path, &info, |start_row, row_count, samples| {
        callbacks.push((start_row, row_count, samples.to_vec()));
        Ok(())
    })
    .unwrap();

    assert_eq!(callbacks.len(), 3);
    assert_eq!(callbacks[0].0, 0);
    assert_eq!(callbacks[1].0, 1);
    assert_eq!(callbacks[2].0, 2);
    assert!(callbacks.iter().all(|(_, row_count, _)| *row_count == 1));
    assert_eq!(
        callbacks[0].2,
        vec![257, 2827, 5397, 514, 3084, 5654]
    );
    assert_eq!(
        callbacks[1].2,
        vec![771, 3341, 5911, 1028, 3598, 6168]
    );
    assert_eq!(
        callbacks[2].2,
        vec![1285, 3855, 6425, 1542, 4112, 6682]
    );

    let _ = fs::remove_file(path);
}
