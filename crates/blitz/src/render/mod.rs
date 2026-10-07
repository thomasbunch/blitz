//! Turns terminal snapshots and window chrome into quads and draws them.

// One process hosts every session, so a failed HRESULT must never panic.
#![deny(clippy::unwrap_used)]

pub mod atlas;
pub mod builtin;
pub mod chrome;
#[cfg(windows)]
pub mod d3d11;
#[cfg(windows)]
pub mod font;

use std::io::Write as _;
use std::path::Path;

use vt::{Palette, RenderCell, Snapshot};

/// Creates `path` as a new file for a capture or log. Whatever is there is
/// removed first rather than opened, so a link planted at the path cannot
/// send the write to another file.
pub fn create_fresh(path: &Path) -> std::io::Result<std::fs::File> {
    let _ = std::fs::remove_file(path);
    std::fs::File::create_new(path)
}

/// Writes `w * h` BGRA pixels (top row first) as a 24-bit BMP.
pub fn write_bmp(path: &Path, w: u32, h: u32, bgra: &[u8]) -> std::io::Result<()> {
    let (w, h) = (w as usize, h as usize);
    if bgra.len() < w * h * 4 {
        return Err(std::io::Error::other("short pixel buffer"));
    }
    let row = (w * 3).next_multiple_of(4);
    let size = 54 + row * h;
    let mut out = Vec::with_capacity(size);
    out.extend(b"BM");
    out.extend((size as u32).to_le_bytes());
    out.extend([0; 4]);
    out.extend(54u32.to_le_bytes());
    out.extend(40u32.to_le_bytes());
    out.extend((w as i32).to_le_bytes());
    out.extend((h as i32).to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(24u16.to_le_bytes());
    out.extend([0; 24]);
    for y in (0..h).rev() {
        for px in bgra[y * w * 4..(y + 1) * w * 4].as_chunks::<4>().0 {
            out.extend(&px[..3]);
        }
        out.resize(out.len() + row - w * 3, 0);
    }
    create_fresh(path)?.write_all(&out)
}

/// A snapshot of plain text with no escape sequences: one line per row,
/// default colours. Used to check fonts without a terminal.
pub fn text_snapshot(text: &str, cols: u16, rows: u16, pal: &Palette) -> Snapshot {
    let blank = RenderCell {
        fg: pal.fg,
        bg: pal.bg,
        width: 1,
        ..RenderCell::default()
    };
    let mut cells = vec![blank; usize::from(cols) * usize::from(rows)];
    for (r, line) in text.lines().take(usize::from(rows)).enumerate() {
        let row = &mut cells[r * usize::from(cols)..(r + 1) * usize::from(cols)];
        let mut c = 0;
        for ch in line.chars() {
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf);
            let width = vt::cluster_width(s).max(1);
            if c + usize::from(width) > row.len() {
                break;
            }
            let mut text = [0u8; 16];
            text[..s.len()].copy_from_slice(s.as_bytes());
            row[c] = RenderCell {
                text,
                len: s.len() as u8,
                width,
                ..blank
            };
            if width == 2 {
                row[c + 1].width = 0;
            }
            c += usize::from(width);
        }
    }
    Snapshot {
        cols,
        rows,
        cells,
        ..Snapshot::default()
    }
}

