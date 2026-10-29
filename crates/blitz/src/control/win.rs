//! The channel as a named pipe that only this user, in this session and
//! at this integrity level or above, can open.

use std::fs::File;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION,
};
use windows::Win32::System::IO::CancelSynchronousIo;
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientSessionId, GetNamedPipeServerSessionId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT,
};
use windows::core::HSTRING;

/// What every pipe name starts with; 32 hex digits follow.
const PREFIX: &str = r"\\.\pipe\blitz-";
/// How long a caller waits for blitz to open the pipe again while it
/// answers others.
const BUSY: Duration = Duration::from_secs(2);

/// Listens on a new pipe of this process's own and answers each request
/// with `answer`, on a thread per connection. Returns the pipe's name.
pub fn listen(answer: impl Fn(String) -> String + Clone + Send + 'static) -> io::Result<String> {
    let name = format!("{PREFIX}{}", crate::pty::pane_token()?);
    let user = crate::hook::user_sid().ok_or_else(|| io::Error::other("no user SID"))?;
    // Only this user; an elevated blitz only for elevated programs.
    let label = if crate::desktop::elevated() {
        "S:(ML;;NWNR;;;HI)"
    } else {
        ""
    };
    let sddl = HSTRING::from(format!("D:P(A;;GA;;;{user}){label}"));
    let wide = HSTRING::from(&name);
    // Made here, so that no other program can have made the name first.
    let mut waiting = instance(&wide, &sddl, true)?;
    std::thread::Builder::new()
        .name("pane-control".into())
        .spawn(move || {
            loop {
                // SAFETY: a pipe handle `waiting` owns, waited on with no
                // overlapped I/O.
                let connected = unsafe { ConnectNamedPipe(raw(&waiting), None) };
                // The next one before this one goes, so the name is never
                // free for another program to take.
                let next = match instance(&wide, &sddl, false) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("blitz: `blitz pane` stops: {e}");
                        return;
                    }
                };
                let pipe = std::mem::replace(&mut waiting, next);
                if connected.is_err_and(|e| e.code() != ERROR_PIPE_CONNECTED.to_hresult()) {
                    continue;
                }
                // Past the cap a caller is dropped at once, so a flood
                // cannot use up the instances blitz listens with.
                if super::CALLS.fetch_add(1, Ordering::SeqCst) >= super::MAX_CALLS {
                    super::CALLS.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let answer = answer.clone();
                let call = move || {
                    // A program in another session, as another user's
                    // signed in alongside, gets nothing.
                    if same_session(&pipe) {
                        let mut f = File::from(pipe);
                        super::serve(&mut f, answer);
                        // Until the reply is read: closing drops what is
                        // unread.
                        let _ = f.sync_all();
                    }
                };
                let watched = std::thread::Builder::new()
                    .name("pane-control-call".into())
                    .spawn(move || {
                        within(super::DEADLINE, call);
                        super::CALLS.fetch_sub(1, Ordering::SeqCst);
                    });
                if watched.is_err() {
                    super::CALLS.fetch_sub(1, Ordering::SeqCst);
                }
            }
        })?;
    Ok(name)
}

/// Runs `work` on a thread of its own until it ends. Past `deadline` the
/// reads and writes it waits on are cancelled, again until it ends: a
/// caller that sends nothing, or never reads the reply, would otherwise
/// hold it for good.
fn within(deadline: Duration, work: impl FnOnce() + Send + 'static) {
    let (done, ended) = mpsc::channel::<()>();
    let worker = std::thread::Builder::new()
        .name("pane-control-serve".into())
        .spawn(move || {
            work();
            drop(done);
        });
    let Ok(worker) = worker else {
        return;
    };
    if ended.recv_timeout(deadline) == Err(RecvTimeoutError::Timeout) {
        // Again, as it may start its next wait after a cancel.
        while !worker.is_finished() {
            // SAFETY: the thread's own handle, which `worker` keeps open;
            // one that waits on nothing has nothing cancelled.
            let _ = unsafe { CancelSynchronousIo(HANDLE(worker.as_raw_handle())) };
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let _ = worker.join();
}

/// A new instance of the pipe `name`, guarded by the descriptor `sddl`;
/// the `first` fails when the name exists.
fn instance(name: &HSTRING, sddl: &HSTRING, first: bool) -> io::Result<OwnedHandle> {
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: a valid string and out pointer; `sd` is freed below.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl, SDDL_REVISION_1, &mut sd, None)?
    };
    let sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: false.into(),
    };
    let open = match first {
        true => PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
        false => PIPE_ACCESS_DUPLEX,
    };
    let mode = PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS;
    let (n, size) = (PIPE_UNLIMITED_INSTANCES, 1 << 16);
    // SAFETY: valid name and attributes, which the call copies.
    let h = unsafe { CreateNamedPipeW(name, open, mode, n, size, size, 0, Some(&sa)) };
    let failed = h.is_invalid().then(io::Error::last_os_error);
    // SAFETY: made by the conversion above, and used no more.
    unsafe { LocalFree(Some(HLOCAL(sd.0))) };
    match failed {
        Some(e) => Err(e),
        // SAFETY: a new handle nothing else owns.
        None => Ok(unsafe { OwnedHandle::from_raw_handle(h.0) }),
    }
}

