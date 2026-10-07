//! Links in pane text: URLs and file paths found in it, and opening a link
//! without ever running what it points to.

use std::ops::Range;
use std::path::{Path, PathBuf};

use windows::Win32::UI::Shell::{
    ASSOCF_INIT_IGNOREUNKNOWN, ASSOCSTR_FRIENDLYAPPNAME, AssocQueryStringW, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

/// What Ctrl+click on a link opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// An OSC 8 hyperlink's URI, or a URL in the text.
    Uri(String),
    /// A file or folder named in the text, and the line and column after
    /// it.
    Path(PathBuf, Option<(u32, u32)>),
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
    // Letters and digits of any script, as in `https://bücher.de`, but not
    // the quotes and punctuation of the text around it.
    let url_char = |c: char| {
        c.is_ascii_graphic() && !matches!(c, '<' | '>' | '"' | '`' | '{' | '}' | '|' | '\\' | '^')
            || !c.is_ascii() && c.is_alphanumeric()
    };
    let n = trim_end(&s[..word_len(s, url_char)]);
    (n > scheme.len()).then_some(n)
}

/// The length of the run of `ok` chars that starts `s`. A mark or joiner
/// that builds one character with the char before it stays in the run, as
/// in a decomposed `é` or `हिन्दी`, but no invisible format character does,
/// and a joiner left at the end joins nothing.
fn word_len(s: &str, ok: impl Fn(char) -> bool) -> usize {
    let mut prev = None;
    // Marks and joiners are never ASCII or space. A Prepend letter such as
    // U+0D4E joins whatever follows it, so only those can join after one.
    let n = s
        .find(|c: char| {
            let joined = prev.is_some_and(|p| vt::width::joins(p, p, 1, c))
                && !c.is_ascii()
                && !c.is_whitespace()
                && (!vt::width::is_ignorable(c) || matches!(c, '\u{200C}' | '\u{200D}'));
            prev = Some(c);
            !ok(c) && !joined
        })
        .unwrap_or(s.len());
    s[..n].trim_end_matches(['\u{200C}', '\u{200D}']).len()
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
    let word = from + word_len(&p[from..], path_char);
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
/// drive, in the user's folder for `~`, else in the pane's folder `cwd`.
/// Only a path that exists counts, and one that would reach another
/// machine is never looked at, nor is one through a link; see [`plain`].
pub fn resolve(word: &str, cwd: &str) -> Option<PathBuf> {
    let home = std::env::var("USERPROFILE").ok();
    let (full, from) = full_path(word, cwd, home.as_deref())?;
    plain(&full, from).then_some(full)
}

/// [`resolve`] without looking at the disk, `~` standing for `home`, and
/// the length of the folder it starts from, which is trusted.
fn full_path(word: &str, cwd: &str, home: Option<&str>) -> Option<(PathBuf, usize)> {
    let (base, rest) = match word.strip_prefix('~') {
        Some(r) if r.starts_with(['/', '\\']) => (home?, &r[1..]),
        _ if vt::osc::local_dir(word) => return Some((PathBuf::from(word), 3)),
        _ => (cwd, word),
    };
    // A rest that is itself absolute, such as `\\host\share`, replaces
    // the base when joined, so the result is checked too, and then none
    // of it is trusted.
    let full = Path::new(base).join(rest);
    let from = if full.starts_with(base) {
        base.len()
    } else {
        3
    };
    (vt::osc::local_dir(base) && vt::osc::local_dir(&full.to_string_lossy()))
        .then_some((full, from))
}

/// Whether each folder and file `path` names after its first `from`
/// bytes is there and is no symbolic link or junction, and whether `path`
/// is there when it is no more than those bytes, as `Z:\` is. Each is
/// looked at without following it: a link can lead to another machine,
/// and looking at a file there makes Windows sign in to it.
fn plain(path: &Path, from: usize) -> bool {
    (path.ancestors())
        .take_while(|a| a.as_os_str().len() > from)
        .all(|a| std::fs::symlink_metadata(a).is_ok_and(|m| !m.file_type().is_symlink()))
        // A longer `path` was the first ancestor looked at.
        && (path.as_os_str().len() > from || std::fs::symlink_metadata(path).is_ok())
}

/// What opening an OSC 8 link's `uri` does. Only `http`, `https` and
/// local `file` URIs open: any other scheme starts whatever program
/// registered it, and some (`ms-msdt:`, `search-ms:`) have been used to
/// attack Windows. Even `mailto:` hands a message the program wrote,
/// attachments included in some mail programs, to the one registered.
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
        "http" | "https" => Some(Open::Uri(uri.to_owned())),
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
        Target::Path(p, _) => plan_path(p, pathext)?,
    };
    match p {
        Open::Uri(_) => Some(p),
        Open::File(p) | Open::Reveal(p) => {
            // A path from the text passed [`resolve`] already.
            if matches!(t, Target::Uri(_)) && !plain(&p, 3) {
                return None;
            }
            let real = std::fs::canonicalize(p).ok()?;
            let real = real.to_str()?;
            plan_path(
                Path::new(real.strip_prefix(r"\\?\").unwrap_or(real)),
                pathext,
            )
        }
    }
}

/// What opening a file that [`plan`] allows does with `editor`, the
/// `editor_uri` setting: with one, a file opens there at `place`, else at
/// line 1, whatever its type, as an editor never runs it. Without one, a
/// file no program opens (`has_program` says) is shown in Explorer, so
/// Windows does not ask what to open it with. Folders open as they are.
pub fn with_editor(
    how: Open,
    editor: &str,
    place: Option<(u32, u32)>,
    has_program: impl Fn(&Path) -> bool,
) -> Result<Open, &'static str> {
    Ok(match how {
        Open::File(p) | Open::Reveal(p) if !editor.is_empty() && !p.is_dir() => {
            let uri = editor_uri(editor, &p, place.unwrap_or((1, 1)));
            Open::Uri(uri.ok_or("editor_uri must be a URI such as vscode://file/{path}")?)
        }
        Open::File(p) if !p.is_dir() && !has_program(&p) => Open::Reveal(p),
        how => how,
    })
}

