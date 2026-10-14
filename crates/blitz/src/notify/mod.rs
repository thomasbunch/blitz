//! Bringing the user back to a session while blitz is in the background:
//! the taskbar or Dock, notifications, and on Windows a key that works
//! from any program.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(windows)]
mod win;

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "macos")]
pub use mac::*;
#[cfg(windows)]
pub use win::*;

/// Flashing the taskbar button is the window's to do here; see
/// `App::alert`.
#[cfg(not(windows))]
pub fn flash(_hwnd: isize, _count: u32) {}

/// Only Windows lets a program take a key from every other: never taken.
#[cfg(not(windows))]
pub fn global_jump(_hwnd: isize, _on: bool) -> bool {
    false
}

/// No other window could have taken the key [`global_jump`] cannot.
#[cfg(not(windows))]
pub fn other_main(_hwnd: isize) -> bool {
    false
}

/// The most characters a notification line keeps.
const TOAST_LINE: usize = 200;

/// `lines` as a notification shows them: the empty ones left out, and
/// each without the controls and format characters a title loses, bidi
/// overrides among them, cut at [`TOAST_LINE`] characters. Program output
/// chose the text.
#[cfg_attr(windows, allow(dead_code))]
fn clean_lines(lines: &[&str]) -> Vec<String> {
    (lines.iter())
        .filter(|l| !l.is_empty())
        .map(|l| vt::osc::clean(l, TOAST_LINE))
        .filter(|l| !l.is_empty())
        .collect()
}

/// A round badge `size` pixels wide for the taskbar button, drawn like the
/// sidebar's dots: a dot of `fg`, or a ring of it, on a disc of `bg` that
/// sets it apart from the icon under it. Pixels are `0xAARRGGBB`, top row
/// first, with straight alpha as icons take it.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn badge_pixels(size: u32, fg: u32, bg: u32, ring: bool) -> Vec<u32> {
    let r = size as f32 / 2.0;
    let inner = r - (size as f32 / 10.0).max(1.0);
    let stroke = (size as f32 / 8.0).max(1.5);
    // How much of a pixel `d` from the centre a circle of radius `rad` covers.
    let cover = |rad: f32, d: f32| (rad - d + 0.5).clamp(0.0, 1.0);
    (0..size * size)
        .map(|i| {
            let (x, y) = ((i % size) as f32 + 0.5 - r, (i / size) as f32 + 0.5 - r);
            let d = (x * x + y * y).sqrt();
            let mut k = cover(inner, d);
            if ring {
                k -= cover(inner - stroke, d);
            }
            let alpha = (cover(r, d) * 255.0).round() as u32;
            alpha << 24 | crate::theme::mix(bg, fg, k)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badges_are_round_dots_or_rings() {
        let (fg, bg) = (0x11_22_33, 0xee_dd_cc);
        let n = 16;
        let dot = badge_pixels(n, fg, bg, false);
        let ring = badge_pixels(n, fg, bg, true);
        let at = |px: &[u32], x: u32, y: u32| px[(y * n + x) as usize];
        for px in [&dot, &ring] {
            assert_eq!(px.len(), 256);
            // Corners are clear; the edge is the background disc.
            assert_eq!(at(px, 0, 0) >> 24, 0);
            assert_eq!(at(px, 15, 15) >> 24, 0);
            let edge = at(px, 0, 8);
            assert_eq!(edge & 0xff_ffff, bg);
            assert!(edge >> 24 > 0x80, "mostly covered");
            // Round: the same across both axes and both diagonals.
            for y in 0..n {
                for x in 0..n {
                    assert_eq!(at(px, x, y), at(px, n - 1 - x, y));
                    assert_eq!(at(px, x, y), at(px, y, x));
                }
            }
        }
        assert_eq!(at(&dot, 8, 8), 0xff << 24 | fg, "a full dot");
        assert_eq!(at(&ring, 8, 8), 0xff << 24 | bg, "an open ring");
        assert_eq!(at(&ring, 8, 2), 0xff << 24 | fg, "the ring itself");
    }

    #[test]
    fn notification_lines_drop_controls_and_empties() {
        let lines = clean_lines(&["pwsh 3 needs you", "", "a\u{202e}b\x07", &"y".repeat(500)]);
        assert_eq!(lines[..2], ["pwsh 3 needs you", "ab"]);
        assert_eq!(lines[2].len(), TOAST_LINE);
    }
}
