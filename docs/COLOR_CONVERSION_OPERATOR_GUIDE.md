# Color Conversion Operator Guide

Tracks: #84, #88, #93, #96, #101, #103, #113, #205

This guide describes the production Color Conversion workflow as it exists in Shade Editor. It distinguishes display/preview color management from production conversion and states where production claims are still evidence-gated.

## 1. Source project vs Production project

Color Conversion never changes a Source/design project into a Production project in place.

A Source project contains the original RGB/CMYK design Faces and their non-destructive Shade Editor adjustments. A Production project contains TIFF separations derived from one exact saved Source state and one exact production conversion recipe.

The original Source Face file is never overwritten. Production conversion writes a new TIFF/BigTIFF and records reciprocal Source/Production lineage.

## 2. Why the Source project must be saved

Final production conversion is not allowed to consume transient UI state.

Before queueing a conversion:

1. Save the Source `.shade` project.
2. Resolve any blocking Source ICC or transparency preflight finding.
3. Select and verify the production target.
4. Review Candidate Preview and diagnostics.
5. Queue Current Face, Selected Faces, or All Faces.

The queued job freezes the saved Source project identity, Source Face identity, source-file hash, source raster facts, Source ICC interpretation, conversion recipe, destination policy and target identities. A worker does not reread mutable UI settings later.

If the Source project changes after capture, Shade Editor does not silently reinterpret the queued job.

## 3. Preview color management is not production conversion

Preview color management affects what is shown on the monitor. It may use an assigned/embedded preview ICC, proof transform or monitor profile. Those settings do not define production output bytes.

Production conversion is a separate path:

```text
saved adjusted Source samples
    -> production Source ICC interpretation
    -> Output ICC / DeviceLink / Custom Optimizer
    -> target channel planes
    -> production TIFF/BigTIFF
```

Do not use monitor appearance as proof that a production conversion recipe is correct. Candidate Preview is generated from the production conversion recipe, while monitor ICC remains display-only.

## 4. Production Source ICC

Each Source Face must have a deterministic production color interpretation.

Shade Editor resolves it in this order:

1. an explicitly assigned production Source ICC when one is configured;
2. otherwise a valid embedded ICC in the Source image;
3. otherwise, for supported RGB Sources, the deterministic built-in sRGB production fallback;
4. if no valid interpretation/fallback exists for that Source model, production preflight blocks conversion until the missing interpretation is resolved.

The RGB fallback is explicit in preflight/audit state; it is not an invisible monitor-profile assumption.

An explicitly assigned Source ICC is stored by path plus SHA-256 identity. The profile is reverified before conversion. If an assigned profile is missing, replaced, corrupt or declares the wrong source color space, conversion blocks instead of silently replacing that explicit assignment with another interpretation.

Changing or clearing an explicit production Source ICC assignment marks the Source project dirty. Save again before final conversion.

## 5. Production target engines

N-channel is an output topology, not a separate conversion engine mode. The active production engines are Output ICC, DeviceLink and the measured Custom Optimizer path. A verified ICC or DeviceLink can therefore produce CMYK or a supported multi-ink target topology without introducing a fourth `N-channel` engine authority.

### Output ICC (CMYK / N-channel)

Use an Output/printer ICC when the target profile itself defines the separation behavior.

When a verified Output ICC exposes a supported 5–12 channel output space, Shade Editor uses the typed N-channel ICC transform and preserves the exact channel topology in the resulting TIFF/BigTIFF. The same ICC engine also handles normal CMYK targets.

Applicable operator controls include target profile, output precision, rendering intent and Black Point Compensation where supported.

Black-generation or ink-priority controls are **not** applied on top of an Output ICC transform. If the profile does not expose that behavior, Shade Editor does not pretend that a strategy preset changes it.

### DeviceLink

Use a DeviceLink when the device-to-device transform and separation strategy are already encoded in the Link-class profile.

A verified DeviceLink may likewise target a supported multi-channel output topology; the link itself owns the device-to-device separation semantics and exact output channel topology.

Shade Editor verifies the DeviceLink identity and input/output topology. The exact DeviceLink identity is retained in provenance. A DeviceLink is not treated as an Output ICC characterization and is not embedded in the output TIFF as if it were one.

Rendering intent/BPC are not presented as independent Shade Editor separation controls when their meaning is fixed by the DeviceLink.

### Custom Optimizer

Custom Optimizer is the strategy-capable N-ink path intended for measured ceramic separation. It can support Black-focused neutral construction, per-ink preferences, hard channel/total-ink limits and continuity-aware optimization.

Production use remains fail-closed until the exact measured ceramic calibration/evidence is approved under #205 and the exact production authorization path under #191 passes. Until then, operator controls that would imply production-authorized Custom Optimizer behavior must remain unavailable.

## 6. Presets and engine semantics

Conversion presets are target-bound recipe definitions. User presets are persisted under:

```text
%LOCALAPPDATA%\ShadeEditor\conversion-presets.json
```

Built-in presets are reconstructed from the running binary and are never trusted from disk. The `builtin:` preset-ID namespace is binary-owned; persisted/imported user definitions are rejected if they try to claim it.

Shade Editor keeps two concepts separate:

- **compatibility**: does the preset match the selected target/profile/topology/bit depth and related identities?
- **application availability**: does the selected production engine actually consume the separation strategy represented by that preset?

A compatible preset is not automatically applicable.

For Output ICC and DeviceLink, separation-strategy Apply/Save-current is intentionally unavailable when it would only change the recipe/provenance hash while leaving pixels unchanged.

Built-in presets are immutable. User presets may be duplicated, renamed, deleted, imported and exported through the guarded preset lifecycle. Portable preset JSON contains definitions/identity hashes, not ICC/DeviceLink payload bytes or absolute profile paths.

