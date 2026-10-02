//! The window, its event loop and the frame loop.

// One process hosts every session, so a failed HRESULT must never panic.
#![deny(clippy::unwrap_used)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::Ordering;
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
use crate::config::Config;
use crate::debug::Counters;
use crate::keymap::{self, Action};
use crate::layout::PaneId;
use crate::pane::{Note, Pane, Spawn, lock};
use crate::render::d3d11::{Swapchain, is_device_lost};
use crate::render::{Renderer, text_snapshot, write_bmp};

const VK_PROCESSKEY: u16 = 0xe5;
const VK_PACKET: u16 = 0xe7;
const VK_RETURN: u16 = 0x0d;
const VK_F4: u16 = 0x73;

/// How long a multi-line paste waits for a second Ctrl+V.
const PASTE_CONFIRM: Duration = Duration::from_secs(3);
/// How long the notice about the system ConPTY stays up.
const NOTICE: Duration = Duration::from_secs(5);
/// Taskbar flashes per session are at least this far apart.
const FLASH_GAP: Duration = Duration::from_secs(10);
/// Lines scrolled per wheel notch when the program takes no mouse input.
const WHEEL_LINES: isize = 3;

#[derive(Debug)]
pub enum UserEvent {
    Pane(PaneId, Note),
    /// Exit with this code: the self-test finished, or `--exit-after`
    /// ran out.
    Finish(i32),
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
}

