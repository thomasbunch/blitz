//! Turns terminal snapshots and window chrome into quads and draws them.

pub mod atlas;
pub mod builtin;
#[cfg(windows)]
pub mod d3d11;
#[cfg(windows)]
pub mod font;

/// `blitz debug render`: runs a script headlessly, renders the final screen
/// offscreen and writes it to a BMP. Returns the process exit code.
pub fn debug_render(_args: &[String]) -> i32 {
    eprintln!("blitz debug render: not available in this build");
    2
}
