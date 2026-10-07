//! ConPTY: loading, spawning, the I/O threads, resize and shutdown.

use std::ffi::{OsString, c_void};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, mpsc};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, HMODULE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT};
use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
use windows::Win32::System::Console::COORD;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows::Win32::System::LibraryLoader::{
    GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
    LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetExitCodeProcess, INFINITE,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows::core::{HSTRING, PCSTR, PCWSTR, PWSTR, w};

/// Lay out glyphs by grapheme cluster, matching the terminal's width model.
const PSEUDOCONSOLE_GLYPH_WIDTH_GRAPHEMES: u32 = 0x8;
/// Each direction of the pipes buffers this much.
const PIPE_BYTES: u32 = 128 * 1024;
/// The first bytes the bundled ConPTY writes. The inbox one starts with
/// `ESC[?9001h` instead.
const BUNDLED_PRELUDE: &[u8] = b"\x1b[1t";
/// Bytes queued for the child's input beyond which replies are dropped.
const MAX_PENDING: usize = 1 << 20;

/// What `GetProcAddress` returns before it is cast.
type Proc = unsafe extern "system" fn() -> isize;
type CreateFn = unsafe extern "system" fn(COORD, HANDLE, HANDLE, u32, *mut isize) -> i32;
type ResizeFn = unsafe extern "system" fn(isize, COORD) -> i32;
type CloseFn = unsafe extern "system" fn(isize);
type ReleaseFn = unsafe extern "system" fn(isize) -> i32;
type ReparentFn = unsafe extern "system" fn(isize, isize) -> i32;

/// The pseudoconsole functions, from Microsoft's redistributable
/// `conpty.dll` when it is present, else from the system.
struct Conpty {
    create: CreateFn,
    resize: ResizeFn,
    close: CloseFn,
    /// Lets the output pipe reach EOF once every client has exited.
    release: Option<ReleaseFn>,
    reparent: Option<ReparentFn>,
    bundled: bool,
}

/// Set when a pseudoconsole that should be the bundled one starts like the
/// inbox one.
static DEGRADED: AtomicBool = AtomicBool::new(false);

fn conpty() -> Option<&'static Conpty> {
    static API: OnceLock<Option<Conpty>> = OnceLock::new();
    API.get_or_init(|| {
        // A relative folder would load code from wherever blitz was started.
        let dir = std::env::var_os("BLITZ_CONPTY_DIR")
            .map(PathBuf::from)
            .filter(|d| d.is_absolute())
            .or_else(|| {
                std::env::current_exe()
                    .ok()?
                    .parent()
                    .map(Path::to_path_buf)
            })
            .and_then(|d| std::path::absolute(d).ok());
        dir.and_then(|d| load_bundled(&d)).or_else(load_inbox)
    })
    .as_ref()
}

/// Loads `conpty.dll` from `dir`. It starts `OpenConsole.exe` from its own
/// directory, and without that exe it silently falls back to the inbox
/// console host, so both files must be there.
fn load_bundled(dir: &Path) -> Option<Conpty> {
    if !dir.join("OpenConsole.exe").is_file() {
        return None;
    }
    let dll = HSTRING::from(dir.join("conpty.dll").as_path());
    // SAFETY: an absolute path; the search flags keep the DLL's own
    // dependencies out of the current directory and PATH.
    let m = unsafe {
        LoadLibraryExW(
            &dll,
            None,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    }
    .ok()?;
    resolve(m, "Conpty", true)
}

fn load_inbox() -> Option<Conpty> {
    // SAFETY: kernel32 is always loaded.
    let m = unsafe { GetModuleHandleW(w!("kernel32.dll")) }.ok()?;
    resolve(m, "", false)
}

fn resolve(m: HMODULE, prefix: &str, bundled: bool) -> Option<Conpty> {
    let sym = |name: &str| {
        let c = format!("{prefix}{name}\0");
        // SAFETY: `c` is NUL-terminated and `m` is a loaded module.
        unsafe { GetProcAddress(m, PCSTR(c.as_ptr())) }
    };
    // SAFETY: each symbol has the documented pseudoconsole signature, which
    // the redistributable shares with kernel32.
    unsafe {
        Some(Conpty {
            create: std::mem::transmute::<Proc, CreateFn>(sym("CreatePseudoConsole")?),
            resize: std::mem::transmute::<Proc, ResizeFn>(sym("ResizePseudoConsole")?),
            close: std::mem::transmute::<Proc, CloseFn>(sym("ClosePseudoConsole")?),
            release: sym("ReleasePseudoConsole").map(|f| std::mem::transmute::<Proc, ReleaseFn>(f)),
            reparent: sym("ReparentPseudoConsole")
                .map(|f| std::mem::transmute::<Proc, ReparentFn>(f)),
            bundled,
        })
    }
}

/// A one-line notice for the user when blitz runs on the system's ConPTY,
/// which answers terminal queries itself and adds latency.
pub fn inbox_notice() -> Option<&'static str> {
    let bundled = conpty().is_some_and(|c| c.bundled) && !DEGRADED.load(Ordering::Relaxed);
    (!bundled).then_some(
        "Using the Windows console host. Put conpty.dll and OpenConsole.exe next to blitz.exe for full support.",
    )
}

