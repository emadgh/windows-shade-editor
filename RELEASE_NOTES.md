# Shade Editor 0.21.2

- Add dedicated per-Face Production Source ICC assignment, reassign and clear actions to the Color Conversion preflight UI.
- Keep production Source ICC state separate from display-only preview, Soft Proof and monitor profile settings.
- Persist only the external ICC path and stable description/SHA-256 identity; source pixels and ICC files remain unchanged.
- Validate embedded and assigned source profiles for readability, source color-space compatibility and replacement-at-path identity mismatch.
- Make assignments mark the Source project dirty so conversion can only capture the exact saved profile interpretation.
- Preserve schema-v9 compatibility: legacy Face records default to no production assignment and use embedded ICC preflight.

# Shade Editor 0.21.1

- Add the non-destructive PNG RGB/Gray source decoder foundation with RGB8 normalization and native 16-bit precision.
- Keep RGBA/Gray+Alpha opacity separate from printing channels and capture embedded ICC/sRGB metadata for later import wiring.

# Shade Editor 0.21.0

- Establish the Color Conversion architecture, preflight/UI foundation, production ICC transform layers, constrained Black-focused candidate selection, recipe identity and non-destructive output policy.
- Enforce synchronized Cargo/VERSION/lockfile build identity and versioned Windows artifact metadata.

# Shade Editor 0.20.9

- Redesign Preferences into categorized pages with a fixed category sidebar on the right, avoiding one very tall settings page.
- Add persistent History Steps configuration with a safe 10–200 range and a default of 50.
- Apply the configured history limit to live, loaded and cleared-history backup stacks; reducing the limit preserves the current state while trimming older entries.
- Show the configured history depth in Snapshot History instead of a fixed 50-state message.
- Preserve all existing Preferences options and legacy settings defaults.

# Shade Editor 0.20.8

- Match standalone Channels histogram graph backgrounds to the darker Levels and Curve surfaces.
- Preserve histogram dimensions, Light/Pigment presentation and channel accent behavior.

# Shade Editor 0.20.7

- Make Channel Mixer sliders consume the available adjustment-panel width while preserving compact labels and percent fields.
- Add a small non-interactive loading indicator over the current preview only while its exact render generation is being updated.
- Keep the previously rendered preview fully sharp and unchanged during updates; no blur, dimming or interaction blocker is introduced.

# Shade Editor 0.20.6

- Unify Curve and Levels visual language with a shared dark histogram surface and Before/After presentation.
- Add a Photoshop-like 4x4 grid to the square Curve editor.
- Restyle Curve Input/Output fields to match Levels controls.
- Remove persistent instructional prose from Levels, Mixer and Curve tool panels while preserving existing tooltips and interaction semantics.
- Preserve Curve math, Light/Pigment presentation, keyboard focus ownership and `.shade` compatibility.

# Shade Editor 0.20.5

- Integrate the approved Shade Editor icon into the Windows executable, native window and About dialog; `.shade` Explorer fallback icons resolve through `ShadeEditor.exe,0` when project thumbnails are unavailable or suppressed at small sizes.
- Make Levels input Black/Midtone/White and output Black/White markers mouse-draggable while retaining 0–255 numeric editing and Light/Pigment presentation.
- Compact Output Levels into a Photoshop-like single row with endpoint values and a draggable tonal gradient strip.
- Keep the Curve histogram/editor plot strictly 1:1 square and visually aligned with the Levels panel.
- Preserve `.shade` storage and production adjustment math.

# Shade Editor 0.20.4

- Redesign Levels around a Photoshop-style workflow with an in-panel Before/After histogram, compact Input Levels (Black/Midtone/White), Output Levels, and 0–255 sample readouts.
- Make the Levels histogram and tonal marker strip follow the existing Light/Pigment presentation mode without changing production adjustment math.
- Show Channel Mixer coefficients and Constant as integer percentages while preserving normalized float storage in `.shade` projects and render recipes.
- Quantize Mixer slider/readout interaction to 1 percentage-point units for precise editing instead of coarse decimal jumps.
- Move the focused Levels/Mixer presentation helpers into `src/ui/levels_mixer.rs` and add conversion/scale regression tests.
# Shade Editor 0.20.3

