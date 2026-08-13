# Shade Editor Windows Shell integration

This directory contains the native x64 Windows Shell support for `.shade` projects. It is intentionally independent from egui/eframe and the TIFF processing backend so Explorer never loads the full editor runtime.

`ShadeEditorShell.dll` implements one read-only COM class exposing `IInitializeWithStream`, `IThumbnailProvider`, and `IPropertyStore`. The thumbnail provider decodes the PNG already embedded in schema-v9 `.shade` files through Windows Imaging Component (WIC). The property store reads the cached `file_metadata` block and exposes project/active-Face properties without opening source TIFFs.

The custom properties use FMTID `{E1486A27-9A7B-4E56-BBD1-50D7F01C1778}`. The COM class is `{6F49F9D5-0F3A-4BF0-8C74-8A59951A75D2}`.

The build runs two native regressions: `ShadeProjectDataTests` checks schema-v9 JSON/base64 parsing; `ShadeShellTests` loads the produced DLL through `DllGetClassObject`, initializes it from an in-memory `.shade` stream, queries custom properties, and asks WIC for the embedded PNG thumbnail.

`Install-ShadeEditorShell.ps1` requires elevation because Microsoft registers property handlers under `HKLM\Software\Microsoft\Windows\CurrentVersion\PropertySystem\PropertyHandlers`. The COM DLL/property schema are copied to a versioned Program Files folder so a DLL already loaded by Explorer does not block a later upgrade. File association/presentation values are written per-user.

After installation refresh Explorer. Thumbnail cache can retain an older generic icon until the folder or Explorer is refreshed. Production validation should include Details view, Details pane, Windows Search indexing, double-click association, upgrade, and removal on a clean workstation.
