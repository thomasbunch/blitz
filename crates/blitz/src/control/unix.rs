//! The channel as a Unix socket in a folder only this user can reach.

use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

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
    let path = dir.join(format!(
        "blitz-ctl-{}.sock",
        &crate::pty::pane_token()?[..16]
    ));
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    std::thread::Builder::new()
        .name("pane-control".into())
        .spawn(move || {
            for s in listener.incoming() {
                let Ok(mut s) = s else {
                    continue;
                };
                let answer = answer.clone();
                let _ = std::thread::Builder::new()
                    .name("pane-control-call".into())
                    .spawn(move || {
                        let _ = s.set_read_timeout(Some(super::ANSWER));
                        let _ = s.set_write_timeout(Some(super::ANSWER));
                        super::serve(&mut s, answer);
                    });
            }
        })?;
    Ok(path.to_string_lossy().into_owned())
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
