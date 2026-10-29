//! The channel as a Unix socket in a folder only this user can reach.

use std::io;
use std::net::Shutdown;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

/// Listens on a new socket of this process's own and answers each request
/// with `answer`, on a thread per connection. Returns the socket's path,
/// short enough for macOS's limit of 104 bytes.
pub fn listen(answer: impl Fn(String) -> String + Clone + Send + 'static) -> io::Result<String> {
    let dir = crate::handoff::private_dir()
        .ok_or_else(|| io::Error::other("no folder of this user's own"))?;
    // Sockets a blitz that stopped without tidying up left: nothing
    // listens there. ponytail: macOS also refuses while a live blitz's
    // queue of callers is full, which costs that blitz its socket.
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let gone = |e: io::Error| e.kind() == io::ErrorKind::ConnectionRefused;
        if e.file_name().to_str().is_some_and(ours)
            && UnixStream::connect(e.path()).is_err_and(gone)
        {
            let _ = std::fs::remove_file(e.path());
        }
    }
    let path = socket_in(&dir)?;
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    std::thread::Builder::new()
        .name("pane-control".into())
        .spawn(move || {
            for s in listener.incoming() {
                let Ok(mut s) = s else {
                    continue;
                };
                if super::CALLS.fetch_add(1, Ordering::SeqCst) >= super::MAX_CALLS {
                    super::CALLS.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let answer = answer.clone();
                let call = std::thread::Builder::new()
                    .name("pane-control-call".into())
                    .spawn(move || {
                        let _ = s.set_read_timeout(Some(super::ANSWER));
                        let _ = s.set_write_timeout(Some(super::ANSWER));
                        let done = shut_after(&s, super::DEADLINE);
                        super::serve(&mut s, answer);
                        drop(done);
                        super::CALLS.fetch_sub(1, Ordering::SeqCst);
                    });
                if call.is_err() {
                    super::CALLS.fetch_sub(1, Ordering::SeqCst);
                }
            }
        })?;
    Ok(path.to_string_lossy().into_owned())
}

/// Shuts `s` past `deadline`, unless what this returns is dropped first.
/// Its timeouts are per wait, so a caller that sends a byte now and then
/// would otherwise hold it for good.
fn shut_after(s: &UnixStream, deadline: Duration) -> mpsc::Sender<()> {
    let (done, ended) = mpsc::channel::<()>();
    if let Ok(watch) = s.try_clone() {
        let _ = std::thread::Builder::new()
            .name("pane-control-watch".into())
            .spawn(move || {
                if ended.recv_timeout(deadline) == Err(RecvTimeoutError::Timeout) {
                    let _ = watch.shutdown(Shutdown::Both);
                }
            });
    }
    done
}

/// Where in `dir` a new socket goes, through no link: [`valid`] takes
/// none, and TMPDIR on macOS is one.
fn socket_in(dir: &Path) -> io::Result<PathBuf> {
    let name = format!("blitz-ctl-{}.sock", &crate::pty::pane_token()?[..16]);
    Ok(std::fs::canonicalize(dir)?.join(name))
}

/// Whether `name` is a file name [`listen`] gives a socket.
fn ours(name: &str) -> bool {
    (name.strip_prefix("blitz-ctl-"))
        .and_then(|n| n.strip_suffix(".sock"))
        .is_some_and(|h| super::hex(h, 16))
}

/// Whether `name` is a socket [`listen`] could have made: named as it
/// names them, in a folder of this user's alone that is no link. Anywhere
/// else, another user may be listening for the token.
pub fn valid(name: &str) -> bool {
    let path = Path::new(name);
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let private = |d: &Path| {
        std::fs::symlink_metadata(d)
            .is_ok_and(|m| m.is_dir() && m.uid() == uid && m.mode() & 0o077 == 0)
    };
    path.is_absolute()
        && path.file_name().and_then(|n| n.to_str()).is_some_and(ours)
        && path.parent().is_some_and(private)
}