/// `template`, the `editor_uri` setting, with `{path}`, `{line}` and
/// `{col}` filled in. `None` unless it is a URI that is not a `file:` one,
/// so the setting never names a program or a file to run.
pub fn editor_uri(template: &str, path: &Path, (line, col): (u32, u32)) -> Option<String> {
    let (scheme, _) = template.split_once(':')?;
    let named = scheme.len() > 1
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
    let odd = |c: char| c.is_control() || c.is_whitespace() || c == '"';
    if !named || scheme.eq_ignore_ascii_case("file") || template.chars().any(odd) {
        return None;
    }
    let mut p = String::new();
    for b in path.to_str()?.replace('\\', "/").bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                p.push(char::from(b));
            }
            _ => p.push_str(&format!("%{b:02X}")),
        }
    }
    Some(
        (template.replace("{path}", &p))
            .replace("{line}", &line.to_string())
            .replace("{col}", &col.to_string()),
    )
}

/// Whether Windows has a program to open files of `path`'s type.
fn has_program(path: &Path) -> bool {
    let Some(ext) = path.extension() else {
        return false;
    };
    let ext = HSTRING::from(format!(".{}", ext.to_string_lossy()));
    let mut len = 0;
    // SAFETY: a NUL-terminated type that outlives the call; with no buffer
    // it only writes the length.
    let found = unsafe {
        AssocQueryStringW(
            ASSOCF_INIT_IGNOREUNKNOWN,
            ASSOCSTR_FRIENDLYAPPNAME,
            &ext,
            PCWSTR::null(),
            None,
            &mut len,
        )
    };
    found.is_ok()
}

