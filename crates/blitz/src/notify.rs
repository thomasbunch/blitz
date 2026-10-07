//! Bringing the user back to a session while blitz is in the background:
//! the taskbar button's flash and badge, Windows notifications, and a key
//! that works from any program.

use std::ffi::c_void;
use std::sync::Once;

use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{
    NotificationSetting, ToastNotification, ToastNotificationManager,
};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, CreateIcon, FLASHW_TRAY, FLASHWINFO, FindWindowExW, FlashWindowEx,
    HICON, MSGFLT_ALLOW, RegisterWindowMessageW, WM_HOTKEY,
};
use windows::core::{HSTRING, w};
use winit::event_loop::EventLoopProxy;

use crate::app::UserEvent;
use crate::layout::PaneId;

/// The id of blitz's own subclass of its window; the hand-off's is 1.
const SUBCLASS: usize = 2;

/// What the window's subclass keeps: where events go, and the number of
/// the message Explorer sends once it has made the taskbar button.
struct Hook {
    proxy: EventLoopProxy<UserEvent>,
    button: u32,
}

/// The id of the hot key [`global_jump`] takes.
const JUMP: i32 = 1;

/// The event for window message `msg` with `wparam`, if blitz wants it.
/// `button` is the number of TaskbarButtonCreated, 0 when it could not be
/// had.
fn event(msg: u32, wparam: usize, button: u32) -> Option<UserEvent> {
    match msg {
        WM_HOTKEY if wparam == JUMP as usize => Some(UserEvent::GlobalJump),
        _ if button != 0 && msg == button => Some(UserEvent::TaskbarButton),
        _ => None,
    }
}

/// Makes `hwnd` hear when Explorer makes its taskbar button again, as it
/// does after a restart, so the progress and badge can go back on; and
/// when the key [`global_jump`] takes is pressed.
pub fn install(hwnd: isize, proxy: EventLoopProxy<UserEvent>) {
    let hwnd = HWND(hwnd as *mut c_void);
    // SAFETY: a NUL-terminated name.
    let button = unsafe { RegisterWindowMessageW(w!("TaskbarButtonCreated")) };
    // An elevated blitz hears from Explorer only if it lets this one in.
    // SAFETY: a live window owned by this thread.
    let _ = unsafe { ChangeWindowMessageFilterEx(hwnd, button, MSGFLT_ALLOW, None) };
    // Leaked on purpose: the window lives as long as the process.
    let hook = Box::into_raw(Box::new(Hook { proxy, button }));
    // SAFETY: a live window owned by this thread; the subclass proc only
    // reads `hook`, which is never freed.
    let ok = unsafe { SetWindowSubclass(hwnd, Some(proc), SUBCLASS, hook as usize) };
    if !ok.as_bool() {
        eprintln!("blitz: cannot hear from the taskbar or Ctrl+Alt+J");
    }
}

