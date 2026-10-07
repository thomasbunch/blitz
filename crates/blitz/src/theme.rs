//! Colour themes: the built-in ones and files in `%APPDATA%\blitz\themes`,
//! in Ghostty's theme format, and reading the Windows app theme.

use std::path::{Path, PathBuf};

use vt::Palette;

/// Attention accent of the blitz themes; text on it is near-black.
pub const ACCENT: u32 = 0xf2b84b;

/// The least contrast a divider has with the panes' background.
const DIVIDER_CONTRAST: f64 = 1.3;

/// The `theme` setting when there is none: follow the Windows app theme.
pub const DEFAULT: &str = "light:blitz light,dark:blitz dark";

/// Built-in themes as (name, file), the plain blitz pair first. A user
/// file with the same name wins.
const BUILTIN: &[(&str, &str)] = &[
    ("blitz dark", include_str!("../themes/blitz-dark")),
    ("blitz light", include_str!("../themes/blitz-light")),
    (
        "blitz ember dark",
        include_str!("../themes/blitz-ember-dark"),
    ),
    (
        "blitz ember light",
        include_str!("../themes/blitz-ember-light"),
    ),
    ("blitz tide dark", include_str!("../themes/blitz-tide-dark")),
    (
        "blitz tide light",
        include_str!("../themes/blitz-tide-light"),
    ),
    (
        "Catppuccin Mocha",
        include_str!("../themes/catppuccin-mocha"),
    ),
    (
        "Catppuccin Latte",
        include_str!("../themes/catppuccin-latte"),
    ),
    ("GitHub Dark", include_str!("../themes/github-dark")),
    ("GitHub Light", include_str!("../themes/github-light")),
    ("Gruvbox Dark", include_str!("../themes/gruvbox-dark")),
    ("Gruvbox Light", include_str!("../themes/gruvbox-light")),
    ("Rose Pine", include_str!("../themes/rose-pine")),
    ("Rose Pine Dawn", include_str!("../themes/rose-pine-dawn")),
    ("Solarized Dark", include_str!("../themes/solarized-dark")),
    ("Solarized Light", include_str!("../themes/solarized-light")),
    ("Tokyo Night", include_str!("../themes/tokyo-night")),
    ("Tokyo Night Day", include_str!("../themes/tokyo-night-day")),
];

#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub name: String,
    pub pal: Palette,
    /// A light background: light window frame and `CSI ? 996 n` reply.
    pub light: bool,
    pub ui: Ui,
}

/// Window chrome colours, as `0xRRGGBB`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ui {
    pub term_bg: u32,
    pub term_fg: u32,
    pub side_bg: u32,
    pub border: u32,
    pub rule: u32,
    pub row_focus: u32,
    pub name: u32,
    pub dim: u32,
    pub msg: u32,
    pub accent: u32,
    /// Text on the accent; follows it.
    pub chip_fg: u32,
    /// The accent as dots and rings draw it, which have no text to carry
    /// them: moved away from the chrome's backgrounds until it stands out
    /// from each by 3:1. Follows the accent.
    pub mark: u32,
    pub track: u32,
    pub fill: u32,
    pub error: u32,
    pub hdr_bg: u32,
    pub hdr_line: u32,
    pub hdr_name: u32,
    pub hdr_cwd: u32,
    pub rail_focus: u32,
    pub rail_work: u32,
    pub idle: u32,
    pub label: u32,
    pub label_focus: u32,
    pub top_track: u32,
}

/// Chrome colours a theme may leave out, mixed from its background toward
/// its foreground, as (theme file key, amount when dark, amount when light).
const MIXES: &[(&str, f32, f32)] = &[
    ("border", 0.08, 0.15),
    ("rule", 0.10, 0.14),
    ("row-focus", 0.05, 0.10),
    ("dim", 0.63, 0.78),
    ("message", 0.93, 1.0),
    ("track", 0.13, 0.20),
    ("progress", 0.68, 0.69),
    ("header-background", 0.02, 0.065),
    ("header-line", 0.04, 0.10),
    ("header-title", 0.77, 0.90),
    ("header-cwd", 0.55, 0.73),
    ("rail-focus", 0.05, 0.13),
    ("rail-work", 0.62, 0.71),
    ("idle", 0.20, 0.28),
    ("label", 0.55, 0.74),
    ("label-focus", 0.63, 0.78),
    ("top-track", 0.055, 0.085),
];