- Allow opening or creating another `.shade` project while the Export Queue is waiting or processing.
- Keep queued exports independent from the active project by relying on their immutable export recipes; switching projects does not cancel or rebind queued jobs.
- Keep Exit blocked while exports are pending so closing the process cannot terminate active queue work.
- Preserve Save/Discard/Cancel protection for dirty projects and the existing active-operation guard during project transitions.
- Add lifecycle regression tests covering Open/New/Recover during queue activity, Exit blocking, and dirty-project confirmation.

# Shade Editor 0.20.2

- Complete the typed UI-action architecture follow-up for high-value Adjustment surfaces.
- Route Undo/Redo, history clear/restore/jump, palette/channel/composite selection, settings persistence, preview invalidation and adjustment-history commits through `AdjustmentUiAction`.
- Keep Levels/Curve/Mixer value editing local to the Adjustment presentation layer; no framework rewrite or visual redesign.
- Add architecture regression coverage so Adjustment presentation cannot directly regain history/render/settings orchestration calls.
- Preserve revision-aware autosave, Snapshot history, render-generation invalidation and all existing adjustment behavior.

# Shade Editor 0.20.1

- Extract Project View transient state into a focused `ProjectViewState` instead of keeping query/sort/selection/preview/texture-cache fields directly on `ShadeApp`.
- Keep `PreviousShadesStore` as the single persistent recent-project/history owner; the new state object owns UI/session state only.
- Centralize Project View selection and cache cleanup policy with `needs_preview_load`, `clear_selection` and `forget_path` helpers.
- Add state unit tests plus an architecture regression guard preventing Project View transient fields from drifting back to top-level `ShadeApp`.
- No intended Project View, project lifecycle, TIFF, color or export behavior change.

# Shade Editor 0.20.0

- Introduce typed Face, navigation and Project View UI actions so egui presentation code no longer directly orchestrates save/export/delete/relink/lifecycle operations.
- Centralize action dispatch in `src/ui/actions.rs` while preserving the existing project lifecycle, export, Face status, relink and autosave safety paths.
- Make project-title edits emit a typed rename action so revision-aware dirty tracking remains application-owned.
- Add architecture regression coverage that rejects high-risk direct cross-domain mutations from `ui/faces.rs` and `ui/project_navigation.rs`.
- No intended editing, color, TIFF or export behavior change.

# Shade Editor 0.19.2

- Complete the main target set from Issue #40 by extracting Faces, Curve editing, Project View/Recent navigation and the typed input router into focused `src/ui` modules.
- Keep Face relink/loading workflow logic in `workflow.rs` while moving only Faces presentation/context-menu behavior to `src/ui/faces.rs`.
- Move Curve point state, graph interaction and Curve UI support out of the application shell into `src/ui/curve_editor.rs`; the Adjustments module consumes a single Curve UI entry point.
- Move Project View plus the application menu surface that owns Recent Projects into `src/ui/project_navigation.rs`.
- Move the input context router under `src/ui/input_router.rs` without changing shortcut semantics.
- Strengthen architecture regression coverage so extracted UI cannot silently accumulate back in `main.rs` or `workflow.rs`.
- Reduce `src/main.rs` from 7458 to 6347 lines and `src/workflow.rs` from 803 to 597 lines in this pass.

# Shade Editor 0.19.1

- Continue Issue #40 with a behavior-preserving UI decomposition pass: move History/Channels/Adjustments, Export Queue window and Status Bar methods out of the 300+ KB application shell into focused `src/ui` modules.
- Add an architecture regression test that prevents the extracted UI methods from silently accumulating back inside `src/main.rs`.
- Keep application/controller behavior unchanged; this release is intended as a structural maintainability update before further Face/Curve/Recent extraction.
- Reduce `src/main.rs` from 8627 to 7458 lines in this pass.

# Shade Editor 0.19.0

