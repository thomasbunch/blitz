//! Pixel art drawn as rectangles, and the spark: blitz's own critter, a
//! round body with a lightning bolt for an antenna.

use crate::layout::Rect;
use crate::render::chrome::Prim;

/// Width and height of a spark frame, in art pixels.
pub const SPARK_W: i32 = 13;
pub const SPARK_H: i32 = 14;

const BOLT: [&str; 5] = [
    "......kk.....",
    ".....kbk.....",
    "....kbk......",
    "....kbbk.....",
    ".....kbk.....",
];
const TOP: [&str; 3] = ["...kkkkkkk...", "..kyyyyyyyk..", ".kywyyyyyyyk."];
const OPEN: [&str; 2] = [".kyyykyykyyk.", ".kyyykyykyyk."];
const SHUT: [&str; 2] = [".kyyyyyyyyyk.", ".kyykkyykkyk."];
const BOTTOM: [&str; 3] = [".kyyyyyyyyyk.", "..kyyyyyyyk..", "...kkkkkkk..."];
const STAND: &str = "...kk...kk...";
const STRIDE: &str = "..kk.....kk..";

/// A spark frame, top row first: eyes open or shut, feet together or
/// mid-stride.
pub fn spark(open: bool, stride: bool) -> [&'static str; SPARK_H as usize] {
    let eyes = if open { OPEN } else { SHUT };
    let feet = if stride { STRIDE } else { STAND };
    [
        BOLT[0], BOLT[1], BOLT[2], BOLT[3], BOLT[4], TOP[0], TOP[1], TOP[2], eyes[0], eyes[1],
        BOTTOM[0], BOTTOM[1], BOTTOM[2], feet,
    ]
}

/// The spark's colours from its body colour: `y` body, `k` a darker
/// outline, `w` a highlight, `b` the bolt.
pub fn spark_colors(body: u32) -> impl Fn(char) -> Option<u32> {
    move |c| match c {
        'y' => Some(body),
        'k' => Some(shade(body, 0x000000, 0.55)),
        'w' => Some(shade(body, 0xffffff, 0.6)),
        'b' => Some(shade(body, 0xffffff, 0.3)),
        _ => None,
    }
}

/// Draws `art` with its top-left corner at (`x`, `y`), each art pixel a
/// `px` square. `.` and space are clear; other characters take the colour
/// `color` gives them, and none leaves them clear. Runs of one colour in a
/// row become one rectangle.
pub fn draw(
    p: &mut Vec<Prim>,
    art: &[&str],
    x: i32,
    y: i32,
    px: i32,
    color: impl Fn(char) -> Option<u32>,
) {
    for (r, row) in art.iter().enumerate() {
        let mut run: Option<(i32, i32, u32)> = None;
        let flush = |run: Option<(i32, i32, u32)>, p: &mut Vec<Prim>| {
            if let Some((c0, c1, rgb)) = run {
                let r = Rect {
                    x: x + c0 * px,
                    y: y + r as i32 * px,
                    w: (c1 - c0) * px,
                    h: px,
                };
                p.push(Prim::Rect(r, rgb));
            }
        };
        for (c, ch) in row.chars().enumerate() {
            let c = c as i32;
            let rgb = if matches!(ch, '.' | ' ') {
                None
            } else {
                color(ch)
            };
            match (run, rgb) {
                (Some((c0, c1, a)), Some(b)) if a == b && c1 == c => run = Some((c0, c + 1, a)),
                (_, Some(b)) => {
                    flush(run, p);
                    run = Some((c, c + 1, b));
                }
                (_, None) => {
                    flush(run, p);
                    run = None;
                }
            }
        }
        flush(run, p);
    }
}

/// `a` moved `t` of the way to `b`, per channel.
pub fn shade(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = ((a >> s & 0xff) as f32, (b >> s & 0xff) as f32);
        ((x + (y - x) * t).round().clamp(0.0, 255.0) as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spark_frames_are_all_one_size() {
        for open in [false, true] {
            for stride in [false, true] {
                for row in spark(open, stride) {
                    assert_eq!(row.len() as i32, SPARK_W, "{row:?}");
                }
            }
        }
    }

    #[test]
    fn runs_of_one_colour_merge_and_clear_pixels_are_skipped() {
        let mut p = Vec::new();
        draw(&mut p, &["aab.a", " a"], 10, 20, 2, |c| {
            Some(if c == 'a' { 1 } else { 2 })
        });
        let rect = |x, y, w, rgb| Prim::Rect(Rect { x, y, w, h: 2 }, rgb);
        assert_eq!(
            p,
            [
                rect(10, 20, 4, 1),
                rect(14, 20, 2, 2),
                rect(18, 20, 2, 1),
                rect(12, 22, 2, 1),
            ]
        );
    }

    #[test]
    fn shade_moves_each_channel() {
        assert_eq!(shade(0x000000, 0xffffff, 0.5), 0x808080);
        assert_eq!(shade(0x123456, 0xabcdef, 0.0), 0x123456);
    }
}
