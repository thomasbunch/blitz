//! Byte-stream parser after Paul Williams' DEC ANSI state machine
//! (<https://vt100.net/emu/dec_ansi_parser>), UTF-8 native, 7-bit C1 only.
//!
//! Departures from the DEC diagram, shared with most modern terminals:
//! - Bytes 0x80..=0x9F are UTF-8 continuation bytes, never C1 controls.
//!   C1 code points that decode from UTF-8 (U+0080..=U+009F) are dropped.
//! - `:` separates sub-parameters (`4:3`, `38:2::R:G:B`) instead of making
//!   the sequence ignorable.
//! - OSC ends with BEL or ST; the handler learns which.
//! - DEL is ignored everywhere, including ground.
//! - More than [`MAX_PARAMS`] parameters, more than 4 intermediates or an
//!   OSC longer than [`MAX_OSC`] turns the sequence into a no-op.

/// Receives everything the parser recognises, in stream order.
pub trait Handler {
    /// A run of printable text with no control characters.
    fn print(&mut self, s: &str);
    /// A C0 control (BEL, BS, HT, LF, VT, FF, CR, SO, SI, ...).
    fn execute(&mut self, c0: u8);
    /// `ESC <inter> <fin>`. A bare `ESC \` (ST) is consumed by the parser.
    fn esc(&mut self, inter: &[u8], fin: u8);
    /// `CSI <params> <inter> <fin>`. A private marker (`?`, `>`, `<`, `=`)
    /// arrives as `inter[0]`.
    fn csi(&mut self, p: &Params, inter: &[u8], fin: u8);
    /// A complete OSC payload, unsplit. `bel_terminated` tells the handler
    /// which terminator to use for a reply.
    fn osc(&mut self, data: &[u8], bel_terminated: bool);
    /// Start of a DCS string.
    fn dcs_hook(&mut self, p: &Params, inter: &[u8], fin: u8);
    /// Part of a DCS body. May be called any number of times.
    fn dcs_put(&mut self, chunk: &[u8]);
    /// End of a DCS string.
    fn dcs_unhook(&mut self);
}

/// Maximum number of parameters in one sequence. More makes it a no-op.
pub const MAX_PARAMS: usize = 32;

/// Maximum OSC payload in bytes. A longer one is dropped whole.
pub const MAX_OSC: usize = 1 << 20;

const MAX_INTER: usize = 4;

/// OSC buffers that grew past this are freed after dispatch rather than kept.
const OSC_KEEP: usize = 64 << 10;

const REPLACEMENT: &str = "\u{FFFD}";

const CAN: u8 = 0x18;
const SUB: u8 = 0x1A;
const ESC: u8 = 0x1B;
const DEL: u8 = 0x7F;
const BEL: u8 = 0x07;

/// CSI/DCS parameters: up to 32 saturating `u16` values, with a record of
/// which ones followed a `:` (as in `38:2::R:G:B` or `4:3`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Params {
    vals: [u16; MAX_PARAMS],
    /// Bit `i` set: parameter `i + 1` is a sub-parameter of the one before.
    colon_after: u32,
    len: u8,
}

impl Params {
    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_slice(&self) -> &[u16] {
        &self.vals[..self.len as usize]
    }

    /// Parameter `i`, or 0 (the "default" value) when absent.
    pub fn get(&self, i: usize) -> u16 {
        self.as_slice().get(i).copied().unwrap_or(0)
    }

    /// Whether parameter `i` was separated from the previous one by `:`.
    pub fn is_sub(&self, i: usize) -> bool {
        i > 0 && i < self.len() && self.colon_after & (1 << (i - 1)) != 0
    }

