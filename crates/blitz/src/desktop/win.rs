//! Windows: the system settings, taskbar, power and window frame that
//! the app follows.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
    GetDoubleClickTime, GetLastInputInfo, LASTINPUTINFO,
};
use windows::Win32::UI::Shell::{
    ITaskbarList3, TBPF_ERROR, TBPF_INDETERMINATE, TBPF_NOPROGRESS, TBPF_NORMAL, TBPF_PAUSED,
    TaskbarList,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateCaret, DestroyCaret, DestroyIcon, GetCursorPos, GetSystemMetrics, MB_OK, SM_CXSMICON,
    SetCaretPos, SetForegroundWindow,
};
use windows::core::{HSTRING, PWSTR};
use winit::dpi::PhysicalSize;

/// What the OS is called in an issue report.
pub const OS: &str = "Windows";

/// Whether Windows hides the pointer while typing: Mouse settings,
/// Pointer Options. Read once at start.
pub fn mouse_vanish() -> bool {
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
pub fn animations_on() -> bool {
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

/// How long ago the last key or mouse input anywhere was; zero when
/// Windows cannot say.
pub fn idle_for() -> Duration {
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

/// Windows' "lines to scroll" for each wheel notch; 3 if it cannot be read.
pub fn scroll_lines() -> u32 {
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

/// The Windows version, such as `10.0.26200`. The manifest build.rs links
/// in keeps Windows from giving an older one.
pub fn os_version() -> String {
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
pub fn small_icon_size() -> PhysicalSize<u32> {
    // SAFETY: reads a system metric; no pointers.
    let n = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16) as u32;
    PhysicalSize::new(n, n)
}

/// Asks Windows to start blitz again, with its saved session, after it
/// restarts for an update or the user signs back in with "restart apps"
/// on. Not after a crash or a hang, which could happen again at once,
/// nor after an installer closes it: blitz's own starts it again itself,
/// and one run as administrator would start it elevated.
pub fn restart_after_reboot() {
    use windows::Win32::System::Recovery::{
        RESTART_NO_CRASH, RESTART_NO_HANG, RESTART_NO_PATCH, RegisterApplicationRestart,
    };
    let flags = RESTART_NO_CRASH | RESTART_NO_HANG | RESTART_NO_PATCH;
    // SAFETY: no command line, so blitz starts with none.
    let _ = unsafe { RegisterApplicationRestart(None, flags) };
}

/// Whether setup made blitz start when the user signs in, and the user has
/// not since turned that off in Task Manager or Settings.
pub fn starts_at_sign_in() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_ANY, RegGetValueW};
    let value = |sub, buf: Option<&mut [u8; 12]>| {
        let mut size = 12u32;
        let (data, size) = match buf {
            Some(b) => (Some(b.as_mut_ptr().cast()), Some(&mut size as *mut u32)),
            None => (None, None),
        };
        // SAFETY: with a buffer, it holds the `size` bytes given; without
        // one, only whether the value is there comes back.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                sub,
                windows::core::w!("blitz"),
                RRF_RT_ANY,
                None,
                data,
                size,
            )
        };
        r.is_ok()
    };
    let run = windows::core::w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let approved = windows::core::w!(
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"
    );
    let mut flags = [0u8; 12];
    let set = value(approved, Some(&mut flags)).then_some(flags[0]);
    sign_in_starts(value(run, None), set)
}

/// Moves the system caret to `at`, in client pixels, first making one of
/// `size` when `made`, the size of the caret there is, differs. It is
/// never shown, as blitz draws its own cursor, but Magnifier and other
/// tools that follow the text cursor follow it. Returns the caret's size.
pub fn place_caret(
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
pub fn local_stamp() -> String {
    // SAFETY: plain Win32 call with no arguments.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{}-{:02}-{:02} {:02}:{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute
    )
}

