//! Windows .exe icon (Explorer/taskbar), from the shipped brand pack
//! (`assets/brand/ico/ebbtop.ico`: hinted 16-256 px frames from the Ledger
//! mark; see `assets/brand/BRAND.md`). Same mark as the window icon in
//! `src/main.rs` (`assets/ebbtop.rgba`, 64x64 RGBA, top-down).
//!
//! Build-only: `winresource` links the `.ico` via `llvm-windres`/`windres`.
//! No runtime code changes and no image-decoding dependency.
//!
//! Gate: `CARGO_CFG_TARGET_OS == "windows"` (the *target* cfg, set by cargo
//! even when cross-compiling Linux -> Windows). Do NOT use
//! `#[cfg(windows)]`/`cfg!(windows)` here: a build script compiles for the
//! host, so `cfg!(windows)` is false under Linux cross-compiles and the
//! shipped cross-built exe would stay iconless. On Linux/macOS native
//! builds this returns immediately (no windres invocation, no exe change).
//!
//! Failure mode: if the `.ico` is missing or windres/llvm-windres is
//! unavailable (plain `cargo check --target *-windows-gnu` without llvm-mingw
//! on PATH), we warn and continue WITHOUT the exe icon instead of failing
//! the build; the window icon (egui IconData) still applies.
//! `deploy/cross-build-windows.sh` pins WINDRES/AR to the llvm-mingw
//! toolchain so the icon is embedded there.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/brand/ico/ebbtop.ico");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by cargo"));
    let ico_path = manifest_dir.join("assets/brand/ico/ebbtop.ico");
    if !ico_path.is_file() {
        println!(
            "cargo:warning=ebbtop build.rs: {} not found; shipping without exe icon",
            ico_path.display()
        );
        return;
    }

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico_path.to_str().expect("manifest dir is valid UTF-8"));
    // winresource defaults FileDescription/ProductName from the package name
    // (ebbtop-gui); the shipped binary is ebbtop.exe, so reflect that in
    // Explorer's file properties.
    res.set("FileDescription", "EBBTOP");
    res.set("OriginalFilename", "ebbtop.exe");
    if let Err(e) = res.compile() {
        println!(
            "cargo:warning=ebbtop build.rs: winresource failed: {e}; shipping without exe icon (put llvm-mingw on PATH: ~/.local/opt/llvm-mingw)"
        );
    }
}
