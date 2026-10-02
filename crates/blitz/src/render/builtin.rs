//! Box drawing, block elements, braille and powerline glyphs, drawn
//! procedurally so they join up exactly at any cell size.

const LIGHT: u8 = 1;
const HEAVY: u8 = 2;
const DOUBLE: u8 = 3;

/// Whether `c` is drawn here instead of taken from a font.
pub fn is_builtin(c: char) -> bool {
    matches!(c, '\u{2500}'..='\u{259F}' | '\u{2800}'..='\u{28FF}' | '\u{E0B0}'..='\u{E0BF}')
}

/// Coverage mask (`w * h` bytes, row-major) for a builtin glyph one cell
/// wide, or `None` when `c` is not builtin.
pub fn draw(c: char, w: usize, h: usize) -> Option<Vec<u8>> {
    if !is_builtin(c) || w == 0 || h == 0 {
        return None;
    }
    let mut cv = Canvas {
        w,
        h,
        a: vec![0; w * h],
    };
    let cp = c as u32;
    match cp {
        0x2500..=0x257F => box_drawing(&mut cv, cp),
        0x2580..=0x259F => block(&mut cv, cp),
        0x2800..=0x28FF => braille(&mut cv, cp),
        _ => powerline(&mut cv, cp),
    }
    Some(cv.a)
}

struct Canvas {
    w: usize,
    h: usize,
    a: Vec<u8>,
}

impl Canvas {
    /// Light line thickness.
    fn t(&self) -> i32 {
        (self.w as i32 + 4) / 8
    }

    fn rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, v: u8) {
        let (w, h) = (self.w as i32, self.h as i32);
        for y in y0.clamp(0, h)..y1.clamp(0, h) {
            for x in x0.clamp(0, w)..x1.clamp(0, w) {
                self.a[(y * w + x) as usize] = v;
            }
        }
    }

    /// Fills by 4×4 supersampling `inside(x, y)` in pixel coordinates.
    fn shape(&mut self, inside: impl Fn(f32, f32) -> bool) {
        for y in 0..self.h {
            for x in 0..self.w {
                let mut n = 0u32;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                        let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                        n += u32::from(inside(px, py));
                    }
                }
                let v = &mut self.a[y * self.w + x];
                *v = (*v).max((n * 255 / 16) as u8);
            }
        }
    }
}

