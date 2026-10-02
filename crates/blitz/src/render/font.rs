//! DirectWrite font loading, cell metrics, fallback and glyph rasterization.

use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::ManuallyDrop;

use windows::Win32::Foundation::{E_FAIL, E_NOINTERFACE, S_OK};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_METRICS, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE,
    DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_GLYPH_METRICS, DWRITE_GLYPH_RUN,
    DWRITE_GRID_FIT_MODE_DEFAULT, DWRITE_MEASURING_MODE_NATURAL, DWRITE_READING_DIRECTION,
    DWRITE_READING_DIRECTION_LEFT_TO_RIGHT, DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC,
    DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE, DWRITE_TEXTURE_ALIASED_1x1, DWriteCreateFactory,
    IDWriteFactory2, IDWriteFontCollection, IDWriteFontFace, IDWriteFontFallback,
    IDWriteTextAnalysisSource,
};
use windows::core::{BOOL, GUID, HRESULT, IUnknown, Interface, PCWSTR, Result, w};

/// Style bits for [`Font::raster`].
pub const BOLD: u8 = 1;
pub const ITALIC: u8 = 2;

/// Families tried in order; Consolas ships with every Windows.
pub const DEFAULT_FAMILIES: &[&str] = &["Cascadia Mono", "Consolas", "Courier New"];

/// Coverage of a rasterized glyph, placed relative to the top-left corner
/// of its first cell.
pub struct Raster {
    pub w: u32,
    pub h: u32,
    pub dx: i32,
    pub dy: i32,
    pub alpha: Vec<u8>,
}

pub struct Font {
    factory: IDWriteFactory2,
    collection: IDWriteFontCollection,
    fallback: IDWriteFontFallback,
    /// NUL-terminated family name.
    family: Vec<u16>,
    /// Regular, bold, italic, bold italic.
    faces: [IDWriteFontFace; 4],
    px: f32,
    pub cell_w: u32,
    pub cell_h: u32,
    /// Baseline, from the top of the cell.
    pub baseline: i32,
    /// Top of the underline, from the top of the cell.
    pub underline_y: i32,
    pub underline_h: u32,
    fallbacks: HashMap<(char, u8), Option<IDWriteFontFace>>,
}

fn weight_style(style: u8) -> (DWRITE_FONT_WEIGHT, DWRITE_FONT_STYLE) {
    let weight = if style & BOLD != 0 {
        DWRITE_FONT_WEIGHT_BOLD
    } else {
        DWRITE_FONT_WEIGHT_NORMAL
    };
    let slant = if style & ITALIC != 0 {
        DWRITE_FONT_STYLE_ITALIC
    } else {
        DWRITE_FONT_STYLE_NORMAL
    };
    (weight, slant)
}

fn glyph_index(face: &IDWriteFontFace, c: char) -> u16 {
    let cp = c as u32;
    let mut g = 0u16;
    // SAFETY: one code point in, one glyph index out.
    let _ = unsafe { face.GetGlyphIndices(&cp, 1, &mut g) };
    g
}

