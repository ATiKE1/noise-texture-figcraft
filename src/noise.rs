//! Seamless noise, sampled on grids: lattice hashing, gradient noise, fractal sums and cellular
//! noise.
//!
//! Every function here repeats with the period it's given (whole lattice cells across the tile),
//! so a tile built from it wraps around without a seam. A grid holds `n` × `n` samples taken at the
//! centres of the cells of an `n` × `n` division of the tile, row by row; sampling whole grids lets
//! the work shared by neighbouring samples (lattice gradients, positions along each axis) be done
//! once, which matters in FigCraft's metered interpreter.

use std::f32::consts::{FRAC_1_SQRT_2, SQRT_2};

/// A well-mixed 32-bit hash of a lattice point and a seed (the "lowbias32" finaliser).
pub fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = seed ^ (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^ (h >> 16)
}

/// `h` as a number in `0..1`.
pub fn unit(h: u32) -> f32 {
    (h >> 8) as f32 / 16_777_216.0
}

/// A random number in `0..1` for lattice point `(x, y)`, the same every time.
pub fn rand(x: i32, y: i32, seed: u32) -> f32 {
    unit(hash(x, y, seed))
}

/// The seed of channel `k` derived from `seed`, so different random streams don't correlate.
pub fn channel(seed: u32, k: u32) -> u32 {
    hash(k as i32, 0x5eed, seed ^ 0x9e37_79b9)
}

/// Sixteen unit gradients, evenly spaced around the circle.
const GRADIENTS: [(f32, f32); 16] = [
    (1.0, 0.0),
    (0.923_879_5, 0.382_683_43),
    (FRAC_1_SQRT_2, FRAC_1_SQRT_2),
    (0.382_683_43, 0.923_879_5),
    (0.0, 1.0),
    (-0.382_683_43, 0.923_879_5),
    (-FRAC_1_SQRT_2, FRAC_1_SQRT_2),
    (-0.923_879_5, 0.382_683_43),
    (-1.0, 0.0),
    (-0.923_879_5, -0.382_683_43),
    (-FRAC_1_SQRT_2, -FRAC_1_SQRT_2),
    (-0.382_683_43, -0.923_879_5),
    (0.0, -1.0),
    (0.382_683_43, -0.923_879_5),
    (FRAC_1_SQRT_2, -FRAC_1_SQRT_2),
    (0.923_879_5, -0.382_683_43),
];

/// The gradient at lattice point `(x, y)` (already wrapped into the period).
pub fn gradient(x: i32, y: i32, seed: u32) -> (f32, f32) {
    GRADIENTS[(hash(x, y, seed) >> 28) as usize]
}

/// Quintic smoothstep: continuous first and second derivatives at the lattice lines.
pub fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Where each of `n` samples across the tile falls on a lattice of `period` cells: the lattice
/// line before it (wrapped), the offset from it, and its fade.
fn axis(n: usize, period: i32) -> Vec<(usize, f32, f32)> {
    let period = period.max(1);
    (0..n)
        .map(|i| {
            let x = (i as f64 + 0.5) / n as f64 * f64::from(period);
            let x0 = x.floor();
            let f = (x - x0) as f32;
            ((x0 as i32).rem_euclid(period) as usize, f, fade(f))
        })
        .collect()
}

/// The gradients of lattice row `y` in pairs: at `x`, the gradients at `x` and at the next
/// lattice point (wrapped), as `[gx, gy, next gx, next gy]`.
fn gradient_row(row: &mut Vec<[f32; 4]>, period: i32, y: usize, seed: u32) {
    let first = gradient(0, y as i32, seed);
    let mut g = first;
    row.clear();
    row.extend((0..period).map(|x| {
        let next = if x + 1 == period { first } else { gradient(x + 1, y as i32, seed) };
        let pair = [g.0, g.1, next.0, next.1];
        g = next;
        pair
    }));
}