/// A new secret for one pane: 128 bits from the system's random number
/// generator, in hex. The pane's child gets it as `BLITZ_PANE_TOKEN`, and
/// blitz-hook and the shell integration put it in the sequences they print,
/// so the pane can tell them apart from program output, which cannot read
/// the environment.
pub fn pane_token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    // SAFETY: a valid buffer; this flag takes no algorithm handle.
    unsafe { BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }.ok()?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// What the reader thread reports.
pub enum PtyEvent<'a> {
    /// Output from the child, in order. Replies to queries go through the
    /// [`Writer`] passed alongside, so they stay in stream order.
    Data(&'a [u8]),
    /// The pseudoconsole and every process attached to it are gone. Carries
    /// the shell's exit code (`STILL_ACTIVE` if it outlived a forced close).
    Exit(u32),
}

/// Sends bytes to the child's input. Cheap to clone; writes happen on the
/// pane's writer thread, in the order they are sent.
#[derive(Clone)]
pub struct Writer {
    tx: mpsc::Sender<Vec<u8>>,
    /// Bytes sent but not yet written.
    pending: Arc<AtomicUsize>,
}

impl Writer {
    pub fn send(&self, bytes: impl Into<Vec<u8>>) {
        let bytes = bytes.into();
        self.pending.fetch_add(bytes.len(), Ordering::Relaxed);
        // An error only means the child has gone.
        let _ = self.tx.send(bytes);
    }

