from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


def replace_between(text: str, start: str, end: str, replacement: str, label: str) -> str:
    start_index = text.find(start)
    if start_index < 0:
        raise RuntimeError(f"{label}: start marker not found")
    end_index = text.find(end, start_index)
    if end_index < 0:
        raise RuntimeError(f"{label}: end marker not found")
    return text[:start_index] + replacement + text[end_index:]


main_path = ROOT / "src" / "main.rs"
main = main_path.read_text(encoding="utf-8")

main = replace_once(
    main,
    "mod settings;\nmod thumbnail;",
    "mod settings;\nmod snapshot_preview_cache;\nmod thumbnail;",
    "main module declaration",
)

main = replace_once(
    main,
    """    adjusted: Vec<Vec<u16>>,\n    clipping: Vec<render::ChannelClippingStats>,\n    color_status: PreviewColorStatus,\n    texture: Option<egui::TextureHandle>,\n    original_texture: Option<egui::TextureHandle>,\n    embedded_original_texture: Option<egui::TextureHandle>,""",
    """    adjusted_histograms: Vec<[u32; 256]>,\n    clipping: Vec<render::ChannelClippingStats>,\n    color_status: PreviewColorStatus,\n    texture: Option<egui::TextureHandle>,\n    original_texture: Option<egui::TextureHandle>,\n    original_rendered_solo: Option<Option<usize>>,\n    embedded_original_texture: Option<egui::TextureHandle>,""",
    "runtime face render state",
)

main = replace_once(
    main,
    """struct RenderResult {\n    face_index: usize,\n    generation: u64,\n    adjusted: Vec<Vec<u16>>,\n    clipping: Vec<render::ChannelClippingStats>,\n    color_status: PreviewColorStatus,\n    rgba: Vec<u8>,\n    original_rgba: Vec<u8>,\n    embedded_original_rgba: Option<Vec<u8>>,\n    embedded_original_status: Option<PreviewColorStatus>,\n}\n\nstruct ErrorToast""",
    """struct RenderResult {\n    face_index: usize,\n    generation: u64,\n    solo_channel: Option<usize>,\n    adjusted_histograms: Vec<[u32; 256]>,\n    clipping: Vec<render::ChannelClippingStats>,\n    color_status: PreviewColorStatus,\n    rgba: Vec<u8>,\n    original_rgba: Vec<u8>,\n    embedded_original_rgba: Option<Vec<u8>>,\n    embedded_original_status: Option<PreviewColorStatus>,\n}\n\n#[derive(Clone)]\nstruct SnapshotPreviewEntry {\n    texture: egui::TextureHandle,\n    adjusted_histograms: Vec<[u32; 256]>,\n    clipping: Vec<render::ChannelClippingStats>,\n    color_status: PreviewColorStatus,\n}\n\nstruct ErrorToast""",
    "render result and cache entry",
)

main = replace_once(
    main,
    """    render_tx: mpsc::Sender<RenderResult>,\n    render_rx: mpsc::Receiver<RenderResult>,\n    render_busy: Option<(usize, u64)>,\n}""",
    """    render_tx: mpsc::Sender<RenderResult>,\n    render_rx: mpsc::Receiver<RenderResult>,\n    render_busy: Option<(usize, u64)>,\n    snapshot_preview_cache: snapshot_preview_cache::SnapshotPreviewCache<SnapshotPreviewEntry>,\n}""",
    "app cache field",
)

main = replace_once(
    main,
    """            render_tx,\n            render_rx,\n            render_busy: None,\n        }""",
    """            render_tx,\n            render_rx,\n            render_busy: None,\n            snapshot_preview_cache: snapshot_preview_cache::SnapshotPreviewCache::default(),\n        }""",
    "app cache initialization",
)

main = replace_once(
    main,
    """    fn bump_project_session(&mut self) {\n        self.lifecycle.bump_session();\n    }""",
    """    fn bump_project_session(&mut self) {\n        self.lifecycle.bump_session();\n        self.snapshot_preview_cache.clear();\n    }""",
    "project session cache reset",
)

