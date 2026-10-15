//! Font loading, cell metrics, fallback and glyph rasterization for macOS
//! and Linux: what font.rs does with DirectWrite on Windows, with the
//! installed fonts found by fontdb and drawn by swash.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::{Format, Transform};
use swash::{FontRef, GlyphId};

use super::{Error, Result};

/// Style bits for [`Font::raster`].
pub const BOLD: u8 = 1;
pub const ITALIC: u8 = 2;

/// Families tried in order when the configured one is not installed.
#[cfg(target_os = "macos")]
pub const DEFAULT_FAMILIES: &[&str] = &["SF Mono", "Menlo", "Monaco", "Courier New"];
#[cfg(not(target_os = "macos"))]
pub const DEFAULT_FAMILIES: &[&str] = &[
    "DejaVu Sans Mono",
    "Noto Sans Mono",
    "Liberation Mono",
    "Ubuntu Mono",
    "FreeMono",
];

/// Families a character the font lacks is looked for in, in order, after
/// an installed Nerd Font for icons.
#[cfg(target_os = "macos")]
const FALLBACK_FAMILIES: &[&str] = &[
    "Menlo",
    "Apple Symbols",
    "Apple Color Emoji",
    "Arial Unicode MS",
    "PingFang SC",
    "Hiragino Sans",
    "Apple SD Gothic Neo",
    "STIX Two Math",
];
#[cfg(not(target_os = "macos"))]
const FALLBACK_FAMILIES: &[&str] = &[
    "DejaVu Sans Mono",
    "DejaVu Sans",
    "Noto Sans Mono",
    "Noto Sans Symbols",
    "Noto Sans Symbols 2",
    "Noto Sans CJK SC",
    "Noto Sans CJK JP",
    "Noto Color Emoji",
    "Noto Sans Math",
    "FreeSerif",
    "FreeMono",
];

/// What [`Font::raster`] fails with when it has no fallback lookups left.
#[derive(Debug)]
struct Pending;

impl std::fmt::Display for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("font lookups are used up for this frame")
    }
}

impl std::error::Error for Pending {}

/// Whether `e` is [`Font::raster`] out of fallback lookups for the frame.
pub fn is_pending(e: &Error) -> bool {
    e.is::<Pending>()
}

/// Coverage of a rasterized glyph, placed relative to the top-left corner
/// of its first cell.
pub struct Raster {
    pub w: u32,
    pub h: u32,
    pub dx: i32,
    pub dy: i32,
    pub alpha: Vec<u8>,
}

/// The installed fonts, found once: that reads every font file's header.
fn db() -> &'static fontdb::Database {
    static DB: OnceLock<fontdb::Database> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db
    })
}

/// The contents of font files, read once each and kept: a face points
/// into them.
fn file(path: &PathBuf) -> Option<Arc<[u8]>> {
    static FILES: OnceLock<Mutex<HashMap<PathBuf, Arc<[u8]>>>> = OnceLock::new();
    let files = FILES.get_or_init(Default::default);
    let mut files = files
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(d) = files.get(path) {
        return Some(d.clone());
    }
    let data: Arc<[u8]> = std::fs::read(path).ok()?.into();
    files.insert(path.clone(), data.clone());
    Some(data)
}

/// One face of a font file, with what drawing it in a style still needs.
#[derive(Clone)]
struct Face {
    data: Arc<[u8]>,
    index: usize,
    /// The face is not bold or italic itself, so the style is drawn by
    /// thickening or slanting it.
    embolden: bool,
    slant: bool,
}

impl Face {
    /// Whether `o` is this face drawn the same way. Files are loaded once,
    /// so the same file is the same data.
    fn same_as(&self, o: &Face) -> bool {
        Arc::ptr_eq(&self.data, &o.data)
            && (self.index, self.embolden, self.slant) == (o.index, o.embolden, o.slant)
    }