- Centralize keyboard/focus ownership so Curve/editor shortcuts no longer leak into text fields or modal workflows; keep Curve Arrow/Shift+Arrow editing while adding Delete/Backspace midpoint removal and Home identity reset.
- Add revision-safe smart `.shade` autosave after short edit inactivity for already-saved projects, while keeping the existing crash-recovery autosave as a separate layer and preserving Snapshot dirty guards.
- Add application-internal adjustment Copy/Paste plus a default-collapsed Relative Presets panel with cumulative Warmer, Cooler, Darker/Richer, Lighter, Redder and More beige actions and editable custom per-channel presets.
- Add persistent Accepted/Rejected Face workflow with right-click Accepted/Rejected/Delete actions, red Rejected treatment, selection warning, Rejected-last display grouping and Export All exclusion; direct rejected-Face export requires confirmation.
- Harden Face removal against stale background preview completion by invalidating generations after index shifts.
- Upgrade Export Queue QoL with persisted Pause/Resume for waiting work, Retry all failed, separate completed/failed clearing, finite progress sanitization, elapsed/ETA/approximate throughput and compact toolbar status.
- Add `File > Recent projects` backed by Project View history while preserving the centralized Save/Discard/Cancel lifecycle guard.
- Continue architecture decomposition with focused `input_router`, `adjustment_tools` and `project_autosave` modules instead of expanding the application shell with more cross-cutting logic.
- Add regression coverage for focus routing, Curve point lifecycle, relative preset accumulation, adjustment clipboard constraints, legacy Face status/defaults, rejected export filtering, autosave eligibility/revision safety and queue state/progress behavior.

# Shade Editor 0.18.6

- Fix the reproducible Export Queue crash triggered by moving the pointer over the application while an item is Processing.
- Replace the queue progress bar's infinite requested width with bounded finite UI geometry; this prevents invalid hit-test rectangles from reaching egui's pointer interaction path.
- Sanitize non-finite queue progress values before rendering and add a regression test that exercises a second hover frame over the Processing progress bar.
- Use a neutral gray highlight for the contextual selected channel while editing Master, so the channel color no longer implies that Master adjustments target that channel.

# Shade Editor 0.18.5

- Harden Export Queue runtime stability: Release builds now unwind worker panics instead of aborting the entire process, export worker panics are converted into Failed queue rows, and panic details are written to the application log.
- Move the large disk-backed export spool to a local ShadeEditor cache directory before memory mapping. Final TIFF staging/commit still happens beside the requested destination, including UNC/network destinations.
- Protect preview/background jobs from taking down the whole process and surface unexpected worker termination as an application error.
- Replace the two Light/Pigment selector buttons with one toggle button everywhere.
- Correct the user-facing Light/Pigment labels while retaining the existing serialized enum values for settings compatibility.
- Add the same tonal-direction toggle to Settings > Color guides.
- Add regression coverage for worker panic isolation, local spool placement, and queue polling/enqueue activity while an export is processing.

# Shade Editor 0.18.4

- Curve keyboard editing: the selected point moves 1 unit with Arrow keys and 10 units with Shift+Arrow.
- Snapshot dirty guard now offers Stay / Discard / Update snapshot both when switching Snapshots and before project Save/Quick Save.
- Project Save can no longer silently persist a working adjustment state while leaving the active Snapshot stale.
- `~` now toggles Master ↔ selected-channel editing; selecting a channel from keyboard or Channels immediately exits Master.
- Master replaces the previous All channels wording across the editing UI/history.
- History follows newly appended states automatically.
- Added persistent Light / Pigment tonal display modes. Pigment mirrors Curve axes, point values and histograms while keeping 0-255 labels; production adjustment math and TIFF export remain unchanged.

# Shade Editor 0.18.3

- Replace destructive Levels/Curve Broadcast-to-all behavior with independent Photoshop-style Master Levels and Master Curve controls stored as a separate All Channels adjustment state.
- Stack Master Levels/Curve with per-channel edits in preview and full-resolution export without copying, overriding or discarding channel-specific adjustments.
- Keep Channel Mixer output rows per-channel; All Channels no longer mutates individual Levels/Curve controls or their enabled state.
- Use a neutral aggregated histogram for Master Curve instead of borrowing the currently selected channel color/histogram.
- Add `~` / backtick as the All Channels shortcut and return Solo view to Composite when used.
- Preserve Master adjustment state automatically in Snapshots, History and compact Export Queue recipes without changing the `.shade` schema.

# Shade Editor 0.18.2

- Cache the latest rendered preview for each clean Snapshot/Face/display mode in a bounded in-memory LRU so repeated Snapshot comparisons switch immediately after the first render.
- Refresh a Snapshot cache entry when Update commits new adjustments; dirty uncommitted edits never overwrite the saved Snapshot cache.
- Keep cache correctness across Face changes, relinks, preview rebuilds and ICC/display changes, and bound GPU preview cache growth to 32 entries / approximately 256 MiB.
- Store adjusted preview histograms instead of retaining full adjusted preview planes after rendering, reducing CPU memory while keeping histogram/clipping UI synchronized with cached Snapshot previews.

# Shade Editor 0.18.1