main = replace_once(
    main,
    """            dpi: item.dpi,\n            adjusted: Vec::new(),\n            clipping: Vec::new(),\n            color_status: PreviewColorStatus::Pending,\n            texture: None,\n            original_texture: None,\n            embedded_original_texture: None,""",
    """            dpi: item.dpi,\n            adjusted_histograms: Vec::new(),\n            clipping: Vec::new(),\n            color_status: PreviewColorStatus::Pending,\n            texture: None,\n            original_texture: None,\n            original_rendered_solo: None,\n            embedded_original_texture: None,""",
    "runtime face initialization",
)

render_block = r'''    fn mark_all_previews_dirty(&mut self) {
        for face in &mut self.faces {
            face.generation = face.generation.wrapping_add(1).max(1);
        }
        self.project_dirty = true;
    }

    fn mark_current_preview_dirty(&mut self) {
        if let Some(face) = self.faces.get_mut(self.current_face) {
            face.generation = face.generation.wrapping_add(1).max(1);
        }
        let _ = self.restore_active_snapshot_preview();
    }

    /// Re-render textures for display-only color settings. The caller decides
    /// whether the project should be marked dirty; TIFF source/export data is never changed.
    fn invalidate_display_previews(&mut self) {
        self.snapshot_preview_cache.clear();
        for face in &mut self.faces {
            face.generation = face.generation.wrapping_add(1).max(1);
            face.color_status = PreviewColorStatus::Pending;
            face.original_rendered_solo = None;
        }
        self.render_busy = None;
    }

    fn cache_rendered_snapshot_preview(
        &mut self,
        face_index: usize,
        solo_channel: Option<usize>,
    ) -> bool {
        let Some(snapshot_id) = self.project.active_snapshot_id else {
            return false;
        };
        if !self.project.active_snapshot_matches() {
            return false;
        }
        let Some(face) = self.faces.get(face_index) else {
            return false;
        };
        if !face.available
            || face.rendered_generation != face.generation
            || face.original_rendered_solo != Some(solo_channel)
        {
            return false;
        }
        let Some(texture) = face.texture.clone() else {
            return false;
        };
        let estimated_bytes = face
            .preview
            .width
            .saturating_mul(face.preview.height)
            .saturating_mul(4);
        let entry = SnapshotPreviewEntry {
            texture,
            adjusted_histograms: face.adjusted_histograms.clone(),
            clipping: face.clipping.clone(),
            color_status: face.color_status.clone(),
        };
        self.snapshot_preview_cache.insert(
            snapshot_preview_cache::SnapshotPreviewKey::new(
                snapshot_id,
                face_index,
                solo_channel,
            ),
            entry,
            estimated_bytes,
        );
        true
    }

    fn cache_current_snapshot_preview_if_ready(&mut self) -> bool {
        self.cache_rendered_snapshot_preview(self.current_face, self.solo_channel)
    }

    fn restore_active_snapshot_preview(&mut self) -> bool {
        let Some(snapshot_id) = self.project.active_snapshot_id else {
            return false;
        };
        if !self.project.active_snapshot_matches() {
            return false;
        }
        let face_index = self.current_face;
        let solo_channel = self.solo_channel;
        let key = snapshot_preview_cache::SnapshotPreviewKey::new(
            snapshot_id,
            face_index,
            solo_channel,
        );
        let Some(entry) = self.snapshot_preview_cache.get_cloned(key) else {
            return false;
        };
        let Some(face) = self.faces.get_mut(face_index) else {
            return false;
        };
        // BEFORE uses the same display/solo mode. If that companion texture is
        // no longer current, fall through to the renderer rather than mixing states.
        if !face.available || face.original_rendered_solo != Some(solo_channel) {
            return false;
        }
        face.texture = Some(entry.texture);
        face.adjusted_histograms = entry.adjusted_histograms;
        face.clipping = entry.clipping;
        face.color_status = entry.color_status;
        face.rendered_generation = face.generation;
        true
    }

    fn poll_render(&mut self, ctx: &egui::Context) {
        while let Ok(result) = self.render_rx.try_recv() {
            let face_index = result.face_index;
            let generation = result.generation;
            let solo_channel = result.solo_channel;
            if self.render_busy == Some((face_index, generation)) {
                self.render_busy = None;
            }
            let Some(face) = self.faces.get_mut(face_index) else {
                continue;
            };
            if face.generation != generation {
                continue;
            }
            face.adjusted_histograms = result.adjusted_histograms;
            face.clipping = result.clipping;
            face.color_status = result.color_status;
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [face.preview.width, face.preview.height],
                &result.rgba,
            );
            let options = egui::TextureOptions::LINEAR;
            // Snapshot cache entries hold immutable TextureHandles. Always create
            // a fresh adjusted texture here so a later dirty render cannot mutate
            // a texture that another Snapshot is using from the cache.
            face.texture = Some(ctx.load_texture(
                format!("face-preview-{face_index}-{generation}"),
                image,
                options,
            ));
            let original_image = egui::ColorImage::from_rgba_unmultiplied(
                [face.preview.width, face.preview.height],
                &result.original_rgba,
            );
            if let Some(texture) = &mut face.original_texture {
                texture.set(original_image, options);
            } else {
                face.original_texture = Some(ctx.load_texture(
                    format!("face-original-preview-{face_index}"),
                    original_image,
                    options,
                ));
            }
            face.original_rendered_solo = Some(solo_channel);
            if let Some(source_rgba) = result.embedded_original_rgba {
                let source_image = egui::ColorImage::from_rgba_unmultiplied(
                    [face.preview.width, face.preview.height],
                    &source_rgba,
                );
                if let Some(texture) = &mut face.embedded_original_texture {
                    texture.set(source_image, options);
                } else {
                    face.embedded_original_texture = Some(ctx.load_texture(
                        format!("face-embedded-source-preview-{face_index}"),
                        source_image,
                        options,
                    ));
                }
            }
            if let Some(status) = result.embedded_original_status {
                face.embedded_original_status = status;
            }
            face.rendered_generation = generation;
            let _ = face;
            self.cache_rendered_snapshot_preview(face_index, solo_channel);
        }
    }

    fn start_render_if_needed(&mut self, ctx: &egui::Context) {
        if self.render_busy.is_some() || ctx.input(|input| input.pointer.any_down()) {
            return;
        }
        let Some(face) = self.faces.get(self.current_face) else {
            return;
        };
        if !face.available {
            return;
        }
        if face.rendered_generation == face.generation {
            return;
        }
        let face_index = self.current_face;
        let generation = face.generation;
        let needs_embedded_original = face.embedded_original_texture.is_none();
        let preview = Arc::clone(&face.preview);
        let project = self.project.clone();
        let solo_channel = self.solo_channel;
        let color_config = PreviewColorConfig::for_viewport(&self.project, &self.settings);
        let tx = self.render_tx.clone();
        self.render_busy = Some((face_index, generation));
        std::thread::spawn(move || {
            let (adjusted, clipping) = render::adjusted_planes_with_stats(&preview, &project);
            let color =
                color_management::PreviewColorTransform::new(&preview.metadata, color_config);
            let rgba =
                render::rgba_from_planes_with_color(&preview, &adjusted, solo_channel, &color);
            let original_rgba = render::rgba_from_planes_with_color(
                &preview,
                &preview.channels,
                solo_channel,
                &color,
            );
            let adjusted_histograms = adjusted
                .iter()
                .map(|values| render::histogram(values))
                .collect::<Vec<_>>();
            let color_status = color.status().clone();

            let (embedded_original_rgba, embedded_original_status) = if needs_embedded_original {
                let embedded_color = color_management::PreviewColorTransform::new(
                    &preview.metadata,
                    PreviewColorConfig {
                        enabled: true,
                        intent: PreviewRenderingIntent::Perceptual,
                        black_point_compensation: false,
                        assigned_profile_path: None,
                        assigned_profile_identity: None,
                        soft_proof_enabled: false,
                        proof_profile_path: None,
                        proof_profile_identity: None,
                        proofing_intent: PreviewRenderingIntent::RelativeColorimetric,
                        monitor_profile_path: None,
                        monitor_profile_identity: None,
                        gamut_warning: false,
                    },
                );
                let status = embedded_color.status().clone();
                let source_rgba = render::rgba_from_planes_with_color(
                    &preview,
                    &preview.channels,
                    None,
                    &embedded_color,
                );
                (Some(source_rgba), Some(status))
            } else {
                (None, None)
            };

            let _ = tx.send(RenderResult {
                face_index,
                generation,
                solo_channel,
                adjusted_histograms,
                clipping,
                color_status,
                rgba,
                original_rgba,
                embedded_original_rgba,
                embedded_original_status,
            });
        });
    }

'''