/// Arms of U+2500..U+257F as (up, right, down, left), each 0, LIGHT, HEAVY
/// or DOUBLE. Dashed lines use their solid line's arms; arcs and diagonals
/// are zero here and drawn separately.
#[rustfmt::skip]
const ARMS: [[u8; 4]; 128] = {
    const fn a(u: u8, r: u8, d: u8, l: u8) -> [u8; 4] {
        [u, r, d, l]
    }
    const HL: [u8; 4] = a(0, 1, 0, 1);
    const HH: [u8; 4] = a(0, 2, 0, 2);
    const VL: [u8; 4] = a(1, 0, 1, 0);
    const VH: [u8; 4] = a(2, 0, 2, 0);
    const Z: [u8; 4] = a(0, 0, 0, 0);
    [
        // 2500
        HL, HH, VL, VH, HL, HH, VL, VH, HL, HH, VL, VH,
        a(0, 1, 1, 0), a(0, 2, 1, 0), a(0, 1, 2, 0), a(0, 2, 2, 0),
        // 2510
        a(0, 0, 1, 1), a(0, 0, 1, 2), a(0, 0, 2, 1), a(0, 0, 2, 2),
        a(1, 1, 0, 0), a(1, 2, 0, 0), a(2, 1, 0, 0), a(2, 2, 0, 0),
        a(1, 0, 0, 1), a(1, 0, 0, 2), a(2, 0, 0, 1), a(2, 0, 0, 2),
        a(1, 1, 1, 0), a(1, 2, 1, 0), a(2, 1, 1, 0), a(1, 1, 2, 0),
        // 2520
        a(2, 1, 2, 0), a(2, 2, 1, 0), a(1, 2, 2, 0), a(2, 2, 2, 0),
        a(1, 0, 1, 1), a(1, 0, 1, 2), a(2, 0, 1, 1), a(1, 0, 2, 1),
        a(2, 0, 2, 1), a(2, 0, 1, 2), a(1, 0, 2, 2), a(2, 0, 2, 2),
        a(0, 1, 1, 1), a(0, 1, 1, 2), a(0, 2, 1, 1), a(0, 2, 1, 2),
        // 2530
        a(0, 1, 2, 1), a(0, 1, 2, 2), a(0, 2, 2, 1), a(0, 2, 2, 2),
        a(1, 1, 0, 1), a(1, 1, 0, 2), a(1, 2, 0, 1), a(1, 2, 0, 2),
        a(2, 1, 0, 1), a(2, 1, 0, 2), a(2, 2, 0, 1), a(2, 2, 0, 2),
        a(1, 1, 1, 1), a(1, 1, 1, 2), a(1, 2, 1, 1), a(1, 2, 1, 2),
        // 2540
        a(2, 1, 1, 1), a(1, 1, 2, 1), a(2, 1, 2, 1), a(2, 1, 1, 2),
        a(2, 2, 1, 1), a(1, 1, 2, 2), a(1, 2, 2, 1), a(2, 2, 1, 2),
        a(1, 2, 2, 2), a(2, 1, 2, 2), a(2, 2, 2, 1), a(2, 2, 2, 2),
        HL, HH, VL, VH,
        // 2550
        a(0, 3, 0, 3), a(3, 0, 3, 0), a(0, 3, 1, 0), a(0, 1, 3, 0),
        a(0, 3, 3, 0), a(0, 0, 1, 3), a(0, 0, 3, 1), a(0, 0, 3, 3),
        a(1, 3, 0, 0), a(3, 1, 0, 0), a(3, 3, 0, 0), a(1, 0, 0, 3),
        a(3, 0, 0, 1), a(3, 0, 0, 3), a(1, 3, 1, 0), a(3, 1, 3, 0),
        // 2560
        a(3, 3, 3, 0), a(1, 0, 1, 3), a(3, 0, 3, 1), a(3, 0, 3, 3),
        a(0, 3, 1, 3), a(0, 1, 3, 1), a(0, 3, 3, 3), a(1, 3, 0, 3),
        a(3, 1, 0, 1), a(3, 3, 0, 3), a(1, 3, 1, 3), a(3, 1, 3, 1),
        a(3, 3, 3, 3), Z, Z, Z,
        // 2570
        Z, Z, Z, Z,
        a(0, 0, 0, 1), a(1, 0, 0, 0), a(0, 1, 0, 0), a(0, 0, 1, 0),
        a(0, 0, 0, 2), a(2, 0, 0, 0), a(0, 2, 0, 0), a(0, 0, 2, 0),
        a(0, 2, 0, 1), a(1, 0, 2, 0), a(0, 1, 0, 2), a(2, 0, 1, 0),
    ]
};

fn box_drawing(cv: &mut Canvas, cp: u32) {
    let dashes = match cp {
        0x2504..=0x2507 => 3,
        0x2508..=0x250B => 4,
        0x254C..=0x254F => 2,
        _ => 0,
    };
    match cp {
        0x256D..=0x2570 => arc(cv, cp),
        0x2571..=0x2573 => diagonal(cv, cp),
        _ if dashes > 0 => dashed(cv, ARMS[(cp - 0x2500) as usize], dashes),
        _ => lines(cv, ARMS[(cp - 0x2500) as usize]),
    }
}

/// Start and end of a stroke band of `style` centred in `extent` pixels.
/// For DOUBLE this spans both strokes and the gap between them.
fn band(style: u8, extent: i32, t: i32) -> (i32, i32) {
    let n = match style {
        HEAVY => 2,
        DOUBLE => 3,
        _ => 1,
    };
    let lo = (extent - n * t) / 2;
    (lo, lo + n * t)
}

fn lines(cv: &mut Canvas, [up, right, down, left]: [u8; 4]) {
    let (w, h, t) = (cv.w as i32, cv.h as i32, cv.t());
    // Horizontal arms run along x; the vertical arms are their perpendiculars.
    let mut horiz = |x0, y0, x1, y1| cv.rect(x0, y0, x1, y1, 255);
    axis(w, h, t, [left, right], [up, down], &mut horiz);
    let mut vert = |y0, x0, y1, x1| cv.rect(x0, y0, x1, y1, 255);
    axis(h, w, t, [up, down], [left, right], &mut vert);
}

