//! Headless tools: a script runner over a real pseudoconsole, and the GUI
//! self-test.

/// `blitz debug run`: drives a session from a script. Returns the process
/// exit code.
pub fn run(_args: &[String]) -> i32 {
    eprintln!("blitz debug run: not available in this build");
    2
}
