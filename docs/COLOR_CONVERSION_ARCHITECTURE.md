# Color Conversion & N-Channel Separation Architecture

Status: **Accepted foundation / implementation in progress**

Tracks: #84, #86, #87, #88, #90, #95, #98, #99

## Context

Shade Editor currently edits production TIFF separations non-destructively: source Face files are immutable, `.shade` stores adjustment recipes, and export writes new TIFF data. Production color conversion introduces a different operation: the output color space and channel topology may be different from the input. An RGB source can become CMYK or a ceramic N-channel separation, and a CMYK source can become a different N-channel target.

Treating that operation as an ordinary export or as an in-place project color-mode change would mix incompatible adjustment domains and make later reproduction ambiguous.

## Decision 1 — Source and Production projects are distinct

Color conversion is a **derivation event** between two projects:

```text
Source/design .shade
    + immutable source Face
    + saved source Snapshot/adjustments
    + explicit source ICC interpretation
            |
            | Convert Color
            v
Production TIFF/BigTIFF
    + Production .shade
    + production-ink adjustments/Snapshots
```

The Source project remains editable after conversion. The Production project remains a stable result derived from one exact saved Source state.

The source `.shade` is never mutated into the Production `.shade`.

## Decision 2 — Existing projects remain backward-safe Standalone projects

Existing `.shade` files predate this workflow and can represent many real production scenarios. They must not be guessed to be `source` or `production` projects.

The serialized project role has three semantic states:

- `standalone` — backward-compatible/default behavior for existing projects;
- `source` — design/source project participating in conversion lineage;
- `production` — project containing converted production separations.

Missing role data deserializes as `standalone`.

Schema v9 now stores defaultable role, linked-project and per-Face production-provenance fields. This does **not** change `SHADE_SCHEMA_VERSION`: explicit backward-compatibility tests prove that files without the fields retain standalone behavior.

## Decision 3 — Conversion requires a saved Source state

`Convert Color...` cannot operate from transient dirty UI state.

Before conversion:

1. The Source project must have a saved `.shade` path.
2. Unsaved source adjustments must be saved/committed first.
3. The conversion job captures an immutable source Face + Snapshot/recipe reference.
4. Save failure cancels the conversion transition.

No hidden adjusted-RGB intermediate file is required. The conversion backend renders the saved source recipe and passes those samples directly into the production conversion/separation engine.

## Decision 4 — Source files are immutable and conversion never overwrites them

RGB/CMYK TIFF, PNG and JPEG source files are immutable design inputs.

Conversion always writes a new production TIFF/BigTIFF destination. The original source path must be rejected as a destination. Re-conversion defaults to a new versioned output or an explicit transactional replacement flow.

PNG/JPEG are source/design formats only. Production output remains TIFF/BigTIFF.

## Decision 5 — Preview Color Management and Production Conversion remain separate

`color_management.rs` is display infrastructure:

```text
adjusted source/base channels
    -> embedded/assigned preview ICC
    -> optional proof transform
    -> monitor/sRGB display
```

It must remain incapable of changing production bytes.

Production conversion is separate infrastructure:

```text
saved adjusted source samples
    -> source ICC interpretation
    -> ICC / DeviceLink / Custom N-ink separation
    -> new target channel planes
    -> production TIFF metadata/writer
```

Monitor ICC, gamut-warning display settings and soft-proof-only state are forbidden inputs to deterministic conversion recipes.

### Production Source ICC assignment

The production source interpretation is resolved per Source Face with this precedence:

1. an explicit `FaceRef.production_source_profile` assignment, when present;
2. otherwise the valid embedded ICC carried by the source image;
3. otherwise production conversion is blocked until an assignment is made.

An explicit assignment stores only the external profile path plus description/SHA-256 identity. It is an interpretation override, not a pixel conversion, and ICC payload bytes are never embedded in `.shade`. Reopening preflight verifies that the file still exists, its bytes match the stored identity, and its declared color space matches the source Face. Missing, moved, replaced, corrupt or wrong-space profiles block conversion with an actionable assignment/relink error.

Production assignment is deliberately distinct from `PreviewColorSettings.assigned_profile_path`. Changing preview ICC, Soft Proof or monitor ICC cannot satisfy production preflight. Assigning, reassigning or clearing a production Source ICC marks the Source project dirty, so the saved-source gate captures the exact interpretation before conversion.

### Production Target Setup

After Source preflight is ready, Target Setup binds the conversion to one exact external Output ICC or DeviceLink plus its SHA-256 identity. Standard ICC mode accepts only Output/printer profiles. DeviceLink mode accepts only Link-class profiles and additionally verifies that the link input space matches the active RGB/CMYK Source Face.

The currently executable production topology contract is CMYK or 5C–12C. Four-channel output must be the standard ICC CMYK space; an arbitrary generic four-color signature is not silently treated as CMYK. For CMYK, canonical Cyan/Magenta/Yellow/Black order is authoritative. For N-channel profiles, the ICC Colorant Table/Colorant Table Out order is authoritative when complete. If the profile lacks complete colorant names, Target Setup generates placeholders but blocks recipe readiness until the operator enters and explicitly confirms the real RIP/ink order.

