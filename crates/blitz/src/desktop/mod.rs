//! What the app asks of the desktop it runs on: system settings, the
//! taskbar or Dock, power, and the window frame.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;
#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::*;