    /// Appends a value. Returns false, leaving `self` unchanged, when full.
    pub fn push(&mut self, v: u16, after_colon: bool) -> bool {
        let n = self.len();
        if n == MAX_PARAMS {
            return false;
        }
        if after_colon && n > 0 {
            self.colon_after |= 1 << (n - 1);
        }
        self.vals[n] = v;
        self.len += 1;
        true
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    EscInter,
    CsiEntry,
    CsiParam,
    CsiInter,
    CsiIgnore,
    DcsEntry,
    DcsParam,
    DcsInter,
    DcsPass,
    DcsIgnore,
    Osc,
    /// SOS, PM and APC bodies: read up to ST and thrown away.
    SosPmApc,
}

/// Incremental parser. Sequences may be split across `advance` calls at any
/// byte boundary, including inside a UTF-8 character.
#[derive(Debug, Default)]
pub struct Parser {
    state: State,
    params: Params,
    /// The parameter being read.
    cur: u16,
    /// `cur` followed a `:`.
    cur_sub: bool,
    /// A digit, `;` or `:` was seen, so `cur` is a parameter even if empty.
    in_param: bool,
    inter: [u8; MAX_INTER],
    n_inter: u8,
    /// Too many parameters or intermediates, or an oversized OSC.
    overflow: bool,
    osc: Vec<u8>,
    /// Start of a UTF-8 character cut off by the end of the last chunk.
    utf8: [u8; 4],
    utf8_len: u8,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds `bytes`, calling `h` for everything recognised.
    pub fn advance<H: Handler>(&mut self, h: &mut H, bytes: &[u8]) {
        let mut i = 0;
        while i < bytes.len() {
            i = match self.state {
                State::Ground => self.ground(h, bytes, i),
                State::Osc => self.osc_run(h, bytes, i),
                State::DcsPass | State::DcsIgnore | State::SosPmApc => self.string_run(h, bytes, i),
                _ => {
                    self.byte(h, bytes[i]);
                    i + 1
                }
            };
        }
    }

    /// Text up to the next control byte, then that byte. Returns the new index.
    fn ground<H: Handler>(&mut self, h: &mut H, bytes: &[u8], mut i: usize) -> usize {
        while self.utf8_len > 0 && i < bytes.len() {
            let n = self.utf8_len as usize;
            let mut buf = self.utf8;
            buf[n] = bytes[i];
            match std::str::from_utf8(&buf[..=n]) {
                Ok(s) => {
                    print_text(h, s);
                    self.utf8_len = 0;
                    i += 1;
                }
                Err(e) if e.error_len().is_none() => {
                    self.utf8 = buf;
                    self.utf8_len += 1;
                    i += 1;
                }
                Err(_) => {
                    // The carried bytes are one invalid sequence; this byte
                    // starts over.
                    h.print(REPLACEMENT);
                    self.utf8_len = 0;
                }
            }
        }
        let rest = &bytes[i..];
        let n = find_ctl(rest);
        if n > 0 {
            self.text(h, &rest[..n], n == rest.len());
        }
        if n < rest.len() {
            self.byte(h, rest[n]);
            return i + n + 1;
        }
        bytes.len()
    }

    /// Prints a run with no control bytes, replacing invalid UTF-8 with
    /// U+FFFD one maximal subpart at a time (as `String::from_utf8_lossy`
    /// does). An incomplete character at the end of the chunk is carried.
    fn text<H: Handler>(&mut self, h: &mut H, mut run: &[u8], chunk_end: bool) {
        loop {
            match std::str::from_utf8(run) {
                Ok(s) => return print_text(h, s),
                Err(e) => {
                    let (good, bad) = run.split_at(e.valid_up_to());
                    print_text(h, std::str::from_utf8(good).unwrap_or_default());
                    match e.error_len() {
                        Some(n) => {
                            h.print(REPLACEMENT);
                            run = &bad[n..];
                        }
                        None if chunk_end => {
                            self.utf8[..bad.len()].copy_from_slice(bad);
                            self.utf8_len = bad.len() as u8;
                            return;
                        }
                        None => return h.print(REPLACEMENT),
                    }
                }
            }
        }
    }

    fn osc_run<H: Handler>(&mut self, h: &mut H, bytes: &[u8], i: usize) -> usize {
        let rest = &bytes[i..];
        let n = find_ctl(rest);
        if self.osc.len() + n > MAX_OSC {
            self.overflow = true;
        }
        if !self.overflow {
            self.osc.extend_from_slice(&rest[..n]);
        }
        if n < rest.len() {
            self.byte(h, rest[n]);
            return i + n + 1;
        }
        bytes.len()
    }

    /// A DCS, SOS, PM or APC body up to the next byte that can end it. Only
    /// a hooked DCS passes it on.
    fn string_run<H: Handler>(&mut self, h: &mut H, bytes: &[u8], i: usize) -> usize {
        let rest = &bytes[i..];
        let n = rest
            .iter()
            .position(|&b| matches!(b, CAN | SUB | ESC | DEL))
            .unwrap_or(rest.len());
        if n > 0 && self.state == State::DcsPass {
            h.dcs_put(&rest[..n]);
        }
        if n < rest.len() {
            self.byte(h, rest[n]);
            return i + n + 1;
        }
        bytes.len()
    }

