//! Windows executable resources: the .exe icon (Explorer/taskbar) from the
//! shipped brand pack (`assets/brand/ico/ebbtop.ico`: hinted 16-256 px frames
//! from the Ledger mark; see `assets/brand/BRAND.md`), plus the VERSIONINFO
//! metadata block. Same mark as the window icon in `src/main.rs`
//! (`assets/ebbtop.rgba`, 64x64 RGBA, top-down).
//!
//! VERSIONINFO is not cosmetic here: SignPath's artifact metadata restrictions
//! require a product name and product version on every binary that enters a
//! signing request, so `ProductName`/`ProductVersion` are pinned to "EbbTop"
//! and the crate version instead of winresource's defaults (the package name
//! is `ebbtop-gui`, which is not the shipped product name). `CompanyName` is
//! the project's own author identity (Ebbwater, as in the repository URL and
//! LICENSE), not the certificate publisher: the Authenticode publisher on a
//! signed build is SignPath Foundation, and Windows shows the two in separate
//! places (Details tab vs. Digital Signatures).
//!
//! Build-only: `winresource` links the `.ico` and the version block via
//! `llvm-windres`/`windres`. No runtime code changes and no image-decoding
//! dependency.
//!
//! Gate: `CARGO_CFG_TARGET_OS == "windows"` (the *target* cfg, set by cargo
//! even when cross-compiling Linux -> Windows). Do NOT use
//! `#[cfg(windows)]`/`cfg!(windows)` here: a build script compiles for the
//! host, so `cfg!(windows)` is false under Linux cross-compiles and the
//! shipped cross-built exe would stay iconless. On Linux/macOS native
//! builds this returns immediately (no windres invocation, no exe change).
//!
//! Failure mode: a missing `.ico` ships without the icon (the window icon
//! still applies). A failed resource step (no windres/llvm-windres on PATH,
//! e.g. plain `cargo check --target *-windows-gnu` without llvm-mingw) warns
//! and continues, because a metadata-less exe is still usable for a
//! type-check. `deploy/cross-build-windows.sh` and the hosted release
//! workflow export `EBBTOP_REQUIRE_VERSIONINFO=1`, which turns that warning
//! into a build failure, and they re-read the metadata from the built PE
//! (`deploy/windows/versioninfo.py`) before anything is packaged.

use std::env;
use std::path::PathBuf;

/// Product name on every release binary. SignPath rejects a signing request
/// whose artifact lacks the product name or product version attributes.
const PRODUCT_NAME: &str = "EbbTop";
/// Author identity for the Details tab. The certificate publisher on a signed
/// build is SignPath Foundation and is shown by Windows separately.
const COMPANY_NAME: &str = "Ebbwater";
const FILE_DESCRIPTION: &str = "EbbTop desktop client";
const ORIGINAL_FILENAME: &str = "ebbtop.exe";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/brand/ico/ebbtop.ico");
    println!("cargo:rerun-if-env-changed=EBBTOP_REQUIRE_VERSIONINFO");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by cargo"));
    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION set by cargo");
    let require = env::var("EBBTOP_REQUIRE_VERSIONINFO").as_deref() == Ok("1");

    let mut res = winresource::WindowsResource::new();
    res.set("ProductName", PRODUCT_NAME);
    res.set("FileDescription", FILE_DESCRIPTION);
    res.set("OriginalFilename", ORIGINAL_FILENAME);
    res.set("CompanyName", COMPANY_NAME);
    // winresource already defaults these to CARGO_PKG_VERSION; set them
    // explicitly so the block cannot drift with a crate default.
    res.set("FileVersion", &version);
    res.set("ProductVersion", &version);

    let ico_path = manifest_dir.join("assets/brand/ico/ebbtop.ico");
    if ico_path.is_file() {
        res.set_icon(ico_path.to_str().expect("manifest dir is valid UTF-8"));
    } else {
        println!(
            "cargo:warning=ebbtop build.rs: {} not found; shipping without exe icon (VERSIONINFO still embedded)",
            ico_path.display()
        );
    }

    if let Err(e) = res.compile() {
        let msg = format!(
            "ebbtop build.rs: winresource failed: {e} (put llvm-mingw on PATH: ~/.local/opt/llvm-mingw, or set WINDRES/AR)"
        );
        if require {
            panic!("{msg}; EBBTOP_REQUIRE_VERSIONINFO=1 forbids shipping without VERSIONINFO");
        }
        println!("cargo:warning={msg}; shipping without exe icon or VERSIONINFO");
    }
}
