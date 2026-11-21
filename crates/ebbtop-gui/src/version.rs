//! This build's version, compiled into every build.
//!
//! The updater lives behind the `store` feature gate in `update.rs`; the
//! Store package is sealed and has no updater at all, but About and the
//! support summary still have to report the running version, so the version
//! string cannot live in the update module.

/// `CARGO_PKG_VERSION` of this build (workspace version).
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
