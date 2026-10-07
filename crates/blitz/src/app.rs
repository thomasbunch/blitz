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

use vt::{
    Event, InputModes, KeyInput, Mods, MouseEv, MouseKind, MouseMode, Palette, PromptMark, Snapshot,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DwmSetWindowAttribute,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, GetKeyboardState};
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
use winit::window::{CursorIcon, Icon, UserAttentionType, Window, WindowId};

use crate::arcade::run::{self, Run};
use crate::attention::{Attn, Ev};
use crate::config::{Config, Kind};
use crate::debug::Counters;
use crate::keymap::{self, Action};
use crate::layout::{self, Axis, Dir, PaneId, Rect, Tab};
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
const VK_F4: u16 = 0x73;

/// How long a multi-line paste waits for a second Ctrl+V, and closing a
/// busy session for a second Ctrl+Shift+W.
const CONFIRM: Duration = Duration::from_secs(3);
/// How long the notice about the system ConPTY stays up.
const NOTICE: Duration = Duration::from_secs(5);
/// Frame times for blitz run, and for scenery and the spark, which move
/// slowly.
const GAME_FRAME: Duration = Duration::from_millis(16);
const SCENERY_FRAME: Duration = Duration::from_millis(66);
/// The first look for a newer release waits until startup is done, then
/// one runs every few hours.
const UPDATE_FIRST: Duration = Duration::from_secs(10);
const UPDATE_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// Taskbar flashes per session are at least this far apart.
const FLASH_GAP: Duration = Duration::from_secs(10);
/// How long a restored pane waits for its shell's first prompt before it
/// types the Claude Code resume command anyway.
const RESUME_AFTER: Duration = Duration::from_secs(3);
/// Lines of output saved per pane when `restore_scrollback` is on.
const SAVED_LINES: usize = 1000;
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
    /// A left-button drag is moving this divider of the active tab; the
    /// second value is the smallest pane it may leave.
    divider: Option<(usize, (i32, i32))>,
    /// The pointer is over a divider along this axis and shows it.
    over_divider: Option<Axis>,
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
    /// When the program's open synchronized update times out, as of the
    /// last look at its terminal.
    sync_until: Option<Instant>,
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
    /// Windows shows animations; scenery and the spark keep still if not.
    motion: bool,
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
    /// The installer is downloading, or Ctrl+Shift+U is looking for a
    /// release; this pane shows that.
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
    /// This is the main window, whose layout is saved for the next start.
    /// Separate windows and scripted runs leave the saved one alone.
    persist: bool,
    /// The session as last saved.
    saved: Option<session::State>,
    /// Where the window last was while neither minimized nor maximized.
    placed: Geometry,
    /// The terminal a running self-test reads: the focused pane's.
    watched: Option<Arc<selftest::Focus>>,
    started: Instant,
    /// The device started at launch, until the first renderer takes it.
    gpu: Option<std::thread::JoinHandle<windows::core::Result<Gpu>>>,
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
        let mut theme = crate::theme::current(&config.theme);
        if let Some(a) = config.accent {
            theme.ui.set_accent(a);
        }
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
            motion: animations_on(),
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
            persist,
            saved: None,
            placed: Geometry::default(),
            watched: None,
            started: Instant::now(),
            gpu: None,
            counters: Counters::default(),
            code: 0,
        }
    }

    fn font_px(&self) -> f32 {
        self.config.font_size * 96.0 / 72.0 * self.scale as f32
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
        self.frame_theme();
        self.window = Some(window);
        self.ensure_gfx();

        let mut win = layout::Window::default();
        if let Some(s) = &saved {
            match self.restore(s) {
                Ok(()) => win = self.win.clone(),
                Err(e) => eprintln!("blitz: restoring the last session: {e}"),
            }
        }
        // A folder from Explorer gets a tab of its own after the restored ones.
        if self.views.is_empty() || self.args.cwd.is_some() {
            let cwd = match &self.args.cwd {
                Some(dir) => start_dir(dir),
                None => std::env::current_dir().ok(),
            };
            let id = PaneId(self.next_id);
            win.tabs.push(Tab::new(tab_name(cwd.as_deref()), id));
            win.active = win.tabs.len() - 1;
            let cmd = self.args.cmd.clone();
            self.open(win, id, cmd.as_deref(), cwd)?;
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
        if self.config.check_updates && !scripted && !cfg!(debug_assertions) {
            let proxy = self.proxy.clone();
            std::thread::spawn(move || {
                std::thread::sleep(UPDATE_FIRST);
                if let Some((v, log)) = crate::update::failed() {
                    let _ = proxy.send_event(UserEvent::Update(v, Some(log)));
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
        self.spawn(id, &grids, cmd, cwd, None)?;
        self.install(win);
        Ok(())
    }

    /// Starts every pane of a saved session, each in its folder, and shows
    /// its layout. Starts none if one fails.
    fn restore(&mut self, s: &session::State) -> Result<(), String> {
        let (win, panes) = s.layout(self.next_id);
        let grids = self.grids(&win);
        let keys = leaf_keys(&win);
        for (id, meta) in panes {
            let old = (self.config.restore_scrollback)
                .then(|| keys.iter().find(|k| k.0 == id))
                .flatten()
                .and_then(|&(_, tab, leaf)| session::load_output(tab, leaf));
            if let Err(e) = self.spawn(id, &grids, None, start_dir(&meta.cwd), old.as_deref()) {
                self.views.clear();
                return Err(e);
            }
            if let Some(line) = resume_line(self.config.restore_claude, meta.claude.as_deref())
                && let Some(v) = self.views.last_mut()
            {
                // Known from the start, so closing blitz again before the
                // first prompt still resumes it next time.
                v.pane.claude = meta.claude.clone();
                v.resume = Some((line, Instant::now() + RESUME_AFTER));
            }
        }
        self.install(win);
        Ok(())
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
            sync_until: None,
        });
        self.find_branch(id);
        self.next_id = id.0 + 1;
        Ok(())
    }

    /// Shows `win`, a layout whose panes all have sessions.
    fn install(&mut self, win: layout::Window) {
        let before = self.focus_id();
        self.win = win;
        self.focus_moved(before);
    }

    /// Opens a pane in a copy of the layout that `place` changes; tells the
    /// user in the focused pane when that fails. The pane starts in `dir`,
    /// or else where the focused one is.
    fn add(
        &mut self,
        dir: Option<PathBuf>,
        place: impl FnOnce(&mut layout::Window, PaneId, Option<&Path>) -> bool,
    ) {
        let cwd = dir.or_else(|| start_dir(self.current().map_or("", |v| v.pane.cwd.as_str())));
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
        // The divider being dragged may be gone.
        self.mouse.divider = None;
        // Dropping the pane closes its pseudoconsole.
        self.views.retain(|v| v.pane.id != id);
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
            spark: self.config.mascot.then(|| self.anim_time()),
            game: self.game.as_ref().map(|g| &g.0),
        }
    }

    /// Seconds into the scenery and spark animations; always 0 when
    /// Windows animations are off.
    fn anim_time(&self) -> f32 {
        if self.motion {
            self.started.elapsed().as_secs_f32()
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
    fn apply_theme(&mut self, mut t: Theme) {
        if let Some(a) = self.config.accent {
            t.ui.set_accent(a);
        }
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
        let n = pk.matches().len();
        let m = &k.mods;
        // Ctrl and Alt together are AltGr when the layout gives a character.
        let (ctrl, alt) = (m.lctrl || m.rctrl, m.lalt || m.ralt);
        let chord = ctrl != alt || (ctrl && k.uc == 0);
        let step = |by: isize| {
            let last = n.saturating_sub(1) as isize;
            (pk.sel as isize + by).clamp(0, last) as usize
        };
        match k.vk {
            VK_ESCAPE => {
                self.picker = None;
                self.set_theme_from_config();
                return;
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
                                (None, Some(id)) => {
                                    self.set_notice(id, text, Some(Instant::now() + NOTICE), false);
                                }
                                (None, None) => {}
                            }
                        }
                        self.config.theme = setting;
                        self.apply_theme(t);
                    }
                    None => self.set_theme_from_config(),
                }
                return;
            }
            VK_UP => pk.sel = step(-1),
            VK_DOWN => pk.sel = step(1),
            VK_PRIOR => pk.sel = step(-(chrome::PICKER_ROWS as isize)),
            VK_NEXT => pk.sel = step(chrome::PICKER_ROWS as isize),
            VK_BACK if pk.filter.pop().is_some() => pk.sel = 0,
            VK_BACK => return,
            _ if !chord && !k.text.is_empty() => {
                pk.filter.push_str(k.text);
                pk.sel = 0;
            }
            _ => return,
        }
        self.preview();
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

    /// Loads the configured font at the size the window's DPI needs, and
    /// tells each terminal its new cell size.
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

    /// Typed text for the filter of the theme picker or the settings
    /// panel. False when neither is open.
    fn filter_text(&mut self, t: &str) -> bool {
        if self.game.is_some() {
            // The game takes keys, not text.
        } else if let Some(p) = &mut self.picker {
            p.filter.push_str(t);
            p.sel = 0;
            self.preview();
        } else if let Some(p) = &mut self.settings {
            p.filter.push_str(t);
            p.sel = 0;
            self.request_redraw();
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

    /// Shows the banner for release `v`, or for the update to it that
    /// failed and wrote `log`.
    fn offer_update(&mut self, v: String, log: Option<PathBuf>) {
        // A later look finding the same release keeps a failure in view.
        if log.is_none() && self.update.as_ref().is_some_and(|u| u.0 == v) {
            return;
        }
        let text = match log {
            Some(log) => format!(
                "Updating to blitz {v} failed, see {} \u{b7} Ctrl+Shift+U to try again",
                log.display()
            ),
            None => {
                let how = if crate::update::installed() {
                    "update and restart"
                } else {
                    "open the download page"
                };
                format!("blitz {v} is available \u{b7} Ctrl+Shift+U to {how}")
            }
        };
        self.update = Some((v, text));
        self.request_redraw();
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
                Input::Text(t) => {
                    if !self.filter_text(&t) {
                        self.typed(t.into_bytes());
                    }
                }
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
        if let Some((g, _)) = &mut self.game {
            // Every key is the game's, but a shortcut closes it and runs.
            if keymap::action(k).is_some() {
                self.close_game();
            } else {
                if k.down {
                    self.eaten = Some(k.vk);
                    match k.vk {
                        VK_ESCAPE => self.close_game(),
                        VK_SPACE | VK_UP | VK_W => g.jump(),
                        _ => {}
                    }
                    self.request_redraw();
                }
                return;
            }
        }
        if self.picker.is_some() {
            // Every key is the picker's; so is the release of the last.
            if k.down {
                self.eaten = Some(k.vk);
                if keymap::action(k) == Some(Action::ThemePicker) {
                    self.picker = None;
                    self.set_theme_from_config();
                } else {
                    self.picker_key(k);
                }
            }
            return;
        }
        if self.settings.is_some() {
            // Every key is the panel's too, but the theme picker opens over it.
            if k.down {
                self.eaten = Some(k.vk);
                match keymap::action(k) {
                    Some(Action::Settings) => {
                        self.settings = None;
                        self.request_redraw();
                    }
                    Some(Action::ThemePicker) => self.open_picker(),
                    _ => self.settings_key(k),
                }
            }
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
            Action::NewTab => self.add(None, new_tab),
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
                self.add(None, |win, id, _| {
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
            Action::ThemePicker => self.open_picker(),
            Action::Settings => self.open_settings(),
            Action::Focus(dir) => {
                let (area, active) = (self.tab_area(), self.win.active);
                if let Some(t) = self.win.tabs.get_mut(active) {
                    t.focus_dir(dir, area);
                }
                self.focus_moved(before);
            }
            // The focused session is skipped: the user is already looking
            // at it, and a session that exited stays red until closed.
            Action::Resize(dir) | Action::Swap(dir) => {
                let (area, min, (cw, ch)) = (self.tab_area(), self.min_pane(), self.cell());
                let active = self.win.active;
                let Some(t) = (self.win.tabs.get_mut(active)).filter(|t| t.panes().len() >= 2)
                else {
                    return false;
                };
                match (a, dir) {
                    (Action::Swap(_), _) => t.swap(dir, area),
                    (_, Dir::Left | Dir::Right) => t.resize(dir, cw as i32, area, min),
                    _ => t.resize(dir, ch as i32, area, min),
                };
                self.request_redraw();
            }
            Action::JumpToAttention => {
                let waiting = (self.views.iter())
                    .filter(|v| Some(v.pane.id) != before)
                    .map(|v| (v.pane.id, v.pane.attn.state, v.pane.attn.since));
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
                        let _ = proxy.send_event(UserEvent::Checked(crate::update::check()));
                    });
                    return true;
                };
                if !crate::update::installed() {
                    if !crate::update::open_page() {
                        let text = format!(
                            "Could not open a browser; get it at {}",
                            crate::update::PAGE
                        );
                        self.set_notice(id, text, Some(Instant::now() + NOTICE), false);
                    }
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
            // The shell is ready for input: bring back its Claude session.
            Event::Prompt(PromptMark::A { blitz: true }) => {
                if let Some((line, _)) = v.resume.take() {
                    v.pane.send(line);
                }
            }
            Event::Notify { title, body } => {
                if let Some((ev, session)) = Ev::from_notify(&title, &v.pane.token) {
                    // `idle` is SessionEnd: the user quit Claude, so there is
                    // nothing left to resume.
                    if ev == Ev::Idle {
                        v.pane.claude = None;
                    } else if let Some(id) = session {
                        v.pane.claude = Some(id.to_owned());
                    }
                    let changed = self.attention(id, ev);
                    if relabels(ev, changed)
                        && let Some(v) = self.view_mut(id)
                    {
                        v.pane.msg = body;
                    }
                }
            }
            // Like a question from Claude Code: it needs the user, unless
            // they are already looking at the pane.
            Event::Bell if self.config.bell_attention => {
                self.attention(id, Ev::NeedsYou);
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
        let needs_you = changed && v.pane.attn.state == Attn::NeedsYou;
        let kind = (changed && away)
            .then(|| flash_kind(v.pane.attn.state, &mut v.flashed, now))
            .flatten();
        // Back to work: the game ends when a session needs the user.
        if needs_you {
            self.close_game();
        }
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

    /// The divider of the active tab under a point, if the settings panel
    /// is not over it. Dividers are 1 px wide, so a few px either side count.
    fn divider_at(&self, pos: PhysicalPosition<f64>) -> Option<(usize, Axis)> {
        let t = (self.win.tabs.get(self.win.active)).filter(|_| self.settings.is_none())?;
        let slop = (3.0 * self.scale).round() as i32;
        t.divider_at(self.tab_area(), pos.x as i32, pos.y as i32, slop)
    }

    /// The smallest pane, frame included, that still holds `MIN_COLS` by
    /// `MIN_ROWS` cells as the active tab is drawn now.
    fn min_pane(&self) -> (i32, i32) {
        let (cw, ch) = self.cell();
        let chrome = chrome::build(&self.model(&self.win, &[], None));
        let outer = (self.win.tabs.get(self.win.active)).map(|t| t.rects(self.tab_area()));
        let frame = (outer.iter().flatten().zip(&chrome.panes).next())
            .map_or((0, 0), |(o, i)| (o.1.w - i.1.w, o.1.h - i.1.h));
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
        // Presses go to the settings panel; a release still goes wherever
        // its press went.
        if pressed && self.settings.is_some() && self.picker.is_none() {
            if b == 0 {
                self.settings_click();
            }
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
        if let Some((i, min)) = self.mouse.divider {
            let (area, active) = (self.tab_area(), self.win.active);
            if let Some(t) = self.win.tabs.get_mut(active)
                && t.drag(i, pos.x as i32, pos.y as i32, area, min)
            {
                self.request_redraw();
            }
            return;
        }
        if let Some(anchor) = self.mouse.anchor {
            let here = self.cell_at(pos);
            let sel = Some((anchor, here));
            if (self.selection.is_some() || here != anchor) && self.selection != sel {
                self.selection = sel;
                self.request_redraw();
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
        if let Some(p) = self.settings.as_mut().filter(|_| self.picker.is_none()) {
            p.move_by(-steps as isize);
            self.request_redraw();
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
        let Some(g) = &self.gfx else {
            return;
        };
        // Wait for the swap chain before reading the panes, so the frame
        // shows output that arrived during the wait.
        g.chain.wait(100);
        let started = Instant::now();
        let mut waited = Duration::ZERO;
        if let Some((g, at)) = &mut self.game {
            // A long gap, as while the window was in the background,
            // counts as one short step: the game waits rather than jumps.
            let dt = started
                .saturating_duration_since(*at)
                .min(Duration::from_millis(50));
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
            if grid != v.grid {
                v.grid = grid;
                v.pane.resize(grid.0, grid.1);
                lock(&v.pane.term).set_cell_px(cw as u16, ch as u16);
            }
            v.rect = Some(rect);
            let sel = self.selection.filter(|_| Some(id) == focus);
            let mut term = lock(&v.pane.term);
            v.sync_until = term.sync_deadline();
            if !refresh(&mut term, &mut v.snap, &self.theme.pal, sel) {
                self.selection = None;
            }
            if Some(id) == focus {
                v.snap.selection = self.selection;
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
                    let Some(at) = v.rect else {
                        continue;
                    };
                    let dim = dimmed.contains(&v.pane.id);
                    g.r.grid(&v.snap, &pal, at.x, at.y, dim, scenery.is_none());
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
            }
        };
        let mut s = session::State::capture(&self.win, self.placed, meta);
        // Output changes all the time, so it is saved only at exit.
        if force {
            self.save_output();
        }
        let same = self.saved.as_ref().is_some_and(|old| {
            (old.sidebar_expanded, old.active, &old.tabs) == (s.sidebar_expanded, s.active, &s.tabs)
        });
        if same && !force {
            return;
        }
        if let Some(w) = &self.window {
            let maximized = w.is_maximized();
            if !maximized
                && w.is_minimized() != Some(true)
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
            s.window = Geometry {
                maximized,
                ..self.placed
            };
        }
        if let Err(e) = session::save(&s) {
            eprintln!("blitz: saving the session: {e}");
        }
        // Kept even when the write failed, so it is not retried every turn.
        self.saved = Some(s);
    }

    /// Saves each pane's recent output when `restore_scrollback` is on, and
    /// deletes what an earlier run saved when it is off.
    fn save_output(&self) {
        let stamp = local_stamp();
        let keys = if self.config.restore_scrollback {
            leaf_keys(&self.win)
        } else {
            Vec::new()
        };
        let panes: Vec<_> = (keys.into_iter())
            .filter_map(|(id, tab, leaf)| {
                let term = lock(&self.view(id)?.pane.term);
                let mut text = term.scrollback_text();
                // A full-screen program's screen is not output.
                if !term.input_modes().alt_screen {
                    text.push('\n');
                    text += &term.screen_text();
                }
                let text = last_lines(&text, SAVED_LINES);
                (!text.is_empty()).then(|| (tab, leaf, format!("{stamp}\n{text}")))
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
        // The sidebar counts how long each session has been working.
        let sidebar = self.views.len() >= 2 && self.win.sidebar_expanded;
        let timer = (self.views.iter())
            .filter(|v| sidebar && v.pane.attn.state == Attn::Working)
            .map(|v| {
                let since = v.pane.attn.since;
                since + Duration::from_secs(now.saturating_duration_since(since).as_secs() + 1)
            })
            .min();
        let resume = (self.views.iter())
            .filter_map(|v| Some(v.resume.as_ref()?.1))
            .min();
        // Only a window in use animates.
        let still =
            !self.motion || (self.config.scenery == "off" && !(self.config.mascot && sidebar));
        let anim = match (self.focused, self.game.is_some(), still) {
            (true, true, _) => Some(now + GAME_FRAME),
            (true, false, false) => Some(now + SCENERY_FRAME),
            _ => None,
        };
        [sync, notice, timer, resume, anim]
            .into_iter()
            .flatten()
            .min()
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

/// What to type into a restored pane's shell to bring back the Claude Code
/// session it was running, if anything. The id comes from a file on disk,
/// so only a well-formed one is ever typed.
fn resume_line(enabled: bool, claude: Option<&str>) -> Option<String> {
    let id = claude.filter(|id| enabled && crate::hook::is_session_id(id))?;
    Some(format!("claude --resume {id}\r"))
}

/// Every pane of `win` with its tab and leaf index, which its saved output
/// is filed under.
fn leaf_keys(win: &layout::Window) -> Vec<(PaneId, usize, usize)> {
    (win.tabs.iter().enumerate())
        .flat_map(|(t, tab)| (tab.panes().into_iter().enumerate()).map(move |(l, id)| (id, t, l)))
        .collect()
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

/// Puts pane `id` in a new tab after the others and shows that tab.
fn new_tab(win: &mut layout::Window, id: PaneId, cwd: Option<&Path>) -> bool {
    win.tabs.push(Tab::new(tab_name(cwd), id));
    win.active = win.tabs.len() - 1;
    true
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
    // Only a terminal with news can change the selected text.
    let before = sel
        .filter(|_| term.is_changed())
        .map(|s| selection_text(snap, s));
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
                // No prompt mark came: shell integration is off or failed.
                if let Some((line, _)) = v.resume.take_if(|r| r.1 <= now) {
                    v.pane.send(line);
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
                if !self.filter_text(&text) {
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
                let text = match found {
                    Ok(Some(v)) => {
                        let text = format!("blitz {v} is available");
                        self.offer_update(v, None);
                        text
                    }
                    Ok(None) => format!("blitz {} is up to date", env!("CARGO_PKG_VERSION")),
                    Err(e) => format!("Could not look for an update: {e}"),
                };
                if let Some(id) = self.updating.take() {
                    self.set_notice(id, text, Some(Instant::now() + NOTICE), false);
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
                    let text = format!("Update failed: {e}");
                    self.set_notice(id, text, Some(Instant::now() + NOTICE), false);
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
        assert_eq!(parse(&["--cwd", r"C:\foo"]).cwd, Some(r"C:\foo".into()));
        let a = parse(&["--new-window", "--cwd", r"C:\foo"]);
        assert!(a.new_window);
        assert_eq!(a.cwd, Some(r"C:\foo".into()));
        let a = parse(&["--cwd", r"C:\foo", "--new-window"]);
        assert!(a.new_window);
        assert_eq!(a.cwd, Some(r"C:\foo".into()));
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

    #[test]
    fn saved_output_keeps_the_last_lines() {
        assert_eq!(last_lines("\n\na\nb\nc\n\n\n", 2), "b\nc");
        assert_eq!(last_lines("a\n\nb", 10), "a\n\nb");
        assert_eq!(last_lines("  a\n", 10), "  a");
        assert_eq!(last_lines("\n\n", 10), "");
    }

    #[test]
    fn leaf_keys_follow_tabs_and_tree_order() {
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
            leaf_keys(&win),
            [(PaneId(1), 0, 0), (PaneId(2), 0, 1), (PaneId(3), 1, 0)]
        );
    }
}