    /// [`Writer::send`] for answers to the child's queries. They are dropped
    /// while its input is backed up, so a program that floods queries
    /// cannot make blitz queue replies without limit.
    pub fn reply(&self, bytes: impl Into<Vec<u8>>) {
        if self.pending.load(Ordering::Relaxed) < MAX_PENDING {
            self.send(bytes);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SpawnOpts<'a> {
    /// The full command line, e.g. from [`crate::shell::launch`].
    pub cmdline: &'a str,
    pub cwd: Option<&'a Path>,
    /// Added to the child's environment after blitz's own variables.
    pub env: &'a [(String, String)],
    pub cols: u16,
    pub rows: u16,
    /// Exported to the child as `BLITZ_PANE_ID`.
    pub pane_id: u32,
    /// Window that owns the pseudoconsole's hidden console window.
    pub parent: Option<isize>,
}

/// A child process attached to its own pseudoconsole.
pub struct Pty {
    /// The pseudoconsole handle; 0 once closed.
    hpc: Arc<Mutex<isize>>,
    process: Arc<OwnedHandle>,
    writer: Writer,
    spawned: Instant,
    closing: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Closes the pseudoconsole once. This can block until its output has been
/// read, so never call it on the reader thread while the pipe is open.
fn close_hpc(hpc: &Mutex<isize>) {
    let h = std::mem::take(&mut *lock(hpc));
    if let (true, Some(c)) = (h != 0, conpty()) {
        // SAFETY: `h` came from `create` and is closed exactly once.
        unsafe { (c.close)(h) };
    }
}

fn raw(h: &OwnedHandle) -> HANDLE {
    HANDLE(h.as_raw_handle())
}

fn exit_code(process: &OwnedHandle) -> u32 {
    let mut code = 0;
    // SAFETY: a valid process handle and a valid out pointer.
    let _ = unsafe { GetExitCodeProcess(raw(process), &mut code) };
    code
}

fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let (mut r, mut w) = (HANDLE::default(), HANDLE::default());
    // SAFETY: valid out pointers; the handles are owned right away.
    unsafe {
        CreatePipe(&mut r, &mut w, None, PIPE_BYTES)?;
        Ok((
            OwnedHandle::from_raw_handle(r.0),
            OwnedHandle::from_raw_handle(w.0),
        ))
    }
}

impl Pty {
    /// Starts `opts.cmdline` on a new pseudoconsole. `on_event` runs on the
    /// pane's reader thread for every chunk of output and once at exit.
    pub fn spawn(
        opts: &SpawnOpts,
        mut on_event: impl FnMut(PtyEvent<'_>, &Writer) + Send + 'static,
    ) -> io::Result<Pty> {
        let api = conpty().ok_or_else(|| io::Error::other("ConPTY is not available"))?;
        let program = find_program(opts.cmdline, |k| std::env::var_os(k))?;
        let (in_r, in_w) = pipe()?;
        let (out_r, out_w) = pipe()?;
        let size = COORD {
            X: opts.cols.clamp(1, 32767) as i16,
            Y: opts.rows.clamp(1, 32767) as i16,
        };
        let mut hpc = 0isize;
        // SAFETY: valid pipe handles and out pointer.
        let hr = unsafe {
            (api.create)(
                size,
                raw(&in_r),
                raw(&out_w),
                PSEUDOCONSOLE_GLYPH_WIDTH_GRAPHEMES,
                &mut hpc,
            )
        };
        if hr < 0 {
            return Err(windows::core::Error::from_hresult(windows::core::HRESULT(hr)).into());
        }
        // The pseudoconsole holds its own copies of these.
        drop((in_r, out_w));
        // Started before the handle goes into its mutex: a lock taken in the
        // match below would be held through it, and closing takes it again.
        let process = start(opts, program.as_deref(), hpc);
        let hpc = Arc::new(Mutex::new(hpc));
        let process = match process {
            Ok(p) => Arc::new(p),
            Err(e) => {
                close_hpc(&hpc);
                return Err(e);
            }
        };
        let spawned = Instant::now();
        // Without its threads nothing would ever close the pane, so a thread
        // that fails to start takes the child down with it.
        let fail = |e: io::Error| {
            // SAFETY: a valid process handle.
            let _ = unsafe { TerminateProcess(raw(&process), 1) };
            close_hpc(&hpc);
            e
        };
        let h = *lock(&hpc);
        if let (Some(hwnd), Some(reparent)) = (opts.parent, api.reparent) {
            // SAFETY: a live pseudoconsole; a bad window only fails the call.
            unsafe { reparent(h, hwnd) };
        }
        match api.release {
            // The output pipe now reaches EOF once the last client exits,
            // after the final frame has been written.
            // SAFETY: a live pseudoconsole.
            Some(release) => unsafe {
                release(h);
            },
            // Without release the console host outlives its clients; close it
            // when the shell exits so the reader sees EOF.
            None => {
                let (hpc, process) = (hpc.clone(), process.clone());
                std::thread::Builder::new()
                    .name("pty-wait".into())
                    .spawn(move || {
                        // SAFETY: a valid process handle.
                        unsafe { WaitForSingleObject(raw(&process), INFINITE) };
                        close_hpc(&hpc);
                    })
                    .map_err(fail)?;
            }
        }

        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let writer = Writer {
            tx,
            pending: Arc::default(),
        };
        let mut input = File::from(in_w);
        let pending = writer.pending.clone();
        std::thread::Builder::new()
            .name("pty-write".into())
            .spawn(move || {
                for bytes in rx {
                    if input.write_all(&bytes).is_err() {
                        break;
                    }
                    pending.fetch_sub(bytes.len(), Ordering::Relaxed);
                }
            })
            .map_err(fail)?;

        let (reply, hpc2, process2) = (writer.clone(), hpc.clone(), process.clone());
        let mut output = File::from(out_r);
        let bundled = api.bundled;
        std::thread::Builder::new()
            .name("pty-read".into())
            .spawn(move || {
                let mut buf = vec![0; PIPE_BYTES as usize];
                let mut first = true;
                while let Ok(n @ 1..) = output.read(&mut buf) {
                    if std::mem::take(&mut first)
                        && bundled
                        && !buf[..n].starts_with(BUNDLED_PRELUDE)
                    {
                        DEGRADED.store(true, Ordering::Relaxed);
                    }
                    on_event(PtyEvent::Data(&buf[..n]), &reply);
                }
                // EOF arrives just before the shell's exit is visible; give it a
                // moment so the code is real.
                // SAFETY: a valid process handle.
                unsafe { WaitForSingleObject(raw(&process2), 1000) };
                // The pipe is closed, so this cannot block on unread output.
                close_hpc(&hpc2);
                on_event(PtyEvent::Exit(exit_code(&process2)), &reply);
            })
            .map_err(fail)?;

        Ok(Pty {
            hpc,
            process,
            writer,
            spawned,
            closing: AtomicBool::new(false),
        })
    }

    pub fn writer(&self) -> &Writer {
        &self.writer
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        let h = lock(&self.hpc);
        if let (true, Some(c)) = (*h != 0, conpty()) {
            let size = COORD {
                X: cols.min(32767) as i16,
                Y: rows.min(32767) as i16,
            };
            // SAFETY: a live pseudoconsole; the lock keeps it from closing.
            unsafe { (c.resize)(*h, size) };
        }
    }

    /// Asks the child to exit (CTRL_CLOSE_EVENT) and kills it if it is still
    /// running 6 s later. Returns at once; output keeps flowing to the reader
    /// until EOF. Also called on drop.
    pub fn close(&self) {
        if self.closing.swap(true, Ordering::Relaxed) {
            return;
        }
        let (hpc, process, spawned) = (self.hpc.clone(), self.process.clone(), self.spawned);
        let _ = std::thread::Builder::new()
            .name("pty-close".into())
            .spawn(move || {
                // Closing a console while its first process is still starting
                // makes that process fail with 0xC0000142 and an error dialog.
                std::thread::sleep(Duration::from_millis(300).saturating_sub(spawned.elapsed()));
                close_hpc(&hpc);
                // SAFETY: a valid process handle.
                if unsafe { WaitForSingleObject(raw(&process), 6000) } == WAIT_TIMEOUT {
                    // SAFETY: as above.
                    let _ = unsafe { TerminateProcess(raw(&process), 1) };
                }
            });
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.close();
    }
}

/// Puts this process in a job that kills every process it started, console
/// hosts included, when this process exits. For headless runs, so a run that
/// times out or crashes leaves nothing behind.
pub fn kill_children_on_exit() -> io::Result<()> {
    // SAFETY: plain Win32 calls with valid arguments. The job handle is
    // deliberately never closed: closing it is what kills the children.
    unsafe {
        let job = CreateJobObjectW(None, PCWSTR::null())?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            size_of_val(&info) as u32,
        )?;
        AssignProcessToJobObject(job, GetCurrentProcess())?;
    }
    Ok(())
}

/// The program `cmdline` starts, found the way `CreateProcessW` would but
/// never in blitz's own folder or its current directory: blitz starts
/// wherever Explorer was, and a `cmd.exe` in a downloaded folder must not
/// run in place of the real one. A program named with a folder is left to
/// `CreateProcessW` (`None`). One named without is looked for in the system
/// folders, then in each folder on PATH, with `.exe` added when it has no
/// extension, as `CreateProcessW` does.
pub fn find_program(
    cmdline: &str,
    var: impl Fn(&str) -> Option<OsString>,
) -> io::Result<Option<PathBuf>> {
    let s = cmdline.trim_start_matches([' ', '\t']);
    let name = match s.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => s.split([' ', '\t']).next().unwrap_or(s),
    };
    if name.is_empty() || name.contains(['\\', '/', ':']) {
        return Ok(None);
    }
    let file = match Path::new(name).extension() {
        Some(_) => name.to_owned(),
        None => format!("{name}.exe"),
    };
    let root = crate::shell::system_root(&var);
    let path = var("PATH").unwrap_or_default();
    [root.join("System32"), root]
        .into_iter()
        // A relative entry is the current directory again.
        .chain(std::env::split_paths(&path).filter(|d| d.is_absolute()))
        .map(|d| d.join(&file))
        .find(|p| p.is_file())
        .map(Some)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{file} is not in the Windows folder or on PATH"),
            )
        })
}

