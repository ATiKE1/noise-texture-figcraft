//! Texture settings and the generator: a square tile of RGBA pixels that repeats without a seam.
//!
//! The tile is a grid of particles, `size` pixels square. Each kind of texture gives every particle
//! a coverage (where the particle colour shows) and a tone (the mix between the two colours in
//! two-colour mode); the colour mode turns that into a colour (in random-colour mode, a random hue
//! per particle), the particle shape masks it, and it's laid over the background.

use figcraft_plugin_sdk::{Value, json};

use crate::noise::{channel, fbm, hash, turbulence, unit, worley};

/// The kinds of texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Static,
    Grain,
    Clouds,
    Marble,
    Wood,
    Cells,
    Waves,
    Halftone,
}

impl Kind {
    pub const ALL: [Kind; 8] = [Kind::Static, Kind::Grain, Kind::Clouds, Kind::Marble, Kind::Wood, Kind::Cells, Kind::Waves, Kind::Halftone];

    pub fn id(self) -> &'static str {
        match self {
            Kind::Static => "static",
            Kind::Grain => "grain",
            Kind::Clouds => "clouds",
            Kind::Marble => "marble",
            Kind::Wood => "wood",
            Kind::Cells => "cells",
            Kind::Waves => "waves",
            Kind::Halftone => "halftone",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Static => "Static",
            Kind::Grain => "Grain",
            Kind::Clouds => "Clouds",
            Kind::Marble => "Marble",
            Kind::Wood => "Wood",
            Kind::Cells => "Cells",
            Kind::Waves => "Waves",
            Kind::Halftone => "Halftone",
        }
    }

    /// What it looks like and which settings it uses, for the panel.
    pub fn about(self) -> &'static str {
        match self {
            Kind::Static => "Random particles, like TV static. Amount: how many particles there are.",
            Kind::Grain => "Particles of random strength, like film grain. Amount: how many particles there are.",
            Kind::Clouds => "Soft fractal noise. Feature scale: cloud size. Detail: layers of finer noise. Amount: coverage.",
            Kind::Marble => "Veins in turbulent noise. Feature scale: vein spacing. Detail: how turbulent. Amount: coverage.",
            Kind::Wood => "Wavy wood grain. Feature scale: ring spacing. Detail: how wavy. Amount: coverage.",
            Kind::Cells => "Cracked cells, like stone or crackle glaze. Feature scale: cell size. Detail: surface noise. Amount: coverage.",
            Kind::Waves => "Rippled stripes. Feature scale: wavelength. Detail: how rippled. Amount: coverage.",
            Kind::Halftone => "A screen of dots sized by noise. Feature scale: dot spacing. Detail: tone variation. Amount: average tone.",
        }
    }

    /// The kind with this id or label (any case).
    pub fn parse(s: &str) -> Option<Kind> {
        let s = s.trim();
        Kind::ALL.into_iter().find(|k| k.id().eq_ignore_ascii_case(s) || k.label().eq_ignore_ascii_case(s))
    }
}

/// How particles are coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    /// The particle colour; the texture sets its opacity.
    Single,
    /// A mix of the particle colour and the second colour; the texture sets the mix.
    Duo,
    /// A random hue for each particle; the texture sets its opacity.
    Multi,
}

impl ColorMode {
    pub const ALL: [ColorMode; 3] = [ColorMode::Single, ColorMode::Duo, ColorMode::Multi];

    pub fn id(self) -> &'static str {
        match self {
            ColorMode::Single => "single",
            ColorMode::Duo => "duo",
            ColorMode::Multi => "multi",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ColorMode::Single => "Single colour",
            ColorMode::Duo => "Two colours",
            ColorMode::Multi => "Random colours",
        }
    }

    pub fn parse(s: &str) -> Option<ColorMode> {
        let s = s.trim();
        ColorMode::ALL.into_iter().find(|m| m.id().eq_ignore_ascii_case(s) || m.label().eq_ignore_ascii_case(s))
    }
}

/// The shape of a particle in its square.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Square,
    Circle,
}

impl Shape {
    pub const ALL: [Shape; 2] = [Shape::Square, Shape::Circle];

    pub fn label(self) -> &'static str {
        match self {
            Shape::Square => "Square",
            Shape::Circle => "Circle",
        }
    }

    pub fn parse(s: &str) -> Option<Shape> {
        Shape::ALL.into_iter().find(|x| x.label().eq_ignore_ascii_case(s.trim()))
    }
}

