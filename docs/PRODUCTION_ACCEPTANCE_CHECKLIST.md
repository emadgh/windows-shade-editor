# Shade Editor Production Acceptance Checklist

This checklist separates automated conformance evidence from checks that require the real Photoshop/RIP/printer/Windows workstation environment. Do not mark an external item complete without recording concrete evidence.

## Automated CI evidence

- [x] Rust Windows target compiles in the required workflow.
- [x] Full Rust test suite runs in the required workflow.
- [x] Release executable builds in the required workflow.
- [x] Native Explorer Shell extension is built and tested in the required workflow.
- [x] Shell property schema XML is validated in the required workflow.
- [x] Synthetic TIFF matrix covers lossless compression preservation/forced LZW, horizontal predictor, RGB8, CMYK16, Gray export, ExtraSamples/Spot metadata, ICC, Photoshop resources, DPI, planar-separate, tiled-contiguous, tiled-separate and bounded streaming paths.
- [x] BigTIFF structure/container behavior is tested without allocating multi-gigabyte sample buffers.
- [x] Missing-Face placeholder/relink paths have automated unit coverage and remain guarded before export.
- [x] No-adjustment validation compares channel order, samples, ICC, Photoshop resources, Spot DisplayInfo, DPI, compression and predictor.

## Photoshop production round-trip — external evidence required

Status: [ ] NOT SIGNED OFF

Record for each representative production CMYK + Spot TIFF:

- Source file / SHA-256:
- Shade Editor build / commit:
- Photoshop exact version:
- Opened without repair/warning: [ ]
- Dimensions/bit depth unchanged: [ ]
- CMYK base-channel order unchanged: [ ]
- Spot names/order unchanged: [ ]
- Spot color/solidity semantics unchanged: [ ]
- Embedded ICC unchanged where expected: [ ]
- DPI/physical size unchanged: [ ]
- No-adjustment exported samples accepted as equivalent: [ ]
- Evidence location/screenshots/report:
- Reviewer/date:

## Production RIP / printer interpretation — external evidence required

Status: [ ] NOT SIGNED OFF

- RIP/printer model:
- RIP software/version:
- Media/ink/profile configuration:
- Representative source SHA-256:
- Shade Editor output SHA-256:
- Channel count/order interpreted correctly: [ ]
- Spot separations interpreted correctly: [ ]
- No unexpected color conversion by transport: [ ]
- Soft Proof compared against actual RIP/printer result: [ ]
- Gamut-warning behavior reviewed: [ ]
- Evidence / measurement / printed sample reference:
- Reviewer/date:

## >4 GiB BigTIFF production acceptance — external evidence required

Status: [ ] NOT SIGNED OFF

- Test file size:
- Dimensions / channels / bit depth:
- Photoshop exact version:
- RIP exact version:
- Shade Editor export completed: [ ]
- Photoshop opened output without repair: [ ]
- RIP accepted/output channels correctly: [ ]
- Spot metadata verified: [ ]
- Evidence location:
- Reviewer/date:

## Clean Windows workstation Shell integration — external evidence required

Status: [ ] NOT SIGNED OFF

- Windows edition/build:
- Fresh user/profile or VM image:
- Install script elevated successfully: [ ]
- `.shade` association correct: [ ]
- Explorer thumbnail appears after cache refresh: [ ]
- Explorer properties appear correctly: [ ]
- Upgrade from previous packaged version tested: [ ]
- Uninstall removes handlers/association cleanly: [ ]
- Explorer remains stable after install/uninstall: [ ]
- Evidence location:
- Reviewer/date:

## Sign-off rule

The application may be considered **CI-hardened** when all automated checks are green. It may be considered **production-environment accepted** only after all four external sections above are completed with actual evidence. CI or synthetic fixtures must never be substituted for Photoshop/RIP/printer/workstation sign-off.