/// Creates the child process attached to pseudoconsole `hpc`, running
/// `program` when given, else the first word of the command line.
fn start(opts: &SpawnOpts, program: Option<&Path>, hpc: isize) -> io::Result<OwnedHandle> {
    let mut size = 0;
    // SAFETY: a size query; it fails by design with the size filled in.
    let _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &mut size) };
    // u64 keeps the list pointer-aligned.
    let mut buf = vec![0u64; size.div_ceil(8)];
    let list = LPPROC_THREAD_ATTRIBUTE_LIST(buf.as_mut_ptr().cast());
    // SAFETY: `buf` is at least `size` bytes and outlives the list.
    unsafe { InitializeProcThreadAttributeList(Some(list), 1, None, &mut size)? };
    let result = (|| {
        // SAFETY: the pseudoconsole attribute takes the handle itself as
        // the value pointer.
        unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                Some(hpc as *const c_void),
                size_of::<isize>(),
                None,
                None,
            )?;
        }
        let mut si = STARTUPINFOEXW::default();
        si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        // Without this the child inherits our own redirected std handles
        // and writes past the pseudoconsole.
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
        si.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
        si.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
        si.lpAttributeList = list;

        let env = env_block(&child_env(std::env::vars_os(), opts.pane_id, opts.env));
        let mut cmd: Vec<u16> = opts.cmdline.encode_utf16().chain([0]).collect();
        let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain([0]).collect() };
        let cwd = opts.cwd.map(wide);
        let program = program.map(wide);
        let mut pi = PROCESS_INFORMATION::default();
        // SAFETY: every pointer is valid for the duration of the call.
        unsafe {
            CreateProcessW(
                program
                    .as_ref()
                    .map_or(PCWSTR::null(), |p| PCWSTR(p.as_ptr())),
                Some(PWSTR(cmd.as_mut_ptr())),
                None,
                None,
                false,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                Some(env.as_ptr().cast()),
                cwd.as_ref().map_or(PCWSTR::null(), |c| PCWSTR(c.as_ptr())),
                &si.StartupInfo,
                &mut pi,
            )?;
            drop(OwnedHandle::from_raw_handle(pi.hThread.0));
            Ok(OwnedHandle::from_raw_handle(pi.hProcess.0))
        }
    })();
    // SAFETY: initialized above and no longer used.
    unsafe { DeleteProcThreadAttributeList(list) };
    result
}

