from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise RuntimeError(f"missing replacement anchor: {label}")
    return text.replace(old, new, 1)


def replace_between(text: str, start: str, end: str, new: str, label: str) -> str:
    i = text.find(start)
    if i < 0:
        raise RuntimeError(f"missing start anchor: {label}")
    j = text.find(end, i)
    if j < 0:
        raise RuntimeError(f"missing end anchor: {label}")
    return text[:i] + new.rstrip() + "\n\n" + text[j:]


# Cargo / version ------------------------------------------------------------
cargo_path = Path("Cargo.toml")
cargo = cargo_path.read_text(encoding="utf-8")
cargo = replace_once(cargo, 'version = "0.10.1"', 'version = "0.10.2"', "cargo version")
cargo = replace_once(cargo, 'fontdue = "0.9.3"\n', 'fontdue = "0.9.3"\nmemmap2 = "0.9"\n', "memmap dependency")
cargo_path.write_text(cargo, encoding="utf-8")


# Model: Test Code can explicitly target every channel -----------------------
model_path = Path("src/model_v6.rs")
model = model_path.read_text(encoding="utf-8")
model = replace_once(
    model,
    "pub const SHADE_SCHEMA_VERSION: u32 = 9;\n",
    'pub const SHADE_SCHEMA_VERSION: u32 = 9;\npub const TEST_CODE_ALL_CHANNELS: &str = "__all_channels__";\n',
    "test code all-channels constant",
)
model = replace_once(
    model,
    '            channel: "Black".to_owned(),',
    '            channel: TEST_CODE_ALL_CHANNELS.to_owned(),',
    "test code default target",
)
model = replace_once(
    model,
    "        if !names.iter().any(|name| name == &self.test_code.channel) {\n",
    "        if self.test_code.channel != TEST_CODE_ALL_CHANNELS\n            && !names.iter().any(|name| name == &self.test_code.channel)\n        {\n",
    "keep all-channels target during ensure_channels",
)
model = replace_once(
    model,
    '                "Unsupported .shade schema {}. Shade Editor 0.9 accepts schema {} only; pre-production migration code has been removed.",',
    '                "Unsupported .shade schema {}. This build accepts schema {} only; pre-production migration code has been removed.",',
    "schema error wording",
)
model_path.write_text(model, encoding="utf-8")


# Export: correct compressed streaming and multi-channel Test Code -----------
export_path = Path("src/export_v6.rs")
export = export_path.read_text(encoding="utf-8")
export = replace_once(export, "use std::io::BufWriter;", "use std::io::{BufWriter, Write};", "export Write import")
export = replace_once(export, "use fontdue::{Font, FontSettings};\n", "use fontdue::{Font, FontSettings};\nuse memmap2::MmapOptions;\n", "memmap import")
export = replace_once(
    export,
    "use crate::model::{ShadeProject, TestCodePosition, apply_curve, apply_levels};",
    "use crate::model::{\n    ShadeProject, TEST_CODE_ALL_CHANNELS, TestCodePosition, apply_curve, apply_levels,\n};",
    "model import",
)

# Full-decode path: use the same multi-target overlay as streaming export.
direct_start = export.find("fn export_face_direct_with_progress")
test_start = export.find("    if project.test_code.enabled {", direct_start)
test_end = export.find('    progress(0.88, "Writing TIFF");', test_start)
if test_start < 0 or test_end < 0:
    raise RuntimeError("cannot locate direct Test Code block")
new_direct_test = '''    if let Some(overlay) = build_project_test_code_overlay(
        width,
        height,
        &decoded.metadata,
        project,
        dpi_info,
    )? {
        progress(0.82, "Rendering test code");
        apply_text_overlay_to_rows(&mut output, 0, height, width, channels, &overlay);
    }

'''
export = export[:test_start] + new_direct_test + export[test_end:]

