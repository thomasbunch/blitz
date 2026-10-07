//! The spark at the foot of the sidebar, doing what the sessions do.

use super::sprite::{self, SPARK_H, SPARK_W, shade};
use crate::attention::Attn;
use crate::layout::Rect;
use crate::render::chrome::Prim;
use crate::theme::Ui;

/// Lays out the spark at `t` seconds in `area`, the free part of the
/// expanded sidebar under the sessions, for `state`, the most urgent
/// session state. Draws nothing when `area` is too small to hold it.
/// `(tw, th)` is the sidebar font's cell size.
///
/// Idle, it sleeps; working, it runs to and fro; when a session needs
/// you, it hops under a "!"; when one is done, it bounces among
/// sparkles; after an error, it sits grey and smoking.
pub fn draw(
    p: &mut Vec<Prim>,
    area: Rect,
    state: Attn,
    t: f64,
    scale: f32,
    ui: &Ui,
    (tw, th): (i32, i32),
) {
    let s = |v: f32| (v * scale).round() as i32;
    let px = s(2.0).max(2);
    let (sw, sh) = (SPARK_W * px, SPARK_H * px);
    // A margin all round, room beside the spark for z's and sparkles, and
    // room over it for a hop and the bubble.
    let (m, side) = (s(10.0), (2 * tw).max(4 * px));
    let (hop, bh) = (4 * px, th + s(4.0));
    let sky = hop + 3 * px + bh;
    if area.w < sw + 2 * (m + side) || area.h < sh + sky + 2 * m {
        return;
    }
    let floor = area.bottom() - m;
    let sky_top = floor - sh - sky;
    let (lo, hi) = (area.x + m + side, area.right() - m - side - sw);
    let home = (lo + hi) / 2;
    // The run: a triangle wave from `lo` to `hi`, starting in the middle.
    let span = (hi - lo) as f32;
    let (fspan, speed) = (f64::from(span), f64::from(s(48.0)));
    let run = ((t * speed + fspan / 2.0) % (2.0 * fspan).max(1.0)) as f32;
    // Every other motion repeats within an hour; wrapping keeps f32 exact
    // however long blitz stays open.
    let t = (t % 3600.0) as f32;
    // A hop `h` high, `u` of the way through it; on the ground after.
    let arc = |u: f32, h: i32| (4.0 * u * (1.0 - u).max(0.0) * h as f32).round() as i32;

    // Where it is, how high off the ground, and its pose: eyes open,
    // mid-stride, or sitting with its feet tucked under.
    let (x, lift, open, stride, sit) = match state {
        Attn::Idle => (home, 0, false, false, t % 2.0 < 1.0),
        Attn::Working => {
            // Feet apart on the ground, together a pixel up.
            let x = lo + (span - (run - span).abs()) as i32;
            let stride = (t * 7.0) as i32 % 2 == 0;
            (x, if stride { 0 } else { px }, true, stride, false)
        }
        Attn::NeedsYou => (home, arc(t % 0.8 / 0.35, hop), true, false, false),
        Attn::DoneUnseen => {
            let u = t % 3.0 / 0.3;
            let u = if u < 2.0 { u.fract() } else { 1.0 };
            (home, arc(u, 3 * px), true, false, false)
        }
        Attn::Error => (home, 0, false, false, true),
    };
    let body = match state {
        Attn::Error => shade(ui.accent, ui.dim, 0.7),
        _ => ui.accent,
    };
    let art = sprite::spark(open, stride);
    let rows = if sit { &art[..art.len() - 1] } else { &art[..] };
    let top = floor - rows.len() as i32 * px - lift;

    let square = |p: &mut Vec<Prim>, x, y, color| {
        p.push(Prim::Rect(Rect { x, y, w: px, h: px }, color));
    };
    let text = |p: &mut Vec<Prim>, x, y, t: &str, color, bold| {
        p.push(Prim::Text {
            x,
            y,
            text: t.to_string(),
            color,
            bold,
            term: false,
        });
    };

    // A shadow that shrinks as it leaves the ground.
    let w = 9 * px - lift;
    p.push(Prim::Shape {
        r: Rect {
            x: x + (sw - w) / 2,
            y: floor - px / 2,
            w,
            h: px,
        },
        radius: px as f32 / 2.0,
        stroke: 0.0,
        color: ui.rule,
    });
    sprite::draw(p, rows, x, top, px, sprite::spark_colors(body));
    if state == Attn::DoneUnseen {
        // A grin.
        let r = Rect {
            x: x + 6 * px,
            y: top + 10 * px,
            w: 2 * px,
            h: px,
        };
        p.push(Prim::Rect(r, shade(body, 0x000000, 0.55)));
    }

    match state {
        Attn::Idle => {
            // Two z's rising from beside its head, growing and fading.
            let (x0, y0) = (x + sw - 2 * px, floor - sh + 4 * px - th);
            for i in 0..2 {
                let u = (t / 3.0 + i as f32 / 2.0).fract();
                let y = y0 - ((y0 - sky_top) as f32 * u) as i32;
                let fade = shade(ui.dim, ui.side_bg, ((u - 0.6) / 0.4).max(0.0));
                let z = if u < 0.5 { "z" } else { "Z" };
                text(p, x0 + (u * tw as f32) as i32, y, z, fade, false);
            }
        }
        Attn::Working => {
            // Dust kicked up behind it, `n` pixels back.
            let back = |n: i32| {
                if run < span {
                    x - n * px
                } else {
                    x + sw + (n - 1) * px
                }
            };
            let dust = shade(ui.rule, ui.dim, 0.5);
            let thin = shade(dust, ui.side_bg, 0.5);
            if stride {
                square(p, back(1), floor - px, dust);
                square(p, back(3), floor - 2 * px, thin);
            } else {
                square(p, back(4), floor - 2 * px, thin);
            }
        }
        Attn::NeedsYou => {
            // A "!" bubble, its tail pointing at the bolt.
            let (bw, bx) = (tw + s(10.0), x + 6 * px);
            let by = top - 3 * px - bh;
            p.push(Prim::Shape {
                r: Rect {
                    x: bx,
                    y: by,
                    w: bw,
                    h: bh,
                },
                radius: 4.0 * scale,
                stroke: 0.0,
                color: ui.accent,
            });
            square(p, bx + px, by + bh, ui.accent);
            square(p, bx + 2 * px, by + bh, ui.accent);
            square(p, bx + px, by + bh + px, ui.accent);
            text(
                p,
                bx + (bw - tw) / 2,
                by + (bh - th) / 2,
                "!",
                ui.chip_fg,
                true,
            );
        }
        Attn::DoneUnseen => {
            // Sparkles twinkling around it, each a cross at its brightest.
            let star = shade(ui.accent, ui.name, 0.3);
            let spots = [(-2, 4), (14, 2), (-1, 10), (13, 11)];
            for (i, (ax, ay)) in spots.into_iter().enumerate() {
                let v = (t * 1.5 + i as f32 / 4.0).fract();
                let (cx, cy) = (x + ax * px, top + ay * px);
                if v < 0.5 {
                    square(p, cx, cy, star);
                }
                if v < 0.25 {
                    let arm = shade(star, ui.side_bg, 0.4);
                    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                        square(p, cx + dx * px, cy + dy * px, arm);
                    }
                }
            }
        }
        Attn::Error => {
            // Smoke puffs rising off the bolt, growing and thinning.
            for i in 0..3 {
                let u = (t / 2.0 + i as f32 / 3.0).fract();
                let d = 2 * px + (2.0 * px as f32 * u) as i32;
                let wobble = ((u * 9.0).sin() * px as f32) as i32;
                let y0 = top - px - d;
                let y = y0 - ((y0 - sky_top) as f32 * u) as i32;
                p.push(Prim::Shape {
                    r: Rect {
                        x: x + 7 * px - d / 2 + wobble,
                        y,
                        w: d,
                        h: d,
                    },
                    radius: d as f32 / 2.0,
                    stroke: 0.0,
                    color: shade(ui.dim, ui.side_bg, 0.2 + 0.6 * u),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATES: [Attn; 5] = [
        Attn::Idle,
        Attn::Working,
        Attn::DoneUnseen,
        Attn::Error,
        Attn::NeedsYou,
    ];
    const CELL: (i32, i32) = (7, 15);

    fn area() -> Rect {
        Rect {
            x: 0,
            y: 400,
            w: 239,
            h: 160,
        }
    }

    fn spark(area: Rect, state: Attn, t: f64, scale: f32) -> Vec<Prim> {
        let mut p = Vec::new();
        let ui = crate::theme::blitz(false).ui;
        let cell = (CELL.0 * scale as i32, CELL.1 * scale as i32);
        draw(&mut p, area, state, t, scale, &ui, cell);
        p
    }

    /// The box a prim covers, with text one cell per character.
    fn bounds(p: &Prim, scale: f32) -> Rect {
        match p {
            Prim::Rect(r, _) | Prim::Shape { r, .. } | Prim::Branch(r, _) => *r,
            Prim::Text { x, y, text, .. } => Rect {
                x: *x,
                y: *y,
                w: CELL.0 * scale as i32 * text.chars().count() as i32,
                h: CELL.1 * scale as i32,
            },
        }
    }

    fn texts(p: &[Prim]) -> Vec<&str> {
        (p.iter())
            .filter_map(|p| match p {
                Prim::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn nothing_is_drawn_where_it_does_not_fit() {
        let a = area();
        for small in [
            Rect { h: 40, ..a },
            Rect { w: 40, ..a },
            Rect { w: 0, ..a },
            Rect { h: -30, ..a },
        ] {
            for state in STATES {
                assert!(spark(small, state, 1.0, 1.0).is_empty(), "{small:?}");
            }
        }
    }

    #[test]
    fn everything_stays_inside_the_area_at_any_size_and_time() {
        for scale in [1.0, 2.0] {
            for w in (0..300).step_by(13) {
                for h in (0..220).step_by(9) {
                    let a = Rect { x: 5, y: 50, w, h };
                    for state in STATES {
                        for i in 0..40 {
                            let t = f64::from(i) * 0.137;
                            for p in spark(a, state, t, scale) {
                                let r = bounds(&p, scale);
                                let inside = r.x >= a.x
                                    && r.y >= a.y
                                    && r.right() <= a.right()
                                    && r.bottom() <= a.bottom();
                                assert!(inside, "{state:?} at {t} in {a:?}: {p:?}");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_state_draws_at_time_zero() {
        for state in STATES {
            assert!(!spark(area(), state, 0.0, 1.0).is_empty(), "{state:?}");
        }
    }

    #[test]
    fn needing_you_shows_an_exclamation_mark() {
        for i in 0..20 {
            let p = spark(area(), Attn::NeedsYou, f64::from(i) * 0.1, 1.0);
            assert!(texts(&p).contains(&"!"));
        }
    }

    #[test]
    fn sleeping_shows_a_z() {
        for i in 0..20 {
            let p = spark(area(), Attn::Idle, f64::from(i) * 0.3, 1.0);
            assert!(texts(&p).iter().any(|t| t.eq_ignore_ascii_case("z")));
        }
    }

    #[test]
    fn working_runs_across_the_area_and_back() {
        let a = area();
        let left = |t: f64| {
            let p = spark(a, Attn::Working, t, 1.0);
            p.iter().map(|p| bounds(p, 1.0).x).min().expect("drawn")
        };
        assert_ne!(left(0.0), left(0.5));
        let xs: Vec<i32> = (0..200).map(|i| left(f64::from(i) * 0.05)).collect();
        let (min, max) = (xs.iter().min(), xs.iter().max());
        let (min, max) = (*min.expect("min"), *max.expect("max"));
        assert!(min < a.x + 40 && max > a.right() - 80, "{min}..{max}");
        // Still smooth after a month open, where an f32 second steps by 0.25.
        let month = 30.0 * 86_400.0;
        assert_ne!(left(month), left(month + 0.05));
    }
}