fn raw(h: &OwnedHandle) -> HANDLE {
    HANDLE(h.as_raw_handle())
}

/// Whether the program at the other end of `pipe` runs in blitz's session.
fn same_session(pipe: &OwnedHandle) -> bool {
    let (mut client, mut server) = (0, 0);
    // SAFETY: a connected pipe handle and u32s to fill.
    unsafe {
        GetNamedPipeClientSessionId(raw(pipe), &mut client).is_ok()
            && GetNamedPipeServerSessionId(raw(pipe), &mut server).is_ok()
            && client == server
    }
}

/// Whether `name` is a pipe [`listen`] could have made: never a network
/// path, which opening would sign in to that host.
pub fn valid(name: &str) -> bool {
    name.strip_prefix(PREFIX).is_some_and(|h| super::hex(h, 32))
}

/// Opens the pipe `name`, waiting a while when blitz is busy with others.
/// blitz may learn who opened it, but cannot act as them.
pub fn connect(name: &str) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    let start = Instant::now();
    loop {
        let opened = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .security_qos_flags(SECURITY_IDENTIFICATION.0)
            .open(name);
        match opened {
            Err(e)
                if e.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) && start.elapsed() < BUSY =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            r => return r,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, Read, Write};

    use super::*;

    #[test]
    fn control_pipe_names() {
        let ok = format!("{PREFIX}{}", "0123456789abcdef".repeat(2));
        assert!(valid(&ok));
        for bad in [
            r"\\server\pipe\blitz-0123456789abcdef0123456789abcdef",
            r"\\?\UNC\server\pipe\blitz-0123456789abcdef0123456789abcdef",
            r"\\.\pipe\blitz-0123456789ABCDEF0123456789abcdef",
            r"\\.\pipe\blitz-0123456789abcdef0123456789abcde",
            r"\\.\pipe\other-0123456789abcdef0123456789abcdef",
            r"C:\blitz-0123456789abcdef0123456789abcdef",
            "",
        ] {
            assert!(!valid(bad), "{bad}");
        }
        assert!(!valid(&format!("{ok}0")));
        assert!(!valid(&format!("{ok}\\x")));
    }

    /// A caller that connects and says nothing is cut off at the deadline.
    #[test]
    fn control_a_silent_caller_is_cut_off() {
        let name = format!("{PREFIX}{}", crate::pty::pane_token().expect("a name"));
        let me = crate::hook::user_sid().expect("a SID");
        let sddl = HSTRING::from(format!("D:P(A;;GA;;;{me})"));
        let server = instance(&HSTRING::from(&name), &sddl, true).expect("a pipe");
        let _silent = connect(&name).expect("connect");
        let start = Instant::now();
        within(Duration::from_millis(200), move || {
            let _ = File::from(server).read(&mut [0; 1]);
        });
        let took = start.elapsed();
        assert!(took >= Duration::from_millis(200), "it waited: {took:?}");
        assert!(took < Duration::from_secs(5), "{took:?}");
    }

    /// Callers past the cap are dropped at once, and blitz goes on
    /// listening.
    #[test]
    fn control_callers_past_the_cap_are_dropped() {
        let pipe = listen(|_| r#"{"ok":true}"#.into()).expect("listen");
        let calls = || crate::control::CALLS.load(Ordering::SeqCst);
        let wait_for = |done: &dyn Fn() -> bool| {
            let start = Instant::now();
            while !done() {
                assert!(start.elapsed() < Duration::from_secs(10), "{}", calls());
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        let idle: Vec<File> = (0..crate::control::MAX_CALLS)
            .map(|_| connect(&pipe).expect("connect"))
            .collect();
        wait_for(&|| calls() == crate::control::MAX_CALLS);
        let mut over = connect(&pipe).expect("connect");
        let _ = over.write_all(b"{}\n");
        let mut got = Vec::new();
        let _ = over.read_to_end(&mut got);
        assert!(got.is_empty(), "{got:?}");
        drop(idle);
        wait_for(&|| calls() == 0);
        let mut c = connect(&pipe).expect("connect");
        c.write_all(b"x\n").expect("write");
        let mut reply = String::new();
        io::BufReader::new(c).read_line(&mut reply).expect("read");
        assert_eq!(reply, "{\"ok\":true}\n");
    }
}
