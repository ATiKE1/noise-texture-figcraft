//! Render the built-in presets as one contact sheet PNG: `cargo run --release --example previews
//! -- docs/previews.png [cell size] [--tiles]`. Each preset fills a cell over a light grey base (a
//! dark one for light particles), tiled so that seams would show; with `--tiles`, each preset is
//! also written on its own next to the sheet.

use noise_texture::presets;
use noise_texture::texture::{Settings, render};

/// The base under a preset's texture: dark grey under light particles, light grey otherwise.
fn base(s: &Settings) -> [u8; 3] {
    let lum = 0.2126 * f64::from(s.color[0]) + 0.7152 * f64::from(s.color[1]) + 0.0722 * f64::from(s.color[2]);
    if lum > 160.0 && s.background_opacity < 100.0 { [40, 40, 44] } else { [236, 236, 232] }
}

/// The cell of preset `s`: `cell` × `cell` pixels of its tile repeated, over its base.
fn cell_pixels(s: &Settings, cell: u32) -> Vec<u8> {
    let t = render(s);
    let rgba = t.rgba();
    let b = base(s);
    let mut out = Vec::with_capacity((cell * cell * 3) as usize);
    for y in 0..cell {
        for x in 0..cell {
            let i = (((y % t.height) * t.width + x % t.width) * 4) as usize;
            let a = f64::from(rgba[i + 3]) / 255.0;
            for c in 0..3 {
                out.push((f64::from(rgba[i + c]) * a + f64::from(b[c]) * (1.0 - a)).round() as u8);
            }
        }
    }
    out
}

fn write_png(path: &std::path::Path, w: u32, h: u32, rgb: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let pixels: Vec<u32> = rgb.chunks(3).map(|p| u32::from_le_bytes([p[0], p[1], p[2], 255])).collect();
    let png = noise_texture::png::encode(w, h, &pixels).ok_or("encode")?;
    std::fs::write(path, png)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(args.first().map_or("previews.png", String::as_str));
    let cell: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(192);
    let tiles = args.iter().any(|a| a == "--tiles");
    let gap = 8u32;
    let all = presets::built_in();
    let cols = 4u32;
    let rows = (all.len() as u32).div_ceil(cols);
    let (w, h) = (cols * cell + (cols + 1) * gap, rows * cell + (rows + 1) * gap);
    let mut sheet = vec![255u8; (w * h * 3) as usize];
    for (k, (name, v)) in all.iter().enumerate() {
        // Small tiles, so that each cell shows a few repeats.
        let s = Settings { tile: 128, ..Settings::from_json(v) };
        let px = cell_pixels(&s, cell);
        let (ox, oy) = (gap + (k as u32 % cols) * (cell + gap), gap + (k as u32 / cols) * (cell + gap));
        for y in 0..cell {
            let from = (y * cell * 3) as usize;
            let to = (((oy + y) * w + ox) * 3) as usize;
            sheet[to..to + (cell * 3) as usize].copy_from_slice(&px[from..from + (cell * 3) as usize]);
        }
        if tiles {
            let slug: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
            write_png(&out.with_file_name(format!("{slug}.png")), cell, cell, &px)?;
        }
        println!("{name}: {}", s.kind.label());
    }
    write_png(&out, w, h, &sheet)?;
    println!("wrote {}", out.display());
    Ok(())
}