impl Ui {
    /// The chrome colour a theme file key sets.
    fn field(&mut self, key: &str) -> Option<&mut u32> {
        Some(match key {
            "sidebar-background" => &mut self.side_bg,
            "border" => &mut self.border,
            "rule" => &mut self.rule,
            "row-focus" => &mut self.row_focus,
            "title" => &mut self.name,
            "dim" => &mut self.dim,
            "message" => &mut self.msg,
            "accent" => &mut self.accent,
            "track" => &mut self.track,
            "progress" => &mut self.fill,
            "error" => &mut self.error,
            "header-background" => &mut self.hdr_bg,
            "header-line" => &mut self.hdr_line,
            "header-title" => &mut self.hdr_name,
            "header-cwd" => &mut self.hdr_cwd,
            "rail-focus" => &mut self.rail_focus,
            "rail-work" => &mut self.rail_work,
            "idle" => &mut self.idle,
            "label" => &mut self.label,
            "label-focus" => &mut self.label_focus,
            "top-track" => &mut self.top_track,
            _ => return None,
        })
    }

    /// Chrome colours that suit `pal`.
    fn derive(pal: &Palette, light: bool) -> Ui {
        let (bg, fg) = (pal.bg, pal.fg);
        let mut ui = Ui {
            term_bg: bg,
            term_fg: fg,
            side_bg: mix(bg, 0x000000, if light { 0.035 } else { 0.2 }),
            name: mix(fg, if light { 0x000000 } else { 0xffffff }, 0.55),
            error: pal.ansi[1],
            ..Ui::default()
        };
        let side = ui.side_bg;
        for &(key, d, l) in MIXES {
            if let Some(f) = ui.field(key) {
                let mut t = if light { l } else { d };
                *f = mix(bg, fg, t);
                // On a theme with little contrast, Solarized Dark for one,
                // the sidebar's dimmer text would be too faint to read.
                while matches!(key, "dim" | "label" | "label-focus")
                    && contrast(*f, side) < 3.0
                    && t < 1.0
                {
                    t = (t + 0.05).min(1.0);
                    *f = mix(bg, fg, t);
                }
            }
        }
        ui.set_accent(pal.ansi[3]);
        ui
    }

    pub fn set_accent(&mut self, accent: u32) {
        self.accent = accent;
        self.chip_fg = if luminance(accent) > 0.18 {
            0x17140d
        } else {
            0xffffff
        };
        // Amber on a light background is about 1.6:1, too faint for a
        // dot; darken it there, lighten it on a dark one.
        let away = if luminance(self.term_bg) > 0.18 {
            0x000000
        } else {
            0xffffff
        };
        let under = [
            self.term_bg,
            self.side_bg,
            self.row_focus,
            self.hdr_bg,
            self.rail_focus,
        ];
        let mut t = 0.0;
        self.mark = accent;
        while under.iter().any(|&b| contrast(self.mark, b) < 3.0) && t < 1.0 {
            t = (t + 0.05f32).min(1.0);
            self.mark = mix(accent, away, t);
        }
    }
}

/// The terminal colour keys of a theme file, Ghostty's, besides
/// `palette`; [`Ui::field`] has the chrome's.
const KEYS: &[&str] = &[
    "background",
    "foreground",
    "cursor-color",
    "cursor-text",
    "selection-background",
    "selection-foreground",
];

/// The `key = #rrggbb` lines of a theme file in order, and its
/// `palette = N=#rrggbb` lines. `#` starts a comment line.
fn lines(text: &str) -> (Vec<(&str, u32)>, [Option<u32>; 16]) {
    let mut colors = Vec::new();
    let mut ansi = [None; 16];
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key == "palette" {
            if let Some((i, c)) = value.split_once('=')
                && let (Ok(i @ 0..16), Some(c)) = (i.trim().parse::<usize>(), color(c))
            {
                ansi[i] = Some(c);
            }
        } else if let Some(c) = color(value) {
            colors.push((key, c));
        }
    }
    (colors, ansi)
}

/// Whether `text` sets a colour a theme can, so a README or a picture
/// left in the themes folder is not offered as a theme.
fn is_theme(text: &str) -> bool {
    let (colors, ansi) = lines(text);
    ansi.iter().any(Option::is_some)
        || (colors.iter()).any(|&(k, _)| KEYS.contains(&k) || Ui::default().field(k).is_some())
}

/// A theme file: the colours of [`KEYS`] and `palette = N=#rrggbb`, plus
/// the chrome keys of [`Ui::field`]. Anything else is skipped, so a
/// Ghostty theme works as is, and colours a file leaves out come from the
/// blitz theme of the same lightness.
pub fn parse(name: &str, text: &str) -> Theme {
    let (colors, ansi) = lines(text);
    let get = |k: &str| colors.iter().rev().find(|c| c.0 == k).map(|c| c.1);
    let light = get("background").is_some_and(|bg| luminance(bg) > 0.18);
    let base = if light { self::light() } else { dark() };
    let (bg, fg) = (
        get("background").unwrap_or(base.bg),
        get("foreground").unwrap_or(base.fg),
    );
    let pal = Palette {
        fg,
        bg,
        cursor: get("cursor-color").unwrap_or(fg),
        cursor_text: get("cursor-text"),
        selection_bg: get("selection-background")
            .unwrap_or_else(|| mix(bg, fg, if light { 0.15 } else { 0.2 })),
        selection_fg: get("selection-foreground").unwrap_or(fg),
        ansi: std::array::from_fn(|i| ansi[i].unwrap_or(base.ansi[i])),
    };
    let mut ui = Ui::derive(&pal, light);
    for &(key, c) in &colors {
        if let Some(f) = ui.field(key) {
            *f = c;
        }
    }
    // Between two panes that are not dimmed, such as one that needs you
    // beside the focused one, the divider is all that parts them.
    let (border, mut t) = (ui.border, 0.0);
    while contrast(ui.border, ui.term_bg) < DIVIDER_CONTRAST && t < 1.0 {
        t = (t + 0.05f32).min(1.0);
        ui.border = mix(border, ui.term_fg, t);
    }
    ui.set_accent(ui.accent);
    Theme {
        name: name.into(),
        pal,
        light,
        ui,
    }
}