/// Variables that describe the terminal the parent runs in. A child that
/// sees them would believe it runs in that terminal instead of blitz.
const STRIP_PREFIXES: &[&str] = &[
    "TERM_PROGRAM",
    "KITTY_",
    "GHOSTTY_",
    "WEZTERM_",
    "ITERM_",
    "LC_TERMINAL",
    "CONEMU",
    "VSCODE_",
    "ALACRITTY_",
    // blitz's own settings, such as BLITZ_CONPTY_DIR and BLITZ_TRACE, are
    // for the blitz they were set for, not for programs its panes start.
    "BLITZ_",
];

const STRIP: &[&str] = &[
    // TERM changes how Git for Windows tools draw; Windows terminals leave
    // it unset.
    "TERM",
    "WT_SESSION",
    "WT_PROFILE_ID",
    "TERMINAL_EMULATOR",
    "VTE_VERSION",
    "ZED_TERM",
    "CURSOR_TRACE_ID",
    "TILIX_ID",
    "KONSOLE_VERSION",
    "GNOME_TERMINAL_SERVICE",
    "TERMINATOR_UUID",
    "XTERM_VERSION",
    "TMUX",
    "TMUX_PANE",
    "ZELLIJ",
    "STY",
    "COLORTERM",
    // Set when blitz itself was started from a Claude Code session. Each
    // pane starts a fresh session, and the messaging token is a secret.
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_REMOTE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_PLUGIN_DATA",
    "CLAUDE_PROJECT_DIR",
    "CLAUDE_ENV_FILE",
];