    fn font(&self) -> Option<FontRef<'_>> {
        FontRef::from_index(&self.data, self.index)
    }

    fn glyph(&self, c: char) -> GlyphId {
        self.font().map_or(0, |f| f.charmap().map(c))
    }
}

/// The face of `family` closest to `style`, if it is installed.
fn face_of(family: &str, style: u8) -> Option<Face> {
    let db = db();
    let query = fontdb::Query {
        families: &[fontdb::Family::Name(family)],
        weight: if style & BOLD != 0 {
            fontdb::Weight::BOLD
        } else {
            fontdb::Weight::NORMAL
        },
        stretch: fontdb::Stretch::Normal,
        style: if style & ITALIC != 0 {
            fontdb::Style::Italic
        } else {
            fontdb::Style::Normal
        },
    };
    let id = db.query(&query)?;
    let info = db.face(id)?;
    // The query falls back to any face of the family; only a face that
    // really is bold or italic is drawn as it is.
    let data = match &info.source {
        fontdb::Source::File(path) => file(path)?,
        fontdb::Source::Binary(data) => data.as_ref().as_ref().into(),
    };
    let face = Face {
        data,
        index: info.index as usize,
        embolden: style & BOLD != 0 && info.weight < fontdb::Weight::SEMIBOLD,
        slant: style & ITALIC != 0 && info.style == fontdb::Style::Normal,
    };
    face.font().is_some().then_some(face)
}

/// The face fontconfig picks for `c` when no family listed has it, as an
/// installed CJK font none of [`FALLBACK_FAMILIES`] names.
#[cfg(not(target_os = "macos"))]
fn covering(c: char, style: u8) -> Option<Face> {
    let pattern = format!("monospace:charset={:x}", u32::from(c));
    let mut cmd = std::process::Command::new(crate::desktop::tool("fc-match")?);
    cmd.args(["-f", "%{file}\n%{index}", &pattern]);
    let out = crate::desktop::output_within(&mut cmd, std::time::Duration::from_secs(1), 4096)?;
    let out = String::from_utf8(out).ok()?;
    let (path, index) = out.split_once('\n')?;
    let face = Face {
        data: file(&PathBuf::from(path))?,
        index: index.trim().parse().ok()?,
        embolden: style & BOLD != 0,
        slant: style & ITALIC != 0,
    };
    (face.font().is_some() && face.glyph(c) != 0).then_some(face)
}

/// What asking fontconfig takes from [`Font::lookups`]: a renderer's 256
/// a frame allow two asks.
const ASK_COST: u32 = 100;

/// How many times one font asks fontconfig in all.
const MAX_ASKS: u32 = 256;

/// Whether fontconfig might find a font for `c` that the lists lack: not on
/// a Mac, and not for a private character, which only a Nerd Font draws,
/// nor a noncharacter, which none does.
fn worth_asking(c: char) -> bool {
    let n = u32::from(c);
    let nonchar = (0xFDD0..=0xFDEF).contains(&n) || n & 0xFFFE == 0xFFFE;
    cfg!(not(target_os = "macos")) && !is_private(c) && !nonchar
}

/// The Mac's fallback list covers what it ships with.
#[cfg(target_os = "macos")]
fn covering(_c: char, _style: u8) -> Option<Face> {
    None
}

/// The names of the font families installed, sorted, each with whether it
/// is fixed-width.
pub fn families() -> &'static [(String, bool)] {
    static FAMILIES: OnceLock<Vec<(String, bool)>> = OnceLock::new();
    FAMILIES.get_or_init(|| {
        let mut out: Vec<(String, bool)> = (db().faces())
            .filter_map(|f| Some((f.families.first()?.0.clone(), f.monospaced)))
            .collect();
        out.sort_by_key(|n| n.0.to_lowercase());
        // A family is fixed-width when its first face listed is.
        out.dedup_by(|a, b| a.0 == b.0);
        out
    })
}

