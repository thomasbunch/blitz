//! Compiles the HLSL shaders to DXBC in OUT_DIR, so the app never has to
//! load d3dcompiler_47.dll at run time, and links the app icon.

fn main() {
    println!("cargo::rerun-if-changed=src/render/shader.hlsl");
    println!("cargo::rerun-if-changed=icon/blitz.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        #[cfg(windows)]
        shaders::compile();
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
            let res = out.join("icon.res");
            let ico = std::fs::read("icon/blitz.ico").expect("read blitz.ico");
            std::fs::write(&res, icon_res(&ico)).expect("write icon.res");
            // link.exe takes a compiled resource file like an object file.
            println!("cargo::rustc-link-arg-bin=blitz={}", res.display());
        }
    }
}

/// A compiled resource file holding the icons of `ico` as icon group 1,
/// the id the app loads its window icon from. Saves needing rc.exe.
fn icon_res(ico: &[u8]) -> Vec<u8> {
    let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([ico[i], ico[i + 1], ico[i + 2], ico[i + 3]]);
    let mut res = Vec::new();
    // Each entry: a 32-byte header with a numeric type and name, then the
    // data padded to 4 bytes. A .res file starts with an empty entry.
    let mut entry = |kind: u16, name: u16, flags: u16, data: &[u8]| {
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
    };
    entry(0, 0, 0, &[]);
    let count = u16_at(4);
    let mut group = [0, 0, 1, 0].to_vec();
    group.extend(count.to_le_bytes());
    for i in 0..count {
        let e = 6 + 16 * usize::from(i);
        let (len, at) = (u32_at(e + 8), u32_at(e + 12) as usize);
        entry(3, i + 1, 0x1010, &ico[at..at + len as usize]); // RT_ICON
        // The directory entry, with the image's offset swapped for its id.
        group.extend(&ico[e..e + 12]);
        group.extend((i + 1).to_le_bytes());
    }
    entry(14, 1, 0x1030, &group); // RT_GROUP_ICON
    res
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