/// Draws the two arms lying along one axis: `arms` = [towards 0, towards
/// `along`], `perp` = the perpendicular arms on the low and high side.
/// `rect` takes (along0, across0, along1, across1).
fn axis(
    along: i32,
    across: i32,
    t: i32,
    arms: [u8; 2],
    perp: [u8; 2],
    rect: &mut dyn FnMut(i32, i32, i32, i32),
) {
    let bands: Vec<(i32, i32)> = perp
        .iter()
        .filter(|&&s| s != 0)
        .map(|&s| band(s, along, t))
        .collect();
    let light = band(LIGHT, along, t);
    // Where an arm stops so it covers the perpendicular strokes it meets.
    let lo = bands.iter().map(|b| b.0).min().unwrap_or(light.0);
    let hi = bands.iter().map(|b| b.1).max().unwrap_or(light.1);
    let d = band(DOUBLE, along, t).0;
    for (i, &style) in arms.iter().enumerate() {
        if style == 0 {
            continue;
        }
        let strokes = if style == DOUBLE {
            let c = band(DOUBLE, across, t).0;
            vec![((c, c + t), perp[0]), ((c + 2 * t, c + 3 * t), perp[1])]
        } else {
            vec![(band(style, across, t), 0)]
        };
        for ((c0, c1), side) in strokes {
            // A double stroke next to a double perpendicular arm stops at
            // that arm's inner stroke, leaving the corner open.
            if i == 0 {
                let end = if side == DOUBLE { d + t } else { hi };
                rect(0, c0, end, c1);
            } else {
                let start = if side == DOUBLE { d + 2 * t } else { lo };
                rect(start, c0, along, c1);
            }
        }
    }
}

fn dashed(cv: &mut Canvas, arms: [u8; 4], n: i32) {
    let (w, h, t) = (cv.w as i32, cv.h as i32, cv.t());
    let vertical = arms[0] != 0;
    let (along, across) = if vertical { (h, w) } else { (w, h) };
    let (c0, c1) = band(arms[0].max(arms[1]), across, t);
    let gap = (along / n / 3).max(1);
    for k in 0..n {
        let a0 = k * along / n + gap / 2;
        let a1 = (k + 1) * along / n - (gap - gap / 2);
        if vertical {
            cv.rect(c0, a0, c1, a1, 255);
        } else {
            cv.rect(a0, c0, a1, c1, 255);
        }
    }
}

/// Rounded corners: a quarter circle joining the centre lines, with
/// straight runs to the cell edges.
fn arc(cv: &mut Canvas, cp: u32) {
    let (w, h, t) = (cv.w as i32, cv.h as i32, cv.t());
    let tf = t as f32;
    let cx = band(LIGHT, w, t).0 as f32 + tf / 2.0;
    let cy = band(LIGHT, h, t).0 as f32 + tf / 2.0;
    // (sx, sy): which way the arc opens, towards the right/left and down/up.
    let (sx, sy) = match cp {
        0x256D => (1.0, 1.0),
        0x256E => (-1.0, 1.0),
        0x256F => (-1.0, -1.0),
        _ => (1.0, -1.0),
    };
    let r = cx.min(w as f32 - cx).min(cy).min(h as f32 - cy);
    let (ox, oy) = (cx + sx * r, cy + sy * r);
    cv.shape(|x, y| {
        let vertical = (x - cx).abs() <= tf / 2.0 && (y - oy) * sy >= 0.0;
        let horizontal = (y - cy).abs() <= tf / 2.0 && (x - ox) * sx >= 0.0;
        let quadrant = (x - ox) * sx <= 0.0 && (y - oy) * sy <= 0.0;
        let ring = ((x - ox).hypot(y - oy) - r).abs() <= tf / 2.0;
        vertical || horizontal || (quadrant && ring)
    });
}

fn diagonal(cv: &mut Canvas, cp: u32) {
    let (w, h) = (cv.w as f32, cv.h as f32);
    let half = cv.t() as f32 * 0.6;
    let rising = |x: f32, y: f32| seg_dist(x, y, (0.0, h), (w, 0.0)) <= half;
    let falling = |x: f32, y: f32| seg_dist(x, y, (0.0, 0.0), (w, h)) <= half;
    match cp {
        0x2571 => cv.shape(rising),
        0x2572 => cv.shape(falling),
        _ => cv.shape(|x, y| rising(x, y) || falling(x, y)),
    }
}

