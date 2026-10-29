//! The channel as a Unix socket in a folder only this user can reach.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

/// Listens on a new socket of this process's own and answers each request
/// with `answer`, on a thread per connection. Returns the socket's path,
/// short enough for macOS's limit of 104 bytes.
pub fn listen(answer: impl Fn(String) -> String + Clone + Send + 'static) -> io::Result<String> {
    let dir = crate::handoff::private_dir()
        .ok_or_else(|| io::Error::other("no folder of this user's own"))?;
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
                        super::serve(&mut s, answer);
                    });
            }
        })?;
    Ok(path.to_string_lossy().into_owned())
}

/// Whether `name` is a socket path [`listen`] could have made.
pub fn valid(name: &str) -> bool {
    Path::new(name).is_absolute()
}

pub fn connect(name: &str) -> io::Result<UnixStream> {
    UnixStream::connect(name)
}
