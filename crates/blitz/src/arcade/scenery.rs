//! Pixel scenery drawn behind the panes' text: faint enough to read over,
//! in colours mixed from the theme's background.
//!
//! A scene is a pure function of its inputs. Where each star, cloud or
//! flake starts comes from a hash of its index; time moves it on, wrapping
//! around the window so the motion loops.

use std::f64::consts::TAU;

use vt::Palette;

use super::sprite::{self, shade};
use crate::layout::Rect;
use crate::render::chrome::Prim;

/// The scenery settings, as `config.toml` names them; the first is none.
pub const SCENES: &[&str] = &["off", "stars", "hills", "snow"];

/// Lays out scene `name` from [`SCENES`] at `t` seconds over `area`, the
/// whole window. `scale` is the DPI scale, 1.0 at 96 DPI. Draws nothing
/// for `off` or a name it does not know.
pub fn draw(p: &mut Vec<Prim>, name: &str, area: Rect, t: f32, scale: f32, pal: &Palette) {
    if area.w <= 0 || area.h <= 0 {
        return;
    }
    // Pixels grow on very large windows, keeping the cost bounded.
    let big = (f64::from(area.w) * f64::from(area.h) / MAX_CELLS)
        .sqrt()
        .ceil() as i32;
    let px = ((3.0 * scale).round() as i32).max(big).max(1);
    let g = Grid {
        area,
        px,
        cols: (area.w + px - 1) / px,
        rows: (area.h + px - 1) / px,
        rate: f64::from(scale) / f64::from(px),
    };
    let t = f64::from(t);
    let mut out = Vec::new();
    match name {
        "stars" => stars(&mut out, &g, t, pal),
        "hills" => hills(&mut out, &g, t, pal),
        "snow" => snow(&mut out, &g, t, pal),
        _ => return,
    }
    p.extend(out.into_iter().filter_map(|prim| clip(prim, area)));
}

/// The most scenery pixels a window is cut into.
const MAX_CELLS: f64 = 400_000.0;

/// The window as a grid of square scenery pixels, `px` device pixels a
/// side.
struct Grid {
    area: Rect,
    px: i32,
    cols: i32,
    rows: i32,
    /// Cells per second for each pixel per second of speed at 96 DPI.
    rate: f64,
}

impl Grid {
    /// Pushes the `w` by `h` block of cells at column `x`, row `y`.
    fn cell(&self, out: &mut Vec<Prim>, x: i32, y: i32, w: i32, h: i32, rgb: u32) {
        let r = Rect {
            x: self.area.x + x * self.px,
            y: self.area.y + y * self.px,
            w: w * self.px,
            h: h * self.px,
        };
        out.push(Prim::Rect(r, rgb));
    }

    /// How many things fit at one per `per` cells, at most `max`.
    fn count(&self, per: i64, max: i64) -> i64 {
        (i64::from(self.cols) * i64::from(self.rows) / per + 1).min(max)
    }

    /// One column rect per run of equal `tops`, from the top row down.
    fn columns(&self, out: &mut Vec<Prim>, tops: &[i32], rgb: u32) {
        let mut c0 = 0;
        while c0 < tops.len() {
            let y = tops[c0];
            let c1 = tops[c0..].iter().position(|&t| t != y);
            let c1 = c1.map_or(tops.len(), |n| c0 + n);
            self.cell(out, c0 as i32, y, (c1 - c0) as i32, self.rows - y, rgb);
            c0 = c1;
        }
    }
}

/// `prim` cut to `area`, or none when nothing of it is left.
fn clip(prim: Prim, area: Rect) -> Option<Prim> {
    let Prim::Rect(r, rgb) = prim else {
        return Some(prim);
    };
    let (x, y) = (r.x.max(area.x), r.y.max(area.y));
    let (w, h) = (
        r.right().min(area.right()) - x,
        r.bottom().min(area.bottom()) - y,
    );
    (w > 0 && h > 0).then_some(Prim::Rect(Rect { x, y, w, h }, rgb))
}

