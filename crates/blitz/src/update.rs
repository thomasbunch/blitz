//! Finding a newer release on GitHub and installing it.
//!
//! Only full releases count: GitHub's latest release skips drafts and
//! prereleases. HTTP goes through Windows' own curl.exe, by way of the
//! proxy Windows is set to use.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use windows::Win32::Foundation::{GlobalFree, HGLOBAL};
use windows::Win32::Networking::WinHttp::{
    WINHTTP_CURRENT_USER_IE_PROXY_CONFIG, WinHttpGetIEProxyConfigForCurrentUser,
};
use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PWSTR, w};

const REPO: &str = "thomasbunch/blitz";
/// The latest release's page.
pub const PAGE: &str = "https://github.com/thomasbunch/blitz/releases/latest";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The latest release's version when it is newer than this build, or why
/// GitHub could not say.
pub fn check() -> Result<Option<String>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    // Someone is waiting on this one, so it gives up sooner.
    let accept = "Accept: application/vnd.github+json";
    let body = curl(&["--max-time", "30", "-H", accept, &url])?;
    latest(&body, env!("CARGO_PKG_VERSION"))
}

/// The version in GitHub's latest-release `body` when it is newer than
/// `current`.
fn latest(body: &[u8], current: &str) -> Result<Option<String>, String> {
    let json = std::str::from_utf8(body)
        .ok()
        .and_then(crate::hook::Json::parse);
    let tag = json.as_ref().and_then(|j| j.get("tag_name")?.as_str());
    Ok(newer(current, tag.ok_or("GitHub sent no release")?))
}

/// The version `tag` names when it is newer than `current`, rebuilt from
/// its numbers so nothing else from the tag reaches a URL or a path.
pub fn newer(current: &str, tag: &str) -> Option<String> {
    let (c, t) = (version(current)?, version(tag)?);
    (t > c).then(|| format!("{}.{}.{}", t.0, t.1, t.2))
}

/// `1.2.3` or `v1.2.3`; anything else, prereleases included, is `None`.
/// No leading zeros, as in semver: the version rebuilt from the numbers
/// must name the same tag.
fn version(s: &str) -> Option<(u64, u64, u64)> {
    let mut n = s.strip_prefix('v').unwrap_or(s).split('.').map(|p| {
        // Digits only: `parse` also takes a leading `+`.
        let digits = p.bytes().all(|b| b.is_ascii_digit());
        (digits && (p == "0" || !p.starts_with('0')))
            .then(|| p.parse().ok())
            .flatten()
    });
    let v = (n.next()??, n.next()??, n.next()??);
    n.next().is_none().then_some(v)
}

/// The banner text for release `v`, or for the update to it that failed
/// and wrote `log`; `installed` when blitz can update itself, and `keys`
/// what to press to update. `None` keeps
/// the banner shown now, `shown`: a later look finding the same release
/// keeps a failure in view. The action comes before the log's long path,
/// so a narrow window that cuts the text keeps it.
pub fn banner(
    shown: Option<&(String, String)>,
    v: &str,
    log: Option<&Path>,
    installed: bool,
    keys: &str,
) -> Option<String> {
    if log.is_none() && shown.is_some_and(|u| u.0 == v) {
        return None;
    }
    Some(match log {
        Some(log) => format!(
            "Updating to blitz {v} failed \u{b7} {keys} to try again \u{b7} log: {}",
            log.display()
        ),
        None => {
            let how = if installed {
                "update and restart"
            } else {
                "open the download page"
            };
            format!("blitz {v} is available \u{b7} {keys} to {how}")
        }
    })
}

/// The banner text once release `v` is to install when blitz closes;
/// `keys` update now.
pub fn at_close(v: &str, keys: &str) -> String {
    format!("blitz {v} installs when you close blitz \u{b7} {keys} to update and restart now")
}

