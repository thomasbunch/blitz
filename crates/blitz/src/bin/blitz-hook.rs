//! Claude Code hook command. A console program, so the hook runner can read
//! its stdout without a window flashing up.

fn main() {
    std::process::exit(blitz::hook::run());
}