/// splitmix64: a well-mixed number from any seed.
fn mix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Number `k` in [0, 1) of thing `i` in `group`.
fn unit(group: u64, i: i64, k: u64) -> f64 {
    (mix((group << 48) ^ ((i as u64) << 8) ^ k) >> 11) as f64 / (1u64 << 53) as f64
}

/// `start` moved by `by` and wrapped into `0..span`, as a whole cell.
fn wrap(start: f64, by: f64, span: i32) -> i32 {
    (start + by).rem_euclid(f64::from(span)) as i32
}

/// Where thing `i` of `n` starts across `span` cells: in its own slice,
/// moved `jitter` of the way along, so things never bunch up.
fn spread(i: i64, n: i64, jitter: f64, span: i32) -> f64 {
    (i as f64 + jitter) / n as f64 * f64::from(span)
}

/// A sine wave over grid column `x`, `len` columns long.
fn wave(x: i64, len: f64, phase: f64) -> f64 {
    (TAU * (x as f64 / len + phase)).sin()
}

/// `c` pushed further from grey, so a faint mix of it still shows its
/// colour.
fn vivid(c: u32) -> u32 {
    let ch = |s: u32| (c >> s & 0xff) as i32;
    let mean = (ch(16) + ch(8) + ch(0)) / 3;
    [16, 8, 0].into_iter().fold(0, |rgb, s| {
        rgb | ((2 * ch(s) - mean).clamp(0, 255) as u32) << s
    })
}

/// A row of round hills `len` columns wide over grid column `x`: 1 at
/// each top and 0 where two meet.
fn dome(x: i64, len: f64, phase: f64) -> f64 {
    let u = (x as f64 / len + phase).rem_euclid(1.0) * 2.0 - 1.0;
    (1.0 - u * u).sqrt()
}

/// Star layers, far to near: cells of sky per star, drift in px/s and
/// how far the colour is from the background.
const STARS: [(i64, f64, f32); 3] = [(2000, 2.0, 0.11), (4500, 4.0, 0.15), (11000, 7.0, 0.18)];

fn stars(out: &mut Vec<Prim>, g: &Grid, t: f64, pal: &Palette) {
    let tints = [
        pal.fg,
        pal.fg,
        pal.fg,
        pal.ansi[3],
        pal.ansi[6],
        pal.ansi[5],
    ];
    for (l, &(per, speed, amt)) in STARS.iter().enumerate() {
        let near = l == STARS.len() - 1;
        let span = g.cols + 3;
        let n = g.count(per, 400);
        for i in 0..n {
            let r = |k| unit(l as u64, i, k);
            let x = wrap(spread(i, n, r(0), span), -speed * g.rate * t, span) - 1;
            let y = (r(1) * f64::from(g.rows)) as i32;
            let tint = tints[(r(2) * tints.len() as f64) as usize];
            // A third of them dim for part of a cycle of 2 to 5 s.
            let lit = r(3) > 0.33 || (TAU * (t / (2.0 + 3.0 * r(4)) + r(5))).sin() > -0.2;
            let rgb = shade(pal.bg, tint, if lit { amt } else { amt * 0.45 });
            if near && lit {
                g.cell(out, x - 1, y, 3, 1, rgb);
                g.cell(out, x, y - 1, 1, 3, rgb);
            } else {
                g.cell(out, x, y, 1, 1, rgb);
            }
        }
    }

    // A shooting star for one second in every 25.
    let (n, u) = ((t / 25.0).floor() as i64, t.rem_euclid(25.0) - 18.0);
    if !(0.0..1.0).contains(&u) {
        return;
    }
    let r = |k| unit(3, n, k);
    let x0 = ((0.45 + 0.5 * r(0)) * f64::from(g.cols)) as i32;
    let y0 = ((0.04 + 0.3 * r(1)) * f64::from(g.rows)) as i32;
    let head = (u * 40.0) as i32;
    let fade = ((1.0 - u) / 0.3).min(1.0) as f32;
    for j in 0..12.min(head + 1) {
        let s = head - j;
        let amt = 0.18 * fade * (1.0 - j as f32 / 12.0);
        g.cell(out, x0 - 2 * s, y0 + s, 2, 1, shade(pal.bg, pal.fg, amt));
    }
}