fn is_private(c: char) -> bool {
    matches!(c, '\u{E000}'..='\u{F8FF}' | '\u{F0000}'..='\u{FFFFD}' | '\u{100000}'..='\u{10FFFD}')
}

/// The installed Nerd Font, for icons in the private use area.
fn nerd_family() -> Option<&'static str> {
    static NERD: OnceLock<Option<String>> = OnceLock::new();
    NERD.get_or_init(|| {
        let fams = families();
        let nerd = |mono: bool| {
            (fams.iter())
                .find(|(n, m)| *m == mono && n.contains("Nerd Font"))
                .map(|(n, _)| n.clone())
        };
        nerd(true).or_else(|| nerd(false))
    })
    .as_deref()
}

pub struct Font {
    /// Regular, bold, italic, bold italic.
    faces: [Face; 4],
    px: f32,
    pub cell_w: u32,
    pub cell_h: u32,
    /// Baseline, from the top of the cell.
    pub baseline: i32,
    /// Top of the underline, from the top of the cell.
    pub underline_y: i32,
    pub underline_h: u32,
    /// Top of the strikethrough line, from the top of the cell.
    pub strike_y: i32,
    pub strike_h: u32,
    /// Text gamma and grayscale contrast boost, applied by the shader.
    pub gamma: f32,
    pub contrast: f32,
    fallbacks: HashMap<(char, u8), Option<Face>>,
    /// Faces fontconfig found, tried before asking it again.
    found: Vec<Face>,
    /// Times fontconfig was asked, up to [`MAX_ASKS`].
    asked: u32,
    scaler: ScaleContext,
    /// Fallback lookups [`Font::raster`] may still make before it fails
    /// as pending; a renderer allows a few per frame.
    pub lookups: u32,
}

impl Font {
    /// Loads the first installed family of `families` at `px` pixels per em.
    pub fn new(families: &[&str], px: f32) -> Result<Self> {
        let family = (families.iter())
            .find(|f| face_of(f, 0).is_some())
            .ok_or_else(|| format!("none of the fonts {families:?} is installed"))?;
        let face = |style| face_of(family, style).ok_or("a face went missing");
        let faces = [face(0)?, face(BOLD)?, face(ITALIC)?, face(BOLD | ITALIC)?];
        let font = faces[0].font().ok_or("unreadable font")?;
        let m = font.metrics(&[]).scale(px);
        let zero = font.charmap().map('0');
        let advance = font.glyph_metrics(&[]).scale(px).advance_width(zero);
        let cell_w = advance.round().max(1.0) as u32;
        let baseline = (m.leading.max(0.0) / 2.0 + m.ascent).round() as i32;
        let cell_h =
            ((m.ascent + m.descent + m.leading.max(0.0)).round() as i32).max(baseline + 1) as u32;
        let underline_h = m.stroke_size.round().max(1.0) as u32;
        let underline_y =
            (baseline - m.underline_offset.round() as i32).min(cell_h as i32 - underline_h as i32);
        let strike_h = underline_h;
        let strike_up = match m.strikeout_offset {
            0.0 => m.x_height / 2.0 + strike_h as f32 / 2.0,
            p => p,
        };
        let strike_y = (baseline - strike_up.round() as i32)
            .min(baseline - 1)
            .max(0);
        Ok(Self {
            faces,
            px,
            cell_w,
            cell_h,
            baseline,
            underline_y,
            underline_h,
            strike_y,
            strike_h,
            gamma: 1.8,
            contrast: 1.0,
            fallbacks: HashMap::new(),
            found: Vec::new(),
            asked: 0,
            scaler: ScaleContext::new(),
            lookups: u32::MAX,
        })
    }

