//! Claude Code hook command. A console program, so the hook runner can read
//! its stdout without a window flashing up. `blitz-hook mcp` is the server
//! of Claude Code's pane tools.
//!
//! blitz copies it into its plugin folder as `blitz`, which panes find on
//! their PATH: by that name it runs `blitz pane`, waited for in any shell,
//! as blitz.exe, a windowed program on Windows, is not.

fn main() {
    let name = std::env::current_exe().ok().and_then(|p| {
        let stem = p.file_stem()?.to_string_lossy().to_ascii_lowercase();
        Some(stem)
    });
    let code = match name.as_deref() {
        Some("blitz") => {
            let args: Vec<String> = std::env::args().skip(1).collect();
            match args.split_first() {
                Some((pane, rest)) if pane == "pane" => blitz::control::cli(rest),
                _ => {
                    eprintln!("blitz: this copy only runs \"blitz pane\"");
                    2
                }
            }
        }
        _ if std::env::args().nth(1).as_deref() == Some("mcp") => blitz::control::mcp::run(),
        _ => blitz::hook::run(),
    };
    std::process::exit(code);
}
