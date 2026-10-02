//! Finding a newer release on GitHub and installing it.
//!
//! Only full releases count: GitHub's latest release skips drafts and
//! prereleases. HTTP goes through Windows' own curl.exe.

use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::w;

const REPO: &str = "thomasbunch/blitz";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The latest release's version when it is newer than this build.
pub fn check() -> Option<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = curl(&["-H", "Accept: application/vnd.github+json", &url]).ok()?;
    let json = crate::hook::Json::parse(std::str::from_utf8(&body).ok()?)?;
    newer(env!("CARGO_PKG_VERSION"), json.get("tag_name")?.as_str()?)
}

/// The version `tag` names when it is newer than `current`, rebuilt from
/// its numbers so nothing else from the tag reaches a URL or a path.
pub fn newer(current: &str, tag: &str) -> Option<String> {
    let (c, t) = (version(current)?, version(tag)?);
    (t > c).then(|| format!("{}.{}.{}", t.0, t.1, t.2))
}

/// `1.2.3` or `v1.2.3`; anything else, prereleases included, is `None`.
fn version(s: &str) -> Option<(u64, u64, u64)> {
    let mut n = s.strip_prefix('v').unwrap_or(s).split('.').map(|p| {
        // Digits only: `parse` also takes a leading `+`.
        p.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| p.parse().ok())
            .flatten()
    });
    let v = (n.next()??, n.next()??, n.next()??);
    n.next().is_none().then_some(v)
}

/// Whether blitz runs from the installer's folder rather than an unzipped
/// copy, which has to be replaced by hand.
pub fn installed() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("unins000.exe").is_file()))
        .unwrap_or(false)
}

/// Opens the latest release's page in the browser.
pub fn open_page() {
    // SAFETY: static strings and no window.
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            w!("https://github.com/thomasbunch/blitz/releases/latest"),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
}

/// Downloads the installer for version `v` (from `newer`), checks it
/// against the release's SHA256SUMS.txt and starts it. The installer
/// closes what is left of blitz, installs, and starts it again.
// ponytail: the checksum comes from the same release, so it catches a bad
// download, not a bad release. Check an Authenticode signer once releases
// are signed.
pub fn install(v: &str) -> Result<(), String> {
    let base = format!("https://github.com/{REPO}/releases/download/v{v}");
    let name = format!("blitz-{v}-windows-x64-setup.exe");
    let sums = curl(&[&format!("{base}/SHA256SUMS.txt")])?;
    let sums = String::from_utf8_lossy(&sums);
    let want = sum_for(&sums, &name).ok_or("the release has no checksum for its installer")?;
    let exe = curl(&["--max-filesize", "64M", &format!("{base}/{name}")])?;
    if !sha256_hex(&exe).is_some_and(|got| got.eq_ignore_ascii_case(want)) {
        return Err("the download does not match its checksum".into());
    }
    // %TEMP% is the user's own, so nobody else can swap the file between
    // the check and the start.
    let dir = std::env::temp_dir().join(format!("blitz-update-{v}"));
    let path = dir.join(&name);
    std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&path, &exe))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Command::new(&path)
        .args([
            "/VERYSILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/relaunch=1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start the installer: {e}"))?;
    Ok(())
}

/// The hex SHA-256 that a `sha256sum` listing gives for file `name`.
fn sum_for<'a>(sums: &'a str, name: &str) -> Option<&'a str> {
    sums.lines().find_map(|l| {
        let (hex, file) = l.trim_end().split_once(' ')?;
        // ` name` in text mode, `*name` in binary mode.
        (file.trim_start_matches([' ', '*']) == name && hex.len() == 64).then_some(hex)
    })
}

fn sha256_hex(data: &[u8]) -> Option<String> {
    let mut out = [0u8; 32];
    // SAFETY: valid slices; the pseudo-handle needs no setup.
    unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out) }
        .ok()
        .ok()?;
    Some(out.iter().map(|b| format!("{b:02x}")).collect())
}

/// Runs curl.exe over HTTPS only, redirects included, and returns what it
/// downloaded. A bare name finds System32's copy: Rust searches blitz's
/// own folder, then System32, before PATH, and never the current directory.
// ponytail: curl ignores the system proxy; WinHTTP if that bites.
fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let ua = concat!("blitz/", env!("CARGO_PKG_VERSION"));
    let out = Command::new("curl.exe")
        .args(["-fsSL", "--proto", "=https", "--proto-redir", "=https"])
        .args(["--max-time", "120", "-A", ua])
        .args(args)
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(crate::hook::one_line(err.trim()));
    }
    Ok(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_higher_full_release_is_newer() {
        assert_eq!(newer("0.0.1", "v0.0.2").as_deref(), Some("0.0.2"));
        assert_eq!(newer("0.0.9", "v0.0.10").as_deref(), Some("0.0.10"));
        assert_eq!(newer("0.9.9", "1.0.0").as_deref(), Some("1.0.0"));
        for tag in [
            "v0.0.1",
            "v0.0.0",
            "v0.0.2-rc.1",
            "v0.0.2.1",
            "v0.0",
            "v+0.0.2",
            "v0.0.2/../x",
            "",
            "v99999999999999999999.0.0",
        ] {
            assert_eq!(newer("0.0.1", tag), None, "{tag:?}");
        }
        assert_eq!(newer("0.0.1", "v00.0.002").as_deref(), Some("0.0.2"));
    }

    #[test]
    fn checksum_lines() {
        let a = "9481ceb1a4ca53426744bc64bd7ac424bda929775334bd604336db3f5865795c";
        let b = "c93f8707e882761541a414f18fd906564be5783da11660cb7a0671b8499aa844";
        let sums = format!("{a} *blitz-0.0.1-windows-x64-setup.exe\n{b}  blitz-0.0.1.zip\r\n");
        assert_eq!(sum_for(&sums, "blitz-0.0.1-windows-x64-setup.exe"), Some(a));
        assert_eq!(sum_for(&sums, "blitz-0.0.1.zip"), Some(b));
        assert_eq!(sum_for(&sums, "blitz-0.0.1"), None);
        assert_eq!(sum_for("abc *x.exe", "x.exe"), None, "short hash");
    }

    #[test]
    fn sha256_known_answer() {
        assert_eq!(
            sha256_hex(b"abc").as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
    }
}