impl Font {
    /// Loads the first installed family of `families` at `px` pixels per em.
    pub fn new(families: &[&str], px: f32) -> Result<Self> {
        // SAFETY: COM calls with valid out-pointers and NUL-terminated names.
        unsafe {
            let factory: IDWriteFactory2 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let mut collection = None;
            factory.GetSystemFontCollection(&mut collection, false)?;
            let collection = collection.ok_or(windows::core::Error::from(E_FAIL))?;
            let mut found = None;
            for name in families {
                let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
                let (mut index, mut exists) = (0u32, BOOL(0));
                collection.FindFamilyName(PCWSTR(wide.as_ptr()), &mut index, &mut exists)?;
                if exists.as_bool() {
                    found = Some((wide, index));
                    break;
                }
            }
            let (family, index) = found.ok_or(windows::core::Error::from(E_FAIL))?;
            let fam = collection.GetFontFamily(index)?;
            let face = |style| -> Result<IDWriteFontFace> {
                let (weight, slant) = weight_style(style);
                fam.GetFirstMatchingFont(weight, DWRITE_FONT_STRETCH_NORMAL, slant)?
                    .CreateFontFace()
            };
            let faces = [face(0)?, face(BOLD)?, face(ITALIC)?, face(BOLD | ITALIC)?];
            let fallback = factory.GetSystemFontFallback()?;

            let mut m = DWRITE_FONT_METRICS::default();
            faces[0].GetMetrics(&mut m);
            let scale = px / f32::from(m.designUnitsPerEm.max(1));
            let zero = glyph_index(&faces[0], '0');
            let mut gm = DWRITE_GLYPH_METRICS::default();
            faces[0].GetDesignGlyphMetrics(&zero, 1, &mut gm, false)?;
            let cell_w = (gm.advanceWidth as f32 * scale).round().max(1.0) as u32;
            let ascent = f32::from(m.ascent) * scale;
            let descent = f32::from(m.descent) * scale;
            let gap = f32::from(m.lineGap.max(0)) * scale;
            let baseline = (gap / 2.0 + ascent).round() as i32;
            let cell_h = ((ascent + descent + gap).round() as i32).max(baseline + 1) as u32;
            let underline_h = (f32::from(m.underlineThickness) * scale).round().max(1.0) as u32;
            let underline_y = (baseline - (f32::from(m.underlinePosition) * scale).round() as i32)
                .min(cell_h as i32 - underline_h as i32);
            Ok(Self {
                factory,
                collection,
                fallback,
                family,
                faces,
                px,
                cell_w,
                cell_h,
                baseline,
                underline_y,
                underline_h,
                fallbacks: HashMap::new(),
            })
        }
    }

