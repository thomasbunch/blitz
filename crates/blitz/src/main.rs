// Release builds use the GUI subsystem so launching blitz opens no console.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
const USAGE: &str = "usage: blitz [DIR | --cwd DIR] [--new-window] [--cmd CMD]
       blitz --version | --help
       blitz setup claude [--wsl]
       blitz setup shell bash|zsh
       blitz debug run --script FILE [--cmd CMD] [--cwd DIR] [--cols N --rows N]
             [--timeout MS] [--trace FILE] [--setenv K=V]...
       blitz debug render (--vt FILE | --text FILE | --demo) --bmp OUT
             [--cols N --rows N] [--px N] [--warp] [--light] [--theme NAME]
             [--collapsed] [--banner TEXT] [--picker FILTER] [--settings N]
             [--find TEXT]
for tests: blitz [--selftest FILE] [--capture BMP] [--exit-after MS] ...";

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
    // Prints go to the console blitz was started from, if any.
    // SAFETY: plain Win32 call; failure just means there is no console.
    let attach = || unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let gui = !console(&a);
    if !gui {
        let _ = attach();
    }

    let code = match a.as_slice() {
        _ if gui => blitz::app::run(&args).unwrap_or_else(|e| {
            let _ = attach();
            eprintln!("blitz: {e}\n{USAGE}");
            2
        }),
        ["--version"] => {
            println!("blitz {}", env!("CARGO_PKG_VERSION"));
            0
        }
        ["--help" | "-h" | "/?"] => {
            println!("{USAGE}");
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

/// Whether `args` ask for something said on the console rather than a
/// window. `setup` or `debug` alone opens a folder of that name, as
/// `blitz DIR` does, unless no such folder is there.
#[cfg(windows)]
fn console(args: &[&str]) -> bool {
    match args {
        ["--version" | "--help" | "-h" | "/?", ..] => true,
        [tool @ ("setup" | "debug")] => !std::path::Path::new(tool).is_dir(),
        ["setup" | "debug", ..] => true,
        _ => false,
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("blitz: only Windows is supported for now");
    std::process::exit(1);
}

#[cfg(all(test, windows))]
mod tests {
    /// The only test in this binary: it changes the current folder.
    #[test]
    fn a_folder_named_like_a_command_opens() {
        let dir = std::env::temp_dir().join(format!("blitz-main-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("setup")).unwrap();
        std::env::set_current_dir(&dir).unwrap();
        let (setup, debug) = (super::console(&["setup"]), super::console(&["debug"]));
        std::env::set_current_dir(std::env::temp_dir()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!setup);
        assert!(debug);
        assert!(super::console(&["setup", "claude"]));
        assert!(super::console(&["--version"]));
        assert!(!super::console(&["."]));
    }
}
