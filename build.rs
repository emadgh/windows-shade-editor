use std::{env, fs, path::PathBuf};

fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let clean: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if clean.len() % 4 != 0 {
        return Err("invalid base64 length".to_owned());
    }

    let mut out = Vec::with_capacity(clean.len() / 4 * 3);
    for chunk in clean.chunks_exact(4) {
        let a = value(chunk[0]).ok_or_else(|| "invalid base64 character".to_owned())? as u32;
        let b = value(chunk[1]).ok_or_else(|| "invalid base64 character".to_owned())? as u32;
        let c = if chunk[2] == b'=' {
            0
        } else {
            value(chunk[2]).ok_or_else(|| "invalid base64 character".to_owned())? as u32
        };
        let d = if chunk[3] == b'=' {
            0
        } else {
            value(chunk[3]).ok_or_else(|| "invalid base64 character".to_owned())? as u32
        };
        let packed = (a << 18) | (b << 12) | (c << 6) | d;
        out.push(((packed >> 16) & 0xff) as u8);
        if chunk[2] != b'=' {
            out.push(((packed >> 8) & 0xff) as u8);
        }
        if chunk[3] != b'=' {
            out.push((packed & 0xff) as u8);
        }
    }
    Ok(out)
}

fn png_as_ico(png: &[u8], width: u8, height: u8) -> Vec<u8> {
    let mut ico = Vec::with_capacity(22 + png.len());
    ico.extend_from_slice(&0u16.to_le_bytes());
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.push(width);
    ico.push(height);
    ico.push(0);
    ico.push(0);
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&32u16.to_le_bytes());
    ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(png);
    ico
}

fn main() {
    println!("cargo:rerun-if-changed=assets/shade-editor-icon.png.b64");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let png_b64 = include_str!("assets/shade-editor-icon.png.b64");
    let png = decode_base64(png_b64).expect("decode embedded Shade Editor icon PNG");
    // The committed source icon is a verified 64×64 PNG; Windows can scale the
    // embedded PNG icon cleanly for taskbar and small file-association views.
    let ico = png_as_ico(&png, 64, 64);
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let icon_path = out_dir.join("shade-editor.ico");
    fs::write(&icon_path, ico).expect("write generated Shade Editor ICO");

    let mut resource = winresource::WindowsResource::new();
    resource.set_icon(icon_path.to_string_lossy().as_ref());
    resource.compile().expect("embed Shade Editor Windows icon");
}