Target Setup also captures 8/16-bit output precision, rendering intent and BPC only when the selected engine supports those controls, and a TIFF-only destination. The current UI resolves collisions to a deterministic versioned filename by default or records explicit transactional-replacement intent. It never writes pixels itself. The conversion worker must re-open and hash-verify the target profile immediately before execution and must commit through the transactional output boundary.

## Decision 6 — Three explicit conversion engine modes

Every production conversion records exactly one engine mode:

### ICC

Standard source-to-output ICC transform. Separation behavior is whatever the output profile defines. Expert controls must not pretend to alter Black/ink construction if the profile does not expose that capability.

### DeviceLink

A precomputed device-to-device transform with a validated separation strategy. The strategy is normally fixed by the DeviceLink.

### Custom Optimizer

Shade Editor performs constrained N-ink separation using a measured/validated target characterization. This is the path that can support operator-controlled ink preference, Black-focused neutral construction and ceramic-specific optimization.

The UI must expose capabilities honestly for the selected mode.

## Decision 7 — Ink priority is an optimization preference, not a channel multiplier

A requested strategy such as `Black-focused` means:

> Among separations that reproduce the target color within configured tolerances, prefer solutions that construct neutral/gray colors with more Black and less competing chromatic ink.

It must **not** mean:

```text
K *= 2
Blue *= 0.5
```

after conversion.

Post-transform multipliers can move color unpredictably and violate characterization/ink limits.

The expert separation strategy can eventually contain:

- Black generation strength;
- Black start;
- Black maximum;
- neutral C* threshold;
- per-ink preference/penalty weights;
- total and per-channel laydown limits;
- maximum accepted color-error constraint;
- target-specific process constraints.

All values are part of the versioned conversion recipe.

## Decision 8 — Target topology is authoritative and explicit

A conversion target is more than a list of display colors. It identifies a characterized production configuration and declares:

- target/profile identity;
- engine capability/mode;
- authoritative channel count;
- channel names and order;
- bit depth;
- optional display/Solidity metadata;
- optional per-channel and total laydown limits;
- optional custom-optimizer characterization identity.

Palette aliases remain presentation-only and cannot define production topology.

## Decision 9 — Production provenance is per converted Face

Every converted Production Face must be able to answer:

- Which Source project produced me?
- Which Source Face?
- Which saved Snapshot/recipe?
- Which source ICC interpretation?
- Which target ICC/DeviceLink/characterization?
- Which conversion recipe/version?
- Which output topology/bit depth?
- Which output file/hash?

Source edits after conversion never mutate existing Production Faces silently. They only make the lineage stale and allow an explicit re-conversion.

## Decision 10 — Later Faces can join an existing Production project only through compatibility validation

A newly designed Source Face may be converted later and added to an existing linked Production project only when its result matches the established production target contract.

Compatibility must validate at least:

- target/profile identity;
- engine/recipe compatibility policy;
- channel count;
- channel names;
- channel order;
- bit depth;
- polarity/representation requirements.

Incompatible conversion produces a new Production project/target rather than contaminating the existing one.

## Initial code boundary

`src/color_conversion.rs` owns the serialization-safe domain contracts introduced for this subsystem:

- project-role vocabulary;
- engine-mode vocabulary;
- target channel/topology definition;
- separation strategy definition;
- versioned conversion recipe;
- source reference and production provenance structures;
- validation of profile/characterization prerequisites and ink-strategy references.

It does **not** itself perform pixel conversion. Transform execution and output transport remain separate implementation stages under #87, #91 and #95.

## Conversion TIFF writer

`src/conversion_tiff.rs` provides the output boundary for 8/16-bit CMYK and 5C–12C conversion samples. It requests one strip at a time from the conversion worker, selects classic TIFF or BigTIFF from the estimated raw size (or an explicit override), embeds the exact target ICC and writes `InkSet`, NUL-separated `InkNames`, `NumberOfInks`, DPI, orientation and extra-sample topology.

Output is first written beside the destination, then re-opened to verify dimensions, precision, sample count, target ICC and ink tags. Only a verified and synced staged file reaches the atomic replacement boundary; render or verification failure leaves an existing production file untouched.

The bounded callback path currently writes uncompressed strips because the encoder's compressed path requires a complete image slice. LZW therefore requires a later local mmap/spool stage. Photoshop Image Resources/DisplayInfo generation, external Photoshop/RIP round trips and representative >4 GiB BigTIFF validation remain mandatory before claiming production interchange compatibility.

## Worker transaction boundary

`ConversionJobCapture` freezes the saved source adjustment recipe, source project/Face/Snapshot identity, full source-file SHA-256, conversion recipe plus its SHA-256, output destination and Production-project destination. A queued worker must serialize this capture and must not reread mutable UI settings when it starts.

