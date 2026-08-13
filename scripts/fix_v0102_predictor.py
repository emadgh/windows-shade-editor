from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise RuntimeError(f"missing predictor fix anchor: {label}")
    return text.replace(old, new, 1)


export_path = Path("src/export_v6.rs")
export = export_path.read_text(encoding="utf-8")
export = replace_once(
    export,
    "    if metadata.predictor == Some(2) {\n        encoder = encoder.with_predictor(Predictor::Horizontal);\n    }",
    "    // image-tiff's encoder Predictor stride comes from the compile-time RGB/CMYK\n    // color type and does not include appended ExtraSamples. Preserve horizontal\n    // Predictor only when the TIFF has exactly the base channels; for Spot/extra\n    // channels, omit Predictor while preserving the lossless compression itself.\n    if metadata.predictor == Some(2)\n        && metadata.samples_per_pixel == metadata.base_channel_count\n    {\n        encoder = encoder.with_predictor(Predictor::Horizontal);\n    }",
    "safe predictor policy",
)
export = replace_once(
    export,
    "    use tiff::encoder::{Compression, Predictor, TiffEncoder, colortype};",
    "    use tiff::encoder::{Compression, TiffEncoder, colortype};",
    "test predictor import",
)
export = replace_once(
    export,
    "            let mut tiff = TiffEncoder::new(BufWriter::new(file))\n                .unwrap()\n                .with_compression(Compression::Lzw)\n                .with_predictor(Predictor::Horizontal);",
    "            let mut tiff = TiffEncoder::new(BufWriter::new(file))\n                .unwrap()\n                .with_compression(Compression::Lzw);",
    "fixture encoder predictor",
)
export = replace_once(
    export,
    "    #[test]\n    fn streaming_identity_export_preserves_six_channels() {",
    '''    fn apply_dynamic_u8_predictor(data: &mut [u8], width: usize, height: usize, channels: usize) {
        let row_samples = width * channels;
        assert_eq!(data.len(), row_samples * height);
        for row in 0..height {
            let start = row * row_samples;
            for x in (1..width).rev() {
                for channel in 0..channels {
                    let index = start + x * channels + channel;
                    let previous = start + (x - 1) * channels + channel;
                    data[index] = data[index].wrapping_sub(data[previous]);
                }
            }
        }
    }

    #[test]
    fn streaming_identity_export_preserves_six_channels() {''',
    "dynamic predictor fixture helper",
)
export = replace_once(
    export,
    "            image\n                .extra_samples(&[ExtraSamples::Unspecified, ExtraSamples::Unspecified])\n                .unwrap();\n            image.rows_per_strip(1).unwrap();\n            image.write_data(&pixels).unwrap();",
    "            image\n                .extra_samples(&[ExtraSamples::Unspecified, ExtraSamples::Unspecified])\n                .unwrap();\n            // Build a valid six-channel Predictor=2 source without relying on\n            // image-tiff's base-CMYK predictor stride. The decoder must restore\n            // the original samples using SamplesPerPixel=6.\n            image.encoder().write_tag(Tag::Predictor, 2u16).unwrap();\n            image.rows_per_strip(1).unwrap();\n            let mut predicted = pixels.clone();\n            apply_dynamic_u8_predictor(&mut predicted, 2, 2, 6);\n            image.write_data(&predicted).unwrap();",
    "valid predictor fixture",
)
export = replace_once(
    export,
    "        assert_eq!(decoded_output.metadata.predictor, Some(2));\n        assert_eq!(decoded_output.samples, decoded_source.samples);",
    "        // Predictor is intentionally normalized off for extra-channel TIFFs;\n        // decoded pixel/separation data must still be exactly identical.\n        assert_ne!(decoded_output.metadata.predictor, Some(2));\n        assert_eq!(decoded_output.samples, decoded_source.samples);",
    "output predictor assertion",
)
export_path.write_text(export, encoding="utf-8")

validation_path = Path("src/validation.rs")
validation = validation_path.read_text(encoding="utf-8")
validation = replace_once(
    validation,
    '''    let expected_predictor = if source_decoded.metadata.predictor == Some(2) {
        Some(2)
    } else {
        exported_decoded.metadata.predictor
    };''',
    '''    let expected_predictor = if source_decoded.metadata.predictor == Some(2)
        && source_decoded.metadata.samples_per_pixel
            == source_decoded.metadata.base_channel_count
    {
        Some(2)
    } else {
        // Horizontal Predictor is a compression transform, not image semantics.
        // The image-tiff encoder's predictor stride excludes appended
        // ExtraSamples, so production exports intentionally normalize it off
        // for Spot/extra-channel TIFFs to guarantee pixel integrity.
        None
    };''',
    "validator predictor expectation",
)
validation = replace_once(
    validation,
    '            "source={:?}, export={:?}",\n            source_decoded.metadata.predictor, exported_decoded.metadata.predictor',
    '            "source={:?}, expected export={:?}, actual export={:?}{}",\n            source_decoded.metadata.predictor,\n            expected_predictor,\n            exported_decoded.metadata.predictor,\n            if source_decoded.metadata.predictor == Some(2)\n                && source_decoded.metadata.samples_per_pixel\n                    > source_decoded.metadata.base_channel_count\n            {\n                " (Predictor intentionally omitted for extra-channel TIFF pixel safety)"\n            } else {\n                ""\n            }',
    "validator predictor detail",
)
validation_path.write_text(validation, encoding="utf-8")

notes_path = Path("RELEASE_NOTES.md")
notes = notes_path.read_text(encoding="utf-8")
notes = replace_once(
    notes,
    "- Adds a regression test that exports a six-channel CMYK + 2 ExtraSamples source using LZW + horizontal predictor and then fully decodes the exported TIFF byte-for-byte.\n",
    "- Adds a regression test starting from a valid six-channel CMYK + 2 ExtraSamples LZW source with horizontal Predictor and fully decodes the exported TIFF byte-for-byte. Horizontal Predictor is intentionally omitted on export when ExtraSamples exist because image-tiff's built-in encoder predictor stride covers only the base RGB/CMYK samples; LZW compression itself is preserved.\n",
    "release predictor note",
)
notes_path.write_text(notes, encoding="utf-8")
