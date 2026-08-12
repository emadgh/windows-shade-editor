use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;

use crate::model::{ProjectThumbnail, ShadeProject};
use crate::render;
use crate::tiff_io::PreviewFace;

const THUMBNAIL_MAX_DIMENSION: usize = 256;

pub fn build_project_thumbnail(
    face: &PreviewFace,
    project: &ShadeProject,
) -> Result<ProjectThumbnail, String> {
    let planes = render::adjusted_planes(face, project);
    let rgba = render::rgba_from_planes(face, &planes, None);
    let (width, height, resized) =
        resize_rgba(face.width, face.height, &rgba, THUMBNAIL_MAX_DIMENSION)?;
    let png = encode_png(width as u32, height as u32, &resized)?;
    Ok(ProjectThumbnail {
        mime_type: "image/png".to_owned(),
        width: width as u32,
        height: height as u32,
        data_base64: BASE64_STANDARD.encode(png),
    })
}

fn resize_rgba(
    width: usize,
    height: usize,
    rgba: &[u8],
    max_dimension: usize,
) -> Result<(usize, usize, Vec<u8>), String> {
    if width == 0 || height == 0 {
        return Err("Cannot create thumbnail for an empty preview.".to_owned());
    }
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "Thumbnail dimensions overflow.".to_owned())?;
    if rgba.len() < expected {
        return Err("Preview RGBA data is incomplete.".to_owned());
    }

    let scale = (max_dimension as f64 / width.max(height) as f64).min(1.0);
    let out_width = ((width as f64 * scale).round() as usize).max(1);
    let out_height = ((height as f64 * scale).round() as usize).max(1);
    if out_width == width && out_height == height {
        return Ok((width, height, rgba[..expected].to_vec()));
    }

    let mut output = vec![0u8; out_width * out_height * 4];
    for y in 0..out_height {
        let source_y = ((y as f64 + 0.5) * height as f64 / out_height as f64 - 0.5)
            .round()
            .clamp(0.0, (height - 1) as f64) as usize;
        for x in 0..out_width {
            let source_x = ((x as f64 + 0.5) * width as f64 / out_width as f64 - 0.5)
                .round()
                .clamp(0.0, (width - 1) as f64) as usize;
            let source = (source_y * width + source_x) * 4;
            let target = (y * out_width + x) * 4;
            output[target..target + 4].copy_from_slice(&rgba[source..source + 4]);
        }
    }
    Ok((out_width, out_height, output))
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|err| format!("Cannot initialize project thumbnail PNG: {err}"))?;
        writer
            .write_image_data(rgba)
            .map_err(|err| format!("Cannot encode project thumbnail PNG: {err}"))?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_resize_preserves_aspect_ratio() {
        let rgba = vec![255u8; 400 * 200 * 4];
        let (width, height, output) = resize_rgba(400, 200, &rgba, 256).unwrap();
        assert_eq!((width, height), (256, 128));
        assert_eq!(output.len(), 256 * 128 * 4);
    }

    #[test]
    fn thumbnail_png_has_png_signature() {
        let rgba = vec![0u8; 4 * 4 * 4];
        let png = encode_png(4, 4, &rgba).unwrap();
        assert!(png.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]));
    }
}