    /// Rasterizes one grapheme cluster spanning `width` cells. Returns
    /// `None` when it has no ink. Characters missing from the font come
    /// from the system fallback font, shrunk to fit their cells and
    /// centred on them.
    pub fn raster(&mut self, text: &str, style: u8, width: u8) -> Result<Option<Raster>> {
        // No shaping. A cluster is drawn as its base character
        // plus the marks the same font has; emoji ZWJ sequences show their
        // first emoji only (color emoji are not drawn as color anyway).
        let chars: Vec<char> = if text.contains('\u{200D}') {
            text.chars().take(1).collect()
        } else {
            text.chars()
                .filter(|c| !matches!(c, '\u{FE0E}' | '\u{FE0F}'))
                .collect()
        };
        let Some(&first) = chars.first() else {
            return Ok(None);
        };
        let style = style & (BOLD | ITALIC);
        let own = self.faces[usize::from(style)].clone();
        let (face, primary) = if glyph_index(&own, first) != 0 {
            (own, true)
        } else {
            match self.fallback_face(first, style)? {
                Some(f) => (f, false),
                None => (own, true),
            }
        };
        let mut glyphs = vec![glyph_index(&face, first)];
        glyphs.extend(
            chars[1..]
                .iter()
                .map(|&c| glyph_index(&face, c))
                .filter(|&g| g != 0),
        );
        let n = glyphs.len();
        let mut gm = vec![DWRITE_GLYPH_METRICS::default(); n];
        let mut fm = DWRITE_FONT_METRICS::default();
        // SAFETY: `gm` has room for `n` metrics.
        unsafe {
            face.GetDesignGlyphMetrics(glyphs.as_ptr(), n as u32, gm.as_mut_ptr(), false)?;
            face.GetMetrics(&mut fm);
        }
        let upem = f32::from(fm.designUnitsPerEm.max(1));
        let advance = gm[0].advanceWidth as f32 * self.px / upem;
        let cells = (self.cell_w * u32::from(width.max(1))) as f32;
        let (em, x) = if primary && width <= 1 || advance <= 0.0 {
            (self.px, 0.0)
        } else {
            let k = (cells / advance).min(1.0);
            (self.px * k, ((cells - advance * k) / 2.0).round())
        };
        let advances: Vec<f32> = gm
            .iter()
            .map(|g| g.advanceWidth as f32 * em / upem)
            .collect();
        let run = DWRITE_GLYPH_RUN {
            fontFace: ManuallyDrop::new(Some(face)),
            fontEmSize: em,
            glyphCount: n as u32,
            glyphIndices: glyphs.as_ptr(),
            glyphAdvances: advances.as_ptr(),
            ..Default::default()
        };
        // SAFETY: `run` points at `glyphs` and `advances`, which outlive
        // the analysis call.
        let analysis = unsafe {
            self.factory.CreateGlyphRunAnalysis(
                &run,
                None,
                DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC,
                DWRITE_MEASURING_MODE_NATURAL,
                DWRITE_GRID_FIT_MODE_DEFAULT,
                DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                x,
                self.baseline as f32,
            )
        };
        drop(ManuallyDrop::into_inner(run.fontFace));
        let analysis = analysis?;
        // SAFETY: `alpha` is exactly the size of the bounds it is filled for.
        unsafe {
            let r = analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1)?;
            let (w, h) = (
                (r.right - r.left).max(0) as u32,
                (r.bottom - r.top).max(0) as u32,
            );
            if w == 0 || h == 0 {
                return Ok(None);
            }
            let mut alpha = vec![0u8; (w * h) as usize];
            analysis.CreateAlphaTexture(DWRITE_TEXTURE_ALIASED_1x1, &r, &mut alpha)?;
            Ok(Some(Raster {
                w,
                h,
                dx: r.left,
                dy: r.top,
                alpha,
            }))
        }
    }

    /// The system's fallback font face for `c`, cached.
    fn fallback_face(&mut self, c: char, style: u8) -> Result<Option<IDWriteFontFace>> {
        if let Some(f) = self.fallbacks.get(&(c, style)) {
            return Ok(f.clone());
        }
        let mut text = [0u16; 2];
        let len = c.encode_utf16(&mut text).len() as u32;
        let source = Source {
            vtbl: &SOURCE_VTBL,
            text,
            len,
        };
        let raw = (&raw const source).cast_mut().cast::<c_void>();
        let (weight, slant) = weight_style(style);
        let (mut mapped, mut font, mut scale) = (0u32, None, 0f32);
        // SAFETY: `source` lives on this stack frame for the whole call and
        // DirectWrite does not keep it afterwards.
        let face = unsafe {
            let src = IDWriteTextAnalysisSource::from_raw_borrowed(&raw)
                .ok_or(windows::core::Error::from(E_FAIL))?;
            self.fallback.MapCharacters(
                src,
                0,
                len,
                &self.collection,
                PCWSTR(self.family.as_ptr()),
                weight,
                slant,
                DWRITE_FONT_STRETCH_NORMAL,
                &mut mapped,
                &mut font,
                &mut scale,
            )?;
            match font {
                Some(f) => Some(f.CreateFontFace()?),
                None => None,
            }
        };
        let face = face.filter(|f| glyph_index(f, c) != 0);
        self.fallbacks.insert((c, style), face.clone());
        Ok(face)
    }
}

/// A minimal `IDWriteTextAnalysisSource` over one character, kept on the
/// stack for the duration of a `MapCharacters` call.
#[repr(C)]
struct Source {
    vtbl: &'static SourceVtbl,
    text: [u16; 2],
    len: u32,
}

#[repr(C)]
struct SourceVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    text_at: unsafe extern "system" fn(*mut c_void, u32, *mut *const u16, *mut u32) -> HRESULT,
    text_before: unsafe extern "system" fn(*mut c_void, u32, *mut *const u16, *mut u32) -> HRESULT,
    direction: unsafe extern "system" fn(*mut c_void) -> DWRITE_READING_DIRECTION,
    locale: unsafe extern "system" fn(*mut c_void, u32, *mut u32, *mut *const u16) -> HRESULT,
    numbers: unsafe extern "system" fn(*mut c_void, u32, *mut u32, *mut *mut c_void) -> HRESULT,
}

static SOURCE_VTBL: SourceVtbl = SourceVtbl {
    query_interface: source_qi,
    add_ref: source_ref,
    release: source_ref,
    text_at: source_text_at,
    text_before: source_text_before,
    direction: source_direction,
    locale: source_locale,
    numbers: source_numbers,
};

