//! ebbtop: Phase 2 kill. Request grammar, checks and signal delivery.
//!
//! Wire request (text frame): `kill:<pid>:<start_time_ms>:<TERM|INT>` only.
//! Anything else starting with "kill" is refused with an error reply.
//! Replies are JSON `KillOutcome` text frames (small: never gzipped, and the
//! client decodes text or gzip either way).
//!
//! Checks, in order (token auth happens earlier, at the ws upgrade):
//! 1. pid is not 1
//! 2. pid is not this agent's own pid
//! 3. not a kernel thread (ppid == 0 in /proc/<pid>/stat)
//! 4. the process uid equals the agent uid (so root-owned rows are refused)
//! 5. start_time_ms matches the process's current start time (PID reuse)
//! Then the signal is sent via pidfd_open + pidfd_send_signal.
//!
//! Every request and its result is written to the journal. The auth token
//! never appears here: it travels in the URL query string, not in the
//! request text, and no log line includes it.

use crate::types::KillOutcome;

/// A parsed, well-formed kill request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KillRequest {
    pub pid: u32,
    pub start_time_ms: u64,
    pub signal: &'static str, // "TERM" | "INT"
}

/// Stable refusal reasons (journal `err=` and wire `error`).
#[allow(dead_code)] // ERR_PLATFORM only used by the non-Linux attempt_kill
pub const ERR_GRAMMAR: &str = "bad request grammar";
pub const ERR_NO_PROC: &str = "no such process";
pub const ERR_PID1: &str = "pid 1";
pub const ERR_SELF: &str = "agent itself";
pub const ERR_KERNEL: &str = "kernel thread";
pub const ERR_UID: &str = "uid mismatch";
pub const ERR_STALE: &str = "stale start_time";
#[allow(dead_code)]
pub const ERR_PLATFORM: &str = "kill unsupported on this platform";

/// Strict grammar: `kill:<pid>:<start_time_ms>:<TERM|INT>`, nothing else.
/// No whitespace trimming: the client sends exactly this, and anything
/// looser would let odd bytes through to /proc.
pub fn parse_kill_request(text: &str) -> Result<KillRequest, &'static str> {
    let mut it = text.split(':');
    match (it.next(), it.next(), it.next(), it.next(), it.next()) {
        (Some("kill"), Some(p), Some(s), Some(sig), None) => {
            let pid: u32 = p.parse().map_err(|_| ERR_GRAMMAR)?;
            let start_time_ms: u64 = s.parse().map_err(|_| ERR_GRAMMAR)?;
            match sig {
                "TERM" | "INT" => Ok(KillRequest { pid, start_time_ms, signal: if sig == "TERM" { "TERM" } else { "INT" } }),
                _ => Err(ERR_GRAMMAR),
            }
        }
        _ => Err(ERR_GRAMMAR),
    }
}

/// Snapshot of the target process, for the pure check function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcTarget {
    pub ppid: u32,
    pub uid: u32,
    pub start_time_ms: u64,
}

/// Spec checks 1-5, in order. Pure so it tests without /proc.
pub fn decide_kill(req: &KillRequest, t: &ProcTarget, self_pid: u32, self_uid: u32) -> Result<(), &'static str> {
    if req.pid == 1 {
        return Err(ERR_PID1);
    }
    if req.pid == self_pid {
        return Err(ERR_SELF);
    }
    if t.ppid == 0 {
        return Err(ERR_KERNEL);
    }
    if t.uid != self_uid {
        return Err(ERR_UID);
    }
    if req.start_time_ms != t.start_time_ms {
        return Err(ERR_STALE);
    }
    Ok(())
}

fn sig_num(signal: &str) -> i32 {
    match signal {
        "TERM" => 15,
        _ => 2, // "INT" (parser only admits TERM/INT)
    }
}

/// Journal quoting: printable, single-line, `"..."` with `\` and `"` escaped.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars().take(80) {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn err_out(pid: u32, signal: &str, err: &str) -> KillOutcome {
    KillOutcome { ok: false, pid, signal: signal.to_string(), error: Some(err.to_string()) }
}

// ---- Journal -----------------------------------------------------------

