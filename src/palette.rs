use serde::{Deserialize, Serialize};

pub const AUTO_PALETTE_ID: &str = "builtin:auto";
pub const CMYK_PALETTE_ID: &str = "builtin:cmyk";
pub const RGB_PALETTE_ID: &str = "builtin:rgb";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelPaletteEntry {
    pub name: String,
    pub color: [u8; 3],
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelPalette {
    pub id: String,
    pub name: String,
    pub channels: Vec<ChannelPaletteEntry>,
}

impl ChannelPalette {
    pub fn display_name<'a>(&'a self, actual_name: &'a str, index: usize) -> &'a str {
        self.channels
            .get(index)
            .map(|entry| entry.name.trim())
            .filter(|name| !name.is_empty())
            .unwrap_or(actual_name)
    }

    pub fn color(&self, actual_name: &str, index: usize) -> [u8; 3] {
        self.channels
            .get(index)
            .map(|entry| entry.color)
            .unwrap_or_else(|| fallback_channel_color(actual_name, index))
    }
}

pub fn builtin_cmyk() -> ChannelPalette {
    ChannelPalette {
        id: CMYK_PALETTE_ID.to_owned(),
        name: "CMYK — International".to_owned(),
        channels: vec![
            ChannelPaletteEntry {
                name: "Cyan".to_owned(),
                color: [0, 190, 220],
            },
            ChannelPaletteEntry {
                name: "Magenta".to_owned(),
                color: [225, 45, 150],
            },
            ChannelPaletteEntry {
                name: "Yellow".to_owned(),
                color: [225, 190, 20],
            },
            ChannelPaletteEntry {
                name: "Black".to_owned(),
                color: [155, 155, 155],
            },
        ],
    }
}

pub fn builtin_rgb() -> ChannelPalette {
    ChannelPalette {
        id: RGB_PALETTE_ID.to_owned(),
        name: "RGB — International".to_owned(),
        channels: vec![
            ChannelPaletteEntry {
                name: "Red".to_owned(),
                color: [225, 70, 70],
            },
            ChannelPaletteEntry {
                name: "Green".to_owned(),
                color: [65, 185, 95],
            },
            ChannelPaletteEntry {
                name: "Blue".to_owned(),
                color: [65, 125, 225],
            },
        ],
    }
}

pub fn builtin_palettes() -> Vec<ChannelPalette> {
    vec![builtin_cmyk(), builtin_rgb()]
}

pub fn is_builtin_id(id: &str) -> bool {
    matches!(id, CMYK_PALETTE_ID | RGB_PALETTE_ID)
}

pub fn fallback_channel_color(name: &str, index: usize) -> [u8; 3] {
    let lower = name.to_ascii_lowercase();
    if lower == "cyan" || lower == "c" {
        return [0, 190, 220];
    }
    if lower == "magenta" || lower == "m" {
        return [225, 45, 150];
    }
    if lower == "yellow" || lower == "y" {
        return [225, 190, 20];
    }
    if lower == "black" || lower == "k" {
        return [155, 155, 155];
    }
    if lower == "red" || lower == "r" {
        return [225, 70, 70];
    }
    if lower == "green" || lower == "g" {
        return [65, 185, 95];
    }
    if lower == "blue" || lower == "b" {
        return [65, 125, 225];
    }
    const SPOTS: [[u8; 3]; 8] = [
        [130, 95, 220],
        [60, 180, 95],
        [235, 105, 55],
        [65, 135, 230],
        [220, 80, 95],
        [40, 180, 175],
        [190, 110, 45],
        [180, 80, 190],
    ];
    SPOTS[index % SPOTS.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_stable_and_read_only_ids() {
        let cmyk = builtin_cmyk();
        let rgb = builtin_rgb();
        assert_eq!(cmyk.channels.len(), 4);
        assert_eq!(rgb.channels.len(), 3);
        assert!(is_builtin_id(&cmyk.id));
        assert!(is_builtin_id(&rgb.id));
    }

    #[test]
    fn palette_falls_back_after_defined_slots() {
        let palette = builtin_cmyk();
        assert_eq!(palette.display_name("purpol", 4), "purpol");
        assert_eq!(
            palette.color("purpol", 4),
            fallback_channel_color("purpol", 4)
        );
    }
}