- Shorten production export staging paths to deterministic sibling names: `final.tif.tmp` and `final.tif.spool.tmp`, avoiding path-length failures from nested timestamp/PID suffixes.
- Keep persisted Export Queue rows visible after restart but pause every recovered Waiting/Processing job until the operator explicitly resumes it; recovered work can be resumed or cancelled per row or as a group.
- Separate Snapshot/Test export filename templating from Export All. Export Face remains a manual Save As filename, Export All keeps its editable template window, and Snapshot/Test exports use a dedicated Settings template.
- Move History from the right Tools sidebar to the left sidebar directly below Test Code.
- Color-code Export Queue rows by Waiting/Processing/Done/Failed/Cancelled state, move status to the right side, remove duplicate Done text, and add Reveal folder per row.

# Shade Editor 0.18.0

- Centralize New/Open/Project View/Recovery/Exit lifecycle transitions behind one typed Save / Discard / Cancel policy and block unsafe transitions while export work is active.
- Prevent every export path from targeting any source TIFF, including Windows path aliases/file identity, and reserve queued destinations globally before processing.
- Make Export Queue restart-safe with persistent compact immutable recipes, source fingerprints, safe cancellation/retry, project-scoped completion marks, and unambiguous `{snapshot}` / `{testcode}` naming tokens.
- Add Gray TIFF export parity while retaining the production TIFF transport invariants for RGB/CMYK + Spot, compression, predictor, metadata, DPI and atomic destination replacement.
- Complete preview color management as Source ICC -> optional Printer/RIP Soft Proof -> optional Monitor/Display ICC, with optional gamut warning and external profile identity/relink. ICC payloads are not embedded in `.shade`.
- Run TIFF inspection asynchronously with bounded parser limits and expanded production diagnostics, including byte order, SampleFormat, FillOrder, Orientation, strip/tile geometry, page count, InkSet/InkNames and RIP-risk warnings.
- Add explicit validated `.shade.bak` restoration and migrate readable legacy recovery state to the current checksummed recovery format.
- Extract lifecycle, export, color-management and TIFF-inspector controller state from the application shell and add feature-wiring/integration regression guards.
- Add production acceptance guards for lossless compression/predictor, 8/16-bit, Spot/ExtraSamples/Photoshop resources, six-channel streaming, tiled and planar decoding, BigTIFF structure/export, missing-Face relink safety, and required native Windows Shell CI.
- Keep Photoshop/RIP/real >4 GiB BigTIFF and clean-workstation Shell acceptance as explicit manual sign-off items in `docs/PRODUCTION_ACCEPTANCE_CHECKLIST.md`.

# Shade Editor 0.17.2

- Protect `New project` from destructive state loss: dirty projects and unsaved projects with Faces now require an explicit Save / Discard / Cancel decision.
- `Save and create new` waits for the asynchronous `.shade` save to complete successfully before resetting the editor; save cancellation or failure keeps the current project intact.
- Clear recovery state only after an explicit discard, successful save, or normal safe project transition.

# Shade Editor 0.17.1

- Wire the previously-added Export Queue and TIFF Inspector backends into the actual application UI and export workflows.
- Expose Printer/RIP Soft Proof in Color Management with Output-class profile selection, proof intent and persistent enable state.
- Add `File > Inspect TIFF` plus a report window with Copy report and Explorer reveal.
- Route Face, Export All and Snapshot batch exports through the non-blocking queue with Cancel/Retry and safe atomic completion semantics.
- Expose filename and folder templates in Export All, including `{project}`, `{face}`, `{snapshot}`, `{source}` and `{date}`.
- Add a cached middle-mouse SOURCE preview: original TIFF samples rendered only through the TIFF embedded ICC, bypassing edits, assigned source ICC and Printer/RIP Soft Proof. Right mouse remains BEFORE using the current preview color-management setup.

# Shade Editor 0.17.0

- Add true printer/RIP Soft Proof with a separate Output-class proof ICC, proof rendering intent and project persistence; the transform remains preview-only and never enters TIFF export.
- Make project thumbnails use the exact same color-managed / soft-proof preview pipeline as the viewport.
- Add non-blocking Export Queue with Waiting / Processing / Done / Failed / Cancelled states, safe cancel semantics and Retry.
- Extend export naming with `{project}`, `{face}`, `{snapshot}`, `{date}`, `{source}` and nested Folder Templates while keeping legacy tokens compatible.
- Add `File > Inspect TIFF` read-only production diagnostics with Copy report: TIFF/BigTIFF, dimensions, bits, photometric, planar configuration, compression, predictor, ExtraSamples, channel/Spot order, Photoshop resources, ICC, DPI and estimated uncompressed size.
- Change History labels to Photoshop-style `Tool · Channel` naming.
- Re-validate the existing Safe Project Save / Recovery path: `.tmp` + flush/sync + atomic replace, `.bak` backup, rotating recovery version/checksum and corrupt-state rejection.