new_streaming = r'''fn export_face_streaming<F>(
    source: &Path,
    destination: &Path,
    project: &ShadeProject,
    default_dpi: f64,
    stream: &StreamInfo,
    progress: &mut F,
) -> Result<(), String>
where
    F: FnMut(f32, &str),
{
    let metadata = &stream.metadata;
    if !matches!(metadata.color_model, ColorModel::Rgb | ColorModel::Cmyk) {
        return Err(format!(
            "Export currently supports RGB and CMYK Photoshop TIFF; this file is {}.",
            metadata.color_model.title()
        ));
    }
    let channels = metadata.samples_per_pixel;
    let base_channels = metadata.base_channel_count;
    if channels == 0 || channels < base_channels {
        return Err("Invalid TIFF channel layout.".to_owned());
    }
    if !matches!(metadata.bit_depth, 8 | 16) {
        return Err(format!(
            "Unsupported export bit depth/color model: {}-bit.",
            metadata.bit_depth
        ));
    }

    let dpi_info = dpi::read_dpi(source, default_dpi);
    let overlay = build_project_test_code_overlay(
        metadata.width as usize,
        metadata.height as usize,
        metadata,
        project,
        dpi_info,
    )?;
    let spool_path = temporary_spool_path(destination)?;

    let result = (|| -> Result<(), String> {
        progress(0.05, "Streaming adjustments to disk spool");
        {
            let spool_file = File::create(&spool_path)
                .map_err(|err| format!("Cannot create export spool: {err}"))?;
            let mut spool = BufWriter::new(spool_file);
            match metadata.bit_depth {
                8 => stream_spool_u8(
                    source,
                    stream,
                    project,
                    overlay.as_ref(),
                    &mut spool,
                    progress,
                )?,
                16 => stream_spool_u16(
                    source,
                    stream,
                    project,
                    overlay.as_ref(),
                    &mut spool,
                    progress,
                )?,
                _ => unreachable!(),
            }
            spool
                .flush()
                .map_err(|err| format!("Cannot flush export spool: {err}"))?;
        }

        let bytes_per_sample = u64::from(metadata.bit_depth / 8);
        let expected_bytes = u64::from(metadata.width)
            .checked_mul(u64::from(metadata.height))
            .and_then(|value| value.checked_mul(channels as u64))
            .and_then(|value| value.checked_mul(bytes_per_sample))
            .ok_or_else(|| "Export spool size overflow.".to_owned())?;
        let actual_bytes = fs::metadata(&spool_path)
            .map_err(|err| format!("Cannot inspect export spool: {err}"))?
            .len();
        if actual_bytes != expected_bytes {
            return Err(format!(
                "Export spool size mismatch: wrote {actual_bytes} bytes, expected {expected_bytes}."
            ));
        }

        // image-tiff 0.11.x only activates LZW/Deflate/PackBits in
        // ImageEncoder::write_data(). Direct write_strip() calls do not turn
        // the compressor on even though the Compression TIFF tag is present.
        // Keep adjustment processing strip-streamed into a disk-backed spool,
        // then memory-map that spool and let write_data() perform the final
        // correctly compressed strip encoding without allocating the full
        // image in RAM.
        progress(0.72, "Compressing TIFF from disk-backed spool");
        let spool_file = File::open(&spool_path)
            .map_err(|err| format!("Cannot reopen export spool: {err}"))?;
        // SAFETY: this mapping is read-only, the spool file is no longer
        // written after the map is created, and it stays open for the map's
        // lifetime inside this closure.
        let mmap = unsafe {
            MmapOptions::new()
                .map(&spool_file)
                .map_err(|err| format!("Cannot map export spool: {err}"))?
        };

        let file = File::create(destination)
            .map_err(|err| format!("Cannot create export TIFF: {err}"))?;
        let writer = BufWriter::new(file);
        let mut encoder = make_tiff_encoder(writer, metadata)?;

        match (metadata.color_model, metadata.bit_depth) {
            (ColorModel::Rgb, 8) => {
                let mut image = encoder
                    .new_image::<colortype::RGB8>(metadata.width, metadata.height)
                    .map_err(|err| format!("Cannot create RGB 8-bit TIFF image: {err}"))?;
                configure_extras_and_metadata(&mut image, channels, 3, metadata, dpi_info)?;
                image
                    .rows_per_strip(stream.rows_per_strip)
                    .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
                image
                    .write_data(&mmap[..])
                    .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
            }
            (ColorModel::Rgb, 16) => {
                let data = mmap_as_u16(&mmap)?;
                let mut image = encoder
                    .new_image::<colortype::RGB16>(metadata.width, metadata.height)
                    .map_err(|err| format!("Cannot create RGB 16-bit TIFF image: {err}"))?;
                configure_extras_and_metadata(&mut image, channels, 3, metadata, dpi_info)?;
                image
                    .rows_per_strip(stream.rows_per_strip)
                    .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
                image
                    .write_data(data)
                    .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
            }
            (ColorModel::Cmyk, 8) => {
                let mut image = encoder
                    .new_image::<colortype::CMYK8>(metadata.width, metadata.height)
                    .map_err(|err| format!("Cannot create CMYK 8-bit TIFF image: {err}"))?;
                configure_extras_and_metadata(&mut image, channels, 4, metadata, dpi_info)?;
                image
                    .rows_per_strip(stream.rows_per_strip)
                    .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
                image
                    .write_data(&mmap[..])
                    .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
            }
            (ColorModel::Cmyk, 16) => {
                let data = mmap_as_u16(&mmap)?;
                let mut image = encoder
                    .new_image::<colortype::CMYK16>(metadata.width, metadata.height)
                    .map_err(|err| format!("Cannot create CMYK 16-bit TIFF image: {err}"))?;
                configure_extras_and_metadata(&mut image, channels, 4, metadata, dpi_info)?;
                image
                    .rows_per_strip(stream.rows_per_strip)
                    .map_err(|err| format!("Cannot configure output strip size: {err}"))?;
                image
                    .write_data(data)
                    .map_err(|err| format!("Cannot write TIFF pixels: {err}"))?;
            }
            (_, depth) => {
                return Err(format!(
                    "Unsupported export bit depth/color model: {depth}-bit."
                ));
            }
        }

        progress(1.0, "Export complete");
        Ok(())
    })();

    let _ = fs::remove_file(&spool_path);
    result
}'''
export = replace_between(
    export,
    "fn export_face_streaming<F>(",
    "fn adjusted_strip(",
    new_streaming,
    "streaming exporter",
)

