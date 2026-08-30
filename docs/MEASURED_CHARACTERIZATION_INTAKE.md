# Measured Characterization Intake

Tracks: #421, #478, #205, #106, #115, #191

Shade Editor can build a typed measured-characterization package from a laboratory CSV/TSV table inside the existing **Production Color Conversion → measured characterization package builder** surface. This is qualification tooling inside the normal Shade Editor executable; it does not create or distribute a second application.

## Start with an acquisition template

If a laboratory measurement table does not exist yet, enter the exact production channel names/order, measured per-channel maximum coverages and measured total-ink limit, select the intended delimiter/coverage unit, then choose **Export acquisition template...**.

The generated table is deterministic for identical topology/limits and contains:

- one zero-ink substrate baseline;
- four generic single-ink ramp points per production channel;
- two bounded pairwise screening mixes for each channel pair;
- four balanced samples across the declared total-ink envelope.

Every generated coverage vector is bounded by the declared per-channel maximums and total-ink limit, duplicate vectors are removed deterministically, and the plan remains bounded to at most 185 patches for 12 channels. The exported header is the exact existing intake shape: `<authoritative channels...>,L,a,b` for CSV or the equivalent TSV form.

The `L`, `a`, and `b` cells are intentionally empty. Shade Editor does not fabricate PCS measurements. Fill those cells only from the real measurement workflow, then load the completed table back into the builder.

This generic acquisition template is a collection/screening aid, not a claim of a representative production corpus. #205 may still require target-specific density, mixed-ink regions, neutral/near-neutral paths, warm/cool grays, saturated/boundary regions and samples around relevant production constraints. #106 and #115 likewise retain their measured neutral/gradient/continuity requirements.

## What the builder accepts

The table header must contain the production ink/channel columns in the exact authoritative order, followed by `L`, `a`, `b` measured values. The importer never guesses or reorders channel names.

Choose the coverage unit explicitly:

- `Normalized`: channel coverage is `0..1`.
- `Percent`: channel coverage is `0..100` and is converted deterministically to normalized coverage by the typed intake core.

The UI supports 1–12 declared production channels and starts with four rows for convenience. Remove or add channel rows so the declared topology exactly matches the measurement table and the physical/RIP production order.

Enter the measured per-channel maximum coverage, measured total-ink limit, output bit depth, dataset revision, production context, and measurement metadata. For the current Custom Optimizer production qualification path, measured PCS metadata is expected to be D50 / CIE 2°; incompatible metadata is surfaced as a qualification warning rather than silently rewritten.

## Build and save

1. Open the measured characterization package builder from the existing Production Color Conversion/preset surface.
2. Optionally export an acquisition template from the exact channel topology/limits if measurement collection has not started yet.
3. After real measurement, load the completed CSV/TSV table.
4. Select delimiter and coverage unit explicitly.
5. Enter the exact channel names/order and measured limits.
6. Enter production context: machine, RIP/version, linearization/calibration identity, substrate and optional glaze/body/product family.
7. Enter measurement context: instrument, illuminant, observer, measurement condition and optional operator/lab.
8. Leave `Experimental` selected unless the package metadata is intentionally being declared otherwise. Changing the dropdown only changes declared metadata; it does not grant production approval.
9. Choose **Validate & build package**.
10. Review validation errors, content ID and qualification warnings.
11. Save JSON only after validation succeeds.

The UI delegates acquisition-plan generation to the dedicated characterization-acquisition core and delegates measurement parsing, structural validation and canonical `sha256:<hex>` content identity to the existing Rust characterization authority used by production code. Editing any package input after a successful build invalidates the built result and requires validation again before saving.

## What a successful package means

A successful build means only that the measurement table was converted into a schema-valid, content-addressed `CharacterizationPackage` that round-trips through the production loader.

It does **not** prove that the measurements are representative enough for production, that forward-model error is acceptable, that neutral/gradient continuity passes, that thresholds are approved, or that Custom Optimizer is production-authorized.

The generated package is an input to the evidence workflow in **#205**. Production acceptance then remains tied to the exact reviewed calibration/evidence identity and the dependent gates in #106, #115 and #191. A package declaring `ProductionValidated` is still not an approval bypass; the production authorization path remains fail-closed until the tracked evidence gates pass.

External TIFF interoperability is separate: representative generated CMYK/N-channel outputs still require Adobe Photoshop and the actual ceramic RIP acceptance tracked by #96.
