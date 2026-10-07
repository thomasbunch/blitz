//! The window, its event loop and the frame loop.

// One process hosts every session, so a failed HRESULT must never panic.
#![deny(clippy::unwrap_used)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vt::grid::Found;
use vt::{
    Event, InputModes, KeyInput, Mods, MouseEv, MouseKind, MouseMode, Palette, PromptMark, Snapshot,
};
use windows::UI::Notifications::ToastNotification;
use windows::Win32::Foundation::{HANDLE, HWND, POINT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::Power::{
    PowerClearRequest, PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest,
};
use windows::Win32::System::Threading::{
    POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetKeyState, GetKeyboardState, GetLastInputInfo, LASTINPUTINFO,
};
use windows::Win32::UI::Shell::{
    ITaskbarList3, TBPF_ERROR, TBPF_INDETERMINATE, TBPF_NOPROGRESS, TBPF_NORMAL, TBPF_PAUSED,
    TaskbarList,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateCaret, DestroyCaret, DestroyIcon, GetCursorPos, GetSystemMetrics, MB_OK, MSG,
    SM_CXSMICON, SetCaretPos, SetForegroundWindow, TranslateMessage, WM_CHAR, WM_DEADCHAR,
    WM_KEYDOWN, WM_KEYUP, WM_SYSCHAR, WM_SYSDEADCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use windows::core::{HSTRING, PWSTR};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::windows::{
    EventLoopBuilderExtWindows, IconExtWindows, WindowAttributesExtWindows,
};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{CursorIcon, Fullscreen, Icon, Window, WindowId};

use crate::arcade::run::{self, Run};
use crate::attention::{Attn, Ev, claude_title, exit_text};
use crate::config::{Config, Kind};
use crate::debug::Counters;
use crate::keymap::{self, Action};
use crate::layout::{self, Axis, Dir, PaneId, Rect, Tab};
use crate::links::{Link, Target};
use crate::pane::{Note, Pane, Spawn, git_branch, lock, program_name};
use crate::render::chrome::{self, ChromeModel, Side};
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
/// How long a hint stays up: about setting something up, or the keys
/// worth knowing on the first start.
const HINT: Duration = Duration::from_secs(10);
/// How long Claude Code may show it is working with no word from its
/// hooks before blitz says they are not reporting. A turn's first hook
/// lands within a second of it starting.
const HOOKS_QUIET: Duration = Duration::from_secs(45);
/// How long a dim notice that only says something worked stays up.
const BRIEF: Duration = Duration::from_secs(2);
/// How long a shortcut with nothing to do says so.
const NOTHING: Duration = Duration::from_secs(1);
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
/// Alerts about one session are at least this far apart.
const ALERT_GAP: Duration = Duration::from_secs(10);
/// After this long with no key or mouse input anywhere, the user counts as
/// away from the screen, even with blitz in front.
const AWAY_AFTER: Duration = Duration::from_secs(30);
/// How long a restored pane waits for its shell's first prompt before it
/// types the Claude Code resume command anyway.
const RESUME_AFTER: Duration = Duration::from_secs(3);
/// Lines of output saved per pane when `restore_scrollback` is on.
const SAVED_LINES: usize = 1000;
/// How long a changed layout waits before it is saved, so holding a resize
/// key writes the file at most once this often rather than every step. A
/// divider drag writes it once, after the drag.
const SAVE_DELAY: Duration = Duration::from_millis(500);
/// How long to wait before building the renderer again after it failed.
const GFX_RETRY: Duration = Duration::from_secs(1);
/// Time between the steps a drag scrolls while the pointer is held above
/// or below its pane.
const AUTOSCROLL: Duration = Duration::from_millis(50);
/// How long a new window stays hidden waiting for its first frame.
const FIRST_FRAME: Duration = Duration::from_millis(500);
/// How often at most a terminal takes a new size while its pane keeps
/// changing size, as in a live resize or a divider drag: each new size
/// makes the program redraw its whole screen.
const RESIZE_GAP: Duration = Duration::from_millis(80);
/// How often the find bar searches again while output streams into its
/// pane: each search reads all of the scrollback.
const FIND_EVERY: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub enum UserEvent {
    Pane(PaneId, Note),
    /// The git branch of a pane's directory, read on another thread.
    Branch(PaneId, String, Option<String>),
    /// Exit with this code: the self-test finished, or `--exit-after`
    /// ran out.
    Finish(i32),
    /// The update to this newer release failed and wrote this installer
    /// log.
    Failed(String, PathBuf),
    /// What a look for a newer release found, and whether Ctrl+Shift+U
    /// asked for it.
    Checked(Result<Option<String>, String>, bool),
    /// The installer started, so blitz exits; or why it did not.
    Installed(Result<(), String>),
    /// The installer of this release, downloaded to run when blitz
    /// closes, or why it could not be.
    Fetched(String, Result<crate::update::Installer, String>),
    /// Another launch asks the window to come to the front, and maybe to
    /// open a tab in its folder.
    Handoff(crate::handoff::Ask),
    /// Something in `%APPDATA%\blitz` was written: settings or a theme.
    Settings,
    /// The user let go of the window after moving or sizing it.
    Sized,
    /// Explorer made the window's taskbar button, which starts out blank.
    TaskbarButton,
    /// The user clicked the notification about this session.
    ShowPane(PaneId),
    /// The user pressed the key that brings them to blitz from anywhere.
    GlobalJump,
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
            // A folder alone, as in `blitz .`, is `--cwd`. Anything else is
            // a mistyped command, which must not open a window.
            if !flag.starts_with("--") {
                let dir = folder(flag);
                if !dir.is_dir() {
                    return Err(format!("no such folder: {flag}"));
                }
                a.cwd = Some(dir);
                continue;
            }
            let v = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
            match flag.as_str() {
                "--cmd" => a.cmd = Some(v.clone()),
                "--cwd" => a.cwd = Some(folder(v)),
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

    /// A scripted or test run, which nobody watches.
    fn scripted(&self) -> bool {
        self.cmd.is_some()
            || self.selftest.is_some()
            || self.exit_after.is_some()
            || self.capture.is_some()
    }

    /// Takes what the blitz already running made of this launch, `None`
    /// when none runs. True when it took the launch, which is then done.
    /// One that runs but did not take it (it hung, or refused the folder)
    /// keeps the saved session, so this launch gets a separate window: a
    /// second main window would restore the same tabs and resume Claude
    /// Code conversations that are live in the first.
    fn handed_off(&mut self, sent: Option<bool>) -> bool {
        self.new_window |= sent == Some(false);
        sent == Some(true)
    }
}

/// A folder from the command line, made absolute, as the tab it opens is
/// named after it and the session keeps it. Explorer passes a drive root
/// as "C:\", and argv parsing reads the \" as an escaped quote, so it
/// arrives as C:".
fn folder(arg: &str) -> PathBuf {
    let dir: PathBuf = match arg.strip_suffix('"') {
        Some(root) => format!("{root}\\").into(),
        None => arg.into(),
    };
    std::path::absolute(&dir).unwrap_or(dir)
}

/// Runs the GUI until the window closes. Returns the process exit code, or
/// what is wrong with the arguments.
pub fn run(args: &[String]) -> Result<i32, String> {
    let mut args = Args::parse(args)?;
    // Run as administrator, blitz is a window of its own: it takes no
    // launches, gives none away, and leaves the saved session to the
    // normal one, whose Claude Code sessions it would resume elevated.
    let admin = elevated();
    args.new_window |= admin;
    // A launch brings the blitz already running to the front, and a folder
    // opens as a tab there. Scripted and test launches always get a window
    // of their own.
    if !args.new_window && !args.scripted() {
        let sent = crate::handoff::send(args.cwd.as_deref());
        if args.handed_off(sent) {
            return Ok(0);
        }
    }
    catch_crashes();
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
            return Ok(1);
        }
    };
    let mut app = App::new(args, keys, event_loop.create_proxy());
    app.gpu = Some(gpu);
    app.admin = admin;
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("blitz: {e}");
        return Ok(1);
    }
    Ok(app.code)
}

/// Writes a panic on this thread, the window's, to the crash file before
/// it takes blitz down, since a release build has no console to say it
/// on. Other threads catch their own: a pane's reader, an update.
pub fn catch_crashes() {
    let ui = std::thread::current().id();
    let next = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == ui {
            let text = format!(
                "blitz {} stopped at {}\n{info}\n\n{}\n",
                env!("CARGO_PKG_VERSION"),
                local_stamp(),
                std::backtrace::Backtrace::force_capture()
            );
            let _ = session::write_crash(&text);
        }
        next(info);
    }));
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
    /// not, since an action run from it confirms too.
    fn gone(&self, a: Option<Action>, here: bool) -> bool {
        let by = match self {
            Ask::Nothing => return false,
            Ask::Key => return here,
            Ask::Paste(_) => Action::Paste,
            Ask::ClosePane => Action::ClosePane,
            Ask::CloseTab => Action::CloseTab,
            Ask::Update => Action::Update,
            Ask::Quit => return true,
        };
        a != Some(by) && a != Some(Action::Palette)
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
    /// When Ctrl and the wheel last changed the font size.
    font_at: Option<Instant>,
    /// A left-button drag is moving this divider of the active tab; the
    /// second value is the smallest pane it may leave.
    divider: Option<(usize, (i32, i32))>,
    /// What in the sidebar the pointer is over.
    over_side: Option<Side>,
    /// The pointer's shape; see [`pointer`].
    icon: CursorIcon,
    /// The pointer is hidden while typing.
    hidden: bool,
    /// A left-button drag is making a selection.
    drag: Option<Drag>,
    /// When the drag next scrolls, while the pointer is outside its pane.
    scroll_at: Option<Instant>,
    /// The last press that went to selection: when, on which cell, and
    /// how many clicks it made.
    click: Option<(Instant, Pos, u8)>,
    /// The same for the last press on a divider, by its index.
    divider_click: Option<(Instant, Pos, u8)>,
    /// The cell of the left press a program was sent, and whether a drag
    /// from it may still show the Shift+drag hint.
    program_press: Option<((u16, u16), bool)>,
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

/// Whether a press on divider `i` double-clicks it, which gives the panes
/// equal space. `last` holds the last press on a divider and takes this
/// one.
fn evens(last: &mut Option<(Instant, Pos, u8)>, i: usize, now: Instant, within: Duration) -> bool {
    let n = clicks(*last, (i, 0), now, within);
    *last = Some((now, (i, 0), n));
    n == 2
}

/// Selected cells in a pane.
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

    /// The text it held at the last look, once output rewrote it. Lines
    /// above the screen were scrollback then, which output leaves alone;
    /// `None` when some of them are gone.
    fn last_text(&self, term: &vt::Terminal, pal: &Palette) -> Option<String> {
        let (top, shown) = &self.seen;
        if self.start.0 >= *top {
            return Some(shown.clone());
        }
        if !self.kept(term) {
            return None;
        }
        let all = selection_text(term, pal, self, 0);
        let now = selection_text(term, pal, self, *top);
        Some(all.strip_suffix(now.as_str())?.to_owned() + shown)
    }

    /// The cells that move with the text when a new width wraps the lines
    /// again: its ends and its anchor. None for a block, whose columns
    /// mean nothing at another width.
    fn marks(&self) -> Vec<Pos> {
        match self.drag.block {
            true => Vec::new(),
            false => vec![self.start, self.end, self.drag.anchor.0, self.drag.anchor.1],
        }
    }

    /// Takes the cells [`Self::marks`] gave, moved to where `term` wrapped
    /// their text again.
    fn reflowed(&mut self, term: &vt::Terminal, pal: &Palette, marks: &[Pos]) {
        if let &[start, end, a, b] = marks {
            (self.start, self.end, self.drag.anchor) = (start, end, (a, b));
            self.drag.epoch = term.line_epoch();
            self.look(term, pal);
        }
    }
}

/// A selection of the cells from `start` to `end`, as made in code.
fn selection_of(term: &vt::Terminal, pal: &Palette, start: Pos, end: Pos) -> Selection {
    let drag = Drag {
        epoch: term.line_epoch(),
        anchor: (start, end),
        unit: 1,
        block: false,
    };
    Selection::new(term, pal, drag, start)
}

/// The first and last line of the text in `term`: none blank at the end.
fn all_text(term: &vt::Terminal, pal: &Palette) -> Option<(usize, usize)> {
    let lines = term.lines();
    let last = (lines.clone()).rev().find(|&n| !blank(term, pal, n))?;
    Some((lines.start, last))
}

/// The first and last line of the last command's output: the lines from
/// the end of the command at blitz's next to last prompt down to its last
/// prompt, none blank at the end.
fn last_output(term: &vt::Terminal, pal: &Palette) -> Option<(usize, usize)> {
    let mut prompts = term.lines().rev().filter(|&n| term.starts_prompt(n));
    let (now, before) = (prompts.next()?, prompts.next()?);
    let mut first = before;
    while first < now && term.wraps(first) {
        first += 1;
    }
    let last = (first + 1..now).rev().find(|&n| !blank(term, pal, n))?;
    Some((first + 1, last))
}

