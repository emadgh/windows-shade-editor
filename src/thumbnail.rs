use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;

use crate::color_management::{PreviewColorConfig, PreviewColorTransform};
use crate::model::{ProjectThumbnail, ShadeProject};
use crate::render;
use crate::tiff_io::PreviewFace;

const THUMBNAIL_MAX_DIMENSION: usize = 512;

pub fn build_project_thumbnail(
    face: &PreviewFace,
    project: &ShadeProject,
) -> Result<ProjectThumbnail, String> {
    let planes = render::adjusted_planes(face, project);
    let color =
        PreviewColorTransform::new(&face.metadata, PreviewColorConfig::from_project(project));
    let rgba = render::rgba_from_planes_with_color(face, &planes, None, &color);
    let (width, height, resized) =
        resize_rgba(face.width, face.height, &rgba, THUMBNAIL_MAX_DIMENSION)?;
    let png = encode_png(width as u32, height as u32, &resized)?;
    let encoded_bytes = png.len() as u64;
    Ok(ProjectThumbnail {
        mime_type: "image/png".to_owned(),
        thumbnail_version: 1,
        width: width as u32,
        height: height as u32,
        encoded_bytes,
        data_base64: BASE64_STANDARD.encode(png),
    })
}

pub(crate) fn resize_rgba(
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

    // Bilinear filtering removes the blocky nearest-neighbour look that was
    // especially visible in Explorer and Previous Shades thumbnails.
    let mut output = vec![0u8; out_width * out_height * 4];
    let x_scale = width as f64 / out_width as f64;
    let y_scale = height as f64 / out_height as f64;
    for y in 0..out_height {
        let source_y = ((y as f64 + 0.5) * y_scale - 0.5).clamp(0.0, (height - 1) as f64);
        let y0 = source_y.floor() as usize;
        let y1 = (y0 + 1).min(height - 1);
        let fy = source_y - y0 as f64;
        for x in 0..out_width {
            let source_x = ((x as f64 + 0.5) * x_scale - 0.5).clamp(0.0, (width - 1) as f64);
            let x0 = source_x.floor() as usize;
            let x1 = (x0 + 1).min(width - 1);
            let fx = source_x - x0 as f64;
            let target = (y * out_width + x) * 4;
            for channel in 0..4 {
                let p00 = rgba[(y0 * width + x0) * 4 + channel] as f64;
                let p10 = rgba[(y0 * width + x1) * 4 + channel] as f64;
                let p01 = rgba[(y1 * width + x0) * 4 + channel] as f64;
                let p11 = rgba[(y1 * width + x1) * 4 + channel] as f64;
                let top = p00 + (p10 - p00) * fx;
                let bottom = p01 + (p11 - p01) * fx;
                output[target + channel] =
                    (top + (bottom - top) * fy).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Ok((out_width, out_height, output))
}

pub(crate) fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let expected = width as usize * height as usize * 4;
    if rgba.len() < expected {
        return Err("Thumbnail RGBA data is incomplete.".to_owned());
    }
    let opaque = rgba[..expected]
        .chunks_exact(4)
        .all(|pixel| pixel[3] == 255);
    let rgb = opaque.then(|| {
        let mut bytes = Vec::with_capacity(width as usize * height as usize * 3);
        for pixel in rgba[..expected].chunks_exact(4) {
            bytes.extend_from_slice(&pixel[..3]);
        }
        bytes
    });

    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(if opaque {
            png::ColorType::Rgb
        } else {
            png::ColorType::Rgba
        });
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::High);
        let mut writer = encoder
            .write_header()
            .map_err(|err| format!("Cannot initialize project thumbnail PNG: {err}"))?;
        if let Some(rgb) = rgb.as_ref() {
            writer
                .write_image_data(rgb)
                .map_err(|err| format!("Cannot encode project thumbnail PNG: {err}"))?;
        } else {
            writer
                .write_image_data(&rgba[..expected])
                .map_err(|err| format!("Cannot encode project thumbnail PNG: {err}"))?;
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_resize_preserves_aspect_ratio() {
        let rgba = vec![255u8; 1024 * 512 * 4];
        let (width, height, output) = resize_rgba(1024, 512, &rgba, 512).unwrap();
        assert_eq!((width, height), (512, 256));
        assert_eq!(output.len(), 512 * 256 * 4);
    }

    #[test]
    fn thumbnail_png_has_png_signature() {
        let rgba = vec![0u8; 4 * 4 * 4];
        let png = encode_png(4, 4, &rgba).unwrap();
        assert!(png.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]));
    }
}