unsafe extern "system" fn source_qi(
    this: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    // SAFETY: DirectWrite passes valid pointers.
    unsafe {
        if *iid == IUnknown::IID || *iid == IDWriteTextAnalysisSource::IID {
            *out = this;
            S_OK
        } else {
            *out = std::ptr::null_mut();
            E_NOINTERFACE
        }
    }
}

/// The object lives on the caller's stack, so counting is a no-op.
unsafe extern "system" fn source_ref(_: *mut c_void) -> u32 {
    1
}

unsafe extern "system" fn source_text_at(
    this: *mut c_void,
    pos: u32,
    text: *mut *const u16,
    len: *mut u32,
) -> HRESULT {
    // SAFETY: `this` is a `Source`; the out-pointers are valid.
    unsafe {
        let s = &*this.cast::<Source>();
        if pos < s.len {
            *text = s.text.as_ptr().add(pos as usize);
            *len = s.len - pos;
        } else {
            *text = std::ptr::null();
            *len = 0;
        }
    }
    S_OK
}

unsafe extern "system" fn source_text_before(
    this: *mut c_void,
    pos: u32,
    text: *mut *const u16,
    len: *mut u32,
) -> HRESULT {
    // SAFETY: `this` is a `Source`; the out-pointers are valid.
    unsafe {
        let s = &*this.cast::<Source>();
        if pos > 0 && pos <= s.len {
            *text = s.text.as_ptr();
            *len = pos;
        } else {
            *text = std::ptr::null();
            *len = 0;
        }
    }
    S_OK
}

unsafe extern "system" fn source_direction(_: *mut c_void) -> DWRITE_READING_DIRECTION {
    DWRITE_READING_DIRECTION_LEFT_TO_RIGHT
}

unsafe extern "system" fn source_locale(
    this: *mut c_void,
    _pos: u32,
    len: *mut u32,
    locale: *mut *const u16,
) -> HRESULT {
    // SAFETY: `this` is a `Source`; the out-pointers are valid.
    unsafe {
        *len = (*this.cast::<Source>()).len;
        *locale = w!("en-us").as_ptr();
    }
    S_OK
}

unsafe extern "system" fn source_numbers(
    this: *mut c_void,
    _pos: u32,
    len: *mut u32,
    out: *mut *mut c_void,
) -> HRESULT {
    // SAFETY: `this` is a `Source`; the out-pointers are valid.
    unsafe {
        *len = (*this.cast::<Source>()).len;
        *out = std::ptr::null_mut();
    }
    S_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ink(r: &Raster) -> u32 {
        r.alpha.iter().map(|&a| u32::from(a)).sum()
    }

    #[test]
    fn metrics_and_ascii() {
        let mut font = Font::new(DEFAULT_FAMILIES, 16.0).expect("font");
        assert!((7..=12).contains(&font.cell_w), "cell_w {}", font.cell_w);
        assert!((15..=24).contains(&font.cell_h), "cell_h {}", font.cell_h);
        assert!(font.baseline > 0 && font.baseline < font.cell_h as i32);
        let a = font.raster("A", 0, 1).expect("raster").expect("ink");
        assert!(ink(&a) > 0);
        // The glyph sits inside its cell, on the baseline.
        assert!(a.dx >= 0 && a.dx + a.w as i32 <= font.cell_w as i32 + 1);
        assert!(a.dy >= 0 && a.dy + a.h as i32 <= font.baseline + 1);
        assert!(font.raster(" ", 0, 1).expect("raster").is_none());
        let bold = font.raster("A", BOLD, 1).expect("raster").expect("ink");
        assert!(ink(&bold) > ink(&a));
    }

    #[test]
    fn fallback_glyphs_fit_their_cells() {
        let mut font = Font::new(DEFAULT_FAMILIES, 16.0).expect("font");
        for (s, width) in [("\u{23F5}", 1), ("\u{2605}", 1), ("\u{25C9}", 1), ("中", 2)] {
            let r = font.raster(s, 0, width).expect("raster").expect(s);
            assert!(ink(&r) > 0, "{s}");
            let cells = (font.cell_w * u32::from(width)) as i32;
            assert!(r.dx >= -1 && r.dx + r.w as i32 <= cells + 1, "{s} spills");
        }
    }
}