    /// Spaces lines `k` times as far apart as the font does, with the text
    /// centred in its taller or shorter cell.
    pub fn set_line_height(&mut self, k: f32) {
        let h = ((self.cell_h as f32 * k).round() as i32).max(2);
        let shift = (h - self.cell_h as i32) / 2;
        self.cell_h = h as u32;
        self.baseline = (self.baseline + shift).min(h - 1).max(1);
        self.underline_y = (self.underline_y + shift)
            .min(h - self.underline_h as i32)
            .max(0);
        self.strike_y = (self.strike_y + shift).min(self.baseline - 1).max(0);
    }

    /// Rasterizes one grapheme cluster spanning `width` cells. Returns
    /// `None` when it has no ink. Characters missing from the font come
    /// from a fallback font, with their ink shrunk to fit their cells if
    /// needed and centred on them.
    pub fn raster(&mut self, text: &str, style: u8, width: u8) -> Result<Option<Raster>> {
        // No shaping, as on Windows: the base character plus the marks the
        // same font has; an emoji ZWJ sequence shows its first emoji, and
        // a skin tone is left out.
        let chars: Vec<char> = if text.contains('\u{200D}') {
            text.chars().take(1).collect()
        } else {
            (text.chars().enumerate())
                .filter(|&(i, c)| {
                    !matches!(c, '\u{FE0E}' | '\u{FE0F}')
                        && !(i > 0 && matches!(c, '\u{1F3FB}'..='\u{1F3FF}'))
                })
                .map(|(_, c)| c)
                .collect()
        };
        let Some(&first) = chars.first() else {
            return Ok(None);
        };
        let style = style & (BOLD | ITALIC);
        let own = self.faces[usize::from(style)].clone();
        let (face, primary) = if own.glyph(first) != 0 {
            (own, true)
        } else {
            match self.fallback_face(first, style)? {
                Some(f) => (f, false),
                None => (own, true),
            }
        };
        let Some(font) = face.font() else {
            return Ok(None);
        };
        let gm = font.glyph_metrics(&[]).scale(self.px);
        // Each glyph with its pen position: a combining mark starts where
        // the glyph before it does, a flag's second letter after its first.
        let mut glyphs = vec![(font.charmap().map(first), 0.0f32)];
        let mut pen = 0.0;
        for &c in &chars[1..] {
            let g = font.charmap().map(c);
            if g != 0 {
                if vt::width::char_width(c) > 0 {
                    pen += gm.advance_width(glyphs[glyphs.len() - 1].0);
                }
                glyphs.push((g, pen));
            }
        }
        let cells = (self.cell_w * u32::from(width.max(1))) as f32;
        // A colour glyph, as an emoji, draws as its shape in the text's
        // colour.
        let sources = [
            Source::Outline,
            Source::Bitmap(swash::scale::StrikeWith::BestFit),
            Source::ColorOutline(0),
            Source::ColorBitmap(swash::scale::StrikeWith::BestFit),
        ];
        let transform = face.slant.then(|| {
            Transform::skew(
                swash::zeno::Angle::from_degrees(14.0),
                swash::zeno::Angle::ZERO,
            )
        });
        let embolden = if face.embolden { self.px / 24.0 } else { 0.0 };
        let mut draw = |em: f32| {
            let mut scaler = self.scaler.builder(font).size(em).hint(true).build();
            let k = em / self.px;
            let mut images = Vec::new();
            for &(g, x) in &glyphs {
                let mut render = Render::new(&sources);
                render
                    .format(Format::Alpha)
                    .transform(transform)
                    .embolden(embolden)
                    .offset(swash::zeno::Vector::new((x * k).fract(), 0.0));
                if let Some(img) = render.render(&mut scaler, g) {
                    images.push((img, (x * k).floor() as i32));
                }
            }
            images
        };
        let mut images = draw(self.px);
        // The ink box of all the glyphs, in pixels from the pen, y down.
        let bounds = |images: &[(swash::scale::image::Image, i32)]| {
            images.iter().filter(|(i, _)| i.placement.width > 0).fold(
                (i32::MAX, i32::MAX, i32::MIN, i32::MIN),
                |(l, t, r, b), (i, x)| {
                    let p = i.placement;
                    (
                        l.min(x + p.left),
                        t.min(-p.top),
                        r.max(x + p.left + p.width as i32),
                        b.max(-p.top + p.height as i32),
                    )
                },
            )
        };
        let (l, t, r, b) = bounds(&images);
        if l >= r || t >= b {
            return Ok(None);
        }
        let mut shift = 0;
        if !primary || width > 1 {
            // Fit and centre the ink, which keeps a fallback symbol with a
            // wide advance from shrinking to a dot; the baseline stays.
            let (ink_w, ink_h) = ((r - l) as f32, (b - t) as f32);
            let k = (cells / ink_w).min(self.cell_h as f32 / ink_h).min(1.0);
            if k < 1.0 {
                images = draw(self.px * k);
            }
            let (l, _, r, _) = bounds(&images);
            shift = ((cells - (r - l) as f32) / 2.0).round() as i32 - l;
        }
        let (l, t, r, b) = bounds(&images);
        let (w, h) = ((r - l) as u32, (b - t) as u32);
        let mut alpha = vec![0u8; (w * h) as usize];
        for (img, x) in &images {
            let p = img.placement;
            let (ox, oy) = (x + p.left - l, -p.top - t);
            // A colour image's coverage is its alpha.
            let (step, at) = match img.content {
                swash::scale::image::Content::Mask => (1, 0),
                swash::scale::image::Content::Color => (4, 3),
                swash::scale::image::Content::SubpixelMask => (4, 1),
            };
            for row in 0..p.height as i32 {
                for col in 0..p.width as i32 {
                    let src = img.data[(row * p.width as i32 + col) as usize * step + at];
                    let i = ((oy + row) * w as i32 + ox + col) as usize;
                    alpha[i] = alpha[i].max(src);
                }
            }
        }
        Ok(Some(Raster {
            w,
            h,
            dx: l + shift,
            dy: self.baseline + t,
            alpha,
        }))
    }

