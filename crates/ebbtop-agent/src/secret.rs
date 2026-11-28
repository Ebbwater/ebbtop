//! ebbtop P3a: randomness for the boot nonce, pairing codes and client tokens.
//!
//! std-only on purpose: adding a `rand`/`getrandom` dependency would edit
//! `Cargo.lock` for a feature this small, and the agent already relies on the
//! OS for the operator's manual token. On unix we read `/dev/urandom`. The
//! fallback exists only so a non-unix build still compiles and is never used
//! on the shipped Linux agent; it hashes process-local entropy with
//! `RandomState`, which std seeds from the OS.

use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};

/// Fill `buf` with OS randomness. Returns false only when `/dev/urandom`
/// cannot be read.
#[cfg(unix)]
fn fill_os_random(buf: &mut [u8]) -> bool {
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(buf))
        .is_ok()
}

#[cfg(not(unix))]
fn fill_os_random(_buf: &mut [u8]) -> bool {
    false
}

static FALLBACK_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Deterministic-per-input hash fallback. Only reached when `/dev/urandom`
/// is unavailable, i.e. effectively never on the supported platform.
fn fill_fallback(buf: &mut [u8]) {
    use std::hash::BuildHasher;
    let mut written = 0;
    while written < buf.len() {
        let n = FALLBACK_COUNTER.fetch_add(1, Ordering::Relaxed);

        let v = std::collections::hash_map::RandomState::new()
            .hash_one((
                n,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
            ))
            .to_le_bytes();
        let take = v.len().min(buf.len() - written);
        buf[written..written + take].copy_from_slice(&v[..take]);
        written += take;
    }
}

/// `n` random bytes from the OS, with a last-resort local fallback.
pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    if !fill_os_random(&mut buf) {
        fill_fallback(&mut buf);
    }
    buf
}

/// Lowercase hex, the shape every ebbtop credential uses.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    s
}

/// A client credential: 32 random bytes hex (64 chars), the same shape as
/// the operator's manual token, so both live in one credential store and one
/// validator.
pub fn token() -> String {
    hex(&random_bytes(32))
}

/// A numeric pairing code of `digits` digits, uniformly distributed.
///
/// Each digit comes from one random byte, rejecting bytes >= 250 so `% 10`
/// stays unbiased (250 is the largest multiple of 10 that fits a byte).
pub fn code(digits: usize) -> String {
    let mut out = String::with_capacity(digits);
    while out.len() < digits {
        for b in random_bytes(digits * 2 + 8) {
            if b < 250 {
                out.push((b'0' + b % 10) as char);
                if out.len() == digits {
                    break;
                }
            }
        }
    }
    out
}

/// Random per-boot nonce: 16 bytes hex. Not a secret; it only needs to change
/// when the agent restarts.
pub fn nonce() -> String {
    hex(&random_bytes(16))
}

/// Length-independent equality for credentials: folds the length difference
/// into the result so a compare does not return early on the first differing
/// byte (the plain `==` this agent used for the manual token stays where it
/// is; this is for codes and issued tokens).
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = (a.len() ^ b.len()) as u8;
    let n = a.len().max(b.len());
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn token_is_32_bytes_hex() {
        let t = token();
        assert_eq!(t.len(), 64);
        assert!(
            t.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
    }

    #[test]
    fn tokens_differ() {
        let set: HashSet<String> = (0..64).map(|_| token()).collect();
        assert_eq!(set.len(), 64, "repeated token from the OS RNG");
    }

    #[test]
    fn code_has_exact_digit_count_and_charset() {
        for digits in [6usize, 7, 8] {
            for _ in 0..50 {
                let c = code(digits);
                assert_eq!(c.len(), digits);
                assert!(c.bytes().all(|b| b.is_ascii_digit()), "not digits: {c}");
            }
        }
    }

    #[test]
    fn codes_vary() {
        let set: HashSet<String> = (0..64).map(|_| code(6)).collect();
        assert!(set.len() > 60, "codes repeat too often: {}", set.len());
    }

    #[test]
    fn nonce_is_16_bytes_hex() {
        assert_eq!(nonce().len(), 32);
        assert_ne!(nonce(), nonce());
    }

    #[test]
    fn hex_matches_known_vector() {
        assert_eq!(hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
    }

    #[test]
    fn constant_time_eq_behaves() {
        assert!(constant_time_eq("123456", "123456"));
        assert!(!constant_time_eq("123456", "123457"));
        assert!(!constant_time_eq("123456", "1234567"));
        assert!(!constant_time_eq("", "0"));
        assert!(constant_time_eq("", ""));
    }
}