new_spool_functions = r'''fn stream_spool_u8<W, F>(
    source: &Path,
    stream: &StreamInfo,
    project: &ShadeProject,
    overlay: Option<&TextOverlay>,
    writer: &mut W,
    progress: &mut F,
) -> Result<(), String>
where
    W: Write,
    F: FnMut(f32, &str),
{
    let channels = stream.metadata.samples_per_pixel;
    let names = &stream.metadata.channel_names;
    let width = stream.metadata.width as usize;
    for_each_decoded_strip(source, stream, |row_start, row_count, input| {
        let mut adjusted = adjusted_strip(input, channels, names, project);
        if let Some(overlay) = overlay {
            apply_text_overlay_to_rows(
                &mut adjusted,
                row_start as usize,
                row_count as usize,
                width,
                channels,
                overlay,
            );
        }
        let data = adjusted
            .into_iter()
            .map(|value| (value >> 8) as u8)
            .collect::<Vec<_>>();
        let expected = row_count as usize * width * channels;
        if data.len() != expected {
            return Err(format!(
                "Output strip sample mismatch: generated {}, expected {expected}.",
                data.len()
            ));
        }
        writer
            .write_all(&data)
            .map_err(|err| format!("Cannot write export spool: {err}"))?;
        let done =
            row_start.saturating_add(row_count) as f32 / stream.metadata.height.max(1) as f32;
        progress(0.06 + done * 0.60, "Streaming adjustments to disk spool");
        Ok(())
    })
}

fn stream_spool_u16<W, F>(
    source: &Path,
    stream: &StreamInfo,
    project: &ShadeProject,
    overlay: Option<&TextOverlay>,
    writer: &mut W,
    progress: &mut F,
) -> Result<(), String>
where
    W: Write,
    F: FnMut(f32, &str),
{
    let channels = stream.metadata.samples_per_pixel;
    let names = &stream.metadata.channel_names;
    let width = stream.metadata.width as usize;
    for_each_decoded_strip(source, stream, |row_start, row_count, input| {
        let mut adjusted = adjusted_strip(input, channels, names, project);
        if let Some(overlay) = overlay {
            apply_text_overlay_to_rows(
                &mut adjusted,
                row_start as usize,
                row_count as usize,
                width,
                channels,
                overlay,
            );
        }
        let expected = row_count as usize * width * channels;
        if adjusted.len() != expected {
            return Err(format!(
                "Output strip sample mismatch: generated {}, expected {expected}.",
                adjusted.len()
            ));
        }
        let mut bytes = Vec::with_capacity(adjusted.len().saturating_mul(2));
        for value in adjusted {
            bytes.extend_from_slice(&value.to_ne_bytes());
        }
        writer
            .write_all(&bytes)
            .map_err(|err| format!("Cannot write export spool: {err}"))?;
        let done =
            row_start.saturating_add(row_count) as f32 / stream.metadata.height.max(1) as f32;
        progress(0.06 + done * 0.60, "Streaming adjustments to disk spool");
        Ok(())
    })
}

fn mmap_as_u16(mmap: &memmap2::Mmap) -> Result<&[u16], String> {
    if mmap.len() % std::mem::size_of::<u16>() != 0 {
        return Err("16-bit export spool has an odd byte length.".to_owned());
    }
    if (mmap.as_ptr() as usize) % std::mem::align_of::<u16>() != 0 {
        return Err("16-bit export spool is not aligned for u16 samples.".to_owned());
    }
    // SAFETY: length and alignment are checked above. The read-only mmap stays
    // alive for the returned slice lifetime and is never mutated concurrently.
    Ok(unsafe {
        std::slice::from_raw_parts(
            mmap.as_ptr().cast::<u16>(),
            mmap.len() / std::mem::size_of::<u16>(),
        )
    })
}'''
export = replace_between(
    export,
    "fn stream_write_u8<W, C, K, F>(",
    "fn configure_extras_and_metadata<W, C, K>(",
    new_spool_functions,
    "stream spool functions",
)

