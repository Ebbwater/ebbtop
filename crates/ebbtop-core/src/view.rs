//! Renderer-neutral view helpers: formatting, process sort and filter.

use crate::wire::Process;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Pid,
    Name,
    User,
    State,
    Cpu,
    Mem,
}

/// Whether a process row is root-owned. Phase 2 kill is disabled for these
/// (the agent only kills processes owned by its own uid).
pub fn is_root_owned(user: &str) -> bool {
    user == "root"
}

/// btop/ps one-letter state from a status string. Process states arrive as
/// sysinfo Display (`Runnable`, …); thread states arrive as /proc words
/// (`Running`, `Disk Sleep`, …).
pub fn state_letter(s: &str) -> &'static str {
    match s {
        "Runnable" | "Running" => "R",
        "Sleeping" => "S",
        "Idle" => "I",
        "UninterruptibleDiskSleep" | "Disk Sleep" => "D",
        "Stopped" => "T",
        "Tracing" | "Tracing Stop" => "t",
        "Zombie" => "Z",
        "Dead" => "X",
        "Wakekill" => "K",
        "Waking" => "W",
        "Parked" => "P",
        "LockBlocked" => "L",
        "" => "",
        _ => "?",
    }
}

/// Indexes into `rows`, filtered and sorted. Filter matches name, user or
/// pid as a case-insensitive substring. Ties break on pid so the order is
/// stable between ticks.
pub fn select_rows(rows: &[Process], filter: &str, key: SortKey, desc: bool) -> Vec<usize> {
    let f = filter.trim().to_lowercase();
    let mut idx: Vec<usize> = (0..rows.len())
        .filter(|&i| {
            if f.is_empty() {
                return true;
            }
            let r = &rows[i];
            r.name.to_lowercase().contains(&f)
                || r.user.to_lowercase().contains(&f)
                || r.pid.to_string().contains(&f)
        })
        .collect();
    idx.sort_by(|&a, &b| {
        let (x, y) = (&rows[a], &rows[b]);
        let o = match key {
            SortKey::Pid => x.pid.cmp(&y.pid),
            SortKey::Name => x.name.to_lowercase().cmp(&y.name.to_lowercase()),
            SortKey::User => x.user.cmp(&y.user),
            SortKey::State => x.state.cmp(&y.state),
            SortKey::Cpu => x.cpu_usage.total_cmp(&y.cpu_usage),
            SortKey::Mem => x.mem_bytes.cmp(&y.mem_bytes),
        };
        let o = if desc { o.reverse() } else { o };
        o.then(x.pid.cmp(&y.pid))
    });
    idx
}

/// IEC bytes: "512 B", "1.5 KiB", "12.3 GiB".
pub fn bytes(n: u64) -> String {
    const U: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else if v >= 100.0 { format!("{v:.0} {}", U[i]) } else { format!("{v:.1} {}", U[i]) }
}

/// Bytes per second as bits per second, btop style: "1.2 Mbps".
pub fn rate_bits(bps: f64) -> String {
    let b = bps * 8.0;
    let (v, u) = if b >= 1e9 {
        (b / 1e9, "Gbps")
    } else if b >= 1e6 {
        (b / 1e6, "Mbps")
    } else if b >= 1e3 {
        (b / 1e3, "Kbps")
    } else {
        (b, "bps")
    };
    if u == "bps" { format!("{v:.0} {u}") } else { format!("{v:.1} {u}") }
}

/// Millisecond age as "0.3s" / "12s" / "4m".
pub fn age(ms: i64) -> String {
    let ms = ms.max(0);
    if ms < 10_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else if ms < 120_000 {
        format!("{}s", ms / 1000)
    } else {
        format!("{}m", ms / 60_000)
    }
}

/// Uptime as "5d 5h", "3h 12m" or "7m".
pub fn uptime(s: u64) -> String {
    let (d, h, m) = (s / 86_400, s % 86_400 / 3_600, s % 3_600 / 60);
    if d > 0 { format!("{d}d {h}h") } else if h > 0 { format!("{h}h {m}m") } else { format!("{m}m") }
}

/// CPU microseconds as "1.5s", "1m 30s" or "1h 2m".
pub fn cpu_time(us: u64) -> String {
    if us < 60_000_000 {
        format!("{:.1}s", us as f64 / 1_000_000.0)
    } else {
        let s = us / 1_000_000;
        if s < 3600 { format!("{}m {}s", s / 60, s % 60) } else { format!("{}h {}m", s / 3600, s % 3600 / 60) }
    }
}

