//! Compiles the HLSL shaders to DXBC in OUT_DIR, so the app never has to
//! load d3dcompiler_47.dll at run time, and links each exe's resources:
//! version information, a manifest and, for blitz, the app icon.

/// Each exe, its description in Explorer and Task Manager, and whether it
/// carries the app icon.
const BINS: [(&str, &str, bool); 2] = [
    ("blitz", "blitz", true),
    ("blitz-hook", "blitz hook for Claude Code", false),
];

/// Runs as the user who started it, never elevated, and on Windows 10 and
/// 11 without the compatibility shims older programs get.
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
</assembly>
"#;

fn main() {
    println!("cargo::rerun-if-changed=src/render/shader.hlsl");
    println!("cargo::rerun-if-changed=icon/blitz.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        #[cfg(windows)]
        shaders::compile();
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
            let ico = std::fs::read("icon/blitz.ico").expect("read blitz.ico");
            let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
            for (bin, about, icon) in BINS {
                let mut res = Vec::new();
                // A .res file starts with an empty entry.
                entry(&mut res, 0, 0, 0, &[]);
                if icon {
                    icons(&mut res, &ico);
                }
                entry(&mut res, 16, 1, 0x30, &version_info(bin, about, &version)); // RT_VERSION
                entry(&mut res, 24, 1, 0x30, MANIFEST.as_bytes()); // RT_MANIFEST
                let path = out.join(format!("{bin}.res"));
                std::fs::write(&path, res).expect("write .res");
                // link.exe takes a compiled resource file like an object
                // file. Saves needing rc.exe.
                println!("cargo::rustc-link-arg-bin={bin}={}", path.display());
            }
        }
    }
}

/// Adds a resource to a compiled resource file: a 32-byte header with a
/// numeric type and name, then the data padded to 4 bytes.
fn entry(res: &mut Vec<u8>, kind: u16, name: u16, flags: u16, data: &[u8]) {
    for v in [
        data.len() as u32,
        32,
        0xffff | (kind as u32) << 16,
        0xffff | (name as u32) << 16,
        0,
    ] {
        res.extend(v.to_le_bytes());
    }
    res.extend(flags.to_le_bytes());
    res.extend(0x0409u16.to_le_bytes());
    res.extend([0; 8]);
    res.extend(data);
    res.resize(res.len().next_multiple_of(4), 0);
}

/// Adds the icons of `ico` as icon group 1, the id the app loads its
/// window icon from.
fn icons(res: &mut Vec<u8>, ico: &[u8]) {
    let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([ico[i], ico[i + 1], ico[i + 2], ico[i + 3]]);
    let count = u16_at(4);
    let mut group = [0, 0, 1, 0].to_vec();
    group.extend(count.to_le_bytes());
    for i in 0..count {
        let e = 6 + 16 * usize::from(i);
        let (len, at) = (u32_at(e + 8), u32_at(e + 12) as usize);
        entry(res, 3, i + 1, 0x1010, &ico[at..at + len as usize]); // RT_ICON
        // The directory entry, with the image's offset swapped for its id.
        group.extend(&ico[e..e + 12]);
        group.extend((i + 1).to_le_bytes());
    }
    entry(res, 14, 1, 0x1030, &group); // RT_GROUP_ICON
}