spool_path_fn = r'''fn temporary_spool_path(destination: &Path) -> Result<PathBuf, String> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("export.tif");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    for attempt in 0..32u32 {
        let candidate = parent.join(format!(
            ".{file_name}.shade-editor-spool-{}-{stamp}-{attempt}.raw",
            std::process::id()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("Cannot allocate a temporary export spool beside the destination.".to_owned())
}

'''
export = replace_once(
    export,
    "fn temporary_export_path(destination: &Path) -> Result<PathBuf, String> {",
    spool_path_fn + "fn temporary_export_path(destination: &Path) -> Result<PathBuf, String> {",
    "temporary spool path",
)

new_overlay_prefix = r'''struct TextOverlay {
    x0: usize,
    y0: usize,
    targets: Vec<(usize, u16)>,
    bitmap: TextBitmap,
}

fn test_code_target_value(metadata: &TiffMetadata, channel: usize) -> u16 {
    if channel >= metadata.base_channel_count {
        0
    } else if metadata.color_model == ColorModel::Cmyk {
        u16::MAX
    } else {
        0
    }
}

fn test_code_targets(metadata: &TiffMetadata, project: &ShadeProject) -> Vec<(usize, u16)> {
    if project.test_code.channel == TEST_CODE_ALL_CHANNELS {
        return (0..metadata.samples_per_pixel)
            .map(|channel| (channel, test_code_target_value(metadata, channel)))
            .collect();
    }
    metadata
        .channel_names
        .iter()
        .position(|name| name == &project.test_code.channel)
        .map(|channel| vec![(channel, test_code_target_value(metadata, channel))])
        .unwrap_or_default()
}

fn build_project_test_code_overlay(
    width: usize,
    height: usize,
    metadata: &TiffMetadata,
    project: &ShadeProject,
    dpi_info: DpiInfo,
) -> Result<Option<TextOverlay>, String> {
    if !project.test_code.enabled {
        return Ok(None);
    }
    let text = project.effective_test_code_text();
    if text.trim().is_empty() {
        return Ok(None);
    }
    let targets = test_code_targets(metadata, project);
    if targets.is_empty() {
        return Ok(None);
    }
    build_text_overlay(
        width,
        height,
        targets,
        &text,
        project.test_code.font_size_pt,
        project.test_code.margin_cm,
        project.test_code.position,
        dpi_info,
    )
    .map(Some)
}'''
export = replace_between(
    export,
    "struct TextOverlay {",
    "fn build_text_overlay(",
    new_overlay_prefix,
    "multi-target Test Code overlay",
)

export = replace_once(
    export,
    "    target_channel: usize,\n    target_value: u16,\n    text: &str,",
    "    targets: Vec<(usize, u16)>,\n    text: &str,",
    "build_text_overlay target signature",
)
export = replace_once(
    export,
    "        target_channel,\n        target_value,\n        bitmap,",
    "        targets,\n        bitmap,",
    "TextOverlay construction",
)
old_apply = '''            let index = (local_y * width + x) * channels + overlay.target_channel;
            if index >= samples.len() {
                continue;
            }
            let a = f32::from(alpha) / 255.0;
            let current = samples[index] as f32;
            samples[index] = (current * (1.0 - a) + overlay.target_value as f32 * a).round() as u16;'''