# Unreleased

- Replace generic `ICC: managed` metadata text with the active ICC profile description; the profile label is clickable and opens Color Management.
- Add project-owned non-destructive Preview Profile Assignment: use embedded ICC or assign a compatible ICC/ICM only for Shade Editor preview, with Rendering Intent and optional Black Point Compensation stored in `.shade`.
- Add searchable Windows ICC/ICM catalog with Project View-style typing, Up/Down selection and Enter assignment, plus Browse and compatibility checks against the active TIFF base color model.
- Keep assigned profiles strictly out of production export; TIFF samples, embedded ICC and Photoshop resources remain unchanged.
- Consolidate active Rust sources under canonical filenames and remove obsolete versioned implementations and stale one-off documentation.
- Update README/architecture/roadmap to reflect the actual Levels → Mixer → Curve order and current ICC preview boundary.

# Shade Editor 0.16.0

- Add ICC-aware preview color management using each TIFF's embedded RGB/CMYK/Gray profile and an sRGB display destination; rendering intent is selectable in Settings.
- Keep ICC conversion strictly in the preview path. Export continues to use the original TIFF samples and preserve embedded ICC/Photoshop metadata without applying display transforms.
- Composite declared Photoshop Spot separations after the base ICC transform using their existing DisplayInfo color/solidity; known Alpha channels remain excluded from the printing composite.
- Add per-channel Levels and Curve clipping estimates from preview working-space samples, with yellow/red warning indicators in the channel list and detailed percentages in Adjustments.
- Isolate color management behind a dedicated preview module so a future printer/RIP Soft Proof profile can use a proofing transform without changing production TIFF export.
- Keep the adjustment order Levels → Mixer → Curve and keep `.shade` schema v9 unchanged.

# Shade Editor 0.14.2

- Make the Project View Preview pane vertically scrollable while preserving its resizable width and 350px thumbnail cap.
- Project rows show Face count, total source bytes, active-Face pixel dimensions, and the eight newest Snapshot names without repeating the Face filename.
- Reformat Project View project/TIFF metadata into compact two-pair grids and display Snapshots two per row.
- Force white text on the selected accent-filled adjustment channel button.
- Keep 1-9 channel shortcuts active while the Curve graph owns keyboard focus without stealing digits from text/numeric editors.
- Move Export All Reveal folder beside Browse.

# Shade Editor 0.14.1

- Keep Export All Faces compact at a ~500px default width and prevent its text fields from expanding the dialog to the application width.
- Rename the user-facing Previous Shades workspace to Project View and restore the complete v0.13.2 preview metadata while keeping v0.14 relink/remove/reveal/lazy-thumbnail behavior.
- Make the Project View preview pane horizontally resizable and cap its embedded thumbnail display at 350x350px.
- Reflow the Adjustments header and use compact adaptive Levels/Mixer/Curve/Reset tabs so the sidebar stays usable at narrow widths while retaining modified-state indicators.
- Add Ctrl+N, Ctrl+E, Ctrl+Shift+E and G shortcuts for New, Export Face, Export All and Settings.
- Keep operation progress in the right toolbar, widen it, and render operation + stage inside the progress bar without a second detail line.

# Shade Editor 0.14.0

- Export All workspace with destination field, folder TIFF warning, template naming, overwrite/skip/auto-number policies, and optional Explorer reveal after export.
- Project View promoted to a Project Browser with Remove from history, relink for missing .shade files, row-level latest Snapshot/active Face details, Explorer reveal, lazy row rendering, and a bounded thumbnail LRU cache.
- Project thumbnails now persist thumbnail_version and encoded_bytes metadata alongside width/height.
- Adjustment panel shows per-channel modified state and the total number of modified channels.

# Shade Editor 0.13.2

