//! Dark and light palettes, and reading the Windows app theme.

use vt::Palette;

pub fn dark() -> Palette {
    Palette {
        fg: 0xd6d7d9,
        bg: 0x131417,
        cursor: 0xececea,
        selection_bg: 0x2c2e33,
        ansi: [
            0x26282d, 0xe5534b, 0x8cc39a, 0xf2b84b, 0x6aa1ff, 0xb392f0, 0x4fc1b0, 0xc8c9cc,
            0x63666d, 0xff7b72, 0xa6d6af, 0xf8d27a, 0x93bcff, 0xcbb3f6, 0x7fd8ca, 0xececea,
        ],
    }
}

pub fn light() -> Palette {
    Palette {
        fg: 0x2f3135,
        bg: 0xfcfcfb,
        cursor: 0x141518,
        selection_bg: 0xdfdfdb,
        ansi: [
            0x141518, 0xc8382f, 0x2e7a45, 0x9a6700, 0x2160c4, 0x8250df, 0x1b7c83, 0x6f7278,
            0x5c5f65, 0xe5534b, 0x3a9157, 0xb07d00, 0x3b7be0, 0x9a6ae0, 0x2a9aa2, 0xa9acb1,
        ],
    }
}

/// Whether Windows is set to light mode for apps. False when unknown.
#[cfg(windows)]
pub fn system_is_light() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::w;

    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: `value` and `size` are valid for the duration of the call.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
    };
    r.is_ok() && value != 0
}

#[cfg(not(windows))]
pub fn system_is_light() -> bool {
    false
}

/// The palette matching the system theme.
pub fn system() -> Palette {
    if system_is_light() { light() } else { dark() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG 2 contrast ratio between two `0xRRGGBB` colours.
    fn contrast(a: u32, b: u32) -> f64 {
        let lum = |rgb: u32| {
            let [_, r, g, b] = rgb.to_be_bytes();
            let lin = |c: u8| {
                let c = f64::from(c) / 255.0;
                if c <= 0.04045 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
        };
        let (a, b) = (lum(a), lum(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn text_meets_wcag_aa_contrast() {
        assert!((contrast(0xffffff, 0x000000) - 21.0).abs() < 1e-9);
        for (name, p) in [("dark", dark()), ("light", light())] {
            let text = contrast(p.fg, p.bg);
            assert!(text >= 4.5, "{name}: text contrast {text:.2}");
            let sel = contrast(p.fg, p.selection_bg);
            assert!(sel >= 4.5, "{name}: selected text contrast {sel:.2}");
            let cursor = contrast(p.cursor, p.bg);
            assert!(cursor >= 3.0, "{name}: cursor contrast {cursor:.2}");
        }
    }
}