new_apply = '''            for &(target_channel, target_value) in &overlay.targets {
                let index = (local_y * width + x) * channels + target_channel;
                if index >= samples.len() {
                    continue;
                }
                let a = f32::from(alpha) / 255.0;
                let current = samples[index] as f32;
                samples[index] =
                    (current * (1.0 - a) + target_value as f32 * a).round() as u16;
            }'''
export = replace_once(export, old_apply, new_apply, "apply multi-target overlay")
export = replace_between(
    export,
    "fn draw_test_code(",
    "struct TextBitmap {",
    "",
    "remove obsolete single-target draw helper",
)

# Make the streaming regression reproduce the production LZW + predictor path.
export = replace_once(
    export,
    "    use tiff::encoder::{TiffEncoder, colortype};",
    "    use tiff::encoder::{Compression, Predictor, TiffEncoder, colortype};",
    "streaming test imports",
)
export = replace_once(
    export,
    "            let mut tiff = TiffEncoder::new(BufWriter::new(file)).unwrap();",
    "            let mut tiff = TiffEncoder::new(BufWriter::new(file))\n                .unwrap()\n                .with_compression(Compression::Lzw)\n                .with_predictor(Predictor::Horizontal);",
    "LZW source fixture",
)
export = replace_once(
    export,
    "        assert_eq!(info.metadata.samples_per_pixel, 6);",
    "        assert_eq!(info.metadata.samples_per_pixel, 6);\n        assert_eq!(info.metadata.compression, Some(5));\n        assert_eq!(info.metadata.predictor, Some(2));",
    "source compression assertions",
)
export = replace_once(
    export,
    "        assert_eq!(decoded_output.metadata.color_model, ColorModel::Cmyk);\n        assert_eq!(decoded_output.samples, decoded_source.samples);",
    "        assert_eq!(decoded_output.metadata.color_model, ColorModel::Cmyk);\n        assert_eq!(decoded_output.metadata.compression, Some(5));\n        assert_eq!(decoded_output.metadata.predictor, Some(2));\n        assert_eq!(decoded_output.samples, decoded_source.samples);",
    "output compression assertions",
)
export_path.write_text(export, encoding="utf-8")


# App UI: rebuild previews, robust Before/After hit test, all-channel Test Code -
app_path = Path("src/app_main.rs")
app = app_path.read_text(encoding="utf-8")
app = replace_once(
    app,
    "use model::{ChannelAdjustment, ShadeProject, TestCodePosition};",
    "use model::{ChannelAdjustment, ShadeProject, TEST_CODE_ALL_CHANNELS, TestCodePosition};",
    "app model import",
)
app = replace_once(
    app,
    "    AddFaces {\n        faces: Vec<LoadedFace>,\n        errors: Vec<String>,\n    },\n",
    "    AddFaces {\n        faces: Vec<LoadedFace>,\n        errors: Vec<String>,\n    },\n    RebuildPreviews(Result<Vec<LoadedFace>, String>),\n",
    "rebuild preview job result",
)

rebuild_method = r'''    fn rebuild_previews(&mut self) {
        if self.job.is_some() || self.faces.is_empty() {
            return;
        }
        let paths = self
            .faces
            .iter()
            .map(|face| face.path.clone())
            .collect::<Vec<_>>();
        let max_dimension = self.settings.max_preview_dimension;
        let default_dpi = self.settings.default_dpi;
        self.launch_job("Rebuilding previews", move |progress| {
            let result = (|| -> Result<Vec<LoadedFace>, String> {
                let total = paths.len().max(1);
                let mut faces = Vec::with_capacity(paths.len());
                for (index, path) in paths.into_iter().enumerate() {
                    Self::set_progress(
                        &progress,
                        Some(index as f32 / total as f32),
                        "Rebuilding previews",
                        &path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    );
                    let preview = tiff_io::load_preview(&path, max_dimension)
                        .map_err(|err| format!("{}: {err}", path.display()))?;
                    faces.push(LoadedFace {
                        dpi: dpi::read_dpi(&path, default_dpi),
                        path,
                        preview,
                    });
                }
                Self::set_progress(&progress, Some(1.0), "Rebuilding previews", "Complete");
                Ok(faces)
            })();
            JobResult::RebuildPreviews(result)
        });
    }

'''
app = replace_once(
    app,
    "    fn open_project_dialog(&mut self) {",
    rebuild_method + "    fn open_project_dialog(&mut self) {",
    "rebuild previews method",
)

