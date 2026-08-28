# Measured Characterization Intake

Tracks: #421, #205, #106, #115, #191

Shade Editor can build a typed measured-characterization package from a laboratory CSV/TSV table inside the existing **Production Color Conversion → measured characterization package builder** surface. This is qualification tooling inside the normal Shade Editor executable; it does not create or distribute a second application.

## What the builder accepts

The table header must contain the production ink/channel columns in the exact authoritative order, followed by `L`, `a`, `b` measured values. The importer never guesses or reorders channel names.

Choose the coverage unit explicitly:

- `Normalized`: channel coverage is `0..1`.
- `Percent`: channel coverage is `0..100` and is converted deterministically to normalized coverage by the typed intake core.

The UI supports 1–12 declared production channels and starts with four rows for convenience. Remove or add channel rows so the declared topology exactly matches the measurement table and the physical/RIP production order.

Enter the measured per-channel maximum coverage, measured total-ink limit, output bit depth, dataset revision, production context, and measurement metadata. For the current Custom Optimizer production qualification path, measured PCS metadata is expected to be D50 / CIE 2°; incompatible metadata is surfaced as a qualification warning rather than silently rewritten.

## Build and save

1. Open the measured characterization package builder from the existing Production Color Conversion/preset surface.
2. Load the CSV/TSV table.
3. Select delimiter and coverage unit explicitly.
4. Enter the exact channel names/order and measured limits.
5. Enter production context: machine, RIP/version, linearization/calibration identity, substrate and optional glaze/body/product family.
6. Enter measurement context: instrument, illuminant, observer, measurement condition and optional operator/lab.
7. Leave `Experimental` selected unless the package metadata is intentionally being declared otherwise. Changing the dropdown only changes declared metadata; it does not grant production approval.
8. Choose **Validate & build package**.
9. Review validation errors, content ID and qualification warnings.
10. Save JSON only after validation succeeds.

The UI delegates parsing, structural validation and canonical `sha256:<hex>` content identity to the same Rust characterization authority used by production code. Editing any input after a successful build invalidates the built result and requires validation again before saving.

## What a successful package means

A successful build means only that the measurement table was converted into a schema-valid, content-addressed `CharacterizationPackage` that round-trips through the production loader.

It does **not** prove that the measurements are representative enough for production, that forward-model error is acceptable, that neutral/gradient continuity passes, that thresholds are approved, or that Custom Optimizer is production-authorized.

The generated package is an input to the evidence workflow in **#205**. Production acceptance then remains tied to the exact reviewed calibration/evidence identity and the dependent gates in #106, #115 and #191. A package declaring `ProductionValidated` is still not an approval bypass; the production authorization path remains fail-closed until the tracked evidence gates pass.

External TIFF interoperability is separate: representative generated CMYK/N-channel outputs still require Adobe Photoshop and the actual ceramic RIP acceptance tracked by #96.
