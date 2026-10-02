//! Byte-stream parser after Paul Williams' DEC ANSI state machine
//! (<https://vt100.net/emu/dec_ansi_parser>), UTF-8 native, 7-bit C1 only.

/// Receives everything the parser recognises, in stream order.
pub trait Handler {
    /// A run of printable text with no control characters.
    fn print(&mut self, s: &str);
    /// A C0 control (BEL, BS, HT, LF, VT, FF, CR, SO, SI, ...).
    fn execute(&mut self, c0: u8);
    /// `ESC <inter> <fin>`.
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

/// Incremental parser. Sequences may be split across `advance` calls at any
/// byte boundary.
#[derive(Debug, Default)]
pub struct Parser {}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds `bytes`, calling `h` for everything recognised.
    pub fn advance<H: Handler>(&mut self, _h: &mut H, _bytes: &[u8]) {}
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
}