/// What Ctrl+Shift+U says before an update to `v` that would end `busy`
/// sessions or close `others` other blitz windows, or `None` to go ahead;
/// and whether busy sessions leave `v` to install when blitz closes. Only
/// the `main` window can, when it is the last one, as the installer would
/// close the others. `again` says how to confirm it.
pub fn confirm(
    v: &str,
    busy: usize,
    others: usize,
    main: bool,
    again: &str,
) -> Option<(String, bool)> {
    let what = match (busy, others) {
        (0, 0) => return None,
        (0, _) => "Updating",
        (1, _) => "A session is busy",
        _ => "Sessions are busy",
    };
    if busy > 0 && others == 0 && main {
        let text =
            format!("{what}, so blitz {v} installs when you close blitz. {again} to restart now");
        return Some((text, true));
    }
    let and = if busy > 0 { ", and updating" } else { "" };
    let closes = match others {
        0 => String::new(),
        1 => " and closes another blitz window".into(),
        n => format!(" and closes {n} other blitz windows"),
    };
    Some((
        format!("{what}{and} restarts blitz{closes}. {again}"),
        false,
    ))
}

/// What Ctrl+Shift+U says when it looked for a release itself.
pub fn found(r: &Result<Option<String>, String>) -> String {
    match r {
        Ok(Some(v)) => format!("blitz {v} is available"),
        Ok(None) => format!("blitz {} is up to date", env!("CARGO_PKG_VERSION")),
        Err(e) => format!("Could not look for an update: {e}"),
    }
}

/// Whether blitz runs from the installer's folder rather than an unzipped
/// copy, which has to be replaced by hand.
pub fn installed() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("unins000.exe").is_file()))
        .unwrap_or(false)
}

/// The page of release `v` (from `newer`), with its notes.
pub fn notes(v: &str) -> String {
    format!("https://github.com/{REPO}/releases/tag/v{v}")
}

/// Opens a web page in the browser; false if it could not.
pub fn open(url: &str) -> bool {
    // SAFETY: valid strings and no window.
    let h = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(url),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    // Above 32 is success.
    h.0 as isize > 32
}

/// The page for a new issue about blitz, its text ending in `facts` as
/// a list of `name: value`. A word with a backslash, a file or folder
/// that could hold the user's name, is left out.
pub fn issue(facts: &[(&str, &str)]) -> String {
    let mut body =
        String::from("<!-- What happened, and how can it be made to happen again? -->\n\n\n");
    for (name, value) in facts {
        let words: Vec<&str> = (value.split(' '))
            .map(|w| if w.contains('\\') { "(path)" } else { w })
            .collect();
        body += &format!("- {name}: {}\n", words.join(" "));
    }
    let body: String = (body.bytes())
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("https://github.com/{REPO}/issues/new?body={body}")
}

/// Where the release whose banner was closed is kept.
const DISMISSED: &str = "update-dismissed";

/// The release whose banner was closed, as kept in `dir`.
pub fn dismissed_in(dir: &Path) -> Option<String> {
    let v = std::fs::read_to_string(dir.join(DISMISSED)).ok()?;
    version(v.trim())?;
    Some(v.trim().into())
}

/// Keeps `v` as the release whose banner was closed, or with `None`
/// forgets it. A failed save only shows the banner again.
pub fn dismiss_in(dir: &Path, v: Option<&str>) {
    let file = dir.join(DISMISSED);
    match v {
        Some(v) => {
            let _ = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(file, v));
        }
        None => {
            let _ = std::fs::remove_file(file);
        }
    }
}

/// Whether the banner shows release `v` that a look found unasked, or
/// whose update `failed`: never one whose banner was `closed` until a
/// newer release comes, and no offer once `checks` are off.
pub fn show_unasked(v: &str, failed: bool, checks: bool, closed: Option<&str>) -> bool {
    (failed || checks) && closed.is_none_or(|d| newer(d, v).is_some())
}

/// The newest update that `install` started and that did not happen, with
/// the installer's log: its folder is for a version newer than this build.
/// The folders of updates to this version or older are removed, and so are
/// those of updates whose installer never started.
pub fn failed() -> Option<(String, PathBuf)> {
    failed_in(
        &std::env::temp_dir(),
        env!("CARGO_PKG_VERSION"),
        SystemTime::now(),
    )
}

/// How long an update's folder may wait for its installer to start, as
/// another blitz may be about to start it.
const UNSTARTED: Duration = Duration::from_secs(10 * 60);

