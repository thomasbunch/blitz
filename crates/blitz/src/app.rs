//! The window, its event loop and the frame loop.

// One process hosts every session, so a failed HRESULT must never panic.
#![deny(clippy::unwrap_used)]

use std::borrow::Cow;
use std::cell::RefCell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vt::grid::Found;
use vt::{
    Event, InputModes, KeyInput, Mods, MouseEv, MouseKind, MouseMode, Palette, PromptMark, Snapshot,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DwmSetWindowAttribute,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetKeyState, GetKeyboardState, GetLastInputInfo, LASTINPUTINFO,
};
use windows::Win32::UI::Shell::{
    ITaskbarList3, TBPF_ERROR, TBPF_INDETERMINATE, TBPF_NOPROGRESS, TBPF_NORMAL, TBPF_PAUSED,
    TaskbarList,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, MSG, SM_CXSMICON, SetForegroundWindow, TranslateMessage, WM_CHAR,
    WM_DEADCHAR, WM_KEYDOWN, WM_KEYUP, WM_SYSCHAR, WM_SYSDEADCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::windows::{
    EventLoopBuilderExtWindows, IconExtWindows, WindowAttributesExtWindows,
};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{CursorIcon, Fullscreen, Icon, UserAttentionType, Window, WindowId};

use crate::arcade::run::{self, Run};
use crate::attention::{Attn, Ev, claude_title, exit_text};
use crate::config::{Config, Kind};
use crate::debug::Counters;
use crate::keymap::{self, Action};
use crate::layout::{self, Axis, Dir, PaneId, Rect, Tab};
use crate::links::{Link, Target};
use crate::pane::{Note, Pane, Spawn, git_branch, lock, program_name};
use crate::render::chrome::{self, ChromeModel};
use crate::render::d3d11::{Gpu, Swapchain, is_device_lost};
use crate::render::{Renderer, text_snapshot, write_bmp};
use crate::session::{self, Geometry, PaneMeta};
use crate::settings::Panel;
use crate::theme::Theme;

const VK_PROCESSKEY: u16 = 0xe5;
const VK_PACKET: u16 = 0xe7;
const VK_RETURN: u16 = 0x0d;
const VK_ESCAPE: u16 = 0x1b;
const VK_BACK: u16 = 0x08;
const VK_PRIOR: u16 = 0x21;
const VK_NEXT: u16 = 0x22;
const VK_LEFT: u16 = 0x25;
const VK_UP: u16 = 0x26;
const VK_RIGHT: u16 = 0x27;
const VK_DOWN: u16 = 0x28;
const VK_DELETE: u16 = 0x2e;
const VK_SPACE: u16 = 0x20;
const VK_W: u16 = 0x57;
const VK_F3: u16 = 0x72;
const VK_F4: u16 = 0x73;

/// How long a notice that is neither a question nor an error stays up.
const NOTICE: Duration = Duration::from_secs(5);
/// How long a hint about setting something up stays up.
const HINT: Duration = Duration::from_secs(10);
/// How long Claude Code may show it is working with no word from its
/// hooks before blitz says they are not reporting. A turn's first hook
/// lands within a second of it starting.
const HOOKS_QUIET: Duration = Duration::from_secs(45);
/// Frame times for blitz run, and for scenery and the spark, which move
/// slowly.
const GAME_FRAME: Duration = Duration::from_millis(16);
const SCENERY_FRAME: Duration = Duration::from_millis(66);
/// The first look for a newer release waits until startup is done, then
/// one runs every few hours.
const UPDATE_FIRST: Duration = Duration::from_secs(10);
const UPDATE_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// Why an update or a look for one stopped when its thread panicked; only
/// that thread did.
const INTERNAL: &str = "an internal error";
/// Taskbar flashes per session are at least this far apart.
const FLASH_GAP: Duration = Duration::from_secs(10);
/// After this long with no key or mouse input anywhere, the user counts as
/// away from the screen, even with blitz in front.
const AWAY_AFTER: Duration = Duration::from_secs(30);
/// How long a restored pane waits for its shell's first prompt before it
/// types the Claude Code resume command anyway.
const RESUME_AFTER: Duration = Duration::from_secs(3);
/// Lines of output saved per pane when `restore_scrollback` is on.
const SAVED_LINES: usize = 1000;
/// Lines scrolled per wheel notch when the program takes no mouse input.
const WHEEL_LINES: isize = 3;
/// How long a changed layout waits before it is saved, so holding a resize
/// key writes the file at most once this often rather than every step. A
/// divider drag writes it once, after the drag.
const SAVE_DELAY: Duration = Duration::from_millis(500);
/// How long to wait before building the renderer again after it failed.
const GFX_RETRY: Duration = Duration::from_secs(1);
/// Time between the steps a drag scrolls while the pointer is held above
/// or below its pane.
const AUTOSCROLL: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub enum UserEvent {
    Pane(PaneId, Note),
    /// The git branch of a pane's directory, read on another thread.
    Branch(PaneId, String, Option<String>),
    /// Exit with this code: the self-test finished, or `--exit-after`
    /// ran out.
    Finish(i32),
    /// A newer release, by version, with the installer's log when an
    /// update to it failed.
    Update(String, Option<PathBuf>),
    /// What a look for a newer release that Ctrl+Shift+U asked for found.
    Checked(Result<Option<String>, String>),
    /// The installer started, so blitz exits; or why it did not.
    Installed(Result<(), String>),
    /// Another launch handed this folder over to open in a new tab.
    OpenHere(PathBuf),
    /// Something in `%APPDATA%\blitz` was written: settings or a theme.
    Settings,
}

/// Command-line options of the GUI.
#[derive(Default)]
struct Args {
    /// Run this command line instead of the shell.
    cmd: Option<String>,
    cwd: Option<PathBuf>,
    selftest: Option<PathBuf>,
    /// Save the last frame here before exiting.
    capture: Option<PathBuf>,
    exit_after: Option<Duration>,
    /// Open a window of its own, even if blitz is already running.
    new_window: bool,
}

impl Args {
    fn parse(args: &[String]) -> Result<Args, String> {
        let mut a = Args::default();
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            if flag == "--new-window" {
                a.new_window = true;
                continue;
            }
            let v = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
            match flag.as_str() {
                "--cmd" => a.cmd = Some(v.clone()),
                // Explorer passes a drive root as "C:\", and argv parsing
                // reads the \" as an escaped quote, so it arrives as C:".
                "--cwd" => match v.strip_suffix('"') {
                    Some(root) => a.cwd = Some(format!("{root}\\").into()),
                    None => a.cwd = Some(v.into()),
                },
                "--selftest" => a.selftest = Some(v.into()),
                "--capture" => a.capture = Some(v.into()),
                "--exit-after" => {
                    let ms = v.parse().map_err(|_| format!("not a number: {v}"))?;
                    a.exit_after = Some(Duration::from_millis(ms));
                }
                _ => return Err(format!("unknown option {flag}")),
            }
        }
        Ok(a)
    }
}

/// Runs the GUI until the window closes. Returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    let args = match Args::parse(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("blitz: {e}");
            return 2;
        }
    };
    // A folder opens as a tab in the blitz already running. Scripted and
    // test launches always get a window of their own.
    let scripted = args.cmd.is_some()
        || args.selftest.is_some()
        || args.exit_after.is_some()
        || args.capture.is_some();
    if let Some(dir) = &args.cwd
        && !args.new_window
        && !scripted
        && crate::handoff::send(dir)
    {
        return 0;
    }
    // Loading the graphics driver is most of the time to the first
    // frame; it runs while the window is made.
    let gpu = std::thread::spawn(|| Gpu::new(false));
    let keys = Rc::new(RefCell::new(Keys::default()));
    let hook_keys = keys.clone();
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    builder.with_msg_hook(move |msg| {
        // SAFETY: winit passes a pointer to the MSG it just peeked.
        let msg = unsafe { &*msg.cast::<MSG>() };
        hook(&hook_keys, msg)
    });
    let event_loop = match builder.build() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("blitz: {e}");
            return 1;
        }
    };
    let mut app = App::new(args, keys, event_loop.create_proxy());
    app.gpu = Some(gpu);
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("blitz: {e}");
        return 1;
    }
    app.code
}

/// Key input taken from raw window messages, waiting for the app.
#[derive(Default)]
struct Keys {
    queue: Vec<Input>,
    /// A dead key was pressed; the next key's text comes as WM_CHAR.
    dead: bool,
    /// WM_CHAR messages carry text to send: Unicode packets from
    /// SendInput, and what a dead key composed.
    chars: bool,
    /// High half of a surrogate pair from WM_CHAR.
    high: Option<u16>,
    /// Keys whose release was already handled as text. Several can be
    /// down at once, as digits typed fast for an Alt code.
    skip_up: Eaten,
}

enum Input {
    /// A key transition; its text is the second field, and the third says
    /// it is an auto-repeat of a held key.
    Key(KeyInput<'static>, String, bool),
    /// Text to send as is.
    Text(String),
}

/// The message hook: takes keyboard messages before winit sees them, so
/// every transition is encoded from the raw message, including modifier
/// keys and releases. Returns true for messages it consumed.
fn hook(keys: &RefCell<Keys>, msg: &MSG) -> bool {
    let mut k = keys.borrow_mut();
    match msg.message {
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
            let vk = msg.wParam.0 as u16;
            let down = matches!(msg.message, WM_KEYDOWN | WM_SYSKEYDOWN);
            let mut state = [0u8; 256];
            // SAFETY: a valid buffer; the state already includes this key.
            let _ = unsafe { GetKeyboardState(&mut state) };
            let held = |vk: usize| state[vk] & 0x80 != 0;
            // The input method owns the key; winit turns it into IME events.
            if vk == VK_PROCESSKEY {
                return false;
            }
            // Alt+F4 still closes the window.
            if vk == VK_F4 && held(0x12) && !held(0x11) && !held(0x10) {
                return !alt_f4_passes(down, msg.lParam.0);
            }
            if k.skipped(vk, down) {
                return true;
            }
            let mut text = String::new();
            let input =
                keymap::msg_to_key(vk, msg.lParam.0, &state, keymap::system_layout, &mut text);
            // Alt with keypad digits types a character by its code, which
            // Windows sends as WM_CHAR once Alt is released.
            let alt_code = (0x60..=0x69).contains(&vk) && held(0x12) && !held(0x11);
            k.key(&input, alt_code, keymap::held_before(msg.lParam.0));
            // Still translate: it keeps the dead-key state right and posts
            // the WM_CHAR that carries composed text.
            // SAFETY: a valid message from the queue.
            let _ = unsafe { TranslateMessage(msg) };
            true
        }
        // winit mishandles dead keys (rust-windowing/winit#4635); the
        // composed character arrives as WM_CHAR after the next key.
        WM_DEADCHAR | WM_SYSDEADCHAR => {
            k.dead = true;
            true
        }
        WM_CHAR | WM_SYSCHAR => {
            k.unit(msg.wParam.0 as u16);
            true
        }
        _ => false,
    }
}

impl Keys {
    /// Whether this is the release of a key whose text came as WM_CHAR,
    /// which was handled along with it.
    fn skipped(&mut self, vk: u16, down: bool) -> bool {
        !down && self.skip_up.release(vk)
    }

    /// A key transition from a key message. Its text comes as WM_CHAR
    /// instead for a Unicode packet, the key after a dead key, and
    /// keypad digits typed with Alt (`alt_code`), so it is not queued.
    fn key(&mut self, input: &KeyInput, alt_code: bool, held: bool) {
        let (vk, down) = (input.vk, input.down);
        let modifier = matches!(
            input.key,
            vt::Key::Shift | vt::Key::Control | vt::Key::Alt | vt::Key::Super
        );
        if down && !modifier {
            let after_dead = self.dead && matches!(input.key, vt::Key::Char(_));
            self.chars = vk == VK_PACKET || alt_code || after_dead;
            self.dead = false;
        }
        if vk == VK_PACKET || self.chars && down && !modifier {
            self.skip_up.press(vk);
        } else {
            self.queue
                .push(Input::Key(owned(input), input.text.to_string(), held));
        }
    }

    /// A UTF-16 unit from WM_CHAR: queued as text when a key said its
    /// text comes this way. Text for other keys was already sent from the
    /// key message.
    fn unit(&mut self, unit: u16) {
        if !self.chars {
            return;
        }
        let units = match (self.high.take(), unit) {
            (_, 0xd800..=0xdbff) => {
                self.high = Some(unit);
                vec![]
            }
            (Some(h), 0xdc00..=0xdfff) => vec![h, unit],
            _ => vec![unit],
        };
        let text = String::from_utf16_lossy(&units);
        if !text.is_empty() && !text.chars().any(char::is_control) {
            self.queue.push(Input::Text(text));
        }
    }
}

/// Keys whose press blitz kept for a shortcut, the theme picker or the
/// settings panel, so their release is kept too. Several can be down at
/// once, Shift and a letter typed into the picker say.
#[derive(Default)]
struct Eaten(Vec<u16>);

impl Eaten {
    fn press(&mut self, vk: u16) {
        if !self.0.contains(&vk) {
            self.0.push(vk);
        }
    }

    /// Whether the release of `vk` is kept; it is forgotten either way.
    fn release(&mut self, vk: u16) -> bool {
        let kept = self.0.contains(&vk);
        self.0.retain(|&v| v != vk);
        kept
    }
}

/// A copy of `k` that borrows nothing; the text travels separately.
fn owned(k: &KeyInput) -> KeyInput<'static> {
    KeyInput {
        vk: k.vk,
        scan: k.scan,
        extended: k.extended,
        down: k.down,
        repeat: k.repeat,
        mods: k.mods,
        locks: k.locks,
        text: "",
        uc: k.uc,
        cs: k.cs,
        key: k.key,
        us_base: k.us_base,
    }
}

/// Modifier keys held right now, for mouse events.
fn mods_now() -> Mods {
    // SAFETY: plain state queries.
    let held = |vk: i32| unsafe { GetKeyState(vk) } < 0;
    Mods {
        lshift: held(0xa0),
        rshift: held(0xa1),
        lctrl: held(0xa2),
        rctrl: held(0xa3),
        lalt: held(0xa4),
        ralt: held(0xa5),
        lsuper: held(0x5b),
        rsuper: held(0x5c),
    }
}

/// A one-row message drawn over the bottom row of a pane.
struct Notice {
    text: String,
    /// Removed at this time; `None` keeps it until something replaces it.
    until: Option<Instant>,
    dim: bool,
    ask: Ask,
}

/// What a notice waits for. One that asks to confirm stays armed while
/// it shows, with no deadline to read it by.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Ask {
    Nothing,
    /// An error: the next key in its pane takes it away, read by then.
    Key,
    /// The action that showed it, run again in its pane, confirms. Any
    /// other key takes it away and goes on to do what it does.
    Paste(String),
    ClosePane,
    CloseTab,
    Update,
    /// Closing the window again confirms; any key takes it away.
    Quit,
}

impl Ask {
    /// Whether a key that runs `a`, if any, takes the notice away. `here`
    /// when the key goes to the notice's pane. Opening the palette does
    /// not, since an action run from it confirms too. A question about its
    /// own pane goes at any key aimed at another, so the same action there
    /// cannot answer it later.
    fn gone(&self, a: Option<Action>, here: bool) -> bool {
        let by = match self {
            Ask::Nothing => return false,
            Ask::Key => return here,
            Ask::Paste(_) | Ask::ClosePane | Ask::CloseTab if !here => return true,
            Ask::Paste(_) => Action::Paste,
            Ask::ClosePane => Action::ClosePane,
            Ask::CloseTab => Action::CloseTab,
            Ask::Update => Action::Update,
            Ask::Quit => return true,
        };
        a != Some(by) && a != Some(Action::Palette)
    }

    /// Whether the question is about its own pane, so focus leaving that
    /// pane takes it away.
    fn of_pane(&self) -> bool {
        matches!(self, Ask::Paste(_) | Ask::ClosePane | Ask::CloseTab)
    }
}

/// The renderer and the window's swap chain, rebuilt together after the
/// GPU device is lost.
struct Gfx {
    r: Renderer,
    chain: Swapchain,
}

#[derive(Default)]
struct Mouse {
    pos: PhysicalPosition<f64>,
    /// For the left, middle and right button, the pane whose program was
    /// sent its press.
    reported: [Option<PaneId>; 3],
    tracker: vt::keys::MouseTracker,
    /// Wheel movement not yet turned into whole steps.
    wheel: f64,
    /// A left-button drag is moving this divider of the active tab; the
    /// second value is the smallest pane it may leave.
    divider: Option<(usize, (i32, i32))>,
    /// The pointer is over a divider along this axis and shows it.
    over_divider: Option<Axis>,
    /// A left-button drag is making a selection.
    drag: Option<Drag>,
    /// When the drag next scrolls, while the pointer is outside its pane.
    scroll_at: Option<Instant>,
    /// The last press that went to selection: when, on which cell, and
    /// how many clicks it made.
    click: Option<(Instant, Pos, u8)>,
}

/// A cell as a line number and a column. Line numbers stay with their
/// text while output scrolls it into scrollback; see [`vt::Terminal::lines`].
type Pos = vt::terminal::LineCol;

/// Characters a double click takes as part of a word besides letters and
/// digits, so it takes a whole path or URL.
const WORD: &str = "_-./\\:~@?=&%+#";

/// Rows either side of a cell that [`Logical`] reads at most.
const LOGICAL_ROWS: usize = 64;

/// How a selection grows while the button is held, fixed by the press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Drag {
    /// [`vt::Terminal::line_epoch`] at the press: line numbers from
    /// another epoch name other text.
    epoch: u32,
    /// What the press took, first and last cell: the cell pressed, or the
    /// word or line around it.
    anchor: (Pos, Pos),
    /// Clicks of the press: 1 grows the selection by cells, 2 by words, 3
    /// by lines.
    unit: u8,
    /// Alt was held: the same columns of every row between the corners.
    block: bool,
}

impl Drag {
    /// A press on `at` that made `unit` clicks.
    fn new(term: &vt::Terminal, pal: &Palette, at: Pos, unit: u8, block: bool) -> Drag {
        let anchor = match unit {
            2 => word_at(term, pal, at),
            3 => line_at(term, at),
            _ => char_at(term, pal, at),
        };
        Drag {
            epoch: term.line_epoch(),
            anchor,
            unit,
            block,
        }
    }
}

/// Clicks a press on `at` makes: one more than the last press when that
/// was on the same cell within the double-click time `within`, and after
/// three, one again.
fn clicks(last: Option<(Instant, Pos, u8)>, at: Pos, now: Instant, within: Duration) -> u8 {
    match last {
        Some((t, p, n)) if p == at && now.saturating_duration_since(t) <= within => n % 3 + 1,
        _ => 1,
    }
}

/// Selected cells in the focused pane.
struct Selection {
    drag: Drag,
    /// The first and last cell in reading order; a block's top-left and
    /// bottom-right corners.
    start: Pos,
    end: Pos,
    /// The screen's top line at the last look, and the selected text from
    /// there on. Only lines that were on the screen can have been
    /// rewritten since.
    seen: (usize, String),
}

impl Selection {
    /// What `drag` selects with the pointer on `head`.
    fn new(term: &vt::Terminal, pal: &Palette, drag: Drag, head: Pos) -> Selection {
        let (a, b) = drag.anchor;
        let (start, end) = if drag.block {
            let (top, bottom) = (a.0.min(head.0), a.0.max(head.0));
            ((top, a.1.min(head.1)), (bottom, a.1.max(head.1)))
        } else {
            let h = Drag::new(term, pal, head, drag.unit, false).anchor;
            (a.min(h.0), b.max(h.1))
        };
        let mut s = Selection {
            drag,
            start,
            end,
            seen: (0, String::new()),
        };
        s.look(term, pal);
        s
    }

    /// Whether the lines are still the ones picked: none dropped off the
    /// top of scrollback and the numbers did not start over.
    fn kept(&self, term: &vt::Terminal) -> bool {
        term.line_epoch() == self.drag.epoch && term.lines().contains(&self.start.0)
    }

    /// Catches up with output since the last look. False when it rewrote
    /// selected text, which ends the selection, so a copy never takes text
    /// the user did not pick.
    fn still(&mut self, term: &vt::Terminal, pal: &Palette) -> bool {
        if !self.kept(term) || selection_text(term, pal, self, self.seen.0) != self.seen.1 {
            return false;
        }
        self.look(term, pal);
        true
    }

    fn look(&mut self, term: &vt::Terminal, pal: &Palette) {
        let top = term.screen_top();
        self.seen = (top, selection_text(term, pal, self, top));
    }
}

/// The cells from `start` to `end`, or the block they are the corners
/// of, that a view of `cols` by `rows` cells from line `top` shows, as
/// (column, row) of the first and last.
fn in_view(
    start: Pos,
    end: Pos,
    block: bool,
    top: usize,
    (cols, rows): (u16, u16),
) -> Option<((u16, u16), (u16, u16))> {
    let bottom = top + usize::from(rows);
    if end.0 < top || start.0 >= bottom || cols == 0 {
        return None;
    }
    let row = |n: usize| (n - top) as u16;
    let a = match start.0 < top {
        true if block => (start.1, 0),
        true => (0, 0),
        false => (start.1, row(start.0)),
    };
    let b = match end.0 >= bottom {
        true if block => (end.1, rows - 1),
        true => (cols - 1, rows - 1),
        false => (end.1, row(end.0)),
    };
    Some((a, b))
}

/// The drawn text of a logical line, its rows joined where they wrap.
struct Logical {
    text: String,
    /// Each cell's place in `text`, the cell, and its width.
    cells: Vec<(usize, Pos, u8)>,
}

impl Logical {
    /// The logical line through line `n`, at most [`LOGICAL_ROWS`] rows
    /// either side of it.
    fn new(term: &vt::Terminal, pal: &Palette, n: usize) -> Logical {
        let first = term.lines().start;
        let mut top = n;
        while top > first && n - top < LOGICAL_ROWS && term.wraps(top - 1) {
            top -= 1;
        }
        let mut l = Logical {
            text: String::new(),
            cells: Vec::new(),
        };
        let mut row = Vec::new();
        let mut line = top;
        while let Some(wraps) = term.line_cells(line, pal, &mut row) {
            for (col, c) in row.iter().enumerate().filter(|(_, c)| c.width > 0) {
                l.cells.push((l.text.len(), (line, col as u16), c.width));
                push_cells(&mut l.text, &row, col, col);
            }
            if !wraps || line >= n + LOGICAL_ROWS {
                break;
            }
            line += 1;
        }
        l
    }

    /// Which of `cells` covers `at`.
    fn index(&self, at: Pos) -> Option<usize> {
        (self.cells.iter())
            .position(|&(_, (n, c), w)| n == at.0 && (c..c + u16::from(w)).contains(&at.1))
    }

    /// The first and last cell that hold `text[range]`; the last cell's
    /// last column for a wide one.
    fn span(&self, range: std::ops::Range<usize>) -> (Pos, Pos) {
        let i = self.cells.partition_point(|c| c.0 <= range.start);
        let j = self.cells.partition_point(|c| c.0 < range.end);
        let (_, (n, c), w) = self.cells[j.max(1) - 1];
        (self.cells[i.max(1) - 1].1, (n, c + u16::from(w) - 1))
    }
}

/// The cells of the character at `at`: both halves of a wide one, so
/// what is highlighted is what is copied.
fn char_at(term: &vt::Terminal, pal: &Palette, at: Pos) -> (Pos, Pos) {
    let mut cells = Vec::new();
    if term.line_cells(at.0, pal, &mut cells).is_none() {
        return (at, at);
    }
    let width = |x: u16| cells.get(usize::from(x)).map(|c| c.width);
    match width(at.1) {
        Some(0) if at.1 > 0 => ((at.0, at.1 - 1), at),
        Some(2) if width(at.1 + 1).is_some() => (at, (at.0, at.1 + 1)),
        _ => (at, at),
    }
}

/// The word around `at`, following soft wraps: a run of word characters
/// or of blanks, or any other character alone.
fn word_at(term: &vt::Terminal, pal: &Palette, at: Pos) -> (Pos, Pos) {
    let l = Logical::new(term, pal, at.0);
    let Some(i) = l.index(at) else {
        return (at, at);
    };
    let class = |k: usize| match l.text[l.cells[k].0..].chars().next() {
        Some(c) if c.is_alphanumeric() || WORD.contains(c) => 1,
        Some(' ') => 2,
        _ => 0,
    };
    let (mut a, mut b, me) = (i, i, class(i));
    while me != 0 && a > 0 && class(a - 1) == me {
        a -= 1;
    }
    while me != 0 && b + 1 < l.cells.len() && class(b + 1) == me {
        b += 1;
    }
    l.span(l.cells[a].0..l.cells[b].0 + 1)
}

/// The logical line through `at`: every row joined to it by soft wraps.
fn line_at(term: &vt::Terminal, at: Pos) -> (Pos, Pos) {
    let lines = term.lines();
    let (mut a, mut b) = (at.0, at.0);
    while a > lines.start && term.wraps(a - 1) {
        a -= 1;
    }
    while b + 1 < lines.end && term.wraps(b) {
        b += 1;
    }
    ((a, 0), (b, u16::MAX))
}

/// A session and what the window keeps to draw it.
struct View {
    pane: Pane,
    snap: Snapshot,
    /// The terminal's size in cells.
    grid: (u16, u16),
    /// Where the grid went in the last frame; `None` while its tab is
    /// hidden.
    rect: Option<Rect>,
    notice: Option<Notice>,
    /// When the taskbar last flashed for this session.
    flashed: Option<Instant>,
    /// A thread is reading the git branch of the session's directory.
    finding_branch: bool,
    /// A restored Claude Code session: the line to type at the shell's
    /// first prompt, and when to type it anyway.
    resume: Option<(String, Instant)>,
    /// The shell has shown blitz's prompt mark, so the next one means
    /// what ran in it has ended.
    prompted: bool,
    /// When the program's open synchronized update times out, as of the
    /// last look at its terminal.
    sync_until: Option<Instant>,
    /// Names the session's saved output; kept across restarts.
    key: String,
    /// The command line it runs in place of the shell.
    cmd: Option<String>,
    /// The progress the program last reported, and when.
    progress: Option<(chrome::Progress, Instant)>,
    /// When the title first showed Claude Code working.
    claude_working: Option<Instant>,
    /// A hook notification came, so Claude Code's hooks report.
    hooks_seen: bool,
}