/// The blend modes a texture fill can use: (label, FigCraft's id).
pub const BLENDS: [(&str, &str); 16] = [
    ("Normal", "normal"),
    ("Darken", "darken"),
    ("Multiply", "multiply"),
    ("Color burn", "colorBurn"),
    ("Lighten", "lighten"),
    ("Screen", "screen"),
    ("Color dodge", "colorDodge"),
    ("Overlay", "overlay"),
    ("Soft light", "softLight"),
    ("Hard light", "hardLight"),
    ("Difference", "difference"),
    ("Exclusion", "exclusion"),
    ("Hue", "hue"),
    ("Saturation", "saturation"),
    ("Color", "color"),
    ("Luminosity", "luminosity"),
];

/// The index in [`BLENDS`] of the mode with this label or id (any case, spaces ignored).
pub fn parse_blend(s: &str) -> Option<usize> {
    let key: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    BLENDS.iter().position(|(label, id)| label.replace(' ', "").eq_ignore_ascii_case(&key) || id.eq_ignore_ascii_case(&key))
}

/// The tile sizes the panel offers, in pixels.
pub const TILES: [u32; 4] = [64, 128, 256, 512];

/// Ranges of the numeric settings.
pub const SIZE: (u32, u32) = (1, 64);
pub const SCALE: (u32, u32) = (2, 512);
pub const DETAIL: (u32, u32) = (1, 8);
pub const TILE: (u32, u32) = (16, 512);
pub const SEED: (u32, u32) = (0, 999_999);

/// An sRGB colour with alpha, 0–255 each.
pub type Rgba = [u8; 4];

/// `#RGB`, `#RRGGBB` or `#RRGGBBAA` (the `#` optional).
pub fn parse_hex(s: &str) -> Option<Rgba> {
    let h = s.trim().trim_start_matches('#');
    if !h.is_ascii() || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| h.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok());
    match h.len() {
        3 => {
            let d: Vec<u8> = h.chars().filter_map(|c| c.to_digit(16)).map(|d| (d * 17) as u8).collect();
            Some([*d.first()?, *d.get(1)?, *d.get(2)?, 255])
        }
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

/// `#RRGGBB`, or `#RRGGBBAA` when it isn't opaque.
pub fn hex(c: Rgba) -> String {
    if c[3] == 255 { format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]) } else { format!("#{:02X}{:02X}{:02X}{:02X}", c[0], c[1], c[2], c[3]) }
}

/// Everything a texture is made from. The JSON field names are the plugin's parameter names.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub kind: Kind,
    /// Particle size, pixels.
    pub size: u32,
    /// Feature size of the procedural kinds, pixels.
    pub scale: u32,
    /// Amount, percent.
    pub density: f64,
    /// Octaves of detail.
    pub detail: u32,
    pub shape: Shape,
    pub color_mode: ColorMode,
    pub color: Rgba,
    pub color2: Rgba,
    /// Particle opacity, percent.
    pub opacity: f64,
    pub background: Rgba,
    /// Background opacity, percent (0: transparent).
    pub background_opacity: f64,
    /// Index in [`BLENDS`].
    pub blend: usize,
    /// Tile size, pixels (rounded up to whole particles).
    pub tile: u32,
    pub seed: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            kind: Kind::Static,
            size: 1,
            scale: 64,
            density: 50.0,
            detail: 4,
            shape: Shape::Square,
            color_mode: ColorMode::Single,
            color: [0, 0, 0, 255],
            color2: [255, 255, 255, 255],
            opacity: 20.0,
            background: [255, 255, 255, 255],
            background_opacity: 0.0,
            blend: 0,
            tile: 256,
            seed: 1,
        }
    }
}

/// A whole number from a JSON number or numeric string, clamped to `range`.
fn int(v: &Value, range: (u32, u32)) -> Option<u32> {
    num(v).map(|n| n.round().clamp(f64::from(range.0), f64::from(range.1)) as u32)
}

/// A finite number from a JSON number or numeric string.
fn num(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().trim_end_matches(['%', 'x', 'p']).trim().parse().ok())).filter(|n: &f64| n.is_finite())
}

fn percent(v: &Value) -> Option<f64> {
    num(v).map(|n| n.clamp(0.0, 100.0))
}

