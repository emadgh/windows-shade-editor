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

The future serialized project role therefore has three semantic states:

- `standalone` — backward-compatible/default behavior for existing projects;
- `source` — design/source project participating in conversion lineage;
- `production` — project containing converted production separations.

Missing role data must deserialize as `standalone` when the role is integrated into the `.shade` schema.

This foundation does **not** change `SHADE_SCHEMA_VERSION` yet. Role/provenance serialization will land only with explicit migration/backward-compatibility tests.

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

It does **not** yet perform pixel conversion or write TIFFs. Those capabilities remain separate implementation stages under #87, #91 and #95.

## Serialization policy

`CONVERSION_RECIPE_SCHEMA_VERSION` is independent from the `.shade` schema version. Conversion recipes need their own semantic version boundary because optimizer/profile strategy semantics can change independently from the project container.

When project role/provenance fields are eventually added to `ShadeProject`, they should be defaultable where backward-safe. A `.shade` schema bump is required if semantic compatibility cannot be preserved by defaults.

## Validation policy

No production compatibility claim is accepted solely because Shade Editor can write/read its own output. Representative converted files must round-trip through Photoshop and the target production RIP, with measured/approved fixtures where required.

Custom N-ink optimization additionally requires characterized target data; channel display colors alone are never a device model.