rebuild_arm = r'''            JobResult::RebuildPreviews(result) => match result {
                Ok(items) => {
                    let old_generations = self
                        .faces
                        .iter()
                        .map(|face| face.generation)
                        .collect::<Vec<_>>();
                    self.faces = items
                        .into_iter()
                        .map(Self::make_runtime_face)
                        .collect();
                    for (face, old_generation) in
                        self.faces.iter_mut().zip(old_generations.into_iter())
                    {
                        face.generation = old_generation.wrapping_add(1).max(1);
                    }
                    self.current_face = self.current_face.min(self.faces.len().saturating_sub(1));
                    if let Some(face) = self.faces.get(self.current_face) {
                        let count = face.preview.metadata.channel_names.len();
                        self.selected_channel = self.selected_channel.min(count.saturating_sub(1));
                        if self.solo_channel.is_some_and(|channel| channel >= count) {
                            self.solo_channel = None;
                        }
                    }
                    self.render_busy = None;
                    self.fit_requested = true;
                    self.viewport_recenter = true;
                    self.report_info(format!(
                        "Rebuilt {} preview(s) at max dimension {}",
                        self.faces.len(),
                        self.settings.max_preview_dimension
                    ));
                }
                Err(err) => self.report_error(format!("Preview rebuild failed: {err}")),
            },
'''
app = replace_once(
    app,
    "            JobResult::Open(result) => match result {",
    rebuild_arm + "            JobResult::Open(result) => match result {",
    "rebuild preview poll arm",
)

old_before = '''                let show_before = ui.input(|input| {
                    input.pointer.secondary_down()
                        && input
                            .pointer
                            .hover_pos()
                            .is_some_and(|pos| viewport.contains(pos))
                });'''
new_before = '''                let show_before = ui.input(|input| input.pointer.secondary_down())
                    && ui.rect_contains_pointer(image_rect);'''
app = replace_once(app, old_before, new_before, "Before/After pointer hit test")

# Test Code channel picker with explicit all-channel target.
test_fn = app.find("    fn ui_test_code(&mut self, ui: &mut egui::Ui) {")
combo_start = app.find("            if !channel_names.is_empty() {", test_fn)
combo_end = app.find("            ui.horizontal(|ui| {", combo_start)
if combo_start < 0 or combo_end < 0:
    raise RuntimeError("cannot locate Test Code channel combo")
new_combo = '''            if !channel_names.is_empty() {
                let selected_display = if self.project.test_code.channel == TEST_CODE_ALL_CHANNELS {
                    "All channels".to_owned()
                } else {
                    let selected_index = channel_names
                        .iter()
                        .position(|name| name == &self.project.test_code.channel)
                        .unwrap_or(0);
                    channel_display_name(
                        palette.as_ref(),
                        &channel_names[selected_index],
                        selected_index,
                    )
                };
                egui::ComboBox::from_label("Ink / channel")
                    .selected_text(selected_display)
                    .show_ui(ui, |ui| {
                        changed |= ui
                            .selectable_value(
                                &mut self.project.test_code.channel,
                                TEST_CODE_ALL_CHANNELS.to_owned(),
                                "All channels",
                            )
                            .changed();
                        ui.separator();
                        for (index, name) in channel_names.iter().enumerate() {
                            let display = channel_display_name(palette.as_ref(), name, index);
                            changed |= ui
                                .selectable_value(
                                    &mut self.project.test_code.channel,
                                    name.clone(),
                                    display,
                                )
                                .changed();
                        }
                    });
            }
'''
app = app[:combo_start] + new_combo + app[combo_end:]

# Settings: preview max dimension can be applied to already-loaded Faces.
app = replace_once(
    app,
    "        let mut open = self.show_settings;\n        egui::Window::new(\"Settings\")",
    "        let mut open = self.show_settings;\n        let mut rebuild_previews_requested = false;\n        egui::Window::new(\"Settings\")",
    "settings rebuild request state",
)
old_slider = '''                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.settings.max_preview_dimension, 600..=4000)
                            .text("Preview max dimension"),
                    )
                    .changed();'''
