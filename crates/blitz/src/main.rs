// Release builds use the GUI subsystem so launching blitz opens no console.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
const USAGE: &str = "usage: blitz [--version]
       blitz setup claude
       blitz debug run --script FILE [--cmd CMD] [--cwd DIR] [--cols N --rows N] [--timeout MS]
       blitz debug render --script FILE --bmp OUT [--warp]";

#[cfg(windows)]
fn main() {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    use windows::Win32::System::LibraryLoader::{
        LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories,
    };

    // Resolve DLLs only from System32 and our own directory, never from the
    // current directory or PATH.
    // SAFETY: plain Win32 call with valid flags.
    let _ = unsafe {
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_APPLICATION_DIR)
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let gui = a
        .first()
        .is_none_or(|f| f.starts_with("--") && *f != "--version");
    if !gui {
        // Subcommands print to the console they were started from.
        // SAFETY: plain Win32 call; failure just means there is no console.
        let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }

    let code = match a.as_slice() {
        _ if gui => blitz::app::run(&args),
        ["--version"] => {
            println!("blitz {}", env!("CARGO_PKG_VERSION"));
            0
        }
        ["setup", ..] => blitz::hook::setup(&args[1..]),
        ["debug", "run", ..] => blitz::debug::run(&args[2..]),
        ["debug", "render", ..] => blitz::render::debug_render(&args[2..]),
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };
    std::process::exit(code);
}

#[cfg(not(windows))]
fn main() {
    eprintln!("blitz: only Windows is supported for now");
    std::process::exit(1);
}