pub fn connect(name: &str) -> io::Result<UnixStream> {
    UnixStream::connect(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_socket_names() {
        let dir = crate::handoff::private_dir().expect("a folder of this user's own");
        let dir = std::fs::canonicalize(dir).expect("canonical");
        let at = |name: &str| dir.join(name).to_string_lossy().into_owned();
        assert!(valid(&at("blitz-ctl-0123456789abcdef.sock")));
        for bad in [
            at("blitz-ctl-0123456789ABCDEF.sock"),
            at("blitz-ctl-0123456789abcde.sock"),
            at("blitz-0123456789abcdef.sock"),
            at("blitz-ctl-0123456789abcdef.sock.x"),
            // Anyone may make a folder or a socket in /tmp.
            "/tmp/blitz-ctl-0123456789abcdef.sock".into(),
            "blitz-ctl-0123456789abcdef.sock".into(),
            String::new(),
        ] {
            assert!(!valid(&bad), "{bad}");
        }
    }

    /// A folder reached through a link, as XDG_RUNTIME_DIR or TMPDIR may
    /// be, still gives a socket path [`valid`] takes.
    #[test]
    fn control_socket_through_a_link() {
        let real = std::env::temp_dir().join(format!("blitz-ctl-test-{}", std::process::id()));
        std::fs::create_dir_all(&real).expect("folder");
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700)).expect("mode");
        let link = real.with_extension("link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&real, &link).expect("link");
        let path = socket_in(&link).expect("path");
        let ok = valid(&path.to_string_lossy());
        let _ = (std::fs::remove_file(&link), std::fs::remove_dir(&real));
        assert!(ok, "{path:?}");
    }

    /// A caller that sends a byte now and then is shut at the deadline,
    /// and one whose call ended in time is not.
    #[test]
    fn control_a_slow_caller_is_cut_off() {
        use std::io::{Read, Write};
        let (mut server, mut client) = UnixStream::pair().expect("a pair");
        let start = std::time::Instant::now();
        let _watch = shut_after(&server, Duration::from_millis(200));
        let _ = client.write_all(b"{");
        let mut got = Vec::new();
        // So a missed shut fails the assert below instead of hanging.
        let _ = server.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = server.read_to_end(&mut got);
        let took = start.elapsed();
        assert!(took >= Duration::from_millis(200), "it waited: {took:?}");
        assert!(took < Duration::from_secs(5), "{took:?}");
        let (mut server, mut client) = UnixStream::pair().expect("a pair");
        drop(shut_after(&server, Duration::from_millis(50)));
        std::thread::sleep(Duration::from_millis(150));
        let _ = client.write_all(b"x");
        let mut one = [0; 1];
        assert_eq!(server.read(&mut one).ok(), Some(1));
    }

    /// Callers past the cap are dropped at once, and blitz goes on
    /// listening.
    #[test]
    fn control_callers_past_the_cap_are_dropped() {
        use std::io::{Read, Write};
        let path = listen(|_| r#"{"ok":true}"#.into()).expect("listen");
        let calls = || crate::control::CALLS.load(Ordering::SeqCst);
        let wait_for = |done: &dyn Fn() -> bool| {
            let start = std::time::Instant::now();
            while !done() {
                assert!(start.elapsed() < Duration::from_secs(10), "{}", calls());
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        let idle: Vec<UnixStream> = (0..crate::control::MAX_CALLS)
            .map(|_| connect(&path).expect("connect"))
            .collect();
        wait_for(&|| calls() >= crate::control::MAX_CALLS);
        let mut over = connect(&path).expect("connect");
        let _ = over.write_all(b"{}\n");
        let mut got = Vec::new();
        let _ = over.read_to_end(&mut got);
        assert!(got.is_empty(), "{got:?}");
        drop(idle);
        wait_for(&|| calls() == 0);
        let mut again = connect(&path).expect("connect");
        let _ = again.write_all(b"{}\n");
        let mut got = String::new();
        let _ = again.read_to_string(&mut got);
        let _ = std::fs::remove_file(&path);
        assert!(got.contains("ok"), "{got:?}");
    }

    /// The socket of a blitz that stopped without removing it goes when
    /// the next one starts.
    #[test]
    fn control_a_left_socket_goes() {
        let dir = crate::handoff::private_dir().expect("a folder of this user's own");
        let token = crate::pty::pane_token().expect("a token");
        let left = dir.join(format!("blitz-ctl-{}.sock", &token[..16]));
        drop(UnixListener::bind(&left).expect("bind"));
        assert!(left.exists());
        let live = listen(|_| String::new()).expect("listen");
        let gone = !left.exists();
        let kept = connect(&live).is_ok();
        let _ = std::fs::remove_file(&live);
        assert!(gone && kept, "{gone} {kept}");
    }
}