/// Whether line `n` holds no text.
fn blank(term: &vt::Terminal, pal: &Palette, n: usize) -> bool {
    let mut cells = Vec::new();
    term.line_cells(n, pal, &mut cells);
    cells.iter().all(|c| c.len == 0)
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

/// Marks in `snap`, a view from line `top`, what is drawn over the text:
/// its pane's selection, which every pane keeps while another has focus,
/// and the link under the pointer.
fn mark(snap: &mut Snapshot, top: usize, sel: Option<&Selection>, hover: Option<(Pos, Pos)>) {
    let size = (snap.cols, snap.rows);
    snap.selection = sel.and_then(|s| in_view(s.start, s.end, s.drag.block, top, size));
    snap.block = sel.is_some_and(|s| s.drag.block);
    snap.hover = hover.and_then(|(a, b)| in_view(a, b, false, top, size));
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

/// The word around `at`, following soft wraps: a URL or path the link
/// scanner finds there, else a run of word characters without the
/// punctuation that ends a sentence, or of blanks, or any other character
/// alone.
fn word_at(term: &vt::Terminal, pal: &Palette, at: Pos) -> (Pos, Pos) {
    let l = Logical::new(term, pal, at.0);
    let Some(i) = l.index(at) else {
        return (at, at);
    };
    let here = l.cells[i].0;
    let mut links = crate::links::scan(&l.text).into_iter();
    if let Some((range, _)) = links.find(|(r, _)| r.contains(&here)) {
        return l.span(range);
    }
    let char_at = |k: usize| l.text[l.cells[k].0..].chars().next();
    let class = |k: usize| match char_at(k) {
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
    while b > i && char_at(b).is_some_and(|c| ".,:;?!".contains(c)) {
        b -= 1;
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
    /// When the terminal last took a new size, and when its pane's new
    /// size goes to it; see [`resize_wait`].
    resized: Option<Instant>,
    resize_at: Option<Instant>,
    /// Where the grid went in the last frame; `None` while its tab is
    /// hidden.
    rect: Option<Rect>,
    notice: Option<Notice>,
    /// When blitz last alerted the user about this session.
    alerted: Option<Instant>,
    /// The notification about this session, while it is up.
    toast: Option<ToastNotification>,
    /// A thread is reading the git branch of the session's directory.
    finding_branch: bool,
    /// A line to type at the shell's first prompt, and when to type it
    /// anyway: what resumes a restored Claude Code session, or starts a
    /// new one.
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
    /// The number after the session's name, which tells look-alike
    /// sessions apart; kept across restarts.
    num: u32,
    /// The `shell` setting the session runs when one was picked in the
    /// command palette; empty for the one in the settings. Kept across
    /// restarts.
    shell: String,
    /// The progress the program last reported, and when.
    progress: Option<(chrome::Progress, Instant)>,
    /// When the title first showed Claude Code working.
    claude_working: Option<Instant>,
    /// A hook notification came, so Claude Code's hooks report.
    hooks_seen: bool,
    /// Selected text, kept while another pane has focus.
    selection: Option<Selection>,
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
    /// Windows hides the pointer while typing.
    vanish: bool,
    /// The colours of Windows high contrast mode when last looked at.
    contrast: Option<[u32; 3]>,
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
    /// What was last searched for, which the find bar opens with when no
    /// text is selected.
    find_last: String,
    /// Quick select's labels, while they show.
    quick: Option<Quick>,
    scale: f64,
    /// Tabs and the split tree in each.
    win: layout::Window,
    /// Every session, oldest first. The sidebar lists them as their panes
    /// sit.
    views: Vec<View>,
    /// What a click in the sidebar acted on in the last frame.
    side: chrome::SideHits,
    next_id: u32,
    /// The number the next new session shows after its name.
    next_num: u32,
    focused: bool,
    /// The text of a selection in the focused pane that output rewrote,
    /// which Copy still takes until the user does something else.
    orphan: Option<String>,
    /// The link under the pointer while Ctrl is held, drawn underlined:
    /// the line epoch and its first and last cell.
    hover: Option<(u32, Pos, Pos)>,
    mouse: Mouse,
    /// IME composition text, drawn at the cursor.
    preedit: String,
    /// Files dropped on the window, one event each, handled together when
    /// the event loop has nothing more to deliver.
    dropped: Vec<PathBuf>,
    /// Where the last jump to a session that needs you came from, and
    /// where it went; see [`jump`].
    jumped: Option<(PaneId, PaneId)>,
    /// A newer release: its version and the banner text.
    update: Option<(String, String)>,
    /// The installer is downloading, or Ctrl+Shift+U is looking for a
    /// release; this pane hears how it went.
    updating: Option<PaneId>,
    /// The folder and Claude Code session of the pane closed last, which
    /// the palette can reopen.
    closed: Option<(String, Option<String>)>,
    /// Said in the banner in place of the offer for now: the question that
    /// running Update again answers, or that the update is downloading.
    banner_note: Option<(String, Ask)>,
    /// Why the last look for a release, or the last update, failed.
    update_error: Option<String>,
    /// The release that installs when blitz closes, with its installer
    /// once downloaded.
    at_close: Option<AtClose>,
    /// The banner strip and the x that closes it in the last frame, for
    /// clicks.
    banner: Option<(Rect, Rect)>,
    /// The chips on panes scrolled back in the last frame, for clicks.
    below: Vec<(PaneId, Rect)>,
    /// The find bar in the last frame, for clicks.
    find_bar: Option<Rect>,
    /// Keys whose releases belong to a shortcut or a panel and are not sent.
    eaten: Eaten,
    /// Where the IME and the caret were last told typing goes, in client
    /// pixels.
    ime_at: Option<Rect>,
    /// What the title bar shows.
    title: String,
    /// The size of the system caret while the window has one.
    caret: Option<(u32, u32)>,
    /// Checked once the first output shows which ConPTY is running.
    checked_conpty: bool,
    /// A Claude Code hook has reported from a pane since blitz started.
    hooked: bool,
    capture_then_exit: bool,
    /// This is the main window, whose layout is saved for the next start.
    /// Separate windows and scripted runs leave the saved one alone.
    persist: bool,
    /// blitz runs as administrator, and its title says so.
    admin: bool,
    /// The window is hidden until its first frame, or until this time.
    hidden_until: Option<Instant>,
    /// The session as last saved.
    saved: Option<session::State>,
    /// When a changed layout is saved, unless it changes back first.
    save_after: Option<Instant>,
    /// Writes of the session in a row that failed.
    save_fails: u32,
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
    /// The state whose dot badges the taskbar button.
    badge_shows: Option<Attn>,
    /// Ctrl+Alt+J comes to this window from every program.
    jump_key: bool,
    /// The power request that keeps the PC awake; made the first time it
    /// is needed.
    power: Option<HANDLE>,
    /// The PC is kept awake.
    awake: bool,
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
        let (sel, last) = (self.sel as isize, self.matches().len().saturating_sub(1));
        let step = |by: isize| (sel + by).clamp(0, last as isize) as usize;
        match k.vk {
            VK_UP => self.sel = step(-1),
            VK_DOWN => self.sel = step(1),
            VK_PRIOR => self.sel = step(-(chrome::PICKER_ROWS as isize)),
            VK_NEXT => self.sel = step(chrome::PICKER_ROWS as isize),
            _ if edit_field(&mut self.filter, k) => self.sel = 0,
            _ => return false,
        }
        true
    }
}

/// What a row of the command palette does when picked.
#[derive(Clone, Debug, PartialEq)]
enum Pick {
    Run(Action),
    /// Bring this session to the front.
    Show(PaneId),
    /// Open the settings panel on what was typed.
    Settings,
    /// A new tab running this `shell` setting.
    Shell(String),
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
    /// Actions left out, as they have nothing to do now.
    hidden: Vec<Action>,
    /// Sessions to go to, listed in place of the actions: each with its
    /// row and its state.
    sessions: Option<Vec<(PaneId, String, String)>>,
    /// A row for each shell installed, as (label, `shell` setting).
    shells: Vec<(String, String)>,
}

/// What the command palette's line names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rename {
    Session(PaneId),
    /// The tab that holds this session.
    Tab(PaneId),
}

impl Commands {
    /// The palette with a "New tab: <name>" row for each of `shells`, as
    /// [`crate::shell::choices`] lists them; the automatic one is New tab.
    fn new(shells: Vec<(String, String)>) -> Commands {
        let shells = (shells.into_iter())
            .filter(|(_, shell)| !shell.is_empty())
            .map(|(name, shell)| (format!("New tab: {name}"), shell))
            .collect();
        Commands {
            shells,
            ..Commands::default()
        }
    }

    /// The matching rows, each what it does and its label: the sessions
    /// whose row or state holds every typed word, or else actions in
    /// [`keymap::ACTIONS`] order with the shells after New tab, leaving out
    /// the palette itself, going to a tab by number and the `hidden`
    /// actions. A shell's command line matches too, so `wsl` finds every
    /// WSL distribution. When none matches what was typed, one row searches
    /// the settings for it instead.
    fn matches(&self) -> Vec<(Pick, String)> {
        if self.rename.is_some() {
            return Vec::new();
        }
        let words: Vec<String> = (self.filter.split_whitespace())
            .map(str::to_lowercase)
            .collect();
        let hit = |text: String| {
            let text = text.to_lowercase();
            words.iter().all(|w| text.contains(w.as_str()))
        };
        if let Some(list) = &self.sessions {
            return (list.iter())
                .filter(|s| hit(format!("{} {}", s.1, s.2)))
                .map(|s| (Pick::Show(s.0), s.1.clone()))
                .collect();
        }
        let mut rows: Vec<(Pick, String)> = Vec::new();
        let actions = (keymap::ACTIONS.iter())
            .filter(|a| !matches!(a.0, Action::Palette | Action::GoToTab(_) | Action::LastTab))
            .filter(|a| !self.hidden.contains(&a.0));
        for &(a, name, label) in actions {
            if hit(format!("{label} {name}")) {
                rows.push((Pick::Run(a), label.to_string()));
            }
            if a == Action::NewTab {
                for (label, shell) in &self.shells {
                    if hit(format!("{label} {shell}")) {
                        rows.push((Pick::Shell(shell.clone()), label.clone()));
                    }
                }
            }
        }
        let typed = self.filter.trim();
        if rows.is_empty() && !typed.is_empty() {
            rows.push((Pick::Settings, format!("Search settings for \"{typed}\"")));
        }
        rows
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
    /// When the last search ran; `None` asks for one at once.
    searched: Option<Instant>,
    /// The query was put there when the bar opened and is drawn selected:
    /// typing replaces it.
    fresh: bool,
}

impl Find {
    fn new(pane: PaneId) -> Find {
        Find {
            pane,
            query: String::new(),
            found: Vec::new(),
            cur: None,
            stale: false,
            searched: None,
            fresh: false,
        }
    }

    /// Types `t` into the query, in place of a fresh one.
    fn type_text(&mut self, t: &str) {
        if std::mem::take(&mut self.fresh) {
            self.query.clear();
        }
        self.query.push_str(t);
    }

    /// A key for the query, as [`edit_field`] takes it, except that the
    /// first edit of a fresh query replaces all of it, and Backspace then
    /// empties it. False when nothing changed.
    fn edit(&mut self, k: &KeyInput) -> bool {
        if !self.fresh {
            return edit_field(&mut self.query, k);
        }
        let mut typed = String::new();
        if k.vk != VK_BACK && !edit_field(&mut typed, k) {
            return false;
        }
        self.fresh = false;
        self.query = typed;
        true
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
        self.searched = Some(Instant::now());
        let above = self.found.partition_point(|m| m.start <= anchor);
        self.cur = (!self.found.is_empty()).then(|| above.saturating_sub(1));
    }

    /// Moves `by` matches, down when positive, wrapping at either end.
    fn step(&mut self, by: isize) {
        let n = self.found.len() as isize;
        self.cur = (self.cur).map(|i| (i as isize + by).rem_euclid(n) as usize);
    }

    /// When output that came since the last search is searched: at once
    /// after a quiet spell, else `FIND_EVERY` after the last search, so a
    /// pane streaming output is searched a few times a second, not on
    /// every frame. `None` with nothing to search.
    fn due(&self) -> Option<Instant> {
        self.stale
            .then(|| self.searched.map_or(Instant::now(), |t| t + FIND_EVERY))
    }
}

/// Quick select: a label on each URL, path and commit hash in view in the
/// focused pane. Typing a label copies what it marks; with Shift it opens
/// it.
struct Quick {
    pane: PaneId,
    /// The line epoch the matches are numbered in.
    epoch: u32,
    /// The matches from the bottom of the view up, labelled `a` to `z`:
    /// their cells, their text, and what opening them does. A hash opens
    /// nothing.
    items: Vec<QuickItem>,
}

type QuickItem = (Found, String, Option<Target>);

/// The URLs, paths and commit hashes in the `rows` high view of `term`, at
/// most one for each letter, from the bottom up: as [`Quick::items`]. A
/// path counts when `resolve` finds the file.
fn quick_items(
    term: &vt::Terminal,
    pal: &Palette,
    rows: u16,
    resolve: impl Fn(&str) -> Option<PathBuf>,
) -> Vec<QuickItem> {
    let top = term.view_top();
    let shown = top..top + usize::from(rows);
    let mut out = Vec::new();
    let mut n = top;
    while shown.contains(&n) {
        let l = Logical::new(term, pal, n);
        let mut found: Vec<(Range<usize>, Option<Target>)> = Vec::new();
        for (range, link) in crate::links::scan(&l.text) {
            let target = match link {
                Link::Url(u) => Target::Uri(u),
                Link::Path(p, at) => match resolve(&p) {
                    Some(full) => Target::Path(full, at),
                    None => continue,
                },
            };
            found.push((range, Some(target)));
        }
        for range in hashes(&l.text) {
            if !found
                .iter()
                .any(|(r, _)| r.start < range.end && range.start < r.end)
            {
                found.push((range, None));
            }
        }
        found.sort_by_key(|f| f.0.start);
        for (range, target) in found {
            let (start, end) = l.span(range.clone());
            if shown.contains(&start.0) {
                out.push((Found { start, end }, l.text[range].to_owned(), target));
            }
        }
        n = l.cells.last().map_or(n, |c| c.1.0.max(n)) + 1;
    }
    out.reverse();
    out.truncate(26);
    out
}

/// The label a key types for quick select, `a` as 0, and whether Shift
/// asks to open rather than copy. By the key, not its text, so Caps Lock
/// does not turn a copy into an open, and a label types on any layout.
fn quick_label(k: &KeyInput) -> Option<(usize, bool)> {
    let m = &k.mods;
    let letter = (0x41..=0x5a).contains(&k.vk);
    let other = m.lctrl || m.rctrl || m.lalt || m.ralt || m.lsuper || m.rsuper;
    (letter && !other).then(|| (usize::from(k.vk - 0x41), m.lshift || m.rshift))
}

/// Where `text` holds a commit hash: 7 to 40 hex digits with a letter and
/// a digit among them, a word of its own. Not a piece of a path, a GUID
/// or a name such as cargo's `blitz-3f2a1b9c8d7e6f50`.
fn hashes(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let len = b[i..].iter().take_while(|c| c.is_ascii_hexdigit()).count();
        let word = &b[i..i + len];
        let alone =
            |j: Option<&u8>| j.is_none_or(|c| !c.is_ascii_alphanumeric() && !b"_-/\\".contains(c));
        if (7..=40).contains(&len)
            && word.iter().any(u8::is_ascii_digit)
            && word.iter().any(u8::is_ascii_alphabetic)
            && alone(i.checked_sub(1).and_then(|j| b.get(j)))
            && alone(b.get(i + len))
        {
            out.push(i..i + len);
        }
        i += len.max(1);
    }
    out
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
        let persist = !args.new_window && !args.scripted();
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
            vanish: mouse_vanish(),
            contrast: crate::theme::system_contrast(),

            commands: None,
            commands_hits: None,
            font_zoom: 0.0,
            find: None,
            find_last: String::new(),
            quick: None,
            scale: 1.0,
            win: layout::Window::default(),
            views: Vec::new(),
            side: chrome::SideHits::default(),
            next_id: 1,
            next_num: 1,
            focused: false,
            orphan: None,
            hover: None,
            mouse: Mouse::default(),
            preedit: String::new(),
            dropped: Vec::new(),
            jumped: None,
            update: None,
            updating: None,
            closed: None,
            banner_note: None,
            update_error: None,
            at_close: None,
            banner: None,
            below: Vec::new(),
            find_bar: None,
            eaten: Eaten::default(),
            ime_at: None,
            title: "blitz".into(),
            caret: None,
            checked_conpty: false,
            hooked: false,
            capture_then_exit: false,
            persist,
            admin: false,
            hidden_until: None,
            saved: None,
            save_after: None,
            save_fails: 0,
            gfx_retry: None,
            placed: Geometry::default(),
            watched: None,
            started: Instant::now(),
            gpu: None,
            taskbar: None,
            taskbar_shows: None,
            plugin: None,
            hooks_hinted: false,
            badge_shows: None,
            jump_key: false,
            power: None,
            awake: false,
            counters: Counters::default(),
            code: 0,
        }
    }

    /// The terminal font's size in pixels, zoomed, and the size the
    /// settings give it, which the chrome follows.
    fn font_px(&self) -> (f32, f32) {
        let px = |pt: f32| pt * 96.0 / 72.0 * self.scale as f32;
        let set = self.config.font_size;
        (px(set + self.font_zoom), px(set))
    }

    /// Creates the window and starts the first session.
    fn start(&mut self, el: &ActiveEventLoop) -> Result<(), String> {
        // Hidden until its first frame, which would otherwise come after a
        // flash of white or black.
        self.hidden_until = Some(Instant::now() + FIRST_FRAME);
        let mut attrs = Window::default_attributes()
            .with_title(window_title(0, "", self.admin))
            .with_visible(false)
            .with_inner_size(LogicalSize::new(980.0, 620.0))
            // Icon group 1, which build.rs links in.
            .with_window_icon(Icon::from_resource(1, Some(small_icon_size())).ok())
            .with_taskbar_icon(Icon::from_resource(1, None).ok())
            // Only the main window takes folders from other launches.
            .with_class_name(crate::handoff::class(self.args.new_window));
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
        crate::handoff::install(self.hwnd, self.proxy.clone(), !self.args.new_window);
        crate::notify::install(self.hwnd, self.proxy.clone());
        watch_settings(self.proxy.clone());
        self.plugin = crate::hook::install_plugin().map(|d| d.to_string_lossy().into_owned());
        self.frame_theme();
        self.window = Some(window);
        self.ensure_gfx();
        // The background, before sessions start, so the window shows at
        // once however long they take.
        self.redraw();

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
                None => first_dir(
                    std::env::current_dir().ok(),
                    &not_a_start(),
                    std::env::var_os("SystemRoot").map(PathBuf::from).as_deref(),
                ),
            };
            let id = PaneId(self.next_id);
            win.tabs.push(Tab::new(String::new(), id));
            win.active = win.tabs.len() - 1;
            let cmd = self.args.cmd.clone();
            self.open(win, id, cmd.as_deref(), "", cwd)?;
            // Said where it is seen: the release build has no console. It
            // stays, as nothing this window does is saved.
            if let Some(e) = lost {
                let text = format!(
                    "The last session did not come back ({e}); it is kept for the next start, and this window is not saved"
                );
                self.set_notice(id, text, None, false);
            }
        }
        self.note_ignored();

        // A dev build is left alone; a scripted run and a separate window
        // have nothing to come back to.
        if self.persist && !cfg!(debug_assertions) {
            restart_after_reboot();
        }
        // Once there is a pane to say it failed in.
        self.global_jump();
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
        // Once ever, a few keys worth knowing.
        if !self.args.scripted()
            && let Some(id) = self.focus_id()
            && session::dir().is_some_and(|d| session::first_time_in(&d, "keys"))
        {
            let text = first_hint(&self.config.keys);
            self.set_notice(id, text, Some(Instant::now() + HINT), true);
        }
        if !self.args.scripted() && !cfg!(debug_assertions) {
            let proxy = self.proxy.clone();
            let look = self.config.check_updates;
            std::thread::spawn(move || {
                std::thread::sleep(UPDATE_FIRST);
                // Ctrl+Shift+U updates with checks off too, so a failed
                // update is shown, and old ones cleared, either way.
                if let Some((v, log)) = crate::update::failed() {
                    let _ = proxy.send_event(UserEvent::Failed(v, log));
                }
                if !look {
                    return;
                }
                loop {
                    // Quiet when it fails, as offline is normal; the
                    // settings panel and Ctrl+Shift+U say why.
                    let found = crate::update::check();
                    if proxy.send_event(UserEvent::Checked(found, false)).is_err() {
                        return;
                    }
                    std::thread::sleep(UPDATE_EVERY);
                }
            });
        }
        // Told once, in the window, as a release build has no console; a
        // test run, a capture say, leaves it for the user's next start.
        if !self.args.scripted()
            && let Some(id) = self.focus_id()
            && let Some(file) = session::take_crash()
        {
            let text = format!(
                "blitz stopped after an internal error last time; details are in {}",
                file.display()
            );
            self.set_notice(id, text, None, false);
        }
        Ok(())
    }

    /// Starts a session for pane `id` and shows `win`, a layout that
    /// already holds it, running `cmd` or else `shell` (see [`App::spawn`]).
    /// Once a session exists, a layout that leaves the new pane, or the one
    /// it split, below the minimum size is refused; panes the window already
    /// made small do not count.
    fn open(
        &mut self,
        win: layout::Window,
        id: PaneId,
        cmd: Option<&str>,
        shell: &str,
        cwd: Option<PathBuf>,
    ) -> Result<(), String> {
        let grids = self.grids(&win);
        if !self.views.is_empty() && no_room(&win, &grids, id, self.focus_id()) {
            return Err("no room for another pane".into());
        }
        self.spawn(id, &grids, cmd, shell, cwd, None)?;
        self.install(win);
        Ok(())
    }

    /// Starts every pane of a saved session, each in its folder, and shows
    /// its layout. A pane whose folder a process cannot start in (too long
    /// a path for one, say) starts where blitz runs instead, and one whose
    /// shell has gone runs the shell in the settings. Starts none if one
    /// still fails.
    fn restore(&mut self, s: &session::State) -> Result<(), String> {
        let (win, panes) = s.layout(self.next_id);
        let grids = self.grids(&win);
        let (nums, next) = session::numbers(&panes.iter().map(|p| p.1.num).collect::<Vec<_>>());
        for ((id, meta), num) in panes.into_iter().zip(nums) {
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
            let (cwd, old) = (start_dir(&meta.cwd), old.as_deref());
            // When the retries fail too, the first failure says why.
            let started =
                (self.spawn(id, &grids, None, &meta.shell, cwd.clone(), old)).or_else(|first| {
                    (self.spawn(id, &grids, None, &meta.shell, None, old))
                        .or_else(|e| {
                            if meta.shell.is_empty() {
                                return Err(e);
                            }
                            self.spawn(id, &grids, None, "", cwd, old)
                        })
                        .map_err(|then| joined(first, then))
                });
            if let Err(e) = started {
                self.views.clear();
                self.next_num = 1;
                return Err(e);
            }
            if let Some(v) = self.views.last_mut() {
                v.num = num;
                if session::is_key(&meta.key) {
                    v.key = meta.key.clone();
                }
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
        // New sessions take their numbers after the restored ones.
        self.next_num = next;
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
    /// in its folder, resuming its Claude Code session. The new session
    /// gets a new id, so nothing still on its way from the old one lands
    /// in it.
    fn restart(&mut self, id: PaneId) {
        let Some(i) = self.views.iter().position(|v| v.pane.id == id) else {
            return;
        };
        let (old, new) = (&self.views[i], PaneId(self.next_id));
        let (cwd, cmd, shell) = (start_dir(&old.pane.cwd), old.cmd.clone(), old.shell.clone());
        let claude = old.pane.claude.clone().filter(|_| cmd.is_none());
        let mut win = self.win.clone();
        win.replace_pane(id, new);
        let grids = self.grids(&win);
        if let Err(e) = self.spawn(new, &grids, cmd.as_deref(), &shell, cwd, None) {
            self.error(id, e);
            return;
        }
        // The new session takes the old one's row in the sidebar.
        self.views.swap_remove(i);
        self.resume(new, claude);
        self.install(win);
    }

    /// Starts a session for pane `id`, sized as `grids` lays it out (or
    /// 80x24 when it has no place there), running `cmd`, or else `shell`, a
    /// `shell` setting (empty for the one in the settings), below `old`,
    /// output saved by [`session::save_output`].
    fn spawn(
        &mut self,
        id: PaneId,
        grids: &[(PaneId, (i32, i32))],
        cmd: Option<&str>,
        shell: &str,
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
        // The pane's own `shell` setting, else the one in the settings.
        let setting = if shell.is_empty() {
            self.config.shell.as_str()
        } else {
            shell
        };
        let launch_of = |program: &str| {
            crate::shell::launch(
                program,
                self.config.shell_integration,
                &token,
                &self.config.env,
            )
        };
        let mut launch = match cmd {
            Some(c) => crate::shell::Launch {
                cmdline: c.to_string(),
                env: Vec::new(),
            },
            None => launch_of(setting),
        };
        // Claude Code loads blitz's hooks from there, with nothing pasted
        // into its settings.
        let plugin = self.plugin.as_ref().map(|dir| {
            let inherited = std::env::var("CLAUDE_CODE_PLUGIN_DIRS").ok();
            let dirs = crate::hook::plugin_dirs(inherited.as_deref(), dir);
            ("CLAUDE_CODE_PLUGIN_DIRS".to_owned(), dirs)
        });
        launch.env.extend(plugin.clone());
        let start = |launch: &crate::shell::Launch| {
            // The integration's own variables win over the user's.
            let env: Vec<_> = (self.config.env.iter().cloned())
                .chain(launch.env.iter().cloned())
                .collect();
            let proxy = self.proxy.clone();
            Pane::spawn(
                id,
                &Spawn {
                    cmdline: &launch.cmdline,
                    env: &env,
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
        };
        let mut fell_back = None;
        let mut pane = match start(&launch) {
            Ok(p) => p,
            // A shell setting that names a missing or mistyped program would
            // fail every pane, and blitz would close as it opened. The shell
            // blitz finds runs instead, and the pane says why.
            Err(e) if cmd.is_none() && !setting.is_empty() => {
                let mut auto = launch_of("");
                auto.env.extend(plugin);
                let p =
                    (start(&auto)).map_err(|e| format!("cannot start {}: {e}", auto.cmdline))?;
                let keys = keymap::keys_for(Action::Settings, &self.config.keys);
                let using = program_name(&auto.cmdline);
                fell_back = Some(shell_failed(setting, &e, &using, keys));
                launch = auto;
                p
            }
            Err(e) => return Err(format!("cannot start {}: {e}", launch.cmdline)),
        };
        let (cw, ch) = self.cell();
        lock(&pane.term).set_cell_px(cw as u16, ch as u16);
        pane.name = program_name(&launch.cmdline);
        self.views.push(View {
            pane,
            snap: Snapshot::default(),
            grid,
            resized: None,
            resize_at: None,
            rect: None,
            notice: None,
            alerted: None,
            toast: None,
            finding_branch: false,
            resume: None,
            prompted: false,
            sync_until: None,
            key,
            cmd: cmd.map(str::to_owned),
            num: self.next_num,
            shell: shell.into(),
            progress: None,
            claude_working: None,
            hooks_seen: false,
            selection: None,
        });
        if let Some(text) = fell_back {
            // It covers the pane's last row, so it stays only until the
            // next key there, having been seen.
            self.error(id, text);
        }
        self.find_branch(id);
        self.next_id = self.next_id.max(id.0 + 1);
        self.next_num = self.next_num.saturating_add(1);
        Ok(())
    }

    /// Shows `win`, a layout whose panes all have sessions.
    fn install(&mut self, win: layout::Window) {
        let before = self.focus_id();
        self.win = win;
        // A divider being dragged is known by its place in the old layout.
        self.mouse.divider = None;
        self.fit_min_size();
        self.focus_moved(before);
    }

    /// Opens a pane running `shell` (see [`App::spawn`]) in a copy of the
    /// layout that `place` changes; tells the user in the focused pane when
    /// that fails. The pane starts in `dir`, or else where the focused one
    /// is.
    fn add(
        &mut self,
        dir: Option<PathBuf>,
        shell: &str,
        place: impl FnOnce(&mut layout::Window, PaneId) -> bool,
    ) {
        let cwd = dir.or_else(|| start_dir(self.current().map_or("", |v| v.pane.cwd.as_str())));
        let id = PaneId(self.next_id);
        let mut win = self.win.clone();
        if !place(&mut win, id) {
            return;
        }
        if let Err(e) = self.open(win, id, None, shell, cwd)
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
        if self.view_mut(id).is_some_and(|v| v.toast.take().is_some()) {
            crate::notify::untoast(id);
        }
        // Dropping the pane closes its pseudoconsole.
        self.views.retain(|v| v.pane.id != id);
        self.taskbar_progress();
        self.taskbar_badge();
        self.keep_awake();
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
    /// attention, a drag, the find bar and the window title.
    fn focus_moved(&mut self, before: Option<PaneId>) {
        self.request_redraw();
        let now = self.focus_id();
        if now == before {
            return;
        }
        self.orphan = None;
        self.find = None;
        self.quick = None;
        self.set_drag(None);
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
        if let (Some(w), Some(v)) = (&self.watched, self.current()) {
            *lock(w) = v.pane.term.clone();
        }
    }

    /// Puts the focused pane's title and the number of sessions that need
    /// you in the title bar, when either changed.
    fn sync_title(&mut self) {
        let waiting = (self.views.iter())
            .filter(|v| v.pane.attn.state == Attn::NeedsYou)
            .count();
        let pane = self.current().map_or("", |v| v.pane.title.as_str());
        let title = window_title(waiting, pane, self.admin);
        if title != self.title
            && let Some(w) = &self.window
        {
            w.set_title(&title);
            self.title = title;
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

    fn current_mut(&mut self) -> Option<&mut View> {
        self.focus_id().and_then(|id| self.view_mut(id))
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

    /// Cell size of the chrome font.
    fn text_cell(&self) -> (u32, u32) {
        self.gfx.as_ref().map_or((6, 12), |g| g.r.small_cell())
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
            hover: match self.mouse.over_side {
                Some(Side::Session(id)) => Some(id),
                _ => None,
            },
            ui: self.theme.ui,
            size: (size.width as i32, size.height as i32),
            scale: self.scale as f32,
            text_cell: self.text_cell(),
            term_cell: self.cell(),
            now: Instant::now(),
            banner: banner_text(
                self.update.as_ref(),
                self.banner_note.as_ref().map(|n| n.0.as_str()),
            ),
            preedit,
            picker: self.picker.as_ref().map(|p| chrome::Picker {
                filter: &p.filter,
                items: p.matches(),
                sel: p.sel,
            }),
            settings: self.settings.as_ref().map(|p| chrome::Settings {
                filter: &p.filter,
                rows: p.rows(&self.config, self.update_error.as_deref()),
                sel: p.sel,
                top: p.top,
                error: p.error.as_deref(),
            }),
            commands: self.commands.as_ref().map(|cm| chrome::Commands {
                filter: &cm.filter,
                items: (cm.matches().into_iter())
                    .map(|(pick, label)| {
                        let side = match pick {
                            Pick::Run(a) => keymap::keys_for(a, &self.config.keys),
                            Pick::Show(id) => (cm.sessions.iter().flatten())
                                .find(|s| s.0 == id)
                                .map(|s| s.2.clone()),
                            Pick::Settings | Pick::Shell(_) => None,
                        };
                        (label, side.unwrap_or_default())
                    })
                    .collect(),
                sessions: cm.sessions.is_some(),
                sel: cm.sel,
                rename: cm.rename.map(|r| match r {
                    Rename::Session(_) => "Rename session",
                    Rename::Tab(_) => "Rename tab",
                }),
            }),
            find: self.find.as_ref().map(|f| chrome::FindBar {
                query: &f.query,
                count: f.cur.map(|i| (i + 1, f.found.len())),
                fresh: f.fresh,
                screen_only: self.view(f.pane).is_some_and(|v| v.snap.alt_screen),
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
    /// Windows refuses those and keeps the dark or light frame). So is what
    /// a live resize shows past the last frame.
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
        if let Some(g) = &self.gfx {
            g.chain.set_background(self.theme.pal.bg);
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
        // The badge in the new colours.
        self.badge_shows = None;
        self.taskbar_badge();
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
                        let theme = &self.config.theme;
                        let contrast = crate::theme::contrast_for(theme).is_some();
                        let setting = crate::theme::pick(theme, &t.name, light, contrast);
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
        let fonts = crate::render::font::families().to_vec();
        let themes = crate::theme::all().into_iter().map(|t| t.name).collect();
        self.settings = Some(Panel::new(fonts, crate::shell::choices(), themes));
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
            _ if edit_field(&mut p.filter, k) => p.sel = 0,
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
        let old = &self.config;
        let font = (&c.font_family, c.font_size, c.line_height)
            != (&old.font_family, old.font_size, old.line_height);
        if c.font_size != self.config.font_size {
            self.font_zoom = 0.0;
        }
        let jump = c.global_jump != self.config.global_jump;
        // The banner, and an update left for when blitz closes, go with
        // the checks; looks stop at the next start.
        if self.config.check_updates && !c.check_updates {
            self.update = None;
            self.at_close = kept_at_close(self.at_close.take(), self.updating.is_some());
        }
        let skipped = c.skips_other_lines(&self.config);
        self.config = c;
        if jump {
            self.global_jump();
        }
        self.keep_awake();
        if font {
            self.reload_font();
        }
        // Once per change, not each time the settings panel saves.
        if skipped {
            self.note_ignored();
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
        let (px, base) = self.font_px();
        if let Some(g) = &mut self.gfx {
            let (scale, c) = (self.scale as f32, &self.config);
            if let Err(e) = g.r.set_font(&c.font_family, px, base, scale, c.line_height) {
                eprintln!("blitz: font: {e}");
            }
            g.r.set_scale(scale);
        }
        let (cw, ch) = self.cell();
        for v in &self.views {
            lock(&v.pane.term).set_cell_px(cw as u16, ch as u16);
        }
        self.fit_min_size();
        self.request_redraw();
    }

    /// Makes the font a point bigger or smaller, within the range the
    /// font_size setting allows, or with 0 puts the setting's size back.
    fn font_size(&mut self, by: i8) {
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

    /// A key while the command palette is open: up and down choose an
    /// action, typing narrows the list, Enter runs the action and Esc
    /// closes the palette.
    fn commands_key(&mut self, el: &ActiveEventLoop, k: &KeyInput) {
        let Some(cm) = &mut self.commands else {
            return;
        };
        let page = chrome::PICKER_ROWS as isize;
        match k.vk {
            VK_ESCAPE => self.commands = None,
            VK_RETURN => {
                let picked = cm.matches().get(cm.sel).map(|r| r.0.clone());
                let typed = cm.filter.trim().to_string();
                let rename = cm.rename.map(|r| (r, crate::hook::one_line(&cm.filter)));
                self.commands = None;
                if let Some((r, name)) = rename {
                    self.rename(r, name);
                } else if let Some(p) = picked {
                    self.pick(el, p, typed);
                }
            }
            VK_UP => cm.move_by(-1),
            VK_DOWN => cm.move_by(1),
            VK_PRIOR => cm.move_by(-page),
            VK_NEXT => cm.move_by(page),
            _ if edit_field(&mut cm.filter, k) => cm.sel = 0,
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
        let Some(cm) = self.commands.take() else {
            return;
        };
        self.request_redraw();
        if let Some((p, _)) = cm.matches().into_iter().nth(i) {
            self.pick(el, p, cm.filter.trim().to_string());
        }
    }

    /// Does what a row picked in the command palette does, where `typed`
    /// is what was typed there. An action with nothing to do says so.
    fn pick(&mut self, el: &ActiveEventLoop, p: Pick, typed: String) {
        match p {
            Pick::Run(a) => {
                if !self.act(el, a)
                    && let Some(id) = self.focus_id()
                {
                    let text = format!("Nothing to do: {}", keymap::label(a));
                    self.set_notice(id, text, Some(Instant::now() + NOTHING), true);
                }
            }
            Pick::Show(id) => self.show(id),
            Pick::Shell(shell) => self.add(None, &shell, new_tab),
            Pick::Settings => {
                self.open_settings();
                if let Some(p) = &mut self.settings {
                    p.filter = typed;
                }
            }
        }
    }

    /// A key while quick select's labels show: a label copies what it
    /// marks, and with Shift opens it; any other key puts them away.
    fn quick_key(&mut self, k: &KeyInput) {
        if matches!(
            k.key,
            vt::Key::Shift | vt::Key::Control | vt::Key::Alt | vt::Key::Super
        ) {
            return;
        }
        let Some(q) = self.quick.take() else {
            return;
        };
        self.request_redraw();
        let Some((i, open)) = quick_label(k) else {
            return;
        };
        let Some((_, text, target)) = q.items.get(i) else {
            return;
        };
        if open && let Some(t) = target {
            self.open_link(t);
            return;
        }
        let hwnd = HWND(self.hwnd as *mut c_void);
        let said = if crate::clipboard::set_text(Some(hwnd), text) {
            format!("Copied {text}")
        } else {
            "Could not copy to the clipboard".into()
        };
        self.set_notice(q.pane, said, Some(Instant::now() + NOTICE), true);
    }

    /// A key while the find bar is open: typing searches as it goes, Enter
    /// or F3 goes to the next match up, with Shift the next one down, and
    /// Esc closes the bar, leaving the view where it is and the current
    /// match selected, ready to copy.
    fn find_key(&mut self, k: &KeyInput) {
        let Some(f) = &mut self.find else {
            return;
        };
        let m = &k.mods;
        // Matches are oldest first, so up is back through the list.
        let by = if m.lshift || m.rshift { 1 } else { -1 };
        match k.vk {
            VK_ESCAPE => {
                let pal = self.theme.pal;
                if let Some(mut f) = self.find.take()
                    && let Some(v) = self.views.iter_mut().find(|v| v.pane.id == f.pane)
                {
                    let term = lock(&v.pane.term);
                    if f.stale {
                        f.search(&term, v.grid.1);
                    }
                    let found = f.cur.map(|i| f.found[i]);
                    let sel = found.map(|m| selection_of(&term, &pal, m.start, m.end));
                    drop(term);
                    v.selection = sel.or(v.selection.take());
                }
                self.request_redraw();
            }
            VK_RETURN | VK_F3 => self.find_go(false, by),
            _ if f.edit(k) => self.find_go(true, 0),
            _ => {}
        }
    }

    /// Opens the find bar on pane `id` with the first line of its
    /// selection as the query, or else the last one, drawn selected so
    /// typing replaces it. The current match is the selected text.
    fn open_find(&mut self, id: PaneId) {
        let mut f = Find::new(id);
        let mut at = None;
        if let Some(v) = self.view(id) {
            let term = lock(&v.pane.term);
            if let Some(s) = v.selection.as_ref().filter(|s| s.kept(&term)) {
                let text = selection_text(&term, &self.theme.pal, s, 0);
                f.query = text.lines().next().unwrap_or_default().trim().into();
                at = Some(s.start);
            }
        }
        if f.query.is_empty() {
            f.query = self.find_last.clone();
        }
        f.fresh = !f.query.is_empty();
        self.find = Some(f);
        let v = self.views.iter().find(|v| v.pane.id == id);
        if let (Some(f), Some(v)) = (&mut self.find, v) {
            f.search(&lock(&v.pane.term), v.grid.1);
            let picked = f.found.iter().position(|m| Some(m.start) == at);
            f.cur = picked.or(f.cur);
        }
        self.find_go(false, 0);
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
        if search {
            self.find_last.clone_from(&f.query);
        }
        f.step(by);
        let shown = f.cur.map(|i| f.found[i]);
        if let Some(m) = shown {
            reveal(&mut term, m, v.grid.1);
        }
        drop(term);
        self.request_redraw();
    }

    /// Typed or pasted text for the filter of the command palette, the
    /// theme picker or the settings panel, or for the find bar. False when
    /// none is open.
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
            f.type_text(t);
            self.find_go(true, 0);
        } else if self.quick.take().is_some() {
            self.request_redraw();
        } else {
            return false;
        }
        true
    }

    /// The size in cells of each pane of `win` in the window as it is now.
    fn grids(&self, win: &layout::Window) -> Vec<(PaneId, (i32, i32))> {
        tab_grids(win, self.cell(), |w| {
            chrome::build(&self.model(w, &[], None)).panes
        })
    }

    /// Every session as the sidebar shows it.
    fn sessions(&self) -> Vec<chrome::Session> {
        // Only a pane on screen shows how far it is scrolled back; the
        // others' terminals are left to their output.
        let shown = (self.win.tabs.get(self.win.active)).map_or_else(Vec::new, |t| t.panes());
        let mut list: Vec<chrome::Session> = (self.views.iter())
            .map(|v| {
                let p = &v.pane;
                let claude = p.claude_title.is_some();
                let (name, msg) = label(p.named.as_deref(), &p.name, &p.title, claude, &p.msg);
                chrome::Session {
                    id: p.id,
                    name,
                    num: Some(v.num),
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
                    below: if shown.contains(&p.id) {
                        lock(&p.term).viewport()
                    } else {
                        0
                    },
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
        let (c, (px, base), scale) = (&self.config, self.font_px(), self.scale as f32);
        let built = match early {
            Some(gpu) => Ok(gpu),
            None => Gpu::new(false),
        }
        .and_then(|gpu| Renderer::with_gpu(gpu, &c.font_family, px, base, scale, c.line_height));
        let built = built.and_then(|mut r| {
            r.set_scale(scale);
            let hwnd = HWND(self.hwnd as *mut c_void);
            let chain = Swapchain::new(&r.gpu, hwnd, size.width, size.height)?;
            chain.set_background(self.theme.pal.bg);
            Ok(Gfx { r, chain })
        });
        match built {
            Ok(g) => {
                self.gfx = Some(g);
                self.gfx_retry = None;
                self.fit_min_size();
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

    /// Starts or ends the drag that makes a selection in the focused pane,
    /// whose view holds still meanwhile, so output does not slide the text
    /// out from under the pointer.
    fn set_drag(&mut self, drag: Option<Drag>) {
        self.mouse.drag = drag;
        let focus = self.focus_id();
        for v in &self.views {
            lock(&v.pane.term).hold(drag.is_some() && Some(v.pane.id) == focus);
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

    /// Hides the banner until a newer release, in this run and the next,
    /// and drops an update left for when blitz closes.
    fn dismiss_update(&mut self) {
        self.at_close = kept_at_close(self.at_close.take(), self.updating.is_some());
        if let Some((v, _)) = self.update.take() {
            if let Some(dir) = session::dir() {
                crate::update::dismiss_in(&dir, Some(&v));
            }
            self.request_redraw();
        }
    }

    /// Opens the notes of the release the banner shows.
    fn open_notes(&mut self) {
        let Some((v, _)) = &self.update else {
            return;
        };
        let url = crate::update::notes(v);
        if !crate::update::open(&url)
            && let Some(id) = self.focus_id()
        {
            let text = format!("Could not open a browser; see {url}");
            self.set_notice(id, text, Some(Instant::now() + NOTICE), false);
        }
    }

    /// Whether release `v`, found without Ctrl+Shift+U, or the update to
    /// it that `failed`, gets the banner: not after checks were turned off,
    /// and not one whose banner was closed.
    fn unasked(&self, v: &str, failed: bool) -> bool {
        let closed = session::dir().and_then(|d| crate::update::dismissed_in(&d));
        crate::update::show_unasked(v, failed, self.config.check_updates, closed.as_deref())
    }

    /// Updates to release `v` now, downloading its installer unless
    /// `installer` is it. blitz exits once the installer starts.
    fn update_now(&self, v: String, installer: Option<crate::update::Installer>) {
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let done = std::panic::catch_unwind(move || {
                let installer = match installer {
                    Some(p) => p,
                    None => crate::update::fetch(&v)?,
                };
                crate::update::run(&installer, true)
            })
            .unwrap_or_else(|_| Err(INTERNAL.into()));
            let _ = proxy.send_event(UserEvent::Installed(done));
        });
    }

    /// The installer of release `v`, to run when blitz closes, finished
    /// downloading or failed to. If Ctrl+Shift+U asked to update now in
    /// the meantime, it runs now.
    fn fetched(&mut self, v: String, got: Result<crate::update::Installer, String>) {
        // Dropped in the meantime.
        if self.at_close.as_ref().is_none_or(|a| a.0 != v) {
            return;
        }
        match got {
            Ok(installer) if self.updating.is_some() => {
                self.at_close = None;
                self.update_now(v, Some(installer));
            }
            Ok(installer) => self.at_close = Some((v, Some(installer))),
            Err(e) => {
                self.at_close = None;
                self.banner_note = None;
                let text = format!("Update failed: {e}");
                self.update_error = Some(text.clone());
                // Offered as before.
                self.update = None;
                self.offer_update(v, None);
                if let Some(id) = self.updating.take().or_else(|| self.focus_id()) {
                    self.error(id, text);
                }
            }
        }
    }

    /// Shows the banner for release `v`, or for the update to it that
    /// failed and wrote `log`.
    fn offer_update(&mut self, v: String, log: Option<PathBuf>) {
        let installed = crate::update::installed();
        let shown = self.update.as_ref();
        let keys = keymap::press_for(Action::Update, &self.config.keys);
        if let Some(text) = crate::update::banner(shown, &v, log.as_deref(), installed, &keys) {
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
        if let Some(v) = self.view_mut(id) {
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
        // The update's question in the banner goes the same way.
        gone |= (self.banner_note.take_if(|n| n.1.gone(a, true))).is_some();
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
                        self.hide_pointer(true);
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
            // Shift takes the mouse back from a program.
            self.update_pointer();
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
        // A paste key pastes into the open panel's text field; blitz run
        // has none, and a shortcut closes it.
        if panel
            && self.game.is_none()
            && k.down
            && keymap::action(k, &self.config.keys) == Some(Action::Paste)
        {
            self.eaten.press(k.vk);
            if let Some(text) = crate::clipboard::get_text() {
                self.filter_text(&first_line(&text));
            }
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
            match keymap::action(k, &self.config.keys) {
                Some(Action::Palette | Action::GoToSession) => {
                    self.commands = None;
                    self.request_redraw();
                }
                _ => self.commands_key(el, k),
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
        // And for quick select.
        if self.quick.is_some() && k.down {
            self.eaten.press(k.vk);
            if keymap::action(k, &self.config.keys) == Some(Action::QuickSelect) {
                self.quick = None;
                self.request_redraw();
            } else {
                self.quick_key(k);
            }
            return;
        }
        // And for the find bar, but another shortcut runs, and closes it
        // unless it scrolls.
        if self.find.is_some() && k.down {
            self.eaten.press(k.vk);
            match keymap::action(k, &self.config.keys).filter(|&a| find_runs(a)) {
                Some(Action::Find) => {
                    self.find = None;
                    self.request_redraw();
                }
                Some(a) => {
                    if self.act(el, a) && !find_keeps(a) {
                        self.find = None;
                        self.request_redraw();
                    }
                }
                None => self.find_key(k),
            }
            return;
        }
        let modifier = matches!(
            k.key,
            vt::Key::Shift | vt::Key::Control | vt::Key::Alt | vt::Key::Super
        );
        let action = keymap::action(k, &self.config.keys);
        // A copy key with no selection in view goes to the program.
        let hidden = action == Some(Action::Copy) && !copy_key_copies(k, self.selection_shown());
        let action = action.filter(|_| !hidden);
        if k.down && !modifier {
            self.dismiss(action);
        }
        if let Some(a) = action
            && (self.act(el, a) || !keymap::passes_on(a))
        {
            self.eaten.press(k.vk);
            return;
        }
        if action == Some(Action::Copy) && eats_copy_key(k, &self.modes()) {
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
            self.hide_pointer(true);
            self.typed(out);
        } else {
            self.send(out);
        }
    }

    /// Sends input the user typed: the view follows the cursor again, and
    /// it answers what the session asked.
    fn typed(&mut self, bytes: Vec<u8>) {
        let Some(v) = self.current_mut() else {
            return;
        };
        let selected = v.selection.take().is_some();
        lock(&v.pane.term).scroll_viewport(isize::MIN);
        v.pane.send(bytes);
        let id = v.pane.id;
        if selected {
            self.request_redraw();
        }
        // ponytail: from when blitz handles the key, which leaves out
        // its wait in the message queue
        self.counters.typed(Instant::now());
        self.orphan = None;
        self.answered(id);
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

    /// Pastes `text` into pane `id`, the focused one, or asks first as
    /// [`vt::keys::needs_paste_confirm`] says; `confirmed` when this is the
    /// answer. A single line goes without its line break, so it is not run.
    fn paste(&mut self, id: PaneId, text: &str, confirmed: bool) {
        let text = trim_paste(text);
        let Some(v) = self.view(id).filter(|_| !text.is_empty()) else {
            return;
        };
        let label = format!("{} {}", v.pane.name, id.0);
        if let Some(refused) = paste_refused(&label, v.pane.exit_code) {
            self.set_notice(id, refused, None, false);
            return;
        }
        let claude = v.pane.claude.is_some();
        let mut term = lock(&v.pane.term);
        let bracketed = term.input_modes().bracketed;
        let trusted = paste_trusted(&term, claude);
        if confirmed {
            term.confirm_paste();
        }
        drop(term);
        if !confirmed && vt::keys::needs_paste_confirm(text, bracketed, trusted) {
            let key = keymap::keys_for(Action::Paste, &self.config.keys);
            let asked = paste_question(text, key.as_deref());
            self.ask(id, asked, Ask::Paste(text.to_owned()));
            return;
        }
        let mut out = Vec::new();
        vt::encode_paste(text, bracketed, &mut out);
        self.typed(out);
    }

    /// A paste with only an image on the clipboard, such as a screenshot.
    /// Claude Code pastes one on Alt+V, so its pane gets that. False for
    /// any other pane, where the key goes on to the program.
    fn paste_image(&mut self, id: PaneId) -> bool {
        let claude = |v: &&View| v.pane.claude.is_some() && v.pane.exit_code.is_none();
        let Some(v) = self.view(id).filter(claude) else {
            return false;
        };
        if !crate::clipboard::has_image() {
            return false;
        }
        let keys = alt_v(&lock(&v.pane.term).input_modes());
        self.typed(keys);
        true
    }

    /// Whether the selection shows in the focused pane's view, when there
    /// is one.
    fn selection_shown(&self) -> Option<bool> {
        let v = self.current()?;
        let s = v.selection.as_ref()?;
        let top = lock(&v.pane.term).view_top();
        Some(in_view(s.start, s.end, s.drag.block, top, v.grid).is_some())
    }

    /// Copies the selection in the focused pane, or else the text of one
    /// that output just rewrote. False when there is neither, so the key
    /// goes on to the program. When another program holds the clipboard
    /// the selection stays, to copy again. With `unindent`, the text goes
    /// as [`without_indent`] leaves it.
    fn copy(&mut self, unindent: bool) -> bool {
        let Some(v) = self.current() else {
            return false;
        };
        let id = v.pane.id;
        let term = lock(&v.pane.term);
        let (text, rewritten) = match v.selection.as_ref().filter(|s| s.kept(&term)) {
            Some(sel) => (selection_text(&term, &self.theme.pal, sel, 0), false),
            None => match &self.orphan {
                Some(t) => (t.clone(), true),
                None => return false,
            },
        };
        drop(term);
        let text = if unindent {
            without_indent(&text)
        } else {
            text
        };
        let owner = Some(HWND(self.hwnd as *mut c_void));
        let copied = crate::clipboard::set_text(owner, &text);
        if copied {
            if let Some(v) = self.current_mut() {
                v.selection = None;
            }
            self.orphan = None;
        }
        self.notice_copy(id, copy_notice(&text, copied, rewritten), copied);
        true
    }

    /// Says in pane `id` what a copy did: briefly and dimly when it
    /// worked, a while longer when it did not. A brief notice never hides
    /// one that stays up, such as how to close a pane that exited.
    fn notice_copy(&mut self, id: PaneId, text: String, worked: bool) {
        let stays = |v: &View| v.notice.as_ref().is_some_and(|n| n.until.is_none());
        if worked && self.view(id).is_some_and(stays) {
            return;
        }
        let shown = if worked { BRIEF } else { NOTICE };
        self.set_notice(id, text, Some(Instant::now() + shown), worked);
    }

    /// Runs a shortcut. Returns false when it does not apply right now, in
    /// which case the key goes to the program if [`keymap::passes_on`].
    fn act(&mut self, el: &ActiveEventLoop, a: Action) -> bool {
        let before = self.focus_id();
        match a {
            Action::Copy => return self.copy(false),
            Action::CopyUnindented => return self.copy(true),
            Action::Paste => {
                let Some(id) = before else {
                    return false;
                };
                // The answer to a question pastes what it asked about.
                let asked = (self.view_mut(id))
                    .and_then(|v| v.notice.take_if(|n| matches!(n.ask, Ask::Paste(_))));
                if let Some(Notice {
                    ask: Ask::Paste(text),
                    ..
                }) = asked
                {
                    self.request_redraw();
                    self.paste(id, &text, true);
                    return true;
                }
                let text = match crate::clipboard::get_text().filter(|t| !t.is_empty()) {
                    Some(t) => t,
                    // Files copied in Explorer paste as their paths.
                    None => match crate::clipboard::get_files() {
                        Some(files) => quote_paths(&files),
                        None => return self.paste_image(id),
                    },
                };
                self.paste(id, &text, false);
            }
            Action::QuickSelect => {
                let Some(v) = self.current() else {
                    return false;
                };
                let term = lock(&v.pane.term);
                let cwd = &v.pane.cwd;
                let resolve = |p: &str| crate::links::resolve(p, cwd);
                let items = quick_items(&term, &self.theme.pal, v.grid.1, resolve);
                let (id, epoch) = (v.pane.id, term.line_epoch());
                drop(term);
                if items.is_empty() {
                    let until = Some(Instant::now() + NOTICE);
                    self.set_notice(id, "No links, paths or hashes in view", until, true);
                    return true;
                }
                self.find = None;
                self.quick = Some(Quick {
                    pane: id,
                    epoch,
                    items,
                });
                self.request_redraw();
            }
            Action::SelectAll | Action::SelectOutput => {
                let pal = self.theme.pal;
                let Some(v) = self.current_mut() else {
                    return false;
                };
                let mut term = lock(&v.pane.term);
                let found = match a {
                    Action::SelectAll => all_text(&term, &pal),
                    _ => last_output(&term, &pal),
                };
                let Some((first, last)) = found else {
                    return false;
                };
                let sel = selection_of(&term, &pal, (first, 0), (last, u16::MAX));
                if a == Action::SelectOutput {
                    let m = Found {
                        start: (first, 0),
                        end: (last, 0),
                    };
                    reveal(&mut term, m, v.grid.1);
                }
                drop(term);
                v.selection = Some(sel);
                self.request_redraw();
            }
            Action::ScrollPage(dir) => {
                if self.modes().alt_screen {
                    return false;
                }
                let rows = self.current().map_or(1, |v| v.grid.1);
                let page = rows.saturating_sub(1).max(1) as isize;
                self.scroll(page * isize::from(dir));
            }
            Action::ScrollEnd(dir) => {
                if self.modes().alt_screen {
                    return false;
                }
                self.scroll(if dir > 0 { isize::MAX } else { isize::MIN });
            }
            Action::Reset => {
                let Some(v) = self.current() else {
                    return false;
                };
                lock(&v.pane.term).reset_modes();
                self.request_redraw();
            }
            // A full-screen program's screen would be cleared under it.
            Action::ClearScrollback => {
                let Some(v) = self.current().filter(|_| !self.modes().alt_screen) else {
                    return false;
                };
                lock(&v.pane.term).clear_scrollback();
                // The console host draws the screen again after this.
                v.pane.pty.clear();
                if let Some(f) = self.find.as_mut() {
                    f.stale = true;
                }
                self.request_redraw();
            }
            Action::NewTab => self.add(None, "", new_tab),
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
            Action::MoveTab(by) => {
                if !self.win.move_tab(by.into()) {
                    return false;
                }
                self.request_redraw();
            }
            Action::PaneToNewTab => {
                let Some(id) = before else {
                    return false;
                };
                // Unnamed, the new tab goes by the pane's folder.
                if !self.win.pane_to_new_tab(id, String::new()) {
                    return false;
                }
                // A divider being dragged is known by its place in the old
                // layout.
                self.mouse.divider = None;
                self.request_redraw();
            }
            Action::GoToTab(_) | Action::LastTab => {
                let i = match a {
                    Action::GoToTab(i) => usize::from(i),
                    _ => self.win.tabs.len().saturating_sub(1),
                };
                if i < self.win.tabs.len() {
                    self.win.active = i;
                    self.focus_moved(before);
                }
            }
            Action::SplitRight | Action::SplitDown => {
                let dir = if a == Action::SplitRight {
                    Dir::Right
                } else {
                    Dir::Down
                };
                self.add(None, "", split(dir));
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
                self.add(start_dir(&cwd), "", split(Dir::Right));
                if self.view(id).is_some() {
                    self.resume(id, claude);
                } else {
                    self.closed = Some((cwd, claude));
                }
            }
            // Typed at the shell's first prompt, as a restored session is.
            Action::NewClaude => {
                let id = PaneId(self.next_id);
                self.act(el, Action::SplitRight);
                if let Some(v) = self.view_mut(id) {
                    v.resume = Some(("claude\r".into(), Instant::now() + RESUME_AFTER));
                }
            }
            Action::ToggleSidebar => {
                if !self.win.toggle_sidebar() {
                    return false;
                }
                self.fit_min_size();
                self.request_redraw();
            }
            Action::ThemePicker => self.open_picker(),
            Action::Settings => self.open_settings(),
            Action::GoToSession => {
                let now = Instant::now();
                let list = (self.sessions().iter())
                    .map(|s| (s.id, session_row(s), chrome::state_word(s, now)))
                    .collect();
                self.commands = Some(Commands {
                    sessions: Some(list),
                    ..Commands::default()
                });
                self.request_redraw();
            }
            Action::OpenConfig => {
                let opened = (crate::config::file())
                    .map_err(|e| format!("Cannot make config.toml: {e}"))
                    .and_then(|p| crate::links::edit(&p).map_err(String::from));
                self.tell(opened);
            }
            Action::OpenThemes => {
                let opened = match crate::theme::dir() {
                    Some(d) => (std::fs::create_dir_all(&d))
                        .map_err(|e| format!("Cannot make the themes folder: {e}"))
                        .and_then(|()| crate::links::show_folder(&d).map_err(String::from)),
                    None => Err("Cannot find the themes folder: APPDATA is not set".into()),
                };
                self.tell(opened);
            }
            Action::Palette => {
                // Update without a release would only look for one.
                let hidden = match self.update {
                    None => vec![Action::Update],
                    Some(_) => Vec::new(),
                };
                self.commands = Some(Commands {
                    hidden,
                    ..Commands::new(crate::shell::choices())
                });
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
            Action::JumpToAttention => self.jump(),
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
                        let _ = proxy.send_event(UserEvent::Checked(found, true));
                    });
                    return true;
                };
                if !crate::update::installed() {
                    if !crate::update::open(crate::update::PAGE) {
                        let text = format!(
                            "Could not open a browser; get it at {}",
                            crate::update::PAGE
                        );
                        self.error(id, text);
                    }
                    return true;
                }
                // Updating restarts blitz, which ends every session, and
                // the installer closes every other blitz window.
                let busy = self.views.iter().filter(|v| v.busy().is_some()).count();
                let asked = self.banner_note.take_if(|n| n.1 == Ask::Update);
                let again = again(a, &self.config.keys);
                let (others, main) = (crate::handoff::others(), !self.args.new_window);
                let ask = crate::update::confirm(&v, busy, others, main, &again);
                if let Some((text, later)) = ask.filter(|_| asked.is_none()) {
                    self.banner_note = Some((text, Ask::Update));
                    self.request_redraw();
                    if later && arms(self.at_close.as_ref(), &v) {
                        self.at_close = Some((v.clone(), None));
                        let keys = keymap::press_for(Action::Update, &self.config.keys);
                        self.update = Some((v.clone(), crate::update::at_close(&v, &keys)));
                        let proxy = self.proxy.clone();
                        std::thread::spawn(move || {
                            let got = std::panic::catch_unwind(|| crate::update::fetch(&v))
                                .unwrap_or_else(|_| Err(INTERNAL.into()));
                            let _ = proxy.send_event(UserEvent::Fetched(v, got));
                        });
                    }
                    return true;
                }
                // Asked for by hand: shown again if it fails.
                if let Some(dir) = session::dir() {
                    crate::update::dismiss_in(&dir, None);
                }
                self.updating = Some(id);
                let text = format!("Downloading blitz {v}\u{2026}");
                self.banner_note = Some((text, Ask::Nothing));
                self.request_redraw();
                match self.at_close.take() {
                    Some((w, Some(installer))) if w == v => self.update_now(v, Some(installer)),
                    // Its download starts the installer when it lands.
                    Some((w, None)) if w == v => self.at_close = Some((w, None)),
                    _ => self.update_now(v, None),
                }
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
            Action::FontSize(by) => self.font_size(by),
            // Borderless on the window's monitor. winit puts the window back
            // where it was; the session keeps that place, not the monitor's.
            Action::Fullscreen => {
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
                self.open_find(id);
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
                    rename: Some(rename),
                    ..Commands::default()
                });
                self.request_redraw();
            }
            // Windows opens it on Alt+Space itself, but blitz takes every
            // key before Windows sees it.
            Action::SystemMenu => {
                use windows::Win32::Foundation::{LPARAM, WPARAM};
                use windows::Win32::UI::WindowsAndMessaging::{
                    PostMessageW, SC_KEYMENU, WM_SYSCOMMAND,
                };
                let (menu, space) = (
                    WPARAM(SC_KEYMENU as usize),
                    LPARAM(i32::from(b' ') as isize),
                );
                let hwnd = HWND(self.hwnd as *mut c_void);
                // SAFETY: our own window, and a message that carries no pointers.
                let _ = unsafe { PostMessageW(Some(hwnd), WM_SYSCOMMAND, menu, space) };
            }
            Action::SendText(i) => {
                let Some(text) = self.config.texts.get(usize::from(i)) else {
                    return false;
                };
                self.typed(text.clone());
            }
            Action::ReportIssue => self.report_issue(),
        }
        true
    }

    /// Opens a new GitHub issue with what a report needs to know about
    /// this blitz filled in.
    fn report_issue(&mut self) {
        let renderer = match self.gfx.as_ref().map(|g| g.r.gpu.warp) {
            Some(false) => "Direct3D 11",
            Some(true) => "Direct3D 11 WARP, in software",
            None => "none",
        };
        let conpty = match crate::pty::inbox_notice() {
            Some(_) => "the Windows console host",
            None => "bundled",
        };
        let hooks = if self.hooked { "seen" } else { "not seen" };
        let windows = windows_version();
        let facts = [
            ("blitz", env!("CARGO_PKG_VERSION")),
            ("Windows", &windows),
            ("renderer", renderer),
            ("ConPTY", conpty),
            ("Claude Code hooks", hooks),
            (
                "last update error",
                self.update_error.as_deref().unwrap_or("none"),
            ),
        ];
        let url = crate::update::issue(&facts);
        if !crate::update::open(&url)
            && let Some(id) = self.focus_id()
        {
            let text = "Could not open a browser to report an issue";
            self.set_notice(id, text, Some(Instant::now() + NOTICE), false);
        }
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
                if self.focus_id() == Some(id) {
                    self.counters.output();
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
                self.set_notice(
                    id,
                    format!("{} \u{b7} Enter restart \u{b7} Esc close", exit_text(code)),
                    None,
                    false,
                );
            }
            // The selection and link under the pointer end with the line
            // numbers they used; matches are found again.
            // The console host sends the screen again, and events parsed
            // before the panic, such as a notification, are taken now.
            Note::Reset => {
                v.pane.repaint(v.grid.0, v.grid.1);
                if let Some(f) = self.find.as_mut().filter(|f| f.pane == id) {
                    f.stale = true;
                }
                self.on_pane(el, id, Note::Dirty);
                let text = "the screen was cleared after an internal error";
                self.set_notice(id, text, Some(Instant::now() + NOTICE), false);
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
                let asked = v.pane.attn.state == Attn::NeedsYou;
                v.pane.claude_title = now;
                v.pane.title = t;
                // This needs no hooks, and it sees a turn the user
                // interrupted end, which runs no hook at all.
                match (was, now) {
                    // Back at work: a question it showed was answered.
                    (w, Some(true)) if w != Some(true) => {
                        if self.attention(id, Ev::Busy)
                            && asked
                            && let Some(v) = self.view_mut(id)
                        {
                            v.pane.msg.clear();
                        }
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
                // A long command that ended while the user looked away.
                if let Some((ev, msg)) = ended
                    && self.attention(id, ev)
                    && let Some(v) = self.view_mut(id)
                {
                    v.pane.msg = msg;
                }
            }
            Event::Notify { title, body } => match Ev::from_notify(&title, &v.pane.token) {
                Some((ev, session)) => {
                    v.hooks_seen = true;
                    v.pane.cmd.hooked = true;
                    note_hook(&mut v.pane.msg, &mut v.pane.claude, ev, session, body);
                    v.pane.hooked = ev != Ev::Idle;
                    self.hooked = true;
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
            // OSC 52. The pane says so, as any program can write there.
            Event::Clipboard(text) => {
                let owner = Some(HWND(self.hwnd as *mut c_void));
                let copied = crate::clipboard::set_text(owner, &text);
                self.notice_copy(id, program_copy_notice(&text, copied), copied);
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
        let Some(t) = self.taskbar() else {
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

    /// The taskbar button, made the first time it is needed.
    fn taskbar(&mut self) -> Option<ITaskbarList3> {
        if self.taskbar.is_none() {
            // SAFETY: COM calls on the window's thread, where winit has
            // started OLE for drag and drop.
            self.taskbar = unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) }
                .ok()
                .filter(|t: &ITaskbarList3| unsafe { t.HrInit() }.is_ok());
        }
        self.taskbar.clone()
    }

    /// Badges the taskbar button with the sidebar dot of the session that
    /// most wants the user, or clears it once none does.
    fn taskbar_badge(&mut self) {
        let top = badge_state(self.views.iter().map(|v| v.pane.attn.state));
        if top == self.badge_shows {
            return;
        }
        let Some(t) = self.taskbar() else {
            return;
        };
        self.badge_shows = top;
        let ui = &self.theme.ui;
        let (fg, ring, label) = badge_look(top, ui);
        let icon = top.and_then(|_| {
            let size = small_icon_size().width;
            crate::notify::badge_icon(size, fg, ui.term_bg, ring)
        });
        let hwnd = HWND(self.hwnd as *mut c_void);
        // SAFETY: a live window; the taskbar keeps a copy of the icon, so
        // it is destroyed once set.
        unsafe {
            let _ = t.SetOverlayIcon(hwnd, icon.unwrap_or_default(), &HSTRING::from(label));
            if let Some(i) = icon {
                let _ = DestroyIcon(i);
            }
        }
    }

    /// While `keep_awake` is on and a session works, keeps the PC from going
    /// to sleep by itself; `powercfg /requests` says why.
    fn keep_awake(&mut self) {
        let states = self.views.iter().map(|v| v.pane.attn.state);
        let on = stays_awake(self.config.keep_awake, states);
        if on == self.awake {
            return;
        }
        if self.power.is_none() {
            let mut why: Vec<u16> = "A session is working".encode_utf16().chain([0]).collect();
            let context = REASON_CONTEXT {
                // POWER_REQUEST_CONTEXT_VERSION
                Version: 0,
                Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
                Reason: REASON_CONTEXT_0 {
                    SimpleReasonString: PWSTR(why.as_mut_ptr()),
                },
            };
            // SAFETY: the context and its string outlive the call, which
            // copies them. The request lives as long as blitz.
            self.power = unsafe { PowerCreateRequest(&context) }.ok();
        }
        let Some(r) = self.power else {
            return;
        };
        // SAFETY: a power request blitz made and never closes.
        let done = unsafe {
            if on {
                PowerSetRequest(r, PowerRequestSystemRequired)
            } else {
                PowerClearRequest(r, PowerRequestSystemRequired)
            }
        };
        if done.is_ok() {
            self.awake = on;
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
        let attended = here && self.game.is_none() && self.focus_id() == Some(id);
        let away = !here;
        let Some(v) = self.views.iter_mut().find(|v| v.pane.id == id) else {
            return false;
        };
        let changed = v.pane.attn.apply(ev, attended, now);
        let alert = (changed && away)
            .then(|| alert(v.pane.attn.state, &self.config, &mut v.alerted, now))
            .flatten();
        if untoasts(attended || ev == Ev::Attended, v.pane.attn.state) && v.toast.take().is_some() {
            crate::notify::untoast(id);
        }
        // The sidebar shows the new state, and the taskbar button its dot.
        self.request_redraw();
        if changed {
            self.taskbar_badge();
            self.keep_awake();
        }
        if let Some(a) = alert {
            self.alert(id, a);
        }
        changed
    }

    /// Tells the user, who is in another program, about session `id`.
    fn alert(&mut self, id: PaneId, a: Alert) {
        if a.flashes > 0 {
            crate::notify::flash(self.hwnd, a.flashes);
        }
        // Never from a test run.
        if self.args.scripted() {
            return;
        }
        let shown = a.toast && self.toast(id);
        if a.beeps(shown) {
            // SAFETY: a plain call.
            let _ = unsafe { MessageBeep(MB_OK) };
        }
    }

    /// Shows a Windows notification about session `id`; false when none
    /// was shown.
    fn toast(&mut self, id: PaneId) -> bool {
        let Some(s) = self.sessions().into_iter().find(|s| s.id == id) else {
            return false;
        };
        let lines = toast_text(&s);
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let xml = crate::notify::toast_xml(&lines, !self.config.sound);
        match crate::notify::toast(id, &xml, &self.proxy) {
            Ok(Some(t)) => {
                if let Some(v) = self.view_mut(id) {
                    v.toast = Some(t);
                }
                true
            }
            Ok(None) => false,
            Err(e) => {
                eprintln!("blitz: notification: {e}");
                false
            }
        }
    }

    /// Shows the session that most wants the user; see [`jump_to`]. With
    /// none, goes back to where the jumps started, or says nothing needs
    /// the user.
    fn jump(&mut self) {
        let before = self.focus_id();
        let waiting = (self.views.iter()).map(|v| (v.pane.id, v.pane.attn));
        let target = jump_to(waiting, before, self.focused);
        match jump(before, target, &mut self.jumped).filter(|&id| self.view(id).is_some()) {
            Some(id) => self.show(id),
            None => {
                if let Some(id) = before {
                    let until = Some(Instant::now() + NOTHING);
                    self.set_notice(id, "Nothing needs you", until, true);
                }
            }
        }
    }

    /// Takes Ctrl+Alt+J from every program while `global_jump` is on, in
    /// the main window only, or gives it back. Says so in the focused pane
    /// when another program has it.
    fn global_jump(&mut self) {
        let on = self.config.global_jump && self.persist;
        if on == self.jump_key {
            return;
        }
        let ok = crate::notify::global_jump(self.hwnd, on);
        self.jump_key = on && ok;
        // Another main blitz window has it then, and answers it.
        if jump_key_lost(ok, || crate::notify::other_main(self.hwnd))
            && let Some(id) = self.focus_id()
        {
            let text = "Another program has Ctrl+Alt+J, so it cannot bring you to blitz";
            self.set_notice(id, text, Some(Instant::now() + NOTICE), false);
        }
    }

    /// Brings the window to the front, out of the taskbar if minimized.
    /// Windows lets it when the user just asked for blitz.
    fn to_front(&self) {
        // First, since a minimized window has no room for a pane.
        if let Some(w) = &self.window {
            w.set_minimized(false);
        }
        // SAFETY: our own window.
        let _ = unsafe { SetForegroundWindow(HWND(self.hwnd as *mut c_void)) };
    }

    fn cell_at(&self, pos: PhysicalPosition<f64>) -> (u16, u16) {
        self.focus_id().map_or((0, 0), |id| self.cell_in(id, pos))
    }

    /// The cell of pane `id` under `pos`; `None` off its grid.
    fn grid_cell(&self, id: PaneId, pos: PhysicalPosition<f64>) -> Option<(u16, u16)> {
        let v = self.view(id)?;
        grid_cell(v.rect?, v.grid, self.cell(), (pos.x, pos.y))
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
        let tw = self.text_cell().0 as i32;
        chrome::area(
            &self.win,
            size,
            self.scale as f32,
            self.update.as_ref().map(|u| u.1.as_str()),
            tw,
        )
    }

    /// The session under a point in the window: a pane of the active tab,
    /// or a row of the sidebar. The second value is true for the sidebar.
    fn hit(&self, pos: PhysicalPosition<f64>) -> (Option<PaneId>, bool) {
        let (x, y) = (pos.x as i32, pos.y as i32);
        let inside = |r: &Rect| (r.x..r.right()).contains(&x) && (r.y..r.bottom()).contains(&y);
        let area = self.tab_area();
        let (rects, side) = match self.win.tabs.get(self.win.active) {
            Some(_) if x < area.x => (self.side.rows.clone(), true),
            Some(t) => (t.rects(area), false),
            None => (Vec::new(), false),
        };
        let id = rects.into_iter().find(|(_, r)| inside(r)).map(|(id, _)| id);
        (id, side)
    }

    /// Where the pointer is now, in client pixels. A drag from another
    /// program moves it without telling the window, so `mouse.pos` is old.
    fn pointer(&self) -> Option<PhysicalPosition<f64>> {
        let mut pt = POINT::default();
        // SAFETY: plain queries that write one POINT; a stale window only
        // fails the second.
        unsafe {
            GetCursorPos(&mut pt).ok()?;
            ScreenToClient(HWND(self.hwnd as *mut c_void), &mut pt)
                .ok()
                .ok()?;
        }
        Some(PhysicalPosition::new(f64::from(pt.x), f64::from(pt.y)))
    }

    /// Handles the files dropped since the last turn of the event loop,
    /// where the pointer let go of them. blitz run covers the panes, so
    /// nothing is dropped on them while it is open.
    fn on_drop(&mut self) {
        let paths = std::mem::take(&mut self.dropped);
        if paths.is_empty() || self.game.is_some() {
            return;
        }
        let Some(pos) = self.pointer() else {
            return;
        };
        match dropped(paths, self.hit(pos)) {
            Some(Dropped::Paste(id, text)) => {
                self.show(id);
                self.paste(id, &text, false);
            }
            Some(Dropped::Open(dirs)) => {
                for dir in dirs {
                    self.add(Some(dir), "", new_tab);
                }
            }
            None => {}
        }
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
        let th = self.text_cell().1 as i32;
        pane_min(
            self.cell(),
            th,
            self.scale as f32,
            self.win.sidebar_expanded,
        )
    }

    /// Keeps the window from getting smaller than [`min_window`] for the
    /// layout, font and scale in use.
    fn fit_min_size(&self) {
        if let Some(w) = &self.window {
            let min = min_window(&self.win, self.cell(), self.text_cell(), self.scale as f32);
            w.set_min_inner_size(Some(min));
        }
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

    /// Shows one of `ids`, the sessions the sidebar has no room for (see
    /// [`hidden_target`]).
    fn show_hidden(&mut self, ids: &[PaneId]) {
        let states = (ids.iter()).filter_map(|&id| self.view(id));
        let hidden: Vec<_> = states.map(|v| (v.pane.id, v.pane.attn)).collect();
        if let Some(id) = hidden_target(&hidden, self.focus_id()) {
            self.show(id);
        }
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
        let pressed = state == ElementState::Pressed;
        // Asked for every press, so a click with another button never
        // leaves it for the next one.
        let activating = pressed && crate::handoff::take_activating_click();
        let b = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            _ => return,
        };
        // The click that brings blitz to the front only moves focus. Sent
        // on, it could pick an option of a Claude Code prompt the user has
        // not read yet.
        if activating {
            let open = self.commands.is_some() || self.settings.is_some() || self.game.is_some();
            if let (Some(id), false) = (self.hit(self.mouse.pos).0, open) {
                self.show(id);
            }
            return;
        }
        let mods = mods_now();
        if pressed && self.quick.take().is_some() {
            self.request_redraw();
        }
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
        if let Some(click) = banner_click(self.banner, x, y).filter(|_| pressed && b == 0) {
            match click {
                BannerClick::Close => self.dismiss_update(),
                BannerClick::Notes => self.open_notes(),
            }
            return;
        }
        let chip = (self.below.iter())
            .find(|(_, r)| (r.x..r.right()).contains(&x) && (r.y..r.bottom()).contains(&y));
        if pressed
            && b == 0
            && let Some(&(id, _)) = chip
        {
            if let Some(v) = self.view_mut(id) {
                lock(&v.pane.term).scroll_viewport(isize::MIN);
            }
            self.request_redraw();
            return;
        }
        // A click on the find bar keeps it, where one below it closes it.
        let on_find = (self.find_bar)
            .is_some_and(|r| (r.x..r.right()).contains(&x) && (r.y..r.bottom()).contains(&y));
        if pressed && on_find && self.find.is_some() {
            return;
        }
        if b == 0 && !pressed && self.mouse.divider.take().is_some() {
            self.settle();
            return;
        }
        if let Some((i, _)) = self
            .divider_at(self.mouse.pos)
            .filter(|_| pressed && b == 0)
        {
            // SAFETY: a plain query.
            let within = Duration::from_millis(u64::from(unsafe { GetDoubleClickTime() }));
            if evens(&mut self.mouse.divider_click, i, Instant::now(), within) {
                self.act(el, Action::Equalize);
            } else {
                self.mouse.divider = Some((i, self.min_pane()));
            }
            return;
        }
        // A click in the sidebar goes to what it is on.
        if pressed && x < self.tab_area().x {
            match self.side.at(x, y) {
                Some(Side::Session(id)) => self.show(id),
                Some(Side::Tab(i)) => {
                    if let Some(id) = self.win.tabs.get(i).map(|t| t.focus) {
                        self.show(id);
                    }
                }
                Some(Side::More(ids)) => self.show_hidden(&ids),
                Some(Side::Rail) => {
                    self.win.toggle_sidebar();
                    self.request_redraw();
                }
                None => {}
            }
            return;
        }
        // A click on another pane only moves focus. One in the focused
        // pane closes its find bar and goes on, unless it is on the pane's
        // header or outside the panes, where it does nothing.
        if pressed {
            let id = self.hit(self.mouse.pos).0;
            if let Some(id) = id.filter(|&id| Some(id) != self.focus_id()) {
                self.show(id);
                return;
            }
            if id.is_some() && self.find.take().is_some() {
                self.request_redraw();
            }
            let on_grid = (self.current())
                .and_then(|v| v.rect)
                .is_some_and(|r| (r.x..r.right()).contains(&x) && (r.y..r.bottom()).contains(&y));
            if !on_grid {
                return;
            }
        }
        if pressed
            && b == 0
            && let Some((target, _)) = self.ctrl_link(&mods)
        {
            self.open_link(&target);
            return;
        }
        // The program hears of presses on its cells only, not on the padding
        // past them.
        let program = self.mouse_to_program(&mods).and(self.focus_id());
        if pressed && program.is_some_and(|id| self.grid_cell(id, self.mouse.pos).is_none()) {
            return;
        }
        if let Some(id) = route_button(&mut self.mouse.reported, b, pressed, program) {
            let kind = if pressed {
                MouseKind::Press
            } else {
                MouseKind::Release
            };
            if b == 0 && pressed {
                let plain = mods == Mods::default();
                self.mouse.program_press = Some((self.cell_in(id, self.mouse.pos), plain));
            }
            self.mouse_report(id, kind, b as u8, mods);
            // A click can pick an answer in the program's menu.
            if pressed {
                self.answered(id);
            }
            return;
        }
        if b == 2 && pressed {
            let selected = (self.current()).is_some_and(|v| {
                let term = lock(&v.pane.term);
                v.selection.as_ref().is_some_and(|s| s.kept(&term))
            });
            if let Some(a) = right_click_does(self.config.right_click_paste, selected) {
                self.act(el, a);
            }
        }
        if b != 0 {
            return;
        }
        if pressed {
            // With mouse reporting on, Shift is what brought the click
            // here, so it does not extend.
            let shift = mods.lshift || mods.rshift;
            let extend = shift && self.modes().mouse == MouseMode::Off;
            self.press(&mods, extend);
        } else {
            self.set_drag(None);
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
        self.orphan = None;
        let held = (self.current()).and_then(|v| Some(v.selection.as_ref()?.drag));
        if let Some(drag) = held.filter(|d| extend && d.epoch == epoch) {
            self.set_drag(Some(drag));
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
        self.set_drag(Some(drag));
        if self
            .current_mut()
            .and_then(|v| v.selection.take())
            .is_some()
        {
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
            self.set_drag(None);
            return;
        }
        let head = (term.view_top() + usize::from(row), col);
        if v.selection.is_none() && drag.unit == 1 && head == drag.anchor.0 {
            return;
        }
        let s = Selection::new(&term, &self.theme.pal, drag, head);
        drop(term);
        let Some(v) = self.current_mut() else {
            return;
        };
        if v.selection.as_ref().map(|o| (o.start, o.end)) != Some((s.start, s.end)) {
            v.selection = Some(s);
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
            .filter(|v| self.hit(pos) == (Some(v.pane.id), false))?;
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
            Link::Path(p, at) => Target::Path(crate::links::resolve(&p, &v.pane.cwd)?, at),
        };
        Some((target, l.span(range)))
    }

    /// The link under the pointer that a click with `mods` held opens,
    /// with the cells it covers: Ctrl must be held, and a program that
    /// takes the mouse gets the click unless [`opens_link`] says otherwise.
    fn ctrl_link(&self, mods: &Mods) -> Option<(Target, (Pos, Pos))> {
        if !(mods.lctrl || mods.rctrl) || self.mouse.drag.is_some() {
            return None;
        }
        let found = self.link_under(self.mouse.pos)?;
        let program = self.mouse_to_program(mods).is_some();
        let claude = self.current().is_some_and(|v| v.pane.claude.is_some());
        opens_link(&found.0, program, claude, &self.config.editor_uri).then_some(found)
    }

    /// Underlines the link under the pointer, and shows the hand, while a
    /// click would open it; see [`Self::ctrl_link`].
    fn update_hover(&mut self) {
        let hover = (self.ctrl_link(&mods_now())).and_then(|(_, (a, b))| {
            let epoch = lock(&self.current()?.pane.term).line_epoch();
            Some((epoch, a, b))
        });
        self.set_hover(hover);
    }

    fn set_hover(&mut self, hover: Option<(u32, Pos, Pos)>) {
        if hover == self.hover {
            return;
        }
        self.hover = hover;
        self.update_pointer();
        self.request_redraw();
    }

    /// Notes what in the sidebar the pointer is over: the hand says a
    /// click there does something, and a session's row lights up.
    fn set_over_side(&mut self, over: Option<Side>) {
        if over == self.mouse.over_side {
            return;
        }
        self.mouse.over_side = over;
        self.update_pointer();
        self.request_redraw();
    }

    /// Shows the pointer for what is under it; see [`pointer`].
    fn update_pointer(&mut self) {
        let pos = self.mouse.pos;
        let (x, y) = (pos.x as i32, pos.y as i32);
        let inside = |r: &Rect| (r.x..r.right()).contains(&x) && (r.y..r.bottom()).contains(&y);
        let panel = self.commands.is_some()
            || self.settings.is_some()
            || self.picker.is_some()
            || self.game.is_some();
        let divider = self.divider_at(pos).map(|d| d.1).filter(|_| !panel);
        let hand = self.hover.is_some()
            || self.banner.as_ref().is_some_and(|b| inside(&b.0))
            || self.mouse.over_side.is_some();
        let mods = mods_now();
        let grid = (!panel).then(|| self.hit(pos)).and_then(|(id, side)| {
            let v = self.view(id.filter(|_| !side)?)?;
            v.rect.filter(inside)?;
            let mouse = lock(&v.pane.term).input_modes().mouse;
            Some(mouse != MouseMode::Off && !(mods.lshift || mods.rshift))
        });
        let icon = pointer(divider, hand && !panel, grid);
        if icon != self.mouse.icon {
            self.mouse.icon = icon;
            if let Some(w) = &self.window {
                w.set_cursor(icon);
            }
        }
    }

    /// Hides the pointer while typing, or shows it again, as Windows'
    /// "hide pointer while typing" asks.
    fn hide_pointer(&mut self, hide: bool) {
        if hide != self.mouse.hidden && (self.vanish || !hide) {
            self.mouse.hidden = hide;
            if let Some(w) = &self.window {
                w.set_cursor_visible(!hide);
            }
        }
    }

    /// Opens a link, or says in the pane why not.
    fn open_link(&mut self, target: &Target) {
        self.tell(crate::links::open(target, &self.config.editor_uri).map_err(String::from));
    }

    /// Says, dimly in the focused pane, which lines of `config.toml` were
    /// skipped, if any.
    fn note_ignored(&mut self) {
        if let (Some(text), Some(id)) = (self.config.ignored_notice(), self.focus_id()) {
            self.set_notice(id, text, Some(Instant::now() + NOTICE), true);
        }
    }

    /// Says in the focused pane why something did not open.
    fn tell(&mut self, opened: Result<(), String>) {
        if let Err(e) = opened
            && let Some(id) = self.focus_id()
        {
            self.error(id, e);
        }
    }

    fn on_mouse_move(&mut self, pos: PhysicalPosition<f64>) {
        // Windows may send a move that is none, as when a window opens.
        if pos != self.mouse.pos {
            self.hide_pointer(false);
        }
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
        self.update_pointer();
        // blitz run covers the panes, so programs see no motion under it.
        if self.game.is_some() {
            return;
        }
        let (x, y) = (pos.x as i32, pos.y as i32);
        let panel = self.commands.is_some() || self.settings.is_some();
        let over = (x < self.tab_area().x && !panel).then(|| self.side.at(x, y));
        self.set_over_side(over.flatten());
        self.update_hover();
        let mods = mods_now();
        // A drag goes where its press went, like the release will.
        let held = (0..3).find_map(|b| Some((b, self.mouse.reported[b]?)));
        if let Some((b, id)) = held {
            self.mouse_report(id, MouseKind::Move, b as u8, mods);
            // The first time a plain drag goes to a program, say how to
            // select instead.
            let here = self.cell_in(id, pos);
            let press = &mut self.mouse.program_press;
            if b == 0
                && press.take_if(|p| p.1 && p.0 != here).is_some()
                && session::dir().is_some_and(|d| session::first_time_in(&d, "shift-drag"))
            {
                let text = "Shift+drag selects while the program uses the mouse";
                self.set_notice(id, text, Some(Instant::now() + NOTICE), true);
            }
        } else if self.mouse_to_program(&mods).is_some()
            && let Some(id) = self.focus_id()
            && self.grid_cell(id, pos).is_some()
        {
            // Only over the program's own cells: not while the pointer
            // crosses the sidebar, a header or another pane.
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
        // Ctrl and the wheel change the font size, unless the program takes
        // the mouse.
        let mods = mods_now();
        if (mods.lctrl || mods.rctrl) && self.modes().mouse == MouseMode::Off {
            let now = Instant::now();
            if let Some(by) = wheel_font(steps, self.mouse.font_at, now) {
                self.mouse.font_at = Some(now);
                self.font_size(by);
            }
            return;
        }
        // The pane under the pointer takes the wheel without taking focus:
        // its history scrolls, or a full-screen program there gets the
        // reports at its own cell. Outside the panes it is the focused one's.
        let (under, side) = self.hit(self.mouse.pos);
        if side {
            return;
        }
        let Some(v) = under.or(self.focus_id()).and_then(|id| self.view(id)) else {
            return;
        };
        let id = v.pane.id;
        let focused = Some(id) == self.focus_id();
        let m = lock(&v.pane.term).input_modes();
        let lines = steps as isize * wheel_lines(scroll_lines(), v.grid.1);
        let shift = mods.lshift || mods.rshift;
        match wheel_does(&m, shift, v.pane.claude.is_some(), focused) {
            Wheel::Report => {
                let kind = if steps > 0.0 {
                    MouseKind::WheelUp
                } else {
                    MouseKind::WheelDown
                };
                for _ in 0..steps.abs() as u32 {
                    self.mouse_report(id, kind, 0, mods);
                }
            }
            Wheel::Arrows => self.send(wheel_keys(lines, &m)),
            Wheel::Scroll if focused => self.scroll(lines),
            Wheel::Scroll => {
                lock(&v.pane.term).scroll_viewport(lines);
                self.request_redraw();
            }
            Wheel::Nothing => {}
        }
    }

    /// Draws a frame. Resizes each visible session first when its pane
    /// changed size, at most once per [`RESIZE_GAP`].
    fn redraw(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        // A narrow window has no room for the sidebar.
        self.win.fit_width(size.width as f32 / self.scale as f32);
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
        // Where the cursor is drawn, as of the last frame, else where it
        // is: a program may hide it, or the view may be scrolled.
        let cursor = self.current().map(|v| match v.snap.cursor {
            Some((col, row, _)) => (col, row),
            None => {
                let (col, row, _) = lock(&v.pane.term).cursor();
                (col, row)
            }
        });
        let sessions = self.sessions();
        let preedit = cursor
            .filter(|_| !self.preedit.is_empty())
            .map(|(c, r)| (c, r, self.preedit.as_str()));
        let mut chrome = chrome::build(&self.model(&self.win, &sessions, preedit));
        self.side = std::mem::take(&mut chrome.side);
        self.banner = chrome.banner.zip(chrome.banner_close);
        self.below = std::mem::take(&mut chrome.below);
        self.find_bar = chrome.find;
        self.settings_hits = chrome.settings.take();
        self.commands_hits = chrome.commands.take();
        if let (Some(p), Some(h)) = (&mut self.settings, &self.settings_hits) {
            p.top = h.top;
        }

        let split = chrome.panes.len() >= 2;
        let mut dimmed = Vec::new();
        for v in &mut self.views {
            v.rect = None;
            v.resize_at = None;
        }
        for &(id, rect) in &chrome.panes {
            let Some(v) = self.views.iter_mut().find(|v| v.pane.id == id) else {
                continue;
            };
            let fit = |n: i32, cell: u32| (n / cell as i32).clamp(1, i32::from(u16::MAX)) as u16;
            let grid = (fit(rect.w, cw), fit(rect.h, ch));
            let mut find = self.find.as_mut().filter(|f| f.pane == id);
            if grid != v.grid {
                v.resize_at = resize_wait(v.resized, started);
                if v.resize_at.is_none() {
                    v.grid = grid;
                    v.resized = Some(started);
                    // A selection stays on its text as a new width wraps it.
                    let mut marks = v.selection.as_ref().map_or_else(Vec::new, Selection::marks);
                    let kept = v.pane.resize(grid.0, grid.1, &mut marks);
                    let term = lock(&v.pane.term);
                    if let Some(s) = v.selection.as_mut().filter(|_| kept) {
                        s.reflowed(&term, &self.theme.pal, &marks);
                    }
                    drop(term);
                    lock(&v.pane.term).set_cell_px(cw as u16, ch as u16);
                    // A new width rewraps the lines that matched, which a
                    // search must find again before they are drawn.
                    if let Some(f) = &mut find {
                        (f.stale, f.searched) = (true, None);
                    }
                }
            }
            v.rect = Some(rect);
            let mut term = lock(&v.pane.term);
            v.sync_until = term.sync_deadline();
            if !refresh(
                &mut term,
                &mut v.snap,
                &self.theme.pal,
                v.selection.as_mut(),
            ) {
                let lost = v.selection.take();
                if Some(id) == focus {
                    self.orphan = lost.and_then(|s| s.last_text(&term, &self.theme.pal));
                }
            }
            v.snap.highlights.clear();
            if let Some(f) = find {
                // ponytail: searches all of the scrollback again, ~50 ms for
                // 100,000 full rows, at most every FIND_EVERY; keep the
                // matches in scrollback rows if that ever shows.
                if f.due().is_some_and(|t| t <= Instant::now()) {
                    f.search(&term, v.grid.1);
                }
                v.snap.highlight(&f.found, f.cur);
            }
            if let Some(q) = self.quick.as_ref().filter(|q| q.pane == id) {
                let mut found: Vec<Found> = q.items.iter().map(|i| i.0).collect();
                found.sort();
                v.snap.highlight(&found, None);
            }
            // Only the focused pane has a link under the pointer.
            let hover = (self.hover)
                .filter(|h| Some(id) == focus && h.0 == term.line_epoch())
                .map(|(_, a, b)| (a, b));
            mark(&mut v.snap, term.view_top(), v.selection.as_ref(), hover);
            if Some(id) != focus && chrome::dims(split, v.pane.attn.state) {
                dimmed.push(id);
            }
        }

        // Output that numbered the lines anew leaves the labels on nothing.
        let shown = (self.quick.as_ref()).and_then(|q| {
            let v = self.view(q.pane)?;
            let term = lock(&v.pane.term);
            let view = term.view_top()..term.view_top() + usize::from(v.grid.1);
            (term.line_epoch() == q.epoch).then_some((q, v.rect, view))
        });
        match shown {
            Some((q, Some(r), view)) => {
                let ui = &self.theme.ui;
                for (i, (m, _, _)) in q.items.iter().enumerate() {
                    let ((line, col), (cw, ch)) = (m.start, (cw as i32, ch as i32));
                    if !view.contains(&line) {
                        continue;
                    }
                    let x = r.x + i32::from(col) * cw;
                    let y = r.y + (line - view.start) as i32 * ch;
                    let cell = Rect { x, y, w: cw, h: ch };
                    chrome.prims.push(chrome::Prim::Rect(cell, ui.name));
                    chrome.prims.push(chrome::Prim::Text {
                        x,
                        y,
                        text: char::from(b'a' + i as u8).to_string(),
                        color: ui.side_bg,
                        bold: true,
                        term: true,
                    });
                }
            }
            Some(_) => {}
            None => self.quick = None,
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
                    // A terminal still at its old size until it is resized
                    // shows only what fits its pane.
                    let rows = v.grid.1.min((at.h / ch as i32) as u16);
                    g.r.clipped(at, |r| {
                        r.grid(&v.snap, &pal, at.x, at.y, dim, hollow, scenery.is_none());
                        if let Some(n) = &v.notice {
                            draw_notice(r, &pal, at, (v.grid.0, rows), n);
                        }
                    });
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
            Ok(shown) => {
                self.reveal(true);
                self.counters.frames += 1;
                if shown {
                    self.counters.presented(Instant::now());
                }
                if self.counters.first_present_ms.is_none() {
                    self.counters.first_present_ms =
                        Some(self.started.elapsed().as_secs_f64() * 1000.0);
                    // Ready before the settings panel wants them, and
                    // not in the way of the first frame.
                    std::thread::spawn(crate::render::font::families);
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

        let pane = self.current().and_then(|v| v.rect);
        let cell = (cw as i32, ch as i32);
        if let Some(at) = ime_area(chrome.field, pane, cursor, cell)
            && self.ime_at != Some(at)
            && let Some(w) = &self.window
        {
            self.ime_at = Some(at);
            let size = PhysicalSize::new(at.w.max(1) as u32, at.h.max(1) as u32);
            w.set_ime_cursor_area(PhysicalPosition::new(at.x, at.y), size);
            // Magnifier follows the caret to where typing goes.
            if self.focused {
                self.caret = place_caret(self.hwnd, (at.x, at.y), (cw, ch), self.caret);
            }
        }
    }

    /// Gives each shown terminal its pane's size in the next frame, without
    /// waiting out [`RESIZE_GAP`]: the window or a divider was let go.
    fn settle(&mut self) {
        for v in &mut self.views {
            v.resized = None;
        }
        self.request_redraw();
    }

    /// Shows the hidden window when [`shows`] says so.
    fn reveal(&mut self, presented: bool) {
        if shows(self.hidden_until, presented, Instant::now()) {
            self.hidden_until = None;
            if let Some(w) = &self.window {
                w.set_visible(true);
            }
        }
    }

    /// Notes where the window is, each time it moves or changes size; see
    /// [`placement`].
    fn note_place(&mut self) {
        let Some(w) = &self.window else {
            return;
        };
        let Ok(p) = w.outer_position() else {
            return;
        };
        let size = w.inner_size();
        let now = Geometry {
            x: p.x,
            y: p.y,
            w: size.width,
            h: size.height,
            maximized: w.is_maximized(),
        };
        let (min, full) = (w.is_minimized() == Some(true), w.fullscreen().is_some());
        self.placed = placement(self.placed, now, min, full);
    }

    /// Saves the session when it [`changed`] since the last save, or always
    /// with `force`.
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
                num: v.map_or(0, |v| v.num),
                shell: v.map(|v| v.shell.clone()).unwrap_or_default(),
            }
        };
        let s = session::State::capture(&self.win, self.placed, meta);
        let changed = changed(self.saved.as_ref(), &s);
        let dragging = self.mouse.divider.is_some();
        if !force && !save_now(changed, dragging, Instant::now(), &mut self.save_after) {
            return;
        }
        self.save_after = None;
        // Output, which changes all the time, is saved only at exit, and
        // only once the layout holding the keys it is filed by was written.
        let written = match session::save(&s) {
            Ok(()) => {
                if force {
                    self.save_output();
                }
                true
            }
            Err(e) => {
                eprintln!("blitz: saving the session: {e}");
                false
            }
        };
        let now = Instant::now();
        self.save_after = saved(written, s, now, &mut self.saved, &mut self.save_fails);
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
        let resize = self.views.iter().filter_map(|v| v.resize_at).min();
        // A changed layout waiting to be saved, and a renderer to retry.
        // A minimized window draws nothing, so it has nothing to retry.
        let shown = (self.window.as_ref())
            .is_some_and(|w| w.inner_size().width > 0 && w.inner_size().height > 0);
        let gfx = self.gfx_retry.filter(|_| self.gfx.is_none() && shown);
        // Only a frame searches.
        let find = (self.find.as_ref())
            .filter(|_| shown && self.gfx.is_some())
            .and_then(Find::due);
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
            resize,
            find,
            self.save_after,
            gfx,
            anim,
            self.mouse.scroll_at,
            self.hidden_until,
        ]
        .into_iter()
        .flatten()
        .min()
    }
}

/// How long a session that `fails` writes in a row could not save waits
/// before the next try: twice as long each time, up to about a minute.
fn save_retry(fails: u32) -> Duration {
    SAVE_DELAY * 2u32.pow(fails.min(7))
}

/// Takes in whether `s`, the session, was `written` at `now`. Once it is,
/// it is the state saved. A failed write is not, so it is tried again at
/// the time returned: later after each failure in a row, so a busy or
/// full drive is not written to every turn.
fn saved<T>(
    written: bool,
    s: T,
    now: Instant,
    saved: &mut Option<T>,
    fails: &mut u32,
) -> Option<Instant> {
    if written {
        *saved = Some(s);
        *fails = 0;
        return None;
    }
    let next = now + save_retry(*fails);
    *fails = fails.saturating_add(1);
    Some(next)
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

/// The one of `hidden`, sessions the sidebar has no room for, that a
/// click on their count goes to: the one waiting longest for the user,
/// else the one after `focus`, so clicks go round them all.
fn hidden_target(
    hidden: &[(PaneId, crate::attention::PaneAttn)],
    focus: Option<PaneId>,
) -> Option<PaneId> {
    let others = (hidden.iter().copied()).filter(|h| Some(h.0) != focus);
    crate::attention::jump_target(others).or_else(|| {
        let next = (hidden.iter())
            .position(|h| Some(h.0) == focus)
            .map_or(0, |i| i + 1);
        let h = hidden.get(next).or(hidden.first())?;
        (Some(h.0) != focus).then_some(h.0)
    })
}

/// The cell under the point `(x, y)` of a grid of `grid` cells, each
/// `cell` pixels, at `r`; `None` off it, as over the header above it or
/// the padding past its last cell.
fn grid_cell(
    r: Rect,
    grid: (u16, u16),
    cell: (u32, u32),
    (x, y): (f64, f64),
) -> Option<(u16, u16)> {
    let (dx, dy) = (x - f64::from(r.x), y - f64::from(r.y));
    if dx < 0.0 || dy < 0.0 {
        return None;
    }
    let (col, row) = (dx as u32 / cell.0.max(1), dy as u32 / cell.1.max(1));
    let inside = col < u32::from(grid.0) && row < u32::from(grid.1);
    inside.then_some((col as u16, row as u16))
}

/// Whether Ctrl+click on `target` is blitz's to open rather than the
/// `program`'s, when that takes the mouse. Claude Code (`claude`) does in
/// fullscreen, so with an `editor` set blitz opens its file paths there.
fn opens_link(target: &Target, program: bool, claude: bool, editor: &str) -> bool {
    !program || claude && !editor.is_empty() && matches!(target, Target::Path(..))
}

/// What a right click that is not the program's does, with
/// `right_click_paste` `on`: copies the selection when there is one
/// (`selected`), else pastes, asking first as Ctrl+V does.
fn right_click_does(on: bool, selected: bool) -> Option<Action> {
    on.then_some(if selected {
        Action::Copy
    } else {
        Action::Paste
    })
}

/// The pointer's shape: a resize arrow on a `divider` between panes, the
/// hand on a link, a session row or the banner (`hand`), the I-beam over a
/// pane's text that blitz selects, and the arrow over a pane whose program
/// takes the mouse (`grid` says which) and anywhere else.
fn pointer(divider: Option<Axis>, hand: bool, grid: Option<bool>) -> CursorIcon {
    match (divider, hand, grid) {
        (Some(Axis::Row), ..) => CursorIcon::ColResize,
        (Some(Axis::Column), ..) => CursorIcon::RowResize,
        (None, true, _) => CursorIcon::Pointer,
        (None, false, Some(false)) => CursorIcon::Text,
        _ => CursorIcon::Default,
    }
}

/// Whether Windows hides the pointer while typing: Mouse settings,
/// Pointer Options. Read once at start.
fn mouse_vanish() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETMOUSEVANISH, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut on = windows::core::BOOL(0);
    // SAFETY: SPI_GETMOUSEVANISH writes one BOOL.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETMOUSEVANISH,
            0,
            Some((&raw mut on).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    read.is_ok() && on.as_bool()
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

/// Where a jump to the session that needs you goes from `at`: to
/// `waiting`, or with nothing waiting back to where the last jump came
/// from, unless that is `at`. `jumped` is the last jump, from and to. A
/// jump from where the last one landed keeps where that one came from, so
/// going round every waiting session still comes back to the start.
fn jump(
    at: Option<PaneId>,
    waiting: Option<PaneId>,
    jumped: &mut Option<(PaneId, PaneId)>,
) -> Option<PaneId> {
    let Some(to) = waiting else {
        return jumped.take().map(|j| j.0).filter(|&from| Some(from) != at);
    };
    let from = match *jumped {
        Some((from, to)) if Some(to) == at => Some(from),
        _ => at,
    };
    *jumped = from.map(|from| (from, to));
    Some(to)
}

/// Time a font size change from the wheel holds off the next, so a fast
/// spin, each step of which sizes every session again, makes one step.
const FONT_WHEEL: Duration = Duration::from_millis(100);

/// The font size step for `steps` of the wheel with Ctrl held: one point
/// up or down, unless the last step, `last`, was too recent.
fn wheel_font(steps: f64, last: Option<Instant>, now: Instant) -> Option<i8> {
    let ready = last.is_none_or(|t| now.saturating_duration_since(t) >= FONT_WHEEL);
    (ready && steps != 0.0).then_some(if steps > 0.0 { 1 } else { -1 })
}

/// Where the IME composes, which its candidates stay clear of: the field
/// of the panel or bar that takes typing, else the cell of the cursor in
/// the focused pane at `pane`, kept inside the pane.
fn ime_area(
    field: Option<Rect>,
    pane: Option<Rect>,
    cursor: Option<(u16, u16)>,
    (cw, ch): (i32, i32),
) -> Option<Rect> {
    if field.is_some() {
        return field;
    }
    let (r, (col, row)) = pane.zip(cursor)?;
    let x = (r.x + i32::from(col) * cw).min(r.right() - cw).max(r.x);
    Some(Rect {
        x,
        y: r.y + i32::from(row) * ch,
        w: cw,
        h: ch,
    })
}

/// Whether a shortcut pressed while the find bar is open runs, which
/// closes the bar. Paste is left to the bar.
fn find_runs(a: Action) -> bool {
    a != Action::Paste
}

/// Whether the find bar stays open after shortcut `a` ran: scrolling
/// through what was found keeps it, as the wheel does.
fn find_keeps(a: Action) -> bool {
    matches!(
        a,
        Action::ScrollPage(_) | Action::ScrollEnd(_) | Action::JumpToPrompt(_)
    )
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

/// Whether a pane title shows Claude Code working: it puts a half-filled
/// circle in front, which turns as it works.
fn claude_working_title(title: &str) -> bool {
    title
        .chars()
        .next()
        .is_some_and(|c| ('\u{25d0}'..='\u{25d3}').contains(&c))
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
    // either way.
    let mut term = lock(&v.pane.term);
    term.set_focused(focused);
    vt::encode_focus(focused, &term.input_modes(), &mut out);
    drop(term);
    v.pane.send(out);
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

/// The window title: the focused pane's, after how many sessions need you,
/// so Alt+Tab, the taskbar and screen readers tell too, and marked the way
/// Windows marks its own consoles when blitz runs as administrator.
fn window_title(waiting: usize, pane: &str, admin: bool) -> String {
    let pane = if pane.is_empty() { "blitz" } else { pane };
    let pane = if admin {
        format!("Administrator: {pane}")
    } else {
        pane.to_string()
    };
    match waiting {
        0 => pane,
        n => format!("({n}) {pane}"),
    }
}

/// What the banner says about the update in hand: the note on it for now,
/// else its offer; nothing without one.
fn banner_text<'a>(update: Option<&'a (String, String)>, note: Option<&'a str>) -> Option<&'a str> {
    update.map(|u| note.unwrap_or(&u.1))
}

/// The state whose dot badges the taskbar button: the one of `states`
/// that most wants the user, if any does.
fn badge_state(states: impl Iterator<Item = Attn>) -> Option<Attn> {
    states.max().filter(|&s| s >= Attn::DoneUnseen)
}

/// How the badge for `top` looks, in the colours the sidebar marks that
/// state with: its colour, whether it is a ring, and what it says to a
/// screen reader.
fn badge_look(top: Option<Attn>, ui: &crate::theme::Ui) -> (u32, bool, &'static str) {
    match top {
        Some(Attn::NeedsYou) => (ui.mark, false, "A session needs you"),
        Some(Attn::Error) => (ui.error, false, "A session failed"),
        Some(_) => (ui.name, true, "A session finished"),
        None => (0, false, ""),
    }
}

/// How blitz tells the user, who is in another program, about a session.
#[derive(Debug, Default, PartialEq, Eq)]
struct Alert {
    /// Times to flash the taskbar button.
    flashes: u32,
    /// Show a Windows notification.
    toast: bool,
    /// Make a sound: the notification's, or a beep without one.
    sound: bool,
}

impl Alert {
    /// Whether to beep, once the notification was `shown` or not: one that
    /// was makes its own sound.
    fn beeps(&self, shown: bool) -> bool {
        self.sound && !shown
    }
}

/// How to tell the user about a session that just changed to `state`
/// while the window is in the background: three flashes when it needs
/// the user or failed, one when it finished, a notification as `toasts`
/// says, with `sound` the notification's sound or else a beep, and at
/// most once per session every `ALERT_GAP`. `last` is when this session
/// last alerted.
fn alert(state: Attn, c: &Config, last: &mut Option<Instant>, now: Instant) -> Option<Alert> {
    let (flashes, urgent) = match state {
        Attn::NeedsYou | Attn::Error => (3, true),
        Attn::DoneUnseen => (1, false),
        Attn::Working | Attn::Idle => return None,
    };
    let toast = c.toasts == "all" || urgent && c.toasts == "needs-you";
    let a = Alert {
        flashes: if c.flash { flashes } else { 0 },
        toast,
        sound: c.sound,
    };
    if a == Alert::default() || last.is_some_and(|t| now.saturating_duration_since(t) < ALERT_GAP) {
        return None;
    }
    *last = Some(now);
    Some(a)
}

/// Whether blitz keeps the PC awake: `keep_awake` is on and one of
/// `states` is working.
fn stays_awake(keep_awake: bool, mut states: impl Iterator<Item = Attn>) -> bool {
    keep_awake && states.any(|s| s == Attn::Working)
}

/// The session a jump goes to: the one waiting longest among those that
/// most want the user. The focused one counts only while blitz is in the
/// background (`front` false): in front, the user is already looking at
/// it. A session that exited stays red until closed.
fn jump_to(
    sessions: impl Iterator<Item = (PaneId, crate::attention::PaneAttn)>,
    focus: Option<PaneId>,
    front: bool,
) -> Option<PaneId> {
    let skip = focus.filter(|_| front);
    crate::attention::jump_target(sessions.filter(|s| Some(s.0) != skip))
}

/// Whether the user should hear that the jump key could not be had:
/// not when it was `got`, nor when another main blitz window is open,
/// which then has it and brings the user to blitz just the same.
fn jump_key_lost(got: bool, other_blitz: impl FnOnce() -> bool) -> bool {
    !got && !other_blitz()
}

/// Whether a session's notification comes down: the user is looking at
/// the session, or it no longer wants them.
fn untoasts(attended: bool, state: Attn) -> bool {
    attended || state < Attn::DoneUnseen
}

/// The lines of a notification about session `s`: its name, numbered as
/// the sidebar numbers it, and what it wants, its last message, and its
/// folder.
fn toast_text(s: &chrome::Session) -> [String; 3] {
    let what = match s.state {
        Attn::NeedsYou => "needs you",
        Attn::Error => "failed",
        _ => "finished",
    };
    let num = s.num.map(|n| format!(" {n}")).unwrap_or_default();
    [
        format!("{}{num} {what}", s.name),
        s.msg.clone(),
        s.cwd.clone(),
    ]
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

/// What the wheel does over a pane whose program is in modes `m`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wheel {
    /// Wheel reports to the program, which takes the mouse.
    Report,
    /// Arrow keys, as in xterm's alternateScroll (mode 1007): pagers have
    /// no scrollback to show, but scroll by the arrow keys.
    Arrows,
    /// Scroll the main screen's view.
    Scroll,
    Nothing,
}

/// What the wheel does over a pane in modes `m`; `shift` keeps it from a
/// program that takes the mouse. Arrows go only to a program that asked
/// for them in the `focused` pane, where the user types, and never to
/// Claude Code (`claude`), where they walk its prompt history and replace
/// what was typed.
fn wheel_does(m: &InputModes, shift: bool, claude: bool, focused: bool) -> Wheel {
    if m.mouse != MouseMode::Off && !shift {
        Wheel::Report
    } else if !m.alt_screen {
        Wheel::Scroll
    } else if m.alt_scroll && !claude && focused {
        Wheel::Arrows
    } else {
        Wheel::Nothing
    }
}

/// Lines a wheel notch scrolls with Windows' "lines to scroll" set to
/// `setting`: that many, or a page of a `rows` high pane for "one screen
/// at a time".
fn wheel_lines(setting: u32, rows: u16) -> isize {
    const WHEEL_PAGESCROLL: u32 = u32::MAX;
    match setting {
        WHEEL_PAGESCROLL => rows.saturating_sub(1).max(1) as isize,
        n => n.min(100) as isize,
    }
}

/// Windows' "lines to scroll" for each wheel notch; 3 if it cannot be read.
fn scroll_lines() -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut n = 3u32;
    // SAFETY: SPI_GETWHEELSCROLLLINES writes one UINT.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETWHEELSCROLLLINES,
            0,
            Some((&raw mut n).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    if read.is_ok() { n } else { 3 }
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

/// The rows a notice takes in a pane of `grid` cells: broken at spaces
/// onto as many as it needs, so its end, which often names the key to
/// press, is not cut off in a narrow pane.
fn notice_rows(text: &str, (cols, rows): (u16, u16)) -> Vec<String> {
    let lines = chrome::wrap(text, i32::from(cols) - 1, 1, usize::from(rows.max(1)));
    lines.into_iter().map(|l| format!(" {l}")).collect()
}

/// Draws a notice over the bottom rows of the pane whose grid is at `at`.
fn draw_notice(r: &mut Renderer, pal: &Palette, at: Rect, grid: (u16, u16), n: &Notice) {
    let (_, ch) = r.cell();
    let lines = notice_rows(&n.text, grid);
    let rows = (lines.len() as u16).clamp(1, grid.1.max(1));
    let mut s = text_snapshot(&lines.join("\n"), grid.0, rows, pal);
    let bg = if n.dim { pal.bg } else { pal.selection_bg };
    for c in &mut s.cells {
        c.bg = bg;
        if n.dim {
            c.attrs |= vt::snapshot::attr::DIM;
        }
    }
    let banner = Palette { bg, ..*pal };
    let y = at.y + i32::from(grid.1.saturating_sub(rows)) * ch as i32;
    r.snapshot(&s, &banner, at.x, y);
}

/// The Windows version, such as `10.0.26200`. The manifest build.rs links
/// in keeps Windows from giving an older one.
fn windows_version() -> String {
    use windows::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOW};
    let mut v = OSVERSIONINFOW {
        dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: a struct to fill, its size set as the call needs.
    match unsafe { GetVersionExW(&mut v) } {
        Ok(()) => format!(
            "{}.{}.{}",
            v.dwMajorVersion, v.dwMinorVersion, v.dwBuildNumber
        ),
        Err(_) => "unknown".into(),
    }
}

/// The title bar icon size, so Windows picks the hand-tuned small icon
/// rather than shrinking the big one.
fn small_icon_size() -> PhysicalSize<u32> {
    // SAFETY: reads a system metric; no pointers.
    let n = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16) as u32;
    PhysicalSize::new(n, n)
}

/// The smallest pane, frame included, that holds `MIN_COLS` by `MIN_ROWS`
/// cells of `cw` by `ch` pixels in a tab of several panes, at `scale`,
/// with chrome text `th` pixels high.
fn pane_min((cw, ch): (u32, u32), th: i32, scale: f32, expanded: bool) -> (i32, i32) {
    let frame = chrome::pane_frame(scale, expanded, true, th);
    (
        layout::MIN_COLS * cw as i32 + frame.0,
        layout::MIN_ROWS * ch as i32 + frame.1,
    )
}

/// The smallest the window may get: the sidebar or the rail as `win`
/// shows them, and one smallest pane. Any smaller and a pane shrinks to a
/// column or two, and a program such as Claude Code redraws everything at
/// that width.
fn min_window(
    win: &layout::Window,
    cell: (u32, u32),
    (tw, th): (u32, u32),
    scale: f32,
) -> PhysicalSize<u32> {
    let (w, h) = pane_min(cell, th as i32, scale, win.sidebar_expanded);
    // The sidebar takes at most 2/5 of the window, so the pane's room
    // grows by one pixel or none for each the window does.
    let room = |x: i32| chrome::area(win, (x, h), scale, None, tw as i32).w;
    let width = (w..).find(|&x| room(x) >= w).unwrap_or(w);
    PhysicalSize::new(width as u32, h as u32)
}

/// The size in cells of `cw` by `ch` pixels of each pane in every tab of
/// `win`, which `panes` lays out with a given tab shown. Hidden tabs count
/// too: a pane restored in one would start at 80 columns, and Claude Code
/// would draw its conversation at that width until the tab shows.
fn tab_grids(
    win: &layout::Window,
    (cw, ch): (u32, u32),
    panes: impl Fn(&layout::Window) -> Vec<(PaneId, Rect)>,
) -> Vec<(PaneId, (i32, i32))> {
    (0..win.tabs.len())
        .flat_map(|active| {
            panes(&layout::Window {
                active,
                ..win.clone()
            })
        })
        .map(|(id, r)| (id, (r.w / cw as i32, r.h / ch as i32)))
        .collect()
}

/// Whether `grids` leave pane `id`, new in `win`, or `split`, the pane it
/// split, below the minimum size. Only the tab shown counts: a new tab
/// splits nothing, and the pane focused before it may be one the window
/// already made small.
fn no_room(
    win: &layout::Window,
    grids: &[(PaneId, (i32, i32))],
    id: PaneId,
    split: Option<PaneId>,
) -> bool {
    let shown = win.tabs.get(win.active).map(Tab::panes).unwrap_or_default();
    grids.iter().any(|&(p, (c, r))| {
        (p == id || Some(p) == split)
            && shown.contains(&p)
            && (c < layout::MIN_COLS || r < layout::MIN_ROWS)
    })
}

/// When a pane whose size changed gives its terminal the new size: now
/// (`None`), unless the terminal `last` took one within [`RESIZE_GAP`],
/// then that long after it. The first change goes at once and the last
/// always lands.
fn resize_wait(last: Option<Instant>, now: Instant) -> Option<Instant> {
    let due = last? + RESIZE_GAP;
    (now < due).then_some(due)
}

/// Whether a window kept hidden `until` then shows now: once a frame was
/// `presented`, or at `until` without one, so a renderer that fails still
/// leaves a window to see.
fn shows(until: Option<Instant>, presented: bool, now: Instant) -> bool {
    until.is_some_and(|t| presented || now >= t)
}

/// Asks Windows to start blitz again, with its saved session, after it
/// restarts for an update or the user signs back in with "restart apps"
/// on. Not after a crash or a hang, which could happen again at once.
fn restart_after_reboot() {
    use windows::Win32::System::Recovery::{
        RESTART_NO_CRASH, RESTART_NO_HANG, RegisterApplicationRestart,
    };
    // SAFETY: no command line, so blitz starts with none.
    let _ = unsafe { RegisterApplicationRestart(None, RESTART_NO_CRASH | RESTART_NO_HANG) };
}

/// Whether a session `now` differs from the one last `saved`. The window's
/// place counts: Windows ends blitz for an update restart without the
/// save at exit.
fn changed(saved: Option<&session::State>, now: &session::State) -> bool {
    saved != Some(now)
}

/// Where the window goes back to next time, after it moved or changed size
/// from `was` to `now`. Minimized or full screen is no place to go back to.
/// Maximized keeps the place it was maximized from, moved to the middle of
/// the monitor it is maximized on when it is not there, as after
/// Win+Shift+Arrow.
fn placement(was: Geometry, now: Geometry, minimized: bool, fullscreen: bool) -> Geometry {
    if minimized || fullscreen {
        return was;
    }
    if !now.maximized {
        return now;
    }
    let (cx, cy) = (was.x + was.w as i32 / 2, was.y + was.h as i32 / 2);
    let there =
        (now.x..now.x + now.w as i32).contains(&cx) && (now.y..now.y + now.h as i32).contains(&cy);
    let was = Geometry {
        maximized: true,
        ..was
    };
    if there {
        return was;
    }
    let mid = |at: i32, room: u32, size: u32| at + (room as i32 - size as i32).max(0) / 2;
    Geometry {
        x: mid(now.x, now.w, was.w),
        y: mid(now.y, now.h, was.h),
        ..was
    }
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

/// What a pane says after the `shell` setting failed to start with `err`
/// and `using`, the shell blitz finds, started in its place. `keys` open
/// the settings.
fn shell_failed(
    shell: &str,
    err: &dyn std::fmt::Display,
    using: &str,
    keys: Option<String>,
) -> String {
    let fix = keys.map_or_else(String::new, |k| format!(" \u{b7} {k} settings"));
    format!("The shell {shell} could not start ({err}); using {using}{fix}")
}

/// What blitz says when it cannot start at all, `e` being why: a GUI
/// program has no console to print it to.
fn start_failed(e: &str, config: Option<&Path>) -> String {
    let file = config.map_or_else(String::new, |d| {
        format!("\n\nSettings are in {}", d.join("config.toml").display())
    });
    format!("blitz could not start: {e}{file}")
}

/// The hint the first start shows: the keys of the command palette and of
/// a few actions worth knowing, as bound now. An action without keys is
/// left out.
fn first_hint(user: &[keymap::Binding]) -> String {
    let keys = [
        (Action::Palette, "every action"),
        (Action::SplitRight, "split"),
        (Action::JumpToAttention, "the session that needs you"),
        (Action::Settings, "settings"),
    ];
    let parts: Vec<String> = (keys.iter())
        .filter_map(|&(a, what)| Some(format!("{} {what}", keymap::keys_for(a, user)?)))
        .collect();
    parts.join(" \u{b7} ")
}

/// An update left for when blitz closes: its release, and its installer
/// once downloaded.
type AtClose = (String, Option<crate::update::Installer>);

/// Whether Ctrl+Shift+U, leaving release `v` for when blitz closes, starts
/// its download: not when `at_close` already holds it, but when it holds an
/// older release, which a later look replaced on the banner.
fn arms(at_close: Option<&AtClose>, v: &str) -> bool {
    at_close.is_none_or(|a| a.0 != v)
}

/// What is left of an update for when blitz closes once the banner goes,
/// by its x or with checks turned off: nothing, unless Ctrl+Shift+U asked
/// to restart now (`updating`) while it downloads. That goes ahead, or its
/// "Downloading" notice would stay, and the key do nothing, until a restart.
fn kept_at_close(at_close: Option<AtClose>, updating: bool) -> Option<AtClose> {
    at_close.filter(|_| updating)
}

/// What a click on the banner does.
#[derive(Debug, PartialEq)]
enum BannerClick {
    Close,
    Notes,
}

/// What a click at (`x`, `y`) does to the banner, given its strip and its
/// x: the x closes it and anywhere else opens the release notes. Updating
/// restarts blitz, so only the key does that.
fn banner_click(banner: Option<(Rect, Rect)>, x: i32, y: i32) -> Option<BannerClick> {
    let inside = |r: Rect| (r.x..r.right()).contains(&x) && (r.y..r.bottom()).contains(&y);
    let (_, close) = banner.filter(|b| inside(b.0))?;
    Some(if inside(close) {
        BannerClick::Close
    } else {
        BannerClick::Notes
    })
}

/// The last `n` lines of `text`, without blank lines at either end.
fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.trim_matches('\n').lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Moves the system caret to `at`, in client pixels, first making one of
/// `size` when `made`, the size of the caret there is, differs. It is
/// never shown, as blitz draws its own cursor, but Magnifier and other
/// tools that follow the text cursor follow it. Returns the caret's size.
fn place_caret(
    hwnd: isize,
    at: (i32, i32),
    size: (u32, u32),
    made: Option<(u32, u32)>,
) -> Option<(u32, u32)> {
    let made = match made {
        Some(s) if s == size => made,
        // SAFETY: our own window, on its thread; it replaces any old caret.
        _ => unsafe {
            CreateCaret(
                HWND(hwnd as *mut c_void),
                None,
                size.0 as i32,
                size.1 as i32,
            )
        }
        .ok()
        .map(|()| size),
    };
    // SAFETY: plain call; it moves this thread's caret, if there is one.
    let _ = unsafe { SetCaretPos(at.0, at.1) };
    made
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

/// Whether blitz runs elevated, as administrator.
fn elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token = HANDLE::default();
    let mut e = TOKEN_ELEVATION::default();
    let mut len = 0;
    // SAFETY: the out value is as large as the call is told; the token is
    // closed after use.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some((&raw mut e).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && e.TokenIsElevated != 0
    }
}

/// Where the first pane starts when no folder was given: `cwd`, where
/// blitz was started, unless that is one of `avoid` or inside Windows' own
/// folder, `system_root`, as for blitz started when the user signs in;
/// else the user's profile folder.
fn first_dir(
    cwd: Option<PathBuf>,
    avoid: &[PathBuf],
    system_root: Option<&Path>,
) -> Option<PathBuf> {
    let key = |p: &Path| p.to_string_lossy().trim_end_matches('\\').to_lowercase();
    let inside = |d: &Path| system_root.is_some_and(|r| Path::new(&key(d)).starts_with(key(r)));
    match cwd {
        Some(d) if !avoid.iter().any(|a| key(a) == key(&d)) && !inside(&d) => Some(d),
        _ => start_dir(""),
    }
}

/// Folders nobody means to work in that the Start menu, a pinned icon or
/// Win+R start blitz in: its own folder and the Windows system folder.
fn not_a_start() -> Vec<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut buf = [0u16; 260];
    // SAFETY: a buffer the call is told the length of.
    let n = unsafe { GetSystemDirectoryW(Some(&mut buf)) } as usize;
    let system =
        (n > 0 && n < buf.len()).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buf[..n])));
    let exe = std::env::current_exe().ok();
    let own = exe.as_deref().and_then(Path::parent).map(Path::to_path_buf);
    [system, own].into_iter().flatten().collect()
}

/// Puts pane `id` in a new tab after the others and shows that tab. It
/// goes by the folder of its focused pane until the user names it.
fn new_tab(win: &mut layout::Window, id: PaneId) -> bool {
    win.tabs.push(Tab::new(String::new(), id));
    win.active = win.tabs.len() - 1;
    true
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

/// `text` without Claude Code's gutter: the mark that starts a reply (⏺,
/// or ● outside macOS) and the ⎿ before a tool's output become spaces,
/// then the indent every line shares goes. Lines are never joined.
fn without_indent(text: &str) -> String {
    let lines: Vec<String> = (text.lines())
        .map(|l| {
            let body = l.trim_start_matches(' ');
            match body.chars().next() {
                Some(c @ ('\u{23FA}' | '\u{25CF}' | '\u{23BF}')) => {
                    format!("{} {}", &l[..l.len() - body.len()], &body[c.len_utf8()..])
                }
                _ => l.to_owned(),
            }
        })
        .collect();
    let common = (lines.iter())
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start_matches(' ').len())
        .min()
        .unwrap_or(0);
    let lines: Vec<&str> = (lines.iter())
        .map(|l| l.get(common..).unwrap_or_default().trim_end())
        .collect();
    lines.join("\r\n")
}

/// What a pane says when its program copied `text` with OSC 52.
fn program_copy_notice(text: &str, copied: bool) -> String {
    if !copied {
        return copy_notice(text, false, false);
    }
    let n = text.chars().count();
    let s = if n == 1 { "" } else { "s" };
    format!("Program copied {n} character{s}")
}

/// What a copy of `text` says: how many lines went to the clipboard, and
/// whether they were the selection output then rewrote, or that another
/// program held the clipboard.
fn copy_notice(text: &str, copied: bool, rewritten: bool) -> String {
    if !copied {
        return "Clipboard busy; nothing was copied".into();
    }
    let lines = text.split('\n').count();
    let s = if lines == 1 { "" } else { "s" };
    let old = if rewritten {
        ", as selected before the output changed"
    } else {
        ""
    };
    format!("Copied {lines} line{s}{old}")
}

/// A key for a one-line text field, such as the find bar or a list's
/// filter: a key that types adds its text, Backspace takes off a character
/// and Ctrl+Backspace a word. Returns whether the field changed.
fn edit_field(field: &mut String, k: &KeyInput) -> bool {
    let m = &k.mods;
    // Ctrl and Alt together are AltGr when the layout gives a character.
    let (ctrl, alt) = (m.lctrl || m.rctrl, m.lalt || m.ralt);
    let chord = ctrl != alt || (ctrl && k.uc == 0);
    match k.vk {
        VK_BACK if ctrl && !alt => {
            let word = (field.trim_end())
                .trim_end_matches(|c: char| !c.is_whitespace())
                .len();
            let cut = word < field.len();
            field.truncate(word);
            cut
        }
        VK_BACK => field.pop().is_some(),
        _ if !chord && !k.text.is_empty() => {
            field.push_str(k.text);
            true
        }
        _ => false,
    }
}

/// What a paste adds to a one-line text field: the first line of `text`,
/// past any line breaks it starts with, without control characters.
fn first_line(text: &str) -> String {
    let line = text.trim_start_matches(['\r', '\n']).lines().next();
    line.unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

/// Whether copy key `k` copies a selection that `shown` says is in view or
/// not, when there is one. Plain Ctrl+C is the interrupt too, so it copies
/// only what the user can see; the other copy keys always copy.
fn copy_key_copies(k: &KeyInput, shown: Option<bool>) -> bool {
    shown != Some(false) || !vt::keys::is_interrupt(k)
}

/// Whether a copy key that found nothing to copy is kept from the program.
/// Ctrl+Shift+C would reach it as Ctrl+C and interrupt it, unless kitty
/// flags make it a key of its own; plain Ctrl+C is meant to interrupt.
fn eats_copy_key(k: &KeyInput, m: &InputModes) -> bool {
    !vt::keys::is_interrupt(k) && vt::keys::interrupts(k, m)
}

/// Whether a paste into `term` goes in without asking; see
/// [`vt::keys::needs_paste_confirm`]. Claude Code, known by its hook
/// notifications (`claude`), reads every paste under bracketed paste as
/// text, so there it needs no confirmed paste first.
fn paste_trusted(term: &vt::Terminal, claude: bool) -> bool {
    term.paste_trusted() || claude && term.input_modes().bracketed
}

/// Paths as a paste types them: joined by spaces, each in quotes when it
/// holds a space, as Windows Terminal does.
fn quote_paths(paths: &[PathBuf]) -> String {
    let quoted: Vec<String> = (paths.iter())
        .map(|p| match p.to_string_lossy() {
            s if s.contains(' ') => format!("\"{s}\""),
            s => s.into_owned(),
        })
        .collect();
    quoted.join(" ")
}

/// What files dropped on the window do.
#[derive(Debug, PartialEq)]
enum Dropped {
    /// Pasted into this pane as their paths, the way [`quote_paths`] types
    /// them.
    Paste(PaneId, String),
    /// The folders among them, dropped on the sidebar, each opened in a
    /// new tab.
    Open(Vec<PathBuf>),
}

/// What `paths` dropped where [`App::hit`] found `hit` do.
fn dropped(paths: Vec<PathBuf>, hit: (Option<PaneId>, bool)) -> Option<Dropped> {
    match hit {
        (_, true) => Some(Dropped::Open(
            paths.into_iter().filter(|p| p.is_dir()).collect(),
        )),
        (Some(id), false) => Some(Dropped::Paste(id, quote_paths(&paths))),
        (None, false) => None,
    }
}

/// Alt+V pressed and released, as the program asked keys to be sent:
/// the key Claude Code pastes an image on.
fn alt_v(m: &InputModes) -> Vec<u8> {
    let mut out = Vec::new();
    for down in [true, false] {
        let k = KeyInput {
            vk: 0x56,
            scan: 0x2f,
            extended: false,
            down,
            repeat: 1,
            mods: Mods {
                lalt: true,
                ..Mods::default()
            },
            locks: vt::Locks::default(),
            text: "v",
            uc: u16::from(b'v'),
            cs: 0,
            key: vt::Key::Char('v'),
            us_base: Some('v'),
        };
        vt::encode_key(&k, m, &mut out);
    }
    out
}

/// Why nothing is pasted into the pane `label` names, when its program
/// exited with `code`. The notice it replaces said how to close the pane,
/// so this one does too.
fn paste_refused(label: &str, code: Option<u32>) -> Option<String> {
    let code = code?;
    Some(format!(
        "{label} exited with code {code}, so nothing was pasted \u{b7} Enter close"
    ))
}

/// `text` without the line break at its end when that is its only one: a
/// command copied with its line break is pasted, not run.
fn trim_paste(text: &str) -> &str {
    let line = text.strip_suffix('\n').unwrap_or(text);
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.contains(['\r', '\n']) {
        text
    } else {
        line
    }
}

/// What a paste that needs confirming asks: how many lines, how they
/// start, and the paste `key`, when one is bound.
fn paste_question(text: &str, key: Option<&str>) -> String {
    let first = text.trim_start().lines().next().unwrap_or_default();
    let mut chars = first.chars().filter(|c| !c.is_control());
    let mut start: String = chars.by_ref().take(40).collect();
    if chars.next().is_some() {
        start.push('\u{2026}');
    }
    let lines = text.lines().count();
    let s = if lines == 1 { "" } else { "s" };
    let again = match key {
        Some(k) => format!("Press {k} again"),
        None => "Paste again".into(),
    };
    format!("Paste {lines} line{s} starting \"{start}\"? {again}")
}

/// A session as the palette lists it: its name, folder and branch.
fn session_row(s: &chrome::Session) -> String {
    let mut row = s.name.clone();
    let folder = (!s.cwd.is_empty()).then(|| chrome::folder_name(&s.cwd));
    for part in [folder.as_deref(), s.branch.as_deref()]
        .into_iter()
        .flatten()
    {
        row.push_str(" \u{b7} ");
        row.push_str(part);
    }
    row
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
            if !self.args.scripted() {
                use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
                let text = start_failed(&e, crate::config::dir().as_deref());
                // SAFETY: two strings that outlive the call, and no owner.
                unsafe {
                    MessageBoxW(
                        None,
                        &windows::core::HSTRING::from(text),
                        &windows::core::HSTRING::from("blitz"),
                        MB_OK | MB_ICONERROR,
                    )
                };
            }
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
            self.reveal(false);
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
            WindowEvent::Resized(_) => {
                self.note_place();
                self.request_redraw();
            }
            WindowEvent::Moved(_) => self.note_place(),
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
                    crate::handoff::forget_activating_click();
                    self.set_drag(None);
                    self.mouse.scroll_at = None;
                    let mods = mods_now();
                    for b in 0..3 {
                        if let Some(id) = route_button(&mut self.mouse.reported, b, false, None) {
                            self.mouse_report(id, MouseKind::Release, b as u8, mods);
                        }
                    }
                }
                // Only the window with the keys has a caret; the next frame
                // makes it again.
                if !f && self.caret.take().is_some() {
                    // SAFETY: plain call on the thread that made the caret.
                    let _ = unsafe { DestroyCaret() };
                }
                self.ime_at = None;
                // High contrast mode may have been turned on or off while
                // the window was in the background, and winit passes on no
                // event for that.
                if f {
                    let contrast = crate::theme::system_contrast();
                    if std::mem::replace(&mut self.contrast, contrast) != contrast
                        && self.picker.is_none()
                    {
                        self.set_theme_from_config();
                    }
                }
                // Ctrl may be let go while another window has the keys.
                self.set_hover(None);
                self.hide_pointer(false);
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
                    self.hide_pointer(true);
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
            WindowEvent::CursorLeft { .. } => self.set_over_side(None),
            WindowEvent::MouseInput { state, button, .. } => {
                self.on_mouse_button(el, state, button);
            }
            WindowEvent::MouseWheel { delta, .. } => self.on_wheel(delta),
            WindowEvent::DroppedFile(path) => self.dropped.push(path),
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
            UserEvent::Failed(v, log) => {
                if self.unasked(&v, true) {
                    self.offer_update(v, Some(log));
                }
            }
            UserEvent::Checked(found, asked) => {
                let (text, failed) = (crate::update::found(&found), found.is_err());
                self.update_error = failed.then(|| text.clone());
                if let Ok(Some(v)) = found
                    && (asked || self.unasked(&v, false))
                {
                    self.offer_update(v, None);
                }
                // A look nobody asked for leaves an update under way alone.
                let id = if asked { self.updating.take() } else { None };
                match id {
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
            UserEvent::Sized => self.settle(),
            UserEvent::Fetched(v, got) => self.fetched(v, got),
            UserEvent::Installed(Ok(())) => el.exit(),
            UserEvent::Installed(Err(e)) => {
                eprintln!("blitz: update: {e}");
                self.banner_note = None;
                let text = format!("Update failed: {e}");
                self.update_error = Some(text.clone());
                if let Some(id) = self.updating.take() {
                    self.error(id, text);
                }
            }
            // Explorer restarted. A new TaskbarList too, as the old one
            // may still talk to the Explorer that is gone.
            UserEvent::TaskbarButton => {
                self.taskbar = None;
                self.taskbar_shows = None;
                self.badge_shows = None;
                self.taskbar_progress();
                self.taskbar_badge();
            }
            UserEvent::Handoff(ask) => {
                crate::handoff::to_current_desktop(HWND(self.hwnd as *mut c_void));
                // The launch that sent this let this process take the
                // foreground.
                self.to_front();
                if let crate::handoff::Ask::Open(dir) = ask {
                    self.add(Some(dir), "", new_tab);
                }
            }
            // So does a click on a notification, and the jump key.
            UserEvent::ShowPane(id) => {
                self.to_front();
                self.show(id);
            }
            UserEvent::GlobalJump => {
                self.jump();
                self.to_front();
            }
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.drain_keys(el);
        self.on_drop();
        self.save_session(false);
        // Here, after every batch of events, rather than at each change:
        // a minimized window is not drawn.
        self.sync_title();
        let flow = match self.next_deadline() {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        };
        el.set_control_flow(flow);
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        // Closing the window, Alt+F4 and an update all keep the layout.
        self.save_session(true);
        crate::notify::untoast_all();
        // Not when that would close another blitz window opened since; the
        // banner offers it again at the next start.
        if let Some((_, Some(installer))) = self.at_close.take()
            && crate::handoff::others() == 0
            && let Err(e) = crate::update::run(&installer, false)
        {
            eprintln!("blitz: update: {e}");
        }

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
    fn app_update_notes_take_the_banner_while_there_is_one() {
        let update = ("0.2.0".to_string(), "blitz 0.2.0 is available".to_string());
        assert_eq!(
            banner_text(Some(&update), None),
            Some("blitz 0.2.0 is available")
        );
        let note = Some("Downloading blitz 0.2.0\u{2026}");
        assert_eq!(banner_text(Some(&update), note), note);
        assert_eq!(banner_text(None, note), None);
    }

    #[test]
    fn app_title_counts_the_sessions_that_need_you() {
        assert_eq!(window_title(0, "pwsh", false), "pwsh");
        assert_eq!(window_title(2, "pwsh", false), "(2) pwsh");
        assert_eq!(window_title(0, "", false), "blitz");
        assert_eq!(window_title(1, "", false), "(1) blitz");
        assert_eq!(window_title(2, "pwsh", true), "(2) Administrator: pwsh");
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
    fn app_selection_follows_its_text_to_a_new_width() {
        let pal = crate::theme::dark();
        let mut t = fed(6, 3, "one two three\r\nfour");
        let mut s = Snapshot::default();
        // "two three", across the wrap.
        let mut sel = select(&t, (0, 4), (2, 0));
        assert_eq!(selection_text(&t, &pal, &sel, 0), "two three");
        for cols in [20, 4, 9] {
            let mut marks = sel.marks();
            assert!(t.resize_keeping(cols, 3, &mut marks), "{cols}");
            sel.reflowed(&t, &pal, &marks);
            assert!(refresh(&mut t, &mut s, &pal, Some(&mut sel)), "{cols}");
            assert_eq!(selection_text(&t, &pal, &sel, 0), "two three", "{cols}");
        }
        // A block's columns mean nothing at another width.
        let block = drag(&t, (0, 0), (1, 1), 1, true);
        assert!(block.marks().is_empty());
    }

    #[test]
    fn app_selection_outlives_blitz_prompt() {
        let pal = crate::theme::dark();
        let mut t = fed(10, 3, "ls\r\na b\r\n");
        let mut s = Snapshot::default();
        let mut sel = select(&t, (1, 0), (1, 2));
        // What blitz's shell integration prints before each prompt.
        t.feed(b"\x1b[?1049h\x1b[?1049l\x1b[!pPS> ");
        assert!(refresh(&mut t, &mut s, &pal, Some(&mut sel)));
        assert_eq!(selection_text(&t, &pal, &sel, 0), "a b");
    }

    #[test]
    fn app_every_pane_shows_its_own_selection() {
        let pal = crate::theme::dark();
        let (a, b) = (fed(10, 2, "one\r\ntwo"), fed(10, 2, "three"));
        let (sa, sb) = (select(&a, (0, 0), (0, 2)), select(&b, (0, 1), (0, 3)));
        let (mut snap_a, mut snap_b) = (Snapshot::default(), Snapshot::default());
        let (mut a, mut b) = (a, b);
        a.snapshot(&mut snap_a, &pal);
        b.snapshot(&mut snap_b, &pal);
        // Pane a has focus and the pointer on a link; pane b keeps its
        // selection all the same.
        mark(&mut snap_a, 0, Some(&sa), Some(((1, 0), (1, 2))));
        mark(&mut snap_b, 0, Some(&sb), None);
        assert_eq!(snap_a.selection, Some(((0, 0), (2, 0))));
        assert_eq!(snap_a.hover, Some(((0, 1), (2, 1))));
        assert_eq!(snap_b.selection, Some(((1, 0), (3, 0))));
        assert_eq!(snap_b.hover, None);
        mark(&mut snap_b, 0, None, None);
        assert_eq!(snap_b.selection, None);
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
    fn app_double_click_takes_a_whole_url_or_path_without_the_full_stop() {
        let t = fed(40, 3, "at https://x.com/a_(b), see a.rs(3,4). Done.");
        let pal = crate::theme::dark();
        let words = |at| {
            let s = drag(&t, at, at, 2, false);
            selection_text(&t, &pal, &s, 0)
        };
        assert_eq!(words((0, 10)), "https://x.com/a_(b)");
        assert_eq!(words((0, 29)), "a.rs(3,4)");
        assert_eq!(words((1, 1)), "Done");
        assert_eq!(words((1, 3)), "Done.", "a click on the full stop keeps it");
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
    fn app_quick_select_labels_urls_paths_and_hashes_from_the_bottom() {
        let pal = crate::theme::dark();
        let t = fed(
            30,
            4,
            "see https://x.com/a\r\nat src/a.rs:3 and b/none.rs\r\ncommit 1a2b3c4d done\r\n",
        );
        let found = |p: &str| (p == "src/a.rs").then(|| PathBuf::from(r"C:\x\src\a.rs"));
        let items = quick_items(&t, &pal, 4, found);
        let texts: Vec<&str> = items.iter().map(|i| i.1.as_str()).collect();
        assert_eq!(texts, ["1a2b3c4d", "src/a.rs:3", "https://x.com/a"]);
        assert_eq!(
            items[0].0,
            Found {
                start: (2, 7),
                end: (2, 14)
            }
        );
        assert_eq!(items[0].2, None, "a hash opens nothing");
        let path = Target::Path(PathBuf::from(r"C:\x\src\a.rs"), Some((3, 1)));
        assert_eq!(items[1].2, Some(path));
        assert_eq!(items[2].2, Some(Target::Uri("https://x.com/a".into())));
        // Only what is in view.
        assert_eq!(quick_items(&t, &pal, 1, found).len(), 1);
    }

    #[test]
    fn app_quick_select_labels_go_by_the_key() {
        let key = |vk: u16, text, mods, caps| KeyInput {
            vk,
            scan: 0,
            extended: false,
            down: true,
            repeat: 1,
            mods,
            locks: vt::Locks {
                caps,
                ..vt::Locks::default()
            },
            text,
            uc: 0,
            cs: 0,
            key: vt::Key::Char('x'),
            us_base: None,
        };
        let shift = Mods {
            lshift: true,
            ..Mods::default()
        };
        let ctrl = Mods {
            rctrl: true,
            ..Mods::default()
        };
        let label = |vk, text, mods, caps| quick_label(&key(vk, text, mods, caps));
        assert_eq!(label(0x43, "c", Mods::default(), false), Some((2, false)));
        assert_eq!(label(0x43, "C", shift, false), Some((2, true)));
        assert_eq!(
            label(0x43, "C", Mods::default(), true),
            Some((2, false)),
            "Caps Lock"
        );
        assert_eq!(
            label(0x41, "\u{444}", Mods::default(), false),
            Some((0, false)),
            "Cyrillic"
        );
        assert_eq!(label(0x5a, "z", Mods::default(), false), Some((25, false)));
        assert_eq!(label(0x43, "c", ctrl, false), None);
        assert_eq!(label(0x31, "1", Mods::default(), false), None);
    }

    #[test]
    fn app_hashes_are_hex_words_with_letters_and_digits() {
        let text = "abc1234 deadbeefcafe 1234567 abcdefa x1a2b3c4 1a2b3c4_ 9f8e7d6c5b.";
        let found: Vec<&str> = hashes(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(found, ["abc1234", "9f8e7d6c5b"]);
        let text = "abc1234..def5678 build/a1b2c3d/x blitz-3f2a1b9c8d7e6f50                     550e8400-e29b-41d4-a716-446655440000 (fe12ab3)";
        let found: Vec<&str> = hashes(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(found, ["abc1234", "def5678", "fe12ab3"]);
    }

    #[test]
    fn app_hints_show_once_ever() {
        // Every hint is noted in the one file, so the keys shown at the
        // first start and the Shift+drag hint each show once.
        let dir = std::env::temp_dir().join(format!("blitz-hints-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let seen = [
            session::first_time_in(&dir, "keys"),
            session::first_time_in(&dir, "shift-drag"),
            session::first_time_in(&dir, "shift-drag"),
            session::first_time_in(&dir, "keys"),
        ];
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(seen, [true, true, false, false]);
    }

    #[test]
    fn app_pointer_shows_what_a_click_does() {
        assert_eq!(
            pointer(None, false, Some(false)),
            CursorIcon::Text,
            "selectable text"
        );
        assert_eq!(
            pointer(None, false, Some(true)),
            CursorIcon::Default,
            "the program's"
        );
        assert_eq!(
            pointer(None, true, Some(false)),
            CursorIcon::Pointer,
            "a link"
        );
        assert_eq!(
            pointer(None, true, None),
            CursorIcon::Pointer,
            "a row or the banner"
        );
        assert_eq!(pointer(None, false, None), CursorIcon::Default);
        assert_eq!(pointer(Some(Axis::Row), true, None), CursorIcon::ColResize);
        assert_eq!(
            pointer(Some(Axis::Column), false, Some(false)),
            CursorIcon::RowResize
        );
    }

    #[test]
    fn app_mouse_reports_come_only_from_the_grid() {
        let r = Rect {
            x: 100,
            y: 50,
            w: 85,
            h: 64,
        };
        let cell = |x, y| grid_cell(r, (10, 4), (8, 16), (x, y));
        assert_eq!(cell(100.0, 50.0), Some((0, 0)));
        assert_eq!(cell(179.9, 113.9), Some((9, 3)));
        assert_eq!(cell(181.0, 60.0), None, "padding past the last column");
        assert_eq!(cell(120.0, 40.0), None, "the header above");
        assert_eq!(cell(99.0, 60.0), None, "left of the grid");
        assert_eq!(cell(120.0, 114.0), None, "below it");
    }

    #[test]
    fn app_right_click_copies_a_selection_or_pastes() {
        assert_eq!(right_click_does(true, true), Some(Action::Copy));
        assert_eq!(right_click_does(true, false), Some(Action::Paste));
        assert_eq!(right_click_does(false, true), None, "turned off");
    }

    #[test]
    fn app_ctrl_click_in_claude_fullscreen_opens_paths_in_the_editor() {
        let file = Target::Path(PathBuf::from(r"C:\x\a.rs"), Some((3, 1)));
        let web = Target::Uri("https://example.com".into());
        let code = "vscode://file/{path}:{line}:{col}";
        for t in [&file, &web] {
            assert!(
                opens_link(t, false, false, ""),
                "no program takes the mouse"
            );
            assert!(opens_link(t, false, true, code));
        }
        assert!(opens_link(&file, true, true, code));
        assert!(
            !opens_link(&web, true, true, code),
            "Claude Code opens its own URLs"
        );
        assert!(!opens_link(&file, true, true, ""), "no editor");
        assert!(!opens_link(&file, true, false, code), "not Claude Code");
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
    fn app_wheel_follows_the_windows_lines_to_scroll() {
        assert_eq!(wheel_lines(3, 40), 3);
        assert_eq!(wheel_lines(1, 40), 1);
        assert_eq!(wheel_lines(0, 40), 0, "no scrolling");
        assert_eq!(wheel_lines(u32::MAX, 40), 39, "a page");
        assert_eq!(wheel_lines(u32::MAX, 1), 1);
        assert_eq!(wheel_lines(5000, 40), 100);
        assert!(scroll_lines() <= 100 || scroll_lines() == u32::MAX);
    }

    #[test]
    fn app_wheel_sends_arrows_only_when_asked_and_never_to_claude() {
        let alt = InputModes {
            alt_screen: true,
            ..InputModes::default()
        };
        let asked = InputModes {
            alt_scroll: true,
            ..alt
        };
        let mouse = InputModes {
            mouse: MouseMode::Click,
            ..asked
        };
        let does = |m: &InputModes, shift, claude| wheel_does(m, shift, claude, true);
        assert_eq!(does(&InputModes::default(), false, false), Wheel::Scroll);
        assert_eq!(does(&alt, false, false), Wheel::Nothing, "not asked");
        assert_eq!(does(&asked, false, false), Wheel::Arrows);
        assert_eq!(does(&asked, false, true), Wheel::Nothing, "Claude Code");
        assert_eq!(does(&mouse, false, true), Wheel::Report);
        assert_eq!(
            does(&mouse, true, true),
            Wheel::Nothing,
            "Shift over Claude"
        );
        assert_eq!(does(&mouse, true, false), Wheel::Arrows, "Shift");
        let main = InputModes {
            mouse: MouseMode::Any,
            ..InputModes::default()
        };
        assert_eq!(does(&main, true, true), Wheel::Scroll);
    }

    #[test]
    fn app_wheel_over_another_pane_reports_or_scrolls_but_sends_no_keys() {
        let asked = InputModes {
            alt_screen: true,
            alt_scroll: true,
            ..InputModes::default()
        };
        let mouse = InputModes {
            mouse: MouseMode::Drag,
            ..asked
        };
        assert_eq!(wheel_does(&mouse, false, true, false), Wheel::Report);
        assert_eq!(wheel_does(&asked, false, false, false), Wheel::Nothing);
        assert_eq!(wheel_does(&mouse, true, false, false), Wheel::Nothing);
        let main = InputModes::default();
        assert_eq!(wheel_does(&main, false, false, false), Wheel::Scroll);
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

    /// The caret goes where the cursor is, and is made again only for a
    /// new cell size.
    #[test]
    fn app_caret_follows_the_cursor() {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, GetCaretPos, WINDOW_EX_STYLE, WS_POPUP,
        };
        // A hidden window of this thread, which then owns the caret.
        // SAFETY: a system class with no parent; destroyed below.
        let w = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                windows::core::w!("STATIC"),
                None,
                WS_POPUP,
                0,
                0,
                100,
                100,
                None,
                None,
                None,
                None,
            )
        }
        .expect("window");
        let hwnd = w.0 as isize;
        let pos = || {
            let mut p = POINT::default();
            // SAFETY: a valid out pointer.
            unsafe { GetCaretPos(&mut p) }.expect("a caret");
            (p.x, p.y)
        };
        let made = place_caret(hwnd, (16, 32), (8, 16), None);
        assert_eq!(made, Some((8, 16)));
        assert_eq!(pos(), (16, 32));
        assert_eq!(place_caret(hwnd, (24, 32), (8, 16), made), made);
        assert_eq!(pos(), (24, 32));
        assert_eq!(place_caret(hwnd, (30, 40), (10, 20), made), Some((10, 20)));
        assert_eq!(pos(), (30, 40));
        // SAFETY: this thread's caret and window.
        unsafe {
            DestroyCaret().expect("caret");
            DestroyWindow(w).expect("window");
        }
    }

    /// A session that could not be written is tried again, later after
    /// each failure in a row, rather than taken as saved.
    #[test]
    fn app_failed_session_saves_back_off() {
        let waits: Vec<u128> = (0..10).map(|n| save_retry(n).as_millis()).collect();
        assert_eq!(
            waits,
            [
                500, 1000, 2000, 4000, 8000, 16000, 32000, 64000, 64000, 64000
            ]
        );
        assert_eq!(save_retry(u32::MAX), save_retry(7));
        // The next try waits out its delay as any change does.
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let mut due = Some(t0 + save_retry(3));
        assert!(!save_now(true, false, at(3999), &mut due));
        assert!(save_now(true, false, at(4000), &mut due));
        // A failed write keeps the last saved state, so the layout still
        // counts as changed; a write that works is saved and starts over.
        let (mut kept, mut fails) = (Some("old"), 0);
        assert_eq!(
            saved(false, "new", t0, &mut kept, &mut fails),
            Some(at(500))
        );
        assert_eq!((kept, fails), (Some("old"), 1));
        assert_eq!(
            saved(false, "new", t0, &mut kept, &mut fails),
            Some(at(1000))
        );
        assert_eq!(saved(true, "new", t0, &mut kept, &mut fails), None);
        assert_eq!((kept, fails), (Some("new"), 0));
        assert_eq!(
            saved(false, "newer", t0, &mut kept, &mut fails),
            Some(at(500))
        );
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
    fn app_text_fields_delete_words_and_take_the_first_line_of_a_paste() {
        let mut field = String::from("git log  --oneline ");
        let mut back = input(VK_BACK, true, vt::Key::Backspace, "");
        assert!(edit_field(&mut field, &back));
        assert_eq!(field, "git log  --oneline");
        back.mods.lctrl = true;
        assert!(edit_field(&mut field, &back));
        assert_eq!(field, "git log  ");
        assert!(edit_field(&mut field, &back));
        assert_eq!(field, "git ");
        assert!(edit_field(&mut field, &back));
        assert_eq!(field, "");
        assert!(!edit_field(&mut field, &back), "nothing left");
        assert!(edit_field(
            &mut field,
            &input(0x41, true, vt::Key::Char('a'), "a")
        ));
        assert_eq!(field, "a");
        assert_eq!(first_line("\r\ngit\tstatus\r\nrm -rf x"), "gitstatus");
        assert_eq!(first_line(""), "");
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
    fn app_jumps_come_back_when_nothing_waits() {
        let (a, b, c, d) = (PaneId(1), PaneId(2), PaneId(3), PaneId(4));
        let mut j = None;
        assert_eq!(jump(Some(a), None, &mut j), None, "nothing waits");
        // Round the waiting sessions, then back to the start.
        assert_eq!(jump(Some(a), Some(b), &mut j), Some(b));
        assert_eq!(jump(Some(b), Some(c), &mut j), Some(c));
        assert_eq!(jump(Some(c), None, &mut j), Some(a));
        assert_eq!(jump(Some(a), None, &mut j), None, "once");
        // Moving away by hand starts over from there.
        assert_eq!(jump(Some(a), Some(b), &mut j), Some(b));
        assert_eq!(jump(Some(d), Some(c), &mut j), Some(c));
        assert_eq!(jump(Some(c), None, &mut j), Some(d));
        // Already back where it came from.
        assert_eq!(jump(Some(a), Some(b), &mut j), Some(b));
        assert_eq!(jump(Some(a), None, &mut j), None);
        assert_eq!(j, None);
    }

    #[test]
    fn app_ctrl_wheel_steps_the_font_a_point_at_a_time() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        assert_eq!(wheel_font(1.0, None, t0), Some(1));
        assert_eq!(wheel_font(-3.0, None, t0), Some(-1), "one point a step");
        assert_eq!(wheel_font(1.0, Some(t0), ms(99)), None, "a fast spin");
        assert_eq!(wheel_font(1.0, Some(t0), ms(100)), Some(1));
        assert_eq!(wheel_font(0.0, None, t0), None);
    }

    #[test]
    fn app_ime_composes_in_the_field_or_at_the_cursor() {
        let pane = Rect {
            x: 100,
            y: 50,
            w: 90,
            h: 200,
        };
        let cell = (9, 20);
        let at = |x, y| Rect { x, y, w: 9, h: 20 };
        assert_eq!(
            ime_area(None, Some(pane), Some((2, 1)), cell),
            Some(at(118, 70))
        );
        // A cursor past the last whole cell stays in the pane.
        assert_eq!(
            ime_area(None, Some(pane), Some((10, 0)), cell),
            Some(at(181, 50))
        );
        let field = Rect {
            x: 400,
            y: 60,
            w: 200,
            h: 15,
        };
        assert_eq!(
            ime_area(Some(field), Some(pane), Some((2, 1)), cell),
            Some(field)
        );
        assert_eq!(ime_area(None, None, Some((2, 1)), cell), None);
    }

    #[test]
    fn app_shortcuts_close_the_find_bar_and_run() {
        for a in [
            Action::Copy,
            Action::Palette,
            Action::SplitRight,
            Action::Find,
        ] {
            assert!(find_runs(a), "{a:?}");
        }
        assert!(!find_runs(Action::Paste), "for the bar");
        // Scrolling looks through the matches; the rest leave the bar.
        for a in [
            Action::ScrollPage(1),
            Action::ScrollEnd(-1),
            Action::JumpToPrompt(1),
        ] {
            assert!(find_runs(a) && find_keeps(a), "{a:?}");
        }
        for a in [Action::Copy, Action::SplitRight, Action::FontSize(1)] {
            assert!(!find_keeps(a), "{a:?}");
        }
    }

    #[test]
    fn app_find_searches_streaming_output_a_few_times_a_second() {
        let t = fed(10, 2, "abc");
        let mut f = Find::new(PaneId(1));
        f.query = "b".into();
        assert_eq!(f.due(), None, "nothing new");
        f.stale = true;
        assert!(f.due().is_some_and(|t| t <= Instant::now()), "at once");
        f.search(&t, 2);
        assert_eq!(f.due(), None);
        f.stale = true;
        let at = f.due().expect("a search to come");
        assert!(at > Instant::now() && at <= Instant::now() + FIND_EVERY);
    }

    #[test]
    fn app_typing_replaces_a_query_find_opened_with() {
        let back = input(VK_BACK, true, vt::Key::Backspace, "");
        let n = input(0x4e, true, vt::Key::Char('n'), "n");
        let e = input(0x45, true, vt::Key::Char('e'), "e");
        let mut f = Find::new(PaneId(1));
        f.query = "old".into();
        f.fresh = true;
        assert!(!f.edit(&input(0x70, true, vt::Key::F(1), "")), "F1");
        assert!(f.fresh);
        assert!(f.edit(&n) && f.edit(&e));
        assert_eq!((f.query.as_str(), f.fresh), ("ne", false));
        assert!(f.edit(&back));
        assert_eq!(f.query, "n");
        f.fresh = true;
        assert!(f.edit(&back), "Backspace takes all of a fresh query");
        assert!(f.query.is_empty() && !f.edit(&back));
        f.query = "old".into();
        f.fresh = true;
        f.type_text("pasted");
        assert_eq!(f.query, "pasted");
    }

    #[test]
    fn app_selects_all_text_or_the_last_command_output() {
        let pal = crate::theme::dark();
        const PROMPT: &str = "\x1b]133;A;blitz=1\x07$ ";
        let text = format!(
            "{PROMPT}ls\r\none\r\n{PROMPT}cargo build --release\r\n\
             Compiling x\r\nFinished\r\n\r\n{PROMPT}"
        );
        let t = fed(12, 4, &text);
        let text = |(a, b): (usize, usize)| {
            let s = selection_of(&t, &pal, (a, 0), (b, u16::MAX));
            selection_text(&t, &pal, &s, 0)
        };
        let out = last_output(&t, &pal).expect("output");
        assert_eq!(
            text(out),
            "Compiling x\r\nFinished",
            "past the wrapped command"
        );
        let all = all_text(&t, &pal).expect("text");
        assert!(text(all).starts_with("$ ls\r\none\r\n$ cargo build"));
        assert!(text(all).ends_with("Finished\r\n\r\n$"));
        // One prompt has no command before it.
        let t = fed(12, 4, &format!("hello\r\n{PROMPT}"));
        assert_eq!(last_output(&t, &pal), None);
        let t = fed(12, 4, "");
        assert_eq!(all_text(&t, &pal), None, "nothing to select");
    }

    #[test]
    fn app_closing_find_leaves_the_match_selected() {
        let pal = crate::theme::dark();
        let mut t = fed(6, 3, "one Needle, \u{4e2d}x");
        let mut f = Find::new(PaneId(1));
        f.query = "needle, \u{4e2d}".into();
        f.search(&t, 3);
        let m = f.found[f.cur.expect("a match")];
        let mut sel = selection_of(&t, &pal, m.start, m.end);
        assert_eq!(selection_text(&t, &pal, &sel, 0), "Needle, \u{4e2d}");
        let mut s = Snapshot::default();
        assert!(
            refresh(&mut t, &mut s, &pal, Some(&mut sel)),
            "a selection like any"
        );
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

    /// A live resize or a divider drag gives each program a new size at
    /// most every 80 ms: the first change at once, the last one always.
    #[test]
    fn app_programs_take_a_new_size_at_most_every_80_ms() {
        let t0 = Instant::now();
        assert_eq!(resize_wait(None, t0), None, "the first at once");
        let ms = Duration::from_millis;
        assert_eq!(resize_wait(Some(t0), t0 + ms(30)), Some(t0 + RESIZE_GAP));
        assert_eq!(resize_wait(Some(t0), t0 + RESIZE_GAP), None);
        assert_eq!(RESIZE_GAP, ms(80));
    }

    /// A second press on the same divider in time gives the panes equal
    /// space; one on another divider does not.
    #[test]
    fn app_a_double_click_on_a_divider_evens_the_panes() {
        let (t0, ms, within) = (
            Instant::now(),
            Duration::from_millis,
            Duration::from_millis(500),
        );
        let mut last = None;
        assert!(!evens(&mut last, 1, t0, within), "a drag starts");
        assert!(evens(&mut last, 1, t0 + ms(200), within), "the second");
        assert!(!evens(&mut last, 1, t0 + ms(400), within), "a third drags");
        assert!(
            !evens(&mut last, 2, t0 + ms(500), within),
            "another divider"
        );
        assert!(!evens(&mut last, 2, t0 + ms(1100), within), "too late");
    }

    /// A new tab has room even when the pane focused before it, now in a
    /// tab not shown, is below the minimum; a split checks the pane it
    /// splits.
    #[test]
    fn app_a_small_pane_in_another_tab_leaves_room_for_a_new_tab() {
        let any = Rect {
            x: 0,
            y: 0,
            w: 800,
            h: 400,
        };
        let mut a = Tab::new("a".into(), PaneId(1));
        assert!(a.split(Dir::Right, PaneId(3), any, (0, 0)));
        let mut win = layout::Window::default();
        win.tabs.push(a);
        win.tabs.push(Tab::new("b".into(), PaneId(2)));
        win.active = 1;
        let grids = [
            (PaneId(1), (4, 2)),
            (PaneId(3), (4, 2)),
            (PaneId(2), (80, 24)),
        ];
        assert!(!no_room(&win, &grids, PaneId(2), Some(PaneId(1))), "a tab");
        win.active = 0;
        assert!(no_room(&win, &grids, PaneId(3), Some(PaneId(1))), "a split");
    }

    /// Restored panes in tabs not shown start at their real size, so Claude
    /// Code resumes at the width it will be seen at.
    #[test]
    fn app_panes_in_hidden_tabs_start_at_their_size() {
        let mut win = layout::Window::default();
        win.tabs.push(Tab::new("a".into(), PaneId(1)));
        let mut b = Tab::new("b".into(), PaneId(2));
        let any = Rect {
            x: 0,
            y: 0,
            w: 800,
            h: 400,
        };
        assert!(b.split(Dir::Right, PaneId(3), any, (0, 0)));
        win.tabs.push(b);
        let shown = |w: &layout::Window| w.tabs[w.active].rects(any);
        let grids = tab_grids(&win, (10, 20), shown);
        let ids: Vec<PaneId> = grids.iter().map(|g| g.0).collect();
        assert_eq!(ids, [PaneId(1), PaneId(2), PaneId(3)], "every tab");
        assert_eq!(grids[0].1, (80, 20));
        assert!(
            grids[1..].iter().all(|g| g.1.0 < 80 && g.1.1 == 20),
            "{grids:?}"
        );
    }

    /// The window shows with its first frame, never blank before it, and
    /// shows anyway when no frame comes.
    #[test]
    fn app_the_window_shows_with_its_first_frame() {
        let t0 = Instant::now();
        let until = Some(t0 + FIRST_FRAME);
        assert!(shows(until, true, t0), "a frame");
        assert!(!shows(until, false, t0), "nothing to show yet");
        assert!(shows(until, false, t0 + FIRST_FRAME), "no frame in time");
        assert!(!shows(None, true, t0), "already shown");
    }

    /// Windows ends blitz for an update restart without the save at exit,
    /// so a moved window is saved like a new split, and blitz asks to be
    /// started again.
    #[test]
    fn app_a_restart_for_an_update_finds_the_window_where_it_was() {
        use windows::Win32::System::Recovery::{
            GetApplicationRestartSettings, RESTART_NO_CRASH, RESTART_NO_HANG,
        };
        use windows::Win32::System::Threading::GetCurrentProcess;
        let win = layout::Window {
            tabs: vec![Tab::new("a".into(), PaneId(1))],
            ..Default::default()
        };
        let s = session::State::capture(&win, Geometry::default(), |_| PaneMeta::default());
        assert!(changed(None, &s));
        assert!(!changed(Some(&s), &s));
        let mut moved = s.clone();
        moved.window.x = 40;
        assert!(changed(Some(&s), &moved), "the place alone");
        restart_after_reboot();
        let mut buf = [0u16; 64];
        let (mut len, mut flags) = (buf.len() as u32, 0);
        let line = windows::core::PWSTR(buf.as_mut_ptr());
        // SAFETY: a buffer the call is told the length of, and a u32.
        unsafe {
            GetApplicationRestartSettings(
                GetCurrentProcess(),
                Some(line),
                &mut len,
                Some(&mut flags),
            )
        }
        .expect("registered");
        assert_eq!(flags, (RESTART_NO_CRASH | RESTART_NO_HANG).0);
    }

    /// A window moved to another monitor and maximized there opens
    /// maximized on that monitor next time, not on the one it started on.
    #[test]
    fn app_the_window_comes_back_where_it_was() {
        let at = |x, y, w, h, maximized| Geometry {
            x,
            y,
            w,
            h,
            maximized,
        };
        let first = at(100, 100, 800, 600, false);
        let moved = placement(first, at(2100, 100, 800, 600, false), false, false);
        assert_eq!(moved, at(2100, 100, 800, 600, false));
        let maxed = placement(moved, at(1912, -8, 2576, 1416, true), false, false);
        assert_eq!(maxed, at(2100, 100, 800, 600, true));
        // Minimized and full screen are no places to come back to.
        let hidden = at(-32000, -32000, 160, 28, false);
        assert_eq!(placement(maxed, hidden, true, false), maxed);
        assert_eq!(
            placement(maxed, at(1920, 0, 2560, 1440, false), false, true),
            maxed
        );
        let back = placement(maxed, at(2100, 100, 800, 600, false), false, false);
        assert_eq!(back, moved, "restored");
        // Win+Shift+Left takes the maximized window to the first monitor.
        let left = placement(maxed, at(-8, -8, 1936, 1056, true), false, false);
        assert_eq!(left, at(560, 220, 800, 600, true));
        let again = placement(left, at(-8, -8, 1936, 1056, true), false, false);
        assert_eq!(again, left, "already there");
    }

    /// The smallest window still has room for the rail or the sidebar and
    /// one pane of the smallest size, at any scale.
    #[test]
    fn app_the_window_never_gets_smaller_than_one_pane() {
        for expanded in [false, true] {
            let mut win = layout::Window {
                sidebar_expanded: expanded,
                ..Default::default()
            };
            win.tabs.push(Tab::new("a".into(), PaneId(1)));
            win.tabs.push(Tab::new("b".into(), PaneId(2)));
            for (cell, scale) in [((8, 16), 1.0), ((12, 24), 1.5), ((16, 32), 2.0)] {
                let th = cell.1 as i32;
                let min = min_window(&win, cell, (cell.0 / 2, cell.1), scale);
                let size = (min.width as i32, min.height as i32);
                let area = chrome::area(&win, size, scale, None, cell.0 as i32 / 2);
                assert!(area.x > 0, "the side is shown");
                let at = format!("{scale} {expanded}");
                assert_eq!(
                    (area.w, area.h),
                    pane_min(cell, th, scale, expanded),
                    "{at}"
                );
                let frame = chrome::pane_frame(scale, expanded, true, th).0;
                assert_eq!((area.w - frame) / cell.0 as i32, layout::MIN_COLS, "{at}");
            }
        }
        let one = layout::Window {
            tabs: vec![Tab::new("a".into(), PaneId(1))],
            ..Default::default()
        };
        let (w, h) = pane_min((8, 16), 16, 1.0, true);
        let min = min_window(&one, (8, 16), (4, 16), 1.0);
        assert_eq!(min, PhysicalSize::new(w as u32, h as u32), "no rail");
    }

    #[test]
    fn app_an_elevated_window_says_so_in_its_title() {
        assert_eq!(window_title(0, "", false), "blitz");
        assert_eq!(window_title(0, "~/shop", false), "~/shop");
        assert_eq!(window_title(0, "", true), "Administrator: blitz");
        assert_eq!(window_title(0, "~/shop", true), "Administrator: ~/shop");
    }

    /// Started from the Start menu, a pin or Win+R, blitz runs in its own
    /// folder or System32; the first pane opens in the profile instead.
    #[test]
    fn app_the_first_pane_never_opens_in_the_install_or_system_folder() {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
        let avoid = not_a_start();
        let exe = std::env::current_exe().expect("exe");
        let own = exe.parent().expect("folder").to_path_buf();
        assert!(avoid.contains(&own), "{avoid:?}");
        let system = avoid
            .iter()
            .find(|d| d.ends_with("System32") || d.ends_with("system32"));
        let system = system.expect("System32").clone();
        assert_eq!(first_dir(Some(own), &avoid, None), home);
        assert_eq!(first_dir(Some(system.clone()), &avoid, None), home);
        // However Windows spells it.
        let shouted = PathBuf::from(format!("{}\\", system.display()).to_uppercase());
        assert_eq!(first_dir(Some(shouted), &avoid, None), home);
        let dev = PathBuf::from(r"C:\dev\shop");
        assert_eq!(first_dir(Some(dev.clone()), &avoid, None), Some(dev));
        assert_eq!(first_dir(None, &avoid, None), home);
    }

    #[test]
    fn app_alerts_once_per_session_every_ten_seconds() {
        let t0 = Instant::now();
        let c = Config::default();
        let mut last = None;
        let alert = |state, last: &mut Option<Instant>, s| {
            alert(state, &c, last, t0 + Duration::from_secs(s))
        };
        assert_eq!(alert(Attn::Working, &mut last, 0), None);
        assert_eq!(alert(Attn::Idle, &mut last, 0), None);
        assert_eq!(last, None, "only alerts count");
        let urgent = Some(Alert {
            flashes: 3,
            toast: true,
            sound: false,
        });
        assert_eq!(alert(Attn::NeedsYou, &mut last, 0), urgent);
        assert_eq!(alert(Attn::Error, &mut last, 9), None);
        assert_eq!(alert(Attn::Error, &mut last, 10), urgent);
        assert_eq!(
            alert(Attn::DoneUnseen, &mut last, 20),
            Some(Alert {
                flashes: 1,
                toast: false,
                sound: false,
            })
        );
        // Another session has its own limit.
        assert_eq!(alert(Attn::NeedsYou, &mut None, 21), urgent);
    }

    #[test]
    fn app_badge_has_the_sidebar_colours() {
        // On a light theme the accent itself is too faint on the badge's
        // background; the mark the sidebar draws is not.
        let ui = crate::theme::blitz(true).ui;
        assert_ne!(ui.mark, ui.accent);
        assert_eq!(badge_look(Some(Attn::NeedsYou), &ui).0, ui.mark);
        assert_eq!(badge_look(Some(Attn::Error), &ui).0, ui.error);
        assert_eq!(
            badge_look(Some(Attn::DoneUnseen), &ui),
            (ui.name, true, "A session finished")
        );
    }

    #[test]
    fn app_badge_shows_the_session_that_most_wants_you() {
        use Attn::*;
        let badge = |s: &[Attn]| badge_state(s.iter().copied());
        assert_eq!(badge(&[]), None);
        assert_eq!(badge(&[Idle, Working]), None, "nothing to see");
        assert_eq!(badge(&[Working, DoneUnseen, Idle]), Some(DoneUnseen));
        assert_eq!(badge(&[DoneUnseen, Error]), Some(Error));
        assert_eq!(badge(&[Error, NeedsYou, DoneUnseen]), Some(NeedsYou));
    }

    /// A flash with no count goes on until blitz is in front; a few are
    /// enough, as the button stays lit after them.
    #[test]
    fn app_flashes_a_few_times_and_none_when_off() {
        let now = Instant::now();
        for state in [Attn::NeedsYou, Attn::Error, Attn::DoneUnseen] {
            let a = alert(state, &Config::default(), &mut None, now).expect("an alert");
            assert!((1..=3).contains(&a.flashes), "{state:?}");
        }
        let off = Config {
            flash: false,
            toasts: "off".into(),
            ..Config::default()
        };
        let mut last = None;
        assert_eq!(alert(Attn::NeedsYou, &off, &mut last, now), None);
        assert_eq!(last, None, "nothing happened, so nothing to space out");
    }

    #[test]
    fn app_notifies_of_sessions_that_need_you_and_of_finished_ones_if_asked() {
        let now = Instant::now();
        let toast = |toasts: &str, state| {
            let c = Config {
                toasts: toasts.into(),
                ..Config::default()
            };
            alert(state, &c, &mut None, now).is_some_and(|a| a.toast)
        };
        for state in [Attn::NeedsYou, Attn::Error] {
            assert!(toast("needs-you", state), "{state:?}");
            assert!(toast("all", state), "{state:?}");
            assert!(!toast("off", state), "{state:?}");
        }
        assert!(!toast("needs-you", Attn::DoneUnseen));
        assert!(toast("all", Attn::DoneUnseen));
        // With the flash off too, a notification is still news.
        let quiet = Config {
            flash: false,
            ..Config::default()
        };
        let a = alert(Attn::NeedsYou, &quiet, &mut None, now);
        assert_eq!(a.map(|a| (a.flashes, a.toast)), Some((0, true)));
    }

    #[test]
    fn app_sounds_once_and_only_when_asked() {
        let now = Instant::now();
        let alert = |toasts: &str, sound, state| {
            let c = Config {
                toasts: toasts.into(),
                sound,
                ..Config::default()
            };
            alert(state, &c, &mut None, now)
        };
        // As if each notification asked for was shown.
        let beeps = |a: Option<Alert>| a.is_some_and(|a| a.beeps(a.toast));
        assert!(!beeps(alert("off", false, Attn::NeedsYou)));
        assert!(beeps(alert("off", true, Attn::NeedsYou)));
        // The notification makes it.
        assert!(!beeps(alert("all", true, Attn::DoneUnseen)));
        assert!(beeps(alert("needs-you", true, Attn::DoneUnseen)));
        // Unless Windows did not show it, say as the user turned blitz's
        // notifications off there.
        let a = alert("all", true, Attn::NeedsYou).expect("an alert");
        assert!(a.toast && a.beeps(false));
        // A sound is enough of an alert by itself.
        let c = Config {
            flash: false,
            toasts: "off".into(),
            sound: true,
            ..Config::default()
        };
        let a = super::alert(Attn::Error, &c, &mut None, now);
        assert!(a.is_some_and(|a| a.beeps(false) && a.flashes == 0));
    }

    #[test]
    fn app_jumps_skip_the_focused_session_only_while_blitz_is_in_front() {
        let t0 = Instant::now();
        let (a, b) = (PaneId(1), PaneId(2));
        let attn = |state, since| {
            let mut p = crate::attention::PaneAttn::new(t0);
            (p.state, p.since) = (state, since);
            p
        };
        let sessions = || {
            [
                (a, attn(Attn::NeedsYou, t0)),
                (b, attn(Attn::NeedsYou, t0 + Duration::from_secs(1))),
            ]
            .into_iter()
        };
        assert_eq!(jump_to(sessions(), Some(a), true), Some(b));
        assert_eq!(jump_to(sessions(), Some(a), false), Some(a));
        assert_eq!(jump_to(sessions(), None, true), Some(a));
        let idle = [(a, attn(Attn::Idle, t0))].into_iter();
        assert_eq!(jump_to(idle, Some(a), false), None, "nothing waits");
    }

    #[test]
    fn app_says_the_jump_key_is_taken_only_by_another_program() {
        assert!(!jump_key_lost(true, || unreachable!()));
        assert!(jump_key_lost(false, || false));
        assert!(!jump_key_lost(false, || true), "another blitz has it");
    }

    #[test]
    fn app_keeps_the_pc_awake_only_while_a_session_works_and_if_asked() {
        use Attn::*;
        let awake = |on, s: &[Attn]| stays_awake(on, s.iter().copied());
        assert!(awake(true, &[Idle, Working, NeedsYou]));
        assert!(!awake(false, &[Working]), "off by default");
        assert!(!awake(true, &[]));
        // Waiting for the user is not working.
        assert!(!awake(true, &[NeedsYou, DoneUnseen, Error, Idle]));
    }

    #[test]
    fn app_notifications_come_down_once_seen_or_no_longer_wanted() {
        use Attn::*;
        for state in [NeedsYou, Error, DoneUnseen] {
            assert!(!untoasts(false, state), "{state:?} still wants you");
            assert!(untoasts(true, state), "{state:?} seen");
        }
        assert!(untoasts(false, Working), "answered elsewhere");
        assert!(untoasts(false, Idle));
    }

    #[test]
    fn app_notifications_name_the_session_and_say_what_it_wants() {
        let s = |state| chrome::Session {
            id: crate::layout::PaneId(3),
            name: "pwsh 3".into(),
            cwd: r"C:\dev\blitz".into(),
            branch: None,
            state,
            since: Instant::now(),
            num: None,
            turn: None,
            took: None,
            seen: false,
            msg: "Bash: cargo test".into(),
            progress: None,
            exit_code: None,
            below: 0,
        };
        assert_eq!(
            toast_text(&s(Attn::NeedsYou)),
            ["pwsh 3 needs you", "Bash: cargo test", r"C:\dev\blitz"]
        );
        assert_eq!(toast_text(&s(Attn::Error))[0], "pwsh 3 failed");
        assert_eq!(toast_text(&s(Attn::DoneUnseen))[0], "pwsh 3 finished");
        // A session that shares its name has its number, as in the sidebar.
        let twin = chrome::Session {
            name: "claude".into(),
            num: Some(2),
            ..s(Attn::NeedsYou)
        };
        assert_eq!(toast_text(&twin)[0], "claude 2 needs you");
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
        for t in ["\u{25d0} Fix the tests", "\u{25d1} x", "\u{25d3}"] {
            assert!(claude_working_title(t), "{t}");
        }
        for t in ["\u{2733} Fix the tests", "", "pwsh", "x \u{25d0}"] {
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
        assert!(a.scripted());
        // A capture is a test run too, and the user's own launches are not.
        let parse = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            Args::parse(&args).expect("parse")
        };
        assert!(parse(&["--capture", "f.bmp"]).scripted());
        assert!(!parse(&["--cwd", "C:\\", "--new-window"]).scripted());
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
        // A folder alone is the same as --cwd, but it has to be one: a
        // mistyped command opens no window.
        let here = std::env::temp_dir();
        assert_eq!(parse(&[here.to_str().expect("UTF-8")]).cwd, Some(here));
        let a = parse(&["--new-window", "C:\""]);
        assert!(a.new_window);
        assert_eq!(a.cwd, Some(r"C:\".into()));
        let bad = Args::parse(&["stup".into()]).err();
        assert_eq!(bad.as_deref(), Some("no such folder: stup"));
        // A relative one is the folder it names now, not later.
        let here = std::env::current_dir().expect("a current folder");
        assert_eq!(parse(&["."]).cwd.as_ref(), Some(&here));
        assert_eq!(parse(&["--cwd", "."]).cwd, Some(here));
    }

    /// A launch that a running blitz did not take must not open a second
    /// main window, which would restore and resume the same sessions.
    #[test]
    fn a_launch_blitz_did_not_take_leaves_the_session_alone() {
        let mut a = Args::default();
        assert!(!a.handed_off(None), "none running");
        assert!(!a.new_window, "the main window");
        assert!(a.handed_off(Some(true)), "taken");
        assert!(!a.handed_off(Some(false)), "hung or refused");
        assert!(a.new_window, "a window of its own");
    }

    #[test]
    fn the_first_hint_names_the_keys_as_bound() {
        assert_eq!(
            first_hint(&[]),
            "Ctrl+Shift+P every action \u{b7} Ctrl+Shift+R split \u{b7} \
             Ctrl+Shift+J the session that needs you \u{b7} Ctrl+, settings"
        );
        let user = [
            keymap::binding("alt+p=command_palette").expect("a binding"),
            keymap::binding("ctrl+shift+r=none").expect("a binding"),
            keymap::binding("ctrl+shift+j=none").expect("a binding"),
        ];
        assert_eq!(
            first_hint(&user),
            "Alt+P every action \u{b7} Ctrl+, settings"
        );
    }

    #[test]
    fn a_launch_from_the_windows_folder_starts_at_home() {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
        let root = Some(Path::new(r"C:\Windows"));
        // Started at sign-in, the folder is System32, in any case.
        for at in [
            r"C:\Windows\system32",
            r"C:\WINDOWS\System32",
            r"C:\Windows",
        ] {
            assert_eq!(first_dir(Some(at.into()), &[], root), home, "{at}");
        }
        assert_eq!(first_dir(None, &[], root), home);
        for at in [r"C:\src\blitz", r"C:\WindowsApps", r"D:\Windows"] {
            let at = Some(PathBuf::from(at));
            assert_eq!(first_dir(at.clone(), &[], root), at);
        }
        let at = Some(PathBuf::from(r"C:\Windows\system32"));
        assert_eq!(first_dir(at.clone(), &[], None), at);
    }

    #[test]
    fn an_update_left_for_close_follows_the_banner() {
        let setup = || crate::update::Installer {
            path: "setup.exe".into(),
            sum: String::new(),
        };
        let left = |v: &str, got: bool| (v.to_string(), got.then(setup));
        // Pressed again for the same release, nothing downloads twice; for
        // a newer one that took the banner, it does.
        assert!(arms(None, "0.0.5"));
        assert!(!arms(Some(&left("0.0.5", false)), "0.0.5"));
        assert!(!arms(Some(&left("0.0.5", true)), "0.0.5"));
        assert!(arms(Some(&left("0.0.5", true)), "0.0.6"));
        // Hiding the banner drops it, but not a restart asked for while it
        // downloads.
        assert_eq!(kept_at_close(Some(left("0.0.5", true)), false), None);
        assert_eq!(kept_at_close(Some(left("0.0.5", false)), false), None);
        assert_eq!(
            kept_at_close(Some(left("0.0.5", false)), true),
            Some(left("0.0.5", false))
        );
        assert_eq!(kept_at_close(None, true), None);
    }

    #[test]
    fn a_click_on_the_banner_never_updates() {
        let strip = Rect {
            x: 240,
            y: 578,
            w: 740,
            h: 22,
        };
        let close = Rect {
            x: 958,
            w: 22,
            ..strip
        };
        let banner = Some((strip, close));
        assert_eq!(banner_click(banner, 300, 590), Some(BannerClick::Notes));
        assert_eq!(banner_click(banner, 957, 578), Some(BannerClick::Notes));
        assert_eq!(banner_click(banner, 958, 578), Some(BannerClick::Close));
        assert_eq!(banner_click(banner, 979, 599), Some(BannerClick::Close));
        for (x, y) in [(239, 590), (300, 577), (980, 590), (300, 600)] {
            assert_eq!(banner_click(banner, x, y), None, "{x},{y}");
        }
        assert_eq!(banner_click(None, 300, 590), None);
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
    /// in its pane; other notices stay.
    #[test]
    fn notices_go_at_the_next_key_that_does_not_answer_them() {
        let close = Some(Action::ClosePane);
        for here in [true, false] {
            let ask = Ask::ClosePane;
            assert!(!ask.gone(close, here), "answered");
            assert!(
                !ask.gone(Some(Action::Palette), here),
                "answered from the palette"
            );
            assert!(ask.gone(None, here), "typing");
            assert!(ask.gone(Some(Action::Update), here), "another shortcut");
            let paste = Ask::Paste("a\nb".into());
            assert!(!paste.gone(Some(Action::Paste), here));
            assert!(paste.gone(close, here));
            assert!(!Ask::Update.gone(Some(Action::Update), here));
            assert!(Ask::Update.gone(Some(Action::Copy), here));
            assert_eq!(Ask::Key.gone(None, here), here, "an error, read");
            assert_eq!(Ask::Key.gone(close, here), here);
            assert!(!Ask::Nothing.gone(None, here));
            assert!(!Ask::CloseTab.gone(Some(Action::CloseTab), here));
            assert!(Ask::CloseTab.gone(close, here), "not the whole tab");
            assert!(
                Ask::Quit.gone(None, here),
                "closing the window, then typing"
            );
            assert!(Ask::Quit.gone(Some(Action::Palette), here));
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
        let run = |a| (Pick::Run(a), keymap::label(a).to_string());
        assert_eq!(c.matches().len(), keymap::ACTIONS.len() - 10, "not itself");
        c.filter = "tab".into();
        assert!(c.matches().iter().all(|m| m.1 != "Go to the last tab"));
        c.filter = "Split R".into();
        assert_eq!(c.matches(), [run(Action::SplitRight)]);
        c.filter = "font_size_up".into();
        assert_eq!(c.matches(), [run(Action::FontSize(1))]);
        c.filter = "claude".into();
        assert_eq!(
            c.matches(),
            [run(Action::NewClaude), run(Action::ClaudeSetup)]
        );
        // Actions with no keys are here too.
        c.filter = "claude setup".into();
        assert_eq!(c.matches(), [run(Action::ClaudeSetup)]);
        c.filter = "reset term".into();
        assert_eq!(c.matches(), [run(Action::Reset)]);
        c.filter = "clear".into();
        assert_eq!(c.matches(), [run(Action::ClearScrollback)]);
        c.move_by(5);
        assert_eq!(c.sel, 0, "one match");
        // What has nothing to do is left out.
        c.filter = "update".into();
        assert_eq!(c.matches(), [run(Action::Update)]);
        c.hidden = vec![Action::Update];
        let search = (Pick::Settings, "Search settings for \"update\"".into());
        assert_eq!(c.matches(), [search]);
    }

    #[test]
    fn palette_lists_sessions_to_go_to() {
        let t0 = Instant::now();
        let s = |id, name: &str, cwd: &str, branch: Option<&str>| chrome::Session {
            id: PaneId(id),
            name: name.into(),
            cwd: cwd.into(),
            branch: branch.map(Into::into),
            state: Attn::Idle,
            since: t0,
            msg: String::new(),
            num: None,
            turn: None,
            took: None,
            seen: false,
            progress: None,
            exit_code: None,
            below: 0,
        };
        let a = s(1, "claude 1", r"C:\dev\shop", Some("main"));
        assert_eq!(session_row(&a), "claude 1 \u{b7} shop \u{b7} main");
        assert_eq!(
            session_row(&s(2, "pwsh 2", r"C:\", None)),
            "pwsh 2 \u{b7} C:\\"
        );
        assert_eq!(session_row(&s(3, "cmd 3", "", None)), "cmd 3");
        let mut c = Commands {
            sessions: Some(vec![
                (PaneId(1), session_row(&a), "needs you".into()),
                (
                    PaneId(2),
                    "pwsh 2 \u{b7} api".into(),
                    "working \u{b7} 5s".into(),
                ),
            ]),
            ..Commands::default()
        };
        let picks = |c: &Commands| c.matches().into_iter().map(|m| m.0).collect::<Vec<_>>();
        assert_eq!(picks(&c), [Pick::Show(PaneId(1)), Pick::Show(PaneId(2))]);
        // By name, folder, branch or state; never the settings.
        for (typed, want) in [("shop", 1), ("MAIN", 1), ("needs", 1), ("work", 2)] {
            c.filter = typed.into();
            assert_eq!(picks(&c), [Pick::Show(PaneId(want))], "{typed}");
        }
        c.filter = "zzz".into();
        assert!(c.matches().is_empty());
    }

    #[test]
    fn palette_searches_the_settings_when_no_action_matches() {
        let mut c = Commands {
            filter: " scroll lines ".into(),
            ..Commands::default()
        };
        let search = (
            Pick::Settings,
            "Search settings for \"scroll lines\"".into(),
        );
        assert_eq!(c.matches(), [search]);
        c.filter = "  ".into();
        assert_eq!(
            c.matches().len(),
            keymap::ACTIONS.len() - 10,
            "nothing typed"
        );
    }

    #[test]
    fn a_shell_that_cannot_start_says_why_and_what_runs_instead() {
        let err = std::io::Error::other("pwshh.exe is not on PATH");
        let text = shell_failed("pwshh", &err, "pwsh", Some("Ctrl+,".into()));
        assert_eq!(
            text,
            "The shell pwshh could not start (pwshh.exe is not on PATH); using pwsh \u{b7} Ctrl+, settings"
        );
        assert!(
            !shell_failed("x", &err, "cmd", None).contains('\u{b7}'),
            "unbound"
        );
    }

    /// The note about a shell that could not start covers the pane's last
    /// row, so typing there, having read it, puts it away; a note that a
    /// session exited or stopped stays.
    #[test]
    fn typing_puts_away_only_the_note_it_ends() {
        // The shell note is an error, which a key in its pane puts away.
        assert!(Ask::Key.gone(None, true));
        assert!(!Ask::Key.gone(None, false), "a key in another pane");
        // A note that a session exited stays.
        assert!(!Ask::Nothing.gone(None, true));
    }

    #[test]
    fn a_failed_start_names_the_settings_file() {
        let dir = Path::new(r"C:\Users\me\AppData\Roaming\blitz");
        let text = start_failed("no window", Some(dir));
        assert!(
            text.starts_with("blitz could not start: no window"),
            "{text}"
        );
        assert!(text.ends_with(r"blitz\config.toml"), "{text}");
        assert_eq!(start_failed("x", None), "blitz could not start: x");
    }

    #[test]
    fn app_copy_without_indent_drops_claude_codes_gutter() {
        let reply = "\u{23FA} Here is the fix:\r\n  fn main() {\r\n      run();\r\n  }";
        assert_eq!(
            without_indent(reply),
            "Here is the fix:\r\nfn main() {\r\n    run();\r\n}"
        );
        let tool = "  \u{23BF}  src/a.rs\r\n     src/b.rs";
        assert_eq!(without_indent(tool), "src/a.rs\r\nsrc/b.rs");
        // Claude Code marks replies with ● outside macOS.
        let both = "\u{25CF} Bash(ls)\r\n  \u{23BF}  a.rs\r\n     b.rs";
        assert_eq!(without_indent(both), "Bash(ls)\r\n   a.rs\r\n   b.rs");
        // Blank lines stay, and lines are never joined.
        assert_eq!(without_indent("    a\r\n\r\n      b"), "a\r\n\r\n  b");
        assert_eq!(without_indent("x \u{23BF} y"), "x \u{23BF} y");
    }

    #[test]
    fn app_a_program_copy_says_so() {
        assert_eq!(program_copy_notice("é", true), "Program copied 1 character");
        assert_eq!(
            program_copy_notice("ls -la", true),
            "Program copied 6 characters"
        );
        assert_eq!(
            program_copy_notice("ls", false),
            "Clipboard busy; nothing was copied"
        );
    }

    #[test]
    fn app_a_copy_says_what_it_did() {
        assert_eq!(copy_notice("ls", true, false), "Copied 1 line");
        assert_eq!(
            copy_notice("", true, false),
            "Copied 1 line",
            "a blank line"
        );
        assert_eq!(copy_notice("a\r\nb\r\n", true, false), "Copied 3 lines");
        assert_eq!(
            copy_notice("a\r\nb", true, true),
            "Copied 2 lines, as selected before the output changed"
        );
        assert_eq!(
            copy_notice("a\r\nb", false, true),
            "Clipboard busy; nothing was copied"
        );
    }

    #[test]
    fn app_copy_takes_the_selected_text_that_output_rewrote() {
        let pal = crate::theme::dark();
        let mut s = Snapshot::default();
        // "one" is in scrollback, line 0; the screen holds lines 1 and 2.
        let mut t = fed(10, 2, "one\r\ntwo\r\nthree");
        assert!(refresh(&mut t, &mut s, &pal, None));
        let mut sel = select(&t, (0, 0), (2, 9));
        t.feed(b"\x1b[2;1HTHREE");
        assert!(!refresh(&mut t, &mut s, &pal, Some(&mut sel)));
        assert_eq!(
            sel.last_text(&t, &pal).as_deref(),
            Some("one\r\ntwo\r\nthree")
        );
        // On the screen only: kept even once the lines are gone.
        let mut sel = select(&t, (1, 0), (1, 9));
        t.feed(b"\x1b[?1049h");
        assert!(!refresh(&mut t, &mut s, &pal, Some(&mut sel)));
        assert_eq!(sel.last_text(&t, &pal).as_deref(), Some("two"));
        t.feed(b"\x1b[?1049l");
        // Partly in scrollback that is gone: nothing to take.
        let mut sel = select(&t, (0, 0), (2, 9));
        t.resize(8, 2);
        assert!(!refresh(&mut t, &mut s, &pal, Some(&mut sel)));
        assert_eq!(sel.last_text(&t, &pal), None);
    }

    #[test]
    fn app_ctrl_shift_c_with_nothing_to_copy_never_interrupts() {
        let chord = |vk, key, shift| {
            let mut k = input(vk, true, key, "c");
            (k.mods.lctrl, k.mods.lshift) = (true, shift);
            k
        };
        let ctrl_c = chord(0x43, vt::Key::Char('c'), false);
        let ctrl_shift_c = chord(0x43, vt::Key::Char('c'), true);
        let ctrl_insert = chord(0x2d, vt::Key::Insert, false);
        let legacy = InputModes::default();
        let w32im = InputModes {
            w32im: true,
            ..legacy
        };
        let kitty = InputModes { kitty: 1, ..legacy };
        for m in [legacy, w32im] {
            assert!(eats_copy_key(&ctrl_shift_c, &m), "{m:?}");
        }
        // A key of its own under kitty flags, which Claude Code ignores.
        assert!(!eats_copy_key(&ctrl_shift_c, &kitty));
        for m in [legacy, w32im, kitty] {
            assert!(!eats_copy_key(&ctrl_c, &m), "the interrupt");
            assert!(!eats_copy_key(&ctrl_insert, &m));
        }
        // Ctrl+C copies only a selection in view; scrolled out of view it
        // interrupts, and the other copy keys still copy it.
        assert!(copy_key_copies(&ctrl_c, Some(true)));
        assert!(!copy_key_copies(&ctrl_c, Some(false)));
        assert!(copy_key_copies(&ctrl_c, None), "nothing selected");
        for k in [ctrl_shift_c, ctrl_insert] {
            assert!(copy_key_copies(&k, Some(false)));
        }
    }

    #[test]
    fn app_claude_code_takes_bracketed_pastes_without_asking() {
        let mut t = fed(10, 2, "");
        assert!(!paste_trusted(&t, true), "no bracketed paste");
        t.feed(b"\x1b[?2004h");
        assert!(paste_trusted(&t, true));
        assert!(!paste_trusted(&t, false), "a shell asks once first");
        t.confirm_paste();
        assert!(paste_trusted(&t, false));
    }

    #[test]
    fn app_one_line_pastes_without_its_line_break() {
        for (text, pasted) in [
            ("ls -la\r\n", "ls -la"),
            ("ls -la\n", "ls -la"),
            ("ls -la\r", "ls -la"),
            ("ls -la", "ls -la"),
            ("\r\n", ""),
            // More than one line break: the text stays as copied.
            ("a\r\nb\r\n", "a\r\nb\r\n"),
            ("a\n\n", "a\n\n"),
            ("a\nb", "a\nb"),
        ] {
            assert_eq!(trim_paste(text), pasted, "{text:?}");
        }
        assert!(!vt::keys::needs_paste_confirm(
            trim_paste("git status\r\n"),
            false,
            false
        ));
    }

    #[test]
    fn app_files_paste_as_paths_quoted_only_with_spaces() {
        let paths = [r"C:\some dir\shot.png", r"D:\b.txt"].map(PathBuf::from);
        assert_eq!(quote_paths(&paths), r#""C:\some dir\shot.png" D:\b.txt"#);
        assert_eq!(quote_paths(&paths[1..]), r"D:\b.txt");
    }

    #[test]
    fn app_drops_paste_into_a_pane_or_open_folders_from_the_sidebar() {
        let dir = std::env::temp_dir().join(format!("blitz drop {}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("shot.png");
        std::fs::write(&file, "").expect("file");
        let paths = vec![file.clone(), dir.clone()];
        let pane = dropped(paths.clone(), (Some(PaneId(2)), false));
        let side = dropped(paths.clone(), (Some(PaneId(1)), true));
        let nowhere = dropped(paths, (None, false));
        let _ = std::fs::remove_dir_all(&dir);
        let text = format!("\"{}\" \"{}\"", file.display(), dir.display());
        assert_eq!(pane, Some(Dropped::Paste(PaneId(2), text)));
        assert_eq!(side, Some(Dropped::Open(vec![dir])), "folders only");
        assert_eq!(nowhere, None);
    }

    #[test]
    fn app_an_image_reaches_claude_code_as_alt_v() {
        let legacy = InputModes::default();
        assert_eq!(alt_v(&legacy), b"\x1bv");
        let kitty = InputModes { kitty: 1, ..legacy };
        assert_eq!(alt_v(&kitty), b"\x1b[118;3u");
        let w32im = InputModes {
            w32im: true,
            ..legacy
        };
        assert_eq!(alt_v(&w32im), b"\x1b[86;47;118;1;2;1_\x1b[86;47;118;0;2;1_");
    }

    #[test]
    fn app_nothing_is_pasted_into_an_exited_pane() {
        assert_eq!(paste_refused("pwsh 3", None), None);
        assert_eq!(
            paste_refused("pwsh 3", Some(1)).as_deref(),
            Some("pwsh 3 exited with code 1, so nothing was pasted \u{b7} Enter close")
        );
    }

    #[test]
    fn app_paste_question_says_what_and_which_key() {
        assert_eq!(
            paste_question("\r\ngit status\r\ngit diff\r\n", Some("Ctrl+V")),
            "Paste 3 lines starting \"git status\"? Press Ctrl+V again"
        );
        // The first 40 characters, without tabs or other controls.
        let long = format!("{}\tyz", "x".repeat(39));
        assert_eq!(
            paste_question(&long, Some("Shift+Insert")),
            format!(
                "Paste 1 line starting \"{}y\u{2026}\"? Press Shift+Insert again",
                "x".repeat(39)
            )
        );
        assert_eq!(
            paste_question("a\nb", None),
            "Paste 2 lines starting \"a\"? Paste again"
        );
    }

    #[test]
    fn app_a_notice_too_long_for_its_pane_goes_on_to_more_rows() {
        let ask = "Paste 2 lines starting \"echo a\"? Press Ctrl+V again";
        assert_eq!(notice_rows(ask, (80, 24)), [format!(" {ask}")]);
        assert_eq!(
            notice_rows(ask, (34, 24)),
            [" Paste 2 lines starting \"echo a\"?", " Press Ctrl+V again"]
        );
        let rows = notice_rows(ask, (12, 24));
        assert_eq!(rows.last().map(String::as_str), Some(" again"), "{rows:?}");
        // No more rows than the pane has, the last cut with an ellipsis.
        let rows = notice_rows(ask, (12, 2));
        assert_eq!(rows.len(), 2);
        assert!(rows[1].ends_with('\u{2026}'), "{rows:?}");
    }

    #[test]
    fn palette_opens_a_tab_on_each_shell() {
        let wsl = r"C:\Windows\System32\wsl.exe -d Ubuntu";
        let mut c = Commands::new(vec![
            ("Automatic (PowerShell 7)".into(), String::new()),
            (
                "Command Prompt".into(),
                r"C:\Windows\System32\cmd.exe".into(),
            ),
            ("Ubuntu".into(), wsl.into()),
        ]);
        c.filter = "new tab".into();
        let labels: Vec<String> = c.matches().into_iter().map(|m| m.1).collect();
        // Right after New tab, before the actions that follow it.
        assert_eq!(
            labels,
            [
                "New tab",
                "New tab: Command Prompt",
                "New tab: Ubuntu",
                "Move the pane to a new tab"
            ]
        );
        c.filter = "wsl".into();
        assert_eq!(
            c.matches(),
            [(Pick::Shell(wsl.into()), "New tab: Ubuntu".into())]
        );
    }

    #[test]
    fn saved_output_keeps_the_last_lines() {
        assert_eq!(last_lines("\n\na\nb\nc\n\n\n", 2), "b\nc");
        assert_eq!(last_lines("a\n\nb", 10), "a\n\nb");
        assert_eq!(last_lines("  a\n", 10), "  a");
        assert_eq!(last_lines("\n\n", 10), "");
    }

    #[test]
    fn the_count_of_hidden_sessions_goes_round_them() {
        let now = Instant::now();
        let ids = |v: &[u32]| -> Vec<(PaneId, crate::attention::PaneAttn)> {
            let idle = crate::attention::PaneAttn::new(now);
            v.iter().map(|&i| (PaneId(i), idle)).collect()
        };
        let hidden = ids(&[7, 8, 9]);
        // Nothing waits: from a pane in view, the first; then each in turn.
        assert_eq!(hidden_target(&hidden, Some(PaneId(1))), Some(PaneId(7)));
        assert_eq!(hidden_target(&hidden, Some(PaneId(7))), Some(PaneId(8)));
        assert_eq!(hidden_target(&hidden, Some(PaneId(8))), Some(PaneId(9)));
        assert_eq!(hidden_target(&hidden, Some(PaneId(9))), Some(PaneId(7)));
        // One that needs you comes first, unless it is already shown.
        let mut waiting = hidden.clone();
        waiting[2].1.state = Attn::NeedsYou;
        assert_eq!(hidden_target(&waiting, Some(PaneId(7))), Some(PaneId(9)));
        assert_eq!(hidden_target(&waiting, Some(PaneId(9))), Some(PaneId(7)));
        // Only the focused one hidden: nowhere to go.
        assert_eq!(hidden_target(&ids(&[7]), Some(PaneId(7))), None);
        assert_eq!(hidden_target(&[], None), None);
    }

    #[test]
    fn the_pointer_shows_what_is_under_it() {
        assert_eq!(pointer(None, false, None), CursorIcon::Default);
        assert_eq!(pointer(None, true, None), CursorIcon::Pointer);
        assert_eq!(pointer(Some(Axis::Row), true, None), CursorIcon::ColResize);
        assert_eq!(
            pointer(Some(Axis::Column), false, None),
            CursorIcon::RowResize
        );
    }
}
