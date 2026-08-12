# Shade Editor 0.6.1 vector UI icons

Snapshot export and exported-folder status icons are rendered directly with `egui::Painter` geometry.

This removes dependency on Unicode glyph availability for the compact Export and Check controls. The same reusable `VectorIconButton` widget is used at the Snapshot panel, day-group, and individual Snapshot levels.

No icon font installation is required on Windows.