/// Add one layer of gradient noise (about `-1..1`, `|noise|` when `fold`), times `amp`, to the
/// `n` × `n` grid `out`; the lattice has `period` cells across the tile on each axis.
fn add_octave(out: &mut [f32], n: usize, period: (i32, i32), seed: u32, amp: f32, fold: bool) {
    let (px, py) = (period.0.max(1), period.1.max(1));
    let xs = axis(n, px);
    let ys = axis(n, py);
    // The gradients of the two lattice rows around the current sample row, worked out again only
    // when the samples move on to the next lattice row.
    let (mut r0, mut r1) = (Vec::with_capacity(px as usize), Vec::with_capacity(px as usize));
    let mut at = None;
    for (row, &(y0, fy, v)) in out.chunks_exact_mut(n).zip(&ys) {
        if at != Some(y0) {
            let y1 = (y0 + 1) % py as usize;
            if at.is_some_and(|prev| (prev + 1) % py as usize == y0) {
                std::mem::swap(&mut r0, &mut r1);
            } else {
                gradient_row(&mut r0, px, y0, seed);
            }
            gradient_row(&mut r1, px, y1, seed);
            at = Some(y0);
        }
        let fy1 = fy - 1.0;
        for (o, &(x0, fx, u)) in row.iter_mut().zip(&xs) {
            let ([a0, a1, b0, b1], [c0, c1, d0, d1]) = (r0[x0], r1[x0]);
            let fx1 = fx - 1.0;
            let n00 = a0 * fx + a1 * fy;
            let n10 = b0 * fx1 + b1 * fy;
            let n01 = c0 * fx + c1 * fy1;
            let n11 = d0 * fx1 + d1 * fy1;
            let a = n00 + (n10 - n00) * u;
            let b = n01 + (n11 - n01) * u;
            let value = (a + (b - a) * v) * SQRT_2;
            *o += amp * if fold { value.abs() } else { value };
        }
    }
}

/// `octaves` layers of gradient noise, each twice as fine and half as strong as the one before,
/// normalised and stretched (sums of noise rarely reach the ends of their range). The coarsest
/// layer has `cells` lattice cells across the tile; layers finer than `limit` cells are left out.
fn octaves(n: usize, cells: (i32, i32), count: u32, limit: i32, seed: u32, fold: bool) -> Vec<f32> {
    let mut out = vec![0.0f32; n * n];
    let (mut amp, mut norm) = (1.0f32, 0.0f32);
    let (mut cx, mut cy) = (cells.0.max(1), cells.1.max(1));
    for o in 0..count.max(1) {
        if o > 0 && cx.max(cy) > limit.max(1) {
            break;
        }
        add_octave(&mut out, n, (cx, cy), channel(seed, o), amp, fold);
        norm += amp;
        amp *= 0.5;
        cx = cx.saturating_mul(2);
        cy = cy.saturating_mul(2);
    }
    let k = if fold { 1.6 } else { 1.4 } / norm;
    for x in &mut out {
        *x = if fold { (*x * k).clamp(0.0, 1.0) } else { 0.5 + 0.5 * (*x * k).clamp(-1.0, 1.0) };
    }
    out
}

/// Fractal noise in `0..1` on an `n` × `n` grid (see [`octaves`]).
pub fn fbm(n: usize, cells: (i32, i32), count: u32, limit: i32, seed: u32) -> Vec<f32> {
    octaves(n, cells, count, limit, seed, false)
}

/// Turbulence in `0..1`: like [`fbm`] with each layer folded (`|noise|`), for veins and creases.
pub fn turbulence(n: usize, cells: (i32, i32), count: u32, limit: i32, seed: u32) -> Vec<f32> {
    octaves(n, cells, count, limit, seed, true)
}

/// The scattered point of cell `(x, y)` of a cellular grid, inside the cell (in cell widths).
pub fn feature_point(x: i32, y: i32, seed: u32) -> (f32, f32) {
    (0.1 + 0.8 * rand(x, y, seed), 0.1 + 0.8 * rand(x, y, channel(seed, 77)))
}