impl View {
    /// What closing the session would cut short; see [`crate::attention::busy`].
    fn busy(&self) -> Option<&'static str> {
        let p = &self.pane;
        (p.exit_code.is_none())
            .then(|| crate::attention::busy(p.attn.state, &p.cmd, p.claude.is_some()))
            .flatten()
    }
}

struct App {
    args: Args,
    config: Config,
    keys: Rc<RefCell<Keys>>,
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Window>,
    hwnd: isize,
    gfx: Option<Gfx>,
    theme: Theme,
    /// The theme picker, while it is open.
    picker: Option<Picker>,
    /// The settings panel, while it is open. The theme picker opens over it.
    settings: Option<Panel>,
    /// Where the settings panel was in the last frame, for clicks.
    settings_hits: Option<chrome::SettingsHits>,
    /// blitz run while it is open, and when it last moved.
    game: Option<(Run, Instant)>,
    /// When a session needing the user last closed blitz run.
    game_ended: Option<Instant>,
    /// Windows shows animations; scenery and the spark keep still if not.
    motion: bool,
    /// The command palette, while it is open.
    commands: Option<Commands>,
    /// Where the command palette and its rows were in the last frame, for
    /// clicks.
    commands_hits: Option<(Rect, Vec<(usize, Rect)>)>,
    /// Points Ctrl+= and Ctrl+- add to the font size in the settings.
    /// Never saved, and dropped when that setting changes.
    font_zoom: f32,
    /// The find bar of the focused pane, while it is open.
    find: Option<Find>,
    scale: f64,
    /// Tabs and the split tree in each.
    win: layout::Window,
    /// Every session, oldest first, which is the order the sidebar lists.
    views: Vec<View>,
    /// Each session's sidebar row in the last frame, for clicks.
    rows: Vec<(PaneId, Rect)>,
    next_id: u32,
    focused: bool,
    /// A session changed while the user was away from the screen; the
    /// focused pane counts as seen once they are back.
    away: bool,
    /// A selection in the focused pane.
    selection: Option<Selection>,
    /// The link under the pointer while Ctrl is held, drawn underlined:
    /// the line epoch and its first and last cell.
    hover: Option<(u32, Pos, Pos)>,
    mouse: Mouse,
    /// IME composition text, drawn at the cursor.
    preedit: String,
    /// A newer release: its version and the banner text.
    update: Option<(String, String)>,
    /// The installer is downloading, or Ctrl+Shift+U is looking for a
    /// release; this pane shows that.
    updating: Option<PaneId>,
    /// The folder and Claude Code session of the pane closed last, which
    /// the palette can reopen.
    closed: Option<(String, Option<String>)>,
    /// The banner strip in the last frame, for clicks.
    banner: Option<Rect>,
    /// Keys whose releases belong to a shortcut or a panel and are not sent.
    eaten: Eaten,
    /// Where the IME was last told the cursor is, in client pixels.
    ime_at: Option<(i32, i32)>,
    /// Checked once the first output shows which ConPTY is running.
    checked_conpty: bool,
    capture_then_exit: bool,
    /// This is the main window, whose layout is saved for the next start.
    /// Separate windows and scripted runs leave the saved one alone.
    persist: bool,
    /// The session as last saved.
    saved: Option<session::State>,
    /// When a changed layout is saved, unless it changes back first.
    save_after: Option<Instant>,
    /// No renderer could be built; the next try is not before this.
    gfx_retry: Option<Instant>,
    /// Where the window last was while not minimized, maximized or full
    /// screen.
    placed: Geometry,
    /// The terminal a running self-test reads: the focused pane's.
    watched: Option<Arc<selftest::Focus>>,
    started: Instant,
    /// The device started at launch, until the first renderer takes it.
    gpu: Option<std::thread::JoinHandle<windows::core::Result<Gpu>>>,
    /// The taskbar button, for progress; made the first time it is needed.
    taskbar: Option<ITaskbarList3>,
    /// The progress the taskbar button shows.
    taskbar_shows: Option<chrome::Progress>,
    /// blitz's Claude Code plugin, which every pane loads; `None` when it
    /// could not be written.
    plugin: Option<String>,
    /// A hint about Claude Code's hooks was shown; one per run is enough.
    hooks_hinted: bool,
    counters: Counters,
    code: i32,
}

/// The open theme picker.
struct Picker {
    themes: Vec<Theme>,
    /// Typed text; themes whose names contain it, ignoring case, match.
    filter: String,
    /// The highlighted theme, among the matching ones.
    sel: usize,
}

impl Picker {
    fn matches(&self) -> Vec<&Theme> {
        let f = self.filter.to_lowercase();
        (self.themes.iter())
            .filter(|t| t.name.to_lowercase().contains(&f))
            .collect()
    }

    /// Arrows and Page Up and Down move the highlight, which stays on the
    /// list; typing narrows the list and Backspace widens it, each going
    /// back to the first match. Returns false for keys it has no use for.
    fn key(&mut self, k: &KeyInput) -> bool {
        let m = &k.mods;
        // Ctrl and Alt together are AltGr when the layout gives a character.
        let (ctrl, alt) = (m.lctrl || m.rctrl, m.lalt || m.ralt);
        let chord = ctrl != alt || (ctrl && k.uc == 0);
        let (sel, last) = (self.sel as isize, self.matches().len().saturating_sub(1));
        let step = |by: isize| (sel + by).clamp(0, last as isize) as usize;
        match k.vk {
            VK_UP => self.sel = step(-1),
            VK_DOWN => self.sel = step(1),
            VK_PRIOR => self.sel = step(-(chrome::PICKER_ROWS as isize)),
            VK_NEXT => self.sel = step(chrome::PICKER_ROWS as isize),
            VK_BACK if self.filter.pop().is_some() => self.sel = 0,
            _ if !chord && !k.text.is_empty() => {
                self.filter.push_str(k.text);
                self.sel = 0;
            }
            _ => return false,
        }
        true
    }
}

/// The open command palette.
#[derive(Default)]
struct Commands {
    /// Typed text; actions whose label or `config.toml` name holds every
    /// typed word, ignoring case, match.
    filter: String,
    /// The highlighted action, among the matching ones.
    sel: usize,
    /// What the typed text names instead, with no actions to pick.
    rename: Option<Rename>,
}

/// What the command palette's line names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rename {
    Session(PaneId),
    /// The tab that holds this session.
    Tab(PaneId),
}

impl Commands {
    /// The matching actions and their labels, in [`keymap::ACTIONS`] order.
    /// The palette leaves itself out.
    fn matches(&self) -> Vec<(Action, &'static str)> {
        if self.rename.is_some() {
            return Vec::new();
        }
        let words: Vec<String> = (self.filter.split_whitespace())
            .map(str::to_lowercase)
            .collect();
        (keymap::ACTIONS.iter())
            .filter(|a| a.0 != Action::Palette)
            .filter(|a| {
                let text = format!("{} {}", a.2, a.1).to_lowercase();
                words.iter().all(|w| text.contains(w.as_str()))
            })
            .map(|a| (a.0, a.2))
            .collect()
    }

    /// Moves the highlight `by` rows, stopping at either end.
    fn move_by(&mut self, by: isize) {
        let last = self.matches().len().saturating_sub(1);
        self.sel = self.sel.saturating_add_signed(by).min(last);
    }
}

/// The open find bar.
struct Find {
    /// The pane it searches, which has focus.
    pane: PaneId,
    query: String,
    /// Every match, oldest first.
    found: Vec<Found>,
    /// The current match; `None` when there are none.
    cur: Option<usize>,
    /// Output came since the last search.
    stale: bool,
}

impl Find {
    fn new(pane: PaneId) -> Find {
        Find {
            pane,
            query: String::new(),
            found: Vec::new(),
            cur: None,
            stale: false,
        }
    }

    /// Searches `term` again. The current match stays on the one that
    /// starts where it did, or else the nearest one above it; with none
    /// yet, it is the nearest one above the bottom of the `rows` high view.
    fn search(&mut self, term: &vt::Terminal, rows: u16) {
        let anchor = match self.cur.and_then(|i| self.found.get(i)) {
            Some(m) => m.start,
            None => (term.view_top() + usize::from(rows.max(1)) - 1, u16::MAX),
        };
        self.found = term.find(&self.query);
        self.stale = false;
        let above = self.found.partition_point(|m| m.start <= anchor);
        self.cur = (!self.found.is_empty()).then(|| above.saturating_sub(1));
    }

    /// Moves `by` matches, down when positive, wrapping at either end.
    fn step(&mut self, by: isize) {
        let n = self.found.len() as isize;
        self.cur = (self.cur).map(|i| (i as isize + by).rem_euclid(n) as usize);
    }
}

/// Sends [`UserEvent::Settings`] whenever a file in `%APPDATA%\blitz`, or
/// its themes folder, is written. Creates the themes folder, so there is
/// a place to drop theme files.
fn watch_settings(proxy: EventLoopProxy<UserEvent>) {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::WAIT_OBJECT_0;
    use windows::Win32::Storage::FileSystem::{
        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FindFirstChangeNotificationW,
        FindNextChangeNotification,
    };
    use windows::Win32::System::Threading::{INFINITE, WaitForSingleObject};
    use windows::core::PCWSTR;

    let (Some(dir), Some(themes)) = (crate::config::dir(), crate::theme::dir()) else {
        return;
    };
    let _ = std::fs::create_dir_all(themes);
    std::thread::spawn(move || {
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: a NUL-terminated path that outlives the call.
        let Ok(h) = (unsafe {
            FindFirstChangeNotificationW(
                PCWSTR(wide.as_ptr()),
                true,
                FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE,
            )
        }) else {
            return;
        };
        // SAFETY: `h` stays open for the life of the thread, which is the
        // life of the process.
        while unsafe { WaitForSingleObject(h, INFINITE) } == WAIT_OBJECT_0 {
            // Editors save in several writes; take them as one.
            std::thread::sleep(Duration::from_millis(100));
            // SAFETY: as above.
            let next = unsafe { FindNextChangeNotification(h) };
            if next.is_err() || proxy.send_event(UserEvent::Settings).is_err() {
                return;
            }
        }
    });
}

impl App {
    fn new(args: Args, keys: Rc<RefCell<Keys>>, proxy: EventLoopProxy<UserEvent>) -> App {
        let config = Config::load();
        let theme = crate::theme::current(&config.theme);
        let a = &args;
        let persist = !a.new_window
            && a.cmd.is_none()
            && a.selftest.is_none()
            && a.exit_after.is_none()
            && a.capture.is_none();
        App {
            args,
            config,
            keys,
            proxy,
            window: None,
            hwnd: 0,
            gfx: None,
            theme,
            picker: None,
            settings: None,
            settings_hits: None,
            game: None,
            game_ended: None,
            motion: animations_on(),
            commands: None,
            commands_hits: None,
            font_zoom: 0.0,
            find: None,
            scale: 1.0,
            win: layout::Window::default(),
            views: Vec::new(),
            rows: Vec::new(),
            next_id: 1,
            focused: false,
            away: false,
            selection: None,
            hover: None,
            mouse: Mouse::default(),
            preedit: String::new(),
            update: None,
            updating: None,
            closed: None,
            banner: None,
            eaten: Eaten::default(),
            ime_at: None,
            checked_conpty: false,
            capture_then_exit: false,
            persist,
            saved: None,
            save_after: None,
            gfx_retry: None,
            placed: Geometry::default(),
            watched: None,
            started: Instant::now(),
            gpu: None,
            taskbar: None,
            taskbar_shows: None,
            plugin: None,
            hooks_hinted: false,
            counters: Counters::default(),
            code: 0,
        }
    }

    fn font_px(&self) -> f32 {
        (self.config.font_size + self.font_zoom) * 96.0 / 72.0 * self.scale as f32
    }

    /// Creates the window and starts the first session.
    fn start(&mut self, el: &ActiveEventLoop) -> Result<(), String> {
        let mut attrs = Window::default_attributes()
            .with_title("blitz")
            .with_inner_size(LogicalSize::new(980.0, 620.0))
            // Icon group 1, which build.rs links in.
            .with_window_icon(Icon::from_resource(1, Some(small_icon_size())).ok())
            .with_taskbar_icon(Icon::from_resource(1, None).ok());
        // Only the main window takes folders from other launches.
        if !self.args.new_window {
            attrs = attrs.with_class_name(crate::handoff::CLASS);
        }
        let saved = (self.persist && self.config.restore_session)
            .then(session::load)
            .flatten();
        if let Some(g) = saved
            .as_ref()
            .map(|s| s.window)
            .filter(|g| g.w > 0 && g.h > 0)
        {
            let g = on_screen(el, g);
            self.placed = g;
            attrs = attrs
                .with_position(PhysicalPosition::new(g.x, g.y))
                .with_inner_size(PhysicalSize::new(g.w, g.h))
                .with_maximized(g.maximized);
        }
        let window = el.create_window(attrs).map_err(|e| e.to_string())?;
        window.set_ime_allowed(true);
        self.scale = window.scale_factor();
        if let Ok(h) = window.window_handle()
            && let RawWindowHandle::Win32(h) = h.as_raw()
        {
            self.hwnd = h.hwnd.get();
        }
        if !self.args.new_window {
            crate::handoff::install(self.hwnd, self.proxy.clone());
        }
        watch_settings(self.proxy.clone());
        self.plugin = crate::hook::install_plugin().map(|d| d.to_string_lossy().into_owned());
        self.frame_theme();
        self.window = Some(window);
        self.ensure_gfx();

        let mut win = layout::Window::default();
        let mut lost = None;
        if let Some(s) = &saved {
            match self.restore(s) {
                Ok(()) => win = self.win.clone(),
                Err(e) => {
                    eprintln!("blitz: restoring the last session: {e}");
                    // Keep it for the next start rather than saving this
                    // run's fresh tab over it.
                    self.persist = false;
                    lost = Some(e);
                }
            }
        }
        // A folder from Explorer gets a tab of its own after the restored ones.
        if self.views.is_empty() || self.args.cwd.is_some() {
            let cwd = match &self.args.cwd {
                Some(dir) => start_dir(dir),
                None => std::env::current_dir().ok(),
            };
            let id = PaneId(self.next_id);
            win.tabs.push(Tab::new(String::new(), id));
            win.active = win.tabs.len() - 1;
            let cmd = self.args.cmd.clone();
            self.open(win, id, cmd.as_deref(), cwd)?;
            // Said where it is seen: the release build has no console. It
            // stays, as nothing this window does is saved.
            if let Some(e) = lost {
                let text = format!(
                    "The last session did not come back ({e}); it is kept for the next start, and this window is not saved"
                );
                self.set_notice(id, text, None, false);
            }
        }

        if let Some(script) = self.args.selftest.clone() {
            self.start_selftest(script);
        }
        if let Some(after) = self.args.exit_after {
            let proxy = self.proxy.clone();
            std::thread::spawn(move || {
                std::thread::sleep(after);
                let _ = proxy.send_event(UserEvent::Finish(0));
            });
        }
        let scripted = self.args.selftest.is_some() || self.args.exit_after.is_some();
        if !scripted && !cfg!(debug_assertions) {
            let proxy = self.proxy.clone();
            let look = self.config.check_updates;
            std::thread::spawn(move || {
                std::thread::sleep(UPDATE_FIRST);
                // Ctrl+Shift+U updates with checks off too, so a failed
                // update is shown, and old ones cleared, either way.
                if let Some((v, log)) = crate::update::failed() {
                    let _ = proxy.send_event(UserEvent::Update(v, Some(log)));
                }
                if !look {
                    return;
                }
                loop {
                    // Quiet when it fails, as offline is normal; Ctrl+Shift+U
                    // says why.
                    if let Ok(Some(v)) = crate::update::check()
                        && proxy.send_event(UserEvent::Update(v, None)).is_err()
                    {
                        return;
                    }
                    std::thread::sleep(UPDATE_EVERY);
                }
            });
        }
        Ok(())
    }

    /// Starts a session for pane `id` and shows `win`, a layout that
    /// already holds it, running `cmd` or else the shell. Once a session
    /// exists, a layout that leaves the new pane, or the one it split,
    /// below the minimum size is refused; panes the window already made
    /// small do not count.
    fn open(
        &mut self,
        win: layout::Window,
        id: PaneId,
        cmd: Option<&str>,
        cwd: Option<PathBuf>,
    ) -> Result<(), String> {
        let grids = self.grids(&win);
        let split = self.focus_id();
        let small = |&(p, (c, r)): &(PaneId, (i32, i32))| {
            (p == id || Some(p) == split) && (c < layout::MIN_COLS || r < layout::MIN_ROWS)
        };
        if !self.views.is_empty() && grids.iter().any(small) {
            return Err("no room for another pane".into());
        }
        self.spawn(id, &grids, cmd, cwd, None)?;
        self.install(win);
        Ok(())
    }

    /// Starts every pane of a saved session, each in its folder, and shows
    /// its layout. A pane whose folder a process cannot start in (too long
    /// a path for one, say) starts where blitz runs instead. Starts none if
    /// one still fails.
    fn restore(&mut self, s: &session::State) -> Result<(), String> {
        let (win, panes) = s.layout(self.next_id);
        let grids = self.grids(&win);
        for (id, meta) in panes {
            // A session saved before panes had keys filed output by tab
            // and leaf; the pane's fresh key files it anew at the next exit.
            let old = (self.config.restore_scrollback)
                .then(|| {
                    if session::is_key(&meta.key) {
                        session::load_output(&meta.key)
                    } else {
                        (leaf_index(&win, id))
                            .and_then(|(tab, leaf)| session::load_legacy_output(tab, leaf))
                    }
                })
                .flatten();
            // When the retry fails too, the first failure says why.
            let started = (self.spawn(id, &grids, None, start_dir(&meta.cwd), old.as_deref()))
                .or_else(|first| {
                    (self.spawn(id, &grids, None, None, old.as_deref()))
                        .map_err(|then| joined(first, then))
                });
            if let Err(e) = started {
                self.views.clear();
                return Err(e);
            }
            if let Some(v) = self.views.last_mut()
                && session::is_key(&meta.key)
            {
                v.key = meta.key.clone();
            }
            // A result the user had not seen before blitz closed.
            if let Some(msg) = &meta.done
                && let Some(v) = self.views.last_mut()
            {
                v.pane.attn.apply(Ev::Done, false, Instant::now());
                v.pane.msg.clone_from(msg);
            }
            if let Some(v) = self.views.last_mut() {
                v.pane.named.clone_from(&meta.name);
            }
            self.resume(id, meta.claude.clone());
        }
        self.install(win);
        Ok(())
    }

    /// Has the shell of the new pane `id` resume Claude Code session
    /// `claude` once it is ready.
    fn resume(&mut self, id: PaneId, claude: Option<String>) {
        if let Some(line) = resume_line(self.config.restore_claude, claude.as_deref())
            && let Some(v) = self.view_mut(id)
        {
            // Known from the start, so closing blitz again before the
            // first prompt still resumes it next time.
            v.pane.claude = claude;
            v.resume = Some((line, Instant::now() + RESUME_AFTER));
        }
    }

    /// Starts the program of pane `id`, which exited, again in its place:
    /// in its folder, by the name the user gave it, resuming its Claude
    /// Code session. The new session
    /// gets a new id, so nothing still on its way from the old one lands
    /// in it.
    fn restart(&mut self, id: PaneId) {
        let Some(i) = self.views.iter().position(|v| v.pane.id == id) else {
            return;
        };
        let (old, new) = (&self.views[i], PaneId(self.next_id));
        let (cwd, cmd) = (start_dir(&old.pane.cwd), old.cmd.clone());
        let claude = old.pane.claude.clone().filter(|_| cmd.is_none());
        let mut win = self.win.clone();
        win.replace_pane(id, new);
        let grids = self.grids(&win);
        if let Err(e) = self.spawn(new, &grids, cmd.as_deref(), cwd, None) {
            self.error(id, e);
            return;
        }
        // The new session takes the old one's row in the sidebar.
        let named = self.views.swap_remove(i).pane.named;
        if let Some(v) = self.view_mut(new) {
            v.pane.named = named;
        }
        self.resume(new, claude);
        self.install(win);
    }

    /// Starts a session for pane `id`, sized as `grids` lays it out (or
    /// 80x24 while hidden), running `cmd` or else the shell, below `old`,
    /// output saved by [`session::save_output`].
    fn spawn(
        &mut self,
        id: PaneId,
        grids: &[(PaneId, (i32, i32))],
        cmd: Option<&str>,
        cwd: Option<PathBuf>,
        old: Option<&str>,
    ) -> Result<(), String> {
        let fit = |n: i32| n.clamp(1, i32::from(u16::MAX)) as u16;
        let grid = grids
            .iter()
            .find(|g| g.0 == id)
            .map_or((80, 24), |&(_, (c, r))| (fit(c), fit(r)));
        // The first line of saved output says when it was saved.
        let restored = old.map_or_else(Vec::new, |o| {
            let (stamp, text) = o.split_once('\n').unwrap_or(("", o));
            crate::pane::restored(text, stamp, grid.1)
        });
        let token = crate::pty::pane_token().map_err(|e| format!("cannot start a session: {e}"))?;
        // Not the token, which is a secret between the pane and its child.
        let key = crate::pty::pane_token().map_err(|e| format!("cannot start a session: {e}"))?;
        let mut launch = match cmd {
            Some(c) => crate::shell::Launch {
                cmdline: c.to_string(),
                env: Vec::new(),
            },
            None => crate::shell::launch(&self.config.shell, self.config.shell_integration, &token),
        };
        // Claude Code loads blitz's hooks from there, with nothing pasted
        // into its settings.
        if let Some(dir) = &self.plugin {
            let inherited = std::env::var("CLAUDE_CODE_PLUGIN_DIRS").ok();
            let dirs = crate::hook::plugin_dirs(inherited.as_deref(), dir);
            launch.env.push(("CLAUDE_CODE_PLUGIN_DIRS".into(), dirs));
        }
        let proxy = self.proxy.clone();
        let mut pane = Pane::spawn(
            id,
            &Spawn {
                cmdline: &launch.cmdline,
                env: &launch.env,
                cwd: cwd.as_deref(),
                cols: grid.0,
                rows: grid.1,
                scrollback: self.config.scrollback_lines,
                dark: !self.theme.light,
                pal: self.theme.pal,
                parent: Some(self.hwnd),
                token: &token,
                restored: &restored,
            },
            move |id, note| {
                let _ = proxy.send_event(UserEvent::Pane(id, note));
            },
        )
        .map_err(|e| format!("cannot start {}: {e}", launch.cmdline))?;
        let (cw, ch) = self.cell();
        lock(&pane.term).set_cell_px(cw as u16, ch as u16);
        pane.name = program_name(&launch.cmdline);
        self.views.push(View {
            pane,
            snap: Snapshot::default(),
            grid,
            rect: None,
            notice: None,
            flashed: None,
            finding_branch: false,
            resume: None,
            prompted: false,
            sync_until: None,
            key,
            cmd: cmd.map(str::to_owned),
            progress: None,
            claude_working: None,
            hooks_seen: false,
        });
        self.find_branch(id);
        self.next_id = self.next_id.max(id.0 + 1);
        Ok(())
    }

    /// Shows `win`, a layout whose panes all have sessions.
    fn install(&mut self, win: layout::Window) {
        let before = self.focus_id();
        self.win = win;
        // A divider being dragged is known by its place in the old layout.
        self.mouse.divider = None;
        self.focus_moved(before);
    }

    /// Opens a pane in a copy of the layout that `place` changes; tells the
    /// user in the focused pane when that fails. The pane starts in `dir`,
    /// or else where the focused one is.
    fn add(
        &mut self,
        dir: Option<PathBuf>,
        place: impl FnOnce(&mut layout::Window, PaneId) -> bool,
    ) {
        let cwd = dir.or_else(|| start_dir(self.current().map_or("", |v| v.pane.cwd.as_str())));
        let id = PaneId(self.next_id);
        let mut win = self.win.clone();
        if !place(&mut win, id) {
            return;
        }
        if let Err(e) = self.open(win, id, None, cwd)
            && let Some(id) = self.focus_id()
        {
            self.error(id, e);
        }
    }

    /// Closes a session and its pane. The window closes with the last one.
    fn close(&mut self, el: &ActiveEventLoop, id: PaneId) {
        let before = self.focus_id();
        if let Some(v) = self.view(id) {
            self.closed = Some((v.pane.cwd.clone(), v.pane.claude.clone()));
        }
        self.win.close_pane(id);
        // The divider being dragged may be gone, even when focus stays.
        self.mouse.divider = None;
        // Dropping the pane closes its pseudoconsole.
        self.views.retain(|v| v.pane.id != id);
        self.taskbar_progress();
        if self.views.is_empty() {
            // Nothing is left open, so there is nothing to restore.
            if self.persist {
                session::clear();
                self.persist = false;
            }
            el.exit();
            return;
        }
        self.focus_moved(before);
    }

    /// Catches up after the focused pane may have changed: focus reports,
    /// attention, the selection, the find bar and the window title.
    fn focus_moved(&mut self, before: Option<PaneId>) {
        self.request_redraw();
        let now = self.focus_id();
        if now == before {
            return;
        }
        self.selection = None;
        self.find = None;
        if let Some(v) = before.and_then(|id| self.view_mut(id)) {
            v.notice.take_if(|n| n.ask.of_pane());
        }
        self.mouse.drag = None;
        // A drag belongs to the tab it started in.
        self.mouse.divider = None;
        self.set_hover(None);
        self.ime_at = None;
        if self.focused {
            for (id, f) in [(before, false), (now, true)] {
                if let Some(v) = id.and_then(|id| self.view(id)) {
                    tell_focus(v, f);
                }
            }
            if let Some(id) = now {
                self.attention(id, Ev::Attended);
            }
        }
        let title = self.current().map(|v| v.pane.title.clone());
        self.set_title(&title.unwrap_or_default());
        if let (Some(w), Some(v)) = (&self.watched, self.current()) {
            *lock(w) = v.pane.term.clone();
        }
    }

    fn set_title(&self, t: &str) {
        if let Some(w) = &self.window {
            w.set_title(if t.is_empty() { "blitz" } else { t });
        }
    }

    /// The pane with keyboard focus: the focused pane of the active tab.
    fn focus_id(&self) -> Option<PaneId> {
        self.win.tabs.get(self.win.active).map(|t| t.focus)
    }

    fn view(&self, id: PaneId) -> Option<&View> {
        self.views.iter().find(|v| v.pane.id == id)
    }

    fn view_mut(&mut self, id: PaneId) -> Option<&mut View> {
        self.views.iter_mut().find(|v| v.pane.id == id)
    }