main = replace_between(
    main,
    "    fn mark_all_previews_dirty(&mut self) {",
    "    fn select_channel(&mut self, channel: usize, isolate: bool) {",
    render_block,
    "render/cache methods",
)

snapshot_apply = r'''    fn apply_snapshot_now(&mut self, id: u64) {
        self.flush_history_now();
        self.sync_history_to_active_snapshot();
        if self.project.apply_snapshot(id) {
            if let Some(snapshot) = self
                .project
                .snapshots
                .iter()
                .find(|snapshot| snapshot.id == id)
            {
                self.snapshot_rename_id = Some(id);
                self.snapshot_rename_buffer = snapshot.name.clone();
            }
            self.mark_all_previews_dirty();
            let restored = self.restore_active_snapshot_preview();
            let history_label = self
                .project
                .active_snapshot_name()
                .map(|name| format!("Snapshot - {name}"))
                .unwrap_or_else(|| "Snapshot".to_owned());
            self.load_history_for_active_snapshot(&history_label);
            self.history_clear_backup = None;
            self.history_pending_label = None;
            self.history_pending_at = None;
            if restored {
                self.report_info("Snapshot loaded · cached preview");
            } else {
                self.report_info("Snapshot loaded");
            }
        }
    }

'''
main = replace_between(
    main,
    "    fn apply_snapshot_now(&mut self, id: u64) {",
    "    fn request_snapshot_load(&mut self, id: u64) {",
    snapshot_apply,
    "snapshot activation",
)