/// The environment for a pane's child: `parent` minus other terminals'
/// markers, plus blitz's own, plus `extra` (which wins). Names compare
/// case-insensitively, and the result is sorted the way Windows expects.
pub fn child_env(
    parent: impl IntoIterator<Item = (OsString, OsString)>,
    pane_id: u32,
    extra: &[(String, String)],
) -> Vec<(OsString, OsString)> {
    let upper = |k: &OsString| k.to_string_lossy().to_ascii_uppercase();
    let mut env: Vec<(OsString, OsString)> = parent
        .into_iter()
        .filter(|(k, v)| {
            let k = upper(k);
            !STRIP.contains(&k.as_str())
                && !STRIP_PREFIXES.iter().any(|p| k.starts_with(p))
                && !(k == "PROMPT" && crate::shell::is_blitz_prompt(v))
        })
        .collect();
    let id = pane_id.to_string();
    let ours = [
        ("TERM_PROGRAM", "blitz"),
        ("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION")),
        ("COLORTERM", "truecolor"),
        ("FORCE_HYPERLINK", "1"),
        ("BLITZ_PANE_ID", id.as_str()),
    ];
    let sets = ours
        .into_iter()
        .chain(extra.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    for (k, v) in sets {
        let ku = k.to_ascii_uppercase();
        env.retain(|(e, _)| upper(e) != ku);
        env.push((k.into(), v.into()));
    }
    env.sort_by_cached_key(|(k, _)| upper(k));
    env
}