/// One journald record, via the native journal socket (born on every
/// systemd host; the old `/run/systemd/journal/syslog` compat socket is
/// absent on this one). Fallback is stderr, which the unit forwards to the
/// journal anyway when running under systemd. Never includes the token.
fn journal_line(line: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixDatagram;
        if let Ok(sock) = UnixDatagram::unbound() {
            let msg = format!("PRIORITY=6\nMESSAGE={line}\n");
            if sock.send_to(msg.as_bytes(), "/run/systemd/journal/socket").is_ok() {
                return;
            }
        }
    }
    eprintln!("{line}");
}

pub fn journal_request_line(req: &KillRequest) -> String {
    format!("ebbtop kill request pid={} start_time_ms={} signal={}", req.pid, req.start_time_ms, req.signal)
}

pub fn journal_result_line(req: &KillRequest, out: &KillOutcome) -> String {
    match &out.error {
        None => format!("ebbtop kill result pid={} signal={} ok=true", req.pid, req.signal),
        Some(e) => format!("ebbtop kill result pid={} signal={} ok=false err={}", req.pid, req.signal, quote(e)),
    }
}

pub fn journal_bad_request_line(text: &str, out: &KillOutcome) -> String {
    format!("ebbtop kill request raw={} ok=false err={}", quote(text), quote(out.error.as_deref().unwrap_or(ERR_GRAMMAR)))
}

// ---- Entry point (called from ws.rs) -------------------------------------

/// Parse, journal, check, signal, journal, reply. Synchronous: a few small
/// /proc reads and two syscalls (same pattern as the metrics collectors).
pub fn handle_kill_request(text: &str) -> KillOutcome {
    let req = match parse_kill_request(text) {
        Ok(r) => r,
        Err(reason) => {
            let out = err_out(0, "", reason);
            journal_line(&journal_bad_request_line(text, &out));
            return out;
        }
    };
    journal_line(&journal_request_line(&req));
    let out = attempt_kill(&req);
    journal_line(&journal_result_line(&req, &out));
    out
}

#[cfg(target_os = "linux")]
fn attempt_kill(req: &KillRequest) -> KillOutcome {
    let Some(target) = read_target(req.pid) else {
        return err_out(req.pid, req.signal, ERR_NO_PROC);
    };
    let Some(self_uid) = self_uid() else {
        return err_out(req.pid, req.signal, ERR_NO_PROC);
    };
    match decide_kill(req, &target, std::process::id(), self_uid) {
        Err(reason) => err_out(req.pid, req.signal, reason),
        Ok(()) => match send_signal(req.pid, sig_num(req.signal)) {
            Ok(()) => KillOutcome { ok: true, pid: req.pid, signal: req.signal.to_string(), error: None },
            Err(e) => err_out(req.pid, req.signal, &e),
        },
    }
}

#[cfg(not(target_os = "linux"))]
fn attempt_kill(req: &KillRequest) -> KillOutcome {
    err_out(req.pid, req.signal, ERR_PLATFORM)
}

// ---- /proc reads (Linux) -------------------------------------------------

#[cfg(target_os = "linux")]
fn read_target(pid: u32) -> Option<ProcTarget> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = stat.get(stat.rfind(')')? + 2..)?;
    let ppid: u32 = after.split_whitespace().nth(1)?.parse().ok()?;
    let start_ticks: u64 = after.split_whitespace().nth(19)?.parse().ok()?;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let uid: u32 = status
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))
        .and_then(|v| v.split_whitespace().next()?.parse().ok())?;
    let start_time_ms = crate::proc_util::start_time_ms(crate::proc_util::boot_btime()?, start_ticks);
    Some(ProcTarget { ppid, uid, start_time_ms })
}

#[cfg(target_os = "linux")]
fn self_uid() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))
        .and_then(|v| v.split_whitespace().next()?.parse().ok())
}

// ---- pidfd syscalls (Linux, no new deps) -----------------------------------
//
// pidfd_open / pidfd_send_signal via the `syscall` instruction, so the
// vendored agent needs no libc dependency. Numbers 434/424 are the generic
// ones shared by x86_64, aarch64 and the common 64-bit Linux ports.

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn raw_syscall(n: i64, a1: i64, a2: i64, a3: i64, a4: i64) -> i64 {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") n => ret,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
            in("r10") a4,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    ret
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn raw_syscall(n: i64, a1: i64, a2: i64, a3: i64, a4: i64) -> i64 {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "svc 0",
            inlateout("x8") n => _,
            inlateout("x0") a1 => ret,
            in("x1") a2,
            in("x2") a3,
            in("x3") a4,
            options(nostack)
        );
    }
    ret
}