unsafe extern "system" fn proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    hook: usize,
) -> LRESULT {
    // SAFETY: `install` leaked this hook for the window's lifetime.
    let hook = unsafe { &*(hook as *const Hook) };
    if let Some(e) = event(msg, wparam.0, hook.button) {
        let _ = hook.proxy.send_event(e);
    }
    // SAFETY: passes the message on unchanged.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// Flashes the taskbar button of `hwnd` `count` times. The button stays
/// lit after the last one until the window comes to the front, so a few
/// are enough and none goes on for ever.
pub fn flash(hwnd: isize, count: u32) {
    let info = FLASHWINFO {
        cbSize: size_of::<FLASHWINFO>() as u32,
        hwnd: HWND(hwnd as *mut c_void),
        dwFlags: FLASHW_TRAY,
        uCount: count,
        dwTimeout: 0,
    };
    // SAFETY: a filled-in struct that outlives the call; a stale window
    // handle only fails.
    let _ = unsafe { FlashWindowEx(&info) };
}

/// A round badge `size` pixels wide for the taskbar button, drawn like the
/// sidebar's dots: a dot of `fg`, or a ring of it, on a disc of `bg` that
/// sets it apart from the icon under it. Pixels are `0xAARRGGBB`, top row
/// first, with straight alpha as icons take it.
pub fn badge_pixels(size: u32, fg: u32, bg: u32, ring: bool) -> Vec<u32> {
    let r = size as f32 / 2.0;
    let inner = r - (size as f32 / 10.0).max(1.0);
    let stroke = (size as f32 / 8.0).max(1.5);
    // How much of a pixel `d` from the centre a circle of radius `rad` covers.
    let cover = |rad: f32, d: f32| (rad - d + 0.5).clamp(0.0, 1.0);
    (0..size * size)
        .map(|i| {
            let (x, y) = ((i % size) as f32 + 0.5 - r, (i / size) as f32 + 0.5 - r);
            let d = (x * x + y * y).sqrt();
            let mut k = cover(inner, d);
            if ring {
                k -= cover(inner - stroke, d);
            }
            let alpha = (cover(r, d) * 255.0).round() as u32;
            alpha << 24 | crate::theme::mix(bg, fg, k)
        })
        .collect()
}

/// An icon from [`badge_pixels`]; the caller destroys it.
pub fn badge_icon(size: u32, fg: u32, bg: u32, ring: bool) -> Option<HICON> {
    let px = badge_pixels(size, fg, bg, ring);
    // Ignored where the colour has alpha, which is everywhere; one byte a
    // pixel is more than its one bit.
    let mask = vec![0u8; px.len()];
    let n = size as i32;
    // SAFETY: both buffers hold at least `size` rows of `size` pixels in
    // the depths given, and outlive the call, which copies them.
    unsafe { CreateIcon(None, n, n, 1, 32, mask.as_ptr(), px.as_ptr().cast()) }.ok()
}

/// Takes Ctrl+Alt+J from every program for `hwnd`, which then hears of
/// each press, or with `on` false gives it back. False when another
/// program has it.
pub fn global_jump(hwnd: isize, on: bool) -> bool {
    let hwnd = Some(HWND(hwnd as *mut c_void));
    // SAFETY: a window this thread owns.
    unsafe {
        if on {
            let mods = MOD_CONTROL | MOD_ALT | MOD_NOREPEAT;
            RegisterHotKey(hwnd, JUMP, mods, u32::from(b'J')).is_ok()
        } else {
            let _ = UnregisterHotKey(hwnd, JUMP);
            true
        }
    }
}

/// Whether a main blitz window other than `hwnd` is open, which then
/// has the key [`global_jump`] takes.
pub fn other_main(hwnd: isize) -> bool {
    other_window(&HSTRING::from(crate::handoff::CLASS), hwnd)
}

/// Whether a top-level window of `class` other than `hwnd` is open.
fn other_window(class: &HSTRING, hwnd: isize) -> bool {
    let mut after = None;
    // SAFETY: a class name and no window name; a window closed meanwhile
    // only ends the search.
    while let Ok(w) = unsafe { FindWindowExW(None, after, class, None) } {
        if w.0 as isize != hwnd {
            return true;
        }
        after = Some(w);
    }
    false
}

/// The app id notifications show under. blitz gives it a name and icon
/// for this user when it first needs it, and leaves the process its own,
/// so taskbar pins made before still match the window. Debug builds have
/// their own.
const AUMID: &str = if cfg!(debug_assertions) {
    "blitz.terminal.dev"
} else {
    "blitz.terminal"
};

/// Notifications take their icon from a file.
const ICON: &[u8] = include_bytes!("../icon/blitz.ico");

/// Done before the first notification, which is how [`untoast_all`] knows
/// there may be some.
static REGISTERED: Once = Once::new();

/// Names the app id and gives it the icon, under this user's settings.
fn register() {
    let key = HSTRING::from(format!(r"Software\Classes\AppUserModelId\{AUMID}"));
    let name = if cfg!(debug_assertions) {
        "blitz dev"
    } else {
        "blitz"
    };
    let mut values = vec![("DisplayName", name.to_string())];
    if let Some(dir) = crate::session::dir() {
        let ico = dir.join("blitz.ico");
        if std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&ico, ICON))
            .is_ok()
        {
            values.push(("IconUri", ico.to_string_lossy().into_owned()));
        }
    }
    for (name, value) in values {
        let data: Vec<u16> = value.encode_utf16().chain([0]).collect();
        // SAFETY: NUL-terminated strings that outlive the call; the size
        // in bytes counts the NUL.
        let _ = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &key,
                &HSTRING::from(name),
                REG_SZ.0,
                Some(data.as_ptr().cast()),
                (data.len() * 2) as u32,
            )
        };
    }
}

/// The notification's XML: `lines`, the first in bold, and no sound when
/// `silent`. Empty lines are left out.
pub fn toast_xml(lines: &[&str], silent: bool) -> String {
    let mut xml = String::from(r#"<toast><visual><binding template="ToastGeneric">"#);
    for l in lines.iter().filter(|l| !l.is_empty()) {
        xml.push_str("<text>");
        for c in l.chars() {
            match c {
                '&' => xml.push_str("&amp;"),
                '<' => xml.push_str("&lt;"),
                '>' => xml.push_str("&gt;"),
                // XML has no place for most of them.
                c if c.is_control() || c == '\u{fffe}' || c == '\u{ffff}' => xml.push(' '),
                c => xml.push(c),
            }
        }
        xml.push_str("</text>");
    }
    xml.push_str("</binding></visual>");
    if silent {
        xml.push_str(r#"<audio silent="true"/>"#);
    }
    xml.push_str("</toast>");
    xml
}

/// Tells apart the notifications of two blitz processes, whose panes may
/// have the same ids.
fn group() -> HSTRING {
    HSTRING::from(std::process::id().to_string())
}

/// Shows notification `xml` about pane `id`, in place of the one before
/// about it. Clicking it sends [`UserEvent::ShowPane`]. Keep what it
/// returns while it is up, so the click still has somewhere to go. None
/// when the user turned blitz's notifications off in Windows.
pub fn toast(
    id: PaneId,
    xml: &str,
    proxy: &EventLoopProxy<UserEvent>,
) -> windows::core::Result<Option<ToastNotification>> {
    REGISTERED.call_once(register);
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml))?;
    let t = ToastNotification::CreateToastNotification(&doc)?;
    t.SetTag(&HSTRING::from(id.0.to_string()))?;
    t.SetGroup(&group())?;
    let proxy = proxy.clone();
    t.Activated(&TypedEventHandler::new(move |_, _| {
        let _ = proxy.send_event(UserEvent::ShowPane(id));
        Ok(())
    }))?;
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?;
    // Show still succeeds then, and nothing comes up. Where Windows cannot
    // say, the notification is shown.
    if notifier
        .Setting()
        .is_ok_and(|s| s != NotificationSetting::Enabled)
    {
        return Ok(None);
    }
    notifier.Show(&t)?;
    Ok(Some(t))
}

