# v0.18.4 interaction contract

This note records the user-facing interaction invariants introduced in Shade Editor 0.18.4.

## Curve keyboard editing

The selected Curve control point is keyboard-editable while the graph owns focus. Arrow keys move one displayed 8-bit unit and Shift+Arrow moves ten displayed units. Movement is defined in display coordinates so it follows the visible direction in both Light and Pigment modes.

## Master scope

`Master` is the user-facing name for the independent project-wide Levels/Curve finishing pass. The `~` / backtick logical shortcut toggles between Master and the selected channel. Selecting any channel from either the Channels panel or a numeric channel shortcut always returns to channel editing.

## Snapshot safety

An active Snapshot whose working adjustments differ from its stored Snapshot state cannot be silently switched away from or written to a `.shade` save. The shared `Snapshot changes not updated` decision has exactly three outcomes: Stay, Discard, or Update snapshot. Discard restores the stored Snapshot state and truncates the abandoned adjustment-history branch; Update commits the working state before continuing the requested switch/save.

## Light and Pigment display modes

Light and Pigment are presentation/interaction modes only. Internal adjustment math, TIFF working samples, Snapshot recipes and export samples remain unchanged. The UI keeps 0–255 numeric controls. Pigment mirrors both axes and histogram positions through `display = 1 - working`, and user input is mapped back through the same transform. This keeps the identity diagonal visually stable while making low displayed values represent low ink/pigment and high displayed values represent high ink/pigment.

## History

The History panel follows the current state automatically as new entries are appended. A Snapshot discard removes later dirty history states instead of leaving them available as redo states.