/// Columns after which the hills repeat: every wavelength divides it.
const HILL_LOOP: i64 = 1920;

/// The pixel clouds, `o` the body and `s` the shaded underside.
const CLOUDS: [&[&str]; 3] = [
    &[
        ".....oooo.......",
        "...oooooooo.oo..",
        ".oooooooooooooo.",
        "oooooooooooooooo",
        ".ssssssssssssss.",
    ],
    &[
        "........ooo...........",
        "..ooo..ooooooo..oo....",
        ".oooooooooooooooooooo.",
        "oooooooooooooooooooooo",
        "oooooooooooooooooooooo",
        ".ssssssssssssssssssss.",
        "...sssss.....sssss....",
    ],
    &[
        "....ooo.....",
        "..ooooooo...",
        ".oooooooooo.",
        "ssssssssssss",
    ],
];

fn hills(out: &mut Vec<Prim>, g: &Grid, t: f64, pal: &Palette) {
    let body = shade(pal.bg, pal.fg, 0.07);
    let under = shade(pal.bg, shade(pal.fg, pal.ansi[4], 0.5), 0.05);
    for i in 0..i64::from(g.cols / 140 + 2) {
        let r = |k| unit(4, i, k);
        let art = CLOUDS[(r(0) * CLOUDS.len() as f64) as usize];
        // Big clouds are nearer, so faster.
        let size = if r(4) < 0.4 { 3 } else { 2 };
        let speed = f64::from(size) * (1.0 + r(2));
        let w = art[0].len() as i32 * size;
        let span = g.cols + w;
        let x = wrap(r(1) * f64::from(span), -speed * g.rate * t, span) - w;
        let y = ((0.04 + 0.34 * r(3)) * f64::from(g.rows)) as i32;
        let (x, y) = (g.area.x + x * g.px, g.area.y + y * g.px);
        sprite::draw(out, art, x, y, size * g.px, |c| match c {
            'o' => Some(body),
            's' => Some(under),
            _ => None,
        });
    }

    // Far to near: drift in px/s, colour toward which the body and its
    // rim are mixed, and height as a share of the window at column `x`.
    type Height = fn(i64) -> f64;
    let green = vivid(pal.ansi[2]);
    let blue = vivid(shade(pal.ansi[4], pal.ansi[6], 0.3));
    let teal = vivid(shade(pal.ansi[6], pal.ansi[2], 0.5));
    let layers: [(f64, u32, f32, f32, Height); 3] = [
        (2.0, blue, 0.065, 0.065, |x| {
            0.17 + 0.06 * wave(x, 480.0, 0.1) + 0.03 * wave(x, 160.0, 0.6)
        }),
        (4.5, teal, 0.10, 0.15, |x| {
            0.07 + 0.11 * dome(x, 160.0, 0.0).max(0.75 * dome(x, 96.0, 0.3))
        }),
        (9.0, green, 0.13, 0.17, |x| {
            0.055 + 0.012 * wave(x, 320.0, 0.7) + 0.006 * wave(x, 128.0, 0.2)
        }),
    ];
    let rows = f64::from(g.rows);
    for (l, (speed, tint, fill, rim, height)) in layers.into_iter().enumerate() {
        let off = ((speed * g.rate * t) as i64).rem_euclid(HILL_LOOP);
        let tops: Vec<i32> = (0..g.cols)
            .map(|c| g.rows - (height(off + i64::from(c)) * rows).round() as i32)
            .collect();
        g.columns(out, &tops, shade(pal.bg, tint, rim));
        if rim != fill {
            // The body one pixel in from every edge, leaving the rim.
            let inner: Vec<i32> = (0..tops.len())
                .map(|c| {
                    tops[c.saturating_sub(1)..(c + 2).min(tops.len())]
                        .iter()
                        .max()
                })
                .map(|top| top.map_or(g.rows, |&top| top + 1))
                .collect();
            g.columns(out, &inner, shade(pal.bg, tint, fill));
        }
        match l {
            1 => trees(out, g, &tops, off, pal),
            2 => meadow(out, g, &tops, off, pal),
            _ => {}
        }
    }
}