## 7. Black-focused and ink-priority strategies

Black-focused does not mean multiplying the Black channel after conversion.

The intended optimizer semantics are:

> Among candidate separations that stay within the approved color-error and ink constraints, prefer a solution that constructs eligible neutral colors with more Black and less competing chromatic ink.

Likewise, suppressing an unstable or expensive ink is an optimization preference under color/ink constraints, not a post-transform channel multiplier.

These controls are only meaningful for an engine that actually consumes the persisted strategy. Shade Editor must keep them unavailable otherwise.

## 8. Ink limits

Per-channel and total-ink limits are hard production constraints when they are part of the characterized target/recipe used by the active engine.

Candidate diagnostics can report actual converted channel usage and total-ink statistics from the exact cached candidate samples. The committed audit report can retain actual output usage from the committed TIFF.

Do not invent physical ink volume, mass or financial cost from normalized channel coverage. Calibrated physical cost remains a separate future feature.

## 9. Candidate Preview and diagnostics

Candidate Preview and final conversion use the same immutable production recipe path.

Current diagnostics include per-channel coverage statistics, non-zero usage, configured channel-limit hits, total-ink statistics and total-limit hits from the exact converted candidate samples.

Measured ΔE distributions and neutral Black-vs-chromatic classification require approved measured PCS/characterization evidence. Until that evidence exists, Shade Editor must state that measured quality evidence is unavailable rather than synthesize ΔE or infer neutrality from output inks alone.

## 10. Current / Selected / All Faces

The same target recipe can be queued for:

- Current Face;
- Selected Faces;
- All Faces.

Each Face keeps its own captured Source ICC interpretation and Source identity. A batch may therefore contain Faces with different valid Source ICCs while sharing one production target.

Output naming and route ownership are deterministic. Converting a Face individually or as part of a batch must not produce a different canonical route merely because the batch scope changed.

## 11. Adding a Face later

A Source Face added months later can join an existing linked Production project only if the established production contract still matches.

Shade Editor verifies the saved route, target/profile identity, channel count/order/names, output bit depth and recipe compatibility. It does not silently mutate an existing production route to accommodate new settings.

If current conversion settings differ from the saved route, restore the saved route settings or create a new Production route.

## 12. Re-conversion and replacement

Source edits never silently overwrite an existing converted Production Face.

Re-conversion is explicit. Same-route replacement uses a transactional replacement policy and requires the route/output ownership checks to pass. When Production-side work would be discarded, the operator must explicitly authorize that destructive replacement path.

If a transaction fails after the TIFF has already committed, Shade Editor preserves the committed production artifact and records recovery state rather than deleting a valid production file silently.

## 13. Provenance and audit report

Each committed production conversion records enough immutable evidence to answer:

- which Source project and Face produced the output;
- exact Source raster format/model/bit depth/channel count;
- Source ICC identity;
- target engine/profile/DeviceLink/optimizer identities;
- target topology and output bit depth;
- exact recipe identity;
- preflight findings captured for the job;
- committed output path/hash;
- actual output channel/total-ink usage where available;
- application version and completion time;
- Custom Optimizer calibration/LUT/validation identities when applicable.

The audit viewer is read-only. It consumes persisted evidence and does not rerun conversion analytics. Portable export redacts/relativizes machine-specific absolute paths while retaining content identities.

## 14. Practical ceramic examples

### Gray artwork defaults to too much Blue/Brown/Beige

Do not compensate by multiplying Black after an ICC/DeviceLink conversion.

For Output ICC or DeviceLink, use a profile/link whose separation already has the required neutral construction. For Custom Optimizer, a Black-focused strategy may be used only after the measured target is production-authorized. Acceptance must prove that Black increases and competing chromatic ink decreases without violating approved color-error or laydown limits.

### Avoid an unstable or expensive ink

Do not set the output channel to zero after conversion.

A strategy-capable optimizer may penalize that ink while solving for an acceptable alternative separation. Measured acceptance must show the resulting color error remains inside the approved threshold and hard per-channel/total-ink limits remain satisfied.

### Low-ink preset

A low-ink strategy should choose lower-laydown solutions among colorimetrically acceptable candidates. It must not trade arbitrary color accuracy for less ink without exposing that tradeoff.

### Add a new RGB Face later

Open the original Source project, add/save the new Face, restore the existing saved production route, confirm that the Face's production Source ICC interpretation/preflight is ready, and convert the Face into the compatible linked Production project. If the target/profile/recipe contract has changed, create a new production route instead.

## 15. Photoshop and ceramic RIP acceptance

Internal tests validate Shade Editor's deterministic writer/reader and metadata contracts, but they do not substitute for external production interoperability.

Before claiming a target configuration as externally production-accepted, validate representative outputs in Adobe Photoshop and the actual ceramic RIP as tracked by #96.

Check at minimum:

- exact channel count/order/names;
- CMYK vs true non-CMYK Separated interpretation;
- 8/16-bit interpretation as applicable;
- ICC/DeviceLink semantics;
- no stale RGB ICC on production output;
- polarity/coverage interpretation;
- no dropped, inverted, merged or alpha-misread channels;
- dimensions and DPI;
- save/reopen behavior.

Retain fixture hashes plus Photoshop/RIP versions and acceptance notes. Internal green CI alone is not external RIP acceptance.

## 16. Production-claim rule

A Color Conversion feature can be implementation-complete while a particular target configuration is still not production-qualified.

Production claims must be tied to the exact validated target/profile/DeviceLink or approved measured Custom Optimizer evidence. Do not generalize one accepted fixture into a claim that every ICC, DeviceLink, N-channel topology or RIP is automatically compatible.
