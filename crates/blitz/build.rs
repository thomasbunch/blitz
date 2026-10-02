//! Compiles the HLSL shaders to DXBC in OUT_DIR, so the app never has to
//! load d3dcompiler_47.dll at run time.

fn main() {
    println!("cargo::rerun-if-changed=src/render/shader.hlsl");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        #[cfg(windows)]
        shaders::compile();
    }
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