    fn current(&self) -> Option<&View> {
        self.focus_id().and_then(|id| self.view(id))
    }

    /// Runs the `--selftest` script on its own thread; the app exits with
    /// its result.
    fn start_selftest(&mut self, path: PathBuf) {
        let (Some(v), proxy, hwnd) = (self.current(), self.proxy.clone(), self.hwnd) else {
            return;
        };
        let term = Arc::new(Mutex::new(v.pane.term.clone()));
        self.watched = Some(term.clone());
        std::thread::spawn(move || {
            let code = match std::fs::read_to_string(&path) {
                Ok(script) => match selftest::run(&script, hwnd, &term) {
                    Ok(()) => 0,
                    Err(e) => {
                        let screen = selftest::screen(&term);
                        println!("selftest FAILED: {e}\n--- screen ---\n{screen}");
                        1
                    }
                },
                Err(e) => {
                    println!("selftest: {}: {e}", path.display());
                    2
                }
            };
            let _ = proxy.send_event(UserEvent::Finish(code));
        });
    }

    fn cell(&self) -> (u32, u32) {
        self.gfx.as_ref().map_or((8, 16), |g| g.r.cell())
    }

    /// What the chrome needs to lay out `win` in the window as it is now.
    fn model<'a>(
        &'a self,
        win: &'a layout::Window,
        sessions: &'a [chrome::Session],
        preedit: Option<(u16, u16, &'a str)>,
    ) -> ChromeModel<'a> {
        let size = self
            .window
            .as_ref()
            .map_or(PhysicalSize::new(0, 0), |w| w.inner_size());
        ChromeModel {
            win,
            sessions,
            ui: self.theme.ui,
            size: (size.width as i32, size.height as i32),
            scale: self.scale as f32,
            text_cell: self.gfx.as_ref().map_or((6, 12), |g| g.r.small_cell()),
            term_cell: self.cell(),
            now: Instant::now(),
            banner: self.update.as_ref().map(|u| u.1.as_str()),
            preedit,
            picker: self.picker.as_ref().map(|p| chrome::Picker {
                filter: &p.filter,
                items: p.matches(),
                sel: p.sel,
            }),
            settings: self.settings.as_ref().map(|p| chrome::Settings {
                filter: &p.filter,
                rows: p.rows(&self.config),
                sel: p.sel,
                top: p.top,
                error: p.error.as_deref(),
            }),
            commands: self.commands.as_ref().map(|cm| chrome::Commands {
                filter: &cm.filter,
                items: (cm.matches().into_iter())
                    .map(|(a, label)| {
                        let keys = keymap::keys_for(a, &self.config.keys);
                        (label, keys.unwrap_or_default())
                    })
                    .collect(),
                sel: cm.sel,
                rename: cm.rename.map(|r| match r {
                    Rename::Session(_) => "Rename session",
                    Rename::Tab(_) => "Rename tab",
                }),
            }),
            find: self.find.as_ref().map(|f| chrome::FindBar {
                query: &f.query,
                count: f.cur.map(|i| (i + 1, f.found.len())),
            }),
            spark: self.config.mascot.then(|| self.anim_time()),
            game: self.game.as_ref().map(|g| &g.0),
        }
    }

    /// Seconds into the scenery and spark animations; always 0 when
    /// Windows animations are off.
    fn anim_time(&self) -> f64 {
        if self.motion {
            self.started.elapsed().as_secs_f64()
        } else {
            0.0
        }
    }

    /// Opens blitz run in place of the settings panel.
    fn open_game(&mut self) {
        self.settings = None;
        let seed = (self.started.elapsed().as_nanos() as u64) | 1;
        self.game = Some((Run::new(seed, run::load_best()), Instant::now()));
        self.request_redraw();
    }

    /// Closes blitz run, keeping a new best score.
    fn close_game(&mut self) {
        if let Some((g, _)) = self.game.take() {
            if g.best > run::load_best() {
                run::save_best(g.best);
            }
            // The focused pane is in view again, so whatever landed on it
            // under the game is seen now.
            if self.focused
                && let Some(id) = self.focus_id()
            {
                self.attention(id, Ev::Attended);
            }
            self.request_redraw();
        }
    }

    /// Window frame in the theme's colours: dark or light everywhere, and
    /// on Windows 11 the title bar, title text and border too (older
    /// Windows refuses those and keeps the dark or light frame).
    fn frame_theme(&self) {
        let hwnd = HWND(self.hwnd as *mut c_void);
        let dark = windows::core::BOOL::from(!self.theme.light);
        // SAFETY: a live window and a BOOL-sized value.
        let _ = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                (&raw const dark).cast(),
                size_of_val(&dark) as u32,
            )
        };
        let ui = &self.theme.ui;
        for (attr, rgb) in [
            (DWMWA_CAPTION_COLOR, ui.term_bg),
            (DWMWA_TEXT_COLOR, ui.term_fg),
            (DWMWA_BORDER_COLOR, ui.border),
        ] {
            // COLORREF is 0x00BBGGRR.
            let c = rgb.swap_bytes() >> 8;
            // SAFETY: a live window and a COLORREF-sized value.
            let _ = unsafe {
                DwmSetWindowAttribute(hwnd, attr, (&raw const c).cast(), size_of_val(&c) as u32)
            };
        }
    }

    /// Shows `t` everywhere: panes, chrome, window frame, and what colour
    /// queries report.
    fn apply_theme(&mut self, t: Theme) {
        if t == self.theme {
            return;
        }
        self.theme = t;
        self.frame_theme();
        for v in &self.views {
            let mut term = lock(&v.pane.term);
            term.set_theme(!self.theme.light, &self.theme.pal);
            // A mode 2031 report; sent now, as an idle program writes
            // nothing that would carry it out with the replies.
            let mut report = Vec::new();
            term.take_replies(&mut report);
            drop(term);
            if !report.is_empty() {
                v.pane.send(report);
            }
        }
        self.request_redraw();
    }

    /// The theme the settings pick, as opposed to one the picker shows.
    fn set_theme_from_config(&mut self) {
        self.apply_theme(crate::theme::current(&self.config.theme));
    }

    /// Opens the theme picker on the theme in use.
    fn open_picker(&mut self) {
        let themes = crate::theme::all();
        let sel = (themes.iter())
            .position(|t| t.name == self.theme.name)
            .unwrap_or(0);
        self.picker = Some(Picker {
            themes,
            filter: String::new(),
            sel,
        });
        self.request_redraw();
    }

    /// A key while the picker is open: arrows show another theme, typing
    /// narrows the list, Enter keeps the theme and Esc goes back.
    fn picker_key(&mut self, k: &KeyInput) {
        let Some(pk) = &mut self.picker else {
            return;
        };
        match k.vk {
            VK_ESCAPE => {
                self.picker = None;
                self.set_theme_from_config();
            }
            VK_RETURN => {
                let picked = pk.matches().get(pk.sel).map(|t| (*t).clone());
                self.picker = None;
                match picked {
                    Some(t) => {
                        let light = crate::theme::system_is_light();
                        let setting = crate::theme::pick(&self.config.theme, &t.name, light);
                        let value = crate::config::quote(&setting);
                        if let Err(e) = crate::config::save("theme", Some(&value)) {
                            let text = format!("Cannot save the theme to config.toml: {e}");
                            let focus = self.focus_id();
                            match (&mut self.settings, focus) {
                                (Some(p), _) => p.error = Some(text),
                                (None, Some(id)) => self.error(id, text),
                                (None, None) => {}
                            }
                        }
                        self.config.theme = setting;
                        self.apply_theme(t);
                    }
                    None => self.set_theme_from_config(),
                }
            }
            _ if pk.key(k) => self.preview(),
            _ => {}
        }
    }

    /// Shows the theme highlighted in the picker.
    fn preview(&mut self) {
        let shown =
            (self.picker.as_ref()).and_then(|p| p.matches().get(p.sel).map(|t| (*t).clone()));
        match shown {
            Some(t) => self.apply_theme(t),
            None => self.request_redraw(),
        }
    }

    /// Opens the settings panel on its first setting.
    fn open_settings(&mut self) {
        let fonts = crate::render::font::monospace_families();
        self.settings = Some(Panel::new(fonts, crate::shell::choices()));
        self.request_redraw();
    }

    /// A key while the settings panel is open: up and down choose a
    /// setting, left and right change it, Enter flips a switch or goes to
    /// the next value, Delete puts the default back, typing narrows the
    /// list, and Esc clears what was typed, then closes.
    fn settings_key(&mut self, k: &KeyInput) {
        let Some(p) = &mut self.settings else {
            return;
        };
        let m = &k.mods;
        // Ctrl and Alt together are AltGr when the layout gives a character.
        let (ctrl, alt) = (m.lctrl || m.rctrl, m.lalt || m.ralt);
        let chord = ctrl != alt || (ctrl && k.uc == 0);
        match k.vk {
            VK_ESCAPE if !p.filter.is_empty() => {
                p.filter.clear();
                p.sel = 0;
            }
            VK_ESCAPE => self.settings = None,
            VK_UP => p.move_by(-1),
            VK_DOWN => p.move_by(1),
            VK_LEFT => self.change_setting(-1, false),
            VK_RIGHT => self.change_setting(1, false),
            VK_RETURN => self.change_setting(1, true),
            VK_DELETE => {
                if let Some(s) = p.selected() {
                    self.set_setting(s.key, None);
                }
            }
            VK_BACK if p.filter.pop().is_some() => p.sel = 0,
            VK_BACK => return,
            _ if !chord && !k.text.is_empty() => {
                p.filter.push_str(k.text);
                p.sel = 0;
            }
            _ => return,
        }
        self.request_redraw();
    }

    /// A left click while the settings panel is open: a switch flips, the
    /// left or right half of a value steps it, a click elsewhere on a row
    /// highlights it, and one outside the panel closes it.
    fn settings_click(&mut self) {
        let (x, y) = (self.mouse.pos.x as i32, self.mouse.pos.y as i32);
        let inside = |r: &Rect| (r.x..r.right()).contains(&x) && (r.y..r.bottom()).contains(&y);
        let Some(h) = &self.settings_hits else {
            return;
        };
        if !inside(&h.panel) {
            self.settings = None;
            self.request_redraw();
            return;
        }
        let Some(&(i, _, control)) = h.rows.iter().find(|r| inside(&r.1)) else {
            return;
        };
        let Some(p) = &mut self.settings else {
            return;
        };
        p.sel = i;
        match p.selected().map(|s| s.kind) {
            Some(Kind::Toggle) => self.change_setting(1, true),
            Some(Kind::Theme) => self.open_picker(),
            Some(Kind::Game) => self.open_game(),
            Some(Kind::Choice) if inside(&control) => {
                let by = if x < control.x + control.w / 2 { -1 } else { 1 };
                self.change_setting(by, false);
            }
            _ => {}
        }
        self.request_redraw();
    }

    /// Steps the highlighted setting as [`Panel::step`] does; the theme
    /// opens the theme picker instead.
    fn change_setting(&mut self, by: isize, wrap: bool) {
        let Some(p) = &self.settings else {
            return;
        };
        let Some(s) = p.selected() else {
            return;
        };
        match s.kind {
            Kind::Theme => return self.open_picker(),
            Kind::Game => return self.open_game(),
            _ => {}
        }
        if let Some(v) = p.step(s, &self.config, by, wrap) {
            self.set_setting(s.key, Some(v));
        }
    }

    /// Sets `key` to `value` as `config.toml` holds it, or with none to its
    /// default, saves it, and shows the change.
    fn set_setting(&mut self, key: &str, value: Option<String>) {
        let mut c = self.config.clone();
        c.set(
            key,
            &value.clone().unwrap_or_else(|| Config::default().get(key)),
        );
        let saved = crate::config::save(key, value.as_deref());
        if let Some(p) = &mut self.settings {
            p.error = saved
                .err()
                .map(|e| format!("Cannot save to config.toml: {e}"));
        }
        self.apply_config(c);
    }

    /// Takes new settings and shows the font and theme they pick. The
    /// rest are read where they are used.
    fn apply_config(&mut self, c: Config) {
        let font =
            (&c.font_family, c.font_size) != (&self.config.font_family, self.config.font_size);
        if c.font_size != self.config.font_size {
            self.font_zoom = 0.0;
        }
        self.config = c;
        if font {
            self.reload_font();
        }
        // The picker puts the configured theme back when it closes.
        if self.picker.is_none() {
            self.set_theme_from_config();
        }
        self.request_redraw();
    }

    /// Loads the configured font at the size the window's DPI and the font
    /// zoom need, and tells each terminal its new cell size.
    fn reload_font(&mut self) {
        let px = self.font_px();
        if let Some(g) = &mut self.gfx
            && let Err(e) = g.r.set_font(&self.config.font_family, px)
        {
            eprintln!("blitz: font: {e}");
        }
        let (cw, ch) = self.cell();
        for v in &self.views {
            lock(&v.pane.term).set_cell_px(cw as u16, ch as u16);
        }
        self.request_redraw();
    }

    /// A key while the command palette is open: up and down choose an
    /// action, typing narrows the list, Enter runs the action and Esc
    /// closes the palette.
    fn commands_key(&mut self, el: &ActiveEventLoop, k: &KeyInput) {
        let Some(cm) = &mut self.commands else {
            return;
        };
        let m = &k.mods;
        // Ctrl and Alt together are AltGr when the layout gives a character.
        let (ctrl, alt) = (m.lctrl || m.rctrl, m.lalt || m.ralt);
        let chord = ctrl != alt || (ctrl && k.uc == 0);
        let page = chrome::PICKER_ROWS as isize;
        match k.vk {
            VK_ESCAPE => self.commands = None,
            VK_RETURN => {
                let picked = cm.matches().get(cm.sel).map(|a| a.0);
                let rename = cm.rename.map(|r| (r, crate::hook::one_line(&cm.filter)));
                self.commands = None;
                if let Some((r, name)) = rename {
                    self.rename(r, name);
                } else if let Some(a) = picked {
                    self.act(el, a);
                }
            }
            VK_UP => cm.move_by(-1),
            VK_DOWN => cm.move_by(1),
            VK_PRIOR => cm.move_by(-page),
            VK_NEXT => cm.move_by(page),
            VK_BACK if cm.filter.pop().is_some() => cm.sel = 0,
            VK_BACK => return,
            _ if !chord && !k.text.is_empty() => {
                cm.filter.push_str(k.text);
                cm.sel = 0;
            }
            _ => return,
        }
        self.request_redraw();
    }

    /// Gives a session or a tab the name `name`, or with an empty one lets
    /// blitz pick it again.
    fn rename(&mut self, r: Rename, name: String) {
        match r {
            Rename::Session(id) => {
                if let Some(v) = self.view_mut(id) {
                    v.pane.named = (!name.is_empty()).then_some(name);
                }
            }
            Rename::Tab(id) => {
                if let Some(t) = self.win.tabs.iter_mut().find(|t| t.root.contains(id)) {
                    t.name = name;
                }
            }
        }
        self.request_redraw();
    }

    /// A left click while the command palette is open: one on an action
    /// runs it, and one outside the palette closes it.
    fn commands_click(&mut self, el: &ActiveEventLoop) {
        let (x, y) = (self.mouse.pos.x as i32, self.mouse.pos.y as i32);
        let inside = |r: &Rect| (r.x..r.right()).contains(&x) && (r.y..r.bottom()).contains(&y);
        let Some((panel, rows)) = &self.commands_hits else {
            return;
        };
        if !inside(panel) {
            self.commands = None;
            self.request_redraw();
            return;
        }
        let Some(&(i, _)) = rows.iter().find(|r| inside(&r.1)) else {
            return;
        };
        let picked = (self.commands.as_ref()).and_then(|cm| cm.matches().get(i).map(|a| a.0));
        self.commands = None;
        self.request_redraw();
        if let Some(a) = picked {
            self.act(el, a);
        }
    }

    /// A key while the find bar is open: typing searches as it goes, Enter
    /// or F3 goes to the next match up, with Shift the next one down, and
    /// Esc closes the bar, leaving the view where it is.
    fn find_key(&mut self, k: &KeyInput) {
        let Some(f) = &mut self.find else {
            return;
        };
        let m = &k.mods;
        // Ctrl and Alt together are AltGr when the layout gives a character.
        let (ctrl, alt) = (m.lctrl || m.rctrl, m.lalt || m.ralt);
        let chord = ctrl != alt || (ctrl && k.uc == 0);
        // Matches are oldest first, so up is back through the list.
        let by = if m.lshift || m.rshift { 1 } else { -1 };
        match k.vk {
            VK_ESCAPE => {
                self.find = None;
                self.request_redraw();
            }
            VK_RETURN | VK_F3 => self.find_go(false, by),
            VK_BACK if f.query.pop().is_some() => self.find_go(true, 0),
            VK_BACK => {}
            _ if !chord && !k.text.is_empty() => {
                f.query.push_str(k.text);
                self.find_go(true, 0);
            }
            _ => {}
        }
    }

    /// Searches the find bar's pane again when `search` is set or output
    /// came since the last search, moves `by` matches, and scrolls the
    /// current one into view.
    fn find_go(&mut self, search: bool, by: isize) {
        let Some(f) = &mut self.find else {
            return;
        };
        let Some(v) = self.views.iter().find(|v| v.pane.id == f.pane) else {
            return;
        };
        let mut term = lock(&v.pane.term);
        if search || f.stale {
            f.search(&term, v.grid.1);
        }
        f.step(by);
        let shown = f.cur.map(|i| f.found[i]);
        if let Some(m) = shown {
            reveal(&mut term, m, v.grid.1);
        }
        drop(term);
        self.request_redraw();
    }

    /// Typed text for the filter of the command palette, the theme picker
    /// or the settings panel, or for the find bar. False when none is open.
    fn filter_text(&mut self, t: &str) -> bool {
        if self.game.is_some() {
            // The game takes keys, not text.
        } else if let Some(cm) = &mut self.commands {
            cm.filter.push_str(t);
            cm.sel = 0;
            self.request_redraw();
        } else if let Some(p) = &mut self.picker {
            p.filter.push_str(t);
            p.sel = 0;
            self.preview();
        } else if let Some(p) = &mut self.settings {
            p.filter.push_str(t);
            p.sel = 0;
            self.request_redraw();
        } else if let Some(f) = &mut self.find {
            f.query.push_str(t);
            self.find_go(true, 0);
        } else {
            return false;
        }
        true
    }

    /// The size in cells of each pane `win` would show now.
    fn grids(&self, win: &layout::Window) -> Vec<(PaneId, (i32, i32))> {
        let (cw, ch) = self.cell();
        let c = chrome::build(&self.model(win, &[], None));
        c.panes
            .iter()
            .map(|&(id, r)| (id, (r.w / cw as i32, r.h / ch as i32)))
            .collect()
    }

    /// Every session as the sidebar shows it.
    fn sessions(&self) -> Vec<chrome::Session> {
        let mut list: Vec<chrome::Session> = (self.views.iter())
            .map(|v| {
                let p = &v.pane;
                let claude = titled_by_claude(p.claude_title, p.hooked, p.claude.as_deref());
                let (name, msg) = label(p.named.as_deref(), &p.name, &p.title, claude, &p.msg);
                chrome::Session {
                    id: p.id,
                    name,
                    num: None,
                    cwd: p.cwd.clone(),
                    branch: p.branch.clone(),
                    state: p.attn.state,
                    since: p.attn.since,
                    turn: p.attn.turn,
                    took: p.attn.took,
                    seen: p.attn.seen,
                    msg,
                    progress: v.progress.map(|p| p.0),
                    exit_code: p.exit_code,
                }
            })
            .collect();
        chrome::number_twins(&mut list);
        list
    }

    /// Builds the renderer and swap chain if there are none. Never panics:
    /// without them the window stays as it is until the next try, which
    /// comes `GFX_RETRY` after a failed one.
    fn ensure_gfx(&mut self) {
        if self.gfx.is_some() || self.gfx_retry.is_some_and(|t| Instant::now() < t) {
            return;
        }
        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        let early = self.gpu.take().and_then(|h| h.join().ok()?.ok());
        let (family, px) = (&self.config.font_family, self.font_px());
        let built = match early {
            Some(gpu) => Ok(gpu),
            None => Gpu::new(false),
        }
        .and_then(|gpu| Renderer::with_gpu(gpu, family, px));
        let built = built.and_then(|r| {
            let hwnd = HWND(self.hwnd as *mut c_void);
            let chain = Swapchain::new(&r.gpu, hwnd, size.width, size.height)?;
            Ok(Gfx { r, chain })
        });
        match built {
            Ok(g) => {
                self.gfx = Some(g);
                self.gfx_retry = None;
            }
            Err(e) => {
                eprintln!("blitz: renderer: {e}");
                self.gfx_retry = Some(Instant::now() + GFX_RETRY);
            }
        }
    }

    fn request_redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn modes(&self) -> InputModes {
        self.current()
            .map(|v| lock(&v.pane.term).input_modes())
            .unwrap_or_default()
    }

    fn send(&self, bytes: impl Into<Vec<u8>>) {
        if let Some(v) = self.current() {
            v.pane.send(bytes);
        }
    }

    /// Scrolls the main screen; positive is up into the scrollback.
    fn scroll(&mut self, lines: isize) {
        if let Some(v) = self.current() {
            lock(&v.pane.term).scroll_viewport(lines);
            // The pointer is over other text now.
            self.extend_drag();
            self.request_redraw();
        }
    }

    /// Shows the banner for release `v`, or for the update to it that
    /// failed and wrote `log`.
    fn offer_update(&mut self, v: String, log: Option<PathBuf>) {
        let installed = crate::update::installed();
        let shown = self.update.as_ref();
        if let Some(text) = crate::update::banner(shown, &v, log.as_deref(), installed) {
            self.update = Some((v, text));
            self.request_redraw();
        }
    }

    /// Says dimly in pane `id`, for a while, that Claude Code's hooks are
    /// not reporting, or are `older` than this blitz, unless a hint about
    /// them was shown already.
    fn hooks_hint(&mut self, id: PaneId, older: bool) {
        if std::mem::replace(&mut self.hooks_hinted, true) {
            return;
        }
        let palette = keymap::keys_for(Action::Palette, &self.config.keys);
        let text = hooks_hint_text(older, palette);
        self.set_notice(id, text, Some(Instant::now() + HINT), true);
    }

    fn set_notice(
        &mut self,
        id: PaneId,
        text: impl Into<String>,
        until: Option<Instant>,
        dim: bool,
    ) {
        if let Some(v) = self.view_mut(id)
            && (!dim || hint_fits(v.notice.as_ref()))
        {
            v.notice = Some(Notice {
                text: text.into(),
                until,
                dim,
                ask: Ask::Nothing,
            });
        }
        self.request_redraw();
    }

    /// Shows `text` in pane `id` until `ask` is answered or a key takes it
    /// away.
    fn ask(&mut self, id: PaneId, text: impl Into<String>, ask: Ask) {
        if let Some(v) = self.view_mut(id) {
            v.notice = Some(Notice {
                text: text.into(),
                until: None,
                dim: false,
                ask,
            });
        }
        self.request_redraw();
    }

    /// Says what went wrong in pane `id`, until the next key there.
    fn error(&mut self, id: PaneId, text: impl Into<String>) {
        self.ask(id, text, Ask::Key);
    }

    /// Whether pane `id` asks `ask`, which a second run of its action
    /// answers. Takes the question away if so.
    fn confirmed(&mut self, id: PaneId, ask: &Ask) -> bool {
        let yes = (self.view_mut(id))
            .and_then(|v| v.notice.take_if(|n| n.ask == *ask))
            .is_some();
        if yes {
            self.request_redraw();
        }
        yes
    }

    /// A key that runs `a`, if anything, takes away the questions it does
    /// not answer, and the focused pane's error.
    fn dismiss(&mut self, a: Option<Action>) {
        let focus = self.focus_id();
        let mut gone = false;
        for v in &mut self.views {
            let here = Some(v.pane.id) == focus;
            gone |= v.notice.take_if(|n| n.ask.gone(a, here)).is_some();
        }
        if gone {
            self.request_redraw();
        }
    }

    /// Handles queued key input.
    fn drain_keys(&mut self, el: &ActiveEventLoop) {
        let inputs = std::mem::take(&mut self.keys.borrow_mut().queue);
        for input in inputs {
            match input {
                Input::Text(t) => {
                    if !self.filter_text(&t) {
                        // As any other key, it takes questions away.
                        self.dismiss(None);
                        self.typed(t.into_bytes());
                    }
                }
                Input::Key(k, text, held) => {
                    let k = KeyInput { text: &text, ..k };
                    self.key(el, &k, held);
                }
            }
        }
    }

    /// A key transition; `held` when it is the auto-repeat of a held key.
    fn key(&mut self, el: &ActiveEventLoop, k: &KeyInput, held: bool) {
        if matches!(k.key, vt::Key::Control | vt::Key::Shift) {
            self.update_hover();
        }
        if !k.down && self.eaten.release(k.vk) {
            return;
        }
        // A repeat of a key blitz took goes where its press went, and only
        // some keys do anything again; see `keymap::drops_repeat`. blitz
        // run takes keys as a panel does, so a held jump keeps jumping.
        let panel = self.commands.is_some()
            || self.picker.is_some()
            || self.settings.is_some()
            || self.find.is_some()
            || self.game.is_some();
        let taken = self.eaten.0.contains(&k.vk);
        if held && k.down && keymap::drops_repeat(k, &self.config.keys, taken, panel) {
            return;
        }
        // Every key pressed while blitz run is open is the game's, and so is
        // its release; one pressed before it opened is released to the
        // program, as with the picker.
        if let Some((g, _)) = self.game.as_mut().filter(|_| k.down) {
            // Every key is the game's, but a shortcut closes it and runs.
            if keymap::action(k, &self.config.keys).is_some() {
                self.close_game();
            } else {
                self.eaten.press(k.vk);
                match k.vk {
                    VK_ESCAPE => self.close_game(),
                    vk if is_jump(vk) => g.jump(),
                    _ => {}
                }
                self.request_redraw();
                return;
            }
        }
        // The same for the command palette.
        if self.commands.is_some() && k.down {
            self.eaten.press(k.vk);
            if keymap::action(k, &self.config.keys) == Some(Action::Palette) {
                self.commands = None;
                self.request_redraw();
            } else {
                self.commands_key(el, k);
            }
            return;
        }
        // Every key pressed while the picker is open is the picker's, and
        // so is its release. A key pressed before it opened, such as the
        // Ctrl of the shortcut that opened it, is released to the program,
        // which saw the press.
        if self.picker.is_some() && k.down {
            self.eaten.press(k.vk);
            if keymap::action(k, &self.config.keys) == Some(Action::ThemePicker) {
                self.picker = None;
                self.set_theme_from_config();
            } else {
                self.picker_key(k);
            }
            return;
        }
        // The same for the panel, but the theme picker opens over it.
        if self.settings.is_some() && k.down {
            self.eaten.press(k.vk);
            match keymap::action(k, &self.config.keys) {
                Some(Action::Settings) => {
                    self.settings = None;
                    self.request_redraw();
                }
                Some(Action::ThemePicker) => self.open_picker(),
                _ => self.settings_key(k),
            }
            return;
        }
        // And for the find bar.
        if self.find.is_some() && k.down {
            self.eaten.press(k.vk);
            if keymap::action(k, &self.config.keys) == Some(Action::Find) {
                self.find = None;
                self.request_redraw();
            } else {
                self.find_key(k);
            }
            return;
        }
        let modifier = matches!(
            k.key,
            vt::Key::Shift | vt::Key::Control | vt::Key::Alt | vt::Key::Super
        );
        let a = keymap::action(k, &self.config.keys);
        if k.down && !modifier && !lock_key(k.vk) {
            self.dismiss(a);
        }
        if let Some(a) = a
            && self.act(el, a)
        {
            self.eaten.press(k.vk);
            return;
        }
        // The program saw neither the press nor the release of a key blitz
        // took, so it gets none of its repeats either, even once they stop
        // doing anything: a swap that reached the edge, or a jump held when
        // a needs-you closed blitz run.
        if held && k.down && self.eaten.0.contains(&k.vk) {
            return;
        }
        // Nor do jumps already on their way when it closed.
        let since = self.game_ended.map(|t| t.elapsed());
        if k.down && late_jump(k.vk, since) {
            self.eaten.press(k.vk);
            return;
        }
        let Some(v) = self.current() else {
            return;
        };
        if v.pane.exit_code.is_some() {
            if k.down && matches!(k.vk, VK_RETURN | VK_ESCAPE) {
                let id = v.pane.id;
                // The release must not reach the pane that takes focus.
                self.eaten.press(k.vk);
                match k.vk {
                    VK_RETURN => self.restart(id),
                    _ => self.close(el, id),
                }
            }
            return;
        }
        let mut out = Vec::new();
        vt::encode_key(k, &self.modes(), &mut out);
        if k.down && !modifier && !out.is_empty() {
            self.typed(out);
        } else {
            self.send(out);
        }
    }

    /// Sends input the user typed: the view follows the cursor again, and
    /// it answers what the session asked.
    fn typed(&mut self, bytes: Vec<u8>) {
        if self.selection.take().is_some() {
            self.request_redraw();
        }
        if let Some(v) = self.current() {
            lock(&v.pane.term).scroll_viewport(isize::MIN);
            v.pane.send(bytes);
            let id = v.pane.id;
            self.answered(id);
        }
    }

    /// The user typed, pasted or clicked into session `id`. A question it
    /// showed is answered, so its text goes too.
    fn answered(&mut self, id: PaneId) {
        let asked = self
            .view(id)
            .is_some_and(|v| v.pane.attn.state == Attn::NeedsYou);
        if self.attention(id, Ev::Answered)
            && asked
            && let Some(v) = self.view_mut(id)
        {
            v.pane.msg.clear();
        }
    }

    /// Runs a shortcut. Returns false when it does not apply right now, in
    /// which case the key goes to the program.
    fn act(&mut self, el: &ActiveEventLoop, a: Action) -> bool {
        let before = self.focus_id();
        match a {
            Action::Copy => {
                let Some(v) = self.current() else {
                    return false;
                };
                let term = lock(&v.pane.term);
                let Some(sel) = self.selection.as_ref().filter(|s| s.kept(&term)) else {
                    return false;
                };
                let text = selection_text(&term, &self.theme.pal, sel, 0);
                drop(term);
                if !crate::clipboard::set_text(Some(HWND(self.hwnd as *mut c_void)), &text) {
                    eprintln!("blitz: could not copy to the clipboard");
                }
                self.selection = None;
                self.request_redraw();
            }
            Action::Paste => {
                let Some(text) = crate::clipboard::get_text().filter(|t| !t.is_empty()) else {
                    return false;
                };
                let bracketed = self.modes().bracketed;
                let trusted = self
                    .current()
                    .is_some_and(|v| lock(&v.pane.term).paste_trusted());
                if vt::keys::needs_paste_confirm(&text, trusted) {
                    let Some(id) = before else {
                        return false;
                    };
                    if !self.confirmed(id, &Ask::Paste(text.clone())) {
                        let lines = text.lines().count();
                        let asked = format!("Paste {lines} lines? Press Ctrl+V again");
                        self.ask(id, asked, Ask::Paste(text));
                        return true;
                    }
                    if let Some(v) = self.view(id) {
                        lock(&v.pane.term).confirm_paste();
                    }
                }
                let mut out = Vec::new();
                vt::encode_paste(&text, bracketed, &mut out);
                self.typed(out);
            }
            Action::ScrollPage(dir) => {
                if self.modes().alt_screen {
                    return false;
                }
                let rows = self.current().map_or(1, |v| v.grid.1);
                let page = rows.saturating_sub(1).max(1) as isize;
                self.scroll(page * isize::from(dir));
            }
            Action::NewTab => self.add(None, new_tab),
            Action::ClosePane => {
                let Some(v) = self.current() else {
                    return true;
                };
                let (id, busy) = (v.pane.id, v.busy());
                match busy {
                    Some(what) if !self.confirmed(id, &Ask::ClosePane) => {
                        let again = again(a, &self.config.keys);
                        let text = format!("This session is {what}. {again} to close it");
                        self.ask(id, text, Ask::ClosePane);
                    }
                    _ => self.close(el, id),
                }
            }
            Action::CloseTab => {
                let (Some(id), Some(t)) = (before, self.win.tabs.get(self.win.active)) else {
                    return true;
                };
                let panes = t.panes();
                let busy: Vec<_> = (panes.iter())
                    .filter_map(|&p| self.view(p)?.busy())
                    .collect();
                if !busy.is_empty() && !self.confirmed(id, &Ask::CloseTab) {
                    let what = busy_text(&busy, " in this tab");
                    let text = format!("{what}. {} to close it", again(a, &self.config.keys));
                    self.ask(id, text, Ask::CloseTab);
                    return true;
                }
                for p in panes {
                    self.close(el, p);
                }
            }
            Action::CycleTab(step) => {
                let n = self.win.tabs.len() as isize;
                if n > 0 {
                    let i = (self.win.active as isize + isize::from(step)).rem_euclid(n);
                    self.win.active = i as usize;
                    self.focus_moved(before);
                }
            }
            Action::GoToTab(i) => {
                if usize::from(i) < self.win.tabs.len() {
                    self.win.active = usize::from(i);
                    self.focus_moved(before);
                }
            }
            Action::SplitRight | Action::SplitDown => {
                let dir = if a == Action::SplitRight {
                    Dir::Right
                } else {
                    Dir::Down
                };
                self.add(None, split(dir));
            }
            Action::ReopenClosed => {
                let Some((cwd, claude)) = self.closed.take() else {
                    if let Some(id) = before {
                        let until = Some(Instant::now() + NOTICE);
                        self.set_notice(id, "No closed pane to reopen", until, true);
                    }
                    return true;
                };
                let id = PaneId(self.next_id);
                self.add(start_dir(&cwd), split(Dir::Right));
                if self.view(id).is_some() {
                    self.resume(id, claude);
                } else {
                    self.closed = Some((cwd, claude));
                }
            }
            Action::ToggleSidebar => {
                self.win.sidebar_expanded = !self.win.sidebar_expanded;
                self.request_redraw();
            }
            Action::ThemePicker => self.open_picker(),
            Action::Settings => self.open_settings(),
            Action::Palette => {
                self.commands = Some(Commands::default());
                self.request_redraw();
            }
            Action::Focus(dir) => {
                let (area, active) = (self.tab_area(), self.win.active);
                if let Some(t) = self.win.tabs.get_mut(active) {
                    t.focus_dir(dir, area);
                }
                self.focus_moved(before);
            }
            // With no split on that axis or no pane that way, the key goes
            // to the program, as it does in a tab of one pane.
            Action::Resize(dir) | Action::Swap(dir) => {
                let (area, min, (cw, ch)) = (self.tab_area(), self.min_pane(), self.cell());
                let active = self.win.active;
                let Some(t) = self.win.tabs.get_mut(active) else {
                    return false;
                };
                let moved = match (a, dir) {
                    (Action::Swap(_), _) => t.swap(dir, area),
                    (_, Dir::Left | Dir::Right) => t.resize(dir, cw as i32, area, min),
                    _ => t.resize(dir, ch as i32, area, min),
                };
                if !moved {
                    return false;
                }
                self.request_redraw();
            }
            // The focused session is skipped: the user is already looking
            // at it, and a session that exited stays red until closed.
            Action::JumpToAttention => {
                let waiting = (self.views.iter())
                    .filter(|v| Some(v.pane.id) != before)
                    .map(|v| (v.pane.id, v.pane.attn));
                if let Some(id) = crate::attention::jump_target(waiting) {
                    self.show(id);
                }
            }
            Action::Update => {
                let Some(id) = before else {
                    return false;
                };
                if self.updating.is_some() {
                    return true;
                }
                let Some((v, _)) = self.update.clone() else {
                    self.updating = Some(id);
                    self.set_notice(id, "Looking for a newer blitz\u{2026}", None, true);
                    let proxy = self.proxy.clone();
                    std::thread::spawn(move || {
                        // Always answered, or Ctrl+Shift+U stays busy.
                        let found = std::panic::catch_unwind(crate::update::check)
                            .unwrap_or_else(|_| Err(INTERNAL.into()));
                        let _ = proxy.send_event(UserEvent::Checked(found));
                    });
                    return true;
                };
                if !crate::update::installed() {
                    if !crate::update::open_page() {
                        let text = format!(
                            "Could not open a browser; get it at {}",
                            crate::update::PAGE
                        );
                        self.error(id, text);
                    }
                    return true;
                }
                // Updating restarts blitz, which ends every session.
                let busy = self.views.iter().filter(|v| v.busy().is_some()).count();
                if busy > 0 && !self.confirmed(id, &Ask::Update) {
                    let what = if busy == 1 {
                        "A session is"
                    } else {
                        "Sessions are"
                    };
                    let again = again(a, &self.config.keys);
                    let text = format!("{what} busy, and updating restarts blitz. {again}");
                    self.ask(id, text, Ask::Update);
                    return true;
                }
                self.updating = Some(id);
                self.set_notice(id, format!("Downloading blitz {v}\u{2026}"), None, true);
                let proxy = self.proxy.clone();
                std::thread::spawn(move || {
                    let done = std::panic::catch_unwind(move || crate::update::install(&v))
                        .unwrap_or_else(|_| Err(INTERNAL.into()));
                    let _ = proxy.send_event(UserEvent::Installed(done));
                });
            }
            // The next frame sizes each pane's session to its new place, as
            // it does after a split.
            Action::Zoom => {
                let active = self.win.active;
                let Some(t) = self.win.tabs.get_mut(active) else {
                    return false;
                };
                if t.zoom.is_none() && t.panes().len() < 2 {
                    return false;
                }
                t.toggle_zoom();
                self.request_redraw();
            }
            Action::Equalize => {
                let active = self.win.active;
                if let Some(t) = self.win.tabs.get_mut(active) {
                    t.zoom = None;
                    t.equalize();
                }
                self.request_redraw();
            }
            // Within the range the font_size setting allows.
            Action::FontSize(by) => {
                let set = self.config.font_size;
                let now = set + self.font_zoom;
                let pt = match by {
                    0 => set,
                    _ => (now + f32::from(by)).clamp(4.0, 72.0),
                };
                if pt != now {
                    self.font_zoom = pt - set;
                    self.reload_font();
                }
            }
            // Borderless on the window's monitor. winit puts the window back
            // where it was; the session keeps that place, not the monitor's.
            Action::Fullscreen => {
                self.note_place();
                if let Some(w) = &self.window {
                    let full = w.fullscreen().is_none();
                    w.set_fullscreen(full.then_some(Fullscreen::Borderless(None)));
                }
            }
            Action::JumpToPrompt(dir) => {
                let moved =
                    (self.current()).is_some_and(|v| lock(&v.pane.term).jump_to_prompt(dir > 0));
                if !moved {
                    return false;
                }
                self.request_redraw();
            }
            // The file is the user's to edit, so the hooks go on the
            // clipboard.
            Action::ClaudeSetup => {
                let Some(id) = before else {
                    return false;
                };
                let text = match crate::hook::hook_exe() {
                    Ok(hook) => crate::hook::claude_settings(&hook.to_string_lossy()),
                    Err(e) => {
                        self.set_notice(
                            id,
                            format!("blitz cannot find itself: {e}"),
                            Some(Instant::now() + HINT),
                            false,
                        );
                        return true;
                    }
                };
                let copied =
                    crate::clipboard::set_text(Some(HWND(self.hwnd as *mut c_void)), &text);
                let text = if copied {
                    "Copied hooks for Claude Code's settings.json; Claude Code 2.1.280 and later need none"
                } else {
                    "Could not copy to the clipboard"
                };
                self.set_notice(id, text, Some(Instant::now() + HINT), copied);
            }
            Action::Find => {
                let Some(id) = before else {
                    return false;
                };
                self.find = Some(Find::new(id));
                self.request_redraw();
            }
            // The palette's line takes the name, starting from the one the
            // user gave before.
            Action::RenameSession | Action::RenameTab => {
                let Some(id) = before else {
                    return false;
                };
                let (rename, name) = if a == Action::RenameSession {
                    let named = self.view(id).and_then(|v| v.pane.named.clone());
                    (Rename::Session(id), named.unwrap_or_default())
                } else {
                    let tab = self.win.tabs.get(self.win.active);
                    (
                        Rename::Tab(id),
                        tab.map(|t| t.name.clone()).unwrap_or_default(),
                    )
                };
                self.commands = Some(Commands {
                    filter: name,
                    sel: 0,
                    rename: Some(rename),
                });
                self.request_redraw();
            }
        }
        true
    }

    fn on_pane(&mut self, el: &ActiveEventLoop, id: PaneId, note: Note) {
        let Some(v) = self.view_mut(id) else {
            return;
        };
        match note {
            Note::Dirty => {
                v.pane.dirty.store(false, Ordering::Release);
                let mut events = Vec::new();
                let mut term = lock(&v.pane.term);
                term.take_events(&mut events);
                v.sync_until = term.sync_deadline();
                drop(term);
                // Output in a hidden tab only matters for what it tells
                // the sidebar.
                let shown = v.rect.is_some() || !events.is_empty();
                for e in events {
                    self.on_term_event(id, e);
                }
                if let Some(f) = self.find.as_mut().filter(|f| f.pane == id) {
                    f.stale = true;
                }
                if !self.checked_conpty {
                    self.checked_conpty = true;
                    if let Some(text) = crate::pty::inbox_notice() {
                        self.counters.inbox = true;
                        eprintln!("blitz: {text}");
                        self.set_notice(id, text, Some(Instant::now() + NOTICE), true);
                    }
                }
                if shown {
                    self.request_redraw();
                }
            }
            Note::Exit(code) => {
                v.pane.exit_code = Some(code);
                v.pane.cmd = Default::default();
                v.progress = None;
                self.taskbar_progress();
                self.attention(id, Ev::from_exit(code));
                // A clean exit or Ctrl+C closes the session; anything else
                // stays up so the output can be read.
                if matches!(code, 0 | 0xC000_013A) && self.args.selftest.is_none() {
                    self.close(el, id);
                    return;
                }
                // `notice_line` says so from now on.
                self.request_redraw();
            }
            Note::Dead => {
                self.attention(id, Ev::Error { sticky: true });
                self.set_notice(
                    id,
                    "this session stopped updating after an internal error",
                    None,
                    false,
                );
            }
        }
    }

    /// One thing a session's output did. Where Claude Code's signals
    /// disagree, its title says whether it is working, its hooks say when
    /// it needs the user, what to show and which session it is, and
    /// blitz's own prompt coming back says it has exited.
    fn on_term_event(&mut self, id: PaneId, e: Event) {
        let focus = self.focus_id() == Some(id);
        let bell = self.config.bell_attention;
        let Some(v) = self.view_mut(id) else {
            return;
        };
        match e {
            Event::Title(t) => {
                if claude_working_title(&t) {
                    v.claude_working.get_or_insert(Instant::now());
                }
                let silent = hooks_silent(v.claude_working, v.hooks_seen, Instant::now());
                let (was, now) = (v.pane.claude_title, claude_title(&t).map(|c| c.0));
                v.pane.claude_title = now;
                v.pane.title = t;
                if focus {
                    let t = v.pane.title.clone();
                    self.set_title(&t);
                }
                // This needs no hooks, and it sees a turn the user
                // interrupted end, which runs no hook at all.
                match (was, now) {
                    (w, Some(true)) if w != Some(true) => {
                        self.attention(id, Ev::Busy);
                    }
                    (Some(true), Some(false)) => {
                        self.attention(id, Ev::Quiet);
                        self.find_branch(id);
                    }
                    _ => {}
                }
                if silent {
                    self.hooks_hint(id, false);
                }
            }
            Event::Cwd(dir) => {
                v.pane.cwd = dir;
                self.find_branch(id);
            }
            Event::Prompt(m) => {
                let ended = v.pane.cmd.mark(m, Instant::now());
                if m == (PromptMark::A { blitz: true }) {
                    match prompt_back(&mut v.prompted, &mut v.resume) {
                        Prompt::Resume(line) => v.pane.send(line),
                        Prompt::First => {}
                        Prompt::Exited => {
                            v.pane.claude = None;
                            v.pane.claude_title = None;
                            v.pane.hooked = false;
                            self.attention(id, Ev::Exited);
                        }
                    }
                }
                // A long command that ended while the user looked away. Its
                // time, not that of a Claude Code turn before it, shows.
                if let Some((ev, msg, took)) = ended
                    && self.attention(id, ev)
                    && let Some(v) = self.view_mut(id)
                {
                    v.pane.msg = msg;
                    v.pane.attn.took = Some(took);
                }
            }
            Event::Notify { title, body } => match Ev::from_notify(&title, &v.pane.token) {
                Some((ev, session)) => {
                    v.hooks_seen = true;
                    v.pane.cmd.hooked = true;
                    note_hook(&mut v.pane.msg, &mut v.pane.claude, ev, session, body);
                    v.pane.hooked = ev != Ev::Idle;
                    if crate::attention::notify_protocol(&title).1 < crate::hook::PROTOCOL {
                        self.hooks_hint(id, true);
                    }
                    self.attention(id, ev);
                    if turn_ends(ev) {
                        self.find_branch(id);
                    }
                }
                // Another program's, or Claude Code's own without hooks
                // (OSC 9 or 777): like a bell.
                None if rings(bell, v.pane.hooked) => {
                    self.attention(id, Ev::Bell);
                }
                None => {}
            },
            Event::Progress { state, pct } => {
                let next = chrome::Progress::next(v.progress.map(|p| p.0), state, pct);
                v.progress = next.map(|p| (p, Instant::now()));
                self.taskbar_progress();
                self.request_redraw();
            }
            // It needs the user, unless they are already looking at the
            // pane; looking is all it asks for.
            Event::Bell if rings(bell, v.pane.hooked) => {
                self.attention(id, Ev::Bell);
            }
            _ => {}
        }
    }

    /// Reads the git branch of session `id`'s directory on another thread,
    /// so a slow drive never stalls every session. One read per session
    /// runs at a time; a directory reported meanwhile is read after it.
    fn find_branch(&mut self, id: PaneId) {
        let proxy = self.proxy.clone();
        let Some(v) = self.view_mut(id) else {
            return;
        };
        if v.finding_branch || v.pane.cwd.is_empty() {
            return;
        }
        v.finding_branch = true;
        let dir = v.pane.cwd.clone();
        std::thread::spawn(move || {
            let branch = git_branch(Path::new(&dir));
            let _ = proxy.send_event(UserEvent::Branch(id, dir, branch));
        });
    }

    fn on_branch(&mut self, id: PaneId, dir: String, branch: Option<String>) {
        let Some(v) = self.view_mut(id) else {
            return;
        };
        v.finding_branch = false;
        if v.pane.cwd == dir {
            v.pane.branch = branch;
            self.request_redraw();
        } else {
            self.find_branch(id);
        }
    }

    /// Shows on the taskbar button the progress of the session that
    /// reported last, of those still reporting one, or clears it.
    fn taskbar_progress(&mut self) {
        let latest = (self.views.iter())
            .filter_map(|v| v.progress)
            .max_by_key(|p| p.1)
            .map(|p| p.0);
        if latest == self.taskbar_shows {
            return;
        }
        if self.taskbar.is_none() {
            // SAFETY: COM calls on the window's thread, where winit has
            // started OLE for drag and drop.
            self.taskbar = unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) }
                .ok()
                .filter(|t: &ITaskbarList3| unsafe { t.HrInit() }.is_ok());
        }
        let Some(t) = &self.taskbar else {
            return;
        };
        self.taskbar_shows = latest;
        let hwnd = HWND(self.hwnd as *mut c_void);
        let (flag, pct) = match latest {
            None => (TBPF_NOPROGRESS, None),
            Some(p) => match p.state {
                2 => (TBPF_ERROR, p.pct),
                3 => (TBPF_INDETERMINATE, None),
                4 => (TBPF_PAUSED, p.pct),
                _ => (TBPF_NORMAL, p.pct),
            },
        };
        // SAFETY: a live window. A value set after the state keeps its
        // error or pause colour; with none known the bar is full, as in
        // the sidebar.
        unsafe {
            let _ = t.SetProgressState(hwnd, flag);
            if flag != TBPF_NOPROGRESS && flag != TBPF_INDETERMINATE {
                let _ = t.SetProgressValue(hwnd, u64::from(pct.unwrap_or(100)), 100);
            }
        }
    }

    /// Feeds a session's attention state; flashes the taskbar button when
    /// it changes to something the user should see while looking away.
    /// Returns true when the state changed.
    fn attention(&mut self, id: PaneId, ev: Ev) -> bool {
        // Back to work: blitz run ends when a session starts needing the
        // user. It closes first, so the focused pane is in view again for
        // the event, as if the game had never been open.
        let now = Instant::now();
        if self.game.is_some()
            && self
                .view(id)
                .is_some_and(|v| ends_game(v.pane.attn, ev, now))
        {
            self.close_game();
            self.game_ended = Some(now);
        }
        // While it is open, it covers the panes: the focused one is not in
        // view. Nor is anything while the user is away from the screen.
        let here = present(self.focused, idle_for());
        self.away |= !here;
        let attended = here && self.game.is_none() && self.focus_id() == Some(id);
        let away = !here && self.config.flash;
        let Some(v) = self.view_mut(id) else {
            return false;
        };
        let changed = v.pane.attn.apply(ev, attended, now);
        let kind = (changed && away)
            .then(|| flash_kind(v.pane.attn.state, &mut v.flashed, now))
            .flatten();
        // The sidebar shows the new state.
        self.request_redraw();
        if let (Some(kind), Some(w)) = (kind, &self.window) {
            w.request_user_attention(Some(kind));
        }
        changed
    }

    fn cell_at(&self, pos: PhysicalPosition<f64>) -> (u16, u16) {
        self.focus_id().map_or((0, 0), |id| self.cell_in(id, pos))
    }

    /// The cell of pane `id` under `pos`, clamped to its grid.
    fn cell_in(&self, id: PaneId, pos: PhysicalPosition<f64>) -> (u16, u16) {
        let Some(v) = self.view(id) else {
            return (0, 0);
        };
        let r = v.rect.unwrap_or_default();
        let (cw, ch) = self.cell();
        let x = (pos.x - f64::from(r.x)).max(0.0) as u32;
        let y = (pos.y - f64::from(r.y)).max(0.0) as u32;
        let col = (x / cw).min(u32::from(v.grid.0.saturating_sub(1)));
        let row = (y / ch).min(u32::from(v.grid.1.saturating_sub(1)));
        (col as u16, row as u16)
    }

    /// The part of the window the active tab's panes share, as the chrome
    /// lays it out.
    fn tab_area(&self) -> Rect {
        let size = self
            .window
            .as_ref()
            .map_or(PhysicalSize::new(0, 0), |w| w.inner_size());
        let size = (size.width as i32, size.height as i32);
        chrome::area(&self.win, size, self.scale as f32, self.update.is_some())
    }

    /// The session under a point in the window: a pane of the active tab,
    /// or a row of the sidebar. The second value is true for the sidebar.
    fn hit(&self, pos: PhysicalPosition<f64>) -> (Option<PaneId>, bool) {
        let (x, y) = (pos.x as i32, pos.y as i32);
        let inside = |r: &Rect| (r.x..r.right()).contains(&x) && (r.y..r.bottom()).contains(&y);
        let area = self.tab_area();
        let (rects, side) = match self.win.tabs.get(self.win.active) {
            Some(_) if x < area.x => (self.rows.clone(), true),
            Some(t) => (t.rects(area), false),
            None => (Vec::new(), false),
        };
        let id = rects.into_iter().find(|(_, r)| inside(r)).map(|(id, _)| id);
        (id, side)
    }

    /// The divider of the active tab under a point, if the settings panel
    /// is not over it. Dividers are 1 px wide, so a few px either side count.
    fn divider_at(&self, pos: PhysicalPosition<f64>) -> Option<(usize, Axis)> {
        let t = (self.win.tabs.get(self.win.active)).filter(|_| self.settings.is_none())?;
        let slop = (3.0 * self.scale).round() as i32;
        t.divider_at(self.tab_area(), pos.x as i32, pos.y as i32, slop)
    }

    /// The smallest pane, frame included, that still holds `MIN_COLS` by
    /// `MIN_ROWS` cells in a tab of several panes, which is what dragging
    /// and resizing work on.
    fn min_pane(&self) -> (i32, i32) {
        let (cw, ch) = self.cell();
        let frame = chrome::pane_frame(self.scale as f32, self.win.sidebar_expanded, true);
        (
            layout::MIN_COLS * cw as i32 + frame.0,
            layout::MIN_ROWS * ch as i32 + frame.1,
        )
    }

    /// Brings a session to the front: its tab becomes the active one and
    /// it gets focus.
    fn show(&mut self, id: PaneId) {
        let before = self.focus_id();
        if let Some(i) = self.win.tabs.iter().position(|t| t.root.contains(id)) {
            self.win.active = i;
            self.win.tabs[i].focus(id);
        }
        self.focus_moved(before);
    }

    /// Whether mouse events go to the program rather than to selection.
    fn mouse_to_program(&self, mods: &Mods) -> Option<InputModes> {
        let m = self.modes();
        (m.mouse != MouseMode::Off && !(mods.lshift || mods.rshift)).then_some(m)
    }

    /// Sends a mouse event at the pointer to pane `id`'s program, in its
    /// own mouse mode.
    fn mouse_report(&mut self, id: PaneId, kind: MouseKind, button: u8, mods: Mods) {
        let (col, row) = self.cell_in(id, self.mouse.pos);
        let Some(v) = self.views.iter().find(|v| v.pane.id == id) else {
            return;
        };
        let m = lock(&v.pane.term).input_modes();
        let ev = MouseEv {
            kind,
            button,
            col,
            row,
            mods,
        };
        let mut out = Vec::new();
        if self.mouse.tracker.encode(ev, &m, &mut out) {
            v.pane.send(out);
        }
    }

    fn on_mouse_button(&mut self, el: &ActiveEventLoop, state: ElementState, button: MouseButton) {
        let b = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            _ => return,
        };
        let mods = mods_now();
        let pressed = state == ElementState::Pressed;
        // Presses go to the command palette or the settings panel; a
        // release still goes wherever its press went.
        if pressed && self.commands.is_some() {
            if b == 0 {
                self.commands_click(el);
            }
            return;
        }
        if pressed && self.settings.is_some() && self.picker.is_none() {
            if b == 0 {
                self.settings_click();
            }
            return;
        }
        // blitz run covers the panes and keeps the sidebar where it is.
        if pressed && self.game.is_some() {
            return;
        }
        let (x, y) = (self.mouse.pos.x as i32, self.mouse.pos.y as i32);
        let on_banner = (self.banner)
            .is_some_and(|r| (r.x..r.right()).contains(&x) && (r.y..r.bottom()).contains(&y));
        if pressed && b == 0 && on_banner {
            self.act(el, Action::Update);
            return;
        }
        if b == 0 && !pressed && self.mouse.divider.take().is_some() {
            return;
        }
        if let Some((i, _)) = self
            .divider_at(self.mouse.pos)
            .filter(|_| pressed && b == 0)
        {
            self.mouse.divider = Some((i, self.min_pane()));
            return;
        }
        // A click on another pane or in the sidebar only moves focus.
        if pressed {
            let (id, side) = self.hit(self.mouse.pos);
            if side || id.is_some_and(|id| Some(id) != self.focus_id()) {
                if let Some(id) = id {
                    self.show(id);
                }
                return;
            }
        }
        let program = self.mouse_to_program(&mods).and(self.focus_id());
        if let Some(id) = route_button(&mut self.mouse.reported, b, pressed, program) {
            let kind = if pressed {
                MouseKind::Press
            } else {
                MouseKind::Release
            };
            self.mouse_report(id, kind, b as u8, mods);
            // A click can pick an answer in the program's menu.
            if pressed {
                self.answered(id);
            }
            return;
        }
        if b != 0 {
            return;
        }
        if pressed {
            // Ctrl+click opens a link; with mouse reporting on, the click
            // got here because Shift was held too.
            if (mods.lctrl || mods.rctrl)
                && let Some((target, _)) = self.link_under(self.mouse.pos)
            {
                self.open_link(&target);
                return;
            }
            // With mouse reporting on, Shift is what brought the click
            // here, so it does not extend.
            let shift = mods.lshift || mods.rshift;
            let extend = shift && self.modes().mouse == MouseMode::Off;
            self.press(&mods, extend);
        } else {
            self.mouse.drag = None;
            self.mouse.scroll_at = None;
        }
    }

    /// The cell under `pos` in the focused pane, and the terminal's line
    /// epoch.
    fn line_cell(&self, pos: PhysicalPosition<f64>) -> Option<(u32, Pos)> {
        let v = self.current()?;
        let (col, row) = self.cell_at(pos);
        let t = lock(&v.pane.term);
        Some((t.line_epoch(), (t.view_top() + usize::from(row), col)))
    }

    /// A left press that goes to selection. Starts a drag from the cell
    /// under the pointer: by words after a double click, by lines after a
    /// triple click, and a block with Alt held. With `extend`, moves the
    /// end of the selection there instead.
    fn press(&mut self, mods: &Mods, extend: bool) {
        let Some((epoch, here)) = self.line_cell(self.mouse.pos) else {
            return;
        };
        let held = self.selection.as_ref().map(|s| s.drag);
        if let Some(drag) = held.filter(|d| extend && d.epoch == epoch) {
            self.mouse.drag = Some(drag);
            self.extend_drag();
            return;
        }
        // SAFETY: a plain query.
        let within = Duration::from_millis(u64::from(unsafe { GetDoubleClickTime() }));
        let now = Instant::now();
        let n = clicks(self.mouse.click, here, now, within);
        self.mouse.click = Some((now, here, n));
        let block = mods.lalt || mods.ralt;
        let unit = if block { 1 } else { n };
        let Some(v) = self.current() else {
            return;
        };
        let drag = Drag::new(&lock(&v.pane.term), &self.theme.pal, here, unit, block);
        self.mouse.drag = Some(drag);
        if self.selection.take().is_some() {
            self.request_redraw();
        }
        // A double or triple click selects at once.
        self.extend_drag();
    }

    /// Grows the selection being dragged to the cell under the pointer. A
    /// single click shows nothing until the pointer leaves its cell.
    fn extend_drag(&mut self) {
        let Some(drag) = self.mouse.drag else {
            return;
        };
        let Some(v) = self.current() else {
            return;
        };
        let (col, row) = self.cell_at(self.mouse.pos);
        let term = lock(&v.pane.term);
        if term.line_epoch() != drag.epoch {
            drop(term);
            self.mouse.drag = None;
            return;
        }
        let head = (term.view_top() + usize::from(row), col);
        if self.selection.is_none() && drag.unit == 1 && head == drag.anchor.0 {
            return;
        }
        let s = Selection::new(&term, &self.theme.pal, drag, head);
        drop(term);
        if self.selection.as_ref().map(|o| (o.start, o.end)) != Some((s.start, s.end)) {
            self.selection = Some(s);
            self.request_redraw();
        }
    }

    /// While a drag holds the pointer above or below the focused pane,
    /// scrolls toward it, faster the farther out it is, and grows the
    /// selection; then waits for the next step. The alternate screen has
    /// no scrollback to scroll.
    fn autoscroll(&mut self) {
        self.mouse.scroll_at = None;
        let Some(v) = self.current().filter(|_| self.mouse.drag.is_some()) else {
            return;
        };
        let (Some(r), false) = (v.rect, v.snap.alt_screen) else {
            return;
        };
        let y = self.mouse.pos.y as i32;
        let out = if y < r.y {
            r.y - y
        } else if y >= r.bottom() {
            r.bottom() - 1 - y
        } else {
            return;
        };
        let ch = self.cell().1 as i32;
        let lines = (out.abs() / ch + 1).min(10) * out.signum();
        self.scroll(lines as isize);
        self.mouse.scroll_at = Some(Instant::now() + AUTOSCROLL);
    }

    /// The link under `pos` in the focused pane, with the cells it
    /// covers: an OSC 8 hyperlink, else a URL or the path of a file or
    /// folder that exists in the text around it.
    fn link_under(&self, pos: PhysicalPosition<f64>) -> Option<(Target, (Pos, Pos))> {
        let v = self
            .current()
            .filter(|v| self.hit(pos).0 == Some(v.pane.id))?;
        let (col, row) = self.cell_at(pos);
        let t = lock(&v.pane.term);
        let at = (t.view_top() + usize::from(row), col);
        if let Some((uri, a, b)) = t.link_at(at.0, at.1) {
            return Some((Target::Uri(uri.to_owned()), (a, b)));
        }
        let l = Logical::new(&t, &self.theme.pal, at.0);
        let here = l.cells[l.index(at)?].0;
        let (range, found) =
            (crate::links::scan(&l.text).into_iter()).find(|(r, _)| r.contains(&here))?;
        let target = match found {
            Link::Url(u) => Target::Uri(u),
            Link::Path(p) => Target::Path(crate::links::resolve(&p, &v.pane.cwd)?),
        };
        Some((target, l.span(range)))
    }

    /// Underlines the link under the pointer, and shows the hand, while
    /// Ctrl is held; with mouse reporting on, Ctrl and Shift.
    fn update_hover(&mut self) {
        let mods = mods_now();
        let ctrl = (mods.lctrl || mods.rctrl) && self.mouse_to_program(&mods).is_none();
        let hover = (ctrl && self.mouse.drag.is_none())
            .then(|| self.link_under(self.mouse.pos))
            .flatten()
            .and_then(|(_, (a, b))| {
                let epoch = lock(&self.current()?.pane.term).line_epoch();
                Some((epoch, a, b))
            });
        self.set_hover(hover);
    }

    fn set_hover(&mut self, hover: Option<(u32, Pos, Pos)>) {
        if hover == self.hover {
            return;
        }
        if let Some(w) = &self.window
            && hover.is_some() != self.hover.is_some()
        {
            w.set_cursor(if hover.is_some() {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
            });
        }
        self.hover = hover;
        self.request_redraw();
    }

    /// Opens a link, or says in the pane why not.
    fn open_link(&mut self, target: &Target) {
        if let Err(e) = crate::links::open(target)
            && let Some(id) = self.focus_id()
        {
            self.error(id, e);
        }
    }

    fn on_mouse_move(&mut self, pos: PhysicalPosition<f64>) {
        self.mouse.pos = pos;
        if let Some((i, min)) = self.mouse.divider {
            let (area, active) = (self.tab_area(), self.win.active);
            if let Some(t) = self.win.tabs.get_mut(active)
                && t.drag(i, pos.x as i32, pos.y as i32, area, min)
            {
                self.request_redraw();
            }
            return;
        }
        if self.mouse.drag.is_some() {
            self.extend_drag();
            if self.mouse.scroll_at.is_none() {
                self.autoscroll();
            }
            return;
        }
        let over = self.divider_at(pos).map(|d| d.1);
        if over != self.mouse.over_divider {
            self.mouse.over_divider = over;
            if let Some(w) = &self.window {
                w.set_cursor(match over {
                    Some(Axis::Row) => CursorIcon::ColResize,
                    Some(Axis::Column) => CursorIcon::RowResize,
                    None => CursorIcon::Default,
                });
            }
        }
        // blitz run covers the panes, so programs see no motion under it.
        if self.game.is_some() {
            return;
        }
        self.update_hover();
        let mods = mods_now();
        // A drag goes where its press went, like the release will.
        let held = (0..3).find_map(|b| Some((b, self.mouse.reported[b]?)));
        if let Some((b, id)) = held {
            self.mouse_report(id, MouseKind::Move, b as u8, mods);
        } else if self.mouse_to_program(&mods).is_some()
            && let Some(id) = self.focus_id()
        {
            self.mouse_report(id, MouseKind::Move, 3, mods);
        }
    }

    fn on_wheel(&mut self, delta: MouseScrollDelta) {
        let (_, ch) = self.cell();
        self.mouse.wheel += match delta {
            MouseScrollDelta::LineDelta(_, y) => f64::from(y),
            MouseScrollDelta::PixelDelta(p) => p.y / f64::from(ch),
        };
        let steps = self.mouse.wheel.trunc();
        self.mouse.wheel -= steps;
        if steps == 0.0 {
            return;
        }
        if let Some(cm) = &mut self.commands {
            cm.move_by(-steps as isize);
            self.request_redraw();
            return;
        }
        if let Some(p) = self.settings.as_mut().filter(|_| self.picker.is_none()) {
            p.move_by(-steps as isize);
            self.request_redraw();
            return;
        }
        if self.game.is_some() {
            return;
        }
        // Over another pane, the wheel scrolls that pane's history without
        // moving focus.
        // Programs in unfocused panes never get wheel reports;
        // route them by pane if a full-screen app needs them.
        let (under, side) = self.hit(self.mouse.pos);
        if side {
            return;
        }
        if let Some(v) = under
            .filter(|&id| Some(id) != self.focus_id())
            .and_then(|id| self.view(id))
        {
            let mut t = lock(&v.pane.term);
            if !t.input_modes().alt_screen {
                t.scroll_viewport(steps as isize * WHEEL_LINES);
                self.request_redraw();
            }
            return;
        }
        let mods = mods_now();
        if let Some(id) = self.mouse_to_program(&mods).and(self.focus_id()) {
            let kind = if steps > 0.0 {
                MouseKind::WheelUp
            } else {
                MouseKind::WheelDown
            };
            for _ in 0..steps.abs() as u32 {
                self.mouse_report(id, kind, 0, mods);
            }
        } else if self.modes().alt_screen {
            // As in xterm's alternateScroll: pagers such as less and man
            // have no scrollback to show, but scroll by the arrow keys.
            let keys = wheel_keys(steps as isize * WHEEL_LINES, &self.modes());
            self.send(keys);
        } else {
            self.scroll(steps as isize * WHEEL_LINES);
        }
    }

    /// Draws a frame. Resizes each visible session first when its pane
    /// changed size, at most once per frame.
    fn redraw(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.ensure_gfx();
        let Some(g) = &self.gfx else {
            return;
        };
        // Wait for the swap chain before reading the panes, so the frame
        // shows output that arrived during the wait.
        g.chain.wait(100);
        let started = Instant::now();
        let mut waited = Duration::ZERO;
        if let Some((g, at)) = &mut self.game {
            // The game waits while the window is in the background, and a
            // long frame counts as one short step rather than a jump.
            let dt = if self.focused {
                (started.saturating_duration_since(*at)).min(Duration::from_millis(50))
            } else {
                Duration::ZERO
            };
            *at = started;
            if g.step(dt.as_secs_f32()) && g.best > run::load_best() {
                run::save_best(g.best);
            }
        }
        let (cw, ch) = self.cell();
        let focus = self.focus_id();
        let cursor = self.current().map(|v| lock(&v.pane.term).cursor());
        let sessions = self.sessions();
        let preedit = cursor
            .filter(|_| !self.preedit.is_empty())
            .map(|(c, r, _)| (c, r, self.preedit.as_str()));
        let mut chrome = chrome::build(&self.model(&self.win, &sessions, preedit));
        self.rows = std::mem::take(&mut chrome.rows);
        self.banner = chrome.banner;
        self.settings_hits = chrome.settings.take();
        self.commands_hits = chrome.commands.take();
        if let (Some(p), Some(h)) = (&mut self.settings, &self.settings_hits) {
            p.top = h.top;
        }

        let split = chrome.panes.len() >= 2;
        let mut dimmed = Vec::new();
        for v in &mut self.views {
            v.rect = None;
        }
        for &(id, rect) in &chrome.panes {
            let Some(v) = self.views.iter_mut().find(|v| v.pane.id == id) else {
                continue;
            };
            let fit = |n: i32, cell: u32| (n / cell as i32).clamp(1, i32::from(u16::MAX)) as u16;
            let grid = (fit(rect.w, cw), fit(rect.h, ch));
            let mut find = self.find.as_mut().filter(|f| f.pane == id);
            if grid != v.grid {
                v.grid = grid;
                v.pane.resize(grid.0, grid.1);
                lock(&v.pane.term).set_cell_px(cw as u16, ch as u16);
                // A new width rewraps the lines that matched.
                if let Some(f) = &mut find {
                    f.stale = true;
                }
            }
            v.rect = Some(rect);
            let sel = self.selection.as_mut().filter(|_| Some(id) == focus);
            let mut term = lock(&v.pane.term);
            v.sync_until = term.sync_deadline();
            if !refresh(&mut term, &mut v.snap, &self.theme.pal, sel) {
                self.selection = None;
            }
            v.snap.highlights.clear();
            if let Some(f) = find {
                // ponytail: searches all of the scrollback again on each
                // frame with new output, ~50 ms for 100,000 full rows; keep
                // the matches in scrollback rows if that ever shows.
                if f.stale {
                    f.search(&term, grid.1);
                }
                v.snap.highlight(&f.found, f.cur);
            }
            if Some(id) == focus {
                let (top, size) = (term.view_top(), (v.snap.cols, v.snap.rows));
                let sel = self.selection.as_ref();
                v.snap.selection =
                    sel.and_then(|s| in_view(s.start, s.end, s.drag.block, top, size));
                v.snap.block = sel.is_some_and(|s| s.drag.block);
                let hover = self.hover.filter(|h| h.0 == term.line_epoch());
                v.snap.hover = hover.and_then(|(_, a, b)| in_view(a, b, false, top, size));
            } else if split {
                dimmed.push(id);
            }
        }

        let pal = self.theme.pal;
        let scenery = (self.config.scenery != "off").then(|| {
            let mut prims = Vec::new();
            let all = Rect {
                x: 0,
                y: 0,
                w: size.width as i32,
                h: size.height as i32,
            };
            let (scene, t) = (&self.config.scenery, self.anim_time());
            crate::arcade::scenery::draw(&mut prims, scene, all, t, self.scale as f32, &pal);
            chrome::Chrome {
                prims,
                ..Default::default()
            }
        });
        let Some(g) = &mut self.gfx else {
            return;
        };
        let result = (|| {
            g.chain.resize(&g.r.gpu, size.width, size.height)?;
            for _ in 0..2 {
                g.r.begin();
                if let Some(s) = &scenery {
                    g.r.chrome(s);
                }
                for v in &self.views {
                    // A pane with no room for a cell shows nothing, rather
                    // than its 1x1 grid drawn over its neighbour.
                    let Some(at) = v.rect.filter(|r| r.w >= cw as i32 && r.h >= ch as i32) else {
                        continue;
                    };
                    let dim = dimmed.contains(&v.pane.id);
                    let hollow = dim || !self.focused;
                    g.r.grid(&v.snap, &pal, at.x, at.y, dim, hollow, scenery.is_none());
                    if let Some((text, dim)) = notice_line(v.notice.as_ref(), v.pane.exit_code) {
                        draw_notice(&mut g.r, &pal, at, v.grid, &text, dim);
                    }
                }
                g.r.chrome(&chrome);
                let rtv = g.chain.rtv(&g.r.gpu)?;
                if !g.r.draw(&rtv, size.width, size.height, pal.bg)? {
                    break;
                }
            }
            if self.capture_then_exit
                && let Some(path) = &self.args.capture
            {
                let t = g.r.gpu.offscreen(size.width, size.height)?;
                g.r.draw(&t.rtv, size.width, size.height, pal.bg)?;
                let px = g.r.gpu.read(&t)?;
                if let Err(e) = write_bmp(path, size.width, size.height, &px) {
                    eprintln!("blitz: {}: {e}", path.display());
                }
            }
            let t = Instant::now();
            let shown = g.chain.present();
            waited += t.elapsed();
            shown
        })();
        self.counters.frame_cpu_ms += (started.elapsed() - waited).as_secs_f64() * 1000.0;
        match result {
            Ok(_) => {
                self.counters.frames += 1;
                if self.counters.first_present_ms.is_none() {
                    self.counters.first_present_ms =
                        Some(self.started.elapsed().as_secs_f64() * 1000.0);
                }
                // Glyphs still waiting for a font lookup come next frame.
                if self.gfx.as_ref().is_some_and(|g| g.r.pending()) {
                    self.request_redraw();
                }
            }
            Err(e) => {
                eprintln!("blitz: render: {e}");
                // The atlas is only a cache: build everything again, at once
                // for a lost device. Other errors wait, so one that persists
                // cannot spin; either way the frame is drawn again.
                self.gfx = None;
                if is_device_lost(&e) {
                    self.request_redraw();
                } else {
                    self.gfx_retry = Some(Instant::now() + GFX_RETRY);
                }
            }
        }

        let at = self.current().and_then(|v| v.rect).zip(cursor);
        if let Some((r, (col, row, _))) = at {
            let at = (
                r.x + i32::from(col) * cw as i32,
                r.y + i32::from(row) * ch as i32,
            );
            if self.ime_at != Some(at)
                && let Some(w) = &self.window
            {
                self.ime_at = Some(at);
                w.set_ime_cursor_area(PhysicalPosition::new(at.0, at.1), PhysicalSize::new(cw, ch));
            }
        }
    }

    /// Notes where the window is, unless it is minimized, maximized or full
    /// screen, none of which is a place to go back to.
    fn note_place(&mut self) {
        let Some(w) = &self.window else {
            return;
        };
        if !w.is_maximized()
            && w.is_minimized() != Some(true)
            && w.fullscreen().is_none()
            && let Ok(p) = w.outer_position()
        {
            let size = w.inner_size();
            self.placed = Geometry {
                x: p.x,
                y: p.y,
                w: size.width,
                h: size.height,
                maximized: false,
            };
        }
    }

    /// Saves the session when its tabs, splits or folders changed since
    /// the last save, or always with `force`. The window's place alone
    /// does not count, so dragging the window writes nothing until exit.
    fn save_session(&mut self, force: bool) {
        if !self.persist || self.views.is_empty() {
            return;
        }
        let meta = |id| {
            let v = self.view(id);
            PaneMeta {
                cwd: v.map(|v| v.pane.cwd.clone()).unwrap_or_default(),
                claude: v.and_then(|v| v.pane.claude.clone()),
                key: v.map(|v| v.key.clone()).unwrap_or_default(),
                done: (v.filter(|v| v.pane.attn.state == Attn::DoneUnseen))
                    .map(|v| v.pane.msg.clone()),
                name: v.and_then(|v| v.pane.named.clone()),
            }
        };
        let mut s = session::State::capture(&self.win, self.placed, meta);
        let same = self.saved.as_ref().is_some_and(|old| {
            (old.sidebar_expanded, old.active, &old.tabs) == (s.sidebar_expanded, s.active, &s.tabs)
        });
        let dragging = self.mouse.divider.is_some();
        if !force && !save_now(!same, dragging, Instant::now(), &mut self.save_after) {
            return;
        }
        self.save_after = None;
        self.note_place();
        if let Some(w) = &self.window {
            s.window = Geometry {
                maximized: w.is_maximized(),
                ..self.placed
            };
        }
        // Output, which changes all the time, is saved only at exit, and
        // only once the layout holding the keys it is filed by was written.
        match session::save(&s) {
            Ok(()) if force => self.save_output(),
            Ok(()) => {}
            Err(e) => eprintln!("blitz: saving the session: {e}"),
        }
        // Kept even when the write failed, so it is not retried every turn.
        self.saved = Some(s);
    }

    /// Saves each pane's recent output when `restore_scrollback` is on, and
    /// deletes what an earlier run saved when it is off.
    fn save_output(&self) {
        let stamp = local_stamp();
        let views = (self.views.iter()).filter(|_| self.config.restore_scrollback);
        let panes: Vec<_> = views
            .filter_map(|v| {
                let term = lock(&v.pane.term);
                let mut text = term.scrollback_text();
                // A full-screen program's screen is not output.
                if !term.input_modes().alt_screen {
                    text.push('\n');
                    text += &term.screen_text();
                }
                let text = last_lines(&text, SAVED_LINES);
                (!text.is_empty()).then(|| (v.key.clone(), format!("{stamp}\n{text}")))
            })
            .collect();
        if let Err(e) = session::save_output(&panes) {
            eprintln!("blitz: saving output: {e}");
        }
    }

    /// The soonest time something on screen changes by itself.
    fn next_deadline(&self) -> Option<Instant> {
        let now = Instant::now();
        // Hidden panes are drawn when shown, timed out or not.
        let sync = (self.views.iter())
            .filter(|v| v.rect.is_some())
            .filter_map(|v| v.sync_until)
            .filter(|&t| t > now)
            .min();
        let notice = self
            .views
            .iter()
            .filter_map(|v| v.notice.as_ref()?.until)
            .min();
        // The sidebar counts how long each session has been working, and
        // how long a question has waited.
        let sidebar = self.views.len() >= 2 && self.win.sidebar_expanded;
        let timer = (self.views.iter())
            .filter(|_| sidebar)
            .filter_map(|v| chrome::row_tick(&v.pane.attn, now))
            .min();
        let resume = (self.views.iter())
            .filter_map(|v| Some(v.resume.as_ref()?.1))
            .min();
        // A changed layout waiting to be saved, and a renderer to retry.
        // A minimized window draws nothing, so it has nothing to retry.
        let shown = (self.window.as_ref())
            .is_some_and(|w| w.inner_size().width > 0 && w.inner_size().height > 0);
        let gfx = self.gfx_retry.filter(|_| self.gfx.is_none() && shown);
        // Only a window in use animates.
        let still =
            !self.motion || (self.config.scenery == "off" && !(self.config.mascot && sidebar));
        let anim = match (self.focused, self.game.is_some(), still) {
            (true, true, _) => Some(now + GAME_FRAME),
            (true, false, false) => Some(now + SCENERY_FRAME),
            _ => None,
        };
        [
            sync,
            notice,
            timer,
            resume,
            self.save_after,
            gfx,
            anim,
            self.mouse.scroll_at,
        ]
        .into_iter()
        .flatten()
        .min()
    }
}

