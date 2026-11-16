#[path = "src/icon.rs"]
mod icon;

use std::path::Path;

/// Sizes Explorer picks from for list, tile and large-icon views.
const ICO_SIZES: [u32; 4] = [16, 32, 48, 256];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/icon.rs");
    // Version info shows up in the exe's Properties > Details tab; unsigned binaries
    // without it look more suspicious to antivirus heuristics.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
        let ico_path = Path::new(&out_dir).join("mmsend.ico");
        std::fs::write(&ico_path, ico_file()).expect("failed to write icon");

        let mut res = winresource::WindowsResource::new();
        // Task Manager lists the process under this name.
        res.set("FileDescription", "Mattermost 指令傳訊");
        res.set("ProductName", "mmsend");
        res.set_icon(ico_path.to_str().expect("OUT_DIR is valid UTF-8"));
        res.compile().expect("failed to embed Windows resources");
    }
}

/// ICO container with one 32-bit BMP image per size.
fn ico_file() -> Vec<u8> {
    let images: Vec<Vec<u8>> = ICO_SIZES.iter().map(|&s| bmp_image(s)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (&size, image) in ICO_SIZES.iter().zip(&images) {
        let dimension = if size >= 256 { 0 } else { size as u8 }; // 0 means 256
        out.extend_from_slice(&[dimension, dimension, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(image.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += image.len() as u32;
    }
    for image in images {
        out.extend_from_slice(&image);
    }
    out
}

fn bmp_image(size: u32) -> Vec<u8> {
    let rgba = icon::rgba(size);
    let mask_row_bytes = size.div_ceil(32) * 4;
    let mut out = Vec::new();
    // BITMAPINFOHEADER; height is doubled because the AND mask follows the pixels.
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    out.extend_from_slice(&((size * 2) as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]); // no compression, sizes and palette left default
    for row in (0..size).rev() {
        for col in 0..size {
            let i = ((row * size + col) * 4) as usize;
            out.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    // All-zero AND mask: transparency comes from the alpha channel.
    out.extend(std::iter::repeat_n(0u8, (mask_row_bytes * size) as usize));
    out
}
