//! Links in pane text: URLs and file paths found in it, and opening a link
//! without ever running what it points to.

use std::ops::Range;
use std::path::{Path, PathBuf};

use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, w};

/// What Ctrl+click on a link opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// An OSC 8 hyperlink's URI, or a URL in the text.
    Uri(String),
    /// A file or folder named in the text.
    Path(PathBuf),
}

/// A link in the text.
#[derive(Debug, PartialEq, Eq)]
pub enum Link {
    /// An `http` or `https` URL.
    Url(String),
    /// A word that looks like a file path, without the `:line:col` after
    /// it. It is a link only if the file exists; see [`resolve`].
    Path(String),
}

/// How blitz opens a link.
#[derive(Debug, PartialEq, Eq)]
pub enum Open {
    /// Hand it to the program registered for its scheme.
    Uri(String),
    /// Open the file or folder with its program.
    File(PathBuf),
    /// Show the file selected in Explorer: opening it would run it.
    Reveal(PathBuf),
}

/// File types that open with their program: text, source code, images
/// and PDFs, which a program shows rather than runs. Any other type is
/// shown in Explorer: Windows and the programs on it keep adding types
/// that run or install when opened, so no list of those is ever complete.
const OPENS: &[&str] = &[
    "txt", "md", "markdown", "log", "json", "jsonc", "jsonl", "toml", "yaml", "yml", "ini", "sql",
    "rs", "c", "h", "cc", "cpp", "cxx", "hpp", "cs", "go", "java", "kt", "swift", "ts", "tsx",
    "css", "scss", "html", "htm", "png", "jpg", "jpeg", "gif", "bmp", "webp", "ico", "svg", "pdf",
];

/// Every URL and path-like word in `text`, the logical line under the
/// pointer, with where each is.
pub fn scan(text: &str) -> Vec<(Range<usize>, Link)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(c) = text[i..].chars().next() {
        let before = text[..i].chars().next_back();
        if before.is_none_or(|b| !b.is_alphanumeric())
            && let Some(n) = url_len(&text[i..])
        {
            out.push((i..i + n, Link::Url(text[i..i + n].to_owned())));
            i += n;
            continue;
        }
        if before.is_none_or(|b| !path_char(b))
            && let Some((path, end)) = path_at(&text[i..])
        {
            let found = Link::Path(text[i + path.start..i + path.end].to_owned());
            out.push((i + path.start..i + end, found));
            i += end;
            continue;
        }
        i += c.len_utf8();
    }
    out
}

/// The length of the URL that starts `s`, if one does.
fn url_len(s: &str) -> Option<usize> {
    let scheme = ["https://", "http://"]
        .into_iter()
        .find(|p| s.get(..p.len()).is_some_and(|h| h.eq_ignore_ascii_case(p)))?;
    let url_char = |c: char| {
        c.is_ascii_graphic() && !matches!(c, '<' | '>' | '"' | '`' | '{' | '}' | '|' | '\\' | '^')
    };
    let n = trim_end(&s[..s.find(|c| !url_char(c)).unwrap_or(s.len())]);
    (n > scheme.len()).then_some(n)
}

/// What a path word is made of: anything a Windows file name can hold
/// but `:`, which only follows a drive letter, and the backtick, which
/// quotes code.
fn path_char(c: char) -> bool {
    c.is_alphanumeric()
        || c.is_ascii_graphic() && !matches!(c, '<' | '>' | '"' | '|' | '?' | '*' | ':' | '`')
}

/// The path word at the start of `s`: where the path is in it, and where
/// the word ends after any `:line` or `:line:col`. A path starts with a
/// drive, or holds a `/` or `\` and ends in a name with an extension.
fn path_at(s: &str) -> Option<(Range<usize>, usize)> {
    // A bracket or quote before a path is not part of it.
    let lead = s.len() - s.trim_start_matches(['(', '[', '\'']).len();
    let p = &s[lead..];
    let b = p.as_bytes();
    let drive =
        b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/');
    let from = if drive { 2 } else { 0 };
    let word = from + p[from..].find(|c| !path_char(c)).unwrap_or(p.len() - from);
    let path = &p[..trim_end(&p[..word])];
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let named = name.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && !ext.is_empty() && ext.chars().all(char::is_alphanumeric)
    });
    // A path from a separator would be read from the root of the drive, or
    // from another machine for two.
    let relative = path.contains(['/', '\\']) && !path.starts_with(['/', '\\']) && named;
    if !drive && !relative {
        return None;
    }
    // ponytail: editor:line integration later
    let mut end = lead + path.len();
    for _ in 0..2 {
        let digits = s[end..].strip_prefix(':').map_or(0, |r| {
            r.len() - r.trim_start_matches(|c: char| c.is_ascii_digit()).len()
        });
        if digits == 0 {
            break;
        }
        end += 1 + digits;
    }
    Some((lead..lead + path.len(), end))
}