impl Settings {
    /// Settings from JSON (a run's parameters, a panel's values, a saved preset): anything missing
    /// or that doesn't fit is taken from `base`, numbers are clamped to their ranges.
    pub fn from_json_over(v: &Value, base: &Settings) -> Settings {
        let s = |k: &str| v.get(k).and_then(Value::as_str);
        let f = |k: &str| v.get(k).unwrap_or(&Value::Null);
        Settings {
            kind: s("type").and_then(Kind::parse).unwrap_or(base.kind),
            size: int(f("size"), SIZE).unwrap_or(base.size),
            scale: int(f("scale"), SCALE).unwrap_or(base.scale),
            density: percent(f("density")).unwrap_or(base.density),
            detail: int(f("detail"), DETAIL).unwrap_or(base.detail),
            shape: s("shape").and_then(Shape::parse).unwrap_or(base.shape),
            color_mode: s("colorMode").and_then(ColorMode::parse).unwrap_or(base.color_mode),
            color: s("color").and_then(parse_hex).unwrap_or(base.color),
            color2: s("color2").and_then(parse_hex).unwrap_or(base.color2),
            opacity: percent(f("opacity")).unwrap_or(base.opacity),
            background: s("background").and_then(parse_hex).unwrap_or(base.background),
            background_opacity: percent(f("backgroundOpacity")).unwrap_or(base.background_opacity),
            blend: s("blend").and_then(parse_blend).unwrap_or(base.blend),
            tile: int(f("tile"), TILE).unwrap_or(base.tile),
            seed: int(f("seed"), SEED).unwrap_or(base.seed),
        }
    }

    pub fn from_json(v: &Value) -> Settings {
        Settings::from_json_over(v, &Settings::default())
    }

    pub fn to_json(&self) -> Value {
        json!({
            "type": self.kind.label(),
            "size": self.size,
            "scale": self.scale,
            "density": self.density,
            "detail": self.detail,
            "shape": self.shape.label(),
            "colorMode": self.color_mode.label(),
            "color": hex(self.color),
            "color2": hex(self.color2),
            "opacity": self.opacity,
            "background": hex(self.background),
            "backgroundOpacity": self.background_opacity,
            "blend": self.blend_label(),
            "tile": self.tile,
            "seed": self.seed,
        })
    }

    pub fn blend_label(&self) -> &'static str {
        BLENDS.get(self.blend).map_or("Normal", |b| b.0)
    }

    /// FigCraft's id of the blend mode.
    pub fn blend_id(&self) -> &'static str {
        BLENDS.get(self.blend).map_or("normal", |b| b.1)
    }

    /// The tile's side in pixels: whole particles, as close to `tile` as they come.
    pub fn tile_px(&self) -> u32 {
        let size = self.size.clamp(SIZE.0, SIZE.1);
        self.tile.clamp(TILE.0, TILE.1).div_ceil(size).max(1) * size
    }

    /// The same settings with the next seed (the sandbox has no randomness of its own).
    pub fn shuffled(&self) -> Settings {
        let seed = hash(self.seed as i32, 0x5eed, 0x2545_f491) % (SEED.1 + 1);
        Settings { seed: if seed == self.seed { (seed + 1) % (SEED.1 + 1) } else { seed }, ..self.clone() }
    }
}

/// A rendered tile: `width` × `height` straight-alpha RGBA pixels, row by row, each the four bytes
/// R, G, B, A read little-endian.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl Tile {
    /// The pixels as R, G, B, A bytes.
    pub fn rgba(&self) -> Vec<u8> {
        self.pixels.iter().flat_map(|p| p.to_le_bytes()).collect()
    }
}

/// Every particle's looks, row by row.
struct Particles {
    /// Where the particle colour shows, `0..1`: the opacity in single- and random-colour mode.
    cover: Vec<f32>,
    /// The mix in two-colour mode: 0 the second colour, 1 the particle colour.
    tone: Vec<f32>,
    /// Whether there's a particle at all (static and grain leave gaps).
    present: Vec<bool>,
}

/// Generate the tile.
pub fn render(s: &Settings) -> Tile {
    let size = s.size.clamp(SIZE.0, SIZE.1) as usize;
    let side = s.tile_px() as usize;
    let n = side / size;
    // Feature cells across the tile (whole cells, so the tile wraps).
    let cells = ((side as f64 / f64::from(s.scale.clamp(SCALE.0, SCALE.1))).round() as i32).max(1);
    let p = particles(s, n, cells);
    Tile { width: side as u32, height: side as u32, pixels: paint(s, &p, n, size) }
}

