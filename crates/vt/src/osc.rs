//! OSC payload parsing: titles, cwd, hyperlinks, notifications, colours,
//! clipboard and shell-integration marks.

use crate::terminal::PromptMark;

/// What an `OSC 9 ; body` means. ConEmu uses numbered subcommands, iTerm2
/// uses free text for a notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Osc9<'a> {
    Notify(&'a str),
    Progress { state: u8, pct: Option<u8> },
    Cwd(&'a str),
    PromptStart,
    Ignore,
}

/// Classifies the text after `9;`. A body starting with digits is a ConEmu
/// subcommand; only progress (4), cwd (9) and prompt start (12) are used.
/// The others are dropped on purpose: 6 runs a GUI macro, 7 starts a
/// process and 8 echoes environment variables.
pub fn classify_osc9(body: &str) -> Osc9<'_> {
    if body.is_empty() {
        return Osc9::Ignore;
    }
    let (head, rest) = body.split_once(';').unwrap_or((body, ""));
    if !head.bytes().all(|b| b.is_ascii_digit()) {
        return Osc9::Notify(body);
    }
    match head {
        "4" => {
            let mut it = rest.split(';');
            match it.next().and_then(|s| s.parse::<u8>().ok()) {
                Some(state @ 0..=4) => Osc9::Progress {
                    state,
                    pct: it
                        .next()
                        .and_then(|p| p.parse::<u32>().ok())
                        .map(|p| p.min(100) as u8),
                },
                _ => Osc9::Ignore,
            }
        }
        "9" => Osc9::Cwd(rest.trim_matches('"')),
        "12" => Osc9::PromptStart,
        _ => Osc9::Ignore,
    }
}

/// Parses the text after `133;`. Extra `key=value` options are ignored,
/// except that a prompt start tagged `blitz=<token>` comes from blitz's own
/// shell integration. An empty token matches nothing.
pub fn prompt_mark(body: &str, token: &str) -> Option<PromptMark> {
    let mut it = body.split(';');
    Some(match it.next()? {
        "A" => PromptMark::A {
            blitz: !token.is_empty() && it.any(|o| o.strip_prefix("blitz=") == Some(token)),
        },
        "B" => PromptMark::B,
        "C" => PromptMark::C,
        "D" => PromptMark::D(it.next().and_then(|c| c.parse().ok())),
        _ => return None,
    })
}

/// The local path in an OSC 7 `file://host/path` URL, percent-decoded.
/// `file:///C:/x` gives `C:\x`. On Windows a URL naming another host gives
/// nothing: its path is not on this machine.
pub fn file_url_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let (host, path) = rest.split_at(rest.find('/')?);
    let path = percent_decode(path);
    let b = path.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        return Some(path[1..].replace('/', "\\"));
    }
    if cfg!(windows) && !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
        return None;
    }
    Some(path)
}

/// Longest working directory kept, in bytes.
const MAX_CWD: usize = 4096;