fn seg_dist(x: f32, y: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let k = (((x - a.0) * dx + (y - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (x - a.0 - k * dx).hypot(y - a.1 - k * dy)
}

fn block(cv: &mut Canvas, cp: u32) {
    let (w, h) = (cv.w as i32, cv.h as i32);
    let (mx, my) = ((w + 1) / 2, (h + 1) / 2);
    let eighth_h = |k: i32| (h * k + 4) / 8;
    let eighth_w = |k: i32| (w * k + 4) / 8;
    // Quadrants: upper-left, upper-right, lower-left, lower-right.
    let quads = |cv: &mut Canvas, q: [bool; 4]| {
        let cells = [
            (0, 0, mx, my),
            (mx, 0, w, my),
            (0, my, mx, h),
            (mx, my, w, h),
        ];
        for (on, (x0, y0, x1, y1)) in q.into_iter().zip(cells) {
            if on {
                cv.rect(x0, y0, x1, y1, 255);
            }
        }
    };
    match cp {
        0x2580 => cv.rect(0, 0, w, my, 255),
        0x2581..=0x2588 => cv.rect(0, h - eighth_h(cp as i32 - 0x2580), w, h, 255),
        0x2589..=0x258F => cv.rect(0, 0, eighth_w(0x2590 - cp as i32), h, 255),
        0x2590 => cv.rect(mx, 0, w, h, 255),
        0x2591..=0x2593 => cv.rect(0, 0, w, h, (cp as i32 - 0x2590) as u8 * 64),
        0x2594 => cv.rect(0, 0, w, eighth_h(1), 255),
        0x2595 => cv.rect(w - eighth_w(1), 0, w, h, 255),
        0x2596 => quads(cv, [false, false, true, false]),
        0x2597 => quads(cv, [false, false, false, true]),
        0x2598 => quads(cv, [true, false, false, false]),
        0x2599 => quads(cv, [true, false, true, true]),
        0x259A => quads(cv, [true, false, false, true]),
        0x259B => quads(cv, [true, true, true, false]),
        0x259C => quads(cv, [true, true, false, true]),
        0x259D => quads(cv, [false, true, false, false]),
        0x259E => quads(cv, [false, true, true, false]),
        _ => quads(cv, [false, true, true, true]),
    }
}

/// Eight dots in a 2×4 grid; bit order per the Unicode braille block.
fn braille(cv: &mut Canvas, cp: u32) {
    let (w, h) = (cv.w as i32, cv.h as i32);
    let bits = cp - 0x2800;
    let d = (w / 4).max(1);
    // (column, row) of dots 1..8.
    let dots = [
        (0, 0),
        (0, 1),
        (0, 2),
        (1, 0),
        (1, 1),
        (1, 2),
        (0, 3),
        (1, 3),
    ];
    for (i, (col, row)) in dots.into_iter().enumerate() {
        if bits & (1 << i) == 0 {
            continue;
        }
        let x = col * w / 2 + (w / 2 - d) / 2;
        let y = row * h / 4 + (h / 4 - d) / 2;
        cv.rect(x, y, x + d, y + d, 255);
    }
}

fn powerline(cv: &mut Canvas, cp: u32) {
    let (w, h) = (cv.w as f32, cv.h as f32);
    let half = cv.t() as f32 * 0.6;
    let my = h / 2.0;
    let thin = |a, b| move |x, y| seg_dist(x, y, a, b) <= half;
    match cp {
        // Solid and thin arrows.
        0xE0B0 => cv.shape(|x, y| x <= w * (1.0 - (y / my - 1.0).abs())),
        0xE0B1 => cv.shape(|x, y| thin((0.0, 0.0), (w, my))(x, y) || thin((w, my), (0.0, h))(x, y)),
        0xE0B2 => cv.shape(|x, y| x >= w * (y / my - 1.0).abs()),
        0xE0B3 => cv.shape(|x, y| thin((w, 0.0), (0.0, my))(x, y) || thin((0.0, my), (w, h))(x, y)),
        // Half circles.
        0xE0B4..=0xE0B7 => {
            let cx = if cp <= 0xE0B5 { 0.0 } else { w };
            let solid = cp.is_multiple_of(2);
            let edge = half / w.min(my);
            cv.shape(|x, y| {
                let r = ((x - cx) / w).hypot((y - my) / my);
                if solid {
                    r <= 1.0
                } else {
                    (r - 1.0 + edge).abs() <= edge
                }
            });
        }
        // Corner triangles and their diagonals.
        0xE0B8 => cv.shape(|x, y| x / w <= y / h),
        0xE0BA => cv.shape(|x, y| x / w >= 1.0 - y / h),
        0xE0BC => cv.shape(|x, y| x / w <= 1.0 - y / h),
        0xE0BE => cv.shape(|x, y| x / w >= y / h),
        0xE0B9 | 0xE0BF => cv.shape(thin((0.0, 0.0), (w, h))),
        _ => cv.shape(thin((w, 0.0), (0.0, h))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders as rows of '#' (>= 128) and '.'.
    fn ascii(c: char, w: usize, h: usize) -> Vec<String> {
        let a = draw(c, w, h).expect("builtin");
        a.chunks(w)
            .map(|r| {
                r.iter()
                    .map(|&v| if v >= 128 { '#' } else { '.' })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn straight_lines_join_across_cells() {
        // 9×19 cell, 1 px light lines: column 4 and row 9.
        let v = ascii('│', 9, 19);
        assert!(v.iter().all(|r| r == "....#...."));
        let hz = ascii('─', 9, 19);
        assert_eq!(hz[9], "#########");
        assert!(
            hz.iter()
                .enumerate()
                .all(|(i, r)| i == 9 || r == ".........")
        );
        let cross = ascii('┼', 9, 19);
        assert_eq!(cross[9], "#########");
        assert_eq!(cross[0], "....#....");
        assert_eq!(cross[18], "....#....");
    }

    #[test]
    fn corners_meet_the_lines_they_join() {
        let tl = ascii('┌', 9, 19);
        assert_eq!(tl[9], "....#####");
        assert_eq!(tl[8], ".........");
        assert_eq!(tl[18], "....#....");
        // Rounded corners still leave the cell on the centre lines.
        for c in ['╭', '╮', '╯', '╰'] {
            let g = ascii(c, 9, 19);
            let down = matches!(c, '╭' | '╮');
            let edge_row = if down { &g[18] } else { &g[0] };
            assert_eq!(edge_row, "....#....", "{c}");
            let right = matches!(c, '╭' | '╰');
            let col = if right { 8 } else { 0 };
            assert_eq!(&g[9][col..=col], "#", "{c}");
        }
    }

    #[test]
    fn double_corner_is_open_inside() {
        // 15×21 cell, t = 2: strokes at 4..6 and 8..10 across.
        let g = ascii('╔', 15, 21);
        assert_eq!(g[20], "....##..##.....");
        assert_eq!(g[7], "....###########");
        assert_eq!(g[11], "....##..#######");
    }

    #[test]
    fn heavy_lines_are_twice_as_thick() {
        let g = ascii('━', 9, 19);
        assert_eq!(g.iter().filter(|r| r.as_str() == "#########").count(), 2);
        let g = ascii('┃', 16, 20);
        assert_eq!(g[0], "......####......");
    }

    #[test]
    fn blocks_and_quadrants() {
        assert!(ascii('█', 8, 16).iter().all(|r| r == "########"));
        let half = ascii('▐', 8, 16);
        assert!(half.iter().all(|r| r == "....####"));
        let q = ascii('▛', 8, 16);
        assert_eq!(q[0], "########");
        assert_eq!(q[15], "####....");
        let q = ascii('▜', 8, 16);
        assert_eq!(q[15], "....####");
        let low = ascii('▄', 8, 16);
        assert_eq!(low[7], "........");
        assert_eq!(low[8], "########");
    }

    #[test]
    fn braille_dots() {
        let all = ascii('\u{28FF}', 8, 16);
        assert_eq!(all.iter().filter(|r| r.contains('#')).count(), 8);
        let one = ascii('\u{2801}', 8, 16);
        assert_eq!(one[1], ".##.....");
        assert_eq!(one.iter().filter(|r| r.contains('#')).count(), 2);
    }

    #[test]
    fn powerline_and_others() {
        let tri = ascii('\u{E0B0}', 8, 16);
        assert!(tri[8].starts_with("#######"));
        assert_eq!(tri[0], "#.......");
        assert!(draw('a', 8, 16).is_none());
        assert!(draw('─', 0, 16).is_none());
        // Every builtin produces some ink at a small size.
        for c in ('\u{2500}'..='\u{259F}').chain(['\u{E0B0}', '\u{E0B4}', '\u{E0BF}']) {
            let a = draw(c, 7, 15).expect("builtin");
            assert!(a.iter().any(|&v| v > 0), "{c}");
        }
    }
}
