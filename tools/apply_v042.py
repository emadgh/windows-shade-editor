from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "src" / "app_main.rs"
CARGO = ROOT / "Cargo.toml"
NOTES = ROOT / "RELEASE_NOTES.md"

text = APP.read_text(encoding="utf-8")

old_select = '''    fn select_channel(&mut self, channel: usize, isolate: bool) {
        self.selected_channel = channel;
        let next_solo = if isolate { Some(channel) } else { None };
        if self.solo_channel != next_solo {
            self.solo_channel = next_solo;
            self.mark_current_preview_dirty();
        }
    }
'''
new_select = '''    fn select_channel(&mut self, channel: usize, isolate: bool) {
        let previous_solo = self.solo_channel;
        if isolate {
            let (selected, solo) = channel_click_state(self.selected_channel, self.solo_channel, channel);
            self.selected_channel = selected;
            self.solo_channel = solo;
        } else {
            self.selected_channel = channel;
            self.solo_channel = None;
        }
        if self.solo_channel != previous_solo {
            self.mark_current_preview_dirty();
        }
    }
'''
if old_select not in text:
    raise SystemExit("select_channel block not found")
text = text.replace(old_select, new_select, 1)

old_channel_row = '''            let accent = channel_color(name, index);
            let label = format!("●  {name}{suffix}");
            if clickable_row(
                ui,
                self.selected_channel == index,
                &label,
                None,
                Some(accent),
                32.0,
            )
            .clicked()
            {
                self.select_channel(index, true);
            }
'''
new_channel_row = '''            let accent = channel_color(name, index);
            let is_solo = self.solo_channel == Some(index);
            let indicator = if is_solo { "■" } else { "□" };
            let label = format!("{indicator}  {name}{suffix}");
            let response = clickable_row(
                ui,
                self.selected_channel == index,
                &label,
                None,
                Some(accent),
                32.0,
            )
            .on_hover_text("Click to select for editing. Click the active channel again to toggle solo preview.");
            if response.clicked() {
                self.select_channel(index, true);
            }
'''
if old_channel_row not in text:
    raise SystemExit("channel row block not found")
text = text.replace(old_channel_row, new_channel_row, 1)

old_constant = '''        let mut constant_slider = egui::Slider::new(&mut adjustment.mixer.constant, -1.0..=1.0)
            .text("Constant")
            .trailing_fill(true);
'''
new_constant = '''        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        let mut constant_slider = egui::Slider::new(&mut adjustment.mixer.constant, -1.0..=1.0)
            .text("Constant")
            .trailing_fill(true);
'''
if old_constant not in text:
    raise SystemExit("constant slider block not found")
text = text.replace(old_constant, new_constant, 1)

marker = '''fn channel_color(name: &str, index: usize) -> egui::Color32 {
'''
helper = '''fn channel_click_state(
    selected_channel: usize,
    solo_channel: Option<usize>,
    clicked_channel: usize,
) -> (usize, Option<usize>) {
    if selected_channel != clicked_channel {
        // First click on another channel selects it for editing and returns to composite.
        (clicked_channel, None)
    } else if solo_channel == Some(clicked_channel) {
        // Second click while solo is active returns to the composite preview.
        (selected_channel, None)
    } else {
        // Clicking the already-selected channel toggles its monochrome solo preview on.
        (selected_channel, Some(clicked_channel))
    }
}

#[cfg(test)]
mod channel_interaction_tests {
    use super::channel_click_state;

    #[test]
    fn first_click_selects_without_solo_then_active_click_toggles_solo() {
        assert_eq!(channel_click_state(0, None, 2), (2, None));
        assert_eq!(channel_click_state(2, None, 2), (2, Some(2)));
        assert_eq!(channel_click_state(2, Some(2), 2), (2, None));
    }

    #[test]
    fn selecting_another_channel_exits_previous_solo() {
        assert_eq!(channel_click_state(2, Some(2), 4), (4, None));
    }
}

'''
if marker not in text:
    raise SystemExit("channel_color marker not found")
text = text.replace(marker, helper + marker, 1)
APP.write_text(text, encoding="utf-8")

cargo = CARGO.read_text(encoding="utf-8")
if 'version = "0.4.1"' not in cargo:
    raise SystemExit("Cargo version 0.4.1 not found")
cargo = cargo.replace('version = "0.4.1"', 'version = "0.4.2"', 1)
CARGO.write_text(cargo, encoding="utf-8")

NOTES.write_text('''# Shade Editor 0.4.2

Channel selection / solo-preview interaction refinement.

## Changed

- Clicking a different channel now selects it for editing while keeping the composite preview.
- Clicking the already-selected channel toggles that channel's monochrome Solo Preview on/off.
- Channel rows show an outline square (`□`) normally and a filled square (`■`) when that channel is soloed.
- Selecting a different channel while another channel is soloed automatically returns the viewport to Composite before editing the new channel.
- Channel Mixer `Constant` is visually separated from the source-channel coefficients with additional spacing and a divider.

## Retained

- Full-row Face / Channel / Snapshot selection, dated Snapshot groups, unique Snapshot names and per-channel adjustment color cues from 0.4.1.
- Multi-channel RGB/CMYK + Photoshop Spot TIFF support, background rendering, progress, updater, logs, DPI-aware test code and Fit viewport.
''', encoding="utf-8")

print("v0.4.2 source patch applied")
