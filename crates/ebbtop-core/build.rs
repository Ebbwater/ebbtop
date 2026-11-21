//! Windows VERSIONINFO metadata for `ebbtop-probe.exe` (the headless
//! diagnostics binary in `src/bin/ebbtop-probe.rs`).
//!
//! A packaged Windows build needs a product name and product version on every
//! binary, which is what the Microsoft Store packaging expects, so the probe
//! carries the same `ProductName`/`ProductVersion` as the GUI and only
//! `FileDescription`/`OriginalFilename` differ. `CompanyName` is the project's
//! author identity (Ebbwater), which is what Windows shows on the Details tab.
//!
//! Build-only: `winresource` hands a small `.rc` to `llvm-windres`/`windres`
//! and links the resulting `.o`. Nothing is added to the crate's runtime
//! dependency graph.
//!
//! Gate: `CARGO_CFG_TARGET_OS == "windows"` (the *target* cfg, so it is
//! correct while cross-compiling Linux -> Windows; `cfg!(windows)` would be
//! the host and would never fire). Every Linux/macOS build of `ebbtop-core`
//! returns from `main` immediately and is byte-for-byte unaffected.
//!
//! Failure mode: a host without a resource compiler (plain `cargo check
//! --target *-windows-gnu`) warns and continues so type-checking still works.
//! `deploy/cross-build-windows.sh` and the hosted release workflow export
//! `EBBTOP_REQUIRE_VERSIONINFO=1`, which turns that warning into a build
//! failure, and they re-read the metadata from the built PE
//! (`deploy/windows/versioninfo.py`) before anything is packaged.

use std::env;

/// Product name on every release binary. Windows packaging rejects a package
/// whose binaries lack a product name or product version attribute.
const PRODUCT_NAME: &str = "EbbTop";
/// Author identity for the Details tab.
const COMPANY_NAME: &str = "Ebbwater";
const FILE_DESCRIPTION: &str = "EbbTop connection probe";
const ORIGINAL_FILENAME: &str = "ebbtop-probe.exe";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=EBBTOP_REQUIRE_VERSIONINFO");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

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

    if let Err(e) = res.compile() {
        let msg = format!(
            "ebbtop-core build.rs: winresource failed: {e} (put llvm-mingw on PATH: ~/.local/opt/llvm-mingw, or set WINDRES/AR)"
        );
        if require {
            panic!("{msg}; EBBTOP_REQUIRE_VERSIONINFO=1 forbids shipping without VERSIONINFO");
        }
        println!("cargo:warning={msg}; shipping without VERSIONINFO");
    }
}