/// How far the local clock is ahead of UTC, in seconds, at Unix time `t`,
/// daylight saving included.
pub fn utc_offset(t: i64) -> i64 {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{
        FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
    };
    // FILETIME counts 100 ns from 1601.
    let Some(ticks) = (u64::try_from(t).ok())
        .and_then(|s| s.checked_add(11_644_473_600)?.checked_mul(10_000_000))
    else {
        return 0;
    };
    let utc = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let (mut st, mut local, mut out) = (
        SYSTEMTIME::default(),
        SYSTEMTIME::default(),
        FILETIME::default(),
    );
    // SAFETY: each call reads and writes the plain values passed by
    // pointer; None is the time zone in use.
    let ok = unsafe {
        FileTimeToSystemTime(&utc, &mut st).is_ok()
            && SystemTimeToTzSpecificLocalTime(None, &st, &mut local).is_ok()
            && SystemTimeToFileTime(&local, &mut out).is_ok()
    };
    if !ok {
        return 0;
    }
    let local = (u64::from(out.dwHighDateTime) << 32) | u64::from(out.dwLowDateTime);
    (local as i64 - ticks as i64) / 10_000_000
}

/// Whether blitz runs elevated, as administrator.
pub fn elevated() -> bool {
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

/// Folders nobody means to work in that the Start menu, a pinned icon or
/// Win+R start blitz in: its own folder and the Windows system folder.
pub fn not_a_start() -> Vec<PathBuf> {
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

/// Calls `changed` whenever a file in `dir`, or a folder under it, is
/// written, until it returns false.
pub fn watch_dir(dir: PathBuf, mut changed: impl FnMut() -> bool + Send + 'static) {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::WAIT_OBJECT_0;
    use windows::Win32::Storage::FileSystem::{
        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FindFirstChangeNotificationW,
        FindNextChangeNotification,
    };
    use windows::Win32::System::Threading::{INFINITE, WaitForSingleObject};
    use windows::core::PCWSTR;

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
            if next.is_err() || !changed() {
                return;
            }
        }
    });
}

/// Whether a `Run` value starts blitz. Turning it off in Task Manager
/// leaves the value and sets the first byte of blitz's `StartupApproved`
/// entry, `approved`, to an odd one.
fn sign_in_starts(run: bool, approved: Option<u8>) -> bool {
    run && approved.is_none_or(|b| b & 1 == 0)
}

/// How long two clicks may be apart to count as a double click.
pub fn double_click() -> Duration {
    // SAFETY: a plain query.
    Duration::from_millis(u64::from(unsafe { GetDoubleClickTime() }))
}

/// The default sound for something that wants the user.
pub fn beep() {
    // SAFETY: a plain call.
    let _ = unsafe { MessageBeep(MB_OK) };
}

/// Says why blitz could not start, where a release build can: it has no
/// console.
pub fn error_box(text: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MessageBoxW};
    // SAFETY: two strings that outlive the call, and no owner.
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(text),
            &HSTRING::from("blitz"),
            MB_OK | MB_ICONERROR,
        );
    }
}

/// Brings window `hwnd` to the front. Windows lets it when the user just
/// asked for blitz.
pub fn to_front(hwnd: isize) {
    // SAFETY: our own window.
    let _ = unsafe { SetForegroundWindow(HWND(hwnd as *mut c_void)) };
}

/// Where the pointer is now, in the client pixels of `hwnd`.
pub fn pointer(hwnd: isize) -> Option<(f64, f64)> {
    let mut pt = POINT::default();
    // SAFETY: plain queries that write one POINT; a stale window only
    // fails the second.
    unsafe {
        GetCursorPos(&mut pt).ok()?;
        ScreenToClient(HWND(hwnd as *mut c_void), &mut pt)
            .ok()
            .ok()?;
    }
    Some((f64::from(pt.x), f64::from(pt.y)))
}

/// Takes away the caret [`place_caret`] made, from a window that lost the
/// keys.
pub fn drop_caret() {
    // SAFETY: plain call on the thread that made the caret.
    let _ = unsafe { DestroyCaret() };
}