    /// One step of the state machine.
    fn byte<H: Handler>(&mut self, h: &mut H, b: u8) {
        // Transitions from anywhere.
        match b {
            CAN | SUB => {
                self.end_string(h, None);
                h.execute(b);
                self.state = State::Ground;
                return;
            }
            ESC => {
                self.end_string(h, Some(false));
                self.clear();
                self.state = State::Escape;
                return;
            }
            _ => {}
        }
        use State::*;
        match self.state {
            Ground => {
                if b < 0x20 {
                    h.execute(b);
                }
            }
            Escape => match b {
                0x00..=0x1F => h.execute(b),
                0x20..=0x2F => {
                    self.collect(b);
                    self.state = EscInter;
                }
                b'[' => self.state = CsiEntry,
                b'P' => self.state = DcsEntry,
                b']' => self.state = Osc,
                b'X' | b'^' | b'_' => self.state = SosPmApc,
                // ST. Whatever it terminated has already been dispatched.
                b'\\' => self.state = Ground,
                0x30..=0x7E => {
                    h.esc(&[], b);
                    self.state = Ground;
                }
                _ => {}
            },
            EscInter => match b {
                0x00..=0x1F => h.execute(b),
                0x20..=0x2F => self.collect(b),
                0x30..=0x7E => {
                    if !self.overflow {
                        h.esc(&self.inter[..self.n_inter as usize], b);
                    }
                    self.state = Ground;
                }
                _ => {}
            },
            CsiEntry | CsiParam | CsiInter => match (self.state, b) {
                (_, 0x00..=0x1F) => h.execute(b),
                (_, 0x40..=0x7E) => {
                    self.end_params();
                    if !self.overflow {
                        h.csi(&self.params, &self.inter[..self.n_inter as usize], b);
                    }
                    self.state = Ground;
                }
                (CsiEntry | CsiParam, 0x30..=0x3B) => {
                    self.param(b);
                    self.state = CsiParam;
                }
                (CsiEntry, 0x3C..=0x3F) => {
                    self.collect(b);
                    self.state = CsiParam;
                }
                (_, 0x20..=0x2F) => {
                    self.collect(b);
                    self.state = CsiInter;
                }
                (_, 0x30..=0x3F) => self.state = CsiIgnore,
                _ => {}
            },
            CsiIgnore => match b {
                0x00..=0x1F => h.execute(b),
                0x40..=0x7E => self.state = Ground,
                _ => {}
            },
            DcsEntry | DcsParam | DcsInter => match (self.state, b) {
                (_, 0x40..=0x7E) => {
                    self.end_params();
                    if self.overflow {
                        self.state = DcsIgnore;
                    } else {
                        h.dcs_hook(&self.params, &self.inter[..self.n_inter as usize], b);
                        self.state = DcsPass;
                    }
                }
                (DcsEntry | DcsParam, 0x30..=0x3B) => {
                    self.param(b);
                    self.state = DcsParam;
                }
                (DcsEntry, 0x3C..=0x3F) => {
                    self.collect(b);
                    self.state = DcsParam;
                }
                (_, 0x20..=0x2F) => {
                    self.collect(b);
                    self.state = DcsInter;
                }
                (_, 0x30..=0x3F) => self.state = DcsIgnore,
                _ => {}
            },
            Osc if b == BEL => {
                self.end_string(h, Some(true));
                self.state = Ground;
            }
            // String bodies arrive in bulk through `osc_run` and
            // `string_run`; the other controls they hand over are ignored.
            Osc | DcsPass | DcsIgnore | SosPmApc => {}
        }
    }

    /// Leaves an OSC or DCS string. `end` is `Some(bel_terminated)` when
    /// the string finished (BEL, or ESC starting ST) and `None` when CAN or
    /// SUB cancelled it, in which case an OSC is dropped.
    fn end_string<H: Handler>(&mut self, h: &mut H, end: Option<bool>) {
        match self.state {
            State::Osc => {
                if let Some(bel) = end
                    && !self.overflow
                {
                    h.osc(&self.osc, bel);
                }
                self.osc.clear();
                if self.osc.capacity() > OSC_KEEP {
                    self.osc = Vec::new();
                }
            }
            State::DcsPass => h.dcs_unhook(),
            _ => {}
        }
    }