/// Opens `t` as [`plan`] and [`with_editor`] allow.
pub fn open(t: &Target, editor: &str) -> Result<(), &'static str> {
    let pathext = std::env::var("PATHEXT").unwrap_or_default();
    let how = plan(t, &pathext).ok_or("blitz opens only web links and files on this computer")?;
    let place = match t {
        Target::Path(_, place) => *place,
        Target::Uri(_) => None,
    };
    let (file, args) = match with_editor(how, editor, place, has_program)? {
        Open::Uri(u) => (u, String::new()),
        Open::File(p) => (p.display().to_string(), String::new()),
        Open::Reveal(p) => {
            // A relative SystemRoot would run a planted explorer.exe.
            let root = crate::shell::system_root(|k| std::env::var_os(k));
            let explorer = root.join("explorer.exe");
            (
                explorer.display().to_string(),
                format!("/select,\"{}\"", p.display()),
            )
        }
    };
    shell_open(&file, &args).ok_or("Windows could not open the link")
}

/// What opens the text file `path` for editing, as (file, arguments) for
/// `ShellExecuteW`: the file itself when Windows has a program for its
/// type (`associated`), else Notepad from Windows folder `root`, as for
/// `.toml` until a program claims it.
pub fn edit_plan(path: &Path, associated: bool, root: &Path) -> (String, String) {
    let path = path.display().to_string();
    if associated {
        return (path, String::new());
    }
    let notepad = root.join("System32").join("notepad.exe");
    (notepad.display().to_string(), crate::shell::quote(&path))
}

/// Opens the text file `path` for editing, as [`edit_plan`] says.
pub fn edit(path: &Path) -> Result<(), &'static str> {
    // A relative SystemRoot would run a planted notepad.exe.
    let root = crate::shell::system_root(|k| std::env::var_os(k));
    let (file, args) = edit_plan(path, opens(path), &root);
    shell_open(&file, &args).ok_or("Windows could not open the file")
}

/// Whether Windows has a program that opens files of `path`'s type. The
/// "Unknown" type, which offers the Open with dialog, is none.
fn opens(path: &Path) -> bool {
    use windows::Win32::UI::Shell::{
        ASSOCF_INIT_IGNOREUNKNOWN, ASSOCSTR_COMMAND, AssocQueryStringW,
    };
    let ext = path.extension().unwrap_or_default().to_string_lossy();
    let mut n = 0;
    // SAFETY: NUL-terminated strings that outlive the call; with no buffer
    // only the length comes back.
    unsafe {
        AssocQueryStringW(
            ASSOCF_INIT_IGNOREUNKNOWN,
            ASSOCSTR_COMMAND,
            &HSTRING::from(format!(".{ext}")),
            w!("open"),
            None,
            &mut n,
        )
    }
    .is_ok()
}

/// Shows the folder `dir` in Explorer.
pub fn show_folder(dir: &Path) -> Result<(), &'static str> {
    shell_open(&dir.display().to_string(), "").ok_or("Windows could not open the folder")
}