/// Whether a layout that `changed` since the last save is written now. A
/// divider drag or a held resize key changes it many times a second, so a
/// change is written `SAVE_DELAY` after it was first seen, or that long
/// after the drag ends; `due` holds when. A drag holds no deadline, which
/// would wake the event loop over and over once passed.
fn save_now(changed: bool, dragging: bool, now: Instant, due: &mut Option<Instant>) -> bool {
    if !changed {
        *due = None;
        return false;
    }
    match *due {
        _ if dragging => {
            *due = None;
            false
        }
        None => {
            *due = Some(now + SAVE_DELAY);
            false
        }
        Some(t) if now >= t => {
            *due = None;
            true
        }
        Some(_) => false,
    }
}

/// Which pane's program gets a press or release of mouse button `b`, or
/// `None` when it is the window's, for selecting. A press goes to
/// `program`, the focused pane when its program takes the mouse and Shift
/// is up. A release goes wherever its press went, whatever Shift or focus
/// did in between, so no program is left with a button held down.
fn route_button(
    reported: &mut [Option<PaneId>; 3],
    b: usize,
    pressed: bool,
    program: Option<PaneId>,
) -> Option<PaneId> {
    if pressed {
        reported[b] = program;
        program
    } else {
        reported[b].take()
    }
}

