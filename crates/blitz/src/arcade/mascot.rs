//! The spark at the foot of the sidebar, doing what the sessions do.

use crate::attention::Attn;
use crate::layout::Rect;
use crate::render::chrome::Prim;
use crate::theme::Ui;

/// Lays out the spark at `t` seconds in `area`, the free part of the
/// expanded sidebar under the sessions, for `state`, the most urgent
/// session state. Draws nothing when `area` is too small to hold it.
/// `(tw, th)` is the sidebar font's cell size.
pub fn draw(
    p: &mut Vec<Prim>,
    area: Rect,
    state: Attn,
    t: f32,
    scale: f32,
    ui: &Ui,
    (tw, th): (i32, i32),
) {
    let _ = (p, area, state, t, scale, ui, tw, th);
}