- Upgrade embedded project thumbnails to 512px PNG with bilinear resampling, high PNG compression, and RGB encoding when alpha is fully opaque.
- Cache a compact 72px Project View list thumbnail plus Face count and source bytes for fast history rows, including offline entries.
- Use the `.shade` filename when a project still carries the default `Untitled Shade` name, and normalize the name on the next successful Save/Quick Save.
- Project View now supports Enter to open, Up/Down navigation while Search has focus, first-result selection while searching, an Open shade folder action, and a Snapshot list in the preview pane.
- Export all omits Test Code by default; a persistent Export & storage setting can explicitly enable Test Code for all Face exports.
- Adjustments defaults to All channels, moves the enable toggle into the Editing header, swaps the All channels/channel controls, and uses larger, spaced Levels/Mixer/Curve tabs.

# Shade Editor 0.13.1

- Index Snapshot names, Snapshot IDs and effective Test Code values in the persistent Project View cache.
- Project View search can now find projects by a specific Snapshot/Test code without reopening `.shade` files during search.
- Existing Project View history is migrated once when cached Snapshot metadata is missing and the `.shade` file is available.
- Opening, Save and Quick Save refresh the cached Snapshot/Test index immediately so renamed/new tests become searchable at once.
- Search results show the matching Snapshot name/code when a Snapshot term produced the match.

# Shade Editor 0.13.0

- Add TIFF drag-and-drop directly into the Faces list.
- Add Quick Save for unsaved projects, creating a unique `.shade` beside the active/source TIFF without a Save dialog.
- Remind operators after TIFF export when the active Snapshot/Test state still has unsaved `.shade` project changes.
- Add persistent Project View history with search, sorting, embedded thumbnail/metadata preview, and missing-file retention.
- Reserve `Load all shades from system` for the next Everything Search provider integration.

# Shade Editor v0.12.1

- Moved export/storage controls into a dedicated Settings section.
- Added persistent LZW export compression control; enabled by default.
- Added Settings buttons for bundled Windows Shell install/uninstall scripts and a clear separate-package message when the shell folder is missing.
- Reworked About shortcuts into readable groups.
- Error toolbar messages now auto-expire quickly and can be dismissed.
- Widened operation/update progress bars and moved long operation details below the progress label.

# Shade Editor v0.12.0

- Preserves BigTIFF container format when exporting a BigTIFF source.
- Automatically switches to BigTIFF when the uncompressed output layout approaches the 32-bit offset ceiling of classic TIFF.
- Uses the same Levels / Curve / Channel Mixer / Test Code and Photoshop metadata preservation pipeline for classic TIFF and BigTIFF outputs.
- Adds regression coverage for a real BigTIFF identity export plus large-layout format selection without allocating a huge test image.
- `.shade` schema remains v9.
- Adds a native x64 Windows Shell extension for `.shade` files without loading the editor UI runtime inside Explorer.
- Explorer thumbnails use the PNG embedded in schema-v9 `.shade` files through `IThumbnailProvider` and Windows Imaging Component.
- A read-only `IPropertyStore` exposes Face count, active Face, physical/pixel dimensions, DPI, bit depth, color model, channel counts, source TIFF name, source bytes, and save time.
- Ships a custom Windows Property System schema plus an elevated installer for COM/property-handler registration and per-user `.shade` file association.
- Shade Editor accepts a `.shade` path as its first command-line argument so Explorer double-click opens that project.
- Native parser and COM/WIC regression tests validate schema-v9 metadata, custom properties, and embedded thumbnail decoding.

# Shade Editor v0.11.1

- Fixes physical TIFF DPI detection: XResolution/YResolution are TIFF RATIONAL values and are now decoded as numerator/denominator instead of incorrectly requesting DOUBLE tags.
- Honors TIFF 6.0's default ResolutionUnit of inches when tag 296 is omitted.
- Adds a TIFF export conformance matrix covering Uncompressed, LZW, PackBits, Deflate, horizontal Predictor, 16-bit CMYK, CMYK + Spot ExtraSamples, ICC, Photoshop resources 34377/37724, and physical DPI.
- `.shade` schema remains v9.
# Shade Editor v0.11.0

- Extended bounded-memory TIFF decoding to tiled and planar 8/16-bit RGB/CMYK layouts.
- Preview now samples arbitrary TIFF coding regions instead of requiring full-width strips.
- Export uses a random-access disk-backed spool for tiled/planar sources while keeping the proven sequential strip path for normal Photoshop TIFFs.
- Crash recovery now rotates the latest three states and automatically falls back to an older valid state if the newest recovery JSON is damaged.
- Added planar-strip and edge-tile regression fixtures for the new region decoder.