/// Window frame in the theme's colours: `light` or dark everywhere, and
/// on Windows 11 the title bar, title text and border too (older
/// Windows refuses those and keeps the dark or light frame).
pub fn frame_theme(hwnd: isize, light: bool, caption: u32, text: u32, border: u32) {
    let hwnd = HWND(hwnd as *mut c_void);
    let dark = windows::core::BOOL::from(!light);
    // SAFETY: a live window and a BOOL-sized value.
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const dark).cast(),
            size_of_val(&dark) as u32,
        )
    };
    for (attr, rgb) in [
        (DWMWA_CAPTION_COLOR, caption),
        (DWMWA_TEXT_COLOR, text),
        (DWMWA_BORDER_COLOR, border),
    ] {
        // COLORREF is 0x00BBGGRR.
        let c = rgb.swap_bytes() >> 8;
        // SAFETY: a live window and a COLORREF-sized value.
        let _ = unsafe {
            DwmSetWindowAttribute(hwnd, attr, (&raw const c).cast(), size_of_val(&c) as u32)
        };
    }
}

/// The taskbar button, made the first time it is needed.
#[derive(Default)]
pub struct Taskbar(Option<ITaskbarList3>);

impl Taskbar {
    fn get(&mut self) -> Option<ITaskbarList3> {
        if self.0.is_none() {
            // SAFETY: COM calls on the window's thread, where winit has
            // started OLE for drag and drop.
            self.0 = unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) }
                .ok()
                .filter(|t: &ITaskbarList3| unsafe { t.HrInit() }.is_ok());
        }
        self.0.clone()
    }

    /// Shows progress `p` on the button of `hwnd`, or clears it. False
    /// when there is no button to show it on.
    pub fn progress(&mut self, hwnd: isize, p: Option<crate::render::chrome::Progress>) -> bool {
        let Some(t) = self.get() else {
            return false;
        };
        let hwnd = HWND(hwnd as *mut c_void);
        let (flag, pct) = match p {
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
        true
    }

    /// Badges the button of `hwnd` with a dot of `fg` on `bg`, a ring if
    /// `ring`, that says `label` to a screen reader; `None` clears it.
    /// False when there is no button to badge.
    pub fn badge(&mut self, hwnd: isize, dot: Option<(u32, u32, bool)>, label: &str) -> bool {
        let Some(t) = self.get() else {
            return false;
        };
        let icon = dot.and_then(|(fg, bg, ring)| {
            let size = small_icon_size().width;
            crate::notify::badge_icon(size, fg, bg, ring)
        });
        let hwnd = HWND(hwnd as *mut c_void);
        // SAFETY: a live window; the taskbar keeps a copy of the icon, so
        // it is destroyed once set.
        unsafe {
            let _ = t.SetOverlayIcon(hwnd, icon.unwrap_or_default(), &HSTRING::from(label));
            if let Some(i) = icon {
                let _ = DestroyIcon(i);
            }
        }
        true
    }

    /// Explorer restarted: the old button may still talk to the Explorer
    /// that is gone.
    pub fn lost(&mut self) {
        self.0 = None;
    }
}

/// Keeps the PC from going to sleep by itself while on; `powercfg
/// /requests` says why.
#[derive(Default)]
pub struct Awake(Option<HANDLE>);

impl Awake {
    /// Turns it `on` or off; false when Windows refused.
    pub fn set(&mut self, on: bool) -> bool {
        if self.0.is_none() {
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
            self.0 = unsafe { PowerCreateRequest(&context) }.ok();
        }
        let Some(r) = self.0 else {
            return false;
        };
        // SAFETY: a power request blitz made and never closes.
        let done = unsafe {
            if on {
                PowerSetRequest(r, PowerRequestSystemRequired)
            } else {
                PowerClearRequest(r, PowerRequestSystemRequired)
            }
        };
        done.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Startup apps in Task Manager turn the start off without the value.
    #[test]
    fn a_sign_in_start_turned_off_does_not_count() {
        assert!(sign_in_starts(true, None));
        assert!(sign_in_starts(true, Some(0x02)));
        assert!(sign_in_starts(true, Some(0x06)));
        assert!(!sign_in_starts(true, Some(0x03)));
        assert!(!sign_in_starts(true, Some(0x07)));
        assert!(!sign_in_starts(false, None));
        assert!(!sign_in_starts(false, Some(0x02)));
    }

    #[test]
    fn the_clock_is_at_most_14_hours_off_utc() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        assert!(utc_offset(now).abs() <= 14 * 3600);
        assert_eq!(utc_offset(-1), 0);
    }
}