/// `K=V\0...\0\0` in UTF-16, for `CREATE_UNICODE_ENVIRONMENT`.
pub fn env_block(env: &[(OsString, OsString)]) -> Vec<u16> {
    let mut block = Vec::new();
    for (k, v) in env {
        block.extend(k.encode_wide());
        block.push(u16::from(b'='));
        block.extend(v.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_strips_markers_and_sets_ours() {
        let parent = [
            ("CLAUDECODE", "1"),
            ("TERM", "xterm"),
            ("WT_SESSION", "x"),
            ("term_program", "vscode"),
            ("ConEmuPID", "1"),
            ("CLAUDE_CODE_MESSAGING_TOKEN", "secret"),
            ("BLITZ_CONPTY_DIR", "conpty"),
            ("blitz_trace", "t.txt"),
            ("CLAUDE_CONFIG_DIR", "c"),
            ("ANTHROPIC_MODEL", "m"),
            ("=C:", r"C:\work"),
            ("Path", r"C:\bin"),
            // From the blitz pane this blitz was started in: its token is
            // that pane's, and the token is a secret.
            ("BLITZ_PANE_TOKEN", "0f1e"),
            ("PROMPT", crate::shell::cmd_prompt("0f1e").as_str()),
            // In tmux or zellij, Claude Code stops turning the mark in its
            // title.
            ("TMUX", "/tmp/tmux-1000/default,1,0"),
            ("TMUX_PANE", "%3"),
            ("ZELLIJ", "0"),
        ]
        .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let extra = [("Path".to_owned(), r"C:\other".to_owned())];
        let env = child_env(parent, 7, &extra);
        let got: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect();
        let want = [
            ("=C:", r"C:\work"),
            ("ANTHROPIC_MODEL", "m"),
            ("BLITZ_PANE_ID", "7"),
            ("CLAUDE_CONFIG_DIR", "c"),
            ("COLORTERM", "truecolor"),
            ("FORCE_HYPERLINK", "1"),
            ("Path", r"C:\other"),
            ("TERM_PROGRAM", "blitz"),
            ("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION")),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned()));
        assert_eq!(got, want);

        let block = env_block(&env[..1]);
        assert_eq!(String::from_utf16(&block).unwrap(), "=C:=C:\\work\0\0");

        // The user's own prompt is theirs to keep; blitz's own for a pane
        // replaces any.
        let prompt = |parent: &str, extra: &[(String, String)]| {
            let env = child_env([("prompt".into(), parent.into())], 1, extra);
            (env.iter())
                .find(|(k, _)| k.eq_ignore_ascii_case("PROMPT"))
                .map(|(_, v)| v.to_string_lossy().into_owned())
        };
        assert_eq!(prompt("$P$G$_", &[]).as_deref(), Some("$P$G$_"));
        let ours = crate::shell::cmd_prompt("5eed");
        let set = [("PROMPT".to_owned(), ours.clone())];
        assert_eq!(prompt(&crate::shell::cmd_prompt("0f1e"), &set), Some(ours));
    }

    #[test]
    fn replies_wait_for_backed_up_input() {
        let (tx, rx) = mpsc::channel();
        let w = Writer {
            tx,
            pending: Arc::default(),
        };
        w.reply(&b"1"[..]);
        w.send(vec![0; MAX_PENDING]);
        w.reply(&b"2"[..]);
        w.send(&b"typed"[..]);
        let got: Vec<Vec<u8>> = rx.try_iter().collect();
        assert_eq!(got.len(), 3);
        assert_eq!((&got[0][..], &got[2][..]), (&b"1"[..], &b"typed"[..]));
    }

    #[test]
    fn programs_without_a_folder_are_looked_up_but_never_here() {
        let root = std::env::temp_dir().join(format!("blitz-find-{}", std::process::id()));
        let (win, bin, more) = (root.join("win"), root.join("bin"), root.join("more"));
        for f in [
            win.join("System32").join("cmd.exe"),
            bin.join("pwsh.exe"),
            bin.join("tool.com"),
            more.join("pwsh.exe"),
            more.join("late.exe"),
        ] {
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, b"").unwrap();
        }
        // A relative entry, which would be the current directory.
        let path = std::env::join_paths([Path::new("bin"), &bin, &more]).unwrap();
        let var = |k: &str| match k {
            "SystemRoot" => Some(win.clone().into_os_string()),
            "PATH" => Some(path.clone()),
            _ => None,
        };
        let find = |c: &str| find_program(c, var).map_err(|e| e.kind());
        let found = [
            (
                "cmd /d /c echo hi",
                Some(win.join("System32").join("cmd.exe")),
            ),
            ("pwsh -NoLogo", Some(bin.join("pwsh.exe"))),
            ("\"pwsh\" -NoLogo", Some(bin.join("pwsh.exe"))),
            (" \tpwsh", Some(bin.join("pwsh.exe"))),
            ("late", Some(more.join("late.exe"))),
            ("tool.com /x", Some(bin.join("tool.com"))),
            // Named with a folder: what the user asked for, as given.
            (r#""C:\Program Files\x\y.exe" -a"#, None),
            (r".\pwsh.exe", None),
            ("bin/pwsh", None),
            ("C:pwsh", None),
            ("", None),
        ];
        let missing = [
            find("no-such-4b1d"),
            // Only `.exe` is added, as CreateProcessW does.
            find("tool"),
        ];
        let got: Vec<_> = found.iter().map(|(c, _)| find(c)).collect();
        let _ = std::fs::remove_dir_all(&root);
        for ((c, want), got) in found.iter().zip(got) {
            assert_eq!(got.as_ref(), Ok(want), "{c:?}");
        }
        assert_eq!(
            missing,
            [Err(io::ErrorKind::NotFound), Err(io::ErrorKind::NotFound)]
        );
    }

    #[test]
    fn store_app_aliases_are_found_on_path() {
        // pwsh or python from the Microsoft Store is a reparse point in
        // WindowsApps that reads back as no file without std's fallback.
        let apps = std::env::var_os("LOCALAPPDATA")
            .map(|d| Path::new(&d).join(r"Microsoft\WindowsApps"))
            .unwrap_or_default();
        let alias = std::fs::read_dir(&apps)
            .into_iter()
            .flatten()
            .flatten()
            .find(|e| {
                e.file_type().is_ok_and(|t| !t.is_dir())
                    && e.path()
                        .extension()
                        .is_some_and(|x| x.eq_ignore_ascii_case("exe"))
            });
        let Some(alias) = alias else {
            eprintln!("SKIPPED: no app execution alias in {}", apps.display());
            return;
        };
        let name = alias.file_name().to_string_lossy().into_owned();
        let var = |k: &str| (k == "PATH").then(|| apps.clone().into_os_string());
        assert_eq!(find_program(&name, var).ok().flatten(), Some(alias.path()));
    }

    #[test]
    fn pane_tokens_are_random_hex() {
        let (a, b) = (pane_token().unwrap(), pane_token().unwrap());
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()), "{a}");
        assert_ne!(a, b);
    }
}
