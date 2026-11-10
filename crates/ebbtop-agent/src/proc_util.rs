//! ebbtop: load average, uptime and per-disk I/O counters from /proc.
//! Parsers are pure functions over file text so they test without /proc.

use crate::types::DiskIo;

pub fn parse_loadavg(s: &str) -> Option<[f64; 3]> {
    let mut it = s.split_whitespace().map(|x| x.parse::<f64>().ok());
    Some([it.next()??, it.next()??, it.next()??])
}

pub fn parse_uptime(s: &str) -> Option<u64> {
    s.split_whitespace().next()?.parse::<f64>().ok().map(|f| f as u64)
}

/// Whole-disk devices only: skip loop/ram/zram and device-mapper (which
/// would double-count I/O already seen on the physical disk).
fn wanted(name: &str, whole_disks: &[String]) -> bool {
    !(name.starts_with("loop") || name.starts_with("ram") || name.starts_with("zram") || name.starts_with("dm-"))
        && whole_disks.iter().any(|d| d == name)
}

/// /proc/diskstats: field 3 name, 6 sectors read, 10 sectors written (512 B).
pub fn parse_diskstats(s: &str, whole_disks: &[String]) -> Vec<DiskIo> {
    s.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 10 || !wanted(f[2], whole_disks) {
                return None;
            }
            Some(DiskIo {
                name: f[2].to_string(),
                read_bytes: f[5].parse::<u64>().ok()? * 512,
                written_bytes: f[9].parse::<u64>().ok()? * 512,
            })
        })
        .collect()
}

pub fn load_avg() -> Option<[f64; 3]> {
    parse_loadavg(&std::fs::read_to_string("/proc/loadavg").ok()?)
}

pub fn uptime_s() -> Option<u64> {
    parse_uptime(&std::fs::read_to_string("/proc/uptime").ok()?)
}

pub fn disk_io() -> Vec<DiskIo> {
    let whole: Vec<String> = std::fs::read_dir("/sys/block")
        .map(|rd| rd.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect())
        .unwrap_or_default();
    std::fs::read_to_string("/proc/diskstats")
        .map(|s| parse_diskstats(&s, &whole))
        .unwrap_or_default()
}

/// ebbtop: `/proc/stat` `btime` line (seconds since epoch, boot time).
pub fn parse_btime(s: &str) -> Option<u64> {
    s.lines()
        .find_map(|l| l.strip_prefix("btime "))
        .and_then(|v| v.trim().parse::<u64>().ok())
}

pub fn boot_btime() -> Option<u64> {
    parse_btime(&std::fs::read_to_string("/proc/stat").ok()?)
}

/// Clock ticks per second, as in `metrics.rs` (`USER_HZ`, universal on Linux).
pub const TICKS_PER_SEC: u64 = 100;

/// ebbtop: process start as agent epoch ms = (boot_sec + ticks/HZ) * 1000,
/// in exact integer arithmetic. `starttime_ticks` is field 22 of /proc/<pid>/stat
/// (offset 19 after comm, see metrics.rs `procstat`).
pub fn start_time_ms(boot_btime_s: u64, starttime_ticks: u64) -> u64 {
    (boot_btime_s * TICKS_PER_SEC + starttime_ticks) * (1000 / TICKS_PER_SEC)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loadavg_and_uptime() {
        assert_eq!(parse_loadavg("0.52 0.58 0.59 1/389 12345\n"), Some([0.52, 0.58, 0.59]));
        assert_eq!(parse_loadavg("garbage"), None);
        assert_eq!(parse_uptime("453201.77 3539221.48\n"), Some(453201));
    }

    #[test]
    fn diskstats_whole_disks_only() {
        let s = "   7       0 loop0 2057 0 11866 861 0 0 0 0 0 862 861 0 0 0 0 0 0\n\
                 259       0 nvme0n1 100 0 2000 10 50 0 4000 20 0 30 30 0 0 0 0 0 0\n\
                 259       1 nvme0n1p1 1 0 8 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n\
                 252       0 dm-0 90 0 1900 10 40 0 3900 20 0 30 30 0 0 0 0 0 0\n";
        let whole = vec!["loop0".to_string(), "nvme0n1".to_string(), "dm-0".to_string()];
        let v = parse_diskstats(s, &whole);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "nvme0n1");
        assert_eq!(v[0].read_bytes, 2000 * 512);
        assert_eq!(v[0].written_bytes, 4000 * 512);
    }

    #[test]
    fn btime_and_start_time_ms() {
        assert_eq!(parse_btime("cpu  1 2\nbtime 1700000000\nintr 3\n"), Some(1_700_000_000));
        assert_eq!(parse_btime("cpu  1 2\n"), None);
        // 100 ticks = 1 s, so exact: (1_700_000_000 * 100 + 250) * 10 = start+2.5 s
        assert_eq!(start_time_ms(1_700_000_000, 250), 1_700_000_002_500);
        assert_eq!(start_time_ms(1_700_000_000, 0), 1_700_000_000_000);
    }
}
