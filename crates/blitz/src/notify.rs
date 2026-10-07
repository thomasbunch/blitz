//! Bringing the user back to a session while blitz is in the background:
//! the taskbar button's flash and badge.

use std::ffi::c_void;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIcon, FLASHW_TRAY, FLASHWINFO, FlashWindowEx, HICON,
};

/// Flashes the taskbar button of `hwnd` `count` times. The button stays
/// lit after the last one until the window comes to the front, so a few
/// are enough and none goes on for ever.
pub fn flash(hwnd: isize, count: u32) {
    let info = FLASHWINFO {
        cbSize: size_of::<FLASHWINFO>() as u32,
        hwnd: HWND(hwnd as *mut c_void),
        dwFlags: FLASHW_TRAY,
        uCount: count,
        dwTimeout: 0,
    };
    // SAFETY: a filled-in struct that outlives the call; a stale window
    // handle only fails.
    let _ = unsafe { FlashWindowEx(&info) };
}

/// A round badge `size` pixels wide for the taskbar button, drawn like the
/// sidebar's dots: a dot of `fg`, or a ring of it, on a disc of `bg` that
/// sets it apart from the icon under it. Pixels are `0xAARRGGBB`, top row
/// first, with straight alpha as icons take it.
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

/// An icon from [`badge_pixels`]; the caller destroys it.
pub fn badge_icon(size: u32, fg: u32, bg: u32, ring: bool) -> Option<HICON> {
    let px = badge_pixels(size, fg, bg, ring);
    // Ignored where the colour has alpha, which is everywhere; one byte a
    // pixel is more than its one bit.
    let mask = vec![0u8; px.len()];
    let n = size as i32;
    // SAFETY: both buffers hold at least `size` rows of `size` pixels in
    // the depths given, and outlive the call, which copies them.
    unsafe { CreateIcon(None, n, n, 1, 32, mask.as_ptr(), px.as_ptr().cast()) }.ok()
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
}
