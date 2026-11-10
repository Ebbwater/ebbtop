//! Hot-updater core logic (spec: `docs/superpowers/specs/2026-11-10-hot-updater-design.md`).
//!
//! Pure, platform-independent pieces of the update check: semver compare
//! (leading `v` stripped, unequal lengths tolerated), `SHA256SUMS`
//! parsing, hex-digest comparison, and the per-platform asset pick
//! (Windows `ebbtop.exe`, macOS `EbbTop.app.zip`). Network, download,
//! apply, and restart live in the GUI crate; this module never touches
//! the running install.

use std::cmp::Ordering;

/// Release asset served for Windows installs.
pub const WINDOWS_ASSET: &str = "ebbtop.exe";
/// Release asset served for macOS installs.
pub const MACOS_ASSET: &str = "EbbTop.app.zip";

/// One parsed `SHA256SUMS` line: `<hex>  <filename>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sha256Entry {
    pub hash: String,
    pub file: String,
}

pub fn strip_leading_v(s: &str) -> &str {
    s.strip_prefix('v').unwrap_or(s)
}

/// Numeric components of a version string after stripping a leading `v`.
/// Each `.`-separated component contributes its leading ASCII-digit run
/// (`"3-beta"` -> `3`); components without digits contribute `0`, so
/// `"1.2"` parses as `[1, 2]`.
pub fn parse_version(s: &str) -> Vec<u64> {
    strip_leading_v(s.trim())
        .split('.')
        .map(|part| {
            let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<u64>().unwrap_or(0)
        })
        .collect()
}

/// Compare two version strings. Missing trailing components count as
/// zero, so `"1.2" == "1.2.0"` and `"0.2" < "0.2.1"`.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let a = parse_version(a);
    let b = parse_version(b);
    let len = a.len().max(b.len());
    for i in 0..len {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            Ordering::Equal => continue,
            ord => return ord,
        }
    }
    Ordering::Equal
}

/// True when `latest` names a newer release than `current`.
/// Equal or unparsable-as-newer versions mean "no update".
pub fn is_update_available(current: &str, latest: &str) -> bool {
    compare_versions(current, latest) == Ordering::Less
}

/// Parse `SHA256SUMS` text (both `hash  file` and `hash *file` forms).
/// Blank lines are skipped; malformed lines are skipped.
pub fn parse_sha256sums(text: &str) -> Vec<Sha256Entry> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let file = parts.next()?;
            if !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            Some(Sha256Entry {
                hash: hash.to_string(),
                file: file.strip_prefix('*').unwrap_or(file).to_string(),
            })
        })
        .collect()
}

/// Expected hex digest for `asset` from `SHA256SUMS` text, matching the
/// filename exactly (after stripping a binary-mode `*` marker).
pub fn find_hash_for_asset(sums_text: &str, asset: &str) -> Option<String> {
    parse_sha256sums(sums_text)
        .into_iter()
        .find(|entry| entry.file == asset)
        .map(|entry| entry.hash)
}

/// Compare an expected hex digest against an actual one, ignoring case
/// and surrounding whitespace.
pub fn hashes_match(expected_hex: &str, actual_hex: &str) -> bool {
    expected_hex.trim().eq_ignore_ascii_case(actual_hex.trim()) && !expected_hex.trim().is_empty()
}

/// Release asset filename for a platform name (`"windows"`, `"macos"` /
/// `"darwin"`). `None` for anything else.
pub fn asset_name_for_platform(platform: &str) -> Option<&'static str> {
    if platform.eq_ignore_ascii_case("windows") {
        Some(WINDOWS_ASSET)
    } else if platform.eq_ignore_ascii_case("macos") || platform.eq_ignore_ascii_case("darwin") {
        Some(MACOS_ASSET)
    } else {
        None
    }
}