/// The length of `s` without the punctuation of a sentence around it, or
/// a closing bracket it does not open. The brackets are counted once, so
/// a long run of them costs no more than the rest of the text.
fn trim_end(s: &str) -> usize {
    // Round and square brackets, opening and closing.
    let (mut opens, mut closes) = ([0usize; 2], [0usize; 2]);
    for c in s.chars() {
        match c {
            '(' => opens[0] += 1,
            '[' => opens[1] += 1,
            ')' => closes[0] += 1,
            ']' => closes[1] += 1,
            _ => {}
        }
    }
    let mut n = s.len();
    loop {
        let k = match s[..n].chars().next_back() {
            Some('.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"') => None,
            Some(')') => Some(0),
            Some(']') => Some(1),
            _ => return n,
        };
        if let Some(k) = k {
            if opens[k] >= closes[k] {
                return n;
            }
            closes[k] -= 1;
        }
        n -= 1;
    }
}

/// Where a path word from the text is: as it is when it starts with a
/// drive, else in the pane's folder `cwd`. Only a path that exists counts,
/// and one that would reach another machine is never looked at.
// ponytail: looks at the disk on the UI thread for each move over a path
// while Ctrl is held; keep the last answer if a slow drive makes that lag.
pub fn resolve(word: &str, cwd: &str) -> Option<PathBuf> {
    let full = if vt::osc::local_dir(word) {
        PathBuf::from(word)
    } else if vt::osc::local_dir(cwd) {
        Path::new(cwd).join(word)
    } else {
        return None;
    };
    full.exists().then_some(full)
}

/// What opening an OSC 8 link's `uri` does. Only `http`, `https`,
/// `mailto` and local `file` URIs open: any other scheme starts whatever
/// program registered it, and some (`ms-msdt:`, `search-ms:`) have been
/// used to attack Windows.
pub fn plan_uri(uri: &str, pathext: &str) -> Option<Open> {
    // A space or quote could split the command line that starts the
    // program.
    if uri
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '"')
    {
        return None;
    }
    let (scheme, _) = uri.split_once(':')?;
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" | "mailto" => Some(Open::Uri(uri.to_owned())),
        "file" => plan_path(Path::new(&vt::osc::file_url_path(uri)?), pathext),
        _ => None,
    }
}

/// What opening the file or folder `path` does. It must be a plain drive
/// path. Never runs it: only a folder, a file with no type and the types
/// in [`OPENS`] open; anything else, or a type `pathext` runs, is shown in
/// Explorer instead.
pub fn plan_path(path: &Path, pathext: &str) -> Option<Open> {
    let s = path.to_str()?;
    // No `:` stream after the drive, and nothing a name cannot hold.
    if !vt::osc::local_dir(s) || s[2..].contains([':', '"', '*', '?', '<', '>', '|']) {
        return None;
    }
    // Windows drops dots and spaces at the end of a name: `x.exe.` is `x.exe`.
    let name = (path.file_name().and_then(|n| n.to_str()))
        .unwrap_or_default()
        .trim_end_matches(['.', ' ']);
    let runs = name.rsplit_once('.').is_some_and(|(_, ext)| {
        let is = |t: &str| !t.is_empty() && t.eq_ignore_ascii_case(ext);
        !OPENS.iter().any(|t| is(t)) || pathext.split(';').any(|t| is(t.trim_start_matches('.')))
    });
    Some(if runs {
        Open::Reveal(path.into())
    } else {
        Open::File(path.into())
    })
}