# Shade Editor 0.10.2

Production export correctness and preview/Test Code workflow fixes.

- Fixes corrupt LZW/Deflate/PackBits files produced by the strip-streaming exporter. image-tiff 0.11.x activates compression in `write_data()` but not direct `write_strip()` calls; Shade Editor now streams adjustments to a bounded disk-backed spool and memory-maps it into the library's correct compressed writer path.
- Adds a regression test starting from a valid six-channel CMYK + 2 ExtraSamples LZW source with horizontal Predictor and fully decodes the exported TIFF byte-for-byte. Horizontal Predictor is intentionally omitted on export when ExtraSamples exist because image-tiff's built-in encoder predictor stride covers only the base RGB/CMYK samples; LZW compression itself is preserved.
- Before/After right-click now uses the actual image interaction rectangle and egui clipping instead of comparing screen pointer coordinates with the ScrollArea's content-relative viewport, so it works correctly while zoomed/scrolled.
- Settings now has **Rebuild previews** next to Preview max dimension to reload all open Faces at the newly selected preview size.
- Test Code can target **All channels** (the new default) or one selected channel. All-channel mode writes the same rasterized code to every separation using each channel's correct ink polarity.
- Adds a maintained roadmap document for the remaining production work.
- `.shade` schema remains v9.

# Shade Editor 0.10.1

Production round-trip validator.

- Adds **Validate face** beside Export actions. It creates a no-adjustment TIFF through the exact production export backend, re-decodes both source and export, and writes JSON + Markdown validation reports.
- Validation checks decoded sample equality, dimensions, bit depth, color model/channel order, compression/predictor/orientation, physical DPI, ICC, Photoshop Image Resources 34377, ImageSourceData 37724, and parsed Photoshop Spot display metadata.
- The validator uses a fresh identity project and disables Test Code so the result is a true transport/interchange check independent of the current shade recipe.
- Adds regression coverage proving the validator exercises the real six-channel export backend.
- `.shade` schema remains v9. Photoshop/RIP application-level interpretation remains an external production gate even when the automated report passes.

# Shade Editor 0.10.0

Production interchange hardening and real Photoshop Spot display metadata.

- Parses Photoshop Image Resource 1077 DisplayInfo for extra channels, including Spot-vs-Alpha kind, display color, and Solidity.
- Composite preview uses the TIFF's Photoshop Spot display color/Solidity when available; known Alpha channels no longer receive a fake ink tint.
- Channel rows distinguish Spot, Alpha, and undeclared Extra channels; Spot hover information includes Solidity.
- Export preserves source lossless compression intent (LZW/Deflate/PackBits/uncompressed), horizontal predictor, orientation, DPI, ICC, Photoshop Image Resources, and ImageSourceData. Unknown/lossy source compression falls back to lossless LZW.
- TIFF exports are written to a same-directory temporary file and atomically replace the destination only after successful completion, preventing half-written production files.
- GitHub Release updates now require a companion ShadeEditor.exe.sha256 asset; downloaded executables are SHA-256 verified before staging.
- Windows CI creates the checksum in both build artifacts and tagged GitHub Releases.
- Adds production Photoshop/RIP validation guidance and regression coverage for the production-shaped DisplayInfo 1077 payload.

# Shade Editor 0.9.0

Production-workflow foundation: adjustment History, crash recovery, Before/After comparison, and a clean .shade v9 schema.

- Photoshop-style adjustment Undo/Redo shortcuts: Ctrl+Alt+Z and Ctrl+Shift+Z, plus a clickable History panel. Only adjustment edits participate; Face operations, Snapshot operations, and Palette changes are intentionally excluded.
- Adjustment drag/keyboard edits are coalesced into useful history states instead of recording every render frame.
- Dirty active Snapshots retain the existing marker but now get a stronger selected background, border, and visual emphasis.
- Hold the right mouse button over the image viewport to temporarily show the unadjusted Before view. Space remains available for viewport panning.
- Recovery autosaves dirty projects every two minutes to LOCALAPPDATA without marking the project saved. On restart the app offers Recover or Discard recovery.
- Successful manual Save clears the recovery copy; Save and exit still waits for the background save to complete.
- .shade schema v9 is intentionally a clean break. All v1-v8 migration code was removed and the loader accepts schema v9 only.
- TIFF preview/export streaming improvements are part of the same v0.9 release backend work.

