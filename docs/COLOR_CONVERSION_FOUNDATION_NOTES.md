# Color Conversion Foundation Test Scope

This short note accompanies the first implementation slice for #84/#98.

The slice intentionally does **not** change runtime conversion behavior yet. It establishes independently testable domain contracts before pixel conversion, TIFF topology mutation or UI integration.

Covered by unit tests in `src/color_conversion.rs`:

- backward-safe `Standalone` default role vocabulary;
- versioned conversion-recipe round-trip serialization;
- target topology duplicate-channel rejection;
- invalid/unknown ink-priority rejection;
- Custom Optimizer characterization prerequisite;
- Black-focused strategy serialization/validation.

Next implementation slices should build on these contracts rather than introducing parallel UI-owned conversion state.
