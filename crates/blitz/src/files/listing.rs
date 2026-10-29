//! Reading a folder for the Files view.

use std::cmp::Ordering;
use std::ffi::OsString;
use std::io;
use std::path::Path;

/// Entries a listing keeps: past this many, the rest are only counted. A
/// folder like `node_modules` can hold far more than anyone scrolls.
pub const MAX_ENTRIES: usize = 5000;

/// Names looked at before giving up on counting: a folder of millions of
/// files says "and more" instead of taking minutes.
const MAX_SEEN: usize = 200_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    /// Anything else, such as a link to nothing or a device.
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The name as the system has it, to go into or read.
    pub name: OsString,
    /// The name as shown, with anything not Unicode replaced.
    pub label: String,
    /// What it is, following a link to what it points to.
    pub kind: Kind,
    /// It is a symbolic link or a junction.
    pub link: bool,
    /// Bytes, for files.
    pub size: u64,
    /// A dot file, or hidden by Windows.
    pub hidden: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// Folders first, then files, each by name ignoring case.
    pub entries: Vec<Entry>,
    /// Entries left out past [`MAX_ENTRIES`].
    pub more: usize,
    /// Why the folder could not be read.
    pub error: Option<String>,
}

/// Lists `dir`. Never fails: a folder that cannot be read gives a listing
/// that says why.
pub fn read(dir: &Path) -> Listing {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            return Listing {
                error: Some(reason(&e)),
                ..Listing::default()
            };
        }
    };
    let mut all = Vec::new();
    let mut seen = 0;
    for e in rd.flatten() {
        seen += 1;
        if seen > MAX_SEEN {
            break;
        }
        let name = e.file_name();
        let Ok(ft) = e.file_type() else {
            continue;
        };
        let link = ft.is_symlink() || junction(&e);
        // A link is what it points to; one to nothing is Other.
        let meta = if link {
            std::fs::metadata(e.path()).ok()
        } else {
            e.metadata().ok()
        };
        let kind = match &meta {
            Some(m) if m.is_dir() => Kind::Dir,
            Some(m) if m.is_file() => Kind::File,
            _ if !link && ft.is_dir() => Kind::Dir,
            _ => Kind::Other,
        };
        let size = meta.as_ref().filter(|m| m.is_file()).map_or(0, |m| m.len());
        let label = name.to_string_lossy().into_owned();
        let hidden = label.starts_with('.') || system_hidden(&e);
        all.push(Entry {
            name,
            label,
            kind,
            link,
            size,
            hidden,
        });
    }
    all.sort_by(order);
    let more = all.len().saturating_sub(MAX_ENTRIES) + seen.saturating_sub(MAX_SEEN);
    all.truncate(MAX_ENTRIES);
    Listing {
        entries: all,
        more,
        error: None,
    }
}

/// Folders before everything else, then by name ignoring case, then by
/// name, so that `B` and `b` keep one order.
fn order(a: &Entry, b: &Entry) -> Ordering {
    let dir = |e: &Entry| e.kind != Kind::Dir;
    (dir(a).cmp(&dir(b)))
        .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
        .then_with(|| a.label.cmp(&b.label))
}

/// What to say when a folder or file cannot be read.
pub fn reason(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => "It is not there any more".into(),
        io::ErrorKind::PermissionDenied => "blitz is not allowed to read it".into(),
        io::ErrorKind::NotADirectory => "It is not a folder".into(),
        _ => e.to_string(),
    }
}

#[cfg(windows)]
fn system_hidden(e: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    e.metadata()
        .is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
}

#[cfg(not(windows))]
fn system_hidden(_e: &std::fs::DirEntry) -> bool {
    false
}

/// A junction, which Windows does not count as a symbolic link but which
/// points elsewhere just the same.
#[cfg(windows)]
fn junction(e: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    e.metadata()
        .is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

#[cfg(not(windows))]
fn junction(_e: &std::fs::DirEntry) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("blitz-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn labels(l: &Listing) -> Vec<&str> {
        l.entries.iter().map(|e| e.label.as_str()).collect()
    }

    #[test]
    fn folders_come_first_then_names_ignoring_case() {
        let d = temp("order");
        // Distinct names whatever the case: Windows and macOS ignore it.
        for f in ["b.txt", "A.txt", "c.txt", ".env"] {
            std::fs::write(d.join(f), "x").unwrap();
        }
        for f in ["zed", "Src", ".git"] {
            std::fs::create_dir(d.join(f)).unwrap();
        }
        let l = read(&d);
        assert_eq!(
            labels(&l),
            [".git", "Src", "zed", ".env", "A.txt", "b.txt", "c.txt"]
        );
        let hidden: Vec<bool> = l.entries.iter().map(|e| e.hidden).collect();
        assert_eq!(hidden, [true, false, false, true, false, false, false]);
        assert_eq!(l.entries[3].size, 1);
        assert_eq!(l.entries[0].kind, Kind::Dir);
        assert_eq!(l.entries[3].kind, Kind::File);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_folder_that_is_not_there_says_so() {
        let d = temp("gone");
        let l = read(&d.join("nope"));
        assert!(l.entries.is_empty());
        assert_eq!(l.error.as_deref(), Some("It is not there any more"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn big_folders_keep_the_first_entries_and_count_the_rest() {
        let d = temp("big");
        for i in 0..MAX_ENTRIES + 3 {
            std::fs::write(d.join(format!("f{i:05}")), "").unwrap();
        }
        let l = read(&d);
        assert_eq!(l.entries.len(), MAX_ENTRIES);
        assert_eq!(l.more, 3);
        assert_eq!(l.entries[0].label, "f00000");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn links_are_what_they_point_to() {
        let d = temp("links");
        std::fs::create_dir(d.join("real")).unwrap();
        std::os::unix::fs::symlink(d.join("real"), d.join("to-dir")).unwrap();
        std::os::unix::fs::symlink(d.join("nothing"), d.join("broken")).unwrap();
        let l = read(&d);
        let got: Vec<(&str, Kind, bool)> = (l.entries.iter())
            .map(|e| (e.label.as_str(), e.kind, e.link))
            .collect();
        assert_eq!(
            got,
            [
                ("real", Kind::Dir, false),
                ("to-dir", Kind::Dir, true),
                ("broken", Kind::Other, true)
            ]
        );
        std::fs::remove_dir_all(&d).unwrap();
    }
}