/// The particles of an `n` × `n` tile with `cells` feature cells across it.
fn particles(s: &Settings, n: usize, cells: i32) -> Particles {
    let d = (s.density.clamp(0.0, 100.0) / 100.0) as f32;
    let detail = s.detail.clamp(DETAIL.0, DETAIL.1);
    let seed = s.seed;
    match s.kind {
        Kind::Static | Kind::Grain => {
            let mut p = Particles { cover: Vec::with_capacity(n * n), tone: Vec::with_capacity(n * n), present: Vec::with_capacity(n * n) };
            let strength = channel(seed, 2);
            for y in 0..n as i32 {
                for x in 0..n as i32 {
                    // `unit` reads the top 24 bits, the coin the lowest one.
                    let h = hash(x, y, seed);
                    let on = unit(h) < d;
                    let (cover, tone) = if s.kind == Kind::Static {
                        (1.0, if h & 1 == 0 { 1.0 } else { 0.0 })
                    } else {
                        // A bell-shaped strength around the middle: the mean of three bytes.
                        let g = hash(x, y, strength);
                        let g = ((g & 0xff) + ((g >> 8) & 0xff) + ((g >> 16) & 0xff)) as f32 / 765.0;
                        let g = (0.5 + (g - 0.5) * 2.2).clamp(0.0, 1.0);
                        (g, g)
                    };
                    p.cover.push(if on { cover } else { 0.0 });
                    p.tone.push(tone);
                    p.present.push(on);
                }
            }
            p
        }
        Kind::Halftone => {
            let tones = halftone_tones(cells, d, detail, seed);
            // A particle in dot widths, for the anti-aliasing.
            let step = cells as f32 / n as f32;
            let at = |i: usize| (i as f32 + 0.5) * step;
            let cover: Vec<f32> = (0..n * n).map(|k| halftone(&tones, cells, at(k % n), at(k / n), step)).collect();
            Particles { tone: cover.clone(), cover, present: vec![true; n * n] }
        }
        kind => {
            let cover: Vec<f32> = field(kind, n, cells, detail, seed).into_iter().map(|f| coverage(f, d)).collect();
            Particles { tone: cover.clone(), cover, present: vec![true; n * n] }
        }
    }
}