/// `fg` a quarter of the way to `bg`: text in a pane without focus.
#[cfg(windows)]
fn toward(fg: u32, bg: u32) -> u32 {
    let ch = |s: u32| {
        let (f, b) = ((fg >> s & 0xff) as i32, (bg >> s & 0xff) as i32);
        ((b + (f - b) * 3 / 4) as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

/// `blitz debug render`: renders a terminal screen offscreen and writes it
/// to a BMP. Returns the process exit code.
pub fn debug_render(args: &[String]) -> i32 {
    #[cfg(windows)]
    match gpu::debug_render(args) {
        Ok(msg) => {
            println!("{msg}");
            0
        }
        Err(e) => {
            eprintln!("blitz debug render: {e}");
            1
        }
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        eprintln!("blitz debug render: only Windows is supported for now");
        2
    }
}

#[cfg(windows)]
pub use gpu::{Renderer, render_offscreen};

#[cfg(windows)]
mod gpu {
    use std::path::{Path, PathBuf};

    use vt::{CursorShape, Palette, RenderCell, Snapshot, snapshot::attr};
    use windows::Win32::Graphics::Direct3D11::ID3D11RenderTargetView;
    use windows::core::Result;

    use super::atlas::{Atlas, GlyphKey, Slot};
    use super::chrome::{Chrome, Prim, branch_mask, shape_mask};
    use super::d3d11::{ATLAS_SIZE, GLYPH, Gpu, MASK, Quad, SOLID, rgba};
    use super::font::{BOLD, DEFAULT_FAMILIES, E_PENDING, Font, ITALIC};
    use super::{builtin, text_snapshot, write_bmp};

    /// Default font size: 12 pt at 96 DPI.
    pub const DEFAULT_PX: f32 = 16.0;
    /// Sidebar and label text size relative to the terminal font.
    pub const CHROME_TEXT: f32 = 0.75;

    /// [`GlyphKey::style`] bits beyond bold and italic: the glyph comes
    /// from the chrome font, or is a shape from [`shape_mask`].
    const SMALL: u8 = 4;
    const SHAPE: u8 = 8;

    /// Fallback font lookups per frame; the rest wait for the next one.
    const LOOKUPS: u32 = 256;

    pub struct Renderer {
        pub gpu: Gpu,
        pub font: Font,
        /// The terminal font at chrome size, for the sidebar and labels.
        pub small: Font,
        atlas: Atlas,
        quads: Vec<Quad>,
        overflowed: bool,
        /// The atlas was cleared when this frame began.
        cleared: bool,
        /// Glyphs were left out waiting for a font lookup.
        pending: bool,
    }

    /// The terminal font and the chrome font: `family`, or the first of
    /// the defaults installed.
    fn fonts(family: &str, px: f32) -> Result<(Font, Font)> {
        let families: Vec<&str> = (std::iter::once(family).filter(|f| !f.is_empty()))
            .chain(DEFAULT_FAMILIES.iter().copied())
            .collect();
        Ok((
            Font::new(&families, px)?,
            Font::new(&families, px * CHROME_TEXT)?,
        ))
    }

    impl Renderer {
        pub fn new(warp: bool, px: f32) -> Result<Self> {
            Self::with_gpu(Gpu::new(warp)?, "", px)
        }

        /// [`Self::new`] on a device made elsewhere, in font `family`.
        pub fn with_gpu(mut gpu: Gpu, family: &str, px: f32) -> Result<Self> {
            let (font, small) = fonts(family, px)?;
            gpu.set_text_params(font.gamma, font.contrast);
            Ok(Self {
                gpu,
                font,
                small,
                atlas: Atlas::new(ATLAS_SIZE as u16, ATLAS_SIZE as u16),
                quads: Vec::new(),
                overflowed: false,
                cleared: false,
                pending: false,
            })
        }

        /// Loads another font, or the same at a new size after a DPI change.
        pub fn set_font(&mut self, family: &str, px: f32) -> Result<()> {
            (self.font, self.small) = fonts(family, px)?;
            self.atlas.clear();
            Ok(())
        }

        /// Cell width and height in pixels.
        pub fn cell(&self) -> (u32, u32) {
            (self.font.cell_w, self.font.cell_h)
        }

        /// Cell width and height of the chrome font.
        pub fn small_cell(&self) -> (u32, u32) {
            (self.small.cell_w, self.small.cell_h)
        }

        /// Starts collecting a new frame.
        pub fn begin(&mut self) {
            self.quads.clear();
            // A full atlas is cleared between frames, never during one:
            // glyphs already queued point into it.
            self.cleared = self.overflowed;
            if self.overflowed {
                self.atlas.clear();
            }
            self.overflowed = false;
            self.pending = false;
            self.font.lookups = LOOKUPS;
            self.small.lookups = LOOKUPS;
        }

        /// Whether the last frame left glyphs out to keep slow font
        /// lookups from stalling it; draw another frame soon.
        pub fn pending(&self) -> bool {
            self.pending
        }

        pub fn rect(&mut self, x: i32, y: i32, w: u32, h: u32, rgb: u32) {
            if w == 0 || h == 0 {
                return;
            }
            self.quads.push(Quad {
                pos: [x as i16, y as i16],
                size: [w as u16, h as u16],
                color: rgba(rgb),
                flags: SOLID,
                ..Quad::default()
            });
        }

        /// Adds `snap` with its top-left corner at (`x`, `y`). Cell colours
        /// are used as given (the snapshot has already applied inverse and
        /// the palette), except that dim text is drawn halfway to its
        /// background.
        pub fn snapshot(&mut self, snap: &Snapshot, pal: &Palette, x: i32, y: i32) {
            self.grid(snap, pal, x, y, false);
        }

        /// [`Self::snapshot`] for a pane without focus: text at reduced
        /// contrast, each cell's moved a quarter of the way to its
        /// background, and no cursor or selection.
        pub fn dimmed(&mut self, snap: &Snapshot, pal: &Palette, x: i32, y: i32) {
            self.grid(snap, pal, x, y, true);
        }

        fn grid(&mut self, snap: &Snapshot, pal: &Palette, x: i32, y: i32, dim: bool) {
            let (cw, ch) = self.cell();
            let (cols, rows) = (usize::from(snap.cols), usize::from(snap.rows));
            let blank = RenderCell {
                fg: pal.fg,
                bg: pal.bg,
                width: 1,
                ..RenderCell::default()
            };
            let cell = |c: usize, r: usize| *snap.cells.get(r * cols + c).unwrap_or(&blank);
            let selection = snap.selection.filter(|_| !dim);
            let selected = |c: usize, r: usize| {
                selection.is_some_and(|(a, b)| {
                    let (a, b) = if (a.1, a.0) <= (b.1, b.0) {
                        (a, b)
                    } else {
                        (b, a)
                    };
                    let p = (r as u16, c as u16);
                    (a.1, a.0) <= p && p <= (b.1, b.0)
                })
            };
            let px = |c: usize| x + (c as u32 * cw) as i32;
            let py = |r: usize| y + (r as u32 * ch) as i32;

            let first = self.quads.len();
            self.rect(x, y, cols as u32 * cw, rows as u32 * ch, pal.bg);
            for r in 0..rows {
                let mut c = 0;
                while c < cols {
                    let bg = |c| {
                        if selected(c, r) {
                            pal.selection_bg
                        } else {
                            cell(c, r).bg
                        }
                    };
                    let color = bg(c);
                    let start = c;
                    while c < cols && bg(c) == color {
                        c += 1;
                    }
                    if color != pal.bg {
                        self.rect(px(start), py(r), (c - start) as u32 * cw, ch, color);
                    }
                }
            }

            let cursor = snap
                .cursor
                .filter(|&(c, r, _)| !dim && c < snap.cols && r < snap.rows);
            if let Some((c, r, shape)) = cursor {
                let (c, r) = (usize::from(c), usize::from(r));
                let w = u32::from(cell(c, r).width.max(1)) * cw;
                match shape {
                    CursorShape::Block => self.rect(px(c), py(r), w, ch, pal.cursor),
                    CursorShape::Bar => self.rect(px(c), py(r), (cw / 5).max(2), ch, pal.cursor),
                    CursorShape::Underline => {
                        let h = (ch / 10).max(2);
                        self.rect(px(c), py(r) + (ch - h) as i32, w, h, pal.cursor);
                    }
                }
            }

            for r in 0..rows {
                for c in 0..cols {
                    let mut cl = cell(c, r);
                    if dim {
                        cl.fg = super::toward(cl.fg, cl.bg);
                    }
                    let under_block = cursor.is_some_and(|(cc, cr, s)| {
                        (usize::from(cc), usize::from(cr)) == (c, r) && s == CursorShape::Block
                    });
                    // Selected text is drawn in the theme's colour, so
                    // text close to its background shows before it is
                    // copied.
                    let fg = if under_block {
                        pal.bg
                    } else if selected(c, r) {
                        pal.fg
                    } else if cl.attrs & attr::DIM != 0 {
                        mix(cl.fg, cl.bg)
                    } else {
                        cl.fg
                    };
                    let w = u32::from(cl.width.max(1)) * cw;
                    let f = &self.font;
                    for (bit, y, h) in [
                        (attr::UNDERLINE, f.underline_y, f.underline_h),
                        (attr::STRIKE, f.strike_y, f.strike_h),
                        (attr::OVERLINE, 0, f.underline_h),
                    ] {
                        if cl.attrs & bit != 0 {
                            self.rect(px(c), py(r) + y, w, h, fg);
                        }
                    }
                    if cl.width == 0 || cl.len == 0 {
                        continue;
                    }
                    let style = (if cl.attrs & attr::BOLD != 0 { BOLD } else { 0 })
                        | (if cl.attrs & attr::ITALIC != 0 {
                            ITALIC
                        } else {
                            0
                        });
                    let key = GlyphKey {
                        text: cl.text,
                        len: cl.len,
                        style,
                        width: cl.width,
                    };
                    self.push_glyph(key, px(c), py(r), fg);
                }
            }
            // A cluster of several glyphs can be far wider than its cells;
            // keep it inside the grid so it cannot draw over another pane.
            for q in &mut self.quads[first..] {
                clip(q, x, y, px(cols), py(rows));
            }
        }

        /// Queues the glyph for `key` with its cell's top-left corner at
        /// (`x`, `y`).
        fn push_glyph(&mut self, key: GlyphKey, x: i32, y: i32, rgb: u32) {
            let Some(slot) = self.glyph(key) else {
                return;
            };
            if slot.w == 0 {
                return;
            }
            let text = &key.text[..usize::from(key.len).min(16)];
            let exact = key.style & SHAPE != 0 || is_builtin(text);
            self.quads.push(Quad {
                pos: [
                    (x + i32::from(slot.dx)) as i16,
                    (y + i32::from(slot.dy)) as i16,
                ],
                size: [slot.w, slot.h],
                uv: [slot.x, slot.y],
                color: rgba(rgb),
                flags: if exact { MASK } else { GLYPH },
            });
        }

        /// Queues the window chrome. Call after the terminal grids so it
        /// draws over them.
        pub fn chrome(&mut self, chrome: &Chrome) {
            for p in &chrome.prims {
                match p {
                    Prim::Rect(r, rgb) => {
                        self.rect(r.x, r.y, r.w.max(0) as u32, r.h.max(0) as u32, *rgb);
                    }
                    Prim::Shape {
                        r,
                        radius,
                        stroke,
                        color,
                    } => {
                        let key = shape_key(*r, *radius, *stroke, false);
                        self.push_glyph(key, r.x, r.y, *color);
                    }
                    Prim::Branch(r, color) => {
                        self.push_glyph(shape_key(*r, 0.0, 0.0, true), r.x, r.y, *color);
                    }
                    Prim::Text {
                        x,
                        y,
                        text,
                        color,
                        bold,
                        term,
                    } => {
                        let font = if *term { &self.font } else { &self.small };
                        let cw = font.cell_w;
                        let style = (if *bold { BOLD } else { 0 }) | if *term { 0 } else { SMALL };
                        let mut x = *x;
                        for c in text.chars() {
                            let mut key = GlyphKey {
                                text: [0; 16],
                                len: 0,
                                style,
                                width: 1,
                            };
                            key.len = c.encode_utf8(&mut key.text).len() as u8;
                            key.width = vt::cluster_width(c.encode_utf8(&mut [0; 4])).max(1);
                            if c != ' ' {
                                self.push_glyph(key, x, *y, *color);
                            }
                            x += (u32::from(key.width) * cw) as i32;
                        }
                    }
                }
            }
        }

        /// The atlas slot for a glyph, rasterizing it on first use.
        fn glyph(&mut self, key: GlyphKey) -> Option<Slot> {
            let text = &key.text[..usize::from(key.len).min(16)];
            if text == b" " {
                return None;
            }
            if let Some(slot) = self.atlas.get(&key) {
                return Some(slot);
            }
            let font = if key.style & SMALL != 0 {
                &mut self.small
            } else {
                &mut self.font
            };
            let raster = if key.style & SHAPE != 0 {
                let n = |i: usize| u16::from_le_bytes([text[i], text[i + 1]]);
                let (w, h) = (u32::from(n(0)), u32::from(n(2)));
                let (r, s) = (f32::from(n(4)) / 4.0, f32::from(n(6)) / 4.0);
                Some(if text[8] != 0 {
                    let n = w.min(h);
                    (n, n, 0, 0, branch_mask(n))
                } else {
                    (w, h, 0, 0, shape_mask(w, h, r, s))
                })
            } else if is_builtin(text) {
                let c = std::str::from_utf8(text).ok()?.chars().next()?;
                let w = font.cell_w * u32::from(key.width.max(1));
                let h = font.cell_h;
                builtin::draw(c, w as usize, h as usize).map(|a| (w, h, 0, 0, a))
            } else {
                let text = std::str::from_utf8(text).ok()?;
                match font.raster(text, key.style & (BOLD | ITALIC), key.width) {
                    Ok(Some(r)) => Some((r.w, r.h, r.dx, r.dy, r.alpha)),
                    Err(e) if e.code() == E_PENDING => {
                        self.pending = true;
                        return None;
                    }
                    _ => None,
                }
            };
            let Some((w, h, dx, dy, alpha)) = raster.filter(|r| r.0 > 0 && r.1 > 0) else {
                self.atlas.insert_empty(key);
                return None;
            };
            // Terminal text may not fill the last eighth of the atlas, so
            // the window chrome still fits when a pane shows more distinct
            // glyphs than the atlas holds.
            let (aw, ah) = self.atlas.size();
            let bottom = if key.style & (SMALL | SHAPE) != 0 {
                ah
            } else {
                ah - ah / 8
            };
            if w > u32::from(aw) || h > u32::from(bottom) {
                // It would never fit; leave it out for good.
                self.atlas.insert_empty(key);
                return None;
            }
            let (w16, h16) = (w as u16, h as u16);
            let Some(slot) = self
                .atlas
                .insert_above(bottom, key, w16, h16, dx as i16, dy as i16)
            else {
                // Left out of this frame; `begin` clears the full atlas
                // and the caller draws again when `draw` reports it.
                self.overflowed = true;
                return None;
            };
            self.gpu
                .upload(u32::from(slot.x), u32::from(slot.y), w, h, &alpha);
            Some(slot)
        }

        /// Clears `rtv` to `bg` and draws the frame. Returns true when the
        /// glyph atlas filled up and left glyphs out, and an empty atlas
        /// may hold them, in which case the caller should build and draw
        /// the frame again.
        pub fn draw(
            &mut self,
            rtv: &ID3D11RenderTargetView,
            w: u32,
            h: u32,
            bg: u32,
        ) -> Result<bool> {
            self.gpu.draw(rtv, w, h, bg, &self.quads)?;
            Ok(self.overflowed && !self.cleared)
        }

        #[cfg(test)]
        pub fn set_atlas_size(&mut self, w: u16, h: u16) {
            self.atlas = Atlas::new(w, h);
        }
    }

    /// Atlas key for a shape: its size, corner radius and stroke in
    /// quarter pixels, and whether it is the branch icon.
    fn shape_key(r: crate::layout::Rect, radius: f32, stroke: f32, branch: bool) -> GlyphKey {
        let (w, h) = (r.w.clamp(0, 2048) as u16, r.h.clamp(0, 2048) as u16);
        let q = |v: f32| ((v * 4.0).round() as u16).to_le_bytes();
        let mut text = [0u8; 16];
        text[..2].copy_from_slice(&w.to_le_bytes());
        text[2..4].copy_from_slice(&h.to_le_bytes());
        text[4..6].copy_from_slice(&q(radius));
        text[6..8].copy_from_slice(&q(stroke));
        text[8] = u8::from(branch);
        GlyphKey {
            text,
            len: 9,
            style: SHAPE,
            width: 0,
        }
    }

    /// Cuts `q` down to the part inside `x0..x1` × `y0..y1`, moving its
    /// atlas position along with its top-left corner.
    fn clip(q: &mut Quad, x0: i32, y0: i32, x1: i32, y1: i32) {
        let (x, y) = (i32::from(q.pos[0]), i32::from(q.pos[1]));
        let (left, top) = (x.max(x0), y.max(y0));
        let right = (x + i32::from(q.size[0])).min(x1);
        let bottom = (y + i32::from(q.size[1])).min(y1);
        if right <= left || bottom <= top {
            q.size = [0, 0];
            return;
        }
        q.uv = [q.uv[0] + (left - x) as u16, q.uv[1] + (top - y) as u16];
        q.pos = [left as i16, top as i16];
        q.size = [(right - left) as u16, (bottom - top) as u16];
    }

    /// The colour halfway between two `0xRRGGBB` colours.
    fn mix(a: u32, b: u32) -> u32 {
        let [_, ar, ag, ab] = a.to_be_bytes();
        let [_, br, bg, bb] = b.to_be_bytes();
        let m = |x: u8, y: u8| u32::from(x.midpoint(y));
        m(ar, br) << 16 | m(ag, bg) << 8 | m(ab, bb)
    }

    fn is_builtin(text: &[u8]) -> bool {
        let mut chars = std::str::from_utf8(text).unwrap_or("").chars();
        matches!((chars.next(), chars.next()), (Some(c), None) if builtin::is_builtin(c))
    }

    /// Renders `snap` to memory: returns width, height and BGRA pixels.
    pub fn render_offscreen(
        r: &mut Renderer,
        snap: &Snapshot,
        pal: &Palette,
    ) -> Result<(u32, u32, Vec<u8>)> {
        let (cw, ch) = r.cell();
        let (w, h) = (
            (u32::from(snap.cols) * cw).max(1),
            (u32::from(snap.rows) * ch).max(1),
        );
        let target = r.gpu.offscreen(w, h)?;
        loop {
            r.begin();
            r.snapshot(snap, pal, 0, 0);
            if !r.draw(&target.rtv, w, h, pal.bg)? && !r.pending() {
                break;
            }
        }
        Ok((w, h, r.gpu.read(&target)?))
    }

    /// Renders a made-up window of five sessions, four of them split in
    /// one tab, for checking the chrome. With `--demo`, `--cols` and
    /// `--rows` give the window size in pixels, `--picker` opens the
    /// theme picker filtered to `--picker`'s value, and `--settings N`
    /// opens the settings panel with row N highlighted.
    fn render_demo(
        r: &mut Renderer,
        theme: &crate::theme::Theme,
        collapsed: bool,
        banner: Option<&str>,
        (picker, settings): (Option<&str>, Option<usize>),
        (w, h): (u32, u32),
        scale: f32,
    ) -> Result<Vec<u8>> {
        use std::time::{Duration, Instant};

        use crate::attention::Attn;
        use crate::layout::{Dir, PaneId, Rect, Tab, Window};
        use crate::render::chrome::{self, ChromeModel, Session};

        let (pal, light) = (theme.pal, theme.light);
        let area = Rect {
            x: 0,
            y: 0,
            w: w as i32,
            h: h as i32,
        };
        let (api, web, tests, infra, migrate) =
            (PaneId(1), PaneId(2), PaneId(3), PaneId(4), PaneId(5));
        let mut shop = Tab::new("shop".into(), api);
        shop.split(Dir::Right, web, area, (0, 0));
        shop.split(Dir::Down, tests, area, (0, 0));
        shop.focus(api);
        shop.split(Dir::Down, infra, area, (0, 0));
        shop.focus(web);
        let win = Window {
            tabs: vec![shop, Tab::new("migrate".into(), migrate)],
            active: 0,
            sidebar_expanded: !collapsed,
        };
        let now = Instant::now();
        let ago = |s| now.checked_sub(Duration::from_secs(s)).unwrap_or(now);
        let session = |id, name: &str, cwd: &str, branch: &str, state, msg: &str| Session {
            id,
            name: name.into(),
            cwd: cwd.into(),
            branch: Some(branch.into()),
            state,
            since: ago(72),
            msg: msg.into(),
            progress: None,
            exit_code: None,
        };
        let sessions = [
            session(
                api,
                "api",
                r"C:\dev\shop\api",
                "paging",
                Attn::NeedsYou,
                "Edit src/routes/users.rs?",
            ),
            Session {
                progress: Some(42),
                ..session(
                    web,
                    "web",
                    r"C:\dev\shop\web",
                    "forms",
                    Attn::Working,
                    "Refactoring settings form\u{2026}",
                )
            },
            session(
                tests,
                "tests",
                r"C:\dev\shop",
                "main",
                Attn::DoneUnseen,
                "cargo test \u{b7} 142 passed",
            ),
            session(infra, "infra", r"C:\dev\infra", "main", Attn::Idle, ""),
            Session {
                exit_code: Some(1),
                ..session(
                    migrate,
                    "migrate",
                    r"C:\dev\shop\db",
                    "v5-schema",
                    Attn::Error,
                    "sqlx migrate run",
                )
            },
        ];
        // Removed and added line colours for each theme.
        let (del, add) = if light {
            (
                "\x1b[48;2;251;232;232m\x1b[38;2;154;45;45m",
                "\x1b[48;2;229;243;232m\x1b[38;2;34;100;58m",
            )
        } else {
            (
                "\x1b[48;2;43;27;29m\x1b[38;2;230;167;167m",
                "\x1b[48;2;23;39;28m\x1b[38;2;166;214;175m",
            )
        };
        let del = format!("{del}  43 -     Query(p): Query<Page>,\x1b[0m");
        let add = format!("{add}  43 +     Query(p): Query<PageParams>,\x1b[0m");
        let screen = |id: PaneId, cols: usize| -> String {
            let rule = "\u{2500}".repeat(cols.saturating_sub(3));
            let boxed = |lines: &[&str]| {
                let mut out = format!("\x1b[90m\u{256d}{rule}\u{256e}\x1b[0m\r\n");
                for l in lines {
                    out += &format!(
                        "\x1b[90m\u{2502}\x1b[0m {l}\x1b[{}G\x1b[90m\u{2502}\x1b[0m\r\n",
                        cols - 1
                    );
                }
                out + &format!("\x1b[90m\u{2570}{rule}\u{256f}\x1b[0m\r\n")
            };
            match id {
                PaneId(1) => {
                    "\x1b[90m\u{25cf}\x1b[0m \x1b[1mUpdate\x1b[0m(src/routes/users.rs)\r\n\r\n"
                        .to_string()
                        + &boxed(&[
                            "\x1b[1mEdit file\x1b[0m  src/routes/users.rs",
                            "\x1b[90m  42\x1b[0m   pub async fn list_users(",
                            &del,
                            &add,
                            "",
                            "Do you want to make this edit to \x1b[1musers.rs\x1b[0m?",
                            "\x1b[1m\u{276f} 1. Yes\x1b[0m",
                            "  2. Yes, allow all edits during this session",
                            "  3. No, and tell Claude what to do differently",
                        ])
                }
                PaneId(2) => {
                    "\x1b[90m> refactor the settings page to use the new form hooks\x1b[0m\r\n\r\n"
                        .to_string()
                        + "\u{25cf} I'll start by reading the current settings page.\r\n\r\n"
                        + "\x1b[32m\u{25cf}\x1b[0m \x1b[1mRead\x1b[0m(src/pages/Settings.tsx)\r\n"
                        + "  \x1b[90m\u{23bf}  Read \x1b[1m214\x1b[22m lines\x1b[0m\r\n\r\n"
                        + "\x1b[32m\u{25cf}\x1b[0m \x1b[1mUpdate\x1b[0m(src/pages/Settings.tsx)\r\n"
                        + "  \x1b[90m\u{23bf}  Updated with \x1b[1m38\x1b[22m additions and \x1b[1m61\x1b[22m removals\x1b[0m\r\n\r\n"
                        + "\u{273b} Refactoring settings form\u{2026} \x1b[90m(esc to interrupt \u{b7} 1m 12s \u{b7} \u{2193} 3.4k tokens)\x1b[0m\r\n\r\n"
                        + &boxed(&["\x1b[90m>\x1b[0m \x1b7"])
                        + "\x1b[90m  ? for shortcuts\x1b[0m\x1b8"
                }
                PaneId(3) => {
                    "PS C:\\dev\\shop> cargo test\r\n".to_string()
                        + "\x1b[1;32m   Compiling\x1b[0m shop-core v0.4.0 (C:\\dev\\shop\\core)\r\n"
                        + "\x1b[1;32m    Finished\x1b[0m `test` profile [unoptimized + debuginfo] target(s) in 8.41s\r\n"
                        + "\x1b[1;32m     Running\x1b[0m unittests src\\lib.rs\r\n"
                        + "running 142 tests\r\n"
                        + "test cart::tests::applies_discount ... \x1b[32mok\x1b[0m\r\n"
                        + "test cart::tests::rejects_negative_qty ... \x1b[32mok\x1b[0m\r\n"
                        + "test orders::tests::roundtrip_json ... \x1b[32mok\x1b[0m\r\n\r\n"
                        + "test result: \x1b[32mok\x1b[0m. 142 passed; 0 failed; 0 ignored; finished in 2.31s\r\n"
                        + "PS C:\\dev\\shop> "
                }
                _ => "PS C:\\dev\\infra> ".to_string(),
            }
        };

        let (cw, ch) = r.cell();
        let mut model = ChromeModel {
            win: &win,
            sessions: &sessions,
            ui: theme.ui,
            size: (w as i32, h as i32),
            scale,
            text_cell: r.small_cell(),
            term_cell: (cw, ch),
            now,
            banner,
            preedit: None,
            picker: None,
            settings: None,
        };
        // One setting changed, to show its mark and a switch that is off.
        let config = crate::config::Config {
            flash: false,
            ..Default::default()
        };
        let mut panel =
            crate::settings::Panel::new(super::font::monospace_families(), crate::shell::choices());
        if let Some(sel) = settings {
            panel.sel = sel;
            model.settings = Some(chrome::Settings {
                filter: "",
                rows: panel.rows(&config),
                sel,
                top: 0,
                error: None,
            });
        }
        let themes = crate::theme::all();
        if let Some(f) = picker {
            let f = f.to_lowercase();
            let items: Vec<_> = (themes.iter())
                .filter(|t| t.name.to_lowercase().contains(&f))
                .collect();
            let sel = items.iter().position(|t| t.name == theme.name).unwrap_or(0);
            model.picker = Some(chrome::Picker {
                filter: picker.unwrap_or_default(),
                items,
                sel,
            });
        }
        let mut snaps = Vec::new();
        for &(id, rect) in &chrome::build(&model).panes {
            let cols = (rect.w.max(0) as u32 / cw).max(1) as u16;
            let rows = (rect.h.max(0) as u32 / ch).max(1) as u16;
            let mut term = vt::Terminal::new(vt::Options {
                cols,
                rows,
                ..vt::Options::default()
            });
            term.feed(screen(id, usize::from(cols)).as_bytes());
            if id == web {
                // An IME composition at the prompt.
                let (col, row, _) = term.cursor();
                model.preedit = Some((col, row, "\u{65e5}\u{672c}"));
            }
            let mut snap = Snapshot::default();
            term.snapshot(&mut snap, &pal);
            snaps.push((id, rect, snap));
        }
        let chrome = chrome::build(&model);

        let target = r.gpu.offscreen(w, h)?;
        loop {
            r.begin();
            for (id, rect, snap) in &snaps {
                if *id == web {
                    r.snapshot(snap, &pal, rect.x, rect.y);
                } else {
                    r.dimmed(snap, &pal, rect.x, rect.y);
                }
            }
            r.chrome(&chrome);
            if !r.draw(&target.rtv, w, h, pal.bg)? && !r.pending() {
                break;
            }
        }
        r.gpu.read(&target)
    }

    pub fn debug_render(args: &[String]) -> std::result::Result<String, String> {
        let mut vt_file = None;
        let mut text_file = None;
        let mut bmp = None;
        let (mut warp, mut light, mut demo, mut collapsed) = (false, false, false, false);
        let (mut banner, mut theme, mut picker, mut settings) = (None, None, None, None);
        let (mut cols, mut rows): (Option<u16>, Option<u16>) = (None, None);
        let mut px = DEFAULT_PX;
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let mut val = || it.next().ok_or(format!("{a} needs a value"));
            let num = |v: &String| v.parse::<u16>().map_err(|e| format!("{a} {v}: {e}"));
            match a.as_str() {
                "--vt" => vt_file = Some(PathBuf::from(val()?)),
                "--text" => text_file = Some(PathBuf::from(val()?)),
                "--bmp" => bmp = Some(PathBuf::from(val()?)),
                "--cols" => cols = Some(num(val()?)?),
                "--rows" => rows = Some(num(val()?)?),
                "--px" => px = f32::from(num(val()?)?),
                "--warp" => warp = true,
                "--light" => light = true,
                "--demo" => demo = true,
                "--collapsed" => collapsed = true,
                "--banner" => banner = Some(val()?.clone()),
                "--theme" => theme = Some(val()?.clone()),
                "--picker" => picker = Some(val()?.clone()),
                "--settings" => settings = Some(usize::from(num(val()?)?)),
                "--script" => {
                    return Err("--script is not supported yet; pass --vt FILE".into());
                }
                _ => return Err(format!("unknown argument {a}")),
            }
        }
        let bmp = bmp.ok_or("--bmp OUT is required")?;
        let theme = match theme {
            Some(name) => (crate::theme::all().into_iter())
                .find(|t| t.name.eq_ignore_ascii_case(&name))
                .ok_or(format!("no theme {name:?}"))?,
            None => crate::theme::blitz(light),
        };
        let pal = theme.pal;
        if demo {
            let mut r = Renderer::new(warp, px).map_err(|e| format!("renderer: {e}"))?;
            let (w, h) = (
                u32::from(cols.unwrap_or(1440)),
                u32::from(rows.unwrap_or(868)),
            );
            let scale = px / DEFAULT_PX;
            let pixels = render_demo(
                &mut r,
                &theme,
                collapsed,
                banner.as_deref(),
                (picker.as_deref(), settings),
                (w, h),
                scale,
            )
            .map_err(|e| format!("render: {e}"))?;
            write_bmp(&bmp, w, h, &pixels).map_err(|e| format!("{}: {e}", bmp.display()))?;
            return Ok(format!("{}: {w}x{h} demo", bmp.display()));
        }
        let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
        let snap = match (vt_file, text_file) {
            (Some(f), None) => {
                let mut term = vt::Terminal::new(vt::Options {
                    cols: cols.unwrap_or(120),
                    rows: rows.unwrap_or(40),
                    ..vt::Options::default()
                });
                term.feed(&read(&f)?);
                let mut snap = Snapshot::default();
                term.snapshot(&mut snap, &pal);
                snap
            }
            (None, Some(f)) => {
                let bytes = read(&f)?;
                let text = String::from_utf8_lossy(&bytes);
                let width = text.lines().map(|l| l.chars().count()).max().unwrap_or(1);
                let cols = cols.unwrap_or(width.clamp(1, 400) as u16);
                let rows = rows.unwrap_or(text.lines().count().clamp(1, 200) as u16);
                text_snapshot(&text, cols, rows, &pal)
            }
            _ => return Err("pass exactly one of --vt FILE or --text FILE".into()),
        };
        let mut r = Renderer::new(warp, px).map_err(|e| format!("renderer: {e}"))?;
        let (w, h, pixels) =
            render_offscreen(&mut r, &snap, &pal).map_err(|e| format!("render: {e}"))?;
        write_bmp(&bmp, w, h, &pixels).map_err(|e| format!("{}: {e}", bmp.display()))?;
        Ok(format!(
            "{}: {w}x{h}, cell {}x{}, {}",
            bmp.display(),
            r.font.cell_w,
            r.font.cell_h,
            if r.gpu.warp { "warp" } else { "hardware" }
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pal() -> Palette {
        crate::theme::dark()
    }

    #[test]
    fn text_snapshot_lays_out_wide_and_narrow_cells() {
        let s = text_snapshot("a中b\n\u{2500}", 5, 3, &pal());
        assert_eq!((s.cols, s.rows, s.cells.len()), (5, 3, 15));
        let t = |i: usize| &s.cells[i].text[..usize::from(s.cells[i].len)];
        assert_eq!(t(0), b"a");
        assert_eq!((s.cells[1].width, s.cells[2].width), (2, 0));
        assert_eq!(t(3), b"b");
        assert_eq!(t(5), "\u{2500}".as_bytes());
        assert_eq!(s.cells[14].len, 0);
    }

    #[cfg(windows)]
    #[test]
    fn dim_keeps_three_quarters_of_the_contrast() {
        assert_eq!(toward(0xffffff, 0x000000), 0xbfbfbf);
        assert_eq!(toward(0x000000, 0xffffff), 0x404040);
        // Each channel on its own, both ways.
        assert_eq!(toward(0xff0080, 0x00ff80), 0xbf4080);
        assert_eq!(toward(0x123456, 0x123456), 0x123456);
    }

    /// The colour of the pixel at (`x`, `y`) of `w`-wide BGRA pixels.
    #[cfg(windows)]
    fn pixel(px: &[u8], w: u32, x: u32, y: u32) -> u32 {
        let i = ((y * w + x) * 4) as usize;
        u32::from_be_bytes([0, px[i + 2], px[i + 1], px[i]])
    }

    #[cfg(windows)]
    #[test]
    fn render_warp_unfocused_pane_hides_cursor_and_selection() {
        let mut r = Renderer::new(true, 16.0).expect("renderer");
        let p = pal();
        let mut snap = text_snapshot("\u{2588}  ", 3, 1, &p);
        snap.cursor = Some((1, 0, vt::CursorShape::Block));
        snap.selection = Some(((2, 0), (2, 0)));
        let (cw, ch) = r.cell();
        let (w, h) = (3 * cw, ch);
        let t = r.gpu.offscreen(w, h).expect("target");
        r.begin();
        r.dimmed(&snap, &p, 0, 0);
        r.draw(&t.rtv, w, h, p.bg).expect("draw");
        let px = r.gpu.read(&t).expect("read");
        let mid = ch / 2;
        assert_eq!(
            pixel(&px, w, cw / 2, mid),
            toward(p.fg, p.bg),
            "text dimmed"
        );
        assert_eq!(pixel(&px, w, cw + cw / 2, mid), p.bg, "no cursor");
        assert_eq!(pixel(&px, w, 2 * cw + cw / 2, mid), p.bg, "no selection");
        // The same snapshot with focus shows both.
        r.begin();
        r.snapshot(&snap, &p, 0, 0);
        r.draw(&t.rtv, w, h, p.bg).expect("draw");
        let px = r.gpu.read(&t).expect("read");
        assert_eq!(pixel(&px, w, cw / 2, mid), p.fg);
        assert_eq!(pixel(&px, w, cw + cw / 2, mid), p.cursor);
        assert_eq!(pixel(&px, w, 2 * cw + cw / 2, mid), p.selection_bg);
    }

    #[cfg(windows)]
    #[test]
    fn render_warp_glyph_under_a_block_cursor_takes_the_background() {
        let mut r = Renderer::new(true, 16.0).expect("renderer");
        let p = pal();
        let mut snap = text_snapshot("\u{2588}\u{2588}", 2, 1, &p);
        snap.cursor = Some((0, 0, vt::CursorShape::Block));
        let (w, _, px) = render_offscreen(&mut r, &snap, &p).expect("render");
        let (cw, ch) = r.cell();
        assert_eq!(
            pixel(&px, w, cw / 2, ch / 2),
            p.bg,
            "the block shows through"
        );
        assert_eq!(pixel(&px, w, cw + cw / 2, ch / 2), p.fg);
        // An underline cursor leaves the glyph its colour.
        snap.cursor = Some((0, 0, vt::CursorShape::Underline));
        let (w, _, px) = render_offscreen(&mut r, &snap, &p).expect("render");
        assert_eq!(pixel(&px, w, cw / 2, ch / 2), p.fg);
        // A cursor off the grid is not drawn.
        snap.cursor = Some((5, 0, vt::CursorShape::Block));
        let (w, _, px) = render_offscreen(&mut r, &snap, &p).expect("render");
        assert_eq!(pixel(&px, w, cw / 2, ch / 2), p.fg);
    }

    #[cfg(windows)]
    #[test]
    fn render_warp_strike_and_overline_cross_the_cell() {
        let mut r = Renderer::new(true, 16.0).expect("renderer");
        let p = pal();
        // Through the terminal, so the attributes come from SGR.
        let mut t = vt::Terminal::new(vt::Options {
            cols: 4,
            rows: 1,
            ..vt::Options::default()
        });
        t.feed(b"\x1b[9m  \x1b[29;53m \x1b[55m \x1b[?25l");
        let mut snap = Snapshot::default();
        t.snapshot(&mut snap, &p);
        assert_eq!(snap.cursor, None);
        let (w, _, px) = render_offscreen(&mut r, &snap, &p).expect("render");
        let (cw, ch) = r.cell();
        let (sy, oy) = (r.font.strike_y as u32, 0);
        for x in [1, cw / 2, 2 * cw - 2] {
            assert_eq!(pixel(&px, w, x, sy), p.fg, "strike at x {x}");
        }
        assert_eq!(pixel(&px, w, 1, oy), p.bg, "no overline under SGR 9");
        assert_eq!(pixel(&px, w, 2 * cw + 1, oy), p.fg, "overline");
        assert_eq!(
            pixel(&px, w, 2 * cw + 1, sy),
            p.bg,
            "SGR 29 ends the strike"
        );
        assert_eq!(
            pixel(&px, w, 3 * cw + 1, oy),
            p.bg,
            "SGR 55 ends the overline"
        );
        assert_eq!(pixel(&px, w, cw / 2, ch - 1), p.bg, "no underline");
    }

    #[cfg(windows)]
    #[test]
    fn render_warp_set_font_draws_at_the_new_size() {
        let p = pal();
        let snap = text_snapshot("\u{2588}", 1, 1, &p);
        let mut r = Renderer::new(true, 16.0).expect("renderer");
        render_offscreen(&mut r, &snap, &p).expect("render");
        let small = r.cell();
        r.set_font("", 24.0).expect("font");
        assert!(r.cell().1 > small.1, "{:?} after {small:?}", r.cell());
        let (w, h, px) = render_offscreen(&mut r, &snap, &p).expect("render");
        assert_eq!((w, h), r.cell());
        // The block was drawn again at the new size, not taken from the
        // atlas at the old one.
        assert_eq!(pixel(&px, w, w - 1, h - 1), p.fg, "bottom right");
        assert_eq!(pixel(&px, w, 0, 0), p.fg, "top left");
        // A family that is not installed falls back to the defaults.
        r.set_font("No Such Font 4b1d", 16.0)
            .expect("fallback font");
        assert_eq!(r.cell(), small);
    }

    #[test]
    fn bmp_header_and_padding() {
        let dir = std::env::temp_dir().join(format!("blitz-bmp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("t.bmp");
        // 3x2: rows padded from 9 to 12 bytes; bottom row first.
        let px: Vec<u8> = (0..6).flat_map(|i| [i, 0, 0, 255]).collect();
        write_bmp(&path, 3, 2, &px).expect("write");
        let b = std::fs::read(&path).expect("read");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(&b[..2], b"BM");
        assert_eq!(b.len(), 54 + 12 * 2);
        assert_eq!(b[54], 3, "bottom row comes first");
        assert_eq!(b[54 + 12], 0);
    }

    #[test]
    fn bmp_replaces_a_link_at_the_path() {
        let dir = std::env::temp_dir().join(format!("blitz-bmp-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let (other, path) = (dir.join("other.txt"), dir.join("t.bmp"));
        std::fs::write(&other, "keep").expect("write");
        std::fs::hard_link(&other, &path).expect("link");
        write_bmp(&path, 1, 1, &[0; 4]).expect("write");
        let kept = std::fs::read_to_string(&other).expect("read");
        let bmp = std::fs::read(&path).expect("read");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(kept, "keep");
        assert_eq!(&bmp[..2], b"BM");
    }

    #[cfg(windows)]
    #[test]
    fn render_warp_snapshot_draws_glyphs_and_cursor() {
        let mut r = Renderer::new(true, 16.0).expect("renderer");
        let p = pal();
        let mut snap = text_snapshot("A\u{2588}", 3, 1, &p);
        snap.cursor = Some((2, 0, vt::CursorShape::Block));
        let (w, h, px) = render_offscreen(&mut r, &snap, &p).expect("render");
        let (cw, ch) = r.cell();
        assert_eq!((w, h), (3 * cw, ch));
        let at = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            u32::from_be_bytes([0, px[i + 2], px[i + 1], px[i]])
        };
        // The full block fills its cell with the foreground colour.
        assert_eq!(at(cw + cw / 2, ch / 2), p.fg);
        // The block cursor fills the third cell.
        assert_eq!(at(2 * cw + 1, 1), p.cursor);
        // "A" leaves ink somewhere in the first cell, background elsewhere.
        let first: Vec<u32> = (0..ch)
            .flat_map(|y| (0..cw).map(move |x| (x, y)))
            .map(|(x, y)| at(x, y))
            .collect();
        assert!(first.iter().any(|&c| c != p.bg));
        assert_eq!(at(0, 0), p.bg);
    }

    #[cfg(windows)]
    #[test]
    fn render_warp_dim_text_is_halfway_to_its_background() {
        let mut r = Renderer::new(true, 16.0).expect("renderer");
        let p = pal();
        let mut snap = text_snapshot("\u{2588}\u{2588}", 2, 1, &p);
        snap.cells[1].attrs = vt::snapshot::attr::DIM;
        snap.cells[1].bg = 0x000000;
        snap.cells[1].fg = 0xff8040;
        let (w, _, px) = render_offscreen(&mut r, &snap, &p).expect("render");
        let (cw, ch) = r.cell();
        let i = ((ch / 2 * w + cw + cw / 2) * 4) as usize;
        assert_eq!(&px[i..i + 3], &[0x20, 0x40, 0x7f], "BGR of #7f4020");
    }

    #[cfg(windows)]
    #[test]
    fn render_warp_underline_selection_and_bar_cursor() {
        let mut r = Renderer::new(true, 16.0).expect("renderer");
        let p = pal();
        let mut snap = text_snapshot("_中x", 4, 2, &p);
        snap.cells[0].attrs = vt::snapshot::attr::UNDERLINE;
        snap.cells[0].text[0] = b'a';
        snap.cells[3].bg = p.ansi[1];
        snap.selection = Some(((1, 1), (0, 1)));
        snap.cursor = Some((3, 1, vt::CursorShape::Bar));
        let (w, _, px) = render_offscreen(&mut r, &snap, &p).expect("render");
        let (cw, ch) = r.cell();
        let at = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            u32::from_be_bytes([0, px[i + 2], px[i + 1], px[i]])
        };
        let uy = r.font.underline_y as u32;
        assert_eq!(at(1, uy), p.fg, "underline under the first cell");
        assert_eq!(at(3 * cw + 1, 1), p.ansi[1], "cell background");
        // The selection is given end first and covers two cells.
        assert_eq!(at(1, ch + 1), p.selection_bg);
        assert_eq!(at(2 * cw - 1, ch + 1), p.selection_bg);
        assert_eq!(at(2 * cw + 2, ch + 1), p.bg);
        assert_eq!(at(3 * cw, ch + 2), p.cursor, "bar cursor");
        assert_eq!(at(4 * cw - 1, ch + 2), p.bg, "bar is thin");
        // The wide character's ink reaches into its second cell.
        let ink = (0..ch).any(|y| (2 * cw..3 * cw).any(|x| at(x, y) != p.bg));
        assert!(ink, "wide glyph spans two cells");
    }

    #[cfg(windows)]
    #[test]
    fn render_warp_selection_shows_text_close_to_its_background() {
        let mut r = Renderer::new(true, 16.0).expect("renderer");
        let p = pal();
        let mut snap = text_snapshot("\u{2588}", 1, 1, &p);
        snap.cells[0].fg = p.bg + 1;
        snap.selection = Some(((0, 0), (0, 0)));
        let (w, _, px) = render_offscreen(&mut r, &snap, &p).expect("render");
        let (cw, ch) = r.cell();
        let i = ((ch / 2 * w + cw / 2) * 4) as usize;
        let at = u32::from_be_bytes([0, px[i + 2], px[i + 1], px[i]]);
        assert_eq!(at, p.fg);
    }

    #[cfg(windows)]
    #[test]
    fn render_warp_chrome_shapes_and_text() {
        use crate::layout::Rect;
        use chrome::{Chrome, Prim};

        let mut r = Renderer::new(true, 16.0).expect("renderer");
        let p = pal();
        let rect = |x, y, w, h| Rect { x, y, w, h };
        let c = Chrome {
            prims: vec![
                Prim::Shape {
                    r: rect(0, 0, 10, 10),
                    radius: 5.0,
                    stroke: 0.0,
                    color: 0xf2b84b,
                },
                Prim::Shape {
                    r: rect(20, 0, 30, 10),
                    radius: 3.0,
                    stroke: 0.0,
                    color: 0x00ff00,
                },
                Prim::Text {
                    x: 60,
                    y: 0,
                    text: "W".into(),
                    color: p.fg,
                    bold: true,
                    term: false,
                },
            ],
            ..Chrome::default()
        };
        let (w, h) = (80, 20);
        let target = r.gpu.offscreen(w, h).expect("target");
        r.begin();
        r.chrome(&c);
        r.draw(&target.rtv, w, h, p.bg).expect("draw");
        let px = r.gpu.read(&target).expect("read");
        let at = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            u32::from_be_bytes([0, px[i + 2], px[i + 1], px[i]])
        };
        assert_eq!(at(5, 5), 0xf2b84b, "dot centre");
        assert_eq!(at(0, 0), p.bg, "dot corner is round");
        assert_eq!(at(45, 5), 0x00ff00, "a wide shape keeps its width");
        let (sw, sh) = r.small_cell();
        let ink = (0..sh).any(|y| (60..60 + sw).any(|x| at(x, y) != p.bg));
        assert!(ink, "chrome text is drawn");
    }

    #[cfg(windows)]
    #[test]
    fn render_warp_full_atlas_leaves_earlier_glyphs_alone() {
        let p = pal();
        let snap = text_snapshot("HELLO\nabcdefghijklmnopqrstuvwxyz0123456789", 36, 2, &p);
        let mut r = Renderer::new(true, 16.0).expect("renderer");
        let (_, _, want) = render_offscreen(&mut r, &snap, &p).expect("render");
        let mut r = Renderer::new(true, 16.0).expect("renderer");
        r.set_atlas_size(64, 64);
        let (w, _, px) = render_offscreen(&mut r, &snap, &p).expect("render");
        let row = (w * r.cell().1 * 4) as usize;
        assert!(px[..row] == want[..row], "the first row is drawn as usual");
    }

    #[cfg(windows)]
    #[test]
    fn render_warp_glyph_too_big_for_the_atlas_is_left_out() {
        let p = pal();
        let mut r = Renderer::new(true, 16.0).expect("renderer");
        r.set_atlas_size(8, 8);
        let snap = text_snapshot("M", 1, 1, &p);
        let target = r.gpu.offscreen(16, 32).expect("target");
        for _ in 0..2 {
            r.begin();
            r.snapshot(&snap, &p, 0, 0);
            let again = r.draw(&target.rtv, 16, 32, p.bg).expect("draw");
            assert!(!again, "no frame is drawn twice");
        }
    }

    #[cfg(windows)]
    #[test]
    fn render_warp_wide_clusters_stay_inside_their_grid() {
        let p = pal();
        let mut r = Renderer::new(true, 16.0).expect("renderer");
        let mut snap = text_snapshot("ab", 4, 1, &p);
        // Five leading jamo join into one cluster, each with its own advance.
        let jamo = "\u{1100}".repeat(5);
        snap.cells[2].text[..15].copy_from_slice(jamo.as_bytes());
        snap.cells[2].len = 15;
        snap.cells[2].width = 2;
        snap.cells[3].width = 0;
        let (cw, ch) = r.cell();
        let (w, h) = (4 * cw + 100, ch);
        let target = r.gpu.offscreen(w, h).expect("target");
        r.begin();
        r.snapshot(&snap, &p, 0, 0);
        r.draw(&target.rtv, w, h, 0x123456).expect("draw");
        let px = r.gpu.read(&target).expect("read");
        let at = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            u32::from_be_bytes([0, px[i + 2], px[i + 1], px[i]])
        };
        let ink = (0..h).any(|y| (2 * cw..4 * cw).any(|x| at(x, y) != p.bg));
        assert!(ink, "the cluster is drawn");
        let spill = (0..h).any(|y| (4 * cw..w).any(|x| at(x, y) != 0x123456));
        assert!(!spill, "nothing is drawn right of the grid");
    }

    #[cfg(windows)]
    #[test]
    fn render_warp_spreads_font_lookups_over_frames() {
        let p = pal();
        let mut r = Renderer::new(true, 16.0).expect("renderer");
        // More new fallback characters than one frame looks up.
        let text: String = (0..300)
            .filter_map(|i| char::from_u32(0x4e00 + i))
            .collect();
        let snap = text_snapshot(&text, 600, 1, &p);
        r.begin();
        r.snapshot(&snap, &p, 0, 0);
        assert!(r.pending(), "some glyphs wait for the next frame");
        let (w, h, px) = render_offscreen(&mut r, &snap, &p).expect("render");
        assert!(!r.pending());
        let (cw, _) = r.cell();
        let last = (0..h).any(|y| {
            (598 * cw..w).any(|x| {
                let i = ((y * w + x) * 4) as usize;
                u32::from_be_bytes([0, px[i + 2], px[i + 1], px[i]]) != p.bg
            })
        });
        assert!(last, "an offscreen render draws every glyph");
    }
}
