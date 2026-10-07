//! blitz run: a one-button runner for while agents work. The spark runs
//! right; bugs and stacks of blocks come at it, and bolts float over some
//! of them. Distances are in game pixels, one per art pixel, and heights
//! are above the ground.

use super::sprite::{self, SPARK_H, SPARK_W, shade};
use crate::layout::Rect;
use crate::render::chrome::Prim;
use crate::theme::Ui;

/// Speed at the start and the most it gets to, in game pixels a second,
/// and the seconds of running it takes to get there.
const SLOW: f32 = 90.0;
const FAST: f32 = 200.0;
const RAMP: f32 = 90.0;
/// Pull of gravity and the upward speed of a jump.
const GRAVITY: f32 = 960.0;
const JUMP: f32 = 240.0;
/// Seconds a jump spends in the air, and how high it goes.
const AIR: f32 = 2.0 * JUMP / GRAVITY;
const APEX: f32 = JUMP * JUMP / (2.0 * GRAVITY);
/// Longest physics step, in seconds.
const TICK: f32 = 1.0 / 120.0;
/// The part of the spark that hits things: its offset from the art's left
/// edge, and its width. The antenna and the edges of the body miss.
const BODY_X: f32 = 2.0;
const BODY_W: f32 = 9.0;
/// Where the first jump over the course can start.
const FIRST: f32 = 160.0;
/// Seconds between landing from the latest jump that clears one obstacle
/// and the earliest that clears the next, plus up to `SPREAD` more.
const REACT: f32 = 0.15;
const SPREAD: f32 = 0.75;
/// How far ahead the course is laid and how far behind it is kept.
const AHEAD: f32 = 480.0;
const BEHIND: f32 = 120.0;
/// Seconds after a fall before a jump starts again, so that a held key
/// does not.
const COOLDOWN: f32 = 0.4;

/// The field's height in art pixels, and the ground line's row in it.
const FIELD_H: i32 = 62;
const GROUND: i32 = 54;

/// A beetle facing the spark, without its legs.
const BUG: [&str; 6] = [
    ".....kkkk..",
    "k...khheek.",
    ".k.kheeeeek",
    "..kkeeeeeek",
    ".kwkeeeeddk",
    ".kkkdddddk.",
];
const LEGS: [&str; 2] = ["..k.k..k.k.", ".k.k..k.k.."];
const BUG_W: i32 = 11;
const BUG_H: i32 = 7;

const BLOCK: [&str; 6] = [
    "kkkkkkkk", "khhhhhhk", "kffffffk", "kfkkkkfk", "kffffffk", "kkkkkkkk",
];
const BLOCK_W: i32 = 8;
const BLOCK_H: i32 = 6;

const BOLT: [&str; 9] = [
    "....bbk", "...bbk.", "..bbk..", ".bbbbbk", "bbbbbk.", "..bbk..", ".bbk...", ".bk....",
    "bk.....",
];
const BOLT_W: i32 = 7;
const BOLT_H: i32 = 9;
/// A bolt's height: a jump that peaks under it catches it.
const BOLT_Y: f32 = APEX + 5.0;

