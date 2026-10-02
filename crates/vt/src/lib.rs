//! Terminal emulation core: VT parser, screen model and input encoders.
//!
//! This crate has no dependencies outside `std`.

pub mod grid;
pub mod keys;
pub mod modes;
pub mod osc;
pub mod parser;
pub mod reflow;
pub mod snapshot;
pub mod style;
pub mod terminal;
pub mod width;

pub use keys::{
    Key, KeyInput, Locks, Mods, MouseEv, MouseKind, encode_focus, encode_key, encode_mouse,
    encode_paste,
};
pub use modes::{InputModes, MouseMode};
pub use snapshot::{CursorShape, Palette, RenderCell, Snapshot};
pub use terminal::{Event, Options, PromptMark, Terminal};
pub use width::cluster_width;
