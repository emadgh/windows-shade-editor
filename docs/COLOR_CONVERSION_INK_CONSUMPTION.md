# Color Conversion ink-consumption metrics

Issue: #107
Related: #105, #88, #93, #95

## Scope

Shade Editor reports deterministic **relative ink coverage** from converted separation samples. These values are useful for comparing two conversion recipes over the same Source state and target topology.

Relative coverage is intentionally not presented as physical printer consumption. Shade Editor does not currently claim millilitres, grams, drops, currency cost, or actual RIP consumption unless a future version has an explicit, versioned calibration contract for those units.

## Canonical relative unit

For output channel `c`, the canonical integrated relative ink value is:

`integrated_coverage(c) = sum(normalized_coverage(pixel, c))`

where each normalized coverage sample is in `0..=1` after TIFF/working-sample polarity normalization.

Properties:

- deterministic for the same converted raster;
- dimensionless relative coverage units;
- additive across pixels;
- additive across target channels;
- independent of display color, preview tint and UI rendering;
- not corrected into printer drop volume, mass or monetary cost.

`ConversionUsageReport::channels[].integrated_coverage` is the canonical per-channel value. Total relative consumption is the sum of all target-channel integrated values.

## Candidate A/B comparison

`CandidateComparison` compares already-rendered real-engine Candidates generated from the exact same Source-state identity and identical target topology.

For each channel, `integrated_coverage` is Candidate B minus Candidate A. `integrated_total_coverage` is the exact sum of those per-channel B-minus-A values.

A positive value means Candidate B uses more normalized integrated coverage than A. A negative value means B uses less. These are suitable for relative comparisons such as Balanced versus another authorized separation recipe; they are not evidence that the printer will consume the same percentage more or less physical ink.

The comparison path does not retain a second image-sized raster or run UI-only consumption math. It consumes the existing `ConversionUsageReport` produced from converted Candidate samples.

## Full converted TIFF analysis

`analyze_conversion_tiff` computes the same relative coverage semantics from the generated conversion TIFF in bounded memory. It first requires the TIFF sample count and exact channel order/names to match the immutable conversion recipe.

This makes the metric suitable for deterministic output/audit analysis without treating preview RGB values or channel display colors as production data.

## Why relative coverage is not physical consumption

Actual machine consumption may differ materially from image coverage because the RIP/printer can apply factors outside the separation raster, including:

- machine and head configuration;
- RIP version and linearization/calibration;
- ink identity and physical properties;
- waveform and drop-size policy;
- pass strategy and firing mode;
- resolution and screening/halftoning behavior;
- printer-side limits, compensation or remapping;
- maintenance/purge behavior, which is not image consumption at all.

Therefore an uncalibrated coverage value must never be labeled or exported as `ml`, `g`, `kg`, physical drops, or monetary cost.

## Physical volume / mass / cost policy

Physical estimates are **unavailable** unless Shade Editor has a dedicated versioned calibration object that binds the estimate to the production context required to interpret it.

At minimum such a future calibration must identify:

- machine/printhead context;
- RIP and version;
- exact linearization/calibration identity;
- target channel order and ink identities;
- the physical conversion factor and its unit for each channel;
- the measurement/derivation method and calibration revision;
- enough identity information to invalidate the estimate when any relevant production calibration changes.

Mass or cost additionally requires explicit density/price inputs with versioned units and identity. Missing or stale calibration must fail closed to **relative coverage only** rather than silently reusing old physical factors.

Adding this physical-calibration domain is optional post-v1 scope. It is not required for the current deterministic relative-consumption metric and must not be approximated from generic assumptions.

## Operator interpretation

Use relative metrics to answer questions such as:

- Which Candidate uses more or less of a given channel?
- What is the signed total relative coverage difference between two Candidates?
- Does a recipe reduce one ink while increasing another?

Do not use uncalibrated relative metrics to claim:

- litres or kilograms consumed by a job;
- printer cost per square metre;
- exact savings in currency;
- measured machine consumption.

Those claims require the physical calibration contract described above.
