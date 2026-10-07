//! blitz run: a one-button runner for while agents work.

use crate::layout::Rect;
use crate::render::chrome::Prim;
use crate::theme::Ui;

/// One game, from the first jump to the fall and any restarts.
#[derive(Clone, Debug)]
pub struct Run {
    pub score: u32,
    /// The best score so far, this one included.
    pub best: u32,
    /// Fell; the next jump starts again.
    pub over: bool,
}

impl Run {
    /// A new game; `seed` picks the course and `best` is the best score
    /// saved from earlier games.
    pub fn new(seed: u64, best: u32) -> Run {
        let _ = seed;
        Run {
            score: 0,
            best,
            over: false,
        }
    }

    /// Space, Up or W: jump, or start again after a fall.
    pub fn jump(&mut self) {}

    /// Moves the game on by `dt` seconds. True when this step ended it.
    pub fn step(&mut self, dt: f32) -> bool {
        let _ = dt;
        false
    }

    /// Lays out the game as a panel over the middle of `area`, the panes'
    /// part of the window. `(tw, th)` is the sidebar font's cell size.
    pub fn draw(&self, p: &mut Vec<Prim>, area: Rect, scale: f32, ui: &Ui, (tw, th): (i32, i32)) {
        let _ = (p, area, scale, ui, tw, th);
    }
}

/// The best score saved in `%LOCALAPPDATA%\blitz\run-best`, or 0.
pub fn load_best() -> u32 {
    (crate::session::dir())
        .and_then(|d| std::fs::read_to_string(d.join("run-best")).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Saves `best` for the next game. A failed save only loses the score.
pub fn save_best(best: u32) {
    if let Some(d) = crate::session::dir() {
        let _ = std::fs::create_dir_all(&d);
        let _ = std::fs::write(d.join("run-best"), best.to_string());
    }
}