/// `f` with the amount `d` (`0..1`) applied: at 1 all of it, less keeps only the top `d` of the
/// range, stretched back to `0..1`.
fn coverage(f: f32, d: f32) -> f32 {
    if d <= 0.0 {
        0.0
    } else if d >= 1.0 {
        f
    } else {
        ((f - (1.0 - d)) / d).clamp(0.0, 1.0)
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The procedural field of `kind` on an `n` × `n` grid, `0..1`.
fn field(kind: Kind, n: usize, cells: i32, detail: u32, seed: u32) -> Vec<f32> {
    // Noise finer than two particles adds little but speckle (and costs as much as any layer).
    let limit = (n as i32 / 2).max(1);
    let at = |i: usize| (i as f32 + 0.5) / n as f32;
    let half = (cells / 2).max(1);
    match kind {
        Kind::Clouds => fbm(n, (cells, cells), detail, limit, seed).into_iter().map(clouds).collect(),
        Kind::Marble => {
            let t = turbulence(n, (half, half), detail + 1, limit, seed);
            let cloud = fbm(n, (cells, cells), detail.min(4), limit, channel(seed, 9));
            (0..n * n).map(|k| marble(at(k % n), at(k / n), cells, t[k], cloud[k])).collect()
        }
        Kind::Wood => {
            let warp = fbm(n, (1, half), detail, limit, seed);
            let fibre = fbm(n, ((cells / 4).max(1), cells.saturating_mul(8)), 2, limit, channel(seed, 5));
            (0..n * n).map(|k| wood(at(k / n), cells, detail, warp[k], fibre[k])).collect()
        }
        Kind::Cells => {
            let (f1, f2) = worley(n, cells, seed);
            let grit = if detail > 1 { fbm(n, (cells * 4, cells * 4), detail - 1, limit, channel(seed, 6)) } else { vec![0.5; n * n] };
            (0..n * n).map(|k| crackle(f1[k], f2[k], grit[k])).collect()
        }
        Kind::Waves => {
            let warp = fbm(n, (2, 2), detail, limit, seed);
            (0..n * n).map(|k| waves(at(k % n), cells, detail, warp[k])).collect()
        }
        Kind::Static | Kind::Grain | Kind::Halftone => vec![0.0; n * n],
    }
}

/// Clouds: soft fractal noise `f`, its middle stretched.
fn clouds(f: f32) -> f32 {
    smoothstep(0.1, 0.9, f)
}

/// Marble at `(u, v)`: diagonal bands pushed around by turbulence `t`, veins where the bands' sine
/// crosses zero, and a little cloudiness between them.
fn marble(u: f32, v: f32, cells: i32, t: f32, cloud: f32) -> f32 {
    let band = (std::f32::consts::PI * (cells as f32 * (u + v) + 2.5 * t)).sin().abs();
    let vein = (1.0 - band).powi(6);
    (0.75 * vein + 0.25 * cloud * (1.0 - band)).clamp(0.0, 1.0)
}

/// Wood at height `v`: rings across the tile bent by `warp` (detail 4: about a ring either way),
/// with fibres along the grain.
fn wood(v: f32, cells: i32, detail: u32, warp: f32, fibre: f32) -> f32 {
    let t = cells as f32 * v + detail as f32 / 2.0 * (warp - 0.5);
    let ring = t - t.floor();
    let ring = smoothstep(0.0, 0.8, ring) * (1.0 - smoothstep(0.85, 1.0, ring));
    (0.75 * ring + 0.25 * smoothstep(0.3, 0.7, fibre)).clamp(0.0, 1.0)
}

/// Cracked cells: dark where the two nearest points are about as near (the cell borders), shaded
/// towards the cells' edges, with grit.
fn crackle(f1: f32, f2: f32, grit: f32) -> f32 {
    let crack = 1.0 - smoothstep(0.0, 0.12, f2 - f1);
    crack.max(0.35 * (f1 * 0.6).clamp(0.0, 1.0) + 0.25 * grit).clamp(0.0, 1.0)
}

/// Waves at `u`: `cells` stripes across the tile, rippled by `warp`.
fn waves(u: f32, cells: i32, detail: u32, warp: f32) -> f32 {
    let t = cells as f32 * u + detail as f32 / 4.0 * (warp - 0.5) * (cells as f32).sqrt();
    0.5 + 0.5 * (std::f32::consts::TAU * t).sin()
}

/// The tone of each dot of a `cells` × `cells` halftone screen: the amount, varied by smooth
/// noise as much as the detail asks.
fn halftone_tones(cells: i32, d: f32, detail: u32, seed: u32) -> Vec<f32> {
    let variation = (detail as f32 - 1.0) / 7.0;
    let period = ((cells / 6).max(1), (cells / 6).max(1));
    fbm(cells.max(1) as usize, period, 3, cells, seed)
        .into_iter()
        .map(|noise| (d * (1.0 + variation * 4.0 * (noise - 0.5))).clamp(0.0, 1.0))
        .collect()
}

/// How much of the point `(x, y)` (in dot widths) the halftone dots cover, anti-aliased over
/// `px` (a particle, in dot widths). Large dots reach into the next cells, so the neighbouring
/// cells' dots count too.
fn halftone(tones: &[f32], cells: i32, x: f32, y: f32, px: f32) -> f32 {
    let c = cells.max(1);
    let (cx, cy) = (x.floor() as i32, y.floor() as i32);
    let mut cover: f32 = 0.0;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (gx, gy) = (cx + dx, cy + dy);
            let tone = tones.get((gy.rem_euclid(c) * c + gx.rem_euclid(c)) as usize).copied().unwrap_or(0.0);
            if tone <= 0.0 {
                continue;
            }
            // The radius for that area: at full tone the dots overlap a little.
            let r = (tone / std::f32::consts::PI).sqrt() * 1.05;
            let dist = ((x - (gx as f32 + 0.5)).powi(2) + (y - (gy as f32 + 0.5)).powi(2)).sqrt();
            cover = cover.max(((r - dist) / px.max(1e-6) + 0.5).clamp(0.0, 1.0));
        }
    }
    cover
}

/// How much of each pixel of a particle's square the particle covers, row by row.
fn particle_mask(shape: Shape, size: usize) -> Vec<f32> {
    if shape == Shape::Square || size < 2 {
        return vec![1.0; size * size];
    }
    let r = size as f32 / 2.0;
    let mut m = Vec::with_capacity(size * size);
    for y in 0..size {
        for x in 0..size {
            // 4×4 samples per pixel.
            let mut hits = 0u8;
            for sy in 0..4 {
                for sx in 0..4 {
                    let (px, py) = (x as f32 + (sx as f32 + 0.5) / 4.0 - r, y as f32 + (sy as f32 + 0.5) / 4.0 - r);
                    if px * px + py * py <= r * r {
                        hits += 1;
                    }
                }
            }
            m.push(f32::from(hits) / 16.0);
        }
    }
    m
}

/// Hues in random-colour mode.
const HUES: u32 = 36;

/// `x` (`0..1`) as one of 256 levels.
fn level(x: f32) -> usize {
    (x.clamp(0.0, 1.0) * 255.0 + 0.5) as usize
}

