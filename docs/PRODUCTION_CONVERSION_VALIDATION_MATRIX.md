# Production Conversion Validation Matrix

This matrix separates automated internal conformance from external application/RIP approval. A green CI run proves the internal contracts below; it does **not** by itself claim Photoshop, RIP, printer, ink, or fired-tile approval.

## Automated internal release contract

| Area | Required coverage | Current status |
| --- | --- | --- |
| 5-channel TIFF | Direct-coverage N-channel samples round-trip with production ink semantics | Automated in Windows CI |
| 7-channel TIFF | Channel topology, names, embedded target ICC and pixel samples round-trip | Automated in Windows CI |
| BigTIFF | N-channel writer selects/round-trips BigTIFF without requiring multi-GiB fixture allocation | Automated in Windows CI |
| Output commit safety | A destination created after capture is never overwritten by a new-only conversion commit | Automated in Windows CI |
| N-channel target topology | Supported channel counts, unique channel names and missing authoritative colorant metadata are fail-closed/explicit | Automated in Windows CI |
| LittleCMS N-channel formats | Supported channel counts map to explicit formats; unsupported counts fail before transform construction | Automated in Windows CI |
| DeviceLink topology | DeviceLink input/output topology is authoritative and the captured runtime path bypasses a source-ICC chain | Automated in Windows CI |
| DeviceLink fixture | Real CMYK DeviceLink conversion remains deterministic and ink-limited | Automated in Windows CI |

`src/production_acceptance.rs` pins the exact regression-test names that implement this release contract. Removing or renaming one of those tests therefore breaks the acceptance suite instead of silently reducing coverage.

## External acceptance matrix

| Consumer / validation | Required evidence | Status |
| --- | --- | --- |
| Adobe Photoshop | Open generated 5C/7C TIFF; verify dimensions, bit depth, channel count/order/names, spot/base-ink interpretation, embedded target ICC behavior and visible sample parity | Pending manual fixture approval |
| Ceramic RIP | Import the same approved fixtures; verify channel mapping/order/names, bit depth, raster dimensions, ink polarity/coverage and no implicit channel remap | Pending manual fixture approval |
| Production printer workflow | Confirm RIP output maps each generated separation to the intended physical ink and preserves configured limits | Pending production approval |
| Measurement / fired result | Record target, ink set, profile/DeviceLink identity, print/firing conditions and measured/fired acceptance result | Pending calibrated production dataset |

## External evidence record

Every row moved out of `Pending` must be backed by a reproducible evidence record containing at least:

- source fixture name and SHA-256;
- generated output TIFF SHA-256;
- exact target ICC/DeviceLink description and SHA-256;
- Shade Editor version and immutable conversion-recipe identity;
- consumer name/version (Photoshop, RIP, printer workflow, or measurement system);
- dimensions, bit depth, channel count, channel order and exact channel names observed by that consumer;
- pass/fail result plus notes or attached screenshots/reports where appropriate.

Production/fired approval additionally records printer/RIP configuration, physical ink set, substrate/body, firing conditions and the measurement/fired-result reference. Approval evidence must identify the exact fixture and profile bytes; filename-only or visually similar replacements are not equivalent evidence.

`src/external_validation_evidence.rs` provides the typed Photoshop/RIP packet used to prepare this manual evidence from one validated `ConversionAuditRecord`. The packet copies only audit-bound fixture identities/topology, starts both consumers as `pending`, and rejects an asserted `passed` state unless exact consumer/version, observed bit depth, exact channel order/names, required semantic checks, evidence reference and reviewer/timestamp are present. The packet is an evidence-capture aid only: generating it does not move either external row out of `Pending`.

## Approval rule

External rows must stay `Pending` until evidence from the named consumer is attached to the corresponding validation work. Internal unit/integration tests or an uncompleted generated validation packet must never be used as a substitute for external Photoshop/RIP/production approval.

Related milestone work: #91, #96 and #472.