/// `#rrggbb` or `rrggbb`.
fn color(s: &str) -> Option<u32> {
    let s = s.trim();
    let hex = s.strip_prefix('#').unwrap_or(s);
    if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        u32::from_str_radix(hex, 16).ok()
    } else {
        None
    }
}

/// `a` moved `t` of the way to `b`, per channel.
pub(crate) fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = ((a >> s & 0xff) as f32, (b >> s & 0xff) as f32);
        ((x + (y - x) * t).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

/// Each sRGB channel value in linear light, looked up rather than worked
/// out, as the renderer asks for every cell.
fn linear() -> &'static [f64; 256] {
    static LINEAR: std::sync::LazyLock<[f64; 256]> = std::sync::LazyLock::new(|| {
        std::array::from_fn(|i| {
            let c = i as f64 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        })
    });
    &LINEAR
}

/// WCAG 2 relative luminance of `0xRRGGBB`.
fn luminance(rgb: u32) -> f64 {
    let [_, r, g, b] = rgb.to_be_bytes();
    let lin = linear();
    0.2126 * lin[usize::from(r)] + 0.7152 * lin[usize::from(g)] + 0.0722 * lin[usize::from(b)]
}

/// WCAG 2 contrast ratio between two `0xRRGGBB` colours.
pub fn contrast(a: u32, b: u32) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `fg`, or when it stands less than `min`:1 from `bg`, `fg` moved toward
/// white or black, whichever has room, just far enough to. Moving in
/// linear light keeps its hue.
pub fn readable(fg: u32, bg: u32, min: f64) -> u32 {
    let (f, b) = (luminance(fg), luminance(bg));
    if (f.max(b) + 0.05) / (f.min(b) + 0.05) >= min {
        return fg;
    }
    // The luminance `fg` needs, above `bg` or below it.
    let (up, down) = (min * (b + 0.05) - 0.05, (b + 0.05) / min - 0.05);
    let lighter = if f >= b { up <= 1.0 } else { down < 0.0 };
    let lin = linear();
    let ch = |s: u32| {
        let c = lin[(fg >> s & 0xff) as usize];
        // Each channel moves by the share luminance must, and rounds
        // away from `bg`.
        let i = if lighter {
            let want = c + (1.0 - c) * (up - f) / (1.0 - f);
            lin.partition_point(|&l| l < want).min(255)
        } else {
            let want = c * down / f;
            lin.partition_point(|&l| l <= want).saturating_sub(1)
        };
        (i as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

/// `%APPDATA%\blitz\themes`.
pub fn dir() -> Option<PathBuf> {
    crate::config::dir().map(|d| d.join("themes"))
}

/// A user theme's name: its file name without a `.conf` ending, in any
/// case. Hidden files and editor backups (`Midnight~`) are no themes.
fn file_name(p: &Path) -> Option<String> {
    let n = p.file_name()?.to_str()?;
    if n.starts_with('.') || n.ends_with('~') {
        return None;
    }
    let conf = n.len().checked_sub(5).filter(|&i| n.is_char_boundary(i));
    let name = match conf {
        Some(i) if n[i..].eq_ignore_ascii_case(".conf") => &n[..i],
        _ => n,
    };
    (!name.is_empty()).then(|| name.to_string())
}

/// Every theme: the built-ins in order, then the themes folder sorted by
/// name. A file named like a built-in replaces it in place.
pub fn all() -> Vec<Theme> {
    all_in(dir().as_deref())
}

fn all_in(dir: Option<&Path>) -> Vec<Theme> {
    // A file being saved may not be readable for a moment; the save that
    // follows reloads it.
    let mut files: Vec<(String, PathBuf, String)> = dir
        .and_then(|d| std::fs::read_dir(d).ok())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter_map(|p| {
            let name = file_name(&p)?;
            let text = crate::config::decode(&std::fs::read(&p).ok()?);
            is_theme(&text).then_some((name, p, text))
        })
        .collect();
    files.sort_by(|a, b| (a.0.to_lowercase(), &a.1).cmp(&(b.0.to_lowercase(), &b.1)));
    // `X` and `X.conf` name one theme; the first in that order wins.
    files.dedup_by(|b, a| a.0.eq_ignore_ascii_case(&b.0));
    let mut out: Vec<Theme> = BUILTIN
        .iter()
        .map(
            |&(name, text)| match files.iter().position(|f| f.0.eq_ignore_ascii_case(name)) {
                Some(i) => {
                    let (name, _, text) = files.remove(i);
                    parse(&name, &text)
                }
                None => parse(name, text),
            },
        )
        .collect();
    out.extend((files.iter()).map(|(name, _, text)| parse(name, text)));
    out
}

/// The `light:` or `dark:` half of a `theme` setting, by `prefix`. Only a
/// comma before the other half splits the setting, so a theme's name may
/// hold one.
fn half<'a>(setting: &'a str, prefix: &str) -> Option<&'a str> {
    let mut start = 0;
    let mut parts = Vec::new();
    for (i, _) in setting.match_indices(',') {
        let next = setting[i + 1..].trim_start();
        if next.starts_with("light:") || next.starts_with("dark:") {
            parts.push(&setting[start..i]);
            start = i + 1;
        }
    }
    parts.push(&setting[start..]);
    (parts.into_iter())
        .find_map(|p| p.trim().strip_prefix(prefix))
        .map(str::trim)
}

/// The theme a `theme` setting names: one name, or `light:NAME,dark:NAME`
/// to follow the Windows app theme.
pub fn choose(setting: &str, system_light: bool) -> &str {
    let want = if system_light { "light:" } else { "dark:" };
    half(setting, want).unwrap_or(setting.trim())
}

/// The `theme` setting after picking `name`. Of a `light:X,dark:Y` pair
/// only the half in use now changes. While high contrast sets the colours
/// (`contrast`), a pick that would leave the default pair as it is becomes
/// the whole setting, or high contrast would win over it.
pub fn pick(setting: &str, name: &str, system_light: bool, contrast: bool) -> String {
    let paired = match (half(setting, "light:"), half(setting, "dark:")) {
        (Some(_), Some(d)) if system_light => format!("light:{name},dark:{d}"),
        (Some(l), Some(_)) => format!("light:{l},dark:{name}"),
        _ => name.into(),
    };
    if contrast && is_default(&paired) {
        return name.into();
    }
    paired
}

/// The theme the `theme` setting picks now. An unknown name gives the
/// blitz theme that matches the system.
pub fn current(setting: &str) -> Theme {
    match contrast_for(setting) {
        Some(c) => high_contrast(c),
        None => current_of(all(), setting, system_is_light()),
    }
}

/// The name [`high_contrast`] gives its theme.
pub const HIGH_CONTRAST: &str = "high contrast";

/// The colours of Windows high contrast mode while it is on and the
/// `theme` setting is the default; a theme set by hand wins.
pub fn contrast_for(setting: &str) -> Option<[u32; 3]> {
    system_contrast().filter(|_| is_default(setting))
}

/// Whether `setting` picks the default's themes, however it is written.
fn is_default(setting: &str) -> bool {
    [false, true]
        .iter()
        .all(|&light| choose(setting, light).eq_ignore_ascii_case(choose(DEFAULT, light)))
}

/// The theme of Windows high contrast mode: its window background, text
/// and highlight colours, as `0xRRGGBB`, the highlight marking sessions
/// that need you.
// ponytail: the 16 program colours stay the blitz ones of that lightness;
// set them here if one is hard to read in a contrast theme
pub fn high_contrast([window, text, highlight]: [u32; 3]) -> Theme {
    let file = format!(
        "background = #{window:06x}\nforeground = #{text:06x}\naccent = #{highlight:06x}\n"
    );
    parse(HIGH_CONTRAST, &file)
}

fn current_of(mut all: Vec<Theme>, setting: &str, light: bool) -> Theme {
    let mut take = |name: &str| {
        let i = all.iter().position(|t| t.name.eq_ignore_ascii_case(name))?;
        Some(all.swap_remove(i))
    };
    take(choose(setting, light))
        .or_else(|| take(choose(DEFAULT, light)))
        .unwrap_or_else(|| blitz(light))
}

/// The built-in blitz theme of this lightness.
pub fn blitz(light: bool) -> Theme {
    let (name, text) = BUILTIN[usize::from(light)];
    parse(name, text)
}

pub fn dark() -> Palette {
    Palette {
        fg: 0xd6d7d9,
        bg: 0x131417,
        cursor: 0xececea,
        cursor_text: None,
        selection_bg: 0x2c2e33,
        selection_fg: 0xd6d7d9,
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
        cursor_text: None,
        selection_bg: 0xdfdfdb,
        selection_fg: 0x2f3135,
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

/// While Windows high contrast mode is on, its window background, window
/// text and highlight colours, as `0xRRGGBB`.
#[cfg(windows)]
pub fn system_contrast() -> Option<[u32; 3]> {
    use windows::Win32::Graphics::Gdi::{
        COLOR_HIGHLIGHT, COLOR_WINDOW, COLOR_WINDOWTEXT, GetSysColor,
    };
    use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };

    let mut hc = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    // SAFETY: `hc` is the struct this action fills, with its size set.
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            Some((&raw mut hc).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .ok()?;
    if !hc.dwFlags.contains(HCF_HIGHCONTRASTON) {
        return None;
    }
    // SAFETY: plain calls with valid indexes. COLORREF is 0x00BBGGRR.
    let rgb = |i| unsafe { GetSysColor(i) }.swap_bytes() >> 8;
    Some([
        rgb(COLOR_WINDOW),
        rgb(COLOR_WINDOWTEXT),
        rgb(COLOR_HIGHLIGHT),
    ])
}

#[cfg(not(windows))]
pub fn system_contrast() -> Option<[u32; 3]> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every chrome key a theme file can set.
    fn keys() -> impl Iterator<Item = &'static str> {
        ["sidebar-background", "title", "error", "accent"]
            .into_iter()
            .chain(MIXES.iter().map(|m| m.0))
    }

    #[test]
    fn blitz_themes_meet_wcag_aa_contrast() {
        assert!((contrast(0xffffff, 0x000000) - 21.0).abs() < 1e-9);
        for &(name, text) in BUILTIN.iter().filter(|b| b.0.starts_with("blitz ")) {
            let p = parse(name, text).pal;
            let text = contrast(p.fg, p.bg);
            assert!(text >= 4.5, "{name}: text contrast {text:.2}");
            let sel = contrast(p.fg, p.selection_bg);
            assert!(sel >= 4.5, "{name}: selected text contrast {sel:.2}");
            let cursor = contrast(p.cursor, p.bg);
            assert!(cursor >= 3.0, "{name}: cursor contrast {cursor:.2}");
        }
    }

    #[test]
    fn builtin_files_are_complete() {
        assert_eq!(blitz(false).pal, dark());
        assert_eq!(blitz(true).pal, light());
        assert!(!blitz(false).light && blitz(true).light);
        let mut names: Vec<_> = BUILTIN.iter().map(|b| b.0).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), BUILTIN.len(), "duplicate built-in name");
        for &(name, text) in BUILTIN {
            assert!(text.starts_with(&format!("# {name}\n")), "{name}");
            for key in ["background", "foreground", "cursor-color"] {
                assert!(text.contains(&format!("\n{key} = #")), "{name}: {key}");
            }
            for i in 0..16 {
                assert!(text.contains(&format!("\npalette = {i}=#")), "{name}: {i}");
            }
            let n = name.to_lowercase();
            let light = ["light", "latte", "day", "dawn"]
                .iter()
                .any(|w| n.contains(w));
            assert_eq!(parse(name, text).light, light, "{name}");
        }
        // The blitz files spell out every chrome colour, as a template.
        for light in [false, true] {
            let (name, text) = BUILTIN[usize::from(light)];
            for key in keys() {
                assert!(text.contains(&format!("\n{key} = #")), "{name}: {key}");
            }
        }
    }

    #[test]
    fn derived_chrome_stays_close_to_the_hand_tuned_blitz_colours() {
        for light in [false, true] {
            let mut tuned = blitz(light).ui;
            let mut mixed = Ui::derive(&blitz(light).pal, light);
            for key in keys().filter(|&k| k != "accent") {
                let (a, b) = (*tuned.field(key).unwrap(), *mixed.field(key).unwrap());
                let d = |s: u32| (a >> s & 0xff).abs_diff(b >> s & 0xff);
                let off = d(16).max(d(8)).max(d(0));
                assert!(off <= 12, "light {light}, {key}: {a:06x} vs {b:06x}");
            }
        }
        let ui = blitz(true).ui;
        assert_eq!((ui.accent, ui.chip_fg), (ACCENT, 0x17140d));
    }

    #[test]
    fn parse_reads_ghostty_files_and_skips_the_rest() {
        let t = parse(
            "x",
            "\u{feff}# comment\n\
             background = #fdf6e3\n\
             foreground=657B83\r\n\
             palette = 1=#dc322f\n\
             palette = 16=#ffffff\n\
             palette = x=#ffffff\n\
             font-size = 13\n\
             cursor-color = red\n\
             sidebar-background = #eee8d5\n\
             accent = #2160c4\n\
             selection-foreground = #000001\n\
             cursor-text = #000002\n",
        );
        assert_eq!((t.pal.selection_fg, t.pal.cursor_text), (1, Some(2)));
        assert!(t.light);
        assert_eq!((t.pal.bg, t.pal.fg), (0xfdf6e3, 0x657b83));
        assert_eq!(t.pal.ansi[1], 0xdc322f);
        assert_eq!(t.pal.ansi[2], light().ansi[2], "left out: from blitz light");
        assert_eq!(t.pal.cursor, t.pal.fg, "bad colour: cursor follows text");
        assert_eq!(t.ui.side_bg, 0xeee8d5);
        assert_eq!((t.ui.accent, t.ui.chip_fg), (0x2160c4, 0xffffff));
        assert_eq!(t.ui.term_bg, 0xfdf6e3);
        let (e, d) = (parse("e", "").pal, dark());
        assert_eq!((e.bg, e.fg, e.ansi), (d.bg, d.fg, d.ansi));
        // Left out, selected text is the text colour and the cursor's
        // text is picked to suit the cursor.
        assert_eq!((e.selection_fg, e.cursor_text), (e.fg, None));
    }

    #[test]
    fn choose_follows_the_system_or_takes_one_name() {
        assert_eq!(choose(DEFAULT, true), "blitz light");
        assert_eq!(choose(DEFAULT, false), "blitz dark");
        assert_eq!(choose(" Rose Pine ", true), "Rose Pine");
        assert_eq!(choose("light:A, dark: B", false), "B");
        assert_eq!(choose("light:A", false), "light:A");
    }

    #[test]
    fn pick_keeps_the_other_half_of_a_pair() {
        assert_eq!(pick(DEFAULT, "X", true, false), "light:X,dark:blitz dark");
        assert_eq!(pick(DEFAULT, "X", false, false), "light:blitz light,dark:X");
        assert_eq!(pick("Rose Pine", "X", true, false), "X");
        assert_eq!(pick("light:A", "X", true, false), "X");
        assert_eq!(choose(&pick(DEFAULT, "X", false, false), false), "X");
    }

    /// The default pair gives way to high contrast; a blitz theme picked
    /// then must not be that pair again.
    #[test]
    fn a_pick_under_high_contrast_is_not_the_default() {
        assert!(is_default(DEFAULT) && is_default("light: Blitz Light , dark:blitz dark"));
        for light in [false, true] {
            let name = choose(DEFAULT, light);
            assert!(is_default(&pick(DEFAULT, name, light, false)));
            assert_eq!(pick(DEFAULT, name, light, true), name);
            assert!(!is_default(&pick(DEFAULT, name, light, true)));
        }
        // Any other theme keeps the pair.
        assert_eq!(pick(DEFAULT, "X", false, true), "light:blitz light,dark:X");
    }

    #[test]
    fn pick_and_choose_agree_for_any_name() {
        for name in [
            "Rose Pine",
            "Foo, Bar",
            "Dracula's",
            "a ,b",
            "dark",
            "light: x",
        ] {
            for light in [false, true] {
                let setting = pick(DEFAULT, name, light, false);
                assert_eq!(choose(&setting, light), name.trim(), "{setting:?}");
                let other = if light { "blitz dark" } else { "blitz light" };
                assert_eq!(choose(&setting, !light), other, "{setting:?}");
            }
        }
        assert_eq!(choose("light:Foo, Bar, dark:Baz", true), "Foo, Bar");
        assert_eq!(choose("dark: Baz ,light: Foo, Bar", true), "Foo, Bar");
    }

    #[test]
    fn every_builtin_line_is_a_known_key_with_a_good_colour() {
        for &(name, text) in BUILTIN {
            for line in text
                .lines()
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
            {
                let (key, value) = line
                    .split_once(" = ")
                    .unwrap_or_else(|| panic!("{name}: {line:?}"));
                let value = match key {
                    "palette" => {
                        let (i, c) = value.split_once('=').expect("N=#rrggbb");
                        assert!(matches!(i.parse(), Ok(0..16)), "{name}: {line}");
                        c
                    }
                    _ => value,
                };
                assert!(
                    value.starts_with('#') && color(value).is_some(),
                    "{name}: {line}"
                );
                let chrome = Ui::default().field(key).is_some();
                let known = key == "palette" || KEYS.contains(&key);
                assert!(known || chrome, "{name}: unknown key {key}");
            }
        }
    }

    #[test]
    fn every_theme_file_is_built_in() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/themes");
        let mut files: Vec<String> = (std::fs::read_dir(dir).unwrap().flatten())
            .map(|e| e.file_name().into_string().unwrap())
            .collect();
        files.sort();
        let mut built: Vec<String> = (BUILTIN.iter())
            .map(|b| b.0.to_lowercase().replace(' ', "-"))
            .collect();
        built.sort();
        assert_eq!(files, built);
    }

    #[test]
    fn every_builtin_theme_has_readable_text_and_chrome() {
        for t in all_in(None) {
            let (p, ui) = (&t.pal, &t.ui);
            // The blitz themes meet AA (above); the others are as their
            // authors made them, Solarized Light at 4.1, but a broken file
            // would fall far lower.
            let text = contrast(p.fg, p.bg);
            assert!(text >= 4.0, "{}: text contrast {text:.2}", t.name);
            // The chrome is blitz's own, mixed from those colours.
            for (what, c) in [
                ("title", ui.name),
                ("label", ui.label),
                ("label-focus", ui.label_focus),
                ("dim", ui.dim),
            ] {
                let on = contrast(c, ui.side_bg);
                assert!(on >= 3.0, "{}: {what} on the sidebar {on:.2}", t.name);
            }
            let chip = contrast(ui.chip_fg, ui.accent);
            assert!(chip >= 4.5, "{}: text on the accent {chip:.2}", t.name);
            // Dots and rings sit on any of the chrome's backgrounds.
            for (what, c) in [
                ("pane", ui.term_bg),
                ("sidebar", ui.side_bg),
                ("focused row", ui.row_focus),
                ("header", ui.hdr_bg),
                ("rail row", ui.rail_focus),
            ] {
                let on = contrast(ui.mark, c);
                assert!(on >= 3.0, "{}: mark on the {what} {on:.2}", t.name);
            }
            // SGR 90, the grey of hints and Claude Code's tool results.
            let grey = contrast(p.ansi[8], p.bg);
            assert!(grey >= 1.5, "{}: palette 8 contrast {grey:.2}", t.name);
        }
    }

    #[test]
    fn every_builtin_accent_is_an_attention_colour() {
        // Palette 3 is brown in some themes, which reads as text rather
        // than as a session that needs you; those set an accent of their
        // own.
        for t in all_in(None) {
            let [_, r, g, b] = t.ui.accent.to_be_bytes();
            assert!(
                r > b && g > b && r >= 0x90,
                "{}: accent {:06x}",
                t.name,
                t.ui.accent
            );
        }
    }

    #[test]
    fn a_mark_that_stands_out_is_the_accent() {
        let dark = blitz(false).ui;
        assert_eq!(dark.mark, dark.accent);
        // Light: the same amber, darker.
        let light = blitz(true).ui;
        assert_eq!(light.accent, ACCENT);
        assert!(luminance(light.mark) < luminance(ACCENT));
        let [_, r, g, b] = light.mark.to_be_bytes();
        assert!(r > g && g > b, "still amber: {:06x}", light.mark);
    }

    #[test]
    fn dividers_never_vanish_into_the_background() {
        for t in all_in(None) {
            let on = contrast(t.ui.border, t.ui.term_bg);
            assert!(on >= DIVIDER_CONTRAST, "{}: divider {on:.2}", t.name);
        }
        // Even one a theme file sets to the background.
        let t = parse("x", "background = #131417\nborder = #131417");
        assert!(contrast(t.ui.border, t.ui.term_bg) >= DIVIDER_CONTRAST);
        assert!(
            luminance(t.ui.border) > luminance(0x131417),
            "toward the text"
        );
    }

    #[test]
    fn colours_are_six_hex_digits_and_the_last_line_counts() {
        assert_eq!(color("#0a0B0c"), Some(0x0a0b0c));
        assert_eq!(color(" 0a0b0c "), Some(0x0a0b0c));
        for bad in [
            "##0a0b0c",
            "#abc",
            "#0a0b0c0d",
            "#0a0b0g",
            "red",
            "",
            "#",
            "#0a0b0c # x",
        ] {
            assert_eq!(color(bad), None, "{bad:?}");
        }
        let t = parse(
            "x",
            "background = #000001\nbackground = #000002\n\
             palette = 3=#000003\npalette = 3=#000004\n\
             accent = #000005\naccent = #000006\n",
        );
        assert_eq!((t.pal.bg, t.pal.ansi[3], t.ui.accent), (2, 4, 6));
    }

    /// A themes folder for one test, gone when the test ends.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str, files: &[(&str, &[u8])]) -> Folder {
            let dir =
                std::env::temp_dir().join(format!("blitz-themes-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            for (file, text) in files {
                std::fs::write(dir.join(file), text).unwrap();
            }
            Folder(dir)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_user_file_replaces_a_builtin_in_place_and_others_follow() {
        let f = Folder::new(
            "replace",
            &[
                ("rose pine.conf", b"background = #000001\n"),
                ("Zed", b"background = #ffffff\n"),
                ("alpha.CONF", b"background = #000002\n"),
                ("Alpha", b"background = #000003\n"),
                ("Midnight~", b"background = #000004\n"),
                (".hidden", b"background = #000005\n"),
                ("Latin", b"# Th\xe8me\nbackground = #000006\n"),
                ("README.md", b"# Themes\n\nGhostty themes go here.\n"),
                ("logo.png", b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"),
                ("Nord", b"notes = for Nord\n"),
                ("nord.conf", b"palette = 0=#000007\n"),
            ],
        );
        std::fs::create_dir(f.0.join("folder")).unwrap();
        let t = all_in(Some(&f.0));
        let rose = BUILTIN.iter().position(|b| b.0 == "Rose Pine").unwrap();
        assert_eq!((t[rose].name.as_str(), t[rose].pal.bg), ("rose pine", 1));
        let extra: Vec<(&str, u32)> = (t[BUILTIN.len()..].iter())
            .map(|t| (t.name.as_str(), t.pal.bg))
            .collect();
        // `Alpha` and `alpha.CONF` are one theme; backups, hidden files,
        // folders and files that set no colour are none, so `nord.conf`
        // is the only Nord; a file that is not UTF-8 still reads.
        let nord = dark().bg;
        assert_eq!(
            extra,
            [
                ("Alpha", 3),
                ("Latin", 6),
                ("nord", nord),
                ("Zed", 0xffffff)
            ]
        );
        assert_eq!(all_in(None).len(), BUILTIN.len());
        assert_eq!(all_in(Some(&f.0.join("missing"))).len(), BUILTIN.len());
    }

    #[test]
    fn an_unknown_theme_falls_back_to_blitz() {
        let all = || all_in(None);
        assert_eq!(current_of(all(), "rose pine", false).name, "Rose Pine");
        assert_eq!(current_of(all(), DEFAULT, true).name, "blitz light");
        assert_eq!(
            current_of(all(), "light:Gruvbox Light,dark:x", true).name,
            "Gruvbox Light"
        );
        for light in [false, true] {
            let want = if light { "blitz light" } else { "blitz dark" };
            assert_eq!(current_of(all(), "nope", light).name, want);
            assert_eq!(current_of(all(), "light:x,dark:y", light).name, want);
            assert_eq!(current_of(Vec::new(), "nope", light).name, want);
        }
    }

    /// Windows 11's contrast themes as (window, text, highlight): Aquatic,
    /// Desert, Dusk and Night sky.
    const CONTRAST_THEMES: [[u32; 3]; 4] = [
        [0x202020, 0xffffff, 0x8ee3f0],
        [0xfffaef, 0x3d3d3d, 0x903909],
        [0x2d3236, 0xb6f6f0, 0xa1bfde],
        [0x000000, 0xffffff, 0xd6b4fd],
    ];

    #[test]
    fn high_contrast_takes_the_windows_colours() {
        for c in CONTRAST_THEMES {
            let t = high_contrast(c);
            assert_eq!(t.name, HIGH_CONTRAST);
            assert_eq!((t.pal.bg, t.pal.fg, t.ui.accent), (c[0], c[1], c[2]));
            assert_eq!(t.light, c[0] == 0xfffaef, "{c:06x?}");
            let base = if t.light { light() } else { dark() };
            assert_eq!(t.pal.ansi, base.ansi);
            let ui = &t.ui;
            for (what, fg) in [("title", ui.name), ("label", ui.label), ("dim", ui.dim)] {
                let on = contrast(fg, ui.side_bg);
                assert!(on >= 3.0, "{c:06x?}: {what} on the sidebar {on:.2}");
            }
            let chip = contrast(ui.chip_fg, ui.accent);
            assert!(chip >= 4.5, "{c:06x?}: text on the accent {chip:.2}");
        }
        // A theme set by hand wins over the mode.
        assert_eq!(contrast_for(DEFAULT), system_contrast());
        assert_eq!(contrast_for("Rose Pine"), None);
        assert_eq!(contrast_for(&pick(DEFAULT, "Rose Pine", true, false)), None);
    }

    #[test]
    fn readable_moves_text_just_far_enough_from_its_background() {
        let d = dark();
        // Palette 0 on the blitz dark background, near-black text on black
        // and grey on white all come out at 1.3:1, a step past it at most.
        for (fg, bg) in [
            (d.ansi[0], d.bg),
            (0x000000, 0x000001),
            (0x101010, 0x000000),
            (0xf0f0f0, 0xffffff),
            (0x777777, 0x808080),
            (0x808080, 0x777777),
        ] {
            let r = readable(fg, bg, 1.3);
            let c = contrast(r, bg);
            assert!(
                (1.3..1.36).contains(&c),
                "{fg:06x} on {bg:06x}: {r:06x} {c:.3}"
            );
        }
        // Lighter text gets lighter and darker text darker, where there
        // is room.
        assert!(luminance(readable(0x101010, 0x000000, 1.3)) > luminance(0x101010));
        assert!(luminance(readable(0xf0f0f0, 0xffffff, 1.3)) < luminance(0xf0f0f0));
        assert!(luminance(readable(0x000000, 0x020202, 1.3)) > luminance(0x020202));
        // Text that already shows keeps its colour; a dark red stays red.
        assert_eq!(readable(d.fg, d.bg, 1.3), d.fg);
        assert_eq!(readable(0x2e7a45, 0xfcfcfb, 1.3), 0x2e7a45);
        let [_, r, g, b] = readable(0x300000, 0x000000, 1.3).to_be_bytes();
        assert!(r > g && g == b, "{r} {g} {b}");
    }

    #[test]
    fn mix_moves_each_channel() {
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
        assert_eq!(mix(0x102030, 0x102030, 0.7), 0x102030);
        assert_eq!(mix(0xff0000, 0x0000ff, 1.0), 0x0000ff);
    }
}
