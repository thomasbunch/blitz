//! The macOS pasteboard.

use std::path::PathBuf;

use objc2_app_kit::{
    NSPasteboard, NSPasteboardType, NSPasteboardTypeFileURL, NSPasteboardTypePNG,
    NSPasteboardTypeString, NSPasteboardTypeTIFF,
};
use objc2_foundation::{NSArray, NSCopying, NSString, NSURL};

/// Whether the pasteboard holds one of `types`, asked without reading it.
fn has(types: &[&NSPasteboardType]) -> bool {
    // SAFETY: the general pasteboard lives as long as the process, and the
    // array outlives the call.
    unsafe {
        let pb = NSPasteboard::generalPasteboard();
        let types = NSArray::from_vec(types.iter().map(|t| t.copy()).collect());
        pb.availableTypeFromArray(&types).is_some()
    }
}

/// The pasteboard holds an image, such as a screenshot.
pub fn has_image() -> bool {
    // SAFETY: the type names are AppKit's constants.
    let types = unsafe { [NSPasteboardTypePNG, NSPasteboardTypeTIFF] };
    has(&types)
}

/// The pasteboard's text, or `None` when it holds no text. Files copied in
/// the Finder come with their names as text, but paste as their paths;
/// see [`get_files`].
pub fn get_text() -> Option<String> {
    // SAFETY: the type name is AppKit's constant.
    if has(&[unsafe { NSPasteboardTypeFileURL }]) {
        return None;
    }
    // SAFETY: as in `has`; the string is copied out before it is released.
    unsafe {
        let pb = NSPasteboard::generalPasteboard();
        Some(pb.stringForType(NSPasteboardTypeString)?.to_string())
    }
}

/// The files copied in the Finder, or `None` when the pasteboard holds
/// none. The Finder puts file reference URLs there, which name a file by
/// its id, so each is turned back into a path.
pub fn get_files() -> Option<Vec<PathBuf>> {
    // SAFETY: as in `has`; every object read stays retained while it is
    // used.
    unsafe {
        let pb = NSPasteboard::generalPasteboard();
        let items = pb.pasteboardItems()?;
        let files: Vec<PathBuf> = (items.iter())
            .filter_map(|item| {
                let text = item.stringForType(NSPasteboardTypeFileURL)?;
                let url = NSURL::URLWithString(&text)?;
                Some(url.filePathURL()?.path()?.to_string().into())
            })
            .collect();
        (!files.is_empty()).then_some(files)
    }
}

/// Puts `text` on the pasteboard. Returns false if that failed. The owner
/// window is for Windows.
pub fn set_text(_owner: Option<isize>, text: &str) -> bool {
    // SAFETY: as in `has`; the pasteboard copies the string.
    unsafe {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        pb.setString_forType(&NSString::from_str(text), NSPasteboardTypeString)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes and reads back the real pasteboard, then restores its text.
    #[test]
    fn clipboard_round_trip() {
        let before = get_text();
        let text = "blitz clipboard \u{2713}\nline 2";
        assert!(set_text(None, text));
        let got = get_text();
        let files = get_files();
        set_text(None, before.as_deref().unwrap_or(""));
        assert_eq!(got.as_deref(), Some(text));
        assert_eq!(files, None);
    }
}