    /// The face for `c` when the font lacks it, cached: an installed Nerd
    /// Font for an icon, else the first fallback family that has it.
    fn fallback_face(&mut self, c: char, style: u8) -> Result<Option<Face>> {
        if let Some(f) = self.fallbacks.get(&(c, style)) {
            return Ok(f.clone());
        }
        if self.lookups == 0 {
            return Err(Pending.into());
        }
        self.lookups -= 1;
        let nerd = is_private(c).then(nerd_family).flatten();
        let listed = (nerd.into_iter().chain(FALLBACK_FAMILIES.iter().copied()))
            .filter_map(|f| face_of(f, style))
            .chain(self.found.iter().cloned())
            .find(|f| f.glyph(c) != 0);
        let face = match listed {
            Some(face) => Some(face),
            // Asking fontconfig starts a process, so output full of
            // characters no font has could stall every frame: a few asks
            // a frame and a few hundred in all, none for a character no
            // font is likely to have, and each face it finds is tried
            // before asking again.
            None if worth_asking(c) && self.asked < MAX_ASKS => {
                if self.lookups < ASK_COST {
                    return Err(Pending.into());
                }
                self.lookups -= ASK_COST;
                self.asked += 1;
                let face = covering(c, style);
                if let Some(f) = &face
                    && !self.found.iter().any(|g| g.same_as(f))
                {
                    self.found.push(f.clone());
                }
                face
            }
            None => None,
        };
        self.fallbacks.insert((c, style), face.clone());
        Ok(face)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> Option<Font> {
        match Font::new(DEFAULT_FAMILIES, 16.0) {
            Ok(f) => Some(f),
            Err(e) => {
                println!("SKIPPED: {e}");
                None
            }
        }
    }

    /// A character only an unlisted or colour font has still draws, when
    /// one is installed.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn any_installed_font_that_has_it_draws_it() {
        let Some(mut f) = font() else { return };
        for (c, hex) in [("中", "4e2d"), ("👍", "1f44d")] {
            let listed = std::process::Command::new("fc-list")
                .arg(format!(":charset={hex}"))
                .output()
                .is_ok_and(|o| !o.stdout.is_empty());
            if !listed {
                println!("SKIPPED: no font has {c}");
                continue;
            }
            f.lookups = 1 + ASK_COST;
            let r = f.raster(c, 0, 2).expect("raster");
            assert!(r.is_some_and(|r| r.alpha.iter().any(|&a| a > 200)), "{c}");
        }
    }