main = replace_once(
    main,
    """            self.load_history_for_active_snapshot("Snapshot created");\n            self.history_clear_backup = None;\n            self.project_dirty = true;\n        }""",
    """            self.load_history_for_active_snapshot("Snapshot created");\n            self.history_clear_backup = None;\n            self.project_dirty = true;\n            self.cache_current_snapshot_preview_if_ready();\n        }""",
    "new snapshot cache seed",
)

main = replace_once(
    main,
    """        if delete && self.project.delete_snapshot(active_id) {\n            self.snapshot_rename_id = None;""",
    """        if delete && self.project.delete_snapshot(active_id) {\n            self.snapshot_preview_cache.remove_snapshot(active_id);\n            self.snapshot_rename_id = None;""",
    "snapshot cache delete",
)

main = replace_once(
    main,
    """        let adjusted_histograms = face\n            .adjusted\n            .iter()\n            .map(|values| render::histogram(values))\n            .collect::<Vec<_>>();""",
    """        let adjusted_histograms = face.adjusted_histograms.clone();""",
    "channel histogram cache",
)

main = replace_once(
    main,
    """        let all_adjusted_histograms = face\n            .adjusted\n            .iter()\n            .map(|values| render::histogram(values))\n            .collect::<Vec<_>>();""",
    """        let all_adjusted_histograms = face.adjusted_histograms.clone();""",
    "adjustment histogram cache",
)

main = replace_once(
    main,
    """        self.faces.remove(self.current_face);\n        if self.current_face < self.project.faces.len() {""",
    """        self.snapshot_preview_cache.clear();\n        self.faces.remove(self.current_face);\n        if self.current_face < self.project.faces.len() {""",
    "face removal cache reset",
)

main = replace_once(
    main,
    """                if added > 0 {\n                    self.current_face = self.faces.len().saturating_sub(added);""",
    """                if added > 0 {\n                    self.snapshot_preview_cache.clear();\n                    self.current_face = self.faces.len().saturating_sub(added);""",
    "add face cache reset",
)