fn failed_in(temp: &Path, current: &str, now: SystemTime) -> Option<(String, PathBuf)> {
    let mut out: Option<((u64, u64, u64), String, PathBuf)> = None;
    for e in std::fs::read_dir(temp).ok()?.flatten() {
        let name = e.file_name();
        let Some(v) = name.to_str().and_then(|n| n.strip_prefix("blitz-update-")) else {
            continue;
        };
        if !e.path().is_dir() {
            continue;
        }
        let Some(v) = newer(current, v) else {
            // Done, or no longer wanted.
            let _ = std::fs::remove_dir_all(e.path());
            continue;
        };
        // No log: the installer has not run, or another blitz is about to
        // start it; once that is long past, it never will. A log that says
        // it worked comes from an install this older copy of blitz did not
        // do.
        let log = e.path().join("setup.log");
        if !log.is_file() {
            let age = (e.metadata().and_then(|m| m.modified()).ok())
                .and_then(|t| now.duration_since(t).ok());
            if age.is_some_and(|a| a > UNSTARTED) {
                let _ = std::fs::remove_dir_all(e.path());
            }
            continue;
        }
        let worked = std::fs::read(&log)
            .is_ok_and(|b| String::from_utf8_lossy(&b).contains("Installation process succeeded"));
        let Some(n) = version(&v).filter(|_| !worked) else {
            continue;
        };
        if out.as_ref().is_none_or(|o| n > o.0) {
            out = Some((n, v, log));
        }
    }
    out.map(|(_, v, log)| (v, log))
}

/// The installer's file name in release `v`, as `.github/blitz.iss`
/// names it.
fn installer_name(v: &str) -> String {
    format!("blitz-{v}-windows-x64-setup.exe")
}

/// Downloads the installer for version `v` (from `newer`), checks it
/// against the release's SHA256SUMS.txt, and returns where it saved it.
// ponytail: the checksum comes from the same release, so it catches a bad
// download, not a bad release. Check an Authenticode signer once releases
// are signed.
pub fn fetch(v: &str) -> Result<PathBuf, String> {
    let base = format!("https://github.com/{REPO}/releases/download/v{v}");
    let name = installer_name(v);
    let sums = curl(&[&format!("{base}/SHA256SUMS.txt")])?;
    let sums = String::from_utf8_lossy(&sums);
    let want = sum_for(&sums, &name).ok_or("the release has no checksum for its installer")?;
    // A line that trickles just above the stall floor still ends: 64 MB in
    // half an hour is 36 KB/s.
    let url = format!("{base}/{name}");
    let exe = curl(&["--max-time", "1800", "--max-filesize", "64M", &url])?;
    if !sha256_hex(&exe).is_some_and(|got| got.eq_ignore_ascii_case(want)) {
        return Err("the download does not match its checksum".into());
    }
    // %TEMP% is the user's own, so nobody else can swap the file between
    // the check and the start.
    let dir = std::env::temp_dir().join(format!("blitz-update-{v}"));
    let path = dir.join(&name);
    // The error names no path: it can end up in an issue report.
    let saved = std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&path, &exe))
        .map_err(|e| format!("could not save the installer: {e}"));
    // An installer that never ran leaves nothing to report.
    if saved.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    saved.map(|()| path)
}

/// Starts the `installer` that `fetch` saved. It closes what is left of
/// blitz and installs, and with `relaunch` starts blitz again.
pub fn run(installer: &Path, relaunch: bool) -> Result<(), String> {
    // blitz is gone by the time the installer could fail, so the log is
    // what `failed` finds on the next start.
    let log = installer.with_file_name("setup.log");
    let started = Command::new(installer)
        .args(installer_args(&log, relaunch))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
        .map_err(|e| format!("could not start the installer: {e}"));
    // An installer that never ran leaves nothing to report, nor a folder
    // that holds only it.
    if started.is_err() {
        let _ = std::fs::remove_file(installer);
        if let Some(dir) = installer.parent() {
            let _ = std::fs::remove_dir(dir);
        }
    }
    started
}