The transaction reports distinct decode, source-adjustment, conversion, metadata, staged-write, validation, TIFF-commit and Production-project-save phases. Cancellation is honored until the atomic TIFF commit point. Once a validated TIFF is committed, the small Production-project save boundary is completed even if cancellation arrives, because rolling back or deleting a durable production file would violate recovery guarantees.

If project construction or saving fails after TIFF commit, the transaction returns a structured recovery result containing the committed output identity and, when construction succeeded, the clean Production project payload. It never silently deletes the committed TIFF.

## Persistent conversion queue and reciprocal link boundary

Valid Standard Output ICC Target Setup can now capture and enqueue production work. Capture hashes both the saved Source `.shade` and immutable source TIFF in a foreground-independent worker, freezes the source adjustment/profile/target recipe plus destinations, and persists it in `conversion-queue.json` before raster execution begins.

Queue entries have explicit Waiting, Processing, Done, Failed, Cancelled and Needs Recovery states. A process restart converts an interrupted Processing entry back to Waiting and requires explicit operator resume; it never silently repeats a production write. Progress follows transaction phases. Active cancellation signals the transaction token and is honored until atomic TIFF commit. A failure after commit retains the committed output identity and optional clean Production-project payload for diagnosis/retry.

The captured destination policy survives queue delay. Safe versioned work requires the TIFF and Production-project destinations to remain absent. TIFF publication uses an atomic same-volume no-replace hard-link boundary, so a destination created after capture is preserved even at the final commit race. Only an explicitly captured transactional-replacement policy uses the replace-existing commit path.

After TIFF and Production-project commit, the worker re-hashes the Source `.shade` before writing its reciprocal Production link. If the saved Source changed since capture, the worker preserves both committed production artifacts and reports Needs Recovery instead of overwriting newer Source bytes. The open in-memory Source project mirrors a successful disk link while retaining any newer unsaved edits.

Export and conversion workers are independently persistent but are not started concurrently, avoiding competing full-resolution disk/CPU pressure.

## Standard ICC and DeviceLink raster backend

`FilesystemIccConversionBackend` executes the transaction for standard Output ICC and direct DeviceLink recipes. Immediately before work begins it reopens and hashes the complete source file, the embedded or explicitly assigned Source ICC, and the target ICC/DeviceLink. A changed input fails the captured job instead of silently changing its color meaning.

The backend currently accepts streamable TIFF sources containing exactly three RGB or four CMYK samples. It decodes one strip/tile region at a time, applies the frozen saved Source adjustment recipe, and writes row-major 16-bit samples into a uniquely created local mmap spool. The read-only spool lets the atomic output writer request bounded row strips even when the input is tiled, without retaining a second full-resolution image in RAM.

For Standard ICC, LittleCMS combines the Source ICC and Output ICC with the captured intent/BPC policy. For DeviceLink, LittleCMS executes the single LinkClass profile directly: Source ICC remains a verified provenance/interpretation input but is not inserted into the already-encoded link, and intent/BPC remain fixed by that link. Each path converts requested strips to CMYK or a typed 5C-12C target.

The writer emits 8-bit or 16-bit output, preserves DPI/orientation and authoritative target channel order, validates and atomically commits the TIFF, and returns its full SHA-256 before the transaction saves the clean Production project. Standard ICC output embeds and verifies its revalidated Output ICC. DeviceLink output deliberately omits the ICC tag because a LinkClass transform is not an output-device characterization; the exact DeviceLink identity remains in serialized conversion provenance.

This backend deliberately rejects Custom Optimizer recipes, RGB/CMYK sources with Spot or extra samples, and non-streamable TIFF input. Those paths require dedicated semantics rather than an implicit fallback. Deterministic real RGB→CMYK and N-channel Output-ICC fixtures, LZW, Photoshop-specific spot metadata and external Photoshop/RIP validation remain release gates.

## Serialization policy

`CONVERSION_RECIPE_SCHEMA_VERSION` is independent from the `.shade` schema version. Conversion recipes need their own semantic version boundary because optimizer/profile strategy semantics can change independently from the project container.

`ShadeProject` stores role, reciprocal project links and production provenance using Serde defaults. A fresh Production project is built from target channel definitions and intentionally starts with clean target-domain adjustments/Snapshots; source-domain adjustments are never copied into it. A future `.shade` schema bump remains required if semantic compatibility can no longer be preserved by defaults.

Per-Face production Source ICC assignment was added to schema-v9 projects as a backward-safe optional `FaceRef` field with a Serde default. Legacy Face records deserialize with no assignment and continue to use embedded ICC preflight.

## Validation policy

No production compatibility claim is accepted solely because Shade Editor can write/read its own output. Representative converted files must round-trip through Photoshop and the target production RIP, with measured/approved fixtures where required.

Custom N-ink optimization additionally requires characterized target data; channel display colors alone are never a device model.