/// Colour the particles and lay them over the background: the tile's pixels.
fn paint(s: &Settings, p: &Particles, n: usize, size: usize) -> Vec<u32> {
    let side = n * size;
    let opacity = (s.opacity.clamp(0.0, 100.0) / 100.0) as f32;
    let (c1, c2) = (rgb(s.color), rgb(s.color2));
    let (a1, a2) = (f32::from(s.color[3]) / 255.0, f32::from(s.color2[3]) / 255.0);
    let bg = rgb(s.background);
    let bg_a = f32::from(s.background[3]) / 255.0 * (s.background_opacity.clamp(0.0, 100.0) / 100.0) as f32;
    let empty = over([0.0; 3], 0.0, bg, bg_a);
    // Each mode's colours, quantised to 256 levels (255 tones in two-colour mode, so that with the
    // gaps they still fit a PNG palette), worked out once.
    let single: Vec<u32> = (0..256).map(|l| over(c1, l as f32 / 255.0, bg, bg_a)).collect();
    let mix = |t: f32| [c2[0] + (c1[0] - c2[0]) * t, c2[1] + (c1[1] - c2[1]) * t, c2[2] + (c1[2] - c2[2]) * t];
    let duo_at = |t: f32, m: f32| over(mix(t), opacity * (a2 + (a1 - a2) * t) * m, bg, bg_a);
    let duo: Vec<u32> = (0..255).map(|l| duo_at(l as f32 / 254.0, 1.0)).collect();
    let hues: Vec<[f32; 3]> = (0..HUES).map(|h| hsv(h as f32 / HUES as f32, 0.85, 1.0)).collect();
    let hue_seed = channel(s.seed, 1);
    // Random-colour mode's colours by hue and level, worked out as they're first needed.
    let mut multi: Vec<Option<u32>> = vec![None; HUES as usize * 256];

    // The colour of particle `k` covering `m` of a pixel.
    let mut colour = |k: usize, m: f32| -> u32 {
        match s.color_mode {
            ColorMode::Single => single[level(p.cover[k] * a1 * opacity * m)],
            ColorMode::Duo if !p.present[k] || m <= 0.0 => empty,
            ColorMode::Duo => {
                let t = (p.tone[k].clamp(0.0, 1.0) * 254.0 + 0.5) as usize;
                if m >= 1.0 { duo[t.min(254)] } else { duo_at(t as f32 / 254.0, m) }
            }
            ColorMode::Multi => {
                let h = (hash((k % n) as i32, (k / n) as i32, hue_seed) % HUES) as usize;
                let l = level(p.cover[k] * opacity * m);
                *multi[h * 256 + l].get_or_insert_with(|| over(hues[h], l as f32 / 255.0, bg, bg_a))
            }
        }
    };

    if size == 1 {
        return (0..n * n).map(|k| colour(k, 1.0)).collect();
    }
    let mask = particle_mask(s.shape, size);
    let square = s.shape == Shape::Square;
    let mut pixels = vec![0u32; side * side];
    for k in 0..n * n {
        let (px, py) = (k % n, k / n);
        let flat = if square { Some(colour(k, 1.0)) } else { None };
        for sy in 0..size {
            let row = (py * size + sy) * side + px * size;
            for (sx, out) in pixels[row..row + size].iter_mut().enumerate() {
                *out = flat.unwrap_or_else(|| colour(k, mask[sy * size + sx]));
            }
        }
    }
    pixels
}

fn rgb(c: Rgba) -> [f32; 3] {
    [f32::from(c[0]) / 255.0, f32::from(c[1]) / 255.0, f32::from(c[2]) / 255.0]
}