impl Args {
    fn parse(args: &[String]) -> Result<Args, String> {
        let mut a = Args::default();
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            let v = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
            match flag.as_str() {
                "--cmd" => a.cmd = Some(v.clone()),
                "--cwd" => a.cwd = Some(v.into()),
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
            if vk == VK_PACKET || k.chars && down {
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

/// A one-row message drawn over the bottom row of the pane.
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
    pane: Option<Pane>,
    snap: Snapshot,
    /// The pane's size in cells.
    grid: (u16, u16),
    focused: bool,
    selection: Option<((u16, u16), (u16, u16))>,
    mouse: Mouse,
    /// IME composition text and its cursor.
    preedit: String,
    notice: Option<Notice>,
    /// A multi-line paste waiting for its confirming Ctrl+V.
    paste: Option<(String, Instant)>,
    /// The release of this key belongs to a shortcut and is not sent.
    eaten: Option<u16>,
    ime_cell: Option<(u16, u16)>,
    last_flash: Option<Instant>,
    /// Checked once the first output shows which ConPTY is running.
    checked_conpty: bool,
    capture_then_exit: bool,
    started: Instant,
    counters: Counters,
    code: i32,
}

impl App {
    fn new(args: Args, keys: Rc<RefCell<Keys>>, proxy: EventLoopProxy<UserEvent>) -> App {
        let dark = !crate::theme::system_is_light();
        App {
            args,
            config: Config::default(),
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
            pane: None,
            snap: Snapshot::default(),
            grid: (0, 0),
            focused: false,
            selection: None,
            mouse: Mouse::default(),
            preedit: String::new(),
            notice: None,
            paste: None,
            eaten: None,
            ime_cell: None,
            last_flash: None,
            checked_conpty: false,
            capture_then_exit: false,
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
        let size = window.inner_size();
        self.window = Some(window);
        self.ensure_gfx();
        let (cw, ch) = self.cell();
        let (cols, rows) = grid_size(size, (cw, ch), self.pad());

        let launch = match &self.args.cmd {
            Some(c) => crate::shell::Launch {
                cmdline: c.clone(),
                env: Vec::new(),
            },
            None => crate::shell::launch(
                &self.config.shell,
                &self.config.shell_args,
                self.config.shell_integration,
            ),
        };
        let proxy = self.proxy.clone();
        let pane = Pane::spawn(
            PaneId(1),
            &Spawn {
                cmdline: &launch.cmdline,
                env: &launch.env,
                cwd: self.args.cwd.as_deref(),
                cols,
                rows,
                scrollback: self.config.scrollback_lines,
                dark: self.dark,
                parent: Some(self.hwnd),
            },
            move |id, note| {
                let _ = proxy.send_event(UserEvent::Pane(id, note));
            },
        )
        .map_err(|e| format!("cannot start {}: {e}", launch.cmdline))?;
        lock(&pane.term).set_cell_px(cw as u16, ch as u16);
        self.grid = (cols, rows);
        self.pane = Some(pane);

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
        Ok(())
    }

    /// Runs the `--selftest` script on its own thread; the app exits with
    /// its result.
    fn start_selftest(&self, path: PathBuf) {
        let (Some(pane), proxy, hwnd) = (&self.pane, self.proxy.clone(), self.hwnd) else {
            return;
        };
        let term = pane.term.clone();
        std::thread::spawn(move || {
            let code = match std::fs::read_to_string(&path) {
                Ok(script) => match selftest::run(&script, hwnd, &term) {
                    Ok(()) => 0,
                    Err(e) => {
                        let screen = lock(&term).screen_text();
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
        self.pane
            .as_ref()
            .map(|p| lock(&p.term).input_modes())
            .unwrap_or_default()
    }

    fn send(&self, bytes: impl Into<Vec<u8>>) {
        if let Some(p) = &self.pane {
            p.send(bytes);
        }
    }

    /// Scrolls the main screen; positive is up into the scrollback.
    fn scroll(&mut self, lines: isize) {
        if let Some(p) = &self.pane {
            lock(&p.term).scroll_viewport(lines);
            self.selection = None;
            self.request_redraw();
        }
    }

    fn set_notice(&mut self, text: impl Into<String>, until: Option<Instant>, dim: bool) {
        self.notice = Some(Notice {
            text: text.into(),
            until,
            dim,
        });
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
        let Some(pane) = &self.pane else {
            return;
        };
        if pane.exit_code.is_some() {
            if k.down && k.vk == VK_RETURN {
                el.exit();
            }
            return;
        }
        if !k.down && self.eaten == Some(k.vk) {
            self.eaten = None;
            return;
        }
        if let Some(a) = keymap::action(k)
            && self.act(a)
        {
            self.eaten = Some(k.vk);
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
        if let Some(p) = &self.pane {
            lock(&p.term).scroll_viewport(isize::MIN);
            p.send(bytes);
        }
    }

    /// Runs a shortcut. Returns false when it does not apply right now, in
    /// which case the key goes to the program.
    fn act(&mut self, a: Action) -> bool {
        match a {
            Action::Copy => {
                let Some(sel) = self.selection.take() else {
                    return false;
                };
                let text = selection_text(&self.snap, sel);
                if !crate::clipboard::set_text(Some(HWND(self.hwnd as *mut c_void)), &text) {
                    eprintln!("blitz: could not copy to the clipboard");
                }
                self.request_redraw();
                true
            }
            Action::Paste => {
                let Some(text) = crate::clipboard::get_text().filter(|t| !t.is_empty()) else {
                    return false;
                };
                let bracketed = self.modes().bracketed;
                if vt::keys::needs_paste_confirm(&text, bracketed) {
                    let confirmed = self
                        .paste
                        .take()
                        .is_some_and(|(t, until)| t == text && Instant::now() < until);
                    if !confirmed {
                        let lines = text.lines().count();
                        let until = Instant::now() + PASTE_CONFIRM;
                        self.paste = Some((text, until));
                        self.set_notice(
                            format!("Paste {lines} lines? Press Ctrl+V again within 3 s"),
                            Some(until),
                            false,
                        );
                        return true;
                    }
                    self.notice = None;
                }
                let mut out = Vec::new();
                vt::encode_paste(&text, bracketed, &mut out);
                self.typed(out);
                true
            }
            Action::ScrollPage(dir) => {
                if self.modes().alt_screen {
                    return false;
                }
                let page = self.grid.1.saturating_sub(1).max(1) as isize;
                self.scroll(page * isize::from(dir));
                true
            }
            // Tabs, splits and the sidebar need more than one session.
            _ => false,
        }
    }

    fn on_pane(&mut self, el: &ActiveEventLoop, note: Note) {
        let Some(pane) = &mut self.pane else {
            return;
        };
        match note {
            Note::Dirty => {
                pane.dirty.store(false, Ordering::Release);
                let mut events = Vec::new();
                lock(&pane.term).take_events(&mut events);
                for e in events {
                    self.on_term_event(e);
                }
                if !self.checked_conpty {
                    self.checked_conpty = true;
                    if let Some(text) = crate::pty::inbox_notice() {
                        self.counters.inbox = true;
                        eprintln!("blitz: {text}");
                        self.set_notice(text, Some(Instant::now() + NOTICE), true);
                    }
                }
                self.request_redraw();
            }
            Note::Exit(code) => {
                pane.exit_code = Some(code);
                self.attention(Ev::from_exit(code));
                // A clean exit or Ctrl+C closes the session; anything else
                // stays up so the output can be read.
                if matches!(code, 0 | 0xC000_013A) && self.args.selftest.is_none() {
                    el.exit();
                    return;
                }
                self.set_notice(
                    format!("exited with code {code} \u{b7} Enter close"),
                    None,
                    false,
                );
            }
            Note::Dead => {
                self.attention(Ev::Error { sticky: true });
                self.set_notice(
                    "this session stopped updating after an internal error",
                    None,
                    false,
                );
            }
        }
    }

    fn on_term_event(&mut self, e: Event) {
        match e {
            Event::Title(t) => {
                if let Some(w) = &self.window {
                    w.set_title(if t.is_empty() { "blitz" } else { &t });
                }
                if let Some(p) = &mut self.pane {
                    p.title = t;
                }
            }
            Event::Cwd(dir) => {
                if let Some(p) = &mut self.pane {
                    p.cwd = dir;
                }
            }
            Event::Notify { title, body } => {
                if let Some(ev) = Ev::from_notify(&title) {
                    if let Some(p) = &mut self.pane {
                        p.msg = body;
                    }
                    self.attention(ev);
                }
            }
            _ => {}
        }
    }

    /// Feeds the session's attention state; flashes the taskbar button when
    /// it changes to something the user should see while looking away.
    fn attention(&mut self, ev: Ev) {
        let attended = self.focused;
        let Some(p) = &mut self.pane else {
            return;
        };
        if !p.attn.apply(ev, attended, Instant::now()) || attended || !self.config.flash {
            return;
        }
        let kind = match p.attn.state {
            Attn::NeedsYou | Attn::Error => UserAttentionType::Critical,
            Attn::DoneUnseen => UserAttentionType::Informational,
            _ => return,
        };
        if self.last_flash.is_some_and(|t| t.elapsed() < FLASH_GAP) {
            return;
        }
        self.last_flash = Some(Instant::now());
        if let Some(w) = &self.window {
            w.request_user_attention(Some(kind));
        }
    }

    fn cell_at(&self, pos: PhysicalPosition<f64>) -> (u16, u16) {
        let (cw, ch) = self.cell();
        let (px, py) = self.pad();
        let x = (pos.x - f64::from(px)).max(0.0) as u32;
        let y = (pos.y - f64::from(py)).max(0.0) as u32;
        let col = (x / cw).min(u32::from(self.grid.0.saturating_sub(1)));
        let row = (y / ch).min(u32::from(self.grid.1.saturating_sub(1)));
        (col as u16, row as u16)
    }

    /// Space between the window edge and the cells.
    fn pad(&self) -> (u32, u32) {
        let (cw, _) = self.cell();
        ((cw * 3 / 2).saturating_sub(1), (9.0 * self.scale) as u32)
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

    fn on_mouse_button(&mut self, state: ElementState, button: MouseButton) {
        let b = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            _ => return,
        };
        let mods = mods_now();
        let pressed = state == ElementState::Pressed;
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

    /// Draws a frame. Resizes the session first when the window size or
    /// the font changed, at most once per frame.
    fn redraw(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.ensure_gfx();
        let pad = self.pad();
        let Some(g) = &mut self.gfx else {
            return;
        };
        let (cw, ch) = g.r.cell();
        let grid = grid_size(size, (cw, ch), pad);
        let Some(pane) = &self.pane else {
            return;
        };
        if grid != self.grid {
            self.grid = grid;
            pane.resize(grid.0, grid.1);
            lock(&pane.term).set_cell_px(cw as u16, ch as u16);
        }
        let cursor = {
            let mut t = lock(&pane.term);
            t.snapshot(&mut self.snap, &self.pal);
            t.cursor()
        };
        self.snap.selection = self.selection;

        let result = (|| {
            g.chain.resize(&g.r.gpu, size.width, size.height)?;
            g.chain.wait(100);
            for _ in 0..2 {
                g.r.begin();
                g.r.snapshot(&self.snap, &self.pal, pad.0 as i32, pad.1 as i32);
                overlays(
                    &mut g.r,
                    &self.pal,
                    pad,
                    grid,
                    cursor,
                    &self.preedit,
                    &self.notice,
                );
                let rtv = g.chain.rtv(&g.r.gpu)?;
                if !g.r.draw(&rtv, size.width, size.height, self.pal.bg)? {
                    break;
                }
            }
            if self.capture_then_exit
                && let Some(path) = &self.args.capture
            {
                let t = g.r.gpu.offscreen(size.width, size.height)?;
                g.r.draw(&t.rtv, size.width, size.height, self.pal.bg)?;
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

        let cell = (cursor.0, cursor.1);
        if self.ime_cell != Some(cell) {
            self.ime_cell = Some(cell);
            if let Some(w) = &self.window {
                w.set_ime_cursor_area(
                    PhysicalPosition::new(
                        pad.0 + u32::from(cell.0) * cw,
                        pad.1 + u32::from(cell.1) * ch,
                    ),
                    PhysicalSize::new(cw, ch),
                );
            }
        }
    }

    /// The soonest time something on screen changes by itself.
    fn next_deadline(&self) -> Option<Instant> {
        let now = Instant::now();
        let sync = self
            .pane
            .as_ref()
            .is_some_and(|p| lock(&p.term).sync_pending(now))
            .then(|| now + vt::modes::SYNC_TIMEOUT);
        [sync, self.notice.as_ref().and_then(|n| n.until)]
            .into_iter()
            .flatten()
            .min()
    }
}

/// Draws the IME composition at the cursor and the notice row.
fn overlays(
    r: &mut Renderer,
    pal: &Palette,
    pad: (u32, u32),
    grid: (u16, u16),
    cursor: (u16, u16, bool),
    preedit: &str,
    notice: &Option<Notice>,
) {
    let (cw, ch) = r.cell();
    if !preedit.is_empty() {
        let cols = preedit
            .chars()
            .map(|c| u16::from(vt::cluster_width(c.encode_utf8(&mut [0; 4])).max(1)))
            .sum::<u16>()
            .max(1);
        let mut s = text_snapshot(preedit, cols, 1, pal);
        for c in &mut s.cells {
            c.attrs |= vt::snapshot::attr::UNDERLINE;
        }
        let (x, y) = (
            pad.0 + u32::from(cursor.0) * cw,
            pad.1 + u32::from(cursor.1) * ch,
        );
        r.snapshot(&s, pal, x as i32, y as i32);
    }
    if let Some(n) = notice {
        let mut s = text_snapshot(&format!(" {}", n.text), grid.0, 1, pal);
        let bg = if n.dim { pal.bg } else { pal.selection_bg };
        for c in &mut s.cells {
            c.bg = bg;
            if n.dim {
                c.attrs |= vt::snapshot::attr::DIM;
            }
        }
        let banner = Palette { bg, ..*pal };
        let y = pad.1 + u32::from(grid.1.saturating_sub(1)) * ch;
        r.snapshot(&s, &banner, pad.0 as i32, y as i32);
    }
}

/// How many cells fit in a window of `size` pixels.
fn grid_size(size: PhysicalSize<u32>, (cw, ch): (u32, u32), (px, py): (u32, u32)) -> (u16, u16) {
    let fit = |len: u32, pad: u32, cell: u32| {
        (len.saturating_sub(2 * pad) / cell).clamp(1, u32::from(u16::MAX)) as u16
    };
    (fit(size.width, px, cw), fit(size.height, py, ch))
}

/// The text of the cells between two (column, row) points, inclusive, in
/// reading order: trailing blanks trimmed, rows joined by CRLF.
pub fn selection_text(snap: &Snapshot, sel: ((u16, u16), (u16, u16))) -> String {
    let (a, b) = sel;
    let (a, b) = if (a.1, a.0) <= (b.1, b.0) {
        (a, b)
    } else {
        (b, a)
    };
    let cols = usize::from(snap.cols);
    let mut lines = Vec::new();
    for row in a.1..=b.1.min(snap.rows.saturating_sub(1)) {
        let from = if row == a.1 { usize::from(a.0) } else { 0 };
        let to = if row == b.1 {
            usize::from(b.0).min(cols.saturating_sub(1))
        } else {
            cols.saturating_sub(1)
        };
        let mut line = String::new();
        for c in from..=to {
            let Some(cell) = snap.cells.get(usize::from(row) * cols + c) else {
                break;
            };
            match cell.len {
                // The right half of a wide character.
                0 if cell.width == 0 => {}
                0 => line.push(' '),
                n => {
                    line.push_str(std::str::from_utf8(&cell.text[..usize::from(n)]).unwrap_or(" "))
                }
            }
        }
        lines.push(line.trim_end().to_owned());
    }
    lines.join("\r\n")
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
            if self
                .notice
                .as_ref()
                .is_some_and(|n| n.until.is_some_and(|t| t <= now))
            {
                self.notice = None;
            }
            // A synchronized update timed out, or a notice expired.
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
                if f {
                    self.attention(Ev::Attended);
                }
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                self.preedit.clear();
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
            WindowEvent::MouseInput { state, button, .. } => self.on_mouse_button(state, button),
            WindowEvent::MouseWheel { delta, .. } => self.on_wheel(delta),
            _ => {}
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Pane(_, note) => self.on_pane(el, note),
            UserEvent::Finish(code) => {
                self.code = code;
                if self.args.capture.is_some() {
                    self.capture_then_exit = true;
                    self.redraw();
                }
                el.exit();
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
/// against the screen.
///
/// - `sendinput CHORD`: press a chord such as `shift+enter` or `ctrl+c`.
/// - `type TEXT`: type TEXT key by key on the active layout.
/// - `expect [MS] REGEX`: wait until a screen row matches (3000 ms default).
/// - `waitfor MS TEXT`: wait until TEXT appears on the screen.
/// - `sleep MS`, `note TEXT`.
mod selftest {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
        KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput,
        VIRTUAL_KEY, VkKeyScanW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SetForegroundWindow};

    use crate::debug::Regex;
    use crate::pane::lock;

    pub fn run(script: &str, hwnd: isize, term: &Mutex<vt::Terminal>) -> Result<(), String> {
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

    fn step(line: &str, hwnd: isize, term: &Mutex<vt::Terminal>) -> Result<(), String> {
        let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
        let wait = |ms: u64, done: &dyn Fn(&str) -> bool| {
            let end = Instant::now() + Duration::from_millis(ms);
            while Instant::now() < end {
                if done(&lock(term).screen_text()) {
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

    /// Sends key strokes to the foreground window, which must be blitz:
    /// anywhere else they would type into another program.
    fn send(hwnd: isize, strokes: &[Stroke]) -> Result<(), String> {
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
    }
}