/// The VS_VERSIONINFO of exe `bin`, which Explorer's Details tab and
/// Task Manager show: `version` as numbers and as text, in US English.
fn version_info(bin: &str, about: &str, version: &str) -> Vec<u8> {
    // A pre-release or build suffix is not a number; each number has 16
    // bits of its own.
    let n: Vec<u32> = (version.split(['.', '-', '+']).take(3))
        .map(|p| u32::from(p.parse::<u16>().expect("a crate version part of 0..=65535")))
        .collect();
    let (ms, ls) = (n[0] << 16 | n[1], n[2] << 16);
    let fixed: Vec<u8> = [
        0xfeef_04bd, // signature
        0x0001_0000, // structure version
        ms,
        ls,
        ms,
        ls,
        0x3f,        // the flag bits that mean something
        0,           // and none set
        0x0004_0004, // VOS_NT_WINDOWS32
        1,           // VFT_APP
        0,
        0,
        0,
    ]
    .iter()
    .flat_map(|v: &u32| v.to_le_bytes())
    .collect();
    let exe = format!("{bin}.exe");
    let strings: Vec<Vec<u8>> = [
        ("CompanyName", "blitz contributors"),
        ("FileDescription", about),
        ("FileVersion", version),
        ("InternalName", bin),
        (
            "LegalCopyright",
            "Copyright (c) 2026 blitz contributors. MIT License.",
        ),
        ("OriginalFilename", &exe),
        ("ProductName", "blitz"),
        ("ProductVersion", version),
    ]
    .iter()
    .map(|(k, v)| {
        let text: Vec<u8> = (v.encode_utf16().chain([0]))
            .flat_map(u16::to_le_bytes)
            .collect();
        // Text values give their length in characters.
        node(k, &text, text.len() / 2, true, &[])
    })
    .collect();
    let table = node("040904B0", &[], 0, true, &strings);
    let file_info = node("StringFileInfo", &[], 0, true, &[table]);
    // English (US), Unicode.
    let lang = 0x04b0_0409u32.to_le_bytes();
    let var = node("Translation", &lang, lang.len(), false, &[]);
    let var_info = node("VarFileInfo", &[], 0, true, &[var]);
    node(
        "VS_VERSION_INFO",
        &fixed,
        fixed.len(),
        false,
        &[file_info, var_info],
    )
}

/// One block of version information: its length, the value's length, its
/// type (1 for text), the key, then the value and the children, each
/// starting on a 4-byte boundary.
fn node(key: &str, value: &[u8], value_len: usize, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0, 0];
    b.extend((value_len as u16).to_le_bytes());
    b.extend(u16::from(text).to_le_bytes());
    b.extend(key.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
    for part in std::iter::once(value).chain(children.iter().map(Vec::as_slice)) {
        if !part.is_empty() {
            b.resize(b.len().next_multiple_of(4), 0);
            b.extend(part);
        }
    }
    let len = b.len() as u16;
    b[..2].copy_from_slice(&len.to_le_bytes());
    b
}

#[cfg(windows)]
mod shaders {
    use std::path::PathBuf;

    use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
    use windows::Win32::Graphics::Direct3D::{ID3DBlob, ID3DInclude};
    use windows::core::{PCSTR, s};

    pub fn compile() {
        let src = std::fs::read("src/render/shader.hlsl").expect("read shader.hlsl");
        let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
        let stages = [
            (s!("vs_main"), s!("vs_4_0"), "shader.vs.dxbc"),
            (s!("ps_main"), s!("ps_4_0"), "shader.ps.dxbc"),
        ];
        for (entry, target, file) in stages {
            let dxbc = compile_one(&src, entry, target);
            std::fs::write(out.join(file), dxbc).expect("write dxbc");
        }
    }

    fn compile_one(src: &[u8], entry: PCSTR, target: PCSTR) -> Vec<u8> {
        let mut code = None;
        let mut errors = None;
        // SAFETY: `src` outlives the call and both out-pointers are valid.
        let r = unsafe {
            D3DCompile(
                src.as_ptr().cast(),
                src.len(),
                s!("shader.hlsl"),
                None,
                None::<&ID3DInclude>,
                entry,
                target,
                D3DCOMPILE_OPTIMIZATION_LEVEL3,
                0,
                &mut code,
                Some(&mut errors),
            )
        };
        if let Err(e) = r {
            let log = errors.map(|b| bytes(&b)).unwrap_or_default();
            panic!(
                "shader compile failed: {e}\n{}",
                String::from_utf8_lossy(&log)
            );
        }
        bytes(&code.expect("D3DCompile returned no code"))
    }

    fn bytes(b: &ID3DBlob) -> Vec<u8> {
        // SAFETY: the blob owns `GetBufferSize()` bytes at `GetBufferPointer()`.
        unsafe { std::slice::from_raw_parts(b.GetBufferPointer().cast(), b.GetBufferSize()) }
            .to_vec()
    }
}
