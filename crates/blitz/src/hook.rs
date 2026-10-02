//! `blitz-hook claude` reports Claude Code's state to the pane it runs in;
//! `blitz setup claude` prints the hook settings to install.

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