# Shade Editor 0.8.1

Physical Face dimensions and safeguards against accidental state loss.

- File information now shows physical dimensions in centimeters between bit depth and pixel dimensions, calculated from the TIFF pixel size and effective DPI.
- Duplicate TIFF references remain allowed but every duplicated Face row is highlighted and marked with its duplicate count.
- Closing the application with unsaved project changes now offers Save and exit, Discard and exit, or Stay. Save and exit waits for the existing background Save job to complete successfully before closing.
- Switching away from an active Snapshot with edits that have not been written back using Update now asks whether to Stay editing or Discard changes and switch.
- `.shade` schema remains v8.

# Shade Editor 0.8.0

Optional midpoint Curve editing, deterministic Snapshot naming, and richer self-contained project files.

- Curve starts with Black/White endpoints only. Double-click near the rendered line to add the midpoint at the calculated on-line position; double-click the midpoint to remove it.
- Existing schema v7 projects migrate with their midpoint enabled so their rendering is unchanged.
- Remaining UI icon glyphs are replaced by vector painter geometry or plain ASCII text, including Snapshot checks, Channel Solo indicators, and palette swatches.
- Face information is ordered as filename, bit depth, pixel dimensions, DPI, color model, and channel count.
- Snapshot names follow the first Snapshot's trailing numeric sequence (for example XN-A1-1, XN-A1-2, XN-A1-3).
- .shade schema v8 embeds a PNG project thumbnail (max 256 px) and cached file/project metadata: face count, active face, source dimensions, bit depth, color model, channels, DPI, file size, and source modified time.
- Thumbnail generation runs in the existing background Save job.

# Shade Editor 0.7.1

Compact Adjustment layout controls.

- Adds an optional Compact Curve editor setting that hides the selected-point label, Input / Output numeric fields, and helper text while keeping all three graph points directly draggable.
- In Stacked mode, Levels, Curve, and Channel Mixer Reset actions now live on the same row as their foldout headers instead of at the bottom of each tool.
- Reset all is moved to the right side of the Editing channel header.
- Tabs mode keeps a contextual Reset action beside the tool tabs.
- All-channels Stacked mode uses the same header-level Reset behavior for Levels, Curve, and Mixer.
- No `.shade` schema change; the compact Curve choice is an application setting.

# Shade Editor 0.7.0

Direct three-point Curve editing.

- Curve is edited directly on the graph using exactly three draggable points: Black, Midpoint and White.
- Midpoint can move horizontally (Input) and vertically (Output).
- Black and White endpoints are draggable in both axes as well, while point order is constrained.
- Curve sliders were removed. The selected point exposes compact Photoshop-style Input / Output numeric fields in two columns.
- Input / Output fields use a 0-255 display scale while processing remains normalized internally.
- Broadcast and every per-channel Curve foldout use the same direct editor.
- Existing `.shade` files migrate to schema v7: old relative midpoint output becomes an absolute middle control point and midpoint input starts centered between the prior input endpoints.

# Shade Editor 0.6.1

Font-independent icon compatibility fix.

- Snapshot Export icons are now vector geometry drawn by `egui::Painter`.
- Export-success checkmarks are now vector geometry and no longer depend on a Unicode glyph.
- The same reusable vector widget is used for per-Snapshot, per-day and all-Snapshot actions.
- No icon font needs to be installed on Windows.

# Shade Editor 0.5.0

Snapshot export workflow and expanded Curve controls.

## Snapshot export

- Compact export actions are available for every Snapshot, every day group, and the whole Snapshots panel.
- Snapshot export always targets the active Face and reuses the exact same TIFF/Test Code backend as Export face.
- A single Snapshot uses a Save dialog; day/all exports use a destination folder and export every selected Snapshot there.
- Successful exports are marked with a check. Clicking the check opens the latest export folder.
- Export history is stored per Snapshot + Face in `.shade` schema v5, but never locks or disables re-export.

## Curve

- Curve now has Input black and Input white endpoints in addition to output endpoints and relative midpoint.
- In All channels, Broadcast remains available and copies its Curve to every channel.
- Each channel also has its own collapsed full Curve panel for independent refinement after Broadcast.
- With four channels the Curve section therefore shows one Broadcast Curve plus four channel foldouts.

## Adjustment layout

- In Stacked mode, Levels, Curve and Channel Mixer can each be collapsed/expanded independently.
