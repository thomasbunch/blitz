//! The window, its event loop and the frame loop.

// One process hosts every session, so a failed HRESULT must never panic.
#![deny(clippy::unwrap_used)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vt::{Event, InputModes, KeyInput, Mods, MouseEv, MouseKind, MouseMode, Palette, Snapshot};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, GetKeyboardState};
use windows::Win32::UI::WindowsAndMessaging::{
    MSG, TranslateMessage, WM_CHAR, WM_DEADCHAR, WM_KEYDOWN, WM_KEYUP, WM_SYSCHAR, WM_SYSDEADCHAR,
    WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::windows::EventLoopBuilderExtWindows;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{UserAttentionType, Window, WindowId};

use crate::attention::{Attn, Ev};
use crate::config::{Config, ThemeMode};
use crate::debug::Counters;
use crate::keymap::{self, Action};
use crate::layout::{self, Dir, PaneId, Rect, Tab};
use crate::pane::{Note, Pane, Spawn, git_branch, lock, program_name};
use crate::render::chrome::{self, ChromeModel};
use crate::render::d3d11::{Swapchain, is_device_lost};
use crate::render::{Renderer, text_snapshot, write_bmp};

const VK_PROCESSKEY: u16 = 0xe5;
const VK_PACKET: u16 = 0xe7;
const VK_RETURN: u16 = 0x0d;
const VK_F4: u16 = 0x73;

/// How long a multi-line paste waits for a second Ctrl+V, and closing a
/// busy session for a second Ctrl+Shift+W.
const CONFIRM: Duration = Duration::from_secs(3);
/// How long the notice about the system ConPTY stays up.
const NOTICE: Duration = Duration::from_secs(5);
/// The first look for a newer release waits until startup is done, then
/// one runs a day.
const UPDATE_FIRST: Duration = Duration::from_secs(10);
const UPDATE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// Taskbar flashes per session are at least this far apart.
const FLASH_GAP: Duration = Duration::from_secs(10);
/// Lines scrolled per wheel notch when the program takes no mouse input.
const WHEEL_LINES: isize = 3;

#[derive(Debug)]
pub enum UserEvent {
    Pane(PaneId, Note),
    /// The git branch of a pane's directory, read on another thread.
    Branch(PaneId, String, Option<String>),
    /// Exit with this code: the self-test finished, or `--exit-after`
    /// ran out.
    Finish(i32),
    /// A newer release, by version.
    Update(String),
    /// The installer started, so blitz exits; or why it did not.
    Installed(Result<(), String>),
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
    /// The release of this key was already handled as text.
    skip_up: Option<u16>,
}

enum Input {
    /// A key transition; its text is the second field.
    Key(KeyInput<'static>, String),
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
            // Alt+F4 still closes the window.
            if vk == VK_PROCESSKEY || vk == VK_F4 && held(0x12) && !held(0x11) && !held(0x10) {
                return false;
            }
            if !down && k.skip_up == Some(vk) {
                k.skip_up = None;
                return true;
            }
            let mut text = String::new();
            let input =
                keymap::msg_to_key(vk, msg.lParam.0, &state, keymap::system_layout, &mut text);
            let modifier = matches!(
                input.key,
                vt::Key::Shift | vt::Key::Control | vt::Key::Alt | vt::Key::Super
            );
            if down && !modifier {
                k.chars = vk == VK_PACKET || k.dead && matches!(input.key, vt::Key::Char(_));
                k.dead = false;
            }
            if vk == VK_PACKET || k.chars && down && !modifier {
                k.skip_up = Some(vk);
            } else {
                k.queue.push(Input::Key(owned(&input), text));
            }
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
            if k.chars {
                let unit = msg.wParam.0 as u16;
                let units = match (k.high.take(), unit) {
                    (_, 0xd800..=0xdbff) => {
                        k.high = Some(unit);
                        vec![]
                    }
                    (Some(h), 0xdc00..=0xdfff) => vec![h, unit],
                    _ => vec![unit],
                };
                let text = String::from_utf16_lossy(&units);
                if !text.is_empty() && !text.chars().any(char::is_control) {
                    k.queue.push(Input::Text(text));
                }
            }
            // Text for other keys was already sent from the key message.
            true
        }
        _ => false,
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
    /// Buttons sent to the program as pressed, as a bit set.
    reported: u8,
    tracker: vt::keys::MouseTracker,
    /// Wheel movement not yet turned into whole steps.
    wheel: f64,
    /// A left-button drag is making a selection from this cell.
    anchor: Option<(u16, u16)>,
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
}

struct App {
    args: Args,
    config: Config,
    keys: Rc<RefCell<Keys>>,
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Window>,
    hwnd: isize,
    gfx: Option<Gfx>,
    dark: bool,
    pal: Palette,
    scale: f64,
    /// Tabs and the split tree in each.
    win: layout::Window,
    /// Every session, oldest first, which is the order the sidebar lists.
    views: Vec<View>,
    /// Each session's sidebar row in the last frame, for clicks.
    rows: Vec<(PaneId, Rect)>,
    next_id: u32,
    focused: bool,
    /// A selection in the focused pane.
    selection: Option<((u16, u16), (u16, u16))>,
    mouse: Mouse,
    /// IME composition text, drawn at the cursor.
    preedit: String,
    /// A multi-line paste waiting for its confirming Ctrl+V in the pane
    /// that asked.
    paste: Option<(PaneId, String, Instant)>,
    /// A busy session waiting for a second Ctrl+Shift+W.
    close_confirm: Option<(PaneId, Instant)>,
    /// A newer release: its version and the banner text.
    update: Option<(String, String)>,
    /// Busy sessions, waiting for a second Ctrl+Shift+U.
    update_confirm: Option<Instant>,
    /// The installer is downloading; this pane shows that.
    updating: Option<PaneId>,
    /// The banner strip in the last frame, for clicks.
    banner: Option<Rect>,
    /// The release of this key belongs to a shortcut and is not sent.
    eaten: Option<u16>,
    /// Where the IME was last told the cursor is, in client pixels.
    ime_at: Option<(i32, i32)>,
    /// Checked once the first output shows which ConPTY is running.
    checked_conpty: bool,
    capture_then_exit: bool,
    /// The terminal a running self-test reads: the focused pane's.
    watched: Option<Arc<selftest::Focus>>,
    started: Instant,
    counters: Counters,
    code: i32,
}

impl App {
    fn new(args: Args, keys: Rc<RefCell<Keys>>, proxy: EventLoopProxy<UserEvent>) -> App {
        let config = Config::default();
        let dark = match config.theme {
            ThemeMode::System => !crate::theme::system_is_light(),
            ThemeMode::Dark => true,
            ThemeMode::Light => false,
        };
        App {
            args,
            config,
            keys,
            proxy,
            window: None,
            hwnd: 0,
            gfx: None,
            dark,
            pal: if dark {
                crate::theme::dark()
            } else {
                crate::theme::light()
            },
            scale: 1.0,
            win: layout::Window::default(),
            views: Vec::new(),
            rows: Vec::new(),
            next_id: 1,
            focused: false,
            selection: None,
            mouse: Mouse::default(),
            preedit: String::new(),
            paste: None,
            close_confirm: None,
            update: None,
            update_confirm: None,
            updating: None,
            banner: None,
            eaten: None,
            ime_at: None,
            checked_conpty: false,
            capture_then_exit: false,
            watched: None,
            started: Instant::now(),
            counters: Counters::default(),
            code: 0,
        }
    }

    fn font_px(&self) -> f32 {
        self.config.font_size * 96.0 / 72.0 * self.scale as f32
    }

    /// Creates the window and starts the first session.
    fn start(&mut self, el: &ActiveEventLoop) -> Result<(), String> {
        let attrs = Window::default_attributes()
            .with_title("blitz")
            .with_inner_size(LogicalSize::new(980.0, 620.0));
        let window = el.create_window(attrs).map_err(|e| e.to_string())?;
        window.set_ime_allowed(true);
        self.scale = window.scale_factor();
        if let Ok(h) = window.window_handle()
            && let RawWindowHandle::Win32(h) = h.as_raw()
        {
            self.hwnd = h.hwnd.get();
        }
        let dark = windows::core::BOOL::from(self.dark);
        // SAFETY: a live window and a BOOL-sized value.
        let _ = unsafe {
            DwmSetWindowAttribute(
                HWND(self.hwnd as *mut c_void),
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                (&raw const dark).cast(),
                size_of_val(&dark) as u32,
            )
        };
        self.window = Some(window);
        self.ensure_gfx();

        let cwd = match &self.args.cwd {
            Some(dir) => start_dir(dir),
            None => std::env::current_dir().ok(),
        };
        let id = PaneId(self.next_id);
        let mut win = layout::Window::default();
        win.tabs.push(Tab::new(tab_name(cwd.as_deref()), id));
        let cmd = self.args.cmd.clone();
        self.open(win, id, cmd.as_deref(), cwd)?;

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
        if self.config.check_updates && !scripted && !cfg!(debug_assertions) {
            let proxy = self.proxy.clone();
            std::thread::spawn(move || {
                std::thread::sleep(UPDATE_FIRST);
                loop {
                    if let Some(v) = crate::update::check()
                        && proxy.send_event(UserEvent::Update(v)).is_err()
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
    /// exists, a layout with a pane below the minimum size is refused.
    fn open(
        &mut self,
        win: layout::Window,
        id: PaneId,
        cmd: Option<&str>,
        cwd: Option<PathBuf>,
    ) -> Result<(), String> {
        let grids = self.grids(&win);
        let small =
            |&(_, (c, r)): &(PaneId, (i32, i32))| c < layout::MIN_COLS || r < layout::MIN_ROWS;
        if !self.views.is_empty() && grids.iter().any(small) {
            return Err("no room for another pane".into());
        }
        let fit = |n: i32| n.clamp(1, i32::from(u16::MAX)) as u16;
        let grid = grids
            .iter()
            .find(|g| g.0 == id)
            .map_or((80, 24), |&(_, (c, r))| (fit(c), fit(r)));
        let token = crate::pty::pane_token().map_err(|e| format!("cannot start a session: {e}"))?;
        let launch = match cmd {
            Some(c) => crate::shell::Launch {
                cmdline: c.to_string(),
                env: Vec::new(),
            },
            None => crate::shell::launch(
                &self.config.shell,
                &self.config.shell_args,
                self.config.shell_integration,
                &token,
            ),
        };
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
                dark: self.dark,
                parent: Some(self.hwnd),
                token: &token,
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
        });
        self.find_branch(id);
        self.next_id = id.0 + 1;
        let before = self.focus_id();
        self.win = win;
        self.focus_moved(before);
        Ok(())
    }

    /// Opens a pane in a copy of the layout that `place` changes; tells the
    /// user in the focused pane when that fails. New panes start where the
    /// focused one is.
    fn add(&mut self, place: impl FnOnce(&mut layout::Window, PaneId, Option<&Path>) -> bool) {
        let cwd = start_dir(self.current().map_or("", |v| v.pane.cwd.as_str()));
        let id = PaneId(self.next_id);
        let mut win = self.win.clone();
        if !place(&mut win, id, cwd.as_deref()) {
            return;
        }
        if let Err(e) = self.open(win, id, None, cwd)
            && let Some(id) = self.focus_id()
        {
            self.set_notice(id, e, Some(Instant::now() + NOTICE), false);
        }
    }

    /// Closes a session and its pane. The window closes with the last one.
    fn close(&mut self, el: &ActiveEventLoop, id: PaneId) {
        let before = self.focus_id();
        self.win.close_pane(id);
        // Dropping the pane closes its pseudoconsole.
        self.views.retain(|v| v.pane.id != id);
        if self.views.is_empty() {
            el.exit();
            return;
        }
        self.focus_moved(before);
    }

    /// Catches up after the focused pane may have changed: focus reports,
    /// attention, the selection and the window title.
    fn focus_moved(&mut self, before: Option<PaneId>) {
        self.request_redraw();
        let now = self.focus_id();
        if now == before {
            return;
        }
        self.selection = None;
        self.mouse.anchor = None;
        self.ime_at = None;
        if self.focused {
            for (id, f) in [(before, false), (now, true)] {
                if let Some(v) = id.and_then(|id| self.view(id)) {
                    let mut out = Vec::new();
                    vt::encode_focus(f, &lock(&v.pane.term).input_modes(), &mut out);
                    v.pane.send(out);
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
            light: !self.dark,
            accent: self.config.accent.unwrap_or(crate::theme::ACCENT),
            size: (size.width as i32, size.height as i32),
            scale: self.scale as f32,
            text_cell: self.gfx.as_ref().map_or((6, 12), |g| g.r.small_cell()),
            term_cell: self.cell(),
            now: Instant::now(),
            banner: self.update.as_ref().map(|u| u.1.as_str()),
            preedit,
        }
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
        self.views
            .iter()
            .map(|v| {
                let p = &v.pane;
                chrome::Session {
                    id: p.id,
                    // Programs set the rest of the row, so the session's
                    // number is what tells two look-alike sessions apart.
                    name: format!("{} {}", p.name, p.id.0),
                    cwd: p.cwd.clone(),
                    branch: p.branch.clone(),
                    state: p.attn.state,
                    since: p.attn.since,
                    msg: if p.msg.is_empty() {
                        p.title.clone()
                    } else {
                        p.msg.clone()
                    },
                    progress: None,
                    exit_code: p.exit_code,
                }
            })
            .collect()
    }

    /// Builds the renderer and swap chain if there are none. Never panics:
    /// without them the window just stays blank until the next try.
    fn ensure_gfx(&mut self) {
        if self.gfx.is_some() {
            return;
        }
        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        let built = Renderer::new(false, self.font_px()).and_then(|r| {
            let hwnd = HWND(self.hwnd as *mut c_void);
            let chain = Swapchain::new(&r.gpu, hwnd, size.width, size.height)?;
            Ok(Gfx { r, chain })
        });
        match built {
            Ok(g) => self.gfx = Some(g),
            Err(e) => eprintln!("blitz: renderer: {e}"),
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
            self.selection = None;
            self.request_redraw();
        }
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
            });
        }
        self.request_redraw();
    }

    /// Handles queued key input.
    fn drain_keys(&mut self, el: &ActiveEventLoop) {
        let inputs = std::mem::take(&mut self.keys.borrow_mut().queue);
        for input in inputs {
            match input {
                Input::Text(t) => self.typed(t.into_bytes()),
                Input::Key(k, text) => {
                    let k = KeyInput { text: &text, ..k };
                    self.key(el, &k);
                }
            }
        }
    }

    fn key(&mut self, el: &ActiveEventLoop, k: &KeyInput) {
        if !k.down && self.eaten == Some(k.vk) {
            self.eaten = None;
            return;
        }
        if let Some(a) = keymap::action(k)
            && self.act(el, a)
        {
            self.eaten = Some(k.vk);
            return;
        }
        let Some(v) = self.current() else {
            return;
        };
        if v.pane.exit_code.is_some() {
            if k.down && k.vk == VK_RETURN {
                let id = v.pane.id;
                // The release must not reach the pane that takes focus.
                self.eaten = Some(k.vk);
                self.close(el, id);
            }
            return;
        }
        let mut out = Vec::new();
        vt::encode_key(k, &self.modes(), &mut out);
        let modifier = matches!(
            k.key,
            vt::Key::Shift | vt::Key::Control | vt::Key::Alt | vt::Key::Super
        );
        if k.down && !modifier && !out.is_empty() {
            self.typed(out);
        } else {
            self.send(out);
        }
    }

    /// Sends input the user typed: the view follows the cursor again.
    fn typed(&mut self, bytes: Vec<u8>) {
        if self.selection.take().is_some() {
            self.request_redraw();
        }
        if let Some(v) = self.current() {
            lock(&v.pane.term).scroll_viewport(isize::MIN);
            v.pane.send(bytes);
        }
    }

    /// Runs a shortcut. Returns false when it does not apply right now, in
    /// which case the key goes to the program.
    fn act(&mut self, el: &ActiveEventLoop, a: Action) -> bool {
        let before = self.focus_id();
        match a {
            Action::Copy => {
                let (Some(sel), Some(v)) = (self.selection, self.current()) else {
                    return false;
                };
                let text = selection_text(&v.snap, sel);
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
                    let confirmed = self.paste.take().is_some_and(|(p, t, until)| {
                        p == id && t == text && Instant::now() < until
                    });
                    if !confirmed {
                        let lines = text.lines().count();
                        let until = Instant::now() + CONFIRM;
                        self.paste = Some((id, text, until));
                        self.set_notice(
                            id,
                            format!("Paste {lines} lines? Press Ctrl+V again within 3 s"),
                            Some(until),
                            false,
                        );
                        return true;
                    }
                    if let Some(v) = self.view_mut(id) {
                        v.notice = None;
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
            Action::NewTab => self.add(|win, id, cwd| {
                win.tabs.push(Tab::new(tab_name(cwd), id));
                win.active = win.tabs.len() - 1;
                true
            }),
            Action::ClosePane => {
                let Some(v) = self.current() else {
                    return true;
                };
                let id = v.pane.id;
                let busy = match (v.pane.attn.state, v.pane.exit_code) {
                    (Attn::Working, None) => Some("working"),
                    (Attn::NeedsYou, None) => Some("waiting for you"),
                    _ => None,
                };
                let again = (self.close_confirm.take())
                    .is_some_and(|(p, until)| p == id && Instant::now() < until);
                match busy {
                    Some(what) if !again => {
                        let until = Instant::now() + CONFIRM;
                        self.close_confirm = Some((id, until));
                        let text = format!(
                            "This session is {what}. Press Ctrl+Shift+W again within 3 s to close it"
                        );
                        self.set_notice(id, text, Some(until), false);
                    }
                    _ => self.close(el, id),
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
                self.add(|win, id, _| {
                    // Only the pane minimum matters, and `open` checks that
                    // against the real window.
                    let any = Rect {
                        x: 0,
                        y: 0,
                        w: 1 << 16,
                        h: 1 << 16,
                    };
                    let active = win.active;
                    win.tabs
                        .get_mut(active)
                        .is_some_and(|t| t.split(dir, id, any, (0, 0)))
                });
            }
            Action::ToggleSidebar => {
                self.win.sidebar_expanded = !self.win.sidebar_expanded;
                self.request_redraw();
            }
            Action::Focus(dir) => {
                let (area, active) = (self.tab_area(), self.win.active);
                if let Some(t) = self.win.tabs.get_mut(active) {
                    t.focus_dir(dir, area);
                }
                self.focus_moved(before);
            }
            // The focused session is skipped: the user is already looking
            // at it, and a session that exited stays red until closed.
            Action::JumpToAttention => {
                let waiting = (self.views.iter())
                    .filter(|v| Some(v.pane.id) != before)
                    .map(|v| (v.pane.id, v.pane.attn.state, v.pane.attn.since));
                if let Some(id) = crate::attention::jump_target(waiting) {
                    self.show(id);
                }
            }
            Action::Update => {
                let (Some((v, _)), Some(id)) = (self.update.clone(), before) else {
                    return false;
                };
                if self.updating.is_some() {
                    return true;
                }
                if !crate::update::installed() {
                    crate::update::open_page();
                    return true;
                }
                // Updating restarts blitz, which ends every session.
                let busy = (self.views.iter())
                    .filter(|v| v.pane.exit_code.is_none())
                    .filter(|v| matches!(v.pane.attn.state, Attn::Working | Attn::NeedsYou))
                    .count();
                let again = (self.update_confirm.take()).is_some_and(|t| Instant::now() < t);
                if busy > 0 && !again {
                    let until = Instant::now() + CONFIRM;
                    self.update_confirm = Some(until);
                    let what = if busy == 1 {
                        "A session is"
                    } else {
                        "Sessions are"
                    };
                    let text = format!(
                        "{what} busy, and updating restarts blitz. Press Ctrl+Shift+U again within 3 s"
                    );
                    self.set_notice(id, text, Some(until), false);
                    return true;
                }
                self.updating = Some(id);
                self.set_notice(id, format!("Downloading blitz {v}\u{2026}"), None, true);
                let proxy = self.proxy.clone();
                std::thread::spawn(move || {
                    let _ = proxy.send_event(UserEvent::Installed(crate::update::install(&v)));
                });
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
                lock(&v.pane.term).take_events(&mut events);
                for e in events {
                    self.on_term_event(id, e);
                }
                if !self.checked_conpty {
                    self.checked_conpty = true;
                    if let Some(text) = crate::pty::inbox_notice() {
                        self.counters.inbox = true;
                        eprintln!("blitz: {text}");
                        self.set_notice(id, text, Some(Instant::now() + NOTICE), true);
                    }
                }
                self.request_redraw();
            }
            Note::Exit(code) => {
                v.pane.exit_code = Some(code);
                self.attention(id, Ev::from_exit(code));
                // A clean exit or Ctrl+C closes the session; anything else
                // stays up so the output can be read.
                if matches!(code, 0 | 0xC000_013A) && self.args.selftest.is_none() {
                    self.close(el, id);
                    return;
                }
                self.set_notice(
                    id,
                    format!("exited with code {code} \u{b7} Enter close"),
                    None,
                    false,
                );
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

    fn on_term_event(&mut self, id: PaneId, e: Event) {
        let focus = self.focus_id() == Some(id);
        let Some(v) = self.view_mut(id) else {
            return;
        };
        match e {
            Event::Title(t) => {
                v.pane.title = t;
                if focus {
                    let t = v.pane.title.clone();
                    self.set_title(&t);
                }
            }
            Event::Cwd(dir) => {
                v.pane.cwd = dir;
                self.find_branch(id);
            }
            Event::Notify { title, body } => {
                if let Some(ev) = Ev::from_notify(&title, &v.pane.token) {
                    let changed = self.attention(id, ev);
                    if relabels(ev, changed)
                        && let Some(v) = self.view_mut(id)
                    {
                        v.pane.msg = body;
                    }
                }
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

    /// Feeds a session's attention state; flashes the taskbar button when
    /// it changes to something the user should see while looking away.
    /// Returns true when the state changed.
    fn attention(&mut self, id: PaneId, ev: Ev) -> bool {
        let attended = self.focused && self.focus_id() == Some(id);
        let away = !self.focused && self.config.flash;
        let Some(v) = self.view_mut(id) else {
            return false;
        };
        let now = Instant::now();
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
        let Some(v) = self.current() else {
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

    /// The part of the window the active tab's panes share: all of it
    /// but the sidebar or rail, which the chrome shows once there are two
    /// sessions, and the banner strip.
    fn tab_area(&self) -> Rect {
        let size = self
            .window
            .as_ref()
            .map_or(PhysicalSize::new(0, 0), |w| w.inner_size());
        let side = match (self.views.len() >= 2, self.win.sidebar_expanded) {
            (false, _) => 0.0,
            (true, true) => chrome::SIDEBAR_W,
            (true, false) => chrome::RAIL_W,
        };
        let side = (side * self.scale as f32).round() as i32;
        let banner = self.update.as_ref().map_or(0.0, |_| chrome::BANNER_H);
        let banner = (banner * self.scale as f32).round() as i32;
        Rect {
            x: side,
            y: 0,
            w: (size.width as i32 - side).max(0),
            h: (size.height as i32 - banner).max(0),
        }
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

    fn mouse_report(&mut self, kind: MouseKind, button: u8, m: &InputModes, mods: Mods) {
        let (col, row) = self.cell_at(self.mouse.pos);
        let ev = MouseEv {
            kind,
            button,
            col,
            row,
            mods,
        };
        let mut out = Vec::new();
        if self.mouse.tracker.encode(ev, m, &mut out) {
            self.send(out);
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
        let (x, y) = (self.mouse.pos.x as i32, self.mouse.pos.y as i32);
        let on_banner = (self.banner)
            .is_some_and(|r| (r.x..r.right()).contains(&x) && (r.y..r.bottom()).contains(&y));
        if pressed && b == 0 && on_banner {
            self.act(el, Action::Update);
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
        // A release goes wherever its press went.
        let reported = self.mouse.reported & 1 << b != 0;
        let to_program = self.mouse_to_program(&mods);
        if let Some(m) = to_program.filter(|_| pressed || reported) {
            let kind = if pressed {
                self.mouse.reported |= 1 << b;
                MouseKind::Press
            } else {
                self.mouse.reported &= !(1 << b);
                MouseKind::Release
            };
            self.mouse_report(kind, b, &m, mods);
            return;
        }
        self.mouse.reported &= !(1 << b);
        if b != 0 {
            return;
        }
        if pressed {
            self.mouse.anchor = Some(self.cell_at(self.mouse.pos));
            if self.selection.take().is_some() {
                self.request_redraw();
            }
        } else {
            self.mouse.anchor = None;
        }
    }

    fn on_mouse_move(&mut self, pos: PhysicalPosition<f64>) {
        self.mouse.pos = pos;
        if let Some(anchor) = self.mouse.anchor {
            let here = self.cell_at(pos);
            if self.selection.is_some() || here != anchor {
                self.selection = Some((anchor, here));
                self.request_redraw();
            }
            return;
        }
        let mods = mods_now();
        if let Some(m) = self.mouse_to_program(&mods) {
            let held = (0..3).find(|b| self.mouse.reported & 1 << b != 0);
            self.mouse_report(MouseKind::Move, held.unwrap_or(3), &m, mods);
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
        if let Some(m) = self.mouse_to_program(&mods) {
            let kind = if steps > 0.0 {
                MouseKind::WheelUp
            } else {
                MouseKind::WheelDown
            };
            for _ in 0..steps.abs() as u32 {
                self.mouse_report(kind, 0, &m, mods);
            }
        } else if !self.modes().alt_screen {
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
        if self.gfx.is_none() {
            return;
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
            if grid != v.grid {
                v.grid = grid;
                v.pane.resize(grid.0, grid.1);
                lock(&v.pane.term).set_cell_px(cw as u16, ch as u16);
            }
            v.rect = Some(rect);
            let sel = self.selection.filter(|_| Some(id) == focus);
            if !refresh(&mut lock(&v.pane.term), &mut v.snap, &self.pal, sel) {
                self.selection = None;
            }
            if Some(id) == focus {
                v.snap.selection = self.selection;
            } else if split {
                let mut s = v.snap.clone();
                s.selection = None;
                crate::render::dim(&mut s);
                dimmed.push((id, s));
            }
        }

        let pal = self.pal;
        let Some(g) = &mut self.gfx else {
            return;
        };
        let result = (|| {
            g.chain.resize(&g.r.gpu, size.width, size.height)?;
            g.chain.wait(100);
            for _ in 0..2 {
                g.r.begin();
                for v in &self.views {
                    let Some(at) = v.rect else {
                        continue;
                    };
                    let id = v.pane.id;
                    let snap = dimmed.iter().find(|d| d.0 == id).map_or(&v.snap, |d| &d.1);
                    g.r.snapshot(snap, &pal, at.x, at.y);
                    if let Some(n) = &v.notice {
                        draw_notice(&mut g.r, &pal, at, v.grid, n);
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
            g.chain.present()
        })();
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
                let lost = is_device_lost(&e);
                // The atlas is only a cache: build everything again.
                self.gfx = None;
                if lost {
                    self.request_redraw();
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

    /// The soonest time something on screen changes by itself.
    fn next_deadline(&self) -> Option<Instant> {
        let now = Instant::now();
        let sync = self
            .views
            .iter()
            .any(|v| lock(&v.pane.term).sync_pending(now))
            .then(|| now + vt::modes::SYNC_TIMEOUT);
        let notice = self
            .views
            .iter()
            .filter_map(|v| v.notice.as_ref()?.until)
            .min();
        // The sidebar counts how long each session has been working.
        let sidebar = self.views.len() >= 2 && self.win.sidebar_expanded;
        let timer = (self.views.iter())
            .filter(|v| sidebar && v.pane.attn.state == Attn::Working)
            .map(|v| {
                let since = v.pane.attn.since;
                since + Duration::from_secs(now.saturating_duration_since(since).as_secs() + 1)
            })
            .min();
        [sync, notice, timer].into_iter().flatten().min()
    }
}

/// Whether a notification replaces the session's sidebar message. It is
/// kept with the state it came with, so a repeat or an ignored event does
/// not relabel the session. Idle always does: the hook sends it with an
/// empty body when the session ends, which clears the last reply even
/// when the session was already idle.
fn relabels(ev: Ev, changed: bool) -> bool {
    changed || ev == Ev::Idle
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

/// Draws a notice over the bottom row of the pane whose grid is at `at`.
fn draw_notice(r: &mut Renderer, pal: &Palette, at: Rect, grid: (u16, u16), n: &Notice) {
    let (_, ch) = r.cell();
    let mut s = text_snapshot(&format!(" {}", n.text), grid.0, 1, pal);
    let bg = if n.dim { pal.bg } else { pal.selection_bg };
    for c in &mut s.cells {
        c.bg = bg;
        if n.dim {
            c.attrs |= vt::snapshot::attr::DIM;
        }
    }
    let banner = Palette { bg, ..*pal };
    let y = at.y + i32::from(grid.1.saturating_sub(1)) * ch as i32;
    r.snapshot(&s, &banner, at.x, y);
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

/// A new tab is named after the folder it starts in.
fn tab_name(cwd: Option<&Path>) -> String {
    let name = cwd.map(|p| match p.file_name() {
        Some(n) => n.to_string_lossy().into_owned(),
        // A drive root.
        None => p.display().to_string(),
    });
    name.filter(|n| !n.is_empty())
        .unwrap_or_else(|| "shell".into())
}

/// Takes a fresh snapshot of `term` into `snap`. Returns false when that
/// changed the text under `sel`: output that scrolls or rewrites selected
/// text ends the selection, so a copy never takes text the user did not
/// pick.
fn refresh(
    term: &mut vt::Terminal,
    snap: &mut Snapshot,
    pal: &Palette,
    sel: Option<((u16, u16), (u16, u16))>,
) -> bool {
    let before = sel.map(|s| selection_text(snap, s));
    !term.snapshot(snap, pal) || sel.map(|s| selection_text(snap, s)) == before
}

/// The text of the cells between two (column, row) points, inclusive, in
/// reading order: trailing blanks trimmed, rows joined by CRLF unless one
/// wraps into the next.
pub fn selection_text(snap: &Snapshot, sel: ((u16, u16), (u16, u16))) -> String {
    let (a, b) = sel;
    let (a, b) = if (a.1, a.0) <= (b.1, b.0) {
        (a, b)
    } else {
        (b, a)
    };
    let cols = usize::from(snap.cols);
    let last = b.1.min(snap.rows.saturating_sub(1));
    let mut out = String::new();
    for row in a.1..=last {
        let from = if row == a.1 { usize::from(a.0) } else { 0 };
        let to = if row == b.1 {
            usize::from(b.0).min(cols.saturating_sub(1))
        } else {
            cols.saturating_sub(1)
        };
        let mut line = String::new();
        let mut text_end = 0;
        for c in from..=to {
            let Some(cell) = snap.cells.get(usize::from(row) * cols + c) else {
                break;
            };
            match cell.len {
                // The right half of a wide character.
                0 if cell.width == 0 => {}
                // A blank, or hidden text: a space for each column.
                0 => line.extend(std::iter::repeat_n(' ', usize::from(cell.width))),
                n => {
                    let text = std::str::from_utf8(&cell.text[..usize::from(n)]).unwrap_or(" ");
                    push_drawn(&mut line, text, cell.width);
                    text_end = line.len();
                }
            }
        }
        // A row that wraps runs on into the next one: no line break, and
        // only the empty cells at its end are dropped.
        if row < last && snap.wrapped.get(usize::from(row)) == Some(&true) {
            line.truncate(text_end);
            out.push_str(&line);
        } else {
            out.push_str(line.trim_end());
            if row < last {
                out.push_str("\r\n");
            }
        }
    }
    out
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
            }
            // A synchronized update timed out, a notice expired, or a
            // working timer ticked.
            self.request_redraw();
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(_) => self.request_redraw(),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale = scale_factor;
                // The cursor's cell stays, but its pixels move.
                self.ime_at = None;
                let px = self.font_px();
                if let Some(g) = &mut self.gfx
                    && let Err(e) = g.r.set_font_px(px)
                {
                    eprintln!("blitz: font: {e}");
                }
                self.request_redraw();
            }
            WindowEvent::Focused(f) => {
                self.focused = f;
                let mut out = Vec::new();
                vt::encode_focus(f, &self.modes(), &mut out);
                self.send(out);
                if f && let Some(id) = self.focus_id() {
                    self.attention(id, Ev::Attended);
                }
            }
            WindowEvent::Ime(Ime::Commit(mut text)) => {
                self.preedit.clear();
                // Like typed characters, committed text carries no controls
                // that could run or escape anything.
                text.retain(|c| !c.is_control());
                self.typed(text.into_bytes());
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
            UserEvent::Update(v) => {
                let how = if crate::update::installed() {
                    "update and restart"
                } else {
                    "open the download page"
                };
                let text = format!("blitz {v} is available \u{b7} Ctrl+Shift+U to {how}");
                self.update = Some((v, text));
                self.request_redraw();
            }
            UserEvent::Installed(Ok(())) => el.exit(),
            UserEvent::Installed(Err(e)) => {
                eprintln!("blitz: update: {e}");
                if let Some(id) = self.updating.take() {
                    let text = format!("Update failed: {e}");
                    self.set_notice(id, text, Some(Instant::now() + NOTICE), false);
                }
            }
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.drain_keys(el);
        let flow = match self.next_deadline() {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        };
        el.set_control_flow(flow);
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
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

    #[test]
    fn app_selection_text_reads_cells_in_order() {
        let pal = crate::theme::dark();
        let s = text_snapshot("ab  \nc\u{4e2d}d\nxyz", 4, 3, &pal);
        // Given end first; spans three rows.
        assert_eq!(
            selection_text(&s, ((1, 2), (1, 0))),
            "b\r\nc\u{4e2d}d\r\nxy"
        );
        assert_eq!(selection_text(&s, ((0, 1), (3, 1))), "c\u{4e2d}d");
        assert_eq!(selection_text(&s, ((2, 0), (3, 0))), "");
    }

    /// A snapshot of a `cols` x `rows` terminal fed `bytes`.
    fn fed(cols: u16, rows: u16, bytes: &str) -> Snapshot {
        let mut t = vt::Terminal::new(vt::Options {
            cols,
            rows,
            ..vt::Options::default()
        });
        t.feed(bytes.as_bytes());
        let mut s = Snapshot::default();
        t.snapshot(&mut s, &crate::theme::dark());
        s
    }

    #[test]
    fn app_selection_leaves_out_hidden_text() {
        let s = fed(
            30,
            2,
            "git status\x1b[8m; iwr x|iex\x1b[28m!\r\n\x1b[38;2;19;20;23mcalc\x1b[0m",
        );
        assert_eq!(
            selection_text(&s, ((0, 0), (29, 1))),
            "git status           !\r\n"
        );
        let s = fed(4, 1, "\x1b[8m\u{4e2d}\x1b[0mx");
        assert_eq!(selection_text(&s, ((0, 0), (3, 0))), "  x");
    }

    #[test]
    fn app_selection_joins_wrapped_rows() {
        let two = ((0, 0), (3, 1));
        assert_eq!(selection_text(&fed(4, 3, "ab  cd"), two), "ab  cd");
        assert_eq!(selection_text(&fed(4, 3, "ab\r\ncd"), two), "ab\r\ncd");
        // A wide character that did not fit leaves an empty cell behind.
        assert_eq!(selection_text(&fed(3, 3, "ab\u{4e2d}"), two), "ab\u{4e2d}");
    }

    #[test]
    fn app_selection_copies_only_what_is_drawn() {
        let copy = |bytes: &str| selection_text(&fed(40, 1, bytes), ((0, 0), (39, 0)));
        assert_eq!(copy("ls\u{E0069}\u{E0067}\u{E006E}x"), "lsx", "tags");
        assert_eq!(copy("a\u{E0100}\u{FE00}b"), "ab", "variation selectors");
        assert_eq!(
            copy("a\u{200D}\u{301}\u{302}b"),
            "ab",
            "marks after a joiner"
        );
        assert_eq!(copy("a\u{200D}b"), "ab", "a lone joiner");
        assert_eq!(copy("x\u{3164}y\u{2800}z\u{A0}w"), "x  y z w", "fillers");
        assert_eq!(copy("x\u{AD}y"), "x y", "soft hyphen");
        for s in [
            "a\u{200C}\u{200C}\u{200C}b",
            "a\u{34F}b",
            "a\u{180B}\u{180F}b",
            "a\u{17B4}\u{17B5}b",
            "a\u{FE0E}\u{FE0F}b",
            "a\u{2060}\u{FEFF}b",
        ] {
            assert_eq!(copy(s), "ab", "{s:?}");
        }
        assert_eq!(copy("\u{2764}\u{FE0F}\u{FE0F}"), "\u{2764}\u{FE0F}");
        // Text that draws keeps everything.
        for s in [
            "e\u{301}",
            "\u{2764}\u{FE0F}",
            "1\u{FE0F}\u{20E3}",
            "\u{1F44D}\u{1F3FD}",
            "\u{1F468}\u{200D}\u{1F469}",
            "\u{4e2d}",
        ] {
            assert_eq!(copy(s), s);
        }
    }

    #[test]
    fn app_output_that_moves_selected_text_ends_the_selection() {
        let pal = crate::theme::dark();
        let mut t = vt::Terminal::new(vt::Options {
            cols: 10,
            rows: 3,
            ..vt::Options::default()
        });
        t.feed(b"a\r\nb\r\nc");
        let mut s = Snapshot::default();
        let sel = Some(((0, 1), (9, 1)));
        assert!(refresh(&mut t, &mut s, &pal, None));
        assert!(refresh(&mut t, &mut s, &pal, sel), "nothing new");
        t.feed(b"\x1b[1;5Hx");
        assert!(refresh(&mut t, &mut s, &pal, sel), "another row changed");
        t.feed(b"\x1b[3;1H\r\nd");
        assert!(!refresh(&mut t, &mut s, &pal, sel), "scrolled");
        t.feed(b"\x1b[2;1Hz");
        assert!(!refresh(&mut t, &mut s, &pal, sel), "rewritten");
    }

    #[test]
    fn app_new_panes_start_in_the_focused_directory() {
        let here = std::env::temp_dir();
        assert_eq!(start_dir(here.display().to_string()), Some(here));
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
        assert_eq!(start_dir(r"Z:\gone"), home);
        assert_eq!(start_dir(""), home);
        assert_eq!(tab_name(Some(Path::new(r"C:\dev\shop"))), "shop");
        assert_eq!(tab_name(Some(Path::new(r"C:\"))), r"C:\");
        assert_eq!(tab_name(None), "shell");
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
    fn app_message_follows_the_state_and_idle_clears_it() {
        let t0 = Instant::now();
        let mut a = crate::attention::PaneAttn::new(t0);
        let mut feed = |ev, attended| relabels(ev, a.apply(ev, attended, t0));
        assert!(feed(Ev::Working, true));
        assert!(!feed(Ev::Working, true), "a repeat");
        assert!(!feed(Ev::NeedsYou, true), "ignored while looking");
        // Watched to the end: done is seen at once and lands on idle, and
        // the end of the session still clears the message.
        assert!(feed(Ev::Done, true));
        assert!(feed(Ev::Idle, true));
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
        assert_eq!(parse(&["--cwd", r"C:oo"]).cwd, Some(r"C:oo".into()));
        let a = parse(&["--new-window", "--cwd", r"C:oo"]);
        assert!(a.new_window);
        assert_eq!(a.cwd, Some(r"C:oo".into()));
        let a = parse(&["--cwd", r"C:oo", "--new-window"]);
        assert!(a.new_window);
        assert_eq!(a.cwd, Some(r"C:oo".into()));
    }
}