    #[test]
    fn a_cell_holds_a_glyph() {
        let Some(mut f) = font() else { return };
        assert!(
            f.cell_w >= 6 && f.cell_h > f.cell_w,
            "{}x{}",
            f.cell_w,
            f.cell_h
        );
        assert!(f.baseline > 0 && f.baseline < f.cell_h as i32);
        let r = f.raster("M", 0, 1).expect("raster").expect("ink");
        assert!(r.w > 0 && r.w <= f.cell_w + 2 && r.h > 0);
        // The glyph sits on the baseline, inside its cell.
        assert!(
            r.dy >= 0 && r.dy + r.h as i32 <= f.baseline + 1,
            "{} {}",
            r.dy,
            r.h
        );
        assert!(r.alpha.iter().any(|&a| a > 200));
        assert!(f.raster(" ", 0, 1).expect("raster").is_none());
    }

    #[test]
    fn bold_is_heavier_than_regular() {
        let Some(mut f) = font() else { return };
        let ink = |r: Raster| r.alpha.iter().map(|&a| u32::from(a)).sum::<u32>();
        let regular = ink(f.raster("H", 0, 1).expect("raster").expect("ink"));
        let bold = ink(f.raster("H", BOLD, 1).expect("raster").expect("ink"));
        assert!(bold > regular, "{bold} <= {regular}");
    }

    #[test]
    fn lookups_run_out_and_wait_for_the_next_frame() {
        let Some(mut f) = font() else { return };
        f.lookups = 0;
        // A character no monospace font has, so a fallback is looked for.
        let e = f.raster("\u{10FFFD}", 0, 1).err().expect("pending");
        assert!(is_pending(&e));
        f.lookups = 1;
        assert!(f.raster("\u{10FFFD}", 0, 1).is_ok());
    }

    #[test]
    fn fontconfig_is_asked_a_few_times_a_frame_and_never_for_hopeless_characters() {
        assert!(!worth_asking('\u{F0001}') && !worth_asking('\u{FDD0}'));
        assert!(!worth_asking('\u{1FFFF}') && !worth_asking('\u{10FFFE}'));
        assert_eq!(worth_asking('中'), cfg!(not(target_os = "macos")));
        let Some(mut f) = font() else { return };
        // Unassigned, so nothing has it, but worth one ask.
        let unknown = ['\u{2FFF0}', '\u{2FFF1}', '\u{2FFF2}'];
        if !worth_asking(unknown[0]) {
            return;
        }
        f.lookups = 256;
        let pending = (unknown.iter())
            .map(|c| f.raster(&c.to_string(), 0, 1))
            .filter(|r| r.as_ref().is_err_and(is_pending))
            .count();
        assert_eq!(pending, 1, "two asks fit in a frame");
        // Past the cap, a miss is drawn as missing without asking, so a
        // single lookup is enough.
        f.asked = MAX_ASKS;
        f.lookups = 1;
        assert!(f.raster("\u{2FFF3}", 0, 1).is_ok());
    }

    #[test]
    fn line_height_moves_the_baseline_with_the_cell() {
        let Some(mut f) = font() else { return };
        let (h, base) = (f.cell_h as i32, f.baseline);
        f.set_line_height(1.5);
        let grown = f.cell_h as i32 - h;
        assert!(grown > 0);
        assert_eq!(f.baseline, base + grown / 2);
    }
}
