//! The blitz terminal application.

pub mod attention;
pub mod config;
pub mod hook;
pub mod keymap;
pub mod layout;
pub mod render;
pub mod session;
pub mod shell;
pub mod theme;

#[cfg(windows)]
pub mod app;
#[cfg(windows)]
pub mod clipboard;
#[cfg(windows)]
pub mod debug;
#[cfg(windows)]
pub mod handoff;
#[cfg(windows)]
pub mod pane;
#[cfg(windows)]
pub mod pty;
#[cfg(windows)]
pub mod update;