/// The boundary that owns a process, derived from its cgroup path. Engine
/// agnostic on purpose: podman, docker, nspawn, a plain systemd service and a
/// login session all resolve through the same three rules.
///
/// 1. A systemd unit (`*.service`) that is not the login session manager
///    (`user@1000.service`). On a quadlet host this is the container's own
///    unit and it groups the payload together with its conmon/pasta helpers.
/// 2. A container id (`libpod-<id>`, `libpod-payload-<id>`,
///    `libpod-conmon-<id>.scope`). Rootless containers under the user session
///    manager have no unit of their own, so the id is the boundary.
/// 3. The nearest transient scope (`*.scope`).
///
/// Anything else, and any legacy agent that reports no cgroup, returns an
/// empty key, meaning "the host".
pub fn workload_key(cgroup: &str) -> String {
    let segs: Vec<&str> = cgroup.split('/').filter(|s| !s.is_empty()).collect();
    if let Some(s) = segs.iter().rev().find(|s| s.ends_with(".service") && !s.starts_with("user@")) {
        return (*s).to_string();
    }
    if let Some(id) = segs.iter().rev().find_map(|s| container_id(s)) {
        return format!("libpod-{id}");
    }
    if let Some(s) = segs.iter().rev().find(|s| s.ends_with(".scope")) {
        return (*s).to_string();
    }
    String::new()
}

/// Container id embedded in one cgroup segment, if the segment is a podman
/// scope. Accepts `libpod-<id>`, `libpod-payload-<id>` and
/// `libpod-conmon-<id>.scope`, with or without the `.scope` suffix. Requires
/// at least 12 hex characters so a name that merely starts with `libpod-`
/// does not match.
pub fn container_id(segment: &str) -> Option<&str> {
    let rest = segment.strip_prefix("libpod-")?;
    let rest = rest.strip_prefix("payload-").unwrap_or(rest);
    let rest = rest.strip_prefix("conmon-").unwrap_or(rest);
    let rest = rest.strip_suffix(".scope").unwrap_or(rest);
    (rest.len() >= 12 && rest.bytes().all(|b| b.is_ascii_hexdigit())).then_some(rest)
}

/// One workload roll-up: a container, a systemd unit, or the host.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadGroup {
    /// `workload_key` for the group; empty string is the host itself.
    pub key: String,
    pub pids: usize,
    pub cpu_usage: f32,
    pub mem_bytes: u64,
}