/// A round tree, `o` its leaves and `t` its trunk.
const TREE: [&str; 6] = [".ooo.", "ooooo", "ooooo", ".ooo.", "..t..", "..t.."];

/// Trees here and there high on the hills whose top rows are `tops`,
/// scrolled `off` columns.
fn trees(out: &mut Vec<Prim>, g: &Grid, tops: &[i32], off: i64, pal: &Palette) {
    let leaves = shade(pal.bg, vivid(pal.ansi[2]), 0.16);
    let trunk = shade(pal.bg, vivid(pal.ansi[3]), 0.11);
    let high = g.rows - (0.12 * f64::from(g.rows)) as i32;
    for (c, &s) in tops.iter().enumerate() {
        // One spot in 7 columns, so trees never overlap, and 1 in 5 spots.
        let w = off + c as i64;
        if s > high || w % 7 != 0 || !mix((9 << 48) ^ w as u64).is_multiple_of(5) {
            continue;
        }
        let (x, y) = (c as i32 - 2, s - 5);
        let (x, y) = (g.area.x + x * g.px, g.area.y + y * g.px);
        sprite::draw(out, &TREE, x, y, g.px, |c| match c {
            'o' => Some(leaves),
            't' => Some(trunk),
            _ => None,
        });
    }
}

/// Grass and flowers on the flat parts of the hill whose top rows are
/// `tops`, scrolled `off` columns.
fn meadow(out: &mut Vec<Prim>, g: &Grid, tops: &[i32], off: i64, pal: &Palette) {
    let grass = shade(pal.bg, vivid(pal.ansi[2]), 0.17);
    let petals = [pal.ansi[1], pal.ansi[3], pal.ansi[5], pal.fg];
    for c in 1..tops.len().saturating_sub(1) {
        let s = tops[c];
        if tops[c - 1] != s || tops[c + 1] != s {
            continue;
        }
        let h = mix((10 << 48) ^ (off + c as i64) as u64);
        let c = c as i32;
        match h % 23 {
            0 | 1 => {
                g.cell(out, c - 1, s - 1, 3, 1, grass);
                g.cell(out, c - 1, s - 2, 1, 1, grass);
                g.cell(out, c + 1, s - 2, 1, 1, grass);
            }
            2 => {
                let petal = vivid(petals[(h >> 32) as usize % petals.len()]);
                g.cell(out, c, s - 1, 1, 1, grass);
                g.cell(out, c, s - 2, 1, 1, shade(pal.bg, petal, 0.18));
            }
            _ => {}
        }
    }
}

/// Snow layers, far to near: cells of sky per flake, fall in px/s, flake
/// size in cells, colour amount and sway in cells.
const SNOW: [(i64, f64, i32, f32, f64); 3] = [
    (600, 5.0, 1, 0.09, 1.5),
    (1300, 9.0, 1, 0.13, 2.5),
    (4000, 14.0, 2, 0.17, 4.0),
];

