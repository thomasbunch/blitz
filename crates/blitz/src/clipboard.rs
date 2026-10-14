//! Clipboard text, and the files and images a paste can find instead.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT, CLIPBOARD_FORMAT};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

/// Holds the clipboard open; closes it on drop.
struct Open;

impl Open {
    /// Another program may hold the clipboard for a moment, so try a few
    /// times before giving up.
    fn new(owner: Option<HWND>) -> Option<Open> {
        for _ in 0..10 {
            // SAFETY: a plain Win32 call; a stale window only fails it.
            if unsafe { OpenClipboard(owner) }.is_ok() {
                return Some(Open);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: the clipboard was opened by this thread.
        let _ = unsafe { CloseClipboard() };
    }
}

/// Whether the clipboard holds `format`. Asked without opening it, so a
/// paste with nothing to take never waits on a program holding it open.
fn has(format: CLIPBOARD_FORMAT) -> bool {
    // SAFETY: a plain Win32 call.
    unsafe { IsClipboardFormatAvailable(u32::from(format.0)) }.is_ok()
}

/// The clipboard holds an image, such as a screenshot.
pub fn has_image() -> bool {
    has(CF_DIB) || has(CF_DIBV5)
}

/// The clipboard's text, or `None` when it holds no text.
pub fn get_text() -> Option<String> {
    if !has(CF_UNICODETEXT) {
        return None;
    }
    let _open = Open::new(None)?;
    // SAFETY: the clipboard is open; the handle stays valid until it closes,
    // and the locked memory is `GlobalSize` bytes long. Whoever put the text
    // there need not have ended it with a NUL, so the read stops at the end
    // of the block too.
    unsafe {
        let h = GetClipboardData(u32::from(CF_UNICODETEXT.0)).ok()?;
        let mem = HGLOBAL(h.0);
        let p = GlobalLock(mem).cast::<u16>();
        if p.is_null() {
            return None;
        }
        let units = std::slice::from_raw_parts(p, GlobalSize(mem) / 2);
        let n = units.iter().position(|&u| u == 0).unwrap_or(units.len());
        let text = String::from_utf16_lossy(&units[..n]);
        let _ = GlobalUnlock(mem);
        Some(text)
    }
}

/// The files copied in Explorer, or `None` when the clipboard holds none.
pub fn get_files() -> Option<Vec<PathBuf>> {
    if !has(CF_HDROP) {
        return None;
    }
    let _open = Open::new(None)?;
    // SAFETY: the clipboard is open and the handle stays valid until it
    // closes. Each name is read into a buffer one longer than the length
    // asked for first, for its NUL.
    unsafe {
        let drop = HDROP(GetClipboardData(u32::from(CF_HDROP.0)).ok()?.0);
        let mut files = Vec::new();
        for i in 0..DragQueryFileW(drop, u32::MAX, None) {
            let len = DragQueryFileW(drop, i, None) as usize;
            let mut name = vec![0u16; len + 1];
            let got = DragQueryFileW(drop, i, Some(&mut name)) as usize;
            files.push(OsString::from_wide(&name[..got.min(len)]).into());
        }
        (!files.is_empty()).then_some(files)
    }
}

/// Puts `text` on the clipboard. Returns false if that failed.
pub fn set_text(owner: Option<HWND>, text: &str) -> bool {
    let utf16: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let Some(_open) = Open::new(owner) else {
        return false;
    };
    // SAFETY: the clipboard is open; the allocation is large enough for the
    // copy, and the system owns it once SetClipboardData succeeds.
    unsafe {
        if EmptyClipboard().is_err() {
            return false;
        }
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, utf16.len() * 2) else {
            return false;
        };
        let p = GlobalLock(mem).cast::<u16>();
        if p.is_null() {
            let _ = GlobalFree(Some(mem));
            return false;
        }
        std::ptr::copy_nonoverlapping(utf16.as_ptr(), p, utf16.len());
        let _ = GlobalUnlock(mem);
        if SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(mem.0))).is_err() {
            let _ = GlobalFree(Some(mem));
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// The tests share the one clipboard, so they take turns.
    static TURN: Mutex<()> = Mutex::new(());

    /// Puts `bytes` on the clipboard as `format`, exactly as given.
    fn set_raw(format: CLIPBOARD_FORMAT, bytes: &[u8]) -> bool {
        let Some(_open) = Open::new(None) else {
            return false;
        };
        // SAFETY: as in `set_text`.
        unsafe {
            if EmptyClipboard().is_err() {
                return false;
            }
            let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else {
                return false;
            };
            let p = GlobalLock(mem).cast::<u8>();
            if p.is_null() {
                let _ = GlobalFree(Some(mem));
                return false;
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            let _ = GlobalUnlock(mem);
            SetClipboardData(u32::from(format.0), Some(HANDLE(mem.0))).is_ok()
        }
    }

    /// Puts back the user's text when the test ends, passed or not, or
    /// leaves the clipboard empty if it held none.
    struct Restore(Option<String>);

    impl Drop for Restore {
        fn drop(&mut self) {
            match &self.0 {
                Some(b) => {
                    set_text(None, b);
                }
                None => {
                    if let Some(_open) = Open::new(None) {
                        // SAFETY: the clipboard is open.
                        let _ = unsafe { EmptyClipboard() };
                    }
                }
            }
        }
    }

    /// Writes and reads back the real clipboard, then restores its text.
    #[test]
    fn clipboard_round_trip() {
        let _turn = TURN.lock();
        let _restore = Restore(get_text());
        let text = "blitz clipboard \u{2713}\r\nline 2";
        if !set_text(None, text) {
            eprintln!("SKIPPED: the clipboard is unavailable");
            return;
        }
        assert_eq!(get_text().as_deref(), Some(text));
        // Another program can leave out the NUL. With an odd size not even
        // the last unit is zero, and the text must still end with the block.
        let mut read = 0;
        for n in (3..400).step_by(2) {
            // Another program may hold the clipboard for a moment.
            if set_raw(CF_UNICODETEXT, &b"A\0".repeat(n)[..n]) {
                read += 1;
                let got = get_text().unwrap_or_default();
                assert_eq!(got, "A".repeat(n / 2), "{n} bytes");
            }
        }
        assert!(read > 100, "the clipboard was busy {} times", 199 - read);
    }

    /// Files as Explorer copies them: a DROPFILES header, whose first field
    /// says where the names start and whose last says they are UTF-16,
    /// then each name and its NUL, and one more NUL.
    fn dropfiles(paths: &[&str]) -> Vec<u8> {
        let head = size_of::<windows::Win32::UI::Shell::DROPFILES>() as u32;
        let mut b = head.to_le_bytes().to_vec();
        b.resize(head as usize - 4, 0);
        b.extend(1u32.to_le_bytes());
        for unit in paths.iter().flat_map(|p| p.encode_utf16().chain([0])) {
            b.extend(unit.to_le_bytes());
        }
        b.extend([0, 0]);
        b
    }

    /// Files copied in Explorer and a screenshot are found, and neither is
    /// text.
    #[test]
    fn clipboard_files_and_images() {
        let _turn = TURN.lock();
        let _restore = Restore(get_text());
        let paths = [r"C:\some dir\shot.png", r"D:\b.txt"];
        if !set_raw(CF_HDROP, &dropfiles(&paths)) {
            eprintln!("SKIPPED: the clipboard is unavailable");
            return;
        }
        assert_eq!(get_files(), Some(paths.map(PathBuf::from).to_vec()));
        assert_eq!(get_text(), None);
        assert!(!has_image());
        if set_raw(CF_DIB, &[0; 40]) {
            assert!(has_image());
            assert_eq!(get_files(), None);
            assert_eq!(get_text(), None);
        }
    }
}