/// Aggregate the process list by `workload_key`, biggest memory first, then
/// by key so the order is stable between ticks.
///
/// These sums are only as complete as the rows the agent could see. A group
/// whose helper processes live outside the boundary under-reports, so treat
/// the total as "what this boundary's visible processes use", not as a
/// billing-grade container total.
pub fn group_by_workload(rows: &[Process]) -> Vec<WorkloadGroup> {
    let mut out: Vec<WorkloadGroup> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for r in rows {
        let key = workload_key(&r.cgroup);
        match index.get(&key) {
            Some(&i) => {
                let g = &mut out[i];
                g.pids += 1;
                g.cpu_usage += r.cpu_usage;
                g.mem_bytes += r.mem_bytes;
            }
            None => {
                index.insert(key.clone(), out.len());
                out.push(WorkloadGroup { key, pids: 1, cpu_usage: r.cpu_usage, mem_bytes: r.mem_bytes });
            }
        }
    }
    out.sort_by(|a, b| b.mem_bytes.cmp(&a.mem_bytes).then_with(|| a.key.cmp(&b.key)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, name: &str, user: &str, cpu: f32, mem: u64) -> Process {
        Process { pid, name: name.into(), cpu_usage: cpu, mem_bytes: mem, user: user.into(), state: "Sleeping".into(), start_time_ms: 0, cgroup: String::new() }
    }

    #[test]
    fn sort_cpu_desc_with_pid_tiebreak() {
        let rows = vec![p(3, "a", "root", 1.0, 0), p(1, "b", "kyler", 5.0, 0), p(2, "c", "kyler", 1.0, 0)];
        let pids: Vec<u32> = select_rows(&rows, "", SortKey::Cpu, true).iter().map(|&i| rows[i].pid).collect();
        assert_eq!(pids, vec![1, 2, 3]);
    }

    #[test]
    fn filter_matches_name_user_pid_case_insensitive() {
        let rows = vec![p(10, "Hermes", "kyler", 0.0, 0), p(222, "sshd", "root", 0.0, 0)];
        assert_eq!(select_rows(&rows, "herm", SortKey::Pid, false), vec![0]);
        assert_eq!(select_rows(&rows, "ROOT", SortKey::Pid, false), vec![1]);
        assert_eq!(select_rows(&rows, "22", SortKey::Pid, false), vec![1]);
        assert_eq!(select_rows(&rows, "  ", SortKey::Pid, false), vec![0, 1]);
    }

    #[test]
    fn root_owned() {
        assert!(is_root_owned("root"));
        assert!(!is_root_owned("kyler"));
        assert!(!is_root_owned(""));
    }

    #[test]
    fn cpu_time_formatting() {
        assert_eq!(cpu_time(1_500_000), "1.5s");
        assert_eq!(cpu_time(90_000_000), "1m 30s");
        assert_eq!(cpu_time(3_726_000_000), "1h 2m");
        assert_eq!(cpu_time(0), "0.0s");
    }

    #[test]
    fn state_letter_covers_thread_words() {
        assert_eq!(state_letter("Running"), "R");
        assert_eq!(state_letter("Disk Sleep"), "D");
        assert_eq!(state_letter("Tracing Stop"), "t");
        assert_eq!(state_letter("Unknown"), "?");
    }

    #[test]
    fn formatting() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(bytes(32_729_354_240), "30.5 GiB");
        assert_eq!(rate_bits(125_000.0), "1.0 Mbps");
        assert_eq!(rate_bits(10.0), "80 bps");
        assert_eq!(age(300), "0.3s");
        assert_eq!(age(45_000), "45s");
        assert_eq!(age(600_000), "10m");
        assert_eq!(state_letter("UninterruptibleDiskSleep"), "D");
        assert_eq!(state_letter("Mystery"), "?");
        assert_eq!(uptime(453_201), "5d 5h");
        assert_eq!(uptime(11_520), "3h 12m");
        assert_eq!(uptime(420), "7m");
    }

    /// Real cgroup paths from k-server (podman 5.7.0, rootless, quadlet units).
    const ID: &str = "65326a2caf20c491810c381f5b09e81c74c0fef5819e05ddd4205ec0335d25d7";

    fn pg(cgroup: &str, mem: u64, cpu: f32) -> Process {
        Process { cgroup: cgroup.into(), ..p(1, "x", "kyler", cpu, mem) }
    }

    #[test]
    fn workload_key_groups_payload_with_its_helpers() {
        let payload = format!("/user.slice/user-1000.slice/user@1000.service/app.slice/davy-pg.service/libpod-payload-{ID}");
        let conmon = "/user.slice/user-1000.slice/user@1000.service/app.slice/davy-pg.service/runtime";
        assert_eq!(workload_key(&payload), "davy-pg.service");
        assert_eq!(workload_key(conmon), "davy-pg.service");
    }

    #[test]
    fn workload_key_falls_back_to_container_id_without_a_unit() {
        let payload = format!("/user.slice/user-1000.slice/user@1000.service/user.slice/libpod-{ID}.scope/container");
        let conmon = format!("/user.slice/user-1000.slice/user@1000.service/user.slice/libpod-conmon-{ID}.scope");
        assert_eq!(workload_key(&payload), format!("libpod-{ID}"));
        assert_eq!(workload_key(&conmon), format!("libpod-{ID}"));
    }

    #[test]
    fn workload_key_units_scopes_and_host() {
        assert_eq!(workload_key("/user.slice/user-1000.slice/user@1000.service/app.slice/wn-ci-runner-2.service"), "wn-ci-runner-2.service");
        assert_eq!(workload_key("/init.scope"), "init.scope");
        assert_eq!(workload_key(""), "");
        assert_eq!(workload_key("/"), "");
    }

    #[test]
    fn container_id_only_matches_real_ids() {
        assert_eq!(container_id(&format!("libpod-payload-{ID}")), Some(ID));
        assert_eq!(container_id(&format!("libpod-conmon-{ID}.scope")), Some(ID));
        assert_eq!(container_id(&format!("libpod-{ID}.scope")), Some(ID));
        assert_eq!(container_id("libpod-notanid"), None);
        assert_eq!(container_id("davy-pg.service"), None);
    }

    #[test]
    fn group_by_workload_sums_and_orders() {
        let payload = format!("/user.slice/user-1000.slice/user@1000.service/app.slice/davy-pg.service/libpod-payload-{ID}");
        let conmon = "/user.slice/user-1000.slice/user@1000.service/app.slice/davy-pg.service/runtime";
        let ci = "/user.slice/user-1000.slice/user@1000.service/app.slice/wn-ci-runner-2.service";
        let rows = vec![pg(&payload, 900 << 20, 4.0), pg(conmon, 3 << 20, 0.5), pg(ci, 100 << 20, 1.0), pg("", 1 << 20, 0.0)];
        let groups = group_by_workload(&rows);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].key, "davy-pg.service");
        assert_eq!(groups[0].pids, 2);
        assert_eq!(groups[0].mem_bytes, 903 << 20);
        assert_eq!(groups[0].cpu_usage, 4.5);
        assert_eq!(groups[1].key, "wn-ci-runner-2.service");
        assert_eq!(groups[2].key, "");
    }
}