fn snow(out: &mut Vec<Prim>, g: &Grid, t: f64, pal: &Palette) {
    let tint = shade(pal.fg, pal.ansi[12], 0.25);
    let tops: Vec<i32> = (0..i64::from(g.cols))
        .map(|x| 0.03 + 0.012 * wave(x, 160.0, 0.2) + 0.008 * wave(x, 56.0, 0.5))
        .map(|h| g.rows - (h * f64::from(g.rows)).round() as i32)
        .collect();
    g.columns(out, &tops, shade(pal.bg, tint, 0.08));

    for (l, &(per, fall, size, amt, sway)) in SNOW.iter().enumerate() {
        let rgb = shade(pal.bg, tint, amt);
        // Room for a flake to sway out of sight at either side.
        let (wide, tall) = (g.cols + size + 8, g.rows + size);
        let n = g.count(per, 500);
        for i in 0..n {
            let r = |k| unit(6 + l as u64, i, k);
            let y = wrap(r(0) * f64::from(tall), fall * g.rate * t, tall) - size;
            // Sway, and drift left on a light wind.
            let by = sway * (TAU * (t / (5.0 + 4.0 * r(2)) + r(3))).sin() - 0.2 * fall * g.rate * t;
            let x = wrap(spread(i, n, r(1), wide), by, wide) - size - 4;
            g.cell(out, x, y, size, size, rgb);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::blitz;

    fn area(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    fn scene(name: &str, a: Rect, t: f32, scale: f32, pal: &Palette) -> Vec<Prim> {
        let mut p = Vec::new();
        draw(&mut p, name, a, t, scale, pal);
        p
    }

    #[test]
    fn off_and_unknown_scenes_draw_nothing() {
        let pal = blitz(false).pal;
        for name in ["off", "", "rain"] {
            assert!(scene(name, area(0, 0, 800, 600), 3.0, 1.0, &pal).is_empty());
        }
    }

    #[test]
    fn every_scene_draws_and_is_the_same_each_time() {
        let pal = blitz(false).pal;
        for name in &SCENES[1..] {
            let a = scene(name, area(0, 0, 1280, 800), 12.5, 1.25, &pal);
            assert!(!a.is_empty(), "{name}");
            assert_eq!(a, scene(name, area(0, 0, 1280, 800), 12.5, 1.25, &pal));
        }
    }

    #[test]
    fn every_scene_stays_inside_its_area() {
        let pal = blitz(true).pal;
        let areas = [
            area(0, 0, 1920, 1080),
            area(37, 11, 801, 599),
            area(-20, 5, 300, 40),
            area(4, 4, 1, 1),
            area(0, 0, 3, 700),
            area(0, 0, 0, 500),
        ];
        for name in &SCENES[1..] {
            for a in areas {
                for t in [0.0, 1.7, 18.4, 18.95, 333.3, 86_400.0] {
                    for scale in [1.0, 1.5, 2.25] {
                        for prim in scene(name, a, t, scale, &pal) {
                            let Prim::Rect(r, _) = prim else {
                                panic!("{name}: not a rect: {prim:?}");
                            };
                            let inside = r.w > 0
                                && r.h > 0
                                && r.x >= a.x
                                && r.y >= a.y
                                && r.right() <= a.right()
                                && r.bottom() <= a.bottom();
                            assert!(inside, "{name} at {t}: {r:?} outside {a:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn colours_stay_close_to_the_background() {
        for light in [false, true] {
            let pal = blitz(light).pal;
            for name in &SCENES[1..] {
                for t in [0.0, 18.3, 40.0] {
                    for prim in scene(name, area(0, 0, 1600, 900), t, 1.0, &pal) {
                        let Prim::Rect(_, rgb) = prim else { continue };
                        let far = [16, 8, 0]
                            .into_iter()
                            .map(|s| (rgb >> s & 0xff).abs_diff(pal.bg >> s & 0xff))
                            .max();
                        // A mix of at most 0.18 toward any colour.
                        assert!(far <= Some(46), "{name}: {rgb:06x} on {:06x}", pal.bg);
                    }
                }
            }
        }
    }

    #[test]
    fn every_scene_moves() {
        let pal = blitz(false).pal;
        for name in &SCENES[1..] {
            let a = area(0, 0, 1280, 800);
            assert_ne!(
                scene(name, a, 0.0, 1.0, &pal),
                scene(name, a, 10.0, 1.0, &pal),
                "{name}"
            );
        }
    }

    #[test]
    fn big_windows_stay_under_two_thousand_prims() {
        let pal = blitz(false).pal;
        for name in &SCENES[1..] {
            for (w, h) in [(3840, 2160), (7680, 1080)] {
                for t in [0.0, 18.5, 1234.5] {
                    for scale in [1.0, 1.5, 2.0] {
                        let n = scene(name, area(0, 0, w, h), t, scale, &pal).len();
                        assert!(n < 2000, "{name} at {w}x{h}, {scale}: {n} prims");
                    }
                }
            }
        }
    }
}
