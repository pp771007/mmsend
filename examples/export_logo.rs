//! Renders the app icon for the README: `cargo run --example export_logo -- docs/logo.png`

#[path = "../src/icon.rs"]
mod icon;

const SIZE: u32 = 256;

fn main() {
    let path = std::env::args().nth(1).expect("usage: export_logo <output.png>");
    image::save_buffer(&path, &icon::rgba(SIZE), SIZE, SIZE, image::ExtendedColorType::Rgba8).expect("failed to write the png");
}