/// Takes the notification about pane `id` off the screen and out of the
/// notification centre.
pub fn untoast(id: PaneId) {
    if let Ok(h) = ToastNotificationManager::History() {
        let tag = HSTRING::from(id.0.to_string());
        let _ = h.RemoveGroupedTagWithId(&tag, &group(), &HSTRING::from(AUMID));
    }
}

/// Takes down every notification this blitz showed, as nothing will
/// answer a click on one once it exits.
pub fn untoast_all() {
    if REGISTERED.is_completed()
        && let Ok(h) = ToastNotificationManager::History()
    {
        let _ = h.RemoveGroupWithId(&group(), &HSTRING::from(AUMID));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_new_taskbar_button_and_the_jump_key_are_taken() {
        let button = 0xc0de;
        assert!(matches!(
            event(button, 0, button),
            Some(UserEvent::TaskbarButton)
        ));
        assert!(event(0x0010, 0, button).is_none(), "WM_CLOSE");
        // Without the registered number, no message is it, not even WM_NULL.
        assert!(event(0, 0, 0).is_none());
        assert!(matches!(
            event(WM_HOTKEY, JUMP as usize, button),
            Some(UserEvent::GlobalJump)
        ));
        // Ids below 0 are Windows' own, such as IDHOT_SNAPWINDOW.
        assert!(event(WM_HOTKEY, -1isize as usize, button).is_none());
    }

    #[test]
    fn other_windows_are_found_by_class() {
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP,
        };
        assert!(!other_window(&HSTRING::from("blitz.no-such-class"), 0));
        // SAFETY: a plain hidden top-level window of a system class.
        let w = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!(""),
                WS_POPUP,
                0,
                0,
                8,
                8,
                None,
                None,
                None,
                None,
            )
        }
        .expect("window");
        assert!(other_window(&HSTRING::from("STATIC"), 0));
        // SAFETY: the window was made on this thread.
        unsafe { DestroyWindow(w) }.expect("destroy");
    }

    #[test]
    fn badges_are_round_dots_or_rings() {
        let (fg, bg) = (0x11_22_33, 0xee_dd_cc);
        let n = 16;
        let dot = badge_pixels(n, fg, bg, false);
        let ring = badge_pixels(n, fg, bg, true);
        let at = |px: &[u32], x: u32, y: u32| px[(y * n + x) as usize];
        for px in [&dot, &ring] {
            assert_eq!(px.len(), 256);
            // Corners are clear; the edge is the background disc.
            assert_eq!(at(px, 0, 0) >> 24, 0);
            assert_eq!(at(px, 15, 15) >> 24, 0);
            let edge = at(px, 0, 8);
            assert_eq!(edge & 0xff_ffff, bg);
            assert!(edge >> 24 > 0x80, "mostly covered");
            // Round: the same across both axes and both diagonals.
            for y in 0..n {
                for x in 0..n {
                    assert_eq!(at(px, x, y), at(px, n - 1 - x, y));
                    assert_eq!(at(px, x, y), at(px, y, x));
                }
            }
        }
        assert_eq!(at(&dot, 8, 8), 0xff << 24 | fg, "a full dot");
        assert_eq!(at(&ring, 8, 8), 0xff << 24 | bg, "an open ring");
        assert_eq!(at(&ring, 8, 2), 0xff << 24 | fg, "the ring itself");
    }

    #[test]
    fn notifications_carry_their_text_safely() {
        let text = "Bash: a <b> & \"c\"\x07\u{fffe}\u{ffff}";
        let xml = toast_xml(&["pwsh 3 needs you", "", text], true);
        assert_eq!(
            xml,
            "<toast><visual><binding template=\"ToastGeneric\">\
             <text>pwsh 3 needs you</text>\
             <text>Bash: a &lt;b&gt; &amp; \"c\"   </text>\
             </binding></visual><audio silent=\"true\"/></toast>"
        );
        // Windows reads it; nothing is shown.
        let doc = XmlDocument::new().expect("an XML document");
        doc.LoadXml(&HSTRING::from(xml)).expect("well-formed");
        // Windows' own sound, unless the user turned it off there.
        assert!(!toast_xml(&["x"], false).contains("audio"));
    }
}