/// Whether a reported working directory is a plain local path: a drive
/// path such as `C:\x`, or outside Windows a path from a single `/`. Any
/// program can report one, and the host looks in it for a git branch and
/// starts new shells there, so UNC, device and relative paths are
/// refused: just looking at `\\host\share` makes Windows sign in to that
/// host.
pub fn local_dir(p: &str) -> bool {
    let b = p.as_bytes();
    let drive =
        b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/');
    let root = !cfg!(windows) && b.first() == Some(&b'/') && b.get(1) != Some(&b'/');
    (drive || root) && p.len() <= MAX_CWD && !p.chars().any(char::is_control)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b
            .get(i + 1..i + 3)
            .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
            .and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match (b[i], hex) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `s` without control characters, cut to `max` characters. The parser
/// already drops C0 controls inside an OSC; this catches C1.
pub fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

/// Parses an X11 colour spec: `#rgb`, `#rrggbb`, `#rrrgggbbb`,
/// `#rrrrggggbbbb` or `rgb:r/g/b` with 1 to 4 hex digits per channel.
/// Returns `0xRRGGBB`.
pub fn parse_color(spec: &str) -> Option<u32> {
    // Scales a channel of `n` hex digits to 8 bits.
    let chan = |h: &str| -> Option<u32> {
        if h.is_empty() || h.len() > 4 {
            return None;
        }
        let v = u32::from_str_radix(h, 16).ok()?;
        let max = (1u32 << (4 * h.len())) - 1;
        Some((v * 255 + max / 2) / max)
    };
    let (r, g, b) = if let Some(hex) = spec.strip_prefix('#') {
        let n = hex.len() / 3;
        if n == 0 || hex.len() % 3 != 0 || !hex.is_ascii() {
            return None;
        }
        (
            chan(&hex[..n])?,
            chan(&hex[n..2 * n])?,
            chan(&hex[2 * n..])?,
        )
    } else {
        let mut it = spec.strip_prefix("rgb:")?.split('/');
        let c = (chan(it.next()?)?, chan(it.next()?)?, chan(it.next()?)?);
        if it.next().is_some() {
            return None;
        }
        c
    };
    Some(r << 16 | g << 8 | b)
}

/// Appends `OSC n ; rgb:RRRR/GGGG/BBBB` and the terminator the query used.
pub fn color_reply(n: usize, rgb: u32, bel: bool, out: &mut Vec<u8>) {
    let c = |shift: u32| (rgb >> shift & 0xFF) * 0x101;
    let end = if bel { "\x07" } else { "\x1b\\" };
    let s = format!("\x1b]{n};rgb:{:04x}/{:04x}/{:04x}{end}", c(16), c(8), c(0));
    out.extend_from_slice(s.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc9_vectors() {
        let rows = [
            ("build done", Osc9::Notify("build done")),
            ("4 tests failed", Osc9::Notify("4 tests failed")),
            (
                "4;3;",
                Osc9::Progress {
                    state: 3,
                    pct: None,
                },
            ),
            (
                "4;1;150",
                Osc9::Progress {
                    state: 1,
                    pct: Some(100),
                },
            ),
            (
                "4;0",
                Osc9::Progress {
                    state: 0,
                    pct: None,
                },
            ),
            ("4;7", Osc9::Ignore),
            ("4", Osc9::Ignore),
            ("9;\"C:\\x\"", Osc9::Cwd("C:\\x")),
            ("9;C:\\y", Osc9::Cwd("C:\\y")),
            ("12", Osc9::PromptStart),
            ("6;Close(0)", Osc9::Ignore),
            ("7;calc.exe", Osc9::Ignore),
            ("42", Osc9::Ignore),
            ("", Osc9::Ignore),
        ];
        for (body, want) in rows {
            assert_eq!(classify_osc9(body), want, "{body:?}");
        }
    }

    #[test]
    fn prompt_marks() {
        assert_eq!(prompt_mark("A", "1"), Some(PromptMark::A { blitz: false }));
        assert_eq!(
            prompt_mark("A;blitz=1", "1"),
            Some(PromptMark::A { blitz: true })
        );
        assert_eq!(
            prompt_mark("A;redraw=0", "1"),
            Some(PromptMark::A { blitz: false })
        );
        assert_eq!(
            prompt_mark("A;aid=7;blitz=1", "1"),
            Some(PromptMark::A { blitz: true })
        );
        assert_eq!(
            prompt_mark("A;blitz=10", "1"),
            Some(PromptMark::A { blitz: false })
        );
        assert_eq!(prompt_mark("B", "1"), Some(PromptMark::B));
        assert_eq!(prompt_mark("C", "1"), Some(PromptMark::C));
        assert_eq!(prompt_mark("D", "1"), Some(PromptMark::D(None)));
        assert_eq!(prompt_mark("D;0", "1"), Some(PromptMark::D(Some(0))));
        assert_eq!(
            prompt_mark("D;-1;aid=3", "1"),
            Some(PromptMark::D(Some(-1)))
        );
        assert_eq!(prompt_mark("D;x", "1"), Some(PromptMark::D(None)));
        assert_eq!(
            prompt_mark("A;blitz=7f3a", "7f3a"),
            Some(PromptMark::A { blitz: true })
        );
        // Only the host's token marks blitz's own prompt.
        assert_eq!(
            prompt_mark("A;blitz=1", "7f3a"),
            Some(PromptMark::A { blitz: false })
        );
        assert_eq!(
            prompt_mark("A;blitz=", ""),
            Some(PromptMark::A { blitz: false })
        );
        assert_eq!(prompt_mark("Z", "1"), None);
        assert_eq!(prompt_mark("", "1"), None);
    }

    #[test]
    fn file_urls() {
        let p = |u: &str| file_url_path(u);
        assert_eq!(p("file:///C:/x").as_deref(), Some("C:\\x"));
        assert_eq!(
            p("file:///C:/Users/me/My%20Dir/a%23b").as_deref(),
            Some("C:\\Users\\me\\My Dir\\a#b")
        );
        assert_eq!(p("file://localhost/D:/").as_deref(), Some("D:\\"));
        assert_eq!(
            p("file:///home/me/%E2%9C%93").as_deref(),
            Some("/home/me/✓")
        );
        assert_eq!(p("file:///bad%zz%4").as_deref(), Some("/bad%zz%4"));
        if cfg!(windows) {
            assert_eq!(p("file://srv/share/x%20y"), None);
        }
        assert_eq!(p("http://x/y"), None);
        assert_eq!(p("file://host-only"), None);
    }

    #[test]
    fn local_dirs() {
        assert!(local_dir(r"C:\Users\me"));
        assert!(local_dir("d:/x"));
        assert!(local_dir(r"C:\"));
        assert_eq!(local_dir("/home/me"), !cfg!(windows));
        for p in [
            r"\\srv\share",
            "//srv/share",
            r"\\?\UNC\srv\share",
            r"\??\UNC\srv\share",
            r"\\.\pipe\x",
            "C:x",
            r"..\x",
            "",
            "C:\\a\u{1b}b",
        ] {
            assert!(!local_dir(p), "{p:?}");
        }
        assert!(local_dir(&format!("C:{}", r"\a".repeat(2000))));
        assert!(!local_dir(&format!("C:{}", r"\a".repeat(3000))));
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color("#fff"), Some(0xFFFFFF));
        assert_eq!(parse_color("#123456"), Some(0x123456));
        assert_eq!(parse_color("#123456789"), Some(0x124578));
        assert_eq!(parse_color("rgb:12/34/56"), Some(0x123456));
        assert_eq!(parse_color("rgb:ffff/8080/0000"), Some(0xFF8000));
        assert_eq!(parse_color("rgb:f/0/8"), Some(0xFF0088));
        assert_eq!(parse_color("rgb:1/2"), None);
        assert_eq!(parse_color("rgb:1/2/3/4"), None);
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("#ééé"), None);
        assert_eq!(parse_color("red"), None);
        let mut out = Vec::new();
        color_reply(11, 0x131417, true, &mut out);
        color_reply(10, 0xD6D7D9, false, &mut out);
        assert_eq!(
            out,
            b"\x1b]11;rgb:1313/1414/1717\x07\x1b]10;rgb:d6d6/d7d7/d9d9\x1b\\"
        );
    }

    #[test]
    fn clean_drops_controls_and_caps() {
        assert_eq!(clean("a\u{9b}b\u{85}c", 10), "abc");
        assert_eq!(clean("ééééé", 3), "ééé");
    }
}