/// Release asset filename for this build's platform, or `None` on
/// platforms the updater does not serve.
pub fn asset_name_for_current_platform() -> Option<&'static str> {
    if cfg!(windows) {
        Some(WINDOWS_ASSET)
    } else if cfg!(target_os = "macos") {
        Some(MACOS_ASSET)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_v_is_stripped_before_compare() {
        assert_eq!(strip_leading_v("v0.2.0"), "0.2.0");
        assert_eq!(strip_leading_v("0.2.0"), "0.2.0");
        assert_eq!(compare_versions("v0.2.0", "0.2.0"), Ordering::Equal);
    }

    #[test]
    fn unequal_lengths_missing_parts_are_zero() {
        assert_eq!(compare_versions("1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(compare_versions("0.2", "0.2.1"), Ordering::Less);
        assert_eq!(compare_versions("0.2.1", "0.2"), Ordering::Greater);
    }

    #[test]
    fn update_available_only_when_latest_is_newer() {
        assert!(is_update_available("0.2.0", "v0.3.0"));
        assert!(!is_update_available("0.3.0", "0.3.0"));
        assert!(!is_update_available("v0.3.0", "0.2.0"));
        assert!(!is_update_available("0.2.0", "0.2"));
    }

    #[test]
    fn prerelease_suffix_does_not_beat_release() {
        // Numeric-prefix parsing: "0.3.0-beta" -> [0, 3, 0].
        assert_eq!(parse_version("0.3.0-beta"), vec![0, 3, 0]);
        assert_eq!(compare_versions("0.3.0-beta", "0.3.0"), Ordering::Equal);
    }

    #[test]
    fn sha256sums_text_and_binary_forms_parse() {
        let text = "abc123  ebbtop.exe\ndef456 *EbbTop.app.zip\n\n";
        let entries = parse_sha256sums(text);
        assert_eq!(
            entries,
            vec![
                Sha256Entry {
                    hash: "abc123".into(),
                    file: "ebbtop.exe".into()
                },
                Sha256Entry {
                    hash: "def456".into(),
                    file: "EbbTop.app.zip".into()
                },
            ]
        );
    }

    #[test]
    fn malformed_sums_lines_are_skipped() {
        let entries = parse_sha256sums("not-a-hash-line\n\n600d42  ebbtop.exe\n");
        assert_eq!(
            entries,
            vec![Sha256Entry {
                hash: "600d42".into(),
                file: "ebbtop.exe".into()
            }]
        );
    }

    #[test]
    fn hash_lookup_matches_exact_asset_name() {
        let text = "aa11  ebbtop.exe\nbb22  EbbTop.app.zip\n";
        assert_eq!(
            find_hash_for_asset(text, "ebbtop.exe").as_deref(),
            Some("aa11")
        );
        assert_eq!(
            find_hash_for_asset(text, "EbbTop.app.zip").as_deref(),
            Some("bb22")
        );
        assert_eq!(find_hash_for_asset(text, "ebbtop"), None);
    }

    #[test]
    fn hash_compare_ignores_case_and_whitespace() {
        assert!(hashes_match("  AB12cd  ", "ab12CD"));
        assert!(!hashes_match("ab12", "ab13"));
    }

    #[test]
    fn platform_asset_picks() {
        assert_eq!(asset_name_for_platform("windows"), Some(WINDOWS_ASSET));
        assert_eq!(asset_name_for_platform("Windows"), Some(WINDOWS_ASSET));
        assert_eq!(asset_name_for_platform("macos"), Some(MACOS_ASSET));
        assert_eq!(asset_name_for_platform("darwin"), Some(MACOS_ASSET));
        assert_eq!(asset_name_for_platform("linux"), None);
        assert_eq!(asset_name_for_platform(""), None);
    }

    #[test]
    fn current_platform_asset_agrees_with_cfg() {
        if cfg!(windows) {
            assert_eq!(asset_name_for_current_platform(), Some(WINDOWS_ASSET));
        } else if cfg!(target_os = "macos") {
            assert_eq!(asset_name_for_current_platform(), Some(MACOS_ASSET));
        } else {
            assert_eq!(asset_name_for_current_platform(), None);
        }
    }
}
