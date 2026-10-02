//! Turns terminal snapshots and window chrome into quads and draws them.

// One process hosts every session, so a failed HRESULT must never panic.
#![deny(clippy::unwrap_used)]

pub mod atlas;
pub mod builtin;
#[cfg(windows)]
pub mod d3d11;
#[cfg(windows)]
pub mod font;

use std::io::Write as _;
use std::path::Path;

use vt::{Palette, RenderCell, Snapshot};

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
    std::fs::File::create(path)?.write_all(&out)
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
    use super::d3d11::{ATLAS_SIZE, GLYPH, Gpu, MASK, Quad, SOLID, rgba};
    use super::font::{BOLD, DEFAULT_FAMILIES, Font, ITALIC};
    use super::{builtin, text_snapshot, write_bmp};

    /// Default font size: 12 pt at 96 DPI.
    pub const DEFAULT_PX: f32 = 16.0;

    pub struct Renderer {
        pub gpu: Gpu,
        pub font: Font,
        atlas: Atlas,
        quads: Vec<Quad>,
        overflowed: bool,
    }

    impl Renderer {
        pub fn new(warp: bool, px: f32) -> Result<Self> {
            Ok(Self {
                gpu: Gpu::new(warp)?,
                font: Font::new(DEFAULT_FAMILIES, px)?,
                atlas: Atlas::new(ATLAS_SIZE as u16, ATLAS_SIZE as u16),
                quads: Vec::new(),
                overflowed: false,
            })
        }

        /// Cell width and height in pixels.
        pub fn cell(&self) -> (u32, u32) {
            (self.font.cell_w, self.font.cell_h)
        }

        /// Starts collecting a new frame.
        pub fn begin(&mut self) {
            self.quads.clear();
            self.overflowed = false;
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
        /// are used as given: the snapshot has already applied inverse,
        /// dim and the palette.
        pub fn snapshot(&mut self, snap: &Snapshot, pal: &Palette, x: i32, y: i32) {
            let (cw, ch) = self.cell();
            let (cols, rows) = (usize::from(snap.cols), usize::from(snap.rows));
            let blank = RenderCell {
                fg: pal.fg,
                bg: pal.bg,
                width: 1,
                ..RenderCell::default()
            };
            let cell = |c: usize, r: usize| *snap.cells.get(r * cols + c).unwrap_or(&blank);
            let selected = |c: usize, r: usize| {
                snap.selection.is_some_and(|(a, b)| {
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
                .filter(|&(c, r, _)| c < snap.cols && r < snap.rows);
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
                    let cl = cell(c, r);
                    let under_block = cursor.is_some_and(|(cc, cr, s)| {
                        (usize::from(cc), usize::from(cr)) == (c, r) && s == CursorShape::Block
                    });
                    let fg = if under_block { pal.bg } else { cl.fg };
                    if cl.attrs & attr::UNDERLINE != 0 {
                        let w = u32::from(cl.width.max(1)) * cw;
                        let uy = py(r) + self.font.underline_y;
                        self.rect(px(c), uy, w, self.font.underline_h, fg);
                    }
                    if cl.width == 0 || cl.len == 0 {
                        continue;
                    }
                    let Some(slot) = self.glyph(&cl) else {
                        continue;
                    };
                    if slot.w == 0 {
                        continue;
                    }
                    let flags = if is_builtin(&cl) { MASK } else { GLYPH };
                    self.quads.push(Quad {
                        pos: [
                            (px(c) + i32::from(slot.dx)) as i16,
                            (py(r) + i32::from(slot.dy)) as i16,
                        ],
                        size: [slot.w, slot.h],
                        uv: [slot.x, slot.y],
                        color: rgba(fg),
                        flags,
                    });
                }
            }
        }

        /// The atlas slot for a cell's glyph, rasterizing it on first use.
        fn glyph(&mut self, cl: &RenderCell) -> Option<Slot> {
            let text = std::str::from_utf8(&cl.text[..usize::from(cl.len).min(16)]).ok()?;
            if text == " " {
                return None;
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
            if let Some(slot) = self.atlas.get(&key) {
                return Some(slot);
            }
            let (w, h, dx, dy, alpha) = if is_builtin(cl) {
                let c = text.chars().next()?;
                let (cw, ch) = self.cell();
                let w = cw * u32::from(cl.width.max(1));
                (w, ch, 0, 0, builtin::draw(c, w as usize, ch as usize)?)
            } else {
                match self.font.raster(text, style, cl.width) {
                    Ok(Some(r)) => (r.w, r.h, r.dx, r.dy, r.alpha),
                    _ => {
                        self.atlas.insert_empty(key);
                        return None;
                    }
                }
            };
            let fits =
                |atlas: &mut Atlas| atlas.insert(key, w as u16, h as u16, dx as i16, dy as i16);
            let slot = match fits(&mut self.atlas) {
                Some(slot) => slot,
                None => {
                    // ponytail: a full atlas is wiped mid-frame, so glyphs
                    // already queued this frame may show stale pixels; the
                    // caller redraws when `draw` reports it. Fine unless one
                    // frame needs more glyphs than the whole atlas holds.
                    self.overflowed = true;
                    self.atlas.clear();
                    fits(&mut self.atlas)?
                }
            };
            self.gpu
                .upload(u32::from(slot.x), u32::from(slot.y), w, h, &alpha);
            Some(slot)
        }

        /// Clears `rtv` to `bg` and draws the frame. Returns true when the
        /// glyph atlas overflowed while building it, in which case the
        /// caller should build and draw the frame again.
        pub fn draw(
            &mut self,
            rtv: &ID3D11RenderTargetView,
            w: u32,
            h: u32,
            bg: u32,
        ) -> Result<bool> {
            self.gpu.draw(rtv, w, h, bg, &self.quads)?;
            Ok(self.overflowed)
        }
    }

    fn is_builtin(cl: &RenderCell) -> bool {
        let text = &cl.text[..usize::from(cl.len).min(16)];
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
        for _ in 0..2 {
            r.begin();
            r.snapshot(snap, pal, 0, 0);
            if !r.draw(&target.rtv, w, h, pal.bg)? {
                break;
            }
        }
        Ok((w, h, r.gpu.read(&target)?))
    }

    pub fn debug_render(args: &[String]) -> std::result::Result<String, String> {
        let mut vt_file = None;
        let mut text_file = None;
        let mut bmp = None;
        let (mut warp, mut light) = (false, false);
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
                "--script" => {
                    return Err("--script is not supported yet; pass --vt FILE".into());
                }
                _ => return Err(format!("unknown argument {a}")),
            }
        }
        let bmp = bmp.ok_or("--bmp OUT is required")?;
        let pal = if light {
            crate::theme::light()
        } else {
            crate::theme::dark()
        };
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
}