/// Cellular noise on an `n` × `n` grid: the distances (in cell widths) from each sample to the
/// nearest and the second nearest of a scattering of points, one per cell of a `cells` × `cells`
/// grid over the tile.
pub fn worley(n: usize, cells: i32, seed: u32) -> (Vec<f32>, Vec<f32>) {
    let c = cells.max(1);
    let points: Vec<(f32, f32)> = (0..c * c).map(|k| feature_point(k % c, k / c, seed)).collect();
    let pos: Vec<(f32, i32)> = (0..n)
        .map(|i| {
            let x = ((i as f64 + 0.5) / n as f64 * f64::from(c)) as f32;
            (x, x.floor() as i32)
        })
        .collect();
    let (mut f1s, mut f2s) = (Vec::with_capacity(n * n), Vec::with_capacity(n * n));
    for &(y, cy) in &pos {
        for &(x, cx) in &pos {
            let (mut f1, mut f2) = (f32::MAX, f32::MAX);
            for dy in -1..=1 {
                let gy = cy + dy;
                let row = (gy.rem_euclid(c) * c) as usize;
                for dx in -1..=1 {
                    let gx = cx + dx;
                    let p = points[row + gx.rem_euclid(c) as usize];
                    let (ex, ey) = (gx as f32 + p.0 - x, gy as f32 + p.1 - y);
                    let d = ex * ex + ey * ey;
                    if d < f1 {
                        f2 = f1;
                        f1 = d;
                    } else if d < f2 {
                        f2 = d;
                    }
                }
            }
            f1s.push(f1.sqrt());
            f2s.push(f2.sqrt());
        }
    }
    (f1s, f2s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gradient noise at one point, the plain way: the reference the grids are checked against.
    fn perlin(x: f64, y: f64, period: (i32, i32), seed: u32) -> f64 {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = ((x - x0) as f32, (y - y0) as f32);
        let (ix, iy) = (x0 as i32, y0 as i32);
        let dot = |dx: i32, dy: i32| {
            let g = gradient((ix + dx).rem_euclid(period.0), (iy + dy).rem_euclid(period.1), seed);
            g.0 * (fx - dx as f32) + g.1 * (fy - dy as f32)
        };
        let (u, v) = (fade(fx), fade(fy));
        let a = dot(0, 0) + (dot(1, 0) - dot(0, 0)) * u;
        let b = dot(0, 1) + (dot(1, 1) - dot(0, 1)) * u;
        f64::from((a + (b - a) * v) * SQRT_2)
    }

    fn close(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn hash_is_stable_and_spread() {
        assert_eq!(hash(3, 4, 5), hash(3, 4, 5));
        assert_ne!(hash(3, 4, 5), hash(4, 3, 5));
        assert_ne!(hash(3, 4, 5), hash(3, 4, 6));
        let mean = (0..10_000).map(|i| f64::from(rand(i, i / 7, 1))).sum::<f64>() / 10_000.0;
        assert!((mean - 0.5).abs() < 0.02, "{mean}");
        assert!((0..1000).all(|i| (0.0..1.0).contains(&rand(i, -i, 9))));
    }

    #[test]
    fn gradient_noise_repeats_with_its_period_and_stays_in_range() {
        for i in 0..200 {
            let (x, y) = (f64::from(i) * 0.137, f64::from(i) * 0.291);
            let n = perlin(x, y, (5, 3), 42);
            assert!(close(n, perlin(x + 5.0, y, (5, 3), 42), 1e-6));
            assert!(close(n, perlin(x, y + 3.0, (5, 3), 42), 1e-6));
            assert!((-1.0..=1.0).contains(&n), "{n}");
        }
        assert!(close(perlin(2.0, 1.0, (5, 3), 42), 0.0, 1e-9));
    }

    #[test]
    fn grids_sample_the_noise_at_the_cell_centres() {
        for (n, period) in [(16, (4, 4)), (24, (5, 3)), (7, (9, 2)), (40, (1, 1))] {
            let mut grid = vec![0.0; n * n];
            add_octave(&mut grid, n, period, 11, 1.0, false);
            for j in 0..n {
                for i in 0..n {
                    let (u, v) = ((i as f64 + 0.5) / n as f64, (j as f64 + 0.5) / n as f64);
                    let want = perlin(u * f64::from(period.0), v * f64::from(period.1), period, 11);
                    assert!(close(f64::from(grid[j * n + i]), want, 1e-4), "{n} {period:?} at {i},{j}");
                }
            }
        }
    }

    #[test]
    fn fractal_grids_are_in_range_and_vary() {
        for (cells, count) in [((3, 3), 5), ((1, 8), 2), ((2, 2), 8)] {
            let f = fbm(64, cells, count, 64, 7);
            let t = turbulence(64, cells, count, 64, 7);
            assert!(f.iter().chain(&t).all(|x| (0.0..=1.0).contains(x)));
            let spread = |g: &[f32]| g.iter().copied().fold(f32::MIN, f32::max) - g.iter().copied().fold(f32::MAX, f32::min);
            assert!(spread(&f) > 0.4 && spread(&t) > 0.4, "{cells:?}");
        }
        // Layers finer than the limit are left out.
        assert_eq!(fbm(8, (8, 8), 6, 8, 3), fbm(8, (8, 8), 1, 8, 3));
    }

    #[test]
    fn cellular_noise_finds_the_nearest_points() {
        let (n, c) = (30, 5);
        let (f1, f2) = worley(n, c, 3);
        for j in 0..n {
            for i in 0..n {
                let (x, y) = ((i as f32 + 0.5) / n as f32 * c as f32, (j as f32 + 0.5) / n as f32 * c as f32);
                // Every point of the tile and its neighbours, by brute force.
                let mut d: Vec<f32> = Vec::new();
                for gy in -c..2 * c {
                    for gx in -c..2 * c {
                        let p = feature_point(gx.rem_euclid(c), gy.rem_euclid(c), 3);
                        d.push(((gx as f32 + p.0 - x).powi(2) + (gy as f32 + p.1 - y).powi(2)).sqrt());
                    }
                }
                d.sort_by(f32::total_cmp);
                let k = j * n + i;
                assert!((f1[k] - d[0]).abs() < 1e-4 && f1[k] <= f2[k], "{i},{j}");
                assert!((f2[k] - d[1]).abs() < 0.35, "{i},{j}: {} vs {}", f2[k], d[1]);
            }
        }
    }
}
