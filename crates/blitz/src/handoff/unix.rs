//! Hand-offs through a Unix socket in a folder only this user can read.
//! A launch sends the folder it was given, or nothing, and the main
//! window answers with one byte: 1 when it took the launch.
//!
//! Only this user's programs can reach the socket, and it only ever
//! carries a folder, never a command.

use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use winit::event_loop::EventLoopProxy;

use super::Ask;
use crate::app::UserEvent;

/// The largest folder taken, in bytes; as OSC 7's limit.
const MAX_BYTES: usize = 4096;
/// How long a running blitz gets to answer before this one opens its own
/// window.
const TIMEOUT: Duration = Duration::from_secs(2);

/// The socket the main window listens on. Debug builds use their own, so
/// `cargo run` never sends to the installed blitz.
fn socket() -> Option<PathBuf> {
    let name = if cfg!(debug_assertions) {
        "blitz-dev.sock"
    } else {
        "blitz.sock"
    };
    // The user's own runtime folder on Linux; on macOS the temporary
    // folder is the user's own already.
    let dir = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !cfg!(target_os = "macos") => PathBuf::from(d),
        _ => std::env::temp_dir(),
    };
    let private = std::fs::metadata(&dir).is_ok_and(|m| m.permissions().mode() & 0o077 == 0);
    // A temporary folder others can write to gets a folder of this
    // user's own.
    if private {
        return Some(dir.join(name));
    }
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let own = dir.join(format!("blitz-{uid}"));
    let made = std::fs::DirBuilder::new().mode(0o700).create(&own);
    // One made before, by this user alone; never a link someone left.
    let ok = made.is_ok()
        || std::fs::symlink_metadata(&own)
            .is_ok_and(|m| m.is_dir() && m.uid() == uid && m.mode() & 0o077 == 0);
    ok.then(|| own.join(name))
}

/// What a message asks, or `None` when its folder is not a local one.
fn decode(bytes: &[u8]) -> Option<Ask> {
    if bytes.is_empty() {
        return Some(Ask::Show);
    }
    let text = std::str::from_utf8(bytes).ok()?;
    // The same rule as a folder a program reports with OSC 7.
    (bytes.len() <= MAX_BYTES && vt::osc::local_dir(text)).then(|| Ask::Open(text.into()))
}

/// The socket this launch took in [`send`], as no blitz was listening, for
/// [`install`] to listen on once the window is up.
static CLAIMED: std::sync::Mutex<Option<UnixListener>> = std::sync::Mutex::new(None);

/// A connection to the blitz listening at `path`, or `None` when there is
/// none, after this launch took the socket. Taking it first means two
/// launches at once, as a double click starts, cannot both become the
/// main window: the second finds the first's socket.
fn connect_or_claim(path: &Path) -> Option<UnixStream> {
    for _ in 0..2 {
        match UnixListener::bind(path) {
            Ok(l) => {
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
                *CLAIMED
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(l);
                return None;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                match UnixStream::connect(path) {
                    Ok(s) => return Some(s),
                    // Left by a blitz that is gone.
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                        let _ = std::fs::remove_file(path);
                    }
                    Err(_) => return None,
                }
            }
            Err(_) => return None,
        }
    }
    None
}

/// Sends this launch, and `dir` when it names one, to the running blitz.
/// `None` when blitz is not running, else whether it took the launch.
pub fn send(dir: Option<&Path>) -> Option<bool> {
    let mut s = connect_or_claim(&socket()?)?;
    let _ = s.set_read_timeout(Some(TIMEOUT));
    let _ = s.set_write_timeout(Some(TIMEOUT));
    // The receiver's directory is not ours.
    let bytes = match dir.map(std::path::absolute) {
        Some(Ok(d)) => d.into_os_string().into_encoded_bytes(),
        Some(Err(_)) => return Some(false),
        None => Vec::new(),
    };
    if bytes.len() > MAX_BYTES {
        return Some(false);
    }
    let sent = s.write_all(&bytes).is_ok() && s.shutdown(std::net::Shutdown::Write).is_ok();
    let mut took = [0u8];
    Some(sent && s.read_exact(&mut took).is_ok() && took[0] == 1)
}

/// How many blitz windows other processes have open. Only the Windows
/// installer needs to know.
pub fn others() -> usize {
    0
}

/// Whether the press being handled is the click that brought the window
/// to the front. The desktop does not say here, so none is.
pub fn take_activating_click() -> bool {
    false
}

pub fn forget_activating_click() {}

/// With `launches`, listens for other launches: each arrives as
/// `UserEvent::Handoff`. A socket left by a blitz that is gone is taken
/// over.
pub fn install(_hwnd: isize, proxy: EventLoopProxy<UserEvent>, launches: bool) {
    let claimed = CLAIMED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    let Some(path) = socket().filter(|_| launches) else {
        return;
    };
    // `send` took the socket when no blitz answered there.
    let Some(listener) = claimed else {
        eprintln!(
            "blitz: cannot listen for other launches at {}",
            path.display()
        );
        return;
    };
    std::thread::spawn(move || {
        for s in listener.incoming() {
            let Ok(mut s) = s else {
                continue;
            };
            let _ = s.set_read_timeout(Some(TIMEOUT));
            let mut bytes = Vec::new();
            let read = (&mut s).take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes);
            let took = read.is_ok()
                && (decode(&bytes))
                    .filter(|a| !matches!(a, Ask::Open(d) if !d.is_dir()))
                    .is_some_and(|a| proxy.send_event(UserEvent::Handoff(a)).is_ok());
            let _ = s.write_all(&[u8::from(took)]);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_takes_only_local_folders() {
        assert_eq!(decode(b""), Some(Ask::Show));
        assert_eq!(
            decode(b"/home/jo/caf\xc3\xa9"),
            Some(Ask::Open("/home/jo/café".into()))
        );
        for bad in [&b"x"[..], b"../x", b"//host/x", b"/x\ny", b"\xff"] {
            assert_eq!(decode(bad), None, "{bad:?}");
        }
        let long = format!("/{}", "a".repeat(MAX_BYTES));
        assert_eq!(decode(long.as_bytes()), None);
    }

    #[test]
    fn handoff_the_first_launch_takes_the_socket() {
        let dir = std::env::temp_dir().join(format!("blitz-handoff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("t.sock");
        // A socket left by a blitz that is gone is taken over.
        drop(UnixListener::bind(&path).expect("stale"));
        assert!(connect_or_claim(&path).is_none(), "the first takes it");
        assert!(connect_or_claim(&path).is_some(), "the second finds it");
        let held = CLAIMED.lock().expect("lock").take();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(held.is_some());
    }

    #[test]
    fn handoff_socket_is_the_users_own() {
        let path = socket().expect("a socket path");
        let dir = path.parent().expect("a folder");
        let mode = std::fs::metadata(dir).expect("folder").permissions().mode();
        assert_eq!(mode & 0o077, 0, "{}", dir.display());
    }
}