/// What opening `t` does, or `None` when blitz refuses. A file is judged
/// by what it really is: a link or a short 8.3 name can hide its type.
pub fn plan(t: &Target, pathext: &str) -> Option<Open> {
    let p = match t {
        Target::Uri(u) => plan_uri(u, pathext)?,
        Target::Path(p) => plan_path(p, pathext)?,
    };
    match p {
        Open::Uri(_) => Some(p),
        Open::File(p) | Open::Reveal(p) => {
            let real = std::fs::canonicalize(p).ok()?;
            let real = real.to_str()?;
            plan_path(
                Path::new(real.strip_prefix(r"\\?\").unwrap_or(real)),
                pathext,
            )
        }
    }
}

/// Opens `t` as [`plan`] allows.
pub fn open(t: &Target) -> Result<(), &'static str> {
    let pathext = std::env::var("PATHEXT").unwrap_or_default();
    let (file, args) = match plan(t, &pathext) {
        Some(Open::Uri(u)) => (u, String::new()),
        Some(Open::File(p)) => (p.display().to_string(), String::new()),
        Some(Open::Reveal(p)) => {
            // A relative SystemRoot would run a planted explorer.exe.
            let root = crate::shell::system_root(|k| std::env::var_os(k));
            let explorer = root.join("explorer.exe");
            (
                explorer.display().to_string(),
                format!("/select,\"{}\"", p.display()),
            )
        }
        None => return Err("blitz opens only web and mail links and files on this computer"),
    };
    // SAFETY: NUL-terminated strings that outlive the call, and no window.
    let done = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(file),
            &HSTRING::from(args),
            None,
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean it started.
    if done.0 as usize > 32 {
        Ok(())
    } else {
        Err("Windows could not open the link")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(s: &str) -> Vec<(&str, Link)> {
        scan(s).into_iter().map(|(r, f)| (&s[r], f)).collect()
    }

    fn url(s: &str) -> Link {
        Link::Url(s.into())
    }

    fn path(s: &str) -> Link {
        Link::Path(s.into())
    }

    #[test]
    fn links_scan_urls() {
        let one = |text, u: &'static str| assert_eq!(found(text), [(u, url(u))], "{text}");
        one(
            "(see https://example.com/a_(b))",
            "https://example.com/a_(b)",
        );
        one(
            "Visit https://x.com/a?b=1&c=2, then go",
            "https://x.com/a?b=1&c=2",
        );
        one("[docs](https://x.com/d).", "https://x.com/d");
        one("<https://x.com/>", "https://x.com/");
        one("'http://x.com/a#top'!", "http://x.com/a#top");
        one("HTTPS://X.COM/A", "HTTPS://X.COM/A");
        one(
            "https://example.com/a/b.html",
            "https://example.com/a/b.html",
        );
        for none in [
            "https://",
            "xhttps://a.com",
            "ftp://a.com",
            "see http:/a.com",
        ] {
            assert_eq!(found(none), [], "{none}");
        }
    }

    #[test]
    fn links_scan_a_long_run_of_brackets_quickly() {
        let t0 = std::time::Instant::now();
        for word in ["https://x.com/", "a/b.c"] {
            let text = format!("{word}{}", ")]".repeat(50_000));
            assert_eq!(found(&text).len(), 1, "{word}");
        }
        assert!(t0.elapsed().as_millis() < 500, "{:?}", t0.elapsed());
    }

    #[test]
    fn links_scan_paths() {
        assert_eq!(
            found("error in src/foo.rs:42 here"),
            [("src/foo.rs:42", path("src/foo.rs"))]
        );
        assert_eq!(
            found(r"wrote C:\Users\x\file.txt."),
            [(r"C:\Users\x\file.txt", path(r"C:\Users\x\file.txt"))]
        );
        assert_eq!(
            found("C:/dev/blitz/src/app.rs:2066:9: error"),
            [(
                "C:/dev/blitz/src/app.rs:2066:9",
                path("C:/dev/blitz/src/app.rs")
            )]
        );
        assert_eq!(
            found(r"(see .\crates\vt\src\grid.rs) and 'a/b.md'"),
            [
                (r".\crates\vt\src\grid.rs", path(r".\crates\vt\src\grid.rs")),
                ("a/b.md", path("a/b.md"))
            ]
        );
        assert_eq!(
            found("`../x/y.toml`"),
            [("../x/y.toml", path("../x/y.toml"))]
        );
        for none in [
            "file.txt",
            "a/b",
            "1/2 done, v1.2/3",
            r"\\server\share\x.txt",
            "//host/x.txt",
            r"\Windows\notepad.exe",
            "a/.env",
        ] {
            assert_eq!(found(none), [], "{none}");
        }
    }

    #[test]
    fn links_open_only_what_is_safe() {
        let pathext = ".COM;.EXE;.BAT;.CMD;.VBS;.JS;.PY";
        let plan = |u| plan_uri(u, pathext);
        let uri = |u: &str| Some(Open::Uri(u.into()));
        assert_eq!(
            plan("https://example.com/a?b"),
            uri("https://example.com/a?b")
        );
        assert_eq!(plan("HTTP://x.com"), uri("HTTP://x.com"));
        assert_eq!(plan("mailto:a@b.c"), uri("mailto:a@b.c"));
        for bad in [
            "ms-msdt:/id PCWDiagnostic",
            "ms-msdt:-id",
            "search-ms:query=x",
            "javascript:alert(1)",
            "vbscript:x",
            "ms-settings:",
            r"\\server\share\x.txt",
            "file://server/share/x.txt",
            "file:///C:/x/a%0a.txt",
            "file:///C:/x/a.txt:evil.exe",
            "https://x.com/a b",
            "https://x.com/\"a",
            "https://x.com/\u{7}",
            "no scheme",
            "",
        ] {
            assert_eq!(plan(bad), None, "{bad:?}");
        }
        let file = |p: &str| Some(Open::File(p.into()));
        let reveal = |p: &str| Some(Open::Reveal(p.into()));
        assert_eq!(plan("file:///C:/docs/a%20b.txt"), file(r"C:\docs\a b.txt"));
        assert_eq!(plan("file:///C:/docs/"), file(r"C:\docs\"));
        assert_eq!(plan("file:///C:/x/setup.EXE"), reveal(r"C:\x\setup.EXE"));
        assert_eq!(
            plan("file:///C:/x/run.py"),
            reveal(r"C:\x\run.py"),
            "PATHEXT"
        );
        assert_eq!(plan("file:///C:/x/a.exe."), reveal(r"C:\x\a.exe."));
        assert_eq!(plan("file:///C:/x/a.exe%20"), reveal(r"C:\x\a.exe "));
        for t in [
            "lnk",
            "url",
            "scf",
            "hta",
            "msi",
            "msc",
            "ps1",
            "vbs",
            "vbe",
            "js",
            "jse",
            "wsf",
            "wsh",
            "reg",
            "scr",
            "cpl",
            "jar",
            "appref-ms",
            "application",
            "settingcontent-ms",
            "py",
            "chm",
            "diagcab",
            "themepack",
            // Types a list of what runs did not have, until each was found.
            "deskthemepack",
            "wsb",
            "rdp",
            "msu",
            "psc1",
            "jnlp",
            "diagcfg",
            "search-ms",
            "vhdx",
            "iso",
            "docm",
            "xlsm",
            "one",
            "accde",
            "made-up",
        ] {
            let p = format!(r"C:\x\a.{t}");
            assert_eq!(plan_path(Path::new(&p), ""), reveal(&p), "{t}");
        }
        for t in ["a.txt", "a.MD", "a.rs", "a.png", "a.pdf", "README", "dir"] {
            let p = format!(r"C:\x\{t}");
            assert_eq!(plan_path(Path::new(&p), ""), file(&p), "{t}");
        }
        let toml = r"C:\x\a.toml";
        assert_eq!(plan_path(Path::new(toml), ".TOML"), reveal(toml), "PATHEXT");
        assert_eq!(plan_path(Path::new(r"\\?\C:\x.txt"), pathext), None);
        assert_eq!(plan_path(Path::new("x.txt"), pathext), None);
    }

    #[test]
    fn links_judge_the_real_file() {
        let dir = std::env::temp_dir().join(format!("blitz-links-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).expect("dir");
        std::fs::write(dir.join("sub").join("notes.txt"), "x").expect("write");
        std::fs::write(dir.join("tool.cmd"), "x").expect("write");
        let cwd = dir.display().to_string();
        let notes = resolve("sub/notes.txt", &cwd);
        let tool = resolve("tool.cmd", &cwd);
        let gone = resolve("sub/gone.txt", &cwd);
        let far = resolve("sub/notes.txt", r"\\server\share");
        let plan = |p: &Option<PathBuf>| plan(&Target::Path(p.clone().expect("found")), "");
        let notes_plan = plan(&notes);
        let tool_plan = plan(&tool);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(notes_plan, Some(Open::File(p)) if p.ends_with(r"sub\notes.txt")));
        assert!(matches!(tool_plan, Some(Open::Reveal(p)) if p.ends_with("tool.cmd")));
        assert_eq!((gone, far), (None, None));
    }
}
