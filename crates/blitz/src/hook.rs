//! `blitz-hook claude` reports Claude Code's state to the pane it runs in;
//! `blitz setup claude` prints the hook settings to install.

use std::fmt::Write as _;

/// Entry point of `blitz-hook`. Always returns 0, so a hook can never block
/// Claude Code.
pub fn run() -> i32 {
    0
}

/// `blitz setup <app>`. Returns the process exit code.
pub fn setup(_args: &[String]) -> i32 {
    eprintln!("blitz setup: not available in this build");
    2
}

/// A parsed JSON value. Objects keep their keys in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

/// Deeper input is rejected rather than risking the stack.
const MAX_DEPTH: usize = 128;

impl Json {
    /// Parses one JSON document. Returns `None` on malformed input.
    pub fn parse(s: &str) -> Option<Json> {
        let mut p = Parser { s, i: 0 };
        let v = p.value(0)?;
        p.ws();
        (p.i == s.len()).then_some(v)
    }

    /// The value under `key` when this is an object. With duplicate keys the
    /// last one wins, as in JavaScript.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
}

struct Parser<'a> {
    s: &'a str,
    /// Byte offset; always on a char boundary.
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.i).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    /// Skips whitespace, then consumes `b` if it comes next.
    fn eat(&mut self, b: u8) -> bool {
        self.ws();
        let hit = self.peek() == Some(b);
        if hit {
            self.i += 1;
        }
        hit
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.ws();
        match self.peek()? {
            b'{' => {
                self.i += 1;
                let mut m = Vec::new();
                if !self.eat(b'}') {
                    loop {
                        self.ws();
                        if self.peek() != Some(b'"') {
                            return None;
                        }
                        let k = self.string()?;
                        if !self.eat(b':') {
                            return None;
                        }
                        m.push((k, self.value(depth + 1)?));
                        if self.eat(b'}') {
                            break;
                        }
                        if !self.eat(b',') {
                            return None;
                        }
                    }
                }
                Some(Json::Obj(m))
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                if !self.eat(b']') {
                    loop {
                        a.push(self.value(depth + 1)?);
                        if self.eat(b']') {
                            break;
                        }
                        if !self.eat(b',') {
                            return None;
                        }
                    }
                }
                Some(Json::Arr(a))
            }
            b'"' => self.string().map(Json::Str),
            b't' => self.word("true", Json::Bool(true)),
            b'f' => self.word("false", Json::Bool(false)),
            b'n' => self.word("null", Json::Null),
            _ => self.number(),
        }
    }

    fn word(&mut self, w: &str, v: Json) -> Option<Json> {
        self.s[self.i..].starts_with(w).then(|| {
            self.i += w.len();
            v
        })
    }

    fn number(&mut self) -> Option<Json> {
        let start = self.i;
        while matches!(
            self.peek(),
            Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
        ) {
            self.i += 1;
        }
        self.s[start..self.i].parse().ok().map(Json::Num)
    }

    /// Called on the opening quote.
    fn string(&mut self) -> Option<String> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let rest = &self.s[self.i..];
            let n = rest.find(['"', '\\'])?;
            out.push_str(&rest[..n]);
            self.i += n + 1;
            if rest.as_bytes()[n] == b'"' {
                return Some(out);
            }
            let esc = self.peek()?;
            self.i += 1;
            out.push(match esc {
                b'"' => '"',
                b'\\' => '\\',
                b'/' => '/',
                b'b' => '\u{8}',
                b'f' => '\u{c}',
                b'n' => '\n',
                b'r' => '\r',
                b't' => '\t',
                b'u' => self.unicode_escape()?,
                // Also covers a non-ASCII byte, so `i` never stops inside
                // a char.
                _ => return None,
            });
        }
    }

    /// The `XXXX` of `\uXXXX`, joining a surrogate pair when the low half
    /// follows. A lone surrogate becomes U+FFFD.
    fn unicode_escape(&mut self) -> Option<char> {
        let hi = self.hex4()?;
        if (0xD800..0xDC00).contains(&hi) && self.s[self.i..].starts_with("\\u") {
            let save = self.i;
            self.i += 2;
            let lo = self.hex4()?;
            if (0xDC00..0xE000).contains(&lo) {
                return char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00));
            }
            self.i = save;
        }
        Some(char::from_u32(hi).unwrap_or('\u{FFFD}'))
    }

    fn hex4(&mut self) -> Option<u32> {
        let h = self.s.get(self.i..self.i + 4)?;
        if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        self.i += 4;
        u32::from_str_radix(h, 16).ok()
    }
}

/// Appends `s` to `out` as the inside of a JSON string literal.
pub fn escape_json(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_values() {
        let v = Json::parse(
            r#" {"a": [1, -2.5e3, true, false, null], "b": {"c": "d"}, "e": [], "f": {}} "#,
        )
        .unwrap();
        assert_eq!(
            v.get("a"),
            Some(&Json::Arr(vec![
                Json::Num(1.0),
                Json::Num(-2500.0),
                Json::Bool(true),
                Json::Bool(false),
                Json::Null,
            ]))
        );
        assert_eq!(
            v.get("b").and_then(|b| b.get("c")),
            Some(&Json::Str("d".into()))
        );
        assert_eq!(v.get("e"), Some(&Json::Arr(vec![])));
        assert_eq!(v.get("f"), Some(&Json::Obj(vec![])));
        assert_eq!(v.get("zz"), None);
        assert_eq!(Json::parse("\"x\"").unwrap().get("x"), None);
    }

    #[test]
    fn json_string_escapes() {
        let v = Json::parse(r#""q\" s\\ /\/ \b\f\n\r\t \u00e9 \ud83d\ude00 \ud800x é""#).unwrap();
        assert_eq!(
            v.as_str(),
            Some("q\" s\\ // \u{8}\u{c}\n\r\t é 😀 \u{FFFD}x é")
        );
    }

    #[test]
    fn json_last_duplicate_key_wins() {
        let v = Json::parse(r#"{"k": 1, "k": 2}"#).unwrap();
        assert_eq!(v.get("k"), Some(&Json::Num(2.0)));
    }

    #[test]
    fn json_rejects_malformed() {
        for bad in [
            "",
            " ",
            "{",
            "}",
            "[1,]",
            "[1 2]",
            "{\"a\"}",
            "{\"a\":}",
            "{a:1}",
            "\"open",
            "\"bad \\q\"",
            "\"\\u12\"",
            "\"\\u12G4\"",
            "tru",
            "nul",
            "1 2",
            "-",
            "[\"\\é\"]",
        ] {
            assert_eq!(Json::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn json_depth_is_capped() {
        let ok = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert!(Json::parse(&ok).is_some());
        let deep = "[".repeat(100_000);
        assert_eq!(Json::parse(&deep), None);
    }

    #[test]
    fn json_never_panics_on_prefixes() {
        let doc = r#"{"hook_event_name":"Stop","x":[1,{"y":"\u00e9\ud83d\ude00 é\n"}],"z":null}"#;
        for (i, _) in doc.char_indices() {
            let _ = Json::parse(&doc[..i]);
            let _ = Json::parse(&doc[i..]);
        }
        assert!(Json::parse(doc).is_some());
    }

    #[test]
    fn escape_round_trips() {
        let s = "a\"b\\c\x1b]777;\x07\n\u{9b}é😀";
        let mut lit = String::from("\"");
        escape_json(s, &mut lit);
        lit.push('"');
        assert!(lit.contains("\\u001b") && lit.contains("\\u0007") && lit.contains("\\u009b"));
        assert_eq!(Json::parse(&lit).unwrap().as_str(), Some(s));
    }
}