/// A silent install writing `log`; `.github/blitz.iss` starts blitz again
/// after it when told `/relaunch=1`.
fn installer_args(log: &Path, relaunch: bool) -> Vec<String> {
    let relaunch = relaunch.then_some("/relaunch=1");
    (["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"].into_iter())
        .chain(relaunch)
        .map(String::from)
        .chain([format!("/LOG={}", log.display())])
        .collect()
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

/// Runs System32's curl.exe over HTTPS only, redirects included, and
/// returns what it downloaded. Named by its full path: a bare name is
/// looked for in blitz's own folder first, where a planted curl.exe would
/// run instead.
fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let ua = concat!("blitz/", env!("CARGO_PKG_VERSION"));
    let exe = crate::shell::system_root(|k| std::env::var_os(k))
        .join("System32")
        .join("curl.exe");
    // curl does not read Windows' proxy setting itself.
    let proxy = system_proxy();
    // A slow line still finishes the download; one that stalls gives up.
    let out = Command::new(exe)
        .args(["-fsSL", "--proto", "=https", "--proto-redir", "=https"])
        .args(["--connect-timeout", "20", "--speed-limit", "1000"])
        .args(["--speed-time", "30", "-A", ua])
        .args(proxy.as_deref().map(proxy_args).into_iter().flatten())
        .args(args)
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if !out.status.success() {
        return Err(curl_error(&out.stderr, out.status.code()));
    }
    Ok(out.stdout)
}

/// curl's options to go through `proxy`. One that asks who is there, as a
/// work network's often does, is answered as the Windows user, the way
/// browsers do, and with no password asked for.
fn proxy_args(proxy: &str) -> [&str; 5] {
    ["--proxy", proxy, "--proxy-anyauth", "--proxy-user", ":"]
}

/// The proxy for HTTPS that Windows' proxy settings name, if any.
// ponytail: a fixed proxy only; a setup script or automatic detection
// needs WinHttpGetProxyForUrl, and the bypass list is not read.
fn system_proxy() -> Option<String> {
    let mut ie = WINHTTP_CURRENT_USER_IE_PROXY_CONFIG::default();
    // SAFETY: a struct for the call to fill.
    unsafe { WinHttpGetIEProxyConfigForCurrentUser(&mut ie) }.ok()?;
    let take = |s: PWSTR| {
        if s.is_null() {
            return None;
        }
        // SAFETY: a NUL-terminated string WinHTTP allocated, read once,
        // then freed with GlobalFree as its documentation says.
        unsafe {
            let text = s.to_string().ok();
            let _ = GlobalFree(Some(HGLOBAL(s.0.cast())));
            text
        }
    };
    // Every string is freed, used or not.
    let (list, _, _) = (
        take(ie.lpszProxy),
        take(ie.lpszProxyBypass),
        take(ie.lpszAutoConfigUrl),
    );
    https_proxy(&list?)
}

/// The proxy for HTTPS in a WinHTTP proxy list: `host:port` for every
/// scheme, or entries such as `http=a:80;https=b:443`.
fn https_proxy(list: &str) -> Option<String> {
    let mut all = None;
    for e in list.split([';', ' ']).filter(|e| !e.is_empty()) {
        match e.split_once('=') {
            Some((scheme, p)) if scheme.eq_ignore_ascii_case("https") && !p.is_empty() => {
                return Some(p.into());
            }
            Some(_) => {}
            None => {
                all.get_or_insert_with(|| e.to_string());
            }
        }
    }
    all
}