/// `ShellExecuteW`'s open of `file` with `args`; `None` if it failed.
fn shell_open(file: &str, args: &str) -> Option<()> {
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
    (done.0 as usize > 32).then_some(())
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
    fn a_file_with_no_program_for_its_type_opens_in_notepad() {
        let (cfg, win) = (
            Path::new(r"C:\Users\me\blitz\config.toml"),
            Path::new(r"C:\W"),
        );
        assert_eq!(
            edit_plan(cfg, true, win),
            (cfg.display().to_string(), String::new())
        );
        assert_eq!(
            edit_plan(cfg, false, win),
            (
                r"C:\W\System32\notepad.exe".into(),
                cfg.display().to_string()
            )
        );
        let spaced = Path::new(r"C:\Users\Jo Ann\blitz\config.toml");
        assert_eq!(
            edit_plan(spaced, false, win).1,
            format!("\"{}\"", spaced.display())
        );
        // Notepad has text files; nothing has a type no one made up.
        assert!(opens(Path::new("a.txt")));
        assert!(!opens(Path::new("a.blitz-no-such-type-4b1d")));
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
        one("see https://bücher.de/x.", "https://bücher.de/x");
        one("“https://example.com/café”", "https://example.com/café");
        one("https://例え.jp/パス。", "https://例え.jp/パス");
        // Marks and joiners that build a letter, but no bidi control.
        one(
            "https://example.com/cafe\u{301}.",
            "https://example.com/cafe\u{301}",
        );
        one("https://हिन्दी.भारत/", "https://हिन्दी.भारत/");
        one(
            "https://x.ir/می\u{200C}خواهم",
            "https://x.ir/می\u{200C}خواهم",
        );
        one("https://x.com/a\u{202E}b", "https://x.com/a");
        // A Prepend letter joins no space or quote, and no joiner ends a URL.
        one("https://x.com/a\u{D4E} next", "https://x.com/a\u{D4E}");
        one("https://x.com/a\u{D4E}\"q", "https://x.com/a\u{D4E}");
        one("https://x.com/\u{111C2}<b>", "https://x.com/\u{111C2}");
        one("https://x.com/a\u{200C} next", "https://x.com/a");
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
        // One pass takes milliseconds and the old quadratic trim took over
        // half a minute, so a slow, busy runner still has room.
        assert!(t0.elapsed().as_secs() < 5, "{:?}", t0.elapsed());
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
        let accent = "docs/cafe\u{301}.md";
        assert_eq!(found(accent), [(accent, path(accent))]);
        // A Prepend letter joins no space or quote: only the bare name
        // after it.
        let prepend = "docs/a\u{D4E} b.txt rest";
        assert_eq!(found(prepend), [("b.txt", path("b.txt"))]);
        let prepend = "docs\\x\u{111C2}\"y.txt";
        assert_eq!(found(prepend), [("y.txt", path("y.txt"))]);
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
        let full = |w| full_path(w, r"D:\work", home).map(|f| f.0);
        let me = Path::new(r"C:\Users\me");
        assert_eq!(full("~/x/y.rs"), Some(me.join("x/y.rs")));
        assert_eq!(full(r"~\y.rs"), Some(me.join("y.rs")));
        assert_eq!(full("a.rs"), Some(Path::new(r"D:\work").join("a.rs")));
        assert_eq!(full(r"E:\b.rs"), Some(PathBuf::from(r"E:\b.rs")));
        assert_eq!(full_path("~/x", r"D:\work", None), None, "no home");
        assert_eq!(full_path("~/x", r"D:\work", Some(r"\\server\me")), None);
        assert_eq!(full_path("a.rs", r"\\server\share", home), None);
        // Only what the word names is checked for links, not the folder it
        // starts from, unless a drive in the word took that folder's place.
        let from = |w| full_path(w, r"D:\work", home).map(|f| f.1);
        assert_eq!(from("a.rs"), Some(r"D:\work".len()));
        assert_eq!(from("~/x/y.rs"), Some(r"C:\Users\me".len()));
        assert_eq!(from(r"E:\b.rs"), Some(3));
        assert_eq!(from(r"~/E:\b.rs"), Some(3));
        for unc in [
            r"~/\\host\share\x.txt",
            r"~\\\host\share\x.txt",
            r"\\host\share\x.txt",
        ] {
            assert_eq!(full(unc), None, "{unc}");
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
        for bad in [
            "ms-msdt:/id PCWDiagnostic",
            "ms-msdt:-id",
            "search-ms:query=x",
            "javascript:alert(1)",
            "vbscript:x",
            "ms-settings:",
            "mailto:a@b.c?attach=C:/x/secret.txt",
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
        assert_eq!(plan("FILE:///C:/docs/a%20b.txt"), file(r"C:\docs\a b.txt"));
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
        let plan = |p: &Option<PathBuf>| plan(&Target::Path(p.clone().expect("found"), None), "");
        let notes_plan = plan(&notes);
        let tool_plan = plan(&tool);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(notes_plan, Some(Open::File(p)) if p.ends_with(r"sub\notes.txt")));
        assert!(matches!(tool_plan, Some(Open::Reveal(p)) if p.ends_with("tool.cmd")));
        assert_eq!((gone, far), (None, None));
        // A bare drive root counts only when the drive is there.
        let root = |l: char| format!(r"{l}:\");
        let missing = ('A'..='Z').rev().map(root).find(|r| !Path::new(r).exists());
        assert_eq!(resolve(&missing.expect("a free drive letter"), ""), None);
        assert!(resolve(&root('C'), "").is_some());
    }

    const VSCODE: &str = "vscode://file/{path}:{line}:{col}";

    #[test]
    fn links_fill_in_the_editor_uri() {
        let uri = |t, p: &str| editor_uri(t, Path::new(p), (12, 5));
        assert_eq!(
            uri(VSCODE, r"C:\my code\a.rs").as_deref(),
            Some("vscode://file/C:/my%20code/a.rs:12:5")
        );
        assert_eq!(
            uri("idea://open?file={path}&line={line}", r"C:\x\%#&é.rs").as_deref(),
            Some("idea://open?file=C:/x/%25%23%26%C3%A9.rs&line=12")
        );
        // Only a URI: never a program, a file, or text for a command line.
        for bad in [
            r"C:\tools\code.exe {path}",
            "code {path}",
            "c:{path}",
            "file:///{path}",
            "FILE:{path}",
            "vscode://file/{path} --x",
            "vscode://\"{path}\"",
            "1x:{path}",
            "",
        ] {
            assert_eq!(uri(bad, r"C:\a.rs"), None, "{bad}");
        }
    }

    #[test]
    fn links_open_files_in_the_editor_or_show_them() {
        let dir = std::env::temp_dir().join(format!("blitz-editor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("a.rs");
        std::fs::write(&file, "x").expect("write");
        let (yes, no) = (|_: &Path| true, |_: &Path| false);
        let go = |how, editor, program: &dyn Fn(&Path) -> bool| {
            with_editor(how, editor, Some((3, 4)), program)
        };
        let at = editor_uri(VSCODE, &file, (3, 4)).map(Open::Uri).ok_or("");
        let seen = [
            go(Open::File(file.clone()), VSCODE, &no),
            go(Open::Reveal(file.clone()), VSCODE, &no),
            go(Open::File(dir.clone()), VSCODE, &no),
            go(Open::File(file.clone()), "", &no),
            go(Open::File(file.clone()), "", &yes),
            go(Open::Reveal(file.clone()), "", &yes),
            go(Open::File(dir.clone()), "", &no),
            go(Open::File(file.clone()), "notepad {path}", &yes),
        ];
        let line_one = with_editor(Open::File(file.clone()), VSCODE, None, no);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(seen[0], at, "at its line");
        assert_eq!(seen[1], at, "an editor never runs a script");
        assert_eq!(seen[2], Ok(Open::File(dir.clone())), "a folder");
        assert_eq!(seen[3], Ok(Open::Reveal(file.clone())), "no program");
        assert_eq!(seen[4], Ok(Open::File(file.clone())));
        assert_eq!(seen[5], Ok(Open::Reveal(file.clone())));
        assert_eq!(seen[6], Ok(Open::File(dir.clone())));
        assert!(seen[7].is_err(), "not a URI");
        let one = editor_uri(VSCODE, &file, (1, 1)).map(Open::Uri).ok_or("");
        assert_eq!(line_one, one);
    }

    #[test]
    fn links_know_which_types_have_a_program() {
        // Which known types have one depends on the machine: Windows
        // Server can leave even .txt without a program's name.
        assert!(!has_program(Path::new(r"C:\x\a.blitz-no-such-type")));
        assert!(!has_program(Path::new(r"C:\x\Makefile")));
    }

    #[test]
    fn links_never_look_through_a_link() {
        let dir = std::env::temp_dir().join(format!("blitz-links-j-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("real")).expect("dir");
        std::fs::write(dir.join("real").join("a.txt"), "x").expect("write");
        // A junction needs no rights to make, unlike a symbolic link.
        let made = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(dir.join("j"))
            .arg(dir.join("real"))
            .output()
            .is_ok_and(|o| o.status.success());
        let cwd = dir.display().to_string();
        let real = resolve("real/a.txt", &cwd);
        let through = resolve("j/a.txt", &cwd);
        let link = resolve("j", &cwd);
        let uri = format!("file:///{}", dir.join("j").join("a.txt").display());
        let opened = plan(&Target::Uri(uri.replace('\\', "/")), "");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(made, "mklink");
        assert!(real.is_some());
        assert_eq!((through, link, opened), (None, None, None));
    }
}