main = replace_once(
    main,
    """                    self.faces = items.into_iter().map(Self::make_runtime_face).collect();\n                    for (face, old_generation) in""",
    """                    self.faces = items.into_iter().map(Self::make_runtime_face).collect();\n                    self.snapshot_preview_cache.clear();\n                    for (face, old_generation) in""",
    "preview rebuild cache reset",
)

main = replace_once(
    main,
    """        if self.render_busy.is_some() {\n            ui.add(\n                egui::ProgressBar::new(0.45)\n                    .desired_width(300.0)\n                    .text("Rendering preview")\n                    .animate(true),\n            );\n        }""",
    """        if self.render_busy.is_some()\n            && self\n                .faces\n                .get(self.current_face)\n                .is_some_and(|face| face.rendered_generation != face.generation)\n        {\n            ui.add(\n                egui::ProgressBar::new(0.45)\n                    .desired_width(300.0)\n                    .text("Rendering preview")\n                    .animate(true),\n            );\n        }""",
    "render progress cache awareness",
)

main_path.write_text(main, encoding="utf-8")

workflow_path = ROOT / "src" / "workflow.rs"
workflow = workflow_path.read_text(encoding="utf-8")
workflow = replace_once(
    workflow,
    """    if app.project.update_snapshot(active_id) {\n        app.project_dirty = true;\n        app.report_info("Snapshot updated");\n    }""",
    """    if app.project.update_snapshot(active_id) {\n        app.snapshot_preview_cache.remove_snapshot(active_id);\n        app.cache_current_snapshot_preview_if_ready();\n        app.project_dirty = true;\n        app.report_info("Snapshot updated · preview cache refreshed");\n    }""",
    "snapshot update cache refresh",
)
workflow = replace_once(
    workflow,
    """                app.project\n                    .ensure_channels(&item.preview.metadata.channel_names);\n                app.project.faces[index].path = item.path.to_string_lossy().into_owned();\n                app.faces[index] = ShadeApp::make_runtime_face(item);""",
    """                app.project\n                    .ensure_channels(&item.preview.metadata.channel_names);\n                app.snapshot_preview_cache.clear();\n                app.project.faces[index].path = item.path.to_string_lossy().into_owned();\n                app.faces[index] = ShadeApp::make_runtime_face(item);""",
    "single relink cache reset",
)
workflow = replace_once(
    workflow,
    """    if relinked > 0 {\n        app.project_dirty = true;""",
    """    if relinked > 0 {\n        app.snapshot_preview_cache.clear();\n        app.project_dirty = true;""",
    "folder relink cache reset",
)
workflow_path.write_text(workflow, encoding="utf-8")

cargo_path = ROOT / "Cargo.toml"
cargo = cargo_path.read_text(encoding="utf-8")
cargo = replace_once(cargo, 'version = "0.18.1"', 'version = "0.18.2"', "Cargo version")
cargo_path.write_text(cargo, encoding="utf-8")

version_path = ROOT / "VERSION"
if version_path.read_text(encoding="utf-8").strip() != "0.18.1":
    raise RuntimeError("VERSION is not 0.18.1")
version_path.write_text("0.18.2\n", encoding="utf-8")

notes_path = ROOT / "RELEASE_NOTES.md"
notes = notes_path.read_text(encoding="utf-8")
section = """# Shade Editor 0.18.2\n\n- Cache the latest rendered preview for each clean Snapshot/Face/display mode in a bounded in-memory LRU so repeated Snapshot comparisons switch immediately after the first render.\n- Refresh a Snapshot cache entry when Update commits new adjustments; dirty uncommitted edits never overwrite the saved Snapshot cache.\n- Keep cache correctness across Face changes, relinks, preview rebuilds and ICC/display changes, and bound GPU preview cache growth to 32 entries / approximately 256 MiB.\n- Store adjusted preview histograms instead of retaining full adjusted preview planes after rendering, reducing CPU memory while keeping histogram/clipping UI synchronized with cached Snapshot previews.\n\n"""
if notes.startswith("# Shade Editor 0.18.2"):
    raise RuntimeError("Release notes already contain 0.18.2")
notes_path.write_text(section + notes, encoding="utf-8")

print("Applied Shade Editor v0.18.2 snapshot preview cache patch")