#[cfg(all(target_os = "linux", not(any(target_arch = "x86_64", target_arch = "aarch64"))))]
fn raw_syscall(n: i64, _a1: i64, _a2: i64, _a3: i64, _a4: i64) -> i64 {
    let _ = n;
    -38 // -ENOSYS: fail closed on ports we have not written an asm block for
}

#[cfg(target_os = "linux")]
fn send_signal(pid: u32, signum: i32) -> Result<(), String> {
    // pidfd_open(pid, flags=0) = syscall 434.
    let fd = raw_syscall(434, pid as i64, 0, 0, 0);
    if fd < 0 {
        return Err(format!("pidfd_open: {}", err_text(-fd)));
    }
    // From here the fd is owned; dropping the File closes it.
    let file = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd as i32) };
    // pidfd_send_signal(pidfd, sig, siginfo=NULL, mask=empty) = syscall 424.
    let r = raw_syscall(424, fd as i64, signum as i64, 0, 0);
    drop(file);
    if r < 0 {
        Err(format!("pidfd_send_signal: {}", err_text(-r)))
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn err_text(errno: i64) -> String {
    match errno {
        1 => "operation not permitted".to_string(),
        2 => "no such process".to_string(),
        3 => "no such process group".to_string(),
        22 => "invalid argument".to_string(),
        38 => "not supported on this platform".to_string(),
        _ => format!("errno {errno}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(pid: u32, start: u64, signal: &'static str) -> KillRequest {
        KillRequest { pid, start_time_ms: start, signal }
    }

    #[test]
    fn parses_exact_grammar() {
        assert_eq!(parse_kill_request("kill:4242:1700000002500:TERM"), Ok(req(4242, 1_700_000_002_500, "TERM")));
        assert_eq!(parse_kill_request("kill:7:0:INT"), Ok(req(7, 0, "INT")));
    }

    #[test]
    fn refuses_anything_else() {
        for s in [
            "kill", "kill:", "kill:1", "kill:1:2", "kill:1:2:TERM:extra",
            "kill:0:0:TERM", // pid 0 parses; decide handles it (no such process / kernel)
            "kill:abc:2:TERM", "kill:1:2:KILL", "kill:1:2:term", "kill:1:2:9",
            "kill:1:-1:TERM", "kill: 1:2:TERM", "Kill:1:2:TERM", " kill:1:2:TERM",
            "kill:1:2:TERM ", "get_metrics", "kill:99999999999:2:TERM",
        ] {
            let r = parse_kill_request(s);
            // pid 0 and huge-pid parse-fail cases split: pid 0 parses, rest refuse.
            if s == "kill:0:0:TERM" {
                assert!(r.is_ok(), "{s} should parse (decide refuses it)");
            } else {
                assert_eq!(r, Err(ERR_GRAMMAR), "{s}");
            }
        }
    }

    #[test]
    fn checks_in_order() {
        let self_pid = 10_000;
        let self_uid = 1000;
        let good = ProcTarget { ppid: 1, uid: self_uid, start_time_ms: 555 };
        // happy path
        assert_eq!(decide_kill(&req(99, 555, "TERM"), &good, self_pid, self_uid), Ok(()));
        // pid 1 beats everything
        assert_eq!(decide_kill(&req(1, 0, "TERM"), &ProcTarget { ppid: 0, uid: 0, start_time_ms: 0 }, self_pid, self_uid), Err(ERR_PID1));
        // agent itself (before the ppid check, so it wins over ppid==0)
        assert_eq!(decide_kill(&req(self_pid, 0, "TERM"), &ProcTarget { ppid: 0, uid: 0, start_time_ms: 0 }, self_pid, self_uid), Err(ERR_SELF));
        // kernel thread (ppid 0)
        assert_eq!(decide_kill(&req(99, 555, "INT"), &ProcTarget { ppid: 0, uid: self_uid, start_time_ms: 555 }, self_pid, self_uid), Err(ERR_KERNEL));
        // root-owned: uid mismatch
        assert_eq!(decide_kill(&req(99, 555, "TERM"), &ProcTarget { ppid: 1, uid: 0, start_time_ms: 555 }, self_pid, self_uid), Err(ERR_UID));
        // PID reuse: stale start time
        assert_eq!(decide_kill(&req(99, 556, "TERM"), &good, self_pid, self_uid), Err(ERR_STALE));
        // uid check beats stale check (order matters)
        assert_eq!(decide_kill(&req(99, 556, "TERM"), &ProcTarget { ppid: 1, uid: 0, start_time_ms: 555 }, self_pid, self_uid), Err(ERR_UID));
    }

    #[test]
    fn journal_lines_are_stable_and_token_free() {
        let r = req(4242, 1_700_000_002_500, "TERM");
        assert_eq!(journal_request_line(&r), "ebbtop kill request pid=4242 start_time_ms=1700000002500 signal=TERM");
        let ok = KillOutcome { ok: true, pid: 4242, signal: "TERM".into(), error: None };
        assert_eq!(journal_result_line(&r, &ok), "ebbtop kill result pid=4242 signal=TERM ok=true");
        let bad = err_out(4242, "TERM", ERR_STALE);
        assert_eq!(journal_result_line(&r, &bad), r#"ebbtop kill result pid=4242 signal=TERM ok=false err="stale start_time""#);
        let raw = journal_bad_request_line("kill:1:2:KILL", &err_out(0, "", ERR_GRAMMAR));
        assert!(raw.starts_with("ebbtop kill request raw=\"kill:1:2:KILL\" ok=false"), "{raw}");
    }

    #[test]
    fn quote_escapes_and_caps() {
        assert_eq!(quote("a\"b\\c"), r#""a\"b\\c""#);
        assert_eq!(quote("line\nbreak"), r#""line break""#);
        assert_eq!(quote(&"x".repeat(200)).len(), 82); // 80 chars + 2 quotes
    }

    /// End-to-end on Linux: real /proc reads and real pidfd syscalls against
    /// a `sleep` child this test owns. (Writes two lines to the journal.)
    #[cfg(target_os = "linux")]
    #[test]
    fn real_refusals_and_signal() {
        // pid 1 and the agent itself are refused before any /proc check.
        let self_pid = std::process::id();
        assert_eq!(handle_kill_request("kill:1:1:TERM").error.as_deref(), Some(ERR_PID1));
        assert_eq!(handle_kill_request(&format!("kill:{self_pid}:1:TERM")).error.as_deref(), Some(ERR_SELF));
        assert_eq!(handle_kill_request("kill:999999:1:TERM").error.as_deref(), Some(ERR_NO_PROC));
        assert_eq!(handle_kill_request("kill:1:2:KILL").error.as_deref(), Some(ERR_GRAMMAR));

        let mut child = std::process::Command::new("sleep").arg("30").spawn().expect("sleep");
        let pid = child.id();
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).expect("stat");
        let after = stat.get(stat.rfind(')').unwrap() + 2..).unwrap();
        let ticks: u64 = after.split_whitespace().nth(19).unwrap().parse().unwrap();
        let start = crate::proc_util::start_time_ms(crate::proc_util::boot_btime().unwrap(), ticks);

        // Stale start_time (PID reuse guard) refuses and the child lives on.
        let stale = handle_kill_request(&format!("kill:{pid}:{}:TERM", start + 1000));
        assert_eq!(stale.error.as_deref(), Some(ERR_STALE));
        assert!(child.try_wait().unwrap().is_none());

        // Correct start_time delivers SIGTERM.
        let out = handle_kill_request(&format!("kill:{pid}:{start}:TERM"));
        assert!(out.ok && out.error.is_none(), "{out:?}");
        let status = child.wait().unwrap();
        assert_eq!(std::os::unix::process::ExitStatusExt::signal(&status), Some(15), "{status:?}");
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_refuses() {
        let out = handle_kill_request("kill:99:1:TERM");
        assert_eq!(out.error.as_deref(), Some(ERR_PLATFORM));
    }
}