/// Whether Windows shows animations; off under Accessibility, Visual
/// effects. Read once at start.
fn animations_on() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut on = windows::core::BOOL(1);
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&raw mut on).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    read.is_err() || on.as_bool()
}

/// How long after a session needing the user closes blitz run a jump key
/// is still taken for the game, and kept from the pane.
const LATE_JUMP: Duration = Duration::from_millis(400);

/// The keys that jump in blitz run.
fn is_jump(vk: u16) -> bool {
    matches!(vk, VK_SPACE | VK_UP | VK_W)
}

/// Whether `ev` closes blitz run: it starts the session needing the user.
/// A repeat of a needs-you already showing does not.
fn ends_game(attn: crate::attention::PaneAttn, ev: Ev, now: Instant) -> bool {
    let mut seen = attn;
    seen.apply(ev, false, now) && seen.state == Attn::NeedsYou
}

/// Whether a pane title shows Claude Code working, as the sidebar reads
/// it too.
fn claude_working_title(title: &str) -> bool {
    claude_title(title).is_some_and(|(working, _)| working)
}

/// The hint about Claude Code's hooks, `older` than this blitz or not
/// reporting, and where to set them up: the palette, by its keys if it has
/// some. `blitz setup claude` run from a shell prints nothing there.
fn hooks_hint_text(older: bool, palette: Option<String>) -> String {
    let what = if older {
        "Claude Code's settings run an older blitz-hook"
    } else {
        "Claude Code's hooks are not reporting to blitz"
    };
    let palette = palette.map_or_else(|| "the command palette".into(), |k| k + ",");
    format!("{what} \u{b7} {palette} Claude Code setup")
}