new_slider = '''                ui.horizontal(|ui| {
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut self.settings.max_preview_dimension, 600..=4000)
                                .text("Preview max dimension"),
                        )
                        .changed();
                    if ui
                        .add_enabled(
                            !self.faces.is_empty() && self.job.is_none(),
                            egui::Button::new("Rebuild previews"),
                        )
                        .on_hover_text("Reload all current TIFF Faces using this preview size")
                        .clicked()
                    {
                        rebuild_previews_requested = true;
                    }
                });
                ui.small("The max dimension is used when TIFF previews are loaded. Use Rebuild previews to apply a changed value to Faces already open in this project.");'''
app = replace_once(app, old_slider, new_slider, "preview settings controls")
app = replace_once(
    app,
    "        self.show_settings = open;\n    }\n\n    fn ui_about_window",
    "        self.show_settings = open;\n        if rebuild_previews_requested {\n            self.rebuild_previews();\n        }\n    }\n\n    fn ui_about_window",
    "launch preview rebuild after settings window",
)
app_path.write_text(app, encoding="utf-8")


# Release notes / roadmap -----------------------------------------------------
notes_path = Path("RELEASE_NOTES.md")
notes = notes_path.read_text(encoding="utf-8")
release = '''# Shade Editor 0.10.2

Production export correctness and preview/Test Code workflow fixes.

- Fixes corrupt LZW/Deflate/PackBits files produced by the strip-streaming exporter. image-tiff 0.11.x activates compression in `write_data()` but not direct `write_strip()` calls; Shade Editor now streams adjustments to a bounded disk-backed spool and memory-maps it into the library's correct compressed writer path.
- Adds a regression test that exports a six-channel CMYK + 2 ExtraSamples source using LZW + horizontal predictor and then fully decodes the exported TIFF byte-for-byte.
- Before/After right-click now uses the actual image interaction rectangle and egui clipping instead of comparing screen pointer coordinates with the ScrollArea's content-relative viewport, so it works correctly while zoomed/scrolled.
- Settings now has **Rebuild previews** next to Preview max dimension to reload all open Faces at the newly selected preview size.
- Test Code can target **All channels** (the new default) or one selected channel. All-channel mode writes the same rasterized code to every separation using each channel's correct ink polarity.
- Adds a maintained roadmap document for the remaining production work.
- `.shade` schema remains v9.

'''
notes_path.write_text(release + notes, encoding="utf-8")

roadmap_path = Path("docs/ROADMAP.md")
roadmap_path.write_text('''# Shade Editor production roadmap

This file tracks only work that is still intentionally in scope. Snapshot expansion and stronger duplicate-content detection were explicitly removed from the plan.

## Current blocking validation

- Run a real no-adjustment `Validate face` round trip on production CMYK + Spot TIFFs in Photoshop and the production RIP.
- Confirm Spot type/order/name, Photoshop display color/Solidity, ICC, Photoshop resources, DPI, predictor/compression, and press/RIP interpretation.

## Next production workflow

- Relink missing Faces: Locate file, Locate folder, batch resolution of missing sources, and replacement verification against `.shade` cached metadata.
- Complete keyboard workflow: Ctrl+S, Ctrl+Shift+S, F Fit, 1-9 channel selection, S Solo, Ctrl+Enter Update Snapshot, Curve point arrow-key nudging and Shift+Arrow larger steps. Existing Ctrl+Alt+Z / Ctrl+Shift+Z history shortcuts stay unchanged.
- Optional automatic post-export validation summary for normal Export face / Export all, reusing the existing validator and showing a compact verified/failed status.

## Backend follow-up

- Extend bounded streaming to tiled and planar TIFF layouts. Normal chunky strip TIFFs already use the streaming pipeline.
- Rotate crash recovery through the latest three recovery states instead of keeping only one recovery file.
- Continue TIFF conformance regression coverage across compression, predictors, bit depth, ExtraSamples, and Photoshop metadata.

## Native Windows integration

- Windows Explorer `.shade` thumbnail provider using the embedded project PNG.
- Windows Property Handler exposing physical/pixel dimensions, DPI, bit depth, channel/Face counts, and save metadata.

## Explicitly out of scope

- More Snapshot features (notes/status/favorites/comparison/sorting).
- More duplicate detection beyond the current duplicate-reference behavior.
- Additional adjustment types until production transport and workflow validation are complete.
''', encoding="utf-8")

print("v0.10.2 patches applied")
