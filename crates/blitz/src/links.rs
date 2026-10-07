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
    /// A word that looks like a file path, and the line and column after
    /// it, as in `src/app.rs:12:5` or tsc's `src/app.ts(12,5)`; a line alone
    /// is at column 1. It is a link only if the file exists; see
    /// [`resolve`].
    Path(String, Option<(u32, u32)>),
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

/// File types that run when opened, besides those in `%PATHEXT%`.
const RUNS: &[&str] = &[
    "exe",
    "com",
    "bat",
    "cmd",
    "pif",
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
    // Run by an interpreter when one is installed.
    "py",
    "pyw",
    "pyz",
    "pyzw",
    "pyc",
    "rb",
    "rbw",
    "pl",
    "tcl",
    "ahk",
    "au3",
    // Run code or change settings when opened, without being programs.
    "ws",
    "wsc",
    "sct",
    "chm",
    "diagcab",
    "theme",
    "themepack",
    "desktopthemepackfile",
    "library-ms",
    "searchconnector-ms",
    "website",
    "xbap",
    "gadget",
    "msp",
    "mst",
    "msix",
    "msixbundle",
    "appx",
    "appxbundle",
    "appinstaller",
    "xll",
    "iqy",
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
        // Claude Code's tool headers, as in `Update(src/app.rs)`, Markdown
        // links and `--flag=path` put a path right after a word.
        if before.is_none_or(|b| !path_char(b) || matches!(b, '(' | '[' | '=' | '\''))
            && let Some((path, end, at)) = path_at(&text[i..])
        {
            let found = Link::Path(text[i + path.start..i + path.end].to_owned(), at);
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

/// Where a path is in a word, where the word ends, and the line and
/// column after the path.
type PathWord = (Range<usize>, usize, Option<(u32, u32)>);

/// The path word at the start of `s`: where the path is in it, where the
/// word ends after any `:line`, `:line:col` or `(line,col)`, and that line
/// and column. A path starts with a drive or `~`, holds a `/` or `\` and
/// ends in a name with an extension, or is a bare `name.ext`.
fn path_at(s: &str) -> Option<PathWord> {
    // A bracket or quote before a path is not part of it.
    let lead = s.len() - s.trim_start_matches(['(', '[', '\'']).len();
    let p = &s[lead..];
    let b = p.as_bytes();
    let drive =
        b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/');
    let home = p.starts_with("~/") || p.starts_with("~\\");
    let from = if drive { 2 } else { 0 };
    let word = from + p[from..].find(|c| !path_char(c)).unwrap_or(p.len() - from);
    // A Markdown link's text ends where its target starts.
    let word = p[..word].find("](").unwrap_or(word);
    let mut path = &p[..trim_end(&p[..word])];
    // tsc and MSBuild put the place in brackets: `a.ts(12,5)`.
    let mut at = None;
    let mut end = lead + path.len();
    if let Some(i) = path.rfind('(')
        && let Some(place) = path[i + 1..].strip_suffix(')').and_then(bracketed)
    {
        (path, at) = (&path[..i], Some(place));
    }
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let ext = name.rsplit_once('.').and_then(|(stem, ext)| {
        let ok = !stem.is_empty() && !ext.is_empty() && ext.chars().all(char::is_alphanumeric);
        ok.then_some(ext)
    });
    let sep = path.contains(['/', '\\']);
    // A path from a separator would be read from the root of the drive, or
    // from another machine for two.
    let relative = sep && !path.starts_with(['/', '\\']) && ext.is_some();
    // A bare name is looked for in the pane's folder; `v1.2` is no file.
    let bare = !sep && ext.is_some_and(|e| e.chars().any(|c| c.is_ascii_alphabetic()));
    // In `--flag=path` the path starts after the `=`.
    let flag = path
        .split(['/', '\\'])
        .next()
        .is_some_and(|s| s.contains('='));
    if !drive && !home && !relative && !bare || flag {
        return None;
    }
    if at.is_none() {
        let mut place = Vec::new();
        while place.len() < 2
            && let Some(r) = s[end..].strip_prefix(':')
            && let digits = r.len() - r.trim_start_matches(|c: char| c.is_ascii_digit()).len()
            && let Ok(n) = r[..digits].parse()
        {
            place.push(n);
            end += 1 + digits;
        }
        at = place
            .first()
            .map(|&line| (line, place.get(1).copied().unwrap_or(1)));
    }
    Some((lead..lead + path.len(), end, at))
}

/// The line and column in `12,5` or `12`, a line alone at column 1.
fn bracketed(s: &str) -> Option<(u32, u32)> {
    let num = |n: &str| {
        n.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| n.parse().ok())?
    };
    match s.split_once(',') {
        Some((line, col)) => Some((num(line)?, num(col)?)),
        None => Some((num(s)?, 1)),
    }
}

/// The length of `s` without the punctuation of a sentence around it, or
/// a closing bracket it does not open.
fn trim_end(mut s: &str) -> usize {
    loop {
        let unopened =
            |open, close| s.ends_with(close) && s.matches(open).count() < s.matches(close).count();
        if s.ends_with(['.', ',', ';', ':', '!', '?', '\'', '"'])
            || unopened('(', ')')
            || unopened('[', ']')
        {
            s = &s[..s.len() - 1];
        } else {
            return s.len();
        }
    }
}

/// Where a path word from the text is: as it is when it starts with a
/// drive, in the user's folder for `~`, else in the pane's folder `cwd`.
/// Only a path that exists counts, and one that would reach another
/// machine is never looked at.
// ponytail: looks at the disk on the UI thread for each move over a path
// while Ctrl is held; keep the last answer if a slow drive makes that lag.
pub fn resolve(word: &str, cwd: &str) -> Option<PathBuf> {
    let home = std::env::var("USERPROFILE").ok();
    full_path(word, cwd, home.as_deref()).filter(|p| p.exists())
}

/// [`resolve`] without looking at the disk, `~` standing for `home`.
fn full_path(word: &str, cwd: &str, home: Option<&str>) -> Option<PathBuf> {
    let (base, rest) = match word.strip_prefix('~') {
        Some(r) if r.starts_with(['/', '\\']) => (home?, &r[1..]),
        _ if vt::osc::local_dir(word) => return Some(PathBuf::from(word)),
        _ => (cwd, word),
    };
    vt::osc::local_dir(base).then(|| Path::new(base).join(rest))
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
/// path. Never runs it: a type in `pathext` or [`RUNS`] is shown in
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
        let mut types =
            (RUNS.iter().copied()).chain(pathext.split(';').map(|t| t.trim_start_matches('.')));
        types.any(|t| !t.is_empty() && t.eq_ignore_ascii_case(ext))
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
            let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
            let explorer = Path::new(&root).join("explorer.exe");
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
        Link::Path(s.into(), None)
    }

    fn path_at(s: &str, line: u32, col: u32) -> Link {
        Link::Path(s.into(), Some((line, col)))
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
    fn links_scan_paths() {
        assert_eq!(
            found("error in src/foo.rs:42 here"),
            [("src/foo.rs:42", path_at("src/foo.rs", 42, 1))]
        );
        assert_eq!(
            found(r"wrote C:\Users\x\file.txt."),
            [(r"C:\Users\x\file.txt", path(r"C:\Users\x\file.txt"))]
        );
        assert_eq!(
            found("C:/dev/blitz/src/app.rs:2066:9: error"),
            [(
                "C:/dev/blitz/src/app.rs:2066:9",
                path_at("C:/dev/blitz/src/app.rs", 2066, 9)
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
            "a/b",
            "1/2 done, v1.2/3",
            r"\\server\share\x.txt",
            "//host/x.txt",
            r"\Windows\notepad.exe",
            "a/.env",
            "v1.2 and 3.14",
            ".env",
            "~ and ~x",
            "a.b(x)",
        ] {
            assert_eq!(found(none), [], "{none}");
        }
    }

    #[test]
    fn links_scan_tool_headers_markdown_places_and_names() {
        // Claude Code's tool headers.
        assert_eq!(
            found("Update(src/app.rs)"),
            [("src/app.rs", path("src/app.rs"))]
        );
        assert_eq!(found("Read(README.md)"), [("README.md", path("README.md"))]);
        // A Markdown link's target.
        assert_eq!(
            found("[x](docs/x.md) and [a/y.md](y/z.md:3)."),
            [
                ("docs/x.md", path("docs/x.md")),
                ("a/y.md", path("a/y.md")),
                ("y/z.md:3", path_at("y/z.md", 3, 1))
            ]
        );
        assert_eq!(
            found("--manifest-path=crates/vt/Cargo.toml"),
            [("crates/vt/Cargo.toml", path("crates/vt/Cargo.toml"))]
        );
        // tsc and MSBuild.
        assert_eq!(
            found("src/app.ts(12,5): error TS2322"),
            [("src/app.ts(12,5)", path_at("src/app.ts", 12, 5))]
        );
        assert_eq!(
            found("Program.cs(7): warning"),
            [("Program.cs(7)", path_at("Program.cs", 7, 1))]
        );
        assert_eq!(
            found("Update(src/app.ts(1,2))"),
            [("src/app.ts(1,2)", path_at("src/app.ts", 1, 2))]
        );
        // The user's folder, and a bare name, if the file is there.
        assert_eq!(
            found(r"~/.claude/settings.json and ~\notes"),
            [
                ("~/.claude/settings.json", path("~/.claude/settings.json")),
                (r"~\notes", path(r"~\notes"))
            ]
        );
        assert_eq!(
            found("edit file.txt:3:9, then go"),
            [("file.txt:3:9", path_at("file.txt", 3, 9))]
        );
    }

    #[test]
    fn links_find_home_and_relative_paths() {
        let home = Some(r"C:\Users\me");
        let full = |w| full_path(w, r"D:\work", home);
        let me = Path::new(r"C:\Users\me");
        assert_eq!(full("~/x/y.rs"), Some(me.join("x/y.rs")));
        assert_eq!(full(r"~\y.rs"), Some(me.join("y.rs")));
        assert_eq!(full("a.rs"), Some(Path::new(r"D:\work").join("a.rs")));
        assert_eq!(full(r"E:\b.rs"), Some(PathBuf::from(r"E:\b.rs")));
        assert_eq!(full_path("~/x", r"D:\work", None), None, "no home");
        assert_eq!(full_path("~/x", r"D:\work", Some(r"\\server\me")), None);
        assert_eq!(full_path("a.rs", r"\\server\share", home), None);
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
        ] {
            let p = format!(r"C:\x\a.{t}");
            assert_eq!(plan_path(Path::new(&p), ""), reveal(&p), "{t}");
        }
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