/// HSV (each `0..1`) to RGB.
fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = (h - h.floor()) * 6.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as u32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// Colour `top` at opacity `a` over `bottom` at opacity `b`: a straight-alpha RGBA pixel.
fn over(top: [f32; 3], a: f32, bottom: [f32; 3], b: f32) -> u32 {
    let (a, b) = (a.clamp(0.0, 1.0), b.clamp(0.0, 1.0));
    let out = a + b * (1.0 - a);
    if out <= 0.0 {
        return 0;
    }
    let byte = |x: f32| u32::from((x * 255.0 + 0.5).clamp(0.0, 255.0) as u8);
    let mix = |i: usize| (top[i] * a + bottom[i] * b * (1.0 - a)) / out;
    byte(mix(0)) | (byte(mix(1)) << 8) | (byte(mix(2)) << 16) | (byte(out) << 24)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(s: &Settings) -> Tile {
        let t = render(s);
        assert_eq!(t.pixels.len(), (t.width * t.height) as usize);
        t
    }

    fn pixel(t: &Tile, x: u32, y: u32) -> [u8; 4] {
        t.pixels[(y * t.width + x) as usize].to_le_bytes()
    }

    fn rgba(t: &Tile) -> Vec<[u8; 4]> {
        t.pixels.iter().map(|p| p.to_le_bytes()).collect()
    }

    #[test]
    fn settings_parse_leniently_and_round_trip() {
        let s = Settings::from_json(&json!({
            "type": "MARBLE", "size": "3", "scale": 9999, "density": -5, "detail": 2.6, "shape": "circle",
            "colorMode": "Two colours", "color": "#f0a", "color2": "11223344", "opacity": "35%", "blend": "soft light",
            "tile": 100, "seed": 12.2, "background": "nope"
        }));
        assert_eq!(s.kind, Kind::Marble);
        assert_eq!((s.size, s.scale, s.density, s.detail), (3, 512, 0.0, 3));
        assert_eq!((s.shape, s.color_mode), (Shape::Circle, ColorMode::Duo));
        assert_eq!((s.color, s.color2), ([255, 0, 170, 255], [0x11, 0x22, 0x33, 0x44]));
        assert_eq!((s.opacity, s.blend_id(), s.tile, s.seed), (35.0, "softLight", 100, 12));
        assert_eq!(s.background, Settings::default().background);
        assert_eq!(Settings::from_json(&s.to_json()), s);
        assert_eq!(Settings::from_json(&json!(null)), Settings::default());
        assert_eq!(Settings::from_json(&json!({"size": f64::NAN, "type": 3, "blend": "glow"})), Settings::default());
    }

    #[test]
    fn names_parse_by_id_or_label() {
        for k in Kind::ALL {
            assert_eq!(Kind::parse(k.id()), Some(k));
            assert_eq!(Kind::parse(&k.label().to_uppercase()), Some(k));
        }
        for m in ColorMode::ALL {
            assert_eq!(ColorMode::parse(m.label()), Some(m));
        }
        for (i, (label, id)) in BLENDS.iter().enumerate() {
            assert_eq!((parse_blend(label), parse_blend(id)), (Some(i), Some(i)));
        }
        assert_eq!(parse_blend("Color-Dodge"), Some(6));
        assert!(Kind::parse("plasma").is_none() && parse_hex("#12345").is_none() && parse_hex("ééé").is_none());
    }

    #[test]
    fn tiles_are_whole_particles() {
        let s = Settings { tile: 100, size: 3, ..Settings::default() };
        assert_eq!(s.tile_px(), 102);
        assert_eq!(Settings { tile: 5, size: 64, ..Settings::default() }.tile_px(), 64);
        let t = tile(&Settings { tile: 20, size: 3, ..Settings::default() });
        assert_eq!((t.width, t.height), (21, 21));
    }

    #[test]
    fn static_follows_the_amount_and_the_seed() {
        let s = Settings { density: 30.0, opacity: 100.0, ..Settings::default() };
        let t = tile(&s);
        let on = rgba(&t).iter().filter(|p| p[3] > 0).count() as f64 / f64::from(t.width * t.height);
        assert!((on - 0.3).abs() < 0.02, "{on}");
        assert!(rgba(&t).iter().all(|p| *p == [0, 0, 0, 255] || *p == [0, 0, 0, 0]));
        assert_eq!(render(&s), t, "the same settings make the same tile");
        assert_ne!(render(&s.shuffled()), t);
        assert!(render(&Settings { density: 0.0, ..s.clone() }).pixels.iter().all(|p| *p == 0));
    }

    #[test]
    fn particles_are_size_pixels_square() {
        let t = tile(&Settings { size: 4, tile: 64, density: 50.0, ..Settings::default() });
        for y in 0..64 {
            for x in 0..64 {
                assert_eq!(pixel(&t, x, y), pixel(&t, x - x % 4, y - y % 4));
            }
        }
        let c = tile(&Settings { size: 8, tile: 64, density: 100.0, opacity: 100.0, shape: Shape::Circle, ..Settings::default() });
        assert_eq!(pixel(&c, 0, 0)[3], 0, "a circle leaves its square's corners");
        assert_eq!(pixel(&c, 4, 4)[3], 255);
    }

    #[test]
    fn background_and_colour_modes() {
        let base = Settings { density: 0.0, background: [10, 20, 30, 255], background_opacity: 100.0, ..Settings::default() };
        assert!(rgba(&tile(&base)).iter().all(|p| *p == [10, 20, 30, 255]));
        let duo = rgba(&tile(&Settings { color_mode: ColorMode::Duo, density: 100.0, opacity: 100.0, ..Settings::default() }));
        let black = duo.iter().filter(|p| **p == [0, 0, 0, 255]).count();
        let white = duo.iter().filter(|p| **p == [255, 255, 255, 255]).count();
        assert_eq!(black + white, duo.len());
        assert!(black > 1000 && white > 1000);
        let multi = rgba(&tile(&Settings { color_mode: ColorMode::Multi, density: 100.0, opacity: 100.0, ..Settings::default() }));
        let reds = multi.iter().filter(|p| p[0] > 200 && p[1] < 100 && p[2] < 100).count();
        assert!(reds > 100, "{reds}");
    }

    #[test]
    fn textures_fit_a_palette_when_they_can() {
        // One colour at 256 opacities, or 255 tones with gaps: a PNG palette holds them.
        for s in [
            Settings { kind: Kind::Clouds, density: 100.0, opacity: 100.0, ..Settings::default() },
            Settings { kind: Kind::Grain, density: 60.0, color_mode: ColorMode::Duo, ..Settings::default() },
            Settings { kind: Kind::Marble, color_mode: ColorMode::Duo, background_opacity: 100.0, ..Settings::default() },
            Settings { kind: Kind::Static, color_mode: ColorMode::Multi, ..Settings::default() },
        ] {
            let mut colours = tile(&s).pixels;
            colours.sort_unstable();
            colours.dedup();
            assert!(colours.len() <= 256, "{:?}: {}", s.kind, colours.len());
        }
    }

    #[test]
    fn every_kind_tiles_seamlessly() {
        // The fields are functions of the tile coordinates that repeat every tile, whatever the
        // number of feature cells, odd or even (the noise they read repeats too: see `noise`).
        // Static and grain have no correlation between particles to break.
        let close = |a: f32, b: f32| (a - b).abs() < 2e-3;
        for cells in [1, 2, 3, 5, 8, 33] {
            for i in 0..50 {
                let (u, v) = (i as f32 / 50.0, (i * 17 % 50) as f32 / 50.0);
                let (t, w) = (i as f32 / 50.0, 1.0 - i as f32 / 50.0);
                assert!(close(marble(u, v, cells, t, w), marble(u + 1.0, v, cells, t, w)));
                assert!(close(marble(u, v, cells, t, w), marble(u, v - 1.0, cells, t, w)));
                assert!(close(wood(v, cells, 4, t, w), wood(v + 1.0, cells, 4, t, w)));
                assert!(close(waves(u, cells, 6, t), waves(u - 1.0, cells, 6, t)));
            }
            let tones: Vec<f32> = (0..cells * cells).map(|k| (k % 7) as f32 / 6.0).collect();
            for i in 0..80 {
                let (x, y) = (i as f32 * 0.37 % cells as f32, i as f32 * 0.73 % cells as f32);
                let h = halftone(&tones, cells, x, y, 0.05);
                assert!(close(h, halftone(&tones, cells, x + cells as f32, y - cells as f32, 0.05)), "halftone wraps");
            }
        }
        for kind in Kind::ALL {
            let t = tile(&Settings { kind, tile: 64, scale: 16, density: 100.0, opacity: 100.0, color_mode: ColorMode::Duo, ..Settings::default() });
            assert!(t.pixels.iter().any(|p| *p != t.pixels[0]), "{} isn't flat", kind.id());
            assert!(field(kind, 32, 4, 4, 9).iter().all(|f| (0.0..=1.0).contains(f)), "{} in range", kind.id());
        }
    }

    #[test]
    fn colour_helpers() {
        assert_eq!(hsv(0.0, 1.0, 1.0), [1.0, 0.0, 0.0]);
        assert_eq!(hsv(1.0 / 3.0, 1.0, 1.0)[1], 1.0);
        assert_eq!(over([1.0, 0.0, 0.0], 0.0, [0.0, 0.0, 0.0], 0.0), 0);
        assert_eq!(over([1.0, 0.0, 0.0], 0.5, [0.0, 0.0, 1.0], 1.0).to_le_bytes(), [128, 0, 128, 255]);
        assert_eq!((level(-1.0), level(0.5), level(2.0)), (0, 128, 255));
        assert_eq!((coverage(0.7, 1.0), coverage(0.7, 0.0), coverage(0.75, 0.5)), (0.7, 0.0, 0.5));
    }
}
