use base64::{Engine as _, engine::general_purpose::STANDARD};
use eframe::egui;
use std::{io::Cursor, sync::Arc};

const ICON_PNG_B64: &str = include_str!("../assets/shade-editor-icon.png.b64");

fn rgba_image() -> Option<(Vec<u8>, u32, u32)> {
    let compact = ICON_PNG_B64
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect::<String>();
    let png_bytes = STANDARD.decode(compact).ok()?;
    let mut decoder = png::Decoder::new(Cursor::new(png_bytes));
    // The committed icon is palette-optimized to keep repository size small.
    // Expand palette/transparency data before converting the decoded pixels to RGBA.
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buffer).ok()?;
    let bytes = &buffer[..info.buffer_size()];

    let rgba = match info.color_type {
        png::ColorType::Rgba => bytes.to_vec(),
        png::ColorType::Rgb => bytes
            .chunks_exact(3)
            .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
            .collect(),
        png::ColorType::Grayscale => bytes.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        png::ColorType::GrayscaleAlpha => bytes
            .chunks_exact(2)
            .flat_map(|ga| [ga[0], ga[0], ga[0], ga[1]])
            .collect(),
        png::ColorType::Indexed => return None,
    };

    Some((rgba, info.width, info.height))
}

pub(crate) fn viewport_icon() -> Option<Arc<egui::IconData>> {
    let (rgba, width, height) = rgba_image()?;
    Some(Arc::new(egui::IconData {
        rgba,
        width,
        height,
    }))
}

pub(crate) fn color_image() -> Option<egui::ColorImage> {
    let (rgba, width, height) = rgba_image()?;
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [width as usize, height as usize],
        &rgba,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icon_decodes_to_square_rgba_image() {
        let (rgba, width, height) = rgba_image().expect("embedded icon should decode");
        assert_eq!(width, height);
        assert_eq!(rgba.len(), width as usize * height as usize * 4);
    }
}