    fn clear(&mut self) {
        self.params.clear();
        self.cur = 0;
        self.cur_sub = false;
        self.in_param = false;
        self.n_inter = 0;
        self.overflow = false;
    }

    fn collect(&mut self, b: u8) {
        match self.inter.get_mut(self.n_inter as usize) {
            Some(slot) => {
                *slot = b;
                self.n_inter += 1;
            }
            None => self.overflow = true,
        }
    }

    fn param(&mut self, b: u8) {
        self.in_param = true;
        if b.is_ascii_digit() {
            self.cur = self
                .cur
                .saturating_mul(10)
                .saturating_add(u16::from(b - b'0'));
        } else {
            if !self.params.push(self.cur, self.cur_sub) {
                self.overflow = true;
            }
            self.cur = 0;
            self.cur_sub = b == b':';
        }
    }

    fn end_params(&mut self) {
        if self.in_param && !self.params.push(self.cur, self.cur_sub) {
            self.overflow = true;
        }
    }
}

/// Prints valid text, dropping C1 controls (U+0080..=U+009F, which encode as
/// `C2 80..=C2 9F`).
fn print_text<H: Handler>(h: &mut H, s: &str) {
    let b = s.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while let Some(off) = b[i..].iter().position(|&x| x == 0xC2) {
        let j = i + off;
        // 0xC2 is always a lead byte here, so j + 1 is in bounds.
        if b[j + 1] < 0xA0 {
            if start < j {
                h.print(&s[start..j]);
            }
            start = j + 2;
        }
        i = j + 2;
    }
    if start < b.len() {
        h.print(&s[start..]);
    }
}

/// Index of the first C0 control (ESC included) or DEL in `b`, or `b.len()`.
/// Tests eight bytes per step with word arithmetic.
fn find_ctl(b: &[u8]) -> usize {
    const LO: u64 = 0x0101_0101_0101_0101;
    const HI: u64 = 0x8080_8080_8080_8080;
    let (words, tail) = b.as_chunks::<8>();
    for (k, w) in words.iter().enumerate() {
        let w = u64::from_le_bytes(*w);
        // High bit of each byte below 0x20, then of each byte equal to DEL.
        // A borrow only travels towards higher bytes, so the lowest flag in
        // either mask is exact, which is all trailing_zeros looks at.
        let lt = w.wrapping_sub(0x20 * LO) & !w & HI;
        let x = w ^ (u64::from(DEL) * LO);
        let del = x.wrapping_sub(LO) & !x & HI;
        let z = lt | del;
        if z != 0 {
            return k * 8 + (z.trailing_zeros() / 8) as usize;
        }
    }
    words.len() * 8
        + tail
            .iter()
            .position(|&x| x < 0x20 || x == DEL)
            .unwrap_or(tail.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_track_colons_and_capacity() {
        let mut p = Params::default();
        assert!(p.push(38, false));
        assert!(p.push(2, true));
        assert!(p.push(0, true));
        assert!(p.push(7, false));
        assert_eq!(p.as_slice(), &[38, 2, 0, 7]);
        assert!(!p.is_sub(0) && p.is_sub(1) && p.is_sub(2) && !p.is_sub(3));
        assert_eq!(p.get(9), 0);
        for _ in 4..MAX_PARAMS {
            assert!(p.push(1, false));
        }
        assert!(!p.push(1, true));
        assert_eq!(p.len(), MAX_PARAMS);
    }

    #[test]
    fn find_ctl_matches_a_bytewise_scan() {
        const BYTES: [u8; 14] = [
            0x00, 0x07, 0x1B, 0x1F, 0x7F, 0x20, 0x41, 0x7E, 0x80, 0x9F, 0xA0, 0xC2, 0xDF, 0xFF,
        ];
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut buf = [0u8; 40];
        for _ in 0..20_000 {
            for b in &mut buf {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                // Mostly printable, so matches land at every offset.
                *b = if seed.is_multiple_of(8) {
                    BYTES[(seed >> 8) as usize % BYTES.len()]
                } else {
                    BYTES[5 + (seed >> 8) as usize % 9]
                };
            }
            for start in 0..buf.len() {
                let s = &buf[start..];
                let want = s
                    .iter()
                    .position(|&x| x < 0x20 || x == DEL)
                    .unwrap_or(s.len());
                assert_eq!(find_ctl(s), want, "{s:02x?}");
            }
        }
    }
}