/// Whether Claude Code's hooks are not reporting: it has shown it is
/// working, at `working` first, for a while, and no hook has said a word.
fn hooks_silent(working: Option<Instant>, hooks_seen: bool, now: Instant) -> bool {
    !hooks_seen && working.is_some_and(|t| now.saturating_duration_since(t) >= HOOKS_QUIET)
}

/// Whether a press of `vk` is a jump meant for blitz run that arrived
/// after a session closed it, `since` ago.
fn late_jump(vk: u16, since: Option<Duration>) -> bool {
    is_jump(vk) && since.is_some_and(|d| d < LATE_JUMP)
}

/// What a hook's notification says besides the state. Its text replaces
/// the session's message, even when the state stays: the title may have
/// ended the turn before the hook with the reply came, and `idle`, sent
/// with none, clears it; `ready` keeps what the last turn said. It names
/// the Claude Code session, which `idle` (SessionEnd: the user quit
/// Claude) ends, so there is nothing left to resume.
fn note_hook(
    msg: &mut String,
    claude: &mut Option<String>,
    ev: Ev,
    session: Option<&str>,
    body: String,
) {
    if ev == Ev::Idle {
        *claude = None;
    } else if let Some(id) = session {
        *claude = Some(id.to_owned());
    }
    if ev != Ev::Ready {
        *msg = body;
    }
}

/// What blitz's own prompt coming back in a pane means.
#[derive(Debug, PartialEq, Eq)]
enum Prompt {
    /// The shell is ready: bring back its Claude Code session.
    Resume(String),
    /// The shell's first prompt: nothing ran in it yet, though a resume
    /// the timer typed ahead of it is about to.
    First,
    /// Whatever ran has ended, Claude Code too, even one that crashed or
    /// was killed and could tell no hook. A resume that failed is not
    /// tried again at the next start either.
    Exited,
}

fn prompt_back(prompted: &mut bool, resume: &mut Option<(String, Instant)>) -> Prompt {
    let first = !std::mem::replace(prompted, true);
    match resume.take() {
        Some((line, _)) => Prompt::Resume(line),
        None if first => Prompt::First,
        None => Prompt::Exited,
    }
}

/// Tells the program in `v` whether its pane has keyboard focus: now if it
/// asked for focus reports, or else once it does.
fn tell_focus(v: &View, focused: bool) {
    let mut out = Vec::new();
    // Under one lock, so a program turning reports on meanwhile is told
    // either way, and its reports reach it in the order they were made.
    let mut term = lock(&v.pane.term);
    term.set_focused(focused);
    vt::encode_focus(focused, &term.input_modes(), &mut out);
    v.pane.send(out);
    drop(term);
}

/// Whether the user is at the window: it is in front, and they touched a
/// key or the mouse in the last `AWAY_AFTER`, `idle` being how long ago.
/// Walking away from blitz must not let a question pass as seen.
fn present(focused: bool, idle: Duration) -> bool {
    focused && idle < AWAY_AFTER
}

/// How long ago the last key or mouse input anywhere was; zero when
/// Windows cannot say.
fn idle_for() -> Duration {
    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: `info` is a LASTINPUTINFO with its size set, as the call
    // requires.
    if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        return Duration::ZERO;
    }
    // SAFETY: plain Win32 call with no arguments.
    let now = unsafe { windows::Win32::System::SystemInformation::GetTickCount() };
    // Both are milliseconds since boot, kept in 32 bits, so they wrap.
    Duration::from_millis(u64::from(now.wrapping_sub(info.dwTime)))
}

/// Whether a hook's `ev` ends a stretch of Claude Code's work. It may have
/// switched branches meanwhile, which the shell, still waiting under it,
/// never reports.
fn turn_ends(ev: Ev) -> bool {
    matches!(ev, Ev::Done | Ev::NeedsYou | Ev::Idle)
}

/// Whether a bell, or a notification without the pane's token, needs the
/// user: when `bell_attention` is on, and Claude Code's hooks do not
/// report for the pane already, which would only say the same twice.
fn rings(bell_attention: bool, hooked: bool) -> bool {
    bell_attention && !hooked
}

/// "A session is working", or "3 sessions are busy", of the sessions
/// that closing would cut short and what each is doing; `place` follows
/// "session".
fn busy_text(busy: &[&str], place: &str) -> String {
    match busy {
        [what] => format!("A session{place} is {what}"),
        _ => format!("{} sessions{place} are busy", busy.len()),
    }
}

/// Whether a key message for Alt+F4 goes on to Windows, which closes the
/// window on a press: all but the auto-repeat of a held one, so holding it
/// cannot answer the question a busy session asks.
fn alt_f4_passes(down: bool, lparam: isize) -> bool {
    !down || !keymap::held_before(lparam)
}

/// Caps Lock, Num Lock and Scroll Lock, which like a modifier change how
/// keys type and answer no question.
fn lock_key(vk: u16) -> bool {
    matches!(vk, 0x14 | 0x90 | 0x91)
}

/// How to confirm action `a`: its first key again, or with none, the
/// palette.
fn again(a: Action, keys: &[keymap::Binding]) -> String {
    match keymap::keys_for(a, keys) {
        Some(k) => format!("Press {k} again"),
        None => {
            let label = (keymap::ACTIONS.iter())
                .find(|x| x.0 == a)
                .map_or("it", |x| x.2);
            format!("Run {label} again")
        }
    }
}

/// How to flash the taskbar for a session that just changed to `state`
/// while the window is in the background: urgently when it needs the
/// user or failed, gently when it finished, and at most once per session
/// every `FLASH_GAP`. `last` is when this session last flashed.
fn flash_kind(state: Attn, last: &mut Option<Instant>, now: Instant) -> Option<UserAttentionType> {
    let kind = match state {
        Attn::NeedsYou | Attn::Error => UserAttentionType::Critical,
        Attn::DoneUnseen => UserAttentionType::Informational,
        Attn::Working | Attn::Idle => return None,
    };
    if last.is_some_and(|t| now.saturating_duration_since(t) < FLASH_GAP) {
        return None;
    }
    *last = Some(now);
    Some(kind)
}

/// Scrolls `term` so match `m` shows in the middle of its `rows` high view,
/// unless it shows already. Returns whether the view moved.
fn reveal(term: &mut vt::Terminal, m: Found, rows: u16) -> bool {
    let (top, rows) = (term.view_top(), usize::from(rows));
    if (top..top + rows).contains(&m.start.0) {
        return false;
    }
    term.scroll_to(m.start.0.saturating_sub(rows / 2));
    term.view_top() != top
}

/// `n` presses of Up, or of Down for a negative `n`, each with its
/// release, as the program asked keys to be sent.
fn wheel_keys(n: isize, m: &InputModes) -> Vec<u8> {
    let (vk, scan, key) = if n > 0 {
        (VK_UP, 0x48, vt::Key::Up)
    } else {
        (VK_DOWN, 0x50, vt::Key::Down)
    };
    let mut out = Vec::new();
    for down in std::iter::repeat_n([true, false], n.unsigned_abs()).flatten() {
        let k = KeyInput {
            vk,
            scan,
            extended: true,
            down,
            repeat: 1,
            mods: Mods::default(),
            locks: vt::Locks::default(),
            text: "",
            uc: 0,
            cs: 0,
            key,
            us_base: None,
        };
        vt::encode_key(&k, m, &mut out);
    }
    out
}

/// Draws a notice over the bottom row of the pane whose grid is at `at`.
/// A passing hint shows only where no question or error waits, which
/// it would take the place of.
fn hint_fits(n: Option<&Notice>) -> bool {
    n.is_none_or(|n| n.ask == Ask::Nothing)
}

/// The bottom row of a pane, and whether it is dim: its notice, else for
/// a program that exited, how and what Enter and Esc do, which comes
/// back when a notice over it goes.
fn notice_line(n: Option<&Notice>, exit: Option<u32>) -> Option<(Cow<'_, str>, bool)> {
    match (n, exit) {
        (Some(n), _) => Some((Cow::Borrowed(n.text.as_str()), n.dim)),
        (None, Some(code)) => {
            let text = format!("{} \u{b7} Enter restart \u{b7} Esc close", exit_text(code));
            Some((Cow::Owned(text), false))
        }
        (None, None) => None,
    }
}

fn draw_notice(r: &mut Renderer, pal: &Palette, at: Rect, grid: (u16, u16), text: &str, dim: bool) {
    let (_, ch) = r.cell();
    let mut s = text_snapshot(&format!(" {text}"), grid.0, 1, pal);
    let bg = if dim { pal.bg } else { pal.selection_bg };
    for c in &mut s.cells {
        c.bg = bg;
        if dim {
            c.attrs |= vt::snapshot::attr::DIM;
        }
    }
    let banner = Palette { bg, ..*pal };
    let y = at.y + i32::from(grid.1.saturating_sub(1)) * ch as i32;
    r.snapshot(&s, &banner, at.x, y);
}

/// The title bar icon size, so Windows picks the hand-tuned small icon
/// rather than shrinking the big one.
fn small_icon_size() -> PhysicalSize<u32> {
    // SAFETY: reads a system metric; no pointers.
    let n = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16) as u32;
    PhysicalSize::new(n, n)
}

/// `g`, moved onto the primary monitor when no monitor shows enough of it.
fn on_screen(el: &ActiveEventLoop, g: Geometry) -> Geometry {
    let rect = |m: winit::monitor::MonitorHandle| Rect {
        x: m.position().x,
        y: m.position().y,
        w: m.size().width as i32,
        h: m.size().height as i32,
    };
    let monitors: Vec<Rect> = el.available_monitors().map(rect).collect();
    match el.primary_monitor().map(rect).or(monitors.first().copied()) {
        Some(primary) => session::on_screen(g, &monitors, primary),
        None => g,
    }
}

/// The tab and leaf index of pane `id` in `win`, by which output saved
/// before panes had keys is filed.
fn leaf_index(win: &layout::Window, id: PaneId) -> Option<(usize, usize)> {
    (win.tabs.iter().enumerate())
        .find_map(|(t, tab)| Some((t, tab.panes().iter().position(|&p| p == id)?)))
}

/// Why a pane failed to start in its folder and then again without one;
/// once when both say the same.
fn joined(first: String, then: String) -> String {
    if first == then {
        first
    } else {
        format!("{first}; then {then}")
    }
}

/// What to type into a restored pane's shell to bring back the Claude Code
/// session it was running, if anything. The id comes from a file on disk,
/// so only a well-formed one is ever typed.
fn resume_line(enabled: bool, claude: Option<&str>) -> Option<String> {
    let id = claude.filter(|id| enabled && crate::hook::is_session_id(id))?;
    Some(format!("claude --resume {id}\r"))
}

/// The last `n` lines of `text`, without blank lines at either end.
fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.trim_matches('\n').lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// The local time as `2026-10-02 14:32`.
fn local_stamp() -> String {
    // SAFETY: plain Win32 call with no arguments.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{}-{:02}-{:02} {:02}:{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute
    )
}

/// Where a new pane starts: `cwd` if it is still a directory, else the
/// user's profile folder.
fn start_dir(cwd: impl AsRef<Path>) -> Option<PathBuf> {
    let dir = cwd.as_ref();
    if !dir.as_os_str().is_empty() && dir.is_dir() {
        return Some(dir.to_path_buf());
    }
    std::env::var_os("USERPROFILE").map(PathBuf::from)
}

/// Splits the focused pane of the active tab, putting the new one on the
/// `dir` side; for [`App::add`].
fn split(dir: Dir) -> impl FnOnce(&mut layout::Window, PaneId) -> bool {
    move |win, id| {
        // Only the pane minimum matters, and `open` checks that against the
        // real window.
        let any = Rect {
            x: 0,
            y: 0,
            w: 1 << 16,
            h: 1 << 16,
        };
        let active = win.active;
        (win.tabs.get_mut(active)).is_some_and(|t| t.split(dir, id, any, (0, 0)))
    }
}

/// Puts pane `id` in a new tab after the others and shows that tab. It
/// goes by the folder of its focused pane until the user names it.
fn new_tab(win: &mut layout::Window, id: PaneId) -> bool {
    win.tabs.push(Tab::new(String::new(), id));
    win.active = win.tabs.len() - 1;
    true
}

/// Whether a pane's title names its session: it has Claude Code's mark,
/// and Claude Code is known to run there, as its hooks with the pane's
/// token or blitz's resume say. Any program can print the mark, and
/// would then go by a name it picked, perhaps one like another session's.
fn titled_by_claude(mark: Option<bool>, hooked: bool, session: Option<&str>) -> bool {
    mark.is_some() && (hooked || session.is_some())
}

/// What the sidebar calls a session, and the message under its name: the
/// name the user gave it, else for Claude Code the task its title names,
/// which Claude's /rename changes too (`claude` says the title has its
/// mark), else its program. A shell's title is often only its exe's path,
/// so that is never a name. The title shows as the message when there is
/// none, unless it is the name already.
fn label(
    named: Option<&str>,
    program: &str,
    title: &str,
    claude: bool,
    msg: &str,
) -> (String, String) {
    // Without the mark Claude Code puts in front.
    let task = claude_title(title).map_or(title, |c| c.1);
    let name = match named {
        Some(n) => n,
        None if claude && !task.is_empty() => task,
        None => program,
    };
    let msg = match msg {
        "" if task == name => "",
        "" => task,
        m => m,
    };
    (name.to_owned(), msg.to_owned())
}

/// Takes a fresh snapshot of `term` into `snap`. Returns false when that
/// found output rewrote the text under `sel`; see [`Selection::still`].
fn refresh(
    term: &mut vt::Terminal,
    snap: &mut Snapshot,
    pal: &Palette,
    sel: Option<&mut Selection>,
) -> bool {
    // Only a terminal with news can change the selected text.
    !term.snapshot(snap, pal) || sel.is_none_or(|s| s.still(term, pal))
}

/// The text of the selected cells from line `from` on, in reading order:
/// trailing blanks trimmed, rows joined by CRLF unless one wraps into the
/// next. A block takes the same columns of each row.
fn selection_text(term: &vt::Terminal, pal: &Palette, sel: &Selection, from: usize) -> String {
    let (a, b) = (sel.start, sel.end);
    let mut out = String::new();
    let mut cells = Vec::new();
    for n in a.0.max(from)..=b.0 {
        let Some(wraps) = term.line_cells(n, pal, &mut cells) else {
            break;
        };
        let (first, last) = match (sel.drag.block, n == a.0, n == b.0) {
            (true, ..) => (a.1, b.1),
            (false, first, last) => (
                if first { a.1 } else { 0 },
                if last { b.1 } else { u16::MAX },
            ),
        };
        let mut line = String::new();
        let text_end = push_cells(&mut line, &cells, first.into(), last.into());
        // A row that wraps runs on into the next one: no line break, and
        // only the empty cells at its end are dropped. A block's rows
        // always break.
        if n < b.0 && wraps && !sel.drag.block {
            line.truncate(text_end);
            out.push_str(&line);
        } else {
            out.push_str(line.trim_end());
            if n < b.0 {
                out.push_str("\r\n");
            }
        }
    }
    out
}

/// Adds the text of `cells[first..=last]` as the screen shows it. Returns
/// the length of `line` after the last cell that holds text.
fn push_cells(line: &mut String, cells: &[vt::RenderCell], first: usize, last: usize) -> usize {
    let mut text_end = line.len();
    for cell in cells.iter().take(last.saturating_add(1)).skip(first) {
        match cell.len {
            // The right half of a wide character.
            0 if cell.width == 0 => {}
            // A blank, or hidden text: a space for each column.
            0 => line.extend(std::iter::repeat_n(' ', usize::from(cell.width))),
            n => {
                let text = std::str::from_utf8(&cell.text[..usize::from(n)]).unwrap_or(" ");
                push_drawn(line, text, cell.width);
                text_end = line.len();
            }
        }
    }
    text_end
}

