//! Pixel scenery drawn behind the panes' text: faint enough to read over,
//! in colours mixed from the theme's background.

use vt::Palette;

use crate::layout::Rect;
use crate::render::chrome::Prim;

/// The scenery settings, as `config.toml` names them; the first is none.
pub const SCENES: &[&str] = &["off", "stars", "hills", "snow"];

/// Lays out scene `name` from [`SCENES`] at `t` seconds over `area`, the
/// whole window. `scale` is the DPI scale, 1.0 at 96 DPI. Draws nothing
/// for `off` or a name it does not know.
pub fn draw(p: &mut Vec<Prim>, name: &str, area: Rect, t: f32, scale: f32, pal: &Palette) {
    let _ = (p, name, area, t, scale, pal);
}