/// Why curl failed: what it printed, or its exit code when it printed
/// nothing.
fn curl_error(stderr: &[u8], code: Option<i32>) -> String {
    let err = crate::hook::one_line(String::from_utf8_lossy(stderr).trim());
    if !err.is_empty() {
        return err;
    }
    match code {
        Some(c) => format!("curl stopped with code {c}"),
        None => "curl stopped".into(),
    }
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
            // The rebuilt version would name another tag.
            "v00.0.002",
            "v0.01.0",
            "V0.0.2",
            " v0.0.2",
            "v0.0.2\n",
            "vv0.0.2",
            "v-1.0.0",
            "v1..0",
        ] {
            assert_eq!(newer("0.0.1", tag), None, "{tag:?}");
        }
        assert_eq!(newer("0.0.1", "v0.0.10").as_deref(), Some("0.0.10"));
        assert_eq!(newer("0.0.1", "v0.10.0").as_deref(), Some("0.10.0"));
        let max = format!("v{0}.{0}.{0}", u64::MAX);
        assert_eq!(newer("0.0.1", &max), Some(max[1..].to_string()));
        // Never a downgrade, and nothing without a version to compare.
        assert_eq!(newer("0.1.0", "v0.0.9"), None);
        assert_eq!(newer("0.0.2", "v0.0.2"), None);
        assert_eq!(newer("", "v0.0.2"), None);
        assert_eq!(newer("0.0.2-dev", "v0.0.3"), None);
    }

    #[test]
    fn latest_reads_the_tag_or_says_why_not() {
        let tag = |t: &str| format!(r#"{{"tag_name":"{t}","name":"x"}}"#).into_bytes();
        assert_eq!(latest(&tag("v0.0.5"), "0.0.4"), Ok(Some("0.0.5".into())));
        assert_eq!(latest(&tag("v0.0.4"), "0.0.4"), Ok(None));
        assert_eq!(latest(&tag("v0.0.5-rc.1"), "0.0.4"), Ok(None));
        for body in [
            &b"<html>rate limited</html>"[..],
            b"",
            b"{}",
            br#"{"tag_name":5}"#,
            br#"{"tag_name":null}"#,
            b"\xff\xfe",
            br#"{"message":"Not Found"}"#,
        ] {
            assert_eq!(
                latest(body, "0.0.4"),
                Err("GitHub sent no release".into()),
                "{}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn installer_matches_the_setup_script() {
        let iss = include_str!("../../../.github/blitz.iss");
        let base = (iss.lines())
            .find_map(|l| l.strip_prefix("OutputBaseFilename="))
            .expect("OutputBaseFilename");
        let built = format!("{}.exe", base.replace("{#AppVersion}", "1.2.3"));
        assert_eq!(installer_name("1.2.3"), built);
        // Setup starts blitz again only when told to: not after an update
        // left for when blitz closes.
        let log = Path::new(r"C:\t\blitz-update-1.2.3\setup.log");
        let now = installer_args(log, true);
        assert_eq!(
            now,
            [
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/relaunch=1",
                r"/LOG=C:\t\blitz-update-1.2.3\setup.log"
            ]
        );
        let later = installer_args(log, false);
        assert_eq!(later, [&now[..3], &now[4..]].concat());
        assert!(iss.contains("ExpandConstant('{param:relaunch|0}') = '1'"));
        assert!(iss.contains("WizardSilent"));
    }

    /// Setup offers to start blitz at sign-in, for this user, never ticked
    /// for them, and takes it away when unticked or uninstalled.
    #[test]
    fn setup_can_start_blitz_at_sign_in() {
        let iss = include_str!("../../../.github/blitz.iss");
        let task = (iss.lines())
            .find(|l| l.starts_with("Name: startup;"))
            .expect("the task");
        assert!(task.ends_with("Flags: unchecked"), "{task}");
        let run = r#"Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; "#;
        let lines: Vec<&str> = iss.lines().filter_map(|l| l.strip_prefix(run)).collect();
        assert_eq!(
            lines,
            [
                r#"ValueType: string; ValueName: "blitz"; ValueData: """{app}\blitz.exe"""; Flags: uninsdeletevalue; Tasks: startup"#,
                r#"ValueType: none; ValueName: "blitz"; Flags: deletevalue dontcreatekey; Check: not WizardIsTaskSelected('startup')"#,
            ]
        );
    }

    #[test]
    fn the_banner_offers_a_release_and_keeps_a_failure_in_view() {
        let log = Path::new(r"C:\Users\someone\AppData\Local\Temp\blitz-update-0.0.5\setup.log");
        const U: &str = "Ctrl+Shift+U";
        let offer = banner(None, "0.0.5", None, true, U).expect("a banner");
        assert_eq!(
            offer,
            "blitz 0.0.5 is available \u{b7} Ctrl+Shift+U to update and restart"
        );
        let zip = banner(None, "0.0.5", None, false, U).expect("a banner");
        assert!(
            zip.ends_with("Ctrl+Shift+U to open the download page"),
            "{zip}"
        );
        let failed = banner(None, "0.0.5", Some(log), true, U).expect("a banner");
        let action = failed
            .find("Ctrl+Shift+U to try again")
            .expect("the action");
        assert!(
            action < failed.find(r"C:\Users").expect("the log"),
            "{failed}"
        );
        // A later look at the same release keeps the failure; a newer one
        // replaces it, and a failure replaces an offer.
        let shown = ("0.0.5".to_string(), failed.clone());
        assert_eq!(banner(Some(&shown), "0.0.5", None, true, U), None);
        assert!(banner(Some(&shown), "0.0.6", None, true, U).is_some());
        let offered = ("0.0.5".to_string(), offer);
        assert_eq!(
            banner(Some(&offered), "0.0.5", Some(log), true, U),
            Some(failed)
        );
        // The keys are the user's.
        let rebound = banner(None, "0.0.5", None, true, "Alt+F12").expect("a banner");
        assert!(
            rebound.ends_with("Alt+F12 to update and restart"),
            "{rebound}"
        );
    }

    #[test]
    fn a_closed_banner_stays_closed_until_a_newer_release() {
        let dir = std::env::temp_dir().join(format!("blitz-closed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(dismissed_in(&dir), None);
        assert!(show_unasked("0.0.5", false, true, None));
        dismiss_in(&dir, Some("0.0.5"));
        let closed = dismissed_in(&dir);
        assert_eq!(closed.as_deref(), Some("0.0.5"));
        for failed in [false, true] {
            assert!(!show_unasked("0.0.5", failed, true, closed.as_deref()));
            assert!(!show_unasked("0.0.4", failed, true, closed.as_deref()));
            assert!(show_unasked("0.0.6", failed, true, closed.as_deref()));
        }
        // With checks off only a failed update shows.
        assert!(!show_unasked("0.0.6", false, false, closed.as_deref()));
        assert!(show_unasked("0.0.6", true, false, None));
        // Anything else in the file closes nothing.
        std::fs::write(dir.join(DISMISSED), "0.0.5/../x").unwrap();
        assert_eq!(dismissed_in(&dir), None);
        dismiss_in(&dir, Some("0.0.5"));
        dismiss_in(&dir, None);
        assert_eq!(dismissed_in(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_issue_says_what_blitz_runs_on_but_names_no_path() {
        let url = issue(&[
            ("blitz", "0.0.4"),
            ("ConPTY", "the Windows console host"),
            (
                "last update error",
                r"C:\Users\Jo Smith\AppData\Local\Temp\x: Access is denied. (os error 5)",
            ),
        ]);
        let query = url
            .strip_prefix("https://github.com/thomasbunch/blitz/issues/new?body=")
            .expect("the new issue page");
        assert!(
            query
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~%".contains(&b)),
            "{query}"
        );
        // Undo the escapes.
        let mut body = Vec::new();
        let mut bytes = query.bytes();
        while let Some(b) = bytes.next() {
            match b {
                b'%' => {
                    let hex = [bytes.next().unwrap(), bytes.next().unwrap()];
                    let hex = std::str::from_utf8(&hex).unwrap();
                    body.push(u8::from_str_radix(hex, 16).unwrap());
                }
                b => body.push(b),
            }
        }
        let body = String::from_utf8(body).unwrap();
        assert!(body.starts_with("<!-- What happened"), "{body}");
        assert!(
            body.ends_with(
                "\n- blitz: 0.0.4\n- ConPTY: the Windows console host\n\
                 - last update error: (path) (path) Access is denied. (os error 5)\n"
            ),
            "{body}"
        );
    }

    #[test]
    fn the_notes_are_on_the_release_page() {
        assert_eq!(
            notes("0.0.5"),
            "https://github.com/thomasbunch/blitz/releases/tag/v0.0.5"
        );
    }

    #[test]
    fn an_update_that_ends_sessions_or_closes_windows_asks_first() {
        let again = "Press Ctrl+Shift+U again";
        assert_eq!(confirm("0.0.5", 0, 0, true, again), None);
        assert_eq!(confirm("0.0.5", 0, 0, false, again), None);
        for (busy, others, main, text) in [
            (
                1,
                0,
                false,
                "A session is busy, and updating restarts blitz",
            ),
            (
                3,
                0,
                false,
                "Sessions are busy, and updating restarts blitz",
            ),
            (
                0,
                1,
                true,
                "Updating restarts blitz and closes another blitz window",
            ),
            (
                2,
                2,
                true,
                "Sessions are busy, and updating restarts blitz and closes 2 other blitz windows",
            ),
        ] {
            let want = Some((format!("{text}. {again}"), false));
            assert_eq!(confirm("0.0.5", busy, others, main, again), want);
        }
        // Busy sessions in the last window, the main one, can leave it
        // until blitz closes.
        let later = "Sessions are busy, so blitz 0.0.5 installs when you close blitz. \
                     Press Ctrl+Shift+U again to restart now";
        assert_eq!(
            confirm("0.0.5", 2, 0, true, again),
            Some((later.to_string(), true))
        );

        assert_eq!(
            at_close("0.0.5", "Ctrl+Shift+U"),
            "blitz 0.0.5 installs when you close blitz \u{b7} Ctrl+Shift+U to update and restart now"
        );
    }

    #[test]
    fn a_look_by_hand_says_what_it_found() {
        assert_eq!(found(&Ok(Some("9.9.9".into()))), "blitz 9.9.9 is available");
        let now = env!("CARGO_PKG_VERSION");
        assert_eq!(found(&Ok(None)), format!("blitz {now} is up to date"));
        let err = found(&Err("Could not resolve host: api.github.com".into()));
        assert_eq!(
            err,
            "Could not look for an update: Could not resolve host: api.github.com"
        );
    }

    #[test]
    fn curl_errors_always_say_something() {
        let said = b"curl: (6) Could not resolve host: api.github.com\r\n";
        assert_eq!(
            curl_error(said, Some(6)),
            "curl: (6) Could not resolve host: api.github.com"
        );
        assert_eq!(curl_error(b"", Some(28)), "curl stopped with code 28");
        assert_eq!(curl_error(b" \r\n", None), "curl stopped");
    }

    #[test]
    fn the_proxy_for_https_comes_from_the_windows_list() {
        assert_eq!(https_proxy("proxy:8080").as_deref(), Some("proxy:8080"));
        assert_eq!(
            https_proxy("http://proxy.corp:3128").as_deref(),
            Some("http://proxy.corp:3128")
        );
        let split = "http=a:80;https=b:443;ftp=c:21";
        assert_eq!(https_proxy(split).as_deref(), Some("b:443"));
        assert_eq!(
            https_proxy("HTTPS=b:443 http=a:80").as_deref(),
            Some("b:443")
        );
        // A proxy for other schemes only leaves HTTPS direct.
        for list in ["", ";", "http=a:80", "socks=s:1080;ftp=c:21", "https="] {
            assert_eq!(https_proxy(list), None, "{list:?}");
        }
        // Reads, and frees, this machine's own setting.
        let _ = system_proxy();
    }

    /// curl tells a proxy that asks who is there the Windows user, signed
    /// in by Windows itself.
    #[test]
    fn a_proxy_that_asks_is_told_the_windows_user() {
        use std::io::{Read, Write};
        let proxy = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let at = proxy.local_addr().expect("its address").to_string();
        let asked = std::thread::spawn(move || {
            let (mut c, _) = proxy.accept()?;
            c.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
            let (mut seen, mut buf) = (String::new(), [0; 4096]);
            // The first try, and the one that answers the 407.
            for _ in 0..2 {
                let n = c.read(&mut buf)?;
                seen += &String::from_utf8_lossy(&buf[..n]);
                c.write_all(
                    b"HTTP/1.1 407 Proxy Authentication Required\r\n\
                      Proxy-Authenticate: Negotiate\r\nContent-Length: 0\r\n\r\n",
                )?;
            }
            std::io::Result::Ok(seen)
        });
        let _ = Command::new("curl.exe")
            .args(["-sS", "--max-time", "10"])
            .args(proxy_args(&at))
            .arg("https://example.invalid/")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        // Wakes the proxy if curl never came.
        let _ = std::net::TcpStream::connect(&at);
        let seen = asked.join().expect("the proxy").expect("a request");
        assert!(seen.contains("CONNECT example.invalid:443"), "{seen}");
        let auth = "\r\nProxy-Authorization: Negotiate ";
        assert!(seen.contains(auth), "{seen}");
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

    /// Makes `temp` hold these folders, each with a setup.log of the given
    /// text if any, and returns what `failed_in` finds and what is left.
    fn failed_with(
        name: &str,
        dirs: &[(&str, Option<&str>)],
        current: &str,
    ) -> (Option<(String, PathBuf)>, Vec<String>, PathBuf) {
        let temp = std::env::temp_dir().join(format!("blitz-failed-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        for &(dir, log) in dirs {
            std::fs::create_dir_all(temp.join(dir)).unwrap();
            if let Some(text) = log {
                std::fs::write(temp.join(dir).join("setup.log"), text).unwrap();
            }
        }
        // Not a folder: left alone.
        std::fs::write(temp.join("blitz-update-0.0.99"), "").unwrap();
        let got = failed_in(&temp, current, SystemTime::now());
        let mut left: Vec<_> = (std::fs::read_dir(&temp).unwrap().flatten())
            .map(|e| e.file_name().into_string().unwrap())
            .collect();
        left.sort();
        let _ = std::fs::remove_dir_all(&temp);
        (got, left, temp)
    }

    #[test]
    fn a_newer_update_with_a_log_failed() {
        let failed = Some("Setup failed: file in use");
        let (got, left, temp) = failed_with(
            "one",
            &[
                ("blitz-update-0.0.1", failed),
                ("blitz-update-0.0.2", failed),
                ("blitz-update-0.0.3", failed),
                ("blitz-update-0.0.4", None),
                ("other", failed),
            ],
            "0.0.2",
        );
        let log = temp.join("blitz-update-0.0.3").join("setup.log");
        assert_eq!(got, Some(("0.0.3".into(), log)));
        // Updates to this version or older are gone, this one included. One
        // not started yet stays, as another blitz may be about to start it;
        // what is not blitz's stays.
        assert_eq!(
            left,
            [
                "blitz-update-0.0.3",
                "blitz-update-0.0.4",
                "blitz-update-0.0.99",
                "other"
            ]
        );
    }

    #[test]
    fn the_newest_failure_is_the_one_shown() {
        let failed = Some("");
        let (got, _, temp) = failed_with(
            "newest",
            &[
                ("blitz-update-0.0.9", failed),
                ("blitz-update-0.0.10", failed),
                ("blitz-update-0.0.3", failed),
            ],
            "0.0.2",
        );
        let log = temp.join("blitz-update-0.0.10").join("setup.log");
        assert_eq!(got, Some(("0.0.10".into(), log)));
    }

    #[test]
    fn an_install_that_worked_is_no_failure() {
        let worked = "2026-10-07 10:00:05.000   Installation process succeeded.\r\n";
        let (got, left, _) =
            failed_with("worked", &[("blitz-update-0.0.5", Some(worked))], "0.0.2");
        assert_eq!(got, None, "an older copy of blitz is running");
        assert_eq!(left, ["blitz-update-0.0.5", "blitz-update-0.0.99"]);
        let (got, _, _) = failed_with("nothing", &[], "0.0.2");
        assert_eq!(got, None);
        let missing = std::env::temp_dir().join(format!("blitz-no-temp-{}", std::process::id()));
        assert_eq!(failed_in(&missing, "0.0.2", SystemTime::now()), None);
    }

    #[test]
    fn an_update_never_started_goes_in_the_end() {
        let temp = std::env::temp_dir().join(format!("blitz-unstarted-{}", std::process::id()));
        let dir = temp.join("blitz-update-0.0.5");
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&dir).unwrap();
        let after = |s| SystemTime::now() + Duration::from_secs(s);
        // Another blitz may be about to start it.
        let soon = failed_in(&temp, "0.0.2", after(60));
        let kept = dir.exists();
        // One that has not started in ten minutes never will.
        let later = failed_in(&temp, "0.0.2", after(11 * 60));
        let gone = !dir.exists();
        let _ = std::fs::remove_dir_all(&temp);
        assert_eq!((soon, kept, later, gone), (None, true, None, true));
    }

    #[test]
    fn sha256_known_answer() {
        assert_eq!(
            sha256_hex(b"abc").as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
    }
}
