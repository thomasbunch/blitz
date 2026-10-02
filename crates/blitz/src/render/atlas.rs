//! Glyph atlas: a shelf packer over one texture, reset when full.

use std::collections::HashMap;

/// What a glyph is rasterized from: the cluster text, its style and the
/// number of cells it spans.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub text: [u8; 16],
    pub len: u8,
    /// Bit 0 bold, bit 1 italic.
    pub style: u8,
    pub width: u8,
}

/// Where a glyph lives in the atlas and where it goes relative to the
/// top-left corner of its first cell. A zero size means nothing to draw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Slot {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub dx: i16,
    pub dy: i16,
}

#[derive(Clone, Copy, Debug)]
struct Shelf {
    y: u16,
    h: u16,
    /// Next free column.
    x: u16,
}

/// Packs rectangles into horizontal shelves of a fixed-size texture, and
/// remembers which glyph went where.
pub struct Atlas {
    w: u16,
    h: u16,
    shelves: Vec<Shelf>,
    glyphs: HashMap<GlyphKey, Slot>,
}

impl Atlas {
    pub fn new(w: u16, h: u16) -> Self {
        Self {
            w,
            h,
            shelves: Vec::new(),
            glyphs: HashMap::new(),
        }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.w, self.h)
    }

    pub fn get(&self, key: &GlyphKey) -> Option<Slot> {
        self.glyphs.get(key).copied()
    }

    /// Records an empty glyph (a space, or one with no ink).
    pub fn insert_empty(&mut self, key: GlyphKey) {
        self.glyphs.insert(key, Slot::default());
    }

    /// Reserves a `w`×`h` area for `key`. Returns `None` when the atlas is
    /// full; the caller then clears it and starts the frame over.
    pub fn insert(&mut self, key: GlyphKey, w: u16, h: u16, dx: i16, dy: i16) -> Option<Slot> {
        let (x, y) = self.alloc(w, h)?;
        let slot = Slot { x, y, w, h, dx, dy };
        self.glyphs.insert(key, slot);
        Some(slot)
    }

    pub fn clear(&mut self) {
        self.shelves.clear();
        self.glyphs.clear();
    }

    /// Best-fit shelf: the lowest shelf that is tall enough and has room,
    /// else a new shelf below the last one. New shelves are rounded up to a
    /// multiple of 8 px so glyphs of similar height share them.
    fn alloc(&mut self, w: u16, h: u16) -> Option<(u16, u16)> {
        if w > self.w || h > self.h {
            return None;
        }
        let best = self
            .shelves
            .iter_mut()
            .filter(|s| s.h >= h && self.w - s.x >= w)
            .min_by_key(|s| s.h);
        if let Some(s) = best {
            let x = s.x;
            s.x += w;
            return Some((x, s.y));
        }
        let y = self.shelves.last().map_or(0, |s| s.y + s.h);
        if self.h - y < h {
            return None;
        }
        let h = h.next_multiple_of(8).min(self.h - y);
        self.shelves.push(Shelf { y, h, x: w });
        Some((0, y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> GlyphKey {
        GlyphKey {
            text: [n; 16],
            len: 1,
            style: 0,
            width: 1,
        }
    }

    fn overlaps(a: Slot, b: Slot) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    #[test]
    fn packs_without_overlap_until_full() {
        let mut atlas = Atlas::new(64, 64);
        let mut slots = Vec::new();
        for n in 0..=255u8 {
            let h = 8 + u16::from(n % 9);
            match atlas.insert(key(n), 10, h, 0, 0) {
                Some(s) => slots.push(s),
                None => break,
            }
        }
        assert!(slots.len() >= 12, "only {} glyphs fit", slots.len());
        for (i, a) in slots.iter().enumerate() {
            assert!(a.x + a.w <= 64 && a.y + a.h <= 64);
            for b in &slots[i + 1..] {
                assert!(!overlaps(*a, *b), "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn reuses_shelves_and_resets() {
        let mut atlas = Atlas::new(32, 32);
        let a = atlas.insert(key(1), 16, 16, 1, -12).expect("fits");
        let b = atlas.insert(key(2), 16, 10, 0, 0).expect("fits");
        assert_eq!(
            (b.x, b.y),
            (16, 0),
            "a shorter glyph shares the first shelf"
        );
        assert_eq!(atlas.get(&key(1)), Some(a));
        assert!(atlas.insert(key(3), 33, 1, 0, 0).is_none());
        atlas.insert(key(4), 32, 16, 0, 0).expect("second shelf");
        assert!(atlas.insert(key(5), 1, 1, 0, 0).is_none(), "full");
        atlas.clear();
        assert_eq!(atlas.get(&key(1)), None);
        assert!(atlas.insert(key(5), 32, 32, 0, 0).is_some());
    }
}
