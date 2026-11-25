use std::env;
use std::path::{Path, PathBuf};

fn find_windows_kits_root() -> PathBuf {
    if let Ok(root) = env::var("WINDOWSSDKDIR") {
        let p = PathBuf::from(root);
        if p.exists() {
            return p;
        }
    }
    PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10")
}

fn find_latest_iddcx_lib(kits_root: &Path, arch: &str, iddcx_ver: &str) -> PathBuf {
    let lib_root = kits_root.join("Lib");
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&lib_root) {
        for e in entries.flatten() {
            let p = e
                .path()
                .join("um")
                .join(arch)
                .join("iddcx")
                .join(iddcx_ver)
                .join("iddcxstub.lib");
            if p.exists() {
                candidates.push(p);
            }
        }
    }
    candidates.sort();
    candidates
        .pop()
        .unwrap_or_else(|| panic!("Could not find iddcxstub.lib for IddCx {iddcx_ver} under {}", lib_root.display()))
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=WINDOWSSDKDIR");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    let arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default().as_str() {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => panic!("Unsupported target arch for UMDF/IddCx driver: {other}"),
    };

    let kits_root = find_windows_kits_root();
    let wdf_lib_dir = kits_root.join("Lib").join("wdf").join("umdf").join(arch).join("2.33");
    assert!(
        wdf_lib_dir.join("WdfDriverStubUm.lib").exists(),
        "WdfDriverStubUm.lib not found at {}",
        wdf_lib_dir.display()
    );

    let iddcx_lib_path = find_latest_iddcx_lib(&kits_root, arch, "1.10");
    let iddcx_lib_dir = iddcx_lib_path.parent().unwrap();

    println!("cargo:rustc-link-search=native={}", wdf_lib_dir.display());
    println!("cargo:rustc-link-search=native={}", iddcx_lib_dir.display());

    // Link WDK UMDF 2.33 & IddCx 1.10 stubs + Win32/DXGI/D3D11/NT libraries.
    // Using +whole-archive or explicit /INCLUDE ensures the linker retains the
    // .kmdfclassbind$d section entries (`_IddCx_BIND_INFO`) and `FxDriverEntryUm`.
    println!("cargo:rustc-link-lib=static=WdfDriverStubUm");
    println!("cargo:rustc-link-lib=static=iddcxstub");
    println!("cargo:rustc-link-lib=dylib=ntdll");
    println!("cargo:rustc-link-lib=dylib=d3d11");
    println!("cargo:rustc-link-lib=dylib=dxgi");
    println!("cargo:rustc-link-lib=dylib=kernel32");
    println!("cargo:rustc-link-lib=dylib=avrt");

    // Export FxDriverEntryUm (required by WUDFHost.exe) and force retention of _IddCx_BIND_INFO.
    println!("cargo:rustc-cdylib-link-arg=/EXPORT:FxDriverEntryUm");
    println!("cargo:rustc-cdylib-link-arg=/INCLUDE:_IddCx_BIND_INFO");
    println!("cargo:rustc-cdylib-link-arg=/INCLUDE:FxDriverEntryUm");
}