/// Adds a cell's text as the screen shows it, so a copy carries nothing
/// the user could not see. Characters that draw nothing (joiners,
/// variation selectors, tags, invisible format characters) are left out,
/// except a VS16 right after an emoji and a joiner right before one. So is
/// everything after the first character of a joined cluster that is not an
/// emoji sequence, since only that character is drawn. Fillers and other
/// invisible characters that take a cell become spaces.
fn push_drawn(line: &mut String, text: &str, width: u8) {
    use vt::width::{char_width, is_emoji, is_ignorable};
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return;
    };
    if matches!(first, '\u{A0}' | '\u{2800}') || is_ignorable(first) {
        line.extend(std::iter::repeat_n(' ', usize::from(width.max(1))));
        return;
    }
    line.push(first);
    let rest = chars.as_str();
    let shows = |c| char_width(c) > 0;
    let emoji = rest.chars().any(shows)
        && rest
            .chars()
            .all(|c| matches!(c, '\u{200D}' | '\u{FE0F}') || shows(c));
    if rest.contains('\u{200D}') && !emoji {
        return;
    }
    let mut prev = first;
    let mut rest = rest.chars().peekable();
    while let Some(c) = rest.next() {
        let keep = match c {
            '\u{FE0F}' => is_emoji(prev),
            '\u{200D}' => rest.peek().is_some_and(|&n| is_emoji(n)),
            c => !is_ignorable(c),
        };
        if keep {
            line.push(c);
        }
        prev = c;
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        if let Err(e) = self.start(el) {
            eprintln!("blitz: {e}");
            self.code = 1;
            el.exit();
        }
    }

    fn new_events(&mut self, _el: &ActiveEventLoop, cause: StartCause) {
        self.counters.wakeups += 1;
        if let StartCause::ResumeTimeReached { .. } = cause {
            let now = Instant::now();
            for v in &mut self.views {
                if v.notice
                    .as_ref()
                    .is_some_and(|n| n.until.is_some_and(|t| t <= now))
                {
                    v.notice = None;
                }
                // No prompt mark came: shell integration is off or failed.
                if let Some((line, _)) = v.resume.take_if(|r| r.1 <= now) {
                    v.pane.send(line);
                }
            }
            if self.mouse.scroll_at.is_some_and(|t| t <= now) {
                self.autoscroll();
            }
            // A synchronized update timed out, a notice expired, or a
            // working timer ticked.
            self.request_redraw();
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            // Closing ends every session, so a busy one asks first.
            WindowEvent::CloseRequested => {
                let busy: Vec<_> = self.views.iter().filter_map(View::busy).collect();
                match self.focus_id() {
                    Some(id) if !busy.is_empty() && !self.confirmed(id, &Ask::Quit) => {
                        let them = if busy.len() == 1 { "it" } else { "them" };
                        let text = format!(
                            "{}. Close the window again to end {them}",
                            busy_text(&busy, "")
                        );
                        self.ask(id, text, Ask::Quit);
                        // Closed from the taskbar: show the question.
                        if let Some(w) = &self.window
                            && w.is_minimized() == Some(true)
                        {
                            w.set_minimized(false);
                        }
                    }
                    _ => el.exit(),
                }
            }
            WindowEvent::RedrawRequested => {
                // Keys queued behind this paint go out before its vsync wait.
                self.drain_keys(el);
                self.redraw();
            }
            WindowEvent::Resized(_) => self.request_redraw(),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale = scale_factor;
                // The cursor's cell stays, but its pixels move.
                self.ime_at = None;
                self.reload_font();
            }
            WindowEvent::Focused(f) => {
                self.focused = f;
                // A key still held goes up in the other window, so its
                // release never comes back to clear `eaten`; so does a
                // button, ending a drag or a selection. A program that saw
                // the button go down sees it come up here, as it went where
                // its press did.
                if !f {
                    self.eaten = Eaten::default();
                    self.mouse.divider = None;
                    self.mouse.drag = None;
                    self.mouse.scroll_at = None;
                    let mods = mods_now();
                    for b in 0..3 {
                        if let Some(id) = route_button(&mut self.mouse.reported, b, false, None) {
                            self.mouse_report(id, MouseKind::Release, b as u8, mods);
                        }
                    }
                }
                // Ctrl may be let go while another window has the keys.
                self.set_hover(None);
                // The cursor is hollow while the window is in the background.
                self.request_redraw();
                if let Some(v) = self.current() {
                    tell_focus(v, f);
                }
                if f && let Some(id) = self.focus_id() {
                    self.attention(id, Ev::Attended);
                }
            }
            WindowEvent::Ime(Ime::Commit(mut text)) => {
                self.preedit.clear();
                // Like typed characters, committed text carries no controls
                // that could run or escape anything.
                text.retain(|c| !c.is_control());
                if !self.filter_text(&text) {
                    self.dismiss(None);
                    self.typed(text.into_bytes());
                }
            }
            WindowEvent::Ime(Ime::Preedit(text, _)) => {
                self.preedit = text;
                self.request_redraw();
            }
            WindowEvent::Ime(Ime::Disabled) => {
                self.preedit.clear();
                self.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => self.on_mouse_move(position),
            WindowEvent::MouseInput { state, button, .. } => {
                self.on_mouse_button(el, state, button);
            }
            WindowEvent::MouseWheel { delta, .. } => self.on_wheel(delta),
            // Windows switched between light and dark mode.
            WindowEvent::ThemeChanged(_) => {
                // winit has just set the frame to the system's mode.
                self.frame_theme();
                if self.picker.is_none() {
                    self.set_theme_from_config();
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Pane(id, note) => self.on_pane(el, id, note),
            UserEvent::Branch(id, dir, branch) => self.on_branch(id, dir, branch),
            UserEvent::Finish(code) => {
                self.code = code;
                if self.args.capture.is_some() {
                    self.capture_then_exit = true;
                    self.redraw();
                }
                el.exit();
            }
            UserEvent::Update(v, log) => self.offer_update(v, log),
            UserEvent::Checked(found) => {
                let (text, failed) = (crate::update::found(&found), found.is_err());
                if let Ok(Some(v)) = found {
                    self.offer_update(v, None);
                }
                match self.updating.take() {
                    Some(id) if failed => self.error(id, text),
                    Some(id) => self.set_notice(id, text, Some(Instant::now() + NOTICE), false),
                    None => {}
                }
            }
            UserEvent::Settings => match Config::reload() {
                Some(c) => self.apply_config(c),
                // A theme file changed, or config.toml is busy being saved.
                None if self.picker.is_none() => self.set_theme_from_config(),
                None => {}
            },
            UserEvent::Installed(Ok(())) => el.exit(),
            UserEvent::Installed(Err(e)) => {
                eprintln!("blitz: update: {e}");
                if let Some(id) = self.updating.take() {
                    self.error(id, format!("Update failed: {e}"));
                }
            }
            UserEvent::OpenHere(dir) => {
                // First, since a minimized window has no room for a pane.
                if let Some(w) = &self.window {
                    w.set_minimized(false);
                }
                // SAFETY: our own window; the launch that sent the folder
                // allowed this process to take the foreground.
                let _ = unsafe { SetForegroundWindow(HWND(self.hwnd as *mut c_void)) };
                self.add(Some(dir), new_tab);
            }
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.drain_keys(el);
        // Back at the screen, with a key or the mouse: the focused pane
        // is in view again.
        if self.away && self.game.is_none() && present(self.focused, idle_for()) {
            self.away = false;
            if let Some(id) = self.focus_id() {
                self.attention(id, Ev::Attended);
            }
        }
        self.save_session(false);
        let flow = match self.next_deadline() {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        };
        el.set_control_flow(flow);
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        // Closing the window, Alt+F4 and an update all keep the layout.
        self.save_session(true);
        if let Err(e) = self.counters.write_trace() {
            eprintln!("blitz: BLITZ_TRACE: {e}");
        }
    }
}

/// The GUI self-test: real key presses through `SendInput`, checked
/// against the screen of the focused pane.
///
/// - `sendinput CHORD`: press a chord such as `shift+enter` or `ctrl+c`.
/// - `type TEXT`: type TEXT key by key on the active layout.
/// - `expect [MS] REGEX`: wait until a screen row matches (3000 ms default).
/// - `waitfor MS TEXT`: wait until TEXT appears on the screen.
/// - `clip TEXT`: put TEXT on the clipboard; `\n` is a line break.
/// - `resize W H`: resize the window to W by H pixels.
/// - `wheel N X Y`: turn the wheel N notches (up is positive) over the
///   client pixel X, Y.
/// - `click X Y`: click the left button at the client pixel X, Y.
/// - `snap`: print the screen.
/// - `sleep MS`, `note TEXT`.
mod selftest {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::HWND;
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
        KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC,
        MOUSE_EVENT_FLAGS, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT,
        MapVirtualKeyW, SendInput, VIRTUAL_KEY, VkKeyScanW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, SWP_NOMOVE, SWP_NOZORDER, SetCursorPos, SetForegroundWindow,
        SetWindowPos,
    };

    use crate::debug::Regex;
    use crate::pane::lock;

    /// The terminal of the focused pane, swapped when focus moves.
    pub type Focus = Mutex<Arc<Mutex<vt::Terminal>>>;

    pub fn screen(term: &Focus) -> String {
        let t = lock(term).clone();
        lock(&t).screen_text()
    }

    pub fn run(script: &str, hwnd: isize, term: &Focus) -> Result<(), String> {
        for (n, line) in script.lines().enumerate() {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            println!("selftest: {line}");
            step(line, hwnd, term).map_err(|e| format!("line {}: {e}", n + 1))?;
        }
        Ok(())
    }

    fn num<T: std::str::FromStr>(s: &str) -> Result<T, String> {
        s.trim().parse().map_err(|_| format!("not a number: {s:?}"))
    }

    fn step(line: &str, hwnd: isize, term: &Focus) -> Result<(), String> {
        let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
        let wait = |ms: u64, done: &dyn Fn(&str) -> bool| {
            let end = Instant::now() + Duration::from_millis(ms);
            while Instant::now() < end {
                if done(&screen(term)) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        };
        match cmd {
            "sendinput" => send(hwnd, &chord(rest)?)?,
            "type" => {
                for c in rest.chars() {
                    send(hwnd, &char_keys(c))?;
                    std::thread::sleep(Duration::from_millis(30));
                }
            }
            "expect" => {
                let (ms, pattern) = match rest.split_once(' ') {
                    Some((ms, p)) if ms.bytes().all(|b| b.is_ascii_digit()) => (num(ms)?, p),
                    _ => (3000, rest),
                };
                let re = Regex::new(pattern)?;
                if !wait(ms, &|s| re.matches_a_row(s)) {
                    return Err(format!("no row matched {pattern:?} within {ms} ms"));
                }
            }
            "waitfor" => {
                let (ms, text) = rest.split_once(' ').ok_or("waitfor MS TEXT")?;
                let ms = num(ms)?;
                if !wait(ms, &|s| s.contains(text)) {
                    return Err(format!("{text:?} did not show up in {ms} ms"));
                }
            }
            "clip" => {
                if !crate::clipboard::set_text(None, &rest.replace(r"\n", "\r\n")) {
                    return Err("cannot set the clipboard".into());
                }
            }
            "wheel" => {
                let v: Vec<&str> = rest.split_whitespace().collect();
                let [n, x, y] = v[..] else {
                    return Err("wheel N X Y".into());
                };
                let notches: i32 = num(n)?;
                mouse(
                    hwnd,
                    num(x)?,
                    num(y)?,
                    &[(MOUSEEVENTF_WHEEL, notches * 120)],
                )?;
            }
            "click" => {
                let (x, y) = rest.trim().split_once(' ').ok_or("click X Y")?;
                let press = [(MOUSEEVENTF_LEFTDOWN, 0), (MOUSEEVENTF_LEFTUP, 0)];
                mouse(hwnd, num(x)?, num(y)?, &press)?;
            }
            "resize" => {
                let (w, h) = rest.trim().split_once(' ').ok_or("resize W H")?;
                // SAFETY: a live window; the call waits for the UI thread.
                unsafe {
                    SetWindowPos(
                        HWND(hwnd as *mut _),
                        None,
                        0,
                        0,
                        num(w)?,
                        num(h)?,
                        SWP_NOMOVE | SWP_NOZORDER,
                    )
                }
                .map_err(|e| format!("resize: {e}"))?;
            }
            "snap" => println!("--- screen ---\n{}", screen(term)),
            "sleep" => std::thread::sleep(Duration::from_millis(num(rest)?)),
            "note" => {}
            _ => return Err(format!("unknown command {cmd:?}")),
        }
        Ok(())
    }

    /// One key transition: a virtual key, or a UTF-16 unit when `vk` is 0.
    #[derive(Clone, Copy)]
    struct Stroke {
        vk: u16,
        unit: u16,
        up: bool,
    }

    fn press(mods: &[u16], vk: u16) -> Vec<Stroke> {
        let s = |vk, up| Stroke { vk, unit: 0, up };
        let mut out: Vec<Stroke> = mods.iter().map(|&m| s(m, false)).collect();
        out.extend([s(vk, false), s(vk, true)]);
        out.extend(mods.iter().rev().map(|&m| s(m, true)));
        out
    }

    fn chord(spec: &str) -> Result<Vec<Stroke>, String> {
        let spec = spec.trim().to_ascii_lowercase();
        let (mods, key) = spec.rsplit_once('+').unwrap_or(("", &spec));
        let mut held = Vec::new();
        for m in mods.split('+').filter(|m| !m.is_empty()) {
            held.push(match m {
                "shift" => 0xa0,
                "ctrl" => 0xa2,
                "alt" => 0xa4,
                _ => return Err(format!("unknown modifier {m:?}")),
            });
        }
        let vk = match key {
            "enter" => 0x0d,
            "tab" => 0x09,
            "esc" => 0x1b,
            "bs" | "backspace" => 0x08,
            "space" => 0x20,
            "pgup" => 0x21,
            "pgdn" => 0x22,
            "end" => 0x23,
            "home" => 0x24,
            "left" => 0x25,
            "up" => 0x26,
            "right" => 0x27,
            "down" => 0x28,
            "ins" => 0x2d,
            "del" => 0x2e,
            _ => {
                let mut chars = key.chars();
                match (chars.next(), chars.next()) {
                    // SAFETY: a table lookup.
                    (Some(c), None) => (unsafe { VkKeyScanW(c as u16) } & 0xff) as u16,
                    _ => return Err(format!("unknown key {key:?}")),
                }
            }
        };
        Ok(press(&held, vk))
    }

    /// The keys that type `c` on the active layout, or a Unicode packet
    /// when no key does.
    fn char_keys(c: char) -> Vec<Stroke> {
        // SAFETY: a table lookup.
        let scan = unsafe { VkKeyScanW(c as u16) };
        if c.len_utf16() == 1 && scan != -1 && (scan >> 8) & !1 == 0 {
            let mods: &[u16] = if (scan >> 8) & 1 != 0 { &[0xa0] } else { &[] };
            return press(mods, (scan & 0xff) as u16);
        }
        let mut out = Vec::new();
        for &unit in c.encode_utf16(&mut [0; 2]).iter() {
            out.push(Stroke {
                vk: 0,
                unit,
                up: false,
            });
            out.push(Stroke {
                vk: 0,
                unit,
                up: true,
            });
        }
        out
    }

    /// Brings blitz to the front, or fails: input sent anywhere else would
    /// go to another program.
    fn to_front(hwnd: isize) -> Result<(), String> {
        // SAFETY: plain window queries.
        unsafe {
            if GetForegroundWindow().0 as isize != hwnd {
                let _ = SetForegroundWindow(HWND(hwnd as *mut _));
                std::thread::sleep(Duration::from_millis(200));
                if GetForegroundWindow().0 as isize != hwnd {
                    return Err("blitz is not the foreground window".into());
                }
            }
        }
        Ok(())
    }

    /// Moves the pointer to the client pixel (`x`, `y`) and sends mouse
    /// events there, each a flag and its data.
    fn mouse(
        hwnd: isize,
        x: i32,
        y: i32,
        events: &[(MOUSE_EVENT_FLAGS, i32)],
    ) -> Result<(), String> {
        to_front(hwnd)?;
        let mut pt = POINT { x, y };
        let inputs: Vec<INPUT> = events
            .iter()
            .map(|&(flags, data)| INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        mouseData: data as u32,
                        dwFlags: flags,
                        ..Default::default()
                    },
                },
            })
            .collect();
        // SAFETY: a live window, a valid point and valid mouse inputs.
        unsafe {
            let _ = ClientToScreen(HWND(hwnd as *mut _), &mut pt);
            SetCursorPos(pt.x, pt.y).map_err(|e| format!("cursor: {e}"))?;
            // Let the window see the pointer arrive first.
            std::thread::sleep(Duration::from_millis(50));
            if SendInput(&inputs, size_of::<INPUT>() as i32) as usize != inputs.len() {
                return Err("SendInput failed".into());
            }
        }
        Ok(())
    }

    /// Sends key strokes to blitz.
    fn send(hwnd: isize, strokes: &[Stroke]) -> Result<(), String> {
        to_front(hwnd)?;
        let inputs: Vec<INPUT> = strokes
            .iter()
            .map(|s| {
                let mut flags = KEYBD_EVENT_FLAGS(0);
                if s.up {
                    flags |= KEYEVENTF_KEYUP;
                }
                let scan = if s.vk == 0 {
                    flags |= KEYEVENTF_UNICODE;
                    s.unit
                } else {
                    if matches!(s.vk, 0x21..=0x28 | 0x2d | 0x2e) {
                        flags |= KEYEVENTF_EXTENDEDKEY;
                    }
                    // SAFETY: a table lookup.
                    unsafe { MapVirtualKeyW(u32::from(s.vk), MAPVK_VK_TO_VSC) as u16 }
                };
                INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VIRTUAL_KEY(s.vk),
                            wScan: scan,
                            dwFlags: flags,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                }
            })
            .collect();
        // SAFETY: a valid slice of keyboard inputs.
        let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
        if sent as usize != inputs.len() {
            return Err(format!("SendInput sent {sent} of {} inputs", inputs.len()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `cols` x `rows` terminal fed `bytes`.
    fn fed(cols: u16, rows: u16, bytes: &str) -> vt::Terminal {
        let mut t = vt::Terminal::new(vt::Options {
            cols,
            rows,
            ..vt::Options::default()
        });
        t.feed(bytes.as_bytes());
        t
    }

    /// A selection dragged from `a` to `b` after `unit` clicks.
    fn drag(t: &vt::Terminal, a: Pos, b: Pos, unit: u8, block: bool) -> Selection {
        let pal = crate::theme::dark();
        Selection::new(t, &pal, Drag::new(t, &pal, a, unit, block), b)
    }

    /// A selection dragged from `a` to `b`.
    fn select(t: &vt::Terminal, a: Pos, b: Pos) -> Selection {
        drag(t, a, b, 1, false)
    }

    /// What a copy of the selection dragged from `a` to `b` takes.
    fn copy(t: &vt::Terminal, a: Pos, b: Pos) -> String {
        selection_text(t, &crate::theme::dark(), &select(t, a, b), 0)
    }

    #[test]
    fn app_selection_text_reads_cells_in_order() {
        let t = fed(4, 3, "ab  \r\nc\u{4e2d}d\r\nxyz");
        // Dragged from the end; spans three rows.
        assert_eq!(copy(&t, (2, 1), (0, 1)), "b\r\nc\u{4e2d}d\r\nxy");
        assert_eq!(copy(&t, (1, 0), (1, 3)), "c\u{4e2d}d");
        assert_eq!(copy(&t, (0, 2), (0, 3)), "");
    }

    #[test]
    fn app_selection_leaves_out_hidden_text() {
        let t = fed(
            30,
            2,
            "git status\x1b[8m; iwr x|iex\x1b[28m!\r\n\x1b[38;2;19;20;23mcalc\x1b[0m",
        );
        assert_eq!(copy(&t, (0, 0), (1, 29)), "git status           !\r\n");
        let t = fed(4, 1, "\x1b[8m\u{4e2d}\x1b[0mx");
        assert_eq!(copy(&t, (0, 0), (0, 3)), "  x");
    }

    #[test]
    fn app_selection_joins_wrapped_rows() {
        let two = |t| copy(&t, (0, 0), (1, 3));
        assert_eq!(two(fed(4, 3, "ab  cd")), "ab  cd");
        assert_eq!(two(fed(4, 3, "ab\r\ncd")), "ab\r\ncd");
        // A wide character that did not fit leaves an empty cell behind.
        assert_eq!(two(fed(3, 3, "ab\u{4e2d}")), "ab\u{4e2d}");
    }

    #[test]
    fn app_selection_copies_only_what_is_drawn() {
        let drawn = |bytes: &str| copy(&fed(40, 1, bytes), (0, 0), (0, 39));
        assert_eq!(drawn("ls\u{E0069}\u{E0067}\u{E006E}x"), "lsx", "tags");
        assert_eq!(drawn("a\u{E0100}\u{FE00}b"), "ab", "variation selectors");
        assert_eq!(
            drawn("a\u{200D}\u{301}\u{302}b"),
            "ab",
            "marks after a joiner"
        );
        assert_eq!(drawn("a\u{200D}b"), "ab", "a lone joiner");
        assert_eq!(drawn("x\u{3164}y\u{2800}z\u{A0}w"), "x  y z w", "fillers");
        assert_eq!(drawn("x\u{AD}y"), "x y", "soft hyphen");
        for s in [
            "a\u{200C}\u{200C}\u{200C}b",
            "a\u{34F}b",
            "a\u{180B}\u{180F}b",
            "a\u{17B4}\u{17B5}b",
            "a\u{FE0E}\u{FE0F}b",
            "a\u{2060}\u{FEFF}b",
        ] {
            assert_eq!(drawn(s), "ab", "{s:?}");
        }
        assert_eq!(drawn("\u{2764}\u{FE0F}\u{FE0F}"), "\u{2764}\u{FE0F}");
        // Text that draws keeps everything.
        for s in [
            "e\u{301}",
            "\u{2764}\u{FE0F}",
            "1\u{FE0F}\u{20E3}",
            "\u{1F44D}\u{1F3FD}",
            "\u{1F468}\u{200D}\u{1F469}",
            "\u{4e2d}",
        ] {
            assert_eq!(drawn(s), s);
        }
    }

    #[test]
    fn app_copy_reaches_into_scrollback() {
        let t = fed(4, 2, "one\r\ntwo\r\nthree\r\nfour");
        assert_eq!(t.screen_top(), 3);
        assert_eq!(copy(&t, (1, 1), (4, 1)), "wo\r\nthree\r\nfo");
    }

    #[test]
    fn app_selection_stays_on_its_text_until_output_rewrites_it() {
        let pal = crate::theme::dark();
        let mut t = fed(10, 3, "a\r\nb\r\nc");
        let mut s = Snapshot::default();
        let mut sel = select(&t, (1, 0), (1, 9));
        assert!(refresh(&mut t, &mut s, &pal, None));
        assert!(refresh(&mut t, &mut s, &pal, Some(&mut sel)), "nothing new");
        t.feed(b"\x1b[1;5Hx");
        assert!(
            refresh(&mut t, &mut s, &pal, Some(&mut sel)),
            "another row changed"
        );
        t.feed(b"\x1b[3;1H\r\nd");
        assert!(refresh(&mut t, &mut s, &pal, Some(&mut sel)), "scrolled");
        assert_eq!(selection_text(&t, &pal, &sel, 0), "b");
        t.feed(b"\x1b[1;1Hz");
        assert!(!refresh(&mut t, &mut s, &pal, Some(&mut sel)), "rewritten");
        let mut sel = select(&t, (2, 0), (2, 9));
        t.feed(b"\x1b[2;1Hq\x1b[3;1H\r\n\r\n");
        assert!(
            !refresh(&mut t, &mut s, &pal, Some(&mut sel)),
            "rewritten, then scrolled out of the screen"
        );
    }

    #[test]
    fn app_selection_ends_when_its_lines_are_gone() {
        let pal = crate::theme::dark();
        let mut t = vt::Terminal::new(vt::Options {
            cols: 10,
            rows: 2,
            scrollback_lines: 2,
            ..vt::Options::default()
        });
        t.feed(b"a\r\nb");
        let mut s = Snapshot::default();
        let mut sel = select(&t, (0, 0), (0, 0));
        t.feed(b"\r\n1\r\n2");
        assert!(
            refresh(&mut t, &mut s, &pal, Some(&mut sel)),
            "in scrollback"
        );
        t.feed(b"\r\n3");
        assert!(!refresh(&mut t, &mut s, &pal, Some(&mut sel)), "dropped");
        let mut sel = select(&t, (3, 0), (3, 0));
        t.resize(8, 2);
        assert!(!refresh(&mut t, &mut s, &pal, Some(&mut sel)), "rewrapped");
    }

    #[test]
    fn app_selection_shows_the_part_in_view() {
        let size = (10, 3);
        let view = |a, b| in_view(a, b, false, 5, size);
        assert_eq!(view((5, 2), (6, 4)), Some(((2, 0), (4, 1))));
        assert_eq!(
            view((1, 2), (9, 4)),
            Some(((0, 0), (9, 2))),
            "past both ends"
        );
        assert_eq!(view((1, 2), (4, 4)), None, "above");
        assert_eq!(view((8, 0), (9, 4)), None, "below");
        let block = in_view((1, 2), (9, 4), true, 5, size);
        assert_eq!(block, Some(((2, 0), (4, 2))), "a block keeps its columns");
    }

    #[test]
    fn app_double_click_takes_a_word_or_a_path() {
        let t = fed(12, 3, "see src/foo.rs:42, (x)  y");
        let pal = crate::theme::dark();
        let word = |at| Drag::new(&t, &pal, at, 2, false).anchor;
        assert_eq!(word((0, 8)), ((0, 4), (1, 4)), "a path, across the wrap");
        assert_eq!(word((0, 1)), ((0, 0), (0, 2)));
        assert_eq!(word((1, 7)), ((1, 7), (1, 7)), "punctuation alone");
        assert_eq!(word((1, 10)), ((1, 10), (1, 11)), "a run of blanks");
        let by_words = drag(&t, (0, 1), (1, 1), 2, false);
        assert_eq!(selection_text(&t, &pal, &by_words, 0), "see src/foo.rs:42");
        let t = fed(10, 1, "\u{4e2d}\u{6587} x");
        let word = |at| Drag::new(&t, &pal, at, 2, false).anchor;
        assert_eq!(word((0, 1)), ((0, 0), (0, 3)), "wide characters");
    }

    #[test]
    fn app_triple_click_takes_the_logical_line() {
        let t = fed(4, 4, "abcdef\r\ngh");
        let pal = crate::theme::dark();
        let line = |at| Drag::new(&t, &pal, at, 3, false).anchor;
        assert_eq!(line((1, 0)), ((0, 0), (1, u16::MAX)));
        assert_eq!(line((2, 1)), ((2, 0), (2, u16::MAX)));
        let by_lines = drag(&t, (0, 2), (2, 0), 3, false);
        assert_eq!(selection_text(&t, &pal, &by_lines, 0), "abcdef\r\ngh");
    }

    #[test]
    fn app_block_copy_takes_the_same_columns_of_each_row() {
        let pal = crate::theme::dark();
        let t = fed(6, 3, "abcdef\r\ngh  \r\nijklmn");
        let block = drag(&t, (0, 4), (2, 1), 1, true);
        assert_eq!((block.start, block.end), ((0, 1), (2, 4)));
        assert_eq!(selection_text(&t, &pal, &block, 0), "bcde\r\nh\r\njklm");
        // Rows a wrap joined stay apart.
        let t = fed(3, 2, "abcdef");
        let block = drag(&t, (0, 0), (1, 1), 1, true);
        assert_eq!(selection_text(&t, &pal, &block, 0), "ab\r\nde");
    }

    #[test]
    fn app_links_map_back_to_their_cells() {
        let t = fed(10, 3, "go https://e.com/abc now");
        let l = Logical::new(&t, &crate::theme::dark(), 1);
        assert_eq!(l.index((1, 4)).map(|i| l.cells[i].0), Some(14));
        let (range, found) = crate::links::scan(&l.text).remove(0);
        assert_eq!(found, Link::Url("https://e.com/abc".into()));
        assert_eq!(l.span(range), ((0, 3), (1, 9)), "across the wrap");
    }

    #[test]
    fn app_wheel_on_the_alternate_screen_sends_arrow_keys() {
        let legacy = InputModes::default();
        assert_eq!(wheel_keys(3, &legacy), b"\x1b[A\x1b[A\x1b[A");
        assert_eq!(wheel_keys(-1, &legacy), b"\x1b[B");
        let app = InputModes {
            decckm: true,
            ..legacy
        };
        assert_eq!(wheel_keys(2, &app), b"\x1bOA\x1bOA", "DECCKM");
        // Kitty flags 1 and 2: releases are reported too.
        let kitty = InputModes { kitty: 3, ..legacy };
        assert_eq!(wheel_keys(1, &kitty), b"\x1b[A\x1b[1;1:3A");
    }

    #[test]
    fn app_clicks_count_on_the_same_cell_in_time() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let within = ms(500);
        assert_eq!(clicks(None, (3, 1), t0, within), 1);
        let last = Some((t0, (3, 1), 1));
        assert_eq!(clicks(last, (3, 1), t0 + ms(400), within), 2);
        assert_eq!(
            clicks(last, (3, 2), t0 + ms(400), within),
            1,
            "another cell"
        );
        assert_eq!(clicks(last, (3, 1), t0 + ms(600), within), 1, "too late");
        let third = Some((t0, (3, 1), 3));
        assert_eq!(clicks(third, (3, 1), t0, within), 1, "after a triple click");
    }

    #[test]
    fn app_selection_takes_whole_wide_characters() {
        let mut t = vt::Terminal::new(vt::Options {
            cols: 4,
            rows: 1,
            ..vt::Options::default()
        });
        t.feed("a\u{4e2d}b".as_bytes());
        let ends = |a, b| {
            let s = select(&t, a, b);
            (s.start, s.end)
        };
        assert_eq!(copy(&t, (0, 0), (0, 1)), "a\u{4e2d}");
        // Starting on the right half takes the whole character, which is
        // also what is highlighted.
        assert_eq!(copy(&t, (0, 2), (0, 3)), "\u{4e2d}b");
        assert_eq!(ends((0, 2), (0, 3)), ((0, 1), (0, 3)));
        assert_eq!(copy(&t, (0, 2), (0, 2)), "\u{4e2d}");
        assert_eq!(ends((0, 2), (0, 2)), ((0, 1), (0, 2)));
        // Ending on the left half takes the right half too.
        assert_eq!(ends((0, 1), (0, 0)), ((0, 0), (0, 2)));
        assert_eq!(ends((0, 3), (0, 3)), ((0, 3), (0, 3)));
    }

    #[test]
    fn app_selection_text_past_the_last_column_or_not_utf8() {
        let mut t = vt::Terminal::new(vt::Options {
            cols: 2,
            rows: 2,
            ..vt::Options::default()
        });
        t.feed(b"ab\r\ncd");
        assert_eq!(copy(&t, (0, 0), (1, 9)), "ab\r\ncd", "past the last column");
        // Text that is not UTF-8 copies as a space.
        let mut cells = text_snapshot("ab", 2, 1, &crate::theme::dark()).cells;
        cells[0].text[0] = 0xff;
        let mut line = String::new();
        push_cells(&mut line, &cells, 0, usize::MAX);
        assert_eq!(line, " b");
    }

    #[test]
    fn app_mouse_release_goes_where_its_press_went() {
        let (a, b) = (Some(PaneId(1)), Some(PaneId(2)));
        let mut reported = [None; 3];
        // Pressed in a program's pane, then Shift held at release: the
        // program still gets the release.
        assert_eq!(route_button(&mut reported, 0, true, a), a);
        assert_eq!(route_button(&mut reported, 0, false, None), a);
        assert_eq!(reported, [None; 3]);
        // Focus moved between press and release.
        assert_eq!(route_button(&mut reported, 2, true, a), a);
        assert_eq!(route_button(&mut reported, 2, false, b), a);
        // A press with Shift, or with no mouse mode, is the window's, and
        // so is its release, even if the mode turned on meanwhile.
        assert_eq!(route_button(&mut reported, 0, true, None), None);
        assert_eq!(route_button(&mut reported, 0, false, b), None);
        // Each button keeps its own pane.
        route_button(&mut reported, 0, true, a);
        route_button(&mut reported, 1, true, b);
        assert_eq!(route_button(&mut reported, 1, false, None), b);
        assert_eq!(route_button(&mut reported, 0, false, None), a);
        // A second release has nowhere to go.
        assert_eq!(route_button(&mut reported, 0, false, a), None);
    }

    #[test]
    fn app_layout_saves_wait_out_drags_and_bursts() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let mut due = None;
        assert!(!save_now(false, false, at(0), &mut due), "nothing changed");
        assert_eq!(due, None);
        // A change waits, however often it is seen again.
        assert!(!save_now(true, false, at(0), &mut due));
        assert!(!save_now(true, false, at(100), &mut due));
        assert!(!save_now(true, false, at(499), &mut due));
        assert!(save_now(true, false, at(500), &mut due));
        assert_eq!(due, None);
        // A change undone before it is saved is forgotten.
        assert!(!save_now(true, false, at(600), &mut due));
        assert!(!save_now(false, false, at(700), &mut due));
        assert!(!save_now(true, false, at(1200), &mut due));
        assert_eq!(due, Some(at(1700)));
        // Nothing is written during a drag, however long; the end of the
        // drag writes what was due.
        let mut due = None;
        for ms in [0, 100, 600, 5000] {
            assert!(!save_now(true, true, at(ms), &mut due));
        }
        assert!(!save_now(true, false, at(5001), &mut due));
        assert!(save_now(true, false, at(5501), &mut due));
        // A save already due when a drag starts waits for it too, leaving
        // no deadline in the past for the event loop to spin on.
        let mut due = Some(at(0));
        assert!(!save_now(true, true, at(600), &mut due));
        assert_eq!(due, None);
        assert!(!save_now(true, false, at(700), &mut due));
        assert!(save_now(true, false, at(1200), &mut due));
    }

    fn input(vk: u16, down: bool, key: vt::Key, text: &'static str) -> KeyInput<'static> {
        KeyInput {
            vk,
            scan: 0,
            extended: false,
            down,
            repeat: 1,
            mods: Mods::default(),
            locks: vt::Locks::default(),
            text,
            uc: 0,
            cs: 0,
            key,
            us_base: None,
        }
    }

    /// The queued input: `+vk` and `-vk` for keys, text as itself.
    fn queued(k: &mut Keys) -> Vec<String> {
        (k.queue.drain(..))
            .map(|i| match i {
                Input::Key(k, _, _) => format!("{}{:x}", if k.down { '+' } else { '-' }, k.vk),
                Input::Text(t) => t,
            })
            .collect()
    }

    #[test]
    fn app_alt_and_keypad_digits_type_by_code() {
        let mut k = Keys::default();
        let alt = |down| input(0x12, down, vt::Key::Alt, "");
        let pad = |vk: u16, down| input(vk, down, vt::Key::Char('1'), "1");
        k.key(&alt(true), false, false);
        // Alt+0233: the digits are not keys for the program, and their
        // releases were handled with them.
        for vk in [0x60, 0x62, 0x63, 0x63] {
            k.key(&pad(vk, true), true, false);
            assert!(k.skipped(vk, false));
        }
        k.key(&alt(false), false, false);
        k.unit(0xe9);
        // The next key brings its own text, so a stray WM_CHAR is dropped.
        k.key(&input(0x41, true, vt::Key::Char('a'), "a"), false, false);
        k.unit(u16::from(b'a'));
        assert!(!k.skipped(0x41, false));
        k.key(&input(0x41, false, vt::Key::Char('a'), "a"), false, false);
        assert_eq!(queued(&mut k), ["+12", "-12", "\u{e9}", "+41", "-41"]);
        // Without Alt, keypad digits are keys.
        k.key(&pad(0x61, true), false, false);
        assert_eq!(queued(&mut k), ["+61"]);
        // Typed fast, the next digit goes down before the last comes up;
        // neither release reaches the program.
        k.key(&alt(true), false, false);
        k.key(&pad(0x60, true), true, false);
        k.key(&pad(0x62, true), true, false);
        assert!(k.skipped(0x60, false), "the first digit");
        assert!(k.skipped(0x62, false));
        k.key(&alt(false), false, false);
        assert_eq!(queued(&mut k), ["+12", "-12"]);
    }

    #[test]
    fn app_text_from_wm_char_joins_surrogates_and_drops_controls() {
        let mut k = Keys::default();
        k.key(&input(VK_PACKET, true, vt::Key::Other, ""), false, false);
        k.unit(0xd83d);
        assert!(queued(&mut k).is_empty(), "half a pair waits");
        k.unit(0xde00);
        k.unit(0x03);
        k.unit(0x1b);
        // A high surrogate with no low one is dropped.
        k.unit(0xd83d);
        k.unit(u16::from(b'x'));
        assert_eq!(queued(&mut k), ["\u{1F600}", "x"]);
        assert!(k.skipped(VK_PACKET, false));
        assert!(!k.skipped(VK_PACKET, false), "only once");
    }

    #[test]
    fn app_picker_keys_move_and_filter() {
        let names = ["Alpha", "Beta", "Gamma", "Delta", "Epsilon"];
        let mut p = Picker {
            themes: (names.iter()).map(|n| crate::theme::parse(n, "")).collect(),
            filter: String::new(),
            sel: 0,
        };
        let key = |vk, text| input(vk, true, vt::Key::Other, text);
        assert!(p.key(&key(VK_UP, "")) && p.sel == 0, "stays on the list");
        assert!(p.key(&key(VK_DOWN, "")) && p.sel == 1);
        assert!(
            p.key(&key(VK_NEXT, "")) && p.sel == 4,
            "a page stops at the end"
        );
        assert!(p.key(&key(VK_DOWN, "")) && p.sel == 4);
        assert!(p.key(&key(VK_PRIOR, "")) && p.sel == 0);
        // Typing narrows the list, case-insensitively, from its top.
        p.sel = 3;
        assert!(p.key(&key(0x54, "T")));
        assert_eq!((p.filter.as_str(), p.sel, p.matches().len()), ("T", 0, 2));
        assert!(p.key(&key(0x41, "A")));
        let shown: Vec<&str> = p.matches().iter().map(|t| t.name.as_str()).collect();
        assert_eq!(shown, ["Beta", "Delta"]);
        assert!(p.key(&key(VK_DOWN, "")) && p.sel == 1);
        assert!(p.key(&key(VK_BACK, "")));
        assert_eq!((p.filter.as_str(), p.sel), ("T", 0));
        assert!(p.key(&key(VK_BACK, "")));
        assert!(!p.key(&key(VK_BACK, "")), "nothing left to delete");
        // Nothing matches: the highlight has nowhere to go.
        p.filter = "zzz".into();
        assert!(p.key(&key(VK_DOWN, "")) && p.sel == 0);
        // Ctrl or Alt with a letter is a chord, not text; with both, the
        // layout's AltGr character is text.
        p.filter.clear();
        let mut ctrl = key(0x41, "a");
        ctrl.mods.lctrl = true;
        assert!(!p.key(&ctrl));
        let mut alt = key(0x41, "a");
        alt.mods.lalt = true;
        assert!(!p.key(&alt));
        let mut altgr = key(0x51, "@");
        (altgr.mods.lctrl, altgr.mods.ralt, altgr.uc) = (true, true, u16::from(b'@'));
        assert!(p.key(&altgr) && p.filter == "@");
        let mut ctrl_alt = key(0x51, "q");
        (ctrl_alt.mods.lctrl, ctrl_alt.mods.lalt) = (true, true);
        assert!(!p.key(&ctrl_alt), "Ctrl+Alt with no character");
        assert!(!p.key(&key(0x70, "")), "F1");
    }

    #[test]
    fn app_keys_kept_for_a_shortcut_keep_their_release() {
        let mut e = Eaten::default();
        // Shift and A typed into the theme picker, released in any order.
        e.press(0x10);
        e.press(0x41);
        e.press(0x41);
        assert!(e.release(0x10));
        assert!(e.release(0x41));
        assert!(!e.release(0x41), "once");
        // Ctrl was down before the picker opened: its release goes on.
        assert!(!e.release(0x11));
    }

    #[test]
    fn app_find_keeps_its_place_as_output_goes_on() {
        let mut t = vt::Terminal::new(vt::Options {
            cols: 20,
            rows: 3,
            scrollback_lines: 100,
            ..vt::Options::default()
        });
        for i in 0..10 {
            t.feed(format!("match {i}\r\n").as_bytes());
        }
        let mut f = Find::new(PaneId(1));
        f.query = "match".into();
        f.search(&t, 3);
        // The nearest match above the bottom of the view.
        assert_eq!((f.found.len(), f.cur), (10, Some(9)));
        f.step(-1);
        assert_eq!(f.cur, Some(8));
        // Output scrolls the matches up; the current one stays on its line.
        t.feed(b"match 10\r\nmatch 11\r\n");
        f.search(&t, 3);
        assert_eq!(f.found.len(), 12);
        assert_eq!(f.cur.map(|i| f.found[i].start), Some((8, 0)));
        // Up from the oldest goes round to the newest.
        f.cur = Some(0);
        f.step(-1);
        assert_eq!(f.cur, Some(11));
        // A match out of view comes into the middle of it.
        assert!(reveal(&mut t, f.found[2], 3));
        assert_eq!(t.view_top(), 1);
        assert!(!reveal(&mut t, f.found[2], 3), "already in view");

        f.query = "zzz".into();
        f.search(&t, 3);
        f.step(1);
        assert_eq!((f.found.len(), f.cur), (0, None));
    }

    #[test]
    fn app_new_panes_start_in_the_focused_directory() {
        let here = std::env::temp_dir();
        assert_eq!(start_dir(here.display().to_string()), Some(here.clone()));
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
        let gone = here.join(format!("blitz-gone-{}", std::process::id()));
        assert_eq!(start_dir(&gone), home);
        // A file is not a folder to start in.
        let file = here.join(format!("blitz-file-{}", std::process::id()));
        std::fs::write(&file, "").expect("write");
        let got = start_dir(&file);
        let _ = std::fs::remove_file(&file);
        assert_eq!(got, home);
        assert_eq!(start_dir(""), home);
    }

    #[test]
    fn app_only_claude_code_names_a_session_by_its_title() {
        let id = Some("3f2a0c1e-0000-4000-8000-00000000abcd");
        assert!(titled_by_claude(Some(true), true, None));
        assert!(titled_by_claude(Some(false), false, id));
        assert!(!titled_by_claude(Some(false), false, None), "any program");
        assert!(!titled_by_claude(None, true, id), "no mark");
    }

    #[test]
    fn app_claude_sessions_go_by_their_task() {
        let pwsh = r"C:\Program Files\PowerShell\7\pwsh.exe";
        let l = |title, claude, msg| label(None, "pwsh", title, claude, msg);
        let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        assert_eq!(l(pwsh, false, ""), s("pwsh", pwsh));
        assert_eq!(
            l("\u{2733} Fix the login", true, ""),
            s("Fix the login", "")
        );
        assert_eq!(
            l("\u{25D0} Fix the login", true, "Done."),
            s("Fix the login", "Done.")
        );
        assert_eq!(l("\u{2733} ", true, ""), s("pwsh", ""));
        // Claude Code has exited, but its title has not changed yet.
        assert_eq!(
            l("\u{2733} Fix the login", false, ""),
            s("pwsh", "Fix the login")
        );
        // A name the user gave wins, and the task goes under it.
        let named = label(Some("auth"), "pwsh", "\u{2733} Fix", true, "");
        assert_eq!(named, s("auth", "Fix"));
        let named = label(Some("auth"), "pwsh", "\u{2733} Fix", true, "Done.");
        assert_eq!(named, s("auth", "Done."));
    }

    #[test]
    fn palette_renames_on_its_line() {
        let mut c = Commands {
            filter: "Split".into(),
            rename: Some(Rename::Tab(PaneId(1))),
            ..Commands::default()
        };
        assert!(c.matches().is_empty(), "the line is a name, not a filter");
        c.rename = None;
        assert!(!c.matches().is_empty());
        let names: Vec<_> = (keymap::ACTIONS.iter()).map(|a| a.1).collect();
        assert!(names.contains(&"rename_session") && names.contains(&"rename_tab"));
    }

    #[test]
    fn app_taskbar_flashes_once_per_session_every_ten_seconds() {
        let t0 = Instant::now();
        let mut last = None;
        let flash = |state, last: &mut Option<Instant>, s| {
            flash_kind(state, last, t0 + Duration::from_secs(s))
        };
        assert_eq!(flash(Attn::Working, &mut last, 0), None);
        assert_eq!(flash(Attn::Idle, &mut last, 0), None);
        assert_eq!(last, None, "only flashes count");
        let critical = Some(UserAttentionType::Critical);
        assert_eq!(flash(Attn::NeedsYou, &mut last, 0), critical);
        assert_eq!(flash(Attn::Error, &mut last, 9), None);
        assert_eq!(flash(Attn::Error, &mut last, 10), critical);
        let gentle = Some(UserAttentionType::Informational);
        assert_eq!(flash(Attn::DoneUnseen, &mut last, 20), gentle);
        // Another session has its own limit.
        assert_eq!(flash(Attn::NeedsYou, &mut None, 21), critical);
    }

    #[test]
    fn app_hooks_set_the_message_and_the_session() {
        let id = "0b8f6a3e-1c2d-4e5f-9a7b-3c4d5e6f7a8b";
        let (mut msg, mut claude) = (String::new(), None);
        note_hook(
            &mut msg,
            &mut claude,
            Ev::Working,
            Some(id),
            "Fix it".into(),
        );
        assert_eq!((msg.as_str(), claude.as_deref()), ("Fix it", Some(id)));
        // Still working, now waiting on agents: the message says so.
        note_hook(
            &mut msg,
            &mut claude,
            Ev::Working,
            None,
            "waiting on 2 agents".into(),
        );
        assert_eq!(msg, "waiting on 2 agents");
        // The reply replaces it, whatever the state does.
        note_hook(&mut msg, &mut claude, Ev::Done, None, "Fixed.".into());
        assert_eq!((msg.as_str(), claude.as_deref()), ("Fixed.", Some(id)));
        note_hook(&mut msg, &mut claude, Ev::Done, Some(id), "Again.".into());
        assert_eq!(msg, "Again.");
        // Ready keeps what the last turn said.
        note_hook(&mut msg, &mut claude, Ev::Ready, Some(id), String::new());
        assert_eq!(msg, "Again.");
        // The session ended: nothing to show or resume.
        note_hook(&mut msg, &mut claude, Ev::Idle, Some(id), String::new());
        assert_eq!((msg.as_str(), claude), ("", None));
    }

    /// A slow shell's first prompt can come after the timer typed the
    /// resume: Claude Code is starting then, not gone.
    #[test]
    fn app_only_a_later_prompt_ends_claude() {
        let line = || Some(("claude --resume x".to_owned(), Instant::now()));
        let (mut prompted, mut resume) = (false, line());
        assert_eq!(
            prompt_back(&mut prompted, &mut resume),
            Prompt::Resume("claude --resume x".into())
        );
        assert_eq!(prompt_back(&mut prompted, &mut resume), Prompt::Exited);
        // The timer typed it before the shell was ready.
        let (mut prompted, mut resume) = (false, None);
        assert_eq!(prompt_back(&mut prompted, &mut resume), Prompt::First);
        assert_eq!(prompt_back(&mut prompted, &mut resume), Prompt::Exited);
        assert_eq!(prompt_back(&mut prompted, &mut resume), Prompt::Exited);
    }

    #[test]
    fn app_a_user_away_from_the_screen_is_not_watching() {
        let s = Duration::from_secs;
        assert!(present(true, s(0)));
        assert!(present(true, s(29)));
        assert!(!present(true, s(30)), "walked away with blitz in front");
        assert!(!present(false, s(0)), "another window is in front");
    }

    #[test]
    fn app_the_branch_is_read_again_when_a_turn_ends() {
        for ev in [Ev::Done, Ev::NeedsYou, Ev::Idle] {
            assert!(turn_ends(ev), "{ev:?}");
        }
        for ev in [Ev::Working, Ev::Error { sticky: false }] {
            assert!(!turn_ends(ev), "{ev:?}");
        }
    }

    #[test]
    fn app_bells_and_notifications_leave_hooked_panes_to_the_hooks() {
        assert!(rings(true, false));
        assert!(!rings(true, true));
        assert!(!rings(false, false));
        // What reaches the app as a notification: OSC 9 and 777, with
        // no pane token.
        let mut t = vt::Terminal::new(vt::Options::default());
        t.feed(b"\x1b]9;Claude is waiting for your input\x07\x1b]777;notify;Build;done\x07");
        let mut evs = Vec::new();
        t.take_events(&mut evs);
        let untokened = (evs.iter())
            .filter(|e| matches!(e, Event::Notify { title, .. } if Ev::from_notify(title, "0f1e").is_none()))
            .count();
        assert_eq!(untokened, 2);
    }

    /// Both hints lead to the palette entry that sets the hooks up.
    #[test]
    fn app_hooks_hints_name_the_setup() {
        let keys = Some("Ctrl+Shift+P".to_owned());
        assert_eq!(
            hooks_hint_text(false, keys.clone()),
            "Claude Code's hooks are not reporting to blitz \u{b7} Ctrl+Shift+P, Claude Code setup"
        );
        assert_eq!(
            hooks_hint_text(true, keys),
            "Claude Code's settings run an older blitz-hook \u{b7} Ctrl+Shift+P, Claude Code setup"
        );
        assert!(hooks_hint_text(true, None).ends_with("the command palette Claude Code setup"));
    }

    #[test]
    fn app_titles_that_show_claude_working() {
        for t in ["\u{25d0} Fix the tests", "\u{25d1} x"] {
            assert!(claude_working_title(t), "{t}");
        }
        // Not Claude Code's marks, so the sidebar would not see it work.
        for t in [
            "\u{2733} Fix the tests",
            "",
            "pwsh",
            "x \u{25d0}",
            "\u{25d3} x",
        ] {
            assert!(!claude_working_title(t), "{t}");
        }
    }

    /// Working for a while with no hook heard means the hooks are not
    /// reporting; a first hook, or no sign of Claude Code, means nothing.
    #[test]
    fn app_hooks_silent_after_working_quietly() {
        let t0 = Instant::now();
        let later = t0 + HOOKS_QUIET;
        assert!(!hooks_silent(None, false, later));
        assert!(!hooks_silent(
            Some(t0),
            false,
            later - Duration::from_secs(1)
        ));
        assert!(hooks_silent(Some(t0), false, later));
        assert!(!hooks_silent(Some(t0), true, later));
        // A clock that steps back is not a long wait.
        assert!(!hooks_silent(Some(later), false, t0));
    }

    #[test]
    fn a_session_starting_to_need_you_ends_the_game() {
        let t0 = Instant::now();
        let mut a = crate::attention::PaneAttn::new(t0);
        a.apply(Ev::Working, true, t0);
        assert!(ends_game(a, Ev::NeedsYou, t0));
        assert!(!ends_game(a, Ev::Done, t0));
        assert!(!ends_game(a, Ev::Error { sticky: false }, t0));
        assert!(!ends_game(a, Ev::Attended, t0));
        // Asking leaves the state as it was.
        assert_eq!(a.state, Attn::Working);
        a.apply(Ev::NeedsYou, false, t0);
        assert!(!ends_game(a, Ev::NeedsYou, t0), "a repeat");
        // The rule `attention()` relies on by closing the game first: a
        // needs-you on a pane in view is seen at once. This pins
        // `PaneAttn` only; the order inside `attention()` is not covered
        // here.
        let mut b = crate::attention::PaneAttn::new(t0);
        b.apply(Ev::Working, true, t0);
        assert!(b.apply(Ev::NeedsYou, true, t0));
        assert_eq!((b.state, b.seen), (Attn::NeedsYou, true));
        // A bell there needs nothing at all.
        let mut c = crate::attention::PaneAttn::new(t0);
        assert!(ends_game(c, Ev::Bell, t0));
        assert!(!c.apply(Ev::Bell, true, t0));
    }

    #[test]
    fn jumps_just_after_the_game_closes_stay_out_of_the_pane() {
        let ms = |n| Some(Duration::from_millis(n));
        for vk in [VK_SPACE, VK_UP, VK_W] {
            assert!(late_jump(vk, ms(0)));
            assert!(late_jump(vk, ms(399)));
            assert!(!late_jump(vk, ms(400)));
            assert!(!late_jump(vk, None), "the game never closed itself");
        }
        assert!(!late_jump(VK_RETURN, ms(0)), "Enter answers the prompt");
        assert!(!late_jump(VK_DOWN, ms(0)));
    }

    #[test]
    fn app_options() {
        let args: Vec<String> = ["--cmd", "cmd /k", "--exit-after", "4000"]
            .map(String::from)
            .into();
        let a = Args::parse(&args).expect("parse");
        assert_eq!(a.cmd.as_deref(), Some("cmd /k"));
        assert_eq!(a.exit_after, Some(Duration::from_millis(4000)));
        assert!(Args::parse(&["--bogus".into(), "1".into()]).is_err());
        assert!(Args::parse(&["--cmd".into()]).is_err());
        assert!(!a.new_window);
    }

    #[test]
    fn open_here_options() {
        let parse = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            Args::parse(&args).expect("parse")
        };
        assert_eq!(parse(&["--cwd", "C:\""]).cwd, Some(r"C:\".into()));
        assert_eq!(parse(&["--cwd", r"C:\foo"]).cwd, Some(r"C:\foo".into()));
        let a = parse(&["--new-window", "--cwd", r"C:\foo"]);
        assert!(a.new_window);
        assert_eq!(a.cwd, Some(r"C:\foo".into()));
        let a = parse(&["--cwd", r"C:\foo", "--new-window"]);
        assert!(a.new_window);
        assert_eq!(a.cwd, Some(r"C:\foo".into()));
    }

    #[test]
    fn output_from_before_keys_is_found_by_tab_and_tree_order() {
        let area = Rect {
            x: 0,
            y: 0,
            w: 800,
            h: 600,
        };
        let mut a = Tab::new("a".into(), PaneId(1));
        assert!(a.split(layout::Dir::Right, PaneId(2), area, (1, 1)));
        let win = layout::Window {
            tabs: vec![a, Tab::new("b".into(), PaneId(3))],
            ..Default::default()
        };
        assert_eq!(
            [1, 2, 3, 4].map(|i| leaf_index(&win, PaneId(i))),
            [Some((0, 0)), Some((0, 1)), Some((1, 0)), None]
        );
    }

    #[test]
    fn a_pane_that_fails_twice_says_why_first() {
        let (a, b) = ("folder too long".to_string(), "no shell".to_string());
        assert_eq!(joined(a.clone(), a.clone()), a);
        assert_eq!(joined(a, b), "folder too long; then no shell");
    }

    #[test]
    fn resume_line_types_only_a_session_id() {
        const ID: &str = "3f2a9c1e-0b7d-4e5f-9a8b-1c2d3e4f5a6b";
        assert_eq!(
            resume_line(true, Some(ID)).as_deref(),
            Some("claude --resume 3f2a9c1e-0b7d-4e5f-9a8b-1c2d3e4f5a6b\r")
        );
        assert_eq!(resume_line(false, Some(ID)), None);
        assert_eq!(resume_line(true, None), None);
        for bad in [
            "",
            "x; rm -rf ~",
            "3f2a9c1e-0b7d-4e5f-9a8b-1c2d3e4f5a6b\rcalc",
        ] {
            assert_eq!(resume_line(true, Some(bad)), None, "{bad:?}");
        }
    }

    /// A question stays until it is answered or another key is pressed,
    /// which then does what it always does; an error goes at the next key
    /// in its pane; other notices stay. A question about its own pane goes
    /// at any key aimed at another pane, its own action too.
    #[test]
    fn notices_go_at_the_next_key_that_does_not_answer_them() {
        let close = Some(Action::ClosePane);
        for ask in [Ask::ClosePane, Ask::CloseTab, Ask::Paste("a\nb".into())] {
            assert!(ask.of_pane());
            for a in [close, Some(Action::CloseTab), Some(Action::Paste)] {
                assert!(ask.gone(a, false), "{ask:?} at {a:?} elsewhere");
            }
            assert!(ask.gone(Some(Action::Palette), false));
        }
        assert!(!Ask::Update.of_pane() && !Ask::Quit.of_pane() && !Ask::Key.of_pane());
        let paste = Ask::Paste("a\nb".into());
        assert!(!paste.gone(Some(Action::Paste), true));
        assert!(paste.gone(close, true));
        assert!(!Ask::ClosePane.gone(close, true), "answered");
        assert!(
            !Ask::ClosePane.gone(Some(Action::Palette), true),
            "answered from the palette"
        );
        assert!(!Ask::CloseTab.gone(Some(Action::CloseTab), true));
        for here in [true, false] {
            let ask = Ask::ClosePane;
            assert!(ask.gone(None, here), "typing");
            assert!(ask.gone(Some(Action::Update), here), "another shortcut");
            assert!(!Ask::Update.gone(Some(Action::Update), here));
            assert!(Ask::Update.gone(Some(Action::Copy), here));
            assert_eq!(Ask::Key.gone(None, here), here, "an error, read");
            assert_eq!(Ask::Key.gone(close, here), here);
            assert!(!Ask::Nothing.gone(None, here));
            assert!(Ask::CloseTab.gone(close, here), "not the whole tab");
            assert!(
                Ask::Quit.gone(None, here),
                "closing the window, then typing"
            );
            assert!(Ask::Quit.gone(Some(Action::Palette), here));
        }
    }

    /// A passing hint leaves a question or an error where it is, and an
    /// exited program's line comes back once a notice over it goes.
    #[test]
    fn notices_over_questions_and_exits() {
        let notice = |ask, dim| Notice {
            text: "n".into(),
            until: None,
            dim,
            ask,
        };
        assert!(hint_fits(None));
        assert!(hint_fits(Some(&notice(Ask::Nothing, true))));
        for ask in [Ask::ClosePane, Ask::Key, Ask::Quit] {
            assert!(!hint_fits(Some(&notice(ask, false))));
        }
        let exited = notice_line(None, Some(2)).expect("a line");
        assert_eq!(
            exited,
            ("exit 2 \u{b7} Enter restart \u{b7} Esc close".into(), false)
        );
        let error = notice(Ask::Key, false);
        assert_eq!(
            notice_line(Some(&error), Some(2)),
            Some(("n".into(), false))
        );
        assert_eq!(notice_line(None, None), None);
    }

    #[test]
    fn lock_keys_leave_questions() {
        for vk in [0x14, 0x90, 0x91] {
            assert!(lock_key(vk), "{vk:#x}");
        }
        for vk in [0x41, VK_RETURN, VK_ESCAPE, 0x10] {
            assert!(!lock_key(vk), "{vk:#x}");
        }
    }

    #[test]
    fn busy_sessions_by_what_they_do_or_how_many() {
        assert_eq!(busy_text(&["working"], ""), "A session is working");
        assert_eq!(
            busy_text(&["running a command"], " in this tab"),
            "A session in this tab is running a command"
        );
        assert_eq!(
            busy_text(&["working", "waiting for you", "working"], ""),
            "3 sessions are busy"
        );
    }

    /// Holding Alt+F4 closes the window once, so a busy session's question
    /// waits for a second, deliberate press.
    #[test]
    fn a_held_alt_f4_closes_once() {
        let (first, repeat, up) = (0x003e_0001, 0x403e_0001, 0xc03e_0001_u32 as i32 as isize);
        assert!(alt_f4_passes(true, first));
        assert!(!alt_f4_passes(true, repeat));
        assert!(alt_f4_passes(false, up));
    }

    /// How a split, and a reopened pane, go in: beside the focused pane.
    #[test]
    fn a_split_goes_beside_the_focused_pane() {
        let mut win = layout::Window::default();
        win.tabs.push(Tab::new("t".into(), PaneId(1)));
        assert!(split(Dir::Right)(&mut win, PaneId(2)));
        let t = &win.tabs[0];
        assert_eq!(
            (t.panes(), t.focus),
            (vec![PaneId(1), PaneId(2)], PaneId(2))
        );
        let mut none = layout::Window::default();
        assert!(!split(Dir::Down)(&mut none, PaneId(3)), "no tab");
    }

    #[test]
    fn a_question_names_the_key_that_answers_it() {
        assert_eq!(again(Action::ClosePane, &[]), "Press Ctrl+Shift+W again");
        let moved = ["ctrl+shift+w=none", "alt+w=close_pane"].map(keymap::binding);
        let moved: Vec<_> = moved.into_iter().flatten().collect();
        assert_eq!(again(Action::ClosePane, &moved), "Press Alt+W again");
        assert_eq!(
            again(Action::Equalize, &[]),
            "Run Give the panes equal space again"
        );
    }

    #[test]
    fn palette_matches_every_word_of_the_label_or_name() {
        let mut c = Commands::default();
        assert_eq!(c.matches().len(), keymap::ACTIONS.len() - 1, "not itself");
        c.filter = "Split R".into();
        assert_eq!(c.matches(), [(Action::SplitRight, "Split right")]);
        c.filter = "font_size_up".into();
        assert_eq!(c.matches(), [(Action::FontSize(1), "Bigger font")]);
        c.filter = "claude".into();
        assert_eq!(c.matches(), [(Action::ClaudeSetup, "Claude Code setup")]);
        c.move_by(5);
        assert_eq!(c.sel, 0, "one match");
        c.filter = "zzz".into();
        assert!(c.matches().is_empty());
    }

    #[test]
    fn saved_output_keeps_the_last_lines() {
        assert_eq!(last_lines("\n\na\nb\nc\n\n\n", 2), "b\nc");
        assert_eq!(last_lines("a\n\nb", 10), "a\n\nb");
        assert_eq!(last_lines("  a\n", 10), "  a");
        assert_eq!(last_lines("\n\n", 10), "");
    }
}