const CLOUD: [&str; 4] = [
    "....cccc......",
    "..cccccccc....",
    ".cccccccccccc.",
    "cccccccccccccc",
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// A beetle on the ground.
    Bug,
    /// One to three blocks, a little askew.
    Stack(u8),
    /// Bugs in a row: one long jump, timed well.
    Swarm(u8),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Obstacle {
    /// The art's left edge.
    x: f32,
    kind: Kind,
}

impl Obstacle {
    /// Art width and height.
    fn size(self) -> (i32, i32) {
        match self.kind {
            Kind::Bug => (BUG_W, BUG_H),
            Kind::Stack(n) => (BLOCK_W + 1, BLOCK_H * i32::from(n)),
            Kind::Swarm(n) => (i32::from(n) * (BUG_W + 1) - 1, BUG_H),
        }
    }

    /// The part that hits, a little inside the art: left edge, width and
    /// height.
    fn hitbox(self) -> (f32, f32, f32) {
        let (w, h) = self.size();
        (self.x + 1.0, w as f32 - 2.0, h as f32 - 1.0)
    }

    /// The first and last places, as the spark's distance, from which a
    /// jump at speed `v` clears this.
    fn window(self, v: f32) -> (f32, f32) {
        let (x, w, h) = self.hitbox();
        // Seconds of the jump spent above `h`, centred on the top.
        let over = AIR * (1.0 - h / APEX).max(0.0).sqrt();
        (
            x + w - v * (AIR + over) / 2.0,
            x - BODY_W - v * (AIR - over) / 2.0,
        )
    }
}

/// Speed after running `d` game pixels: up by the same amount each second.
fn speed(d: f32) -> f32 {
    let accel = (FAST - SLOW) / RAMP;
    (SLOW * SLOW + 2.0 * accel * d.max(0.0)).sqrt().min(FAST)
}

/// The splitmix64 mixer: well spread bits from any number.
fn mix(z: u64) -> u64 {
    let z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// One game, from the first jump to the fall and any restarts.
#[derive(Clone, Debug)]
pub struct Run {
    pub score: u32,
    /// The best score so far, this one included.
    pub best: u32,
    /// Fell; the next jump starts again.
    pub over: bool,
    /// The first jump has been made.
    running: bool,
    rng: u64,
    /// Seconds since the game opened or restarted.
    clock: f32,
    /// When the run ended, on `clock`.
    fell: f32,
    /// How far the spark has run, at its body's left edge.
    dist: f32,
    /// Height of the spark's feet, and their upward speed.
    y: f32,
    vy: f32,
    /// Bolts caught.
    caught: u32,
    /// The best score when this run began, to tell a new one.
    old_best: u32,
    course: Vec<Obstacle>,
    /// Bolts not yet caught, by their art's left edge.
    bolts: Vec<f32>,
    /// The earliest place the next obstacle's jump may start.
    free: f32,
}

impl Run {
    /// A new game; `seed` picks the course and `best` is the best score
    /// saved from earlier games.
    pub fn new(seed: u64, best: u32) -> Run {
        let mut run = Run {
            score: 0,
            best,
            over: false,
            running: false,
            rng: seed,
            clock: 0.0,
            fell: 0.0,
            dist: 0.0,
            y: 0.0,
            vy: 0.0,
            caught: 0,
            old_best: best,
            course: Vec::new(),
            bolts: Vec::new(),
            free: FIRST,
        };
        run.plan();
        run
    }

    /// Space, Up or W: jump, or start again after a fall.
    pub fn jump(&mut self) {
        if self.over {
            if self.clock - self.fell < COOLDOWN {
                return;
            }
            let seed = self.next();
            *self = Run::new(seed, self.best);
        }
        self.running = true;
        if self.grounded() {
            self.vy = JUMP;
        }
    }

    /// Moves the game on by `dt` seconds. True when this step ended it.
    pub fn step(&mut self, dt: f32) -> bool {
        // The app already keeps dt under 0.05; this only bounds
        // the loop below.
        let dt = dt.min(0.25);
        if dt.is_nan() || dt <= 0.0 {
            return false;
        }
        self.clock += dt;
        if !self.running || self.over {
            return false;
        }
        let n = (dt / TICK).ceil();
        let h = dt / n;
        for _ in 0..n as u32 {
            self.dist += speed(self.dist) * h;
            if !self.grounded() {
                self.y += self.vy * h - GRAVITY * h * h / 2.0;
                self.vy -= GRAVITY * h;
                if self.y <= 0.0 {
                    (self.y, self.vy) = (0.0, 0.0);
                }
            }
            self.plan();
            let (x, y) = (self.dist - BODY_X, self.y);
            let (sw, sh, bw, bh) = (SPARK_W as f32, SPARK_H as f32, BOLT_W as f32, BOLT_H as f32);
            let left = self.bolts.len();
            self.bolts
                .retain(|&b| !(b < x + sw && x < b + bw && BOLT_Y < y + sh && y < BOLT_Y + bh));
            self.caught += (left - self.bolts.len()) as u32;
            self.score = (self.dist / 10.0) as u32 + self.caught * 10;
            self.best = self.best.max(self.score);
            let hit = self.course.iter().any(|o| {
                let (hx, hw, hh) = o.hitbox();
                hx < self.dist + BODY_W && self.dist < hx + hw && self.y < hh
            });
            if hit {
                self.over = true;
                self.fell = self.clock;
                return true;
            }
        }
        false
    }

    fn grounded(&self) -> bool {
        self.y <= 0.0 && self.vy <= 0.0
    }

    /// Next number from the course's generator.
    fn next(&mut self) -> u64 {
        self.rng = self.rng.wrapping_add(0x9e37_79b9_7f4a_7c15);
        mix(self.rng)
    }

    /// A random number from 0 up to 1.
    fn rand(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u32 << 24) as f32
    }

    /// Lays the course out to `AHEAD` past the spark and forgets what is
    /// well behind it. Every jump that clears an obstacle lands in time to
    /// clear the next, so no gap is impossible.
    fn plan(&mut self) {
        while self.free < self.dist + AHEAD {
            // Longer obstacles come in as the run goes on.
            let tier = (self.free / 2500.0) as u32;
            let kind = match (self.rand(), tier) {
                (r, 2..) if r < 0.12 => Kind::Swarm(3),
                (r, 1..) if r < 0.25 => Kind::Swarm(2),
                (r, 1..) if r < 0.4 => Kind::Stack(3),
                (r, _) if r < 0.6 => Kind::Bug,
                (r, _) if r < 0.75 => Kind::Stack(1),
                _ => Kind::Stack(2),
            };
            let v = speed(self.free);
            let first = Obstacle { x: 0.0, kind }.window(v).0;
            let o = Obstacle {
                x: self.free - first,
                kind,
            };
            if self.rand() < 0.35 {
                let w = o.size().0;
                self.bolts.push(o.x + (w - BOLT_W) as f32 / 2.0);
            }
            let last = o.window(v).1;
            self.free = last + v * (AIR + REACT + SPREAD * self.rand());
            self.course.push(o);
        }
        let gone = self.dist - BEHIND;
        self.course.retain(|o| o.x + o.size().0 as f32 > gone);
        self.bolts.retain(|&b| b + BOLT_W as f32 > gone);
    }

    /// Lays out the game as a panel over the middle of `area`, the panes'
    /// part of the window. `(tw, th)` is the sidebar font's cell size.
    pub fn draw(&self, p: &mut Vec<Prim>, area: Rect, scale: f32, ui: &Ui, (tw, th): (i32, i32)) {
        let c = ui;
        let s = |v: f32| (v * scale).round() as i32;
        let px = s(2.0).max(2);
        let (one, pad) = (s(1.0).max(1), s(16.0));
        let (head_h, line_h) = (th + s(16.0), th + s(12.0));
        let pw = s(720.0).min(area.w - s(32.0));
        let ph = 2 * one + head_h + FIELD_H * px + line_h;
        if pw < 2 * pad + 36 * tw || ph > area.h - s(16.0) {
            return;
        }
        let panel = Rect {
            x: area.x + (area.w - pw) / 2,
            y: area.y + (area.h - ph) / 2,
            w: pw,
            h: ph,
        };
        p.push(Prim::Rect(panel, c.border));
        let inner = Rect {
            x: panel.x + one,
            y: panel.y + one,
            w: panel.w - 2 * one,
            h: panel.h - 2 * one,
        };
        p.push(Prim::Rect(inner, c.side_bg));
        let (left, right) = (inner.x + pad, inner.right() - pad);
        let mid = |y: i32, hh: i32| y + (hh - th) / 2;
        let width = |t: &str| t.chars().count() as i32 * tw;
        let text = |p: &mut Vec<Prim>, x: i32, y: i32, t: &str, color: u32, bold: bool| {
            if x >= left && x + width(t) <= right {
                p.push(Prim::Text {
                    x,
                    y,
                    text: t.into(),
                    color,
                    bold,
                    term: false,
                });
            }
        };

        // The title, and the score with the best at the right.
        let hy = mid(inner.y, head_h);
        text(p, left, hy, "blitz run", c.name, true);
        let score = self.score.to_string();
        let sx = right - width(&score);
        text(p, sx, hy, &score, c.name, true);
        let best = format!("best {}", self.best);
        text(p, sx - 2 * tw - width(&best), hy, &best, c.dim, false);

        let field = Rect {
            x: inner.x,
            y: inner.y + head_h,
            w: inner.w,
            h: FIELD_H * px,
        };
        let from = p.len();
        self.scene(p, field, px, pad, c);
        clip(p, from, field);

        // A word over the middle of the field before and after a run.
        let lines = if self.over {
            let got = if self.score > self.old_best {
                (format!("new best {}", self.score), c.accent, true)
            } else {
                (format!("score {}", self.score), c.msg, false)
            };
            vec![
                ("fizzled!".to_string(), c.name, true),
                got,
                (
                    "space to try again \u{b7} esc to quit".to_string(),
                    c.dim,
                    false,
                ),
            ]
        } else if !self.running {
            vec![("space to start".to_string(), c.msg, false)]
        } else {
            Vec::new()
        };
        let step = th + s(4.0);
        let mut y = field.y + (GROUND * px - lines.len() as i32 * step) / 2;
        for (t, color, bold) in &lines {
            text(p, field.x + (field.w - width(t)) / 2, y, t, *color, *bold);
            y += step;
        }

        let hint = "space jump \u{b7} esc quit";
        let hy = mid(inner.bottom() - line_h, line_h);
        text(p, left, hy, hint, c.dim, false);
    }

    /// The field: clouds and hills going by slower than the ground, the
    /// ground, the course and the spark, `lead` in from the left.
    fn scene(&self, p: &mut Vec<Prim>, field: Rect, px: i32, lead: i32, c: &Ui) {
        let pxf = px as f32;
        let ground = field.y + GROUND * px;
        let cols = (field.w / px) as i64 + 2;
        // The course's x at the field's left edge, and the screen x of
        // `x` in a layer `depth` times as far away as the ground.
        let x0 = self.dist - BODY_X - lead as f32 / pxf;
        let sx = |x: f32, depth: f32| field.x + ((x - x0 / depth) * pxf).round() as i32;

        // Clouds, one in each 90 columns.
        let cloud = shade(c.side_bg, c.dim, 0.1);
        let k0 = (x0 / 10.0 / 90.0).floor() as i64 - 1;
        for k in k0..k0 + cols / 90 + 3 {
            let h = mix(k as u64);
            let x = sx(k as f32 * 90.0 + (h % 50) as f32, 10.0);
            let y = field.y + (3 + (h >> 8) as i32 % 14) * px;
            sprite::draw(p, &CLOUD, x, y, px, |_| Some(cloud));
        }

        // Hills, in steps two columns wide.
        let hill = shade(c.side_bg, c.dim, 0.07);
        let k0 = (x0 / 4.0 / 2.0).floor() as i64;
        let mut steps: Vec<(i32, i32)> = (k0..k0 + cols / 2 + 2)
            .map(|k| {
                let x = k as f32 * 2.0;
                let h = 10.0 + 6.0 * (x * 0.035).sin() + 3.0 * (x * 0.11 + 1.3).sin();
                (sx(x, 4.0), h.max(1.0) as i32)
            })
            .collect();
        steps.dedup_by_key(|s| s.1);
        for (i, &(x, h)) in steps.iter().enumerate() {
            let end = steps.get(i + 1).map_or(field.right(), |s| s.0);
            let r = Rect {
                x,
                y: ground - h * px,
                w: end - x,
                h: h * px,
            };
            p.push(Prim::Rect(r, hill));
        }

        // The ground, and dashes under it.
        let line = Rect {
            x: field.x,
            y: ground,
            w: field.w,
            h: px,
        };
        p.push(Prim::Rect(line, shade(c.dim, c.side_bg, 0.35)));
        let dash = shade(c.dim, c.side_bg, 0.55);
        let k0 = (x0 / 16.0).floor() as i64;
        for k in k0..k0 + cols / 16 + 2 {
            let h = mix(k as u64 ^ 0x5eed);
            let r = Rect {
                x: sx(k as f32 * 16.0 + (h % 12) as f32, 1.0),
                y: ground + (2 + 2 * ((h >> 8) % 3) as i32) * px,
                w: (1 + (h >> 16) % 4) as i32 * px,
                h: px,
            };
            p.push(Prim::Rect(r, dash));
        }

        // The course: art `h` tall whose bottom is `y` above the ground.
        let at = |x: f32, y: f32, h: i32| (sx(x, 1.0), ground - h * px - (y * pxf).round() as i32);
        let dark = |c: u32, t: f32| shade(c, 0x000000, t);
        let bug = |ch: char| match ch {
            'e' => Some(c.error),
            'd' => Some(dark(c.error, 0.25)),
            'h' => Some(shade(c.error, 0xffffff, 0.35)),
            'w' => Some(shade(c.error, 0xffffff, 0.85)),
            'k' => Some(dark(c.error, 0.55)),
            _ => None,
        };
        let block = |ch: char| match ch {
            'f' => Some(shade(c.dim, c.side_bg, 0.55)),
            'h' => Some(shade(c.dim, c.side_bg, 0.3)),
            'k' => Some(c.dim),
            _ => None,
        };
        let walk = usize::from(self.running && !self.over && (self.clock * 10.0) as i32 % 2 == 1);
        for o in &self.course {
            match o.kind {
                Kind::Bug | Kind::Swarm(_) => {
                    let n = match o.kind {
                        Kind::Swarm(n) => i32::from(n),
                        _ => 1,
                    };
                    for i in 0..n {
                        let (x, y) = at(o.x + (i * (BUG_W + 1)) as f32, 0.0, BUG_H);
                        sprite::draw(p, &BUG, x, y, px, bug);
                        let legs = [LEGS[(walk + i as usize) % 2]];
                        sprite::draw(p, &legs, x, y + (BUG_H - 1) * px, px, bug);
                    }
                }
                Kind::Stack(n) => {
                    for i in 0..i32::from(n) {
                        let (x, y) = at(o.x + (i % 2) as f32, (i * BLOCK_H) as f32, BLOCK_H);
                        sprite::draw(p, &BLOCK, x, y, px, block);
                    }
                }
            }
        }
        let bolt = |ch: char| match ch {
            'b' => Some(c.accent),
            'k' => Some(dark(c.accent, 0.45)),
            _ => None,
        };
        for &b in &self.bolts {
            let (x, y) = at(b, BOLT_Y, BOLT_H);
            sprite::draw(p, &BOLT, x, y, px, bolt);
        }

        // The spark: blinking, striding as it runs, grey once it fizzles.
        let (x, y) = at(self.dist - BODY_X, self.y, SPARK_H);
        let blink = self.clock.rem_euclid(3.2) > 3.05;
        let stride = !self.grounded() || (self.running && (self.dist / 7.0) as i64 % 2 == 1);
        let body = if self.over {
            shade(c.accent, c.dim, 0.7)
        } else {
            c.accent
        };
        let art = sprite::spark(!self.over && !blink, stride);
        sprite::draw(p, &art, x, y, px, sprite::spark_colors(body));
    }
}

/// Cuts the rectangles pushed since `from` to `r`, dropping any outside it.
fn clip(p: &mut Vec<Prim>, from: usize, r: Rect) {
    let rest = p.split_off(from);
    p.extend(rest.into_iter().filter_map(|q| match q {
        Prim::Rect(a, color) => {
            let (x0, y0) = (a.x.max(r.x), a.y.max(r.y));
            let cut = Rect {
                x: x0,
                y: y0,
                w: a.right().min(r.right()) - x0,
                h: a.bottom().min(r.bottom()) - y0,
            };
            (cut.w > 0 && cut.h > 0).then_some(Prim::Rect(cut, color))
        }
        q => Some(q),
    }));
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
        // A crash mid-write must not leave a cut-off score.
        let tmp = d.join("run-best.tmp");
        if std::fs::write(&tmp, best.to_string()).is_ok() {
            let _ = std::fs::rename(tmp, d.join("run-best"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f32 = 1.0 / 60.0;

    /// Jumps from the middle of the next obstacle's window, as a player
    /// with good timing would.
    fn autopilot(g: &mut Run) {
        let v = speed(g.dist);
        let next = (g.course.iter())
            .map(|o| o.window(v))
            .find(|w| w.1 >= g.dist);
        if let Some((a, b)) = next
            && g.grounded()
            && g.dist >= (a + b) / 2.0
        {
            g.jump();
        }
    }

    /// Steps `g` for `secs` seconds in steps of `dt`, flying it with the
    /// autopilot when `fly`; returns how many steps ended the run.
    fn play(g: &mut Run, secs: f32, dt: f32, fly: bool) -> u32 {
        let mut ends = 0;
        for _ in 0..(secs / dt) as u32 {
            if fly {
                autopilot(g);
            }
            ends += u32::from(g.step(dt));
        }
        ends
    }

    #[test]
    fn nothing_moves_or_scores_before_the_first_jump() {
        let mut g = Run::new(1, 0);
        let course = g.course.clone();
        play(&mut g, 5.0, FRAME, false);
        assert_eq!((g.dist, g.y, g.score, g.over), (0.0, 0.0, 0, false));
        assert_eq!(g.course, course);
    }

    #[test]
    fn jumps_only_from_the_ground_and_comes_back_down() {
        let mut g = Run::new(1, 0);
        g.jump();
        let mut top = 0.0f32;
        for _ in 0..6 {
            g.step(FRAME);
            top = top.max(g.y);
        }
        let vy = g.vy;
        g.jump();
        assert_eq!(g.vy, vy, "no jump in the air");
        while !g.grounded() {
            g.step(FRAME);
            top = top.max(g.y);
        }
        assert_eq!(g.y, 0.0);
        assert!((top - APEX).abs() < 0.5, "{top}");
        assert!(!g.over, "the first jump lands before the course");
    }

    #[test]
    fn hitting_an_obstacle_ends_the_run_once() {
        let mut g = Run::new(2, 0);
        g.jump();
        assert_eq!(play(&mut g, 20.0, FRAME, false), 1);
        assert!(g.over);
        assert!(!g.step(FRAME));
    }

    #[test]
    fn best_follows_the_score_and_survives_a_restart() {
        let mut g = Run::new(3, 5);
        g.jump();
        while !g.step(FRAME) {}
        assert!(g.score > 5);
        assert_eq!(g.best, g.score);
        let (best, course) = (g.best, g.course.clone());
        g.jump();
        assert!(g.over, "a held key does not start again at once");
        play(&mut g, COOLDOWN + 0.1, FRAME, false);
        g.jump();
        assert!(!g.over);
        assert_eq!((g.score, g.best, g.dist), (0, best, 0.0));
        assert_ne!(g.course, course, "a new course");
    }

    #[test]
    fn the_same_seed_lays_the_same_course() {
        let (mut a, mut b, mut c) = (Run::new(7, 0), Run::new(7, 0), Run::new(8, 0));
        for g in [&mut a, &mut b, &mut c] {
            g.jump();
            play(g, 30.0, FRAME, true);
        }
        assert_eq!(a.course, b.course);
        assert_eq!((a.dist, a.score), (b.dist, b.score));
        assert_ne!(a.course, c.course);
    }

    #[test]
    fn long_steps_do_not_pass_through_an_obstacle() {
        for dist in [0.0, 20_000.0] {
            let mut g = Run::new(4, 0);
            (g.running, g.dist) = (true, dist);
            g.plan();
            let o = *(g.course.iter())
                .find(|o| o.x > dist)
                .expect("an obstacle ahead");
            while !g.step(0.05) {}
            let (x, w, _) = o.hitbox();
            assert!(g.dist + BODY_W > x && g.dist < x + w, "{dist}: {}", g.dist);
        }
    }

    #[test]
    fn every_obstacle_can_be_cleared_after_the_last() {
        for seed in 0..20 {
            let mut g = Run::new(seed, 0);
            let mut all: Vec<Obstacle> = Vec::new();
            while g.dist < 30_000.0 {
                g.dist += 50.0;
                g.plan();
                let last = all.last().map_or(f32::MIN, |o| o.x);
                all.extend(g.course.iter().filter(|o| o.x > last));
            }
            for pair in all.windows(2) {
                let (v0, v1) = (speed(pair[0].x), speed(pair[1].x));
                let (a0, b0) = pair[0].window(v0);
                let (a1, b1) = pair[1].window(v1);
                assert!(b0 - a0 > 0.12 * v0, "{:?}", pair[0]);
                assert!(b1 - a1 > 0.12 * v1, "{:?}", pair[1]);
                assert!(a1 > b0 + v0 * (AIR + 0.1), "{pair:?}");
            }
        }
    }

    #[test]
    fn good_timing_runs_on_and_catches_bolts() {
        for seed in 0..8 {
            let mut g = Run::new(seed, 0);
            g.jump();
            assert_eq!(play(&mut g, 150.0, FRAME, true), 0, "seed {seed}");
            assert_eq!(speed(g.dist), FAST);
            assert!(g.caught > 0);
            assert_eq!(g.score, (g.dist / 10.0) as u32 + 10 * g.caught);
        }
    }

    /// A game before the start, in the air over the course, and fallen.
    fn states() -> Vec<(&'static str, Run)> {
        let ready = Run::new(42, 120);
        // Rising to a bolt, with a tall stack ahead.
        let mut air = Run::new(42, 120);
        air.jump();
        play(&mut air, 20.0, FRAME, true);
        let near = |g: &Run, x: f32, d: f32| x > g.dist && x < g.dist + d;
        while !(air.vy > 0.0
            && air.y > APEX / 2.0
            && air.bolts.iter().any(|&b| near(&air, b, 40.0))
            && (air.course.iter()).any(|o| o.size().1 > 12 && near(&air, o.x, 300.0)))
        {
            autopilot(&mut air);
            air.step(FRAME);
        }
        let mut over = Run::new(42, 120);
        over.jump();
        play(&mut over, 20.0, FRAME, false);
        vec![("ready", ready), ("air", air), ("over", over)]
    }

    #[test]
    fn everything_drawn_stays_in_the_panel() {
        let ui = crate::theme::blitz(false).ui;
        let sizes = [
            (
                Rect {
                    x: 0,
                    y: 0,
                    w: 1400,
                    h: 900,
                },
                1.0,
                (7, 15),
            ),
            (
                Rect {
                    x: 240,
                    y: 30,
                    w: 500,
                    h: 260,
                },
                1.0,
                (7, 15),
            ),
            (
                Rect {
                    x: 0,
                    y: 0,
                    w: 1100,
                    h: 500,
                },
                1.5,
                (10, 22),
            ),
            (
                Rect {
                    x: 360,
                    y: 0,
                    w: 2500,
                    h: 1400,
                },
                2.0,
                (14, 30),
            ),
        ];
        for (name, g) in states() {
            for &(area, scale, (tw, th)) in &sizes {
                let mut p = Vec::new();
                g.draw(&mut p, area, scale, &ui, (tw, th));
                let Some(&Prim::Rect(panel, _)) = p.first() else {
                    panic!("{name} {area:?}: nothing drawn");
                };
                let inside = |r: Rect| {
                    r.x >= panel.x
                        && r.y >= panel.y
                        && r.right() <= panel.right()
                        && r.bottom() <= panel.bottom()
                };
                assert!(inside(panel) && panel.x >= area.x && panel.right() <= area.right());
                for q in &p {
                    let r = match q {
                        Prim::Rect(r, _) | Prim::Shape { r, .. } | Prim::Branch(r, _) => *r,
                        Prim::Text { x, y, text, .. } => Rect {
                            x: *x,
                            y: *y,
                            w: text.chars().count() as i32 * tw,
                            h: th,
                        },
                    };
                    assert!(inside(r), "{name} {area:?}: {q:?} outside {panel:?}");
                }
            }
        }
    }

    #[test]
    fn nothing_is_drawn_in_a_tiny_area() {
        let ui = crate::theme::blitz(false).ui;
        for (_, g) in states() {
            let mut p = Vec::new();
            g.draw(
                &mut p,
                Rect {
                    x: 0,
                    y: 0,
                    w: 200,
                    h: 120,
                },
                1.0,
                &ui,
                (7, 15),
            );
            g.draw(
                &mut p,
                Rect {
                    x: 0,
                    y: 0,
                    w: 1400,
                    h: 120,
                },
                1.0,
                &ui,
                (7, 15),
            );
            assert!(p.is_empty());
        }
    }
}
