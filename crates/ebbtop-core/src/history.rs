//! Client-side state: latest snapshot, ring-buffer history, tick statistics.
//! No GUI types here so the probe and any future renderer share it.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use crate::client::Event;
use crate::wire::{DiskInfo, KillResult, Metrics, ProcessMetricsResponse, Processes};

/// 5 minutes at 1 s.
pub const HISTORY_LEN: usize = 300;

#[derive(Debug, Clone)]
pub struct Ring<T> {
    cap: usize,
    buf: VecDeque<T>,
}

impl<T> Ring<T> {
    pub fn new(cap: usize) -> Self {
        Self { cap, buf: VecDeque::with_capacity(cap) }
    }
    pub fn push(&mut self, v: T) {
        if self.buf.len() == self.cap {
            self.buf.pop_front();
        }
        self.buf.push_back(v);
    }
    pub fn len(&self) -> usize {
        self.buf.len()
    }
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &T> + ExactSizeIterator {
        self.buf.iter()
    }
    pub fn last(&self) -> Option<&T> {
        self.buf.back()
    }
    pub fn clear(&mut self) {
        self.buf.clear()
    }
}

/// One point: agent sample time (ms) and value.
pub type Point = (u64, f64);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NetRate {
    pub t_ms: u64,
    pub rx_bps: f64,
    pub tx_bps: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Conn {
    Connecting { attempt: u32 },
    Connected,
    Down { reason: String, retry_in: Duration, since_ms: u64 },
}

/// Nearest-rank percentile, p in 0..=100. None when empty.
pub fn percentile(vals: &[i64], p: f64) -> Option<i64> {
    if vals.is_empty() {
        return None;
    }
    let mut v = vals.to_vec();
    v.sort_unstable();
    let rank = ((p / 100.0) * v.len() as f64).ceil() as usize;
    Some(v[rank.clamp(1, v.len()) - 1])
}

#[derive(Debug, Clone)]
pub struct TickStats {
    /// recv_ms - sampled_at_ms per metrics frame (needs synced clocks).
    pub age_ms: Ring<i64>,
    /// sampled_at delta between consecutive distinct metrics samples.
    pub interval_ms: Ring<i64>,
    /// Same, for process lists (patched agent only).
    pub proc_interval_ms: Ring<i64>,
    pub proc_age_ms: Ring<i64>,
    /// Sample intervals above 2000 ms (acceptance "gap").
    pub gaps: u64,
    /// Metrics frames that repeated the previous sample (agent cache hit).
    pub repeats: u64,
}

impl Default for TickStats {
    fn default() -> Self {
        Self {
            age_ms: Ring::new(HISTORY_LEN),
            interval_ms: Ring::new(HISTORY_LEN),
            proc_interval_ms: Ring::new(HISTORY_LEN),
            proc_age_ms: Ring::new(HISTORY_LEN),
            gaps: 0,
            repeats: 0,
        }
    }
}

fn ring_vec(r: &Ring<i64>) -> Vec<i64> {
    r.iter().copied().collect()
}

impl TickStats {
    pub fn age_p(&self, p: f64) -> Option<i64> {
        percentile(&ring_vec(&self.age_ms), p)
    }
    pub fn interval_p(&self, p: f64) -> Option<i64> {
        percentile(&ring_vec(&self.interval_ms), p)
    }
    pub fn proc_interval_p(&self, p: f64) -> Option<i64> {
        percentile(&ring_vec(&self.proc_interval_ms), p)
    }
    pub fn proc_age_p(&self, p: f64) -> Option<i64> {
        percentile(&ring_vec(&self.proc_age_ms), p)
    }
}

pub const GAP_MS: i64 = 2000;

/// How long a successful kill notice stays in the status bar. Refusals
/// persist until the next kill attempt or a selection change clears them.
pub const KILL_OK_TTL_MS: u64 = 15_000;

#[derive(Debug, Clone)]
pub struct State {
    pub conn: Conn,
    pub reconnects: u64,
    pub metrics: Option<Metrics>,
    pub metrics_recv_ms: u64,
    /// Client clock when the newest *new* metrics sample arrived. Staleness
    /// uses this so it does not depend on the two machines' clocks agreeing.
    pub last_new_sample_recv_ms: u64,
    pub procs: Option<Processes>,
    pub procs_recv_ms: u64,
    pub disks: Vec<DiskInfo>,
    pub cpu: Ring<Point>,
    pub mem_pct: Ring<Point>,
    pub swap_pct: Ring<Point>,
    /// GPU utilization history, one ring per GPU name.
    pub gpu_pct: HashMap<String, Ring<Point>>,
    /// GPU VRAM % history, one ring per GPU name (same clock as gpu_pct).
    pub gpu_mem_pct: HashMap<String, Ring<Point>>,
    pub net: HashMap<String, Ring<NetRate>>,
    /// Per-disk I/O rates, bytes/s (rx = read, tx = written).
    pub disk_io: HashMap<String, Ring<NetRate>>,
    pub stats: TickStats,
    /// Latest `get_process_metrics` reply (for the selected pid at poll time).
    /// `proc_detail_cpu` holds its CPU history; both are stale across
    /// disconnects like `metrics`/`procs` and are reset by a fresh State on
    /// reconnect.
    pub proc_detail: Option<ProcessMetricsResponse>,
    pub proc_detail_recv_ms: u64,
    /// Per-selected-pid CPU% history (one ring for the pid `proc_detail`
    /// belongs to; cleared on pid or start_time change so PID reuse does not
    /// glue two processes' graphs together).
    pub proc_detail_cpu: Ring<Point>,
    /// Per-selected-pid RSS bytes history, same clock and clearing rules.
    pub proc_detail_mem: Ring<Point>,
    /// Newest Phase 2 kill reply, for the status bar (None until one runs).
    pub kill: Option<KillResult>,
    pub kill_recv_ms: u64,
    prev_metrics_t: Option<u64>,
    prev_procs_t: Option<u64>,
    prev_detail_t: Option<u64>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            conn: Conn::Connecting { attempt: 1 },
            reconnects: 0,
            metrics: None,
            metrics_recv_ms: 0,
            last_new_sample_recv_ms: 0,
            procs: None,
            procs_recv_ms: 0,
            disks: Vec::new(),
            cpu: Ring::new(HISTORY_LEN),
            mem_pct: Ring::new(HISTORY_LEN),
            swap_pct: Ring::new(HISTORY_LEN),
            gpu_pct: HashMap::new(),
            gpu_mem_pct: HashMap::new(),
            net: HashMap::new(),
            disk_io: HashMap::new(),
            stats: TickStats::default(),
            proc_detail: None,
            proc_detail_recv_ms: 0,
            proc_detail_cpu: Ring::new(HISTORY_LEN),
            proc_detail_mem: Ring::new(HISTORY_LEN),
            kill: None,
            kill_recv_ms: 0,
            prev_metrics_t: None,
            prev_procs_t: None,
            prev_detail_t: None,
        }
    }
}

fn pct(used: u64, total: u64) -> f64 {
    if total == 0 { 0.0 } else { used as f64 * 100.0 / total as f64 }
}

impl State {
    pub fn apply(&mut self, ev: Event, now_ms: u64) {
        match ev {
            Event::Connecting { attempt } => self.conn = Conn::Connecting { attempt },
            Event::Connected => self.conn = Conn::Connected,
            Event::Disconnected { reason, retry_in } => {
                if self.conn == Conn::Connected {
                    self.reconnects += 1;
                }
                self.conn = Conn::Down { reason, retry_in, since_ms: now_ms };
            }
            Event::Metrics { m, recv_ms } => self.on_metrics(m, recv_ms),
            Event::Processes { p, recv_ms } => {
                let t = p.sampled_at_ms;
                if t != 0 {
                    self.stats.proc_age_ms.push(recv_ms as i64 - t as i64);
                    if let Some(prev) = self.prev_procs_t
                        && t > prev
                    {
                        self.stats.proc_interval_ms.push((t - prev) as i64);
                    }
                    self.prev_procs_t = Some(t);
                }
                self.procs = Some(p);
                self.procs_recv_ms = recv_ms;
            }
            Event::Disks { d, .. } => self.disks = d,
            Event::ProcessDetail { d, recv_ms } => self.on_process_detail(d, recv_ms),
            Event::KillResult { r, recv_ms } => {
                self.kill = Some(r);
                self.kill_recv_ms = recv_ms;
            }
        }
    }

    fn on_metrics(&mut self, m: Metrics, recv_ms: u64) {
        let t = m.sampled_at_ms;
        self.stats.age_ms.push(recv_ms as i64 - t as i64);
        match self.prev_metrics_t {
            Some(prev) if t == prev => {
                self.stats.repeats += 1;
                self.metrics_recv_ms = recv_ms;
                return; // same sample served from agent cache; nothing new to plot
            }
            Some(prev) if t > prev => {
                let iv = (t - prev) as i64;
                self.stats.interval_ms.push(iv);
                if iv > GAP_MS {
                    self.stats.gaps += 1;
                }
            }
            _ => {}
        }

        // Net rates from cumulative counters against the previous sample.
        if let (Some(prev), Some(prev_t)) = (self.metrics.as_ref(), self.prev_metrics_t)
            && t > prev_t
        {
            let dt = (t - prev_t) as f64 / 1000.0;
            for n in &m.networks {
                if let Some(o) = prev.networks.iter().find(|o| o.name == n.name) {
                    // Counter reset (interface recreated) shows as 0, not a spike.
                    let rx = n.received.saturating_sub(o.received) as f64 / dt;
                    let tx = n.transmitted.saturating_sub(o.transmitted) as f64 / dt;
                    self.net
                        .entry(n.name.clone())
                        .or_insert_with(|| Ring::new(HISTORY_LEN))
                        .push(NetRate { t_ms: t, rx_bps: rx, tx_bps: tx });
                }
            }
            for d in &m.disk_io {
                if let Some(o) = prev.disk_io.iter().find(|o| o.name == d.name) {
                    let r = d.read_bytes.saturating_sub(o.read_bytes) as f64 / dt;
                    let w = d.written_bytes.saturating_sub(o.written_bytes) as f64 / dt;
                    self.disk_io
                        .entry(d.name.clone())
                        .or_insert_with(|| Ring::new(HISTORY_LEN))
                        .push(NetRate { t_ms: t, rx_bps: r, tx_bps: w });
                }
            }
        }

        self.cpu.push((t, m.cpu_total as f64));
        self.mem_pct.push((t, pct(m.mem_used, m.mem_total)));
        self.swap_pct.push((t, pct(m.swap_used, m.swap_total)));
        if let Some(gpus) = &m.gpus {
            for g in gpus {
                self.gpu_pct
                    .entry(g.name.clone())
                    .or_insert_with(|| Ring::new(HISTORY_LEN))
                    .push((t, g.utilization_gpu_pct as f64));
                self.gpu_mem_pct
                    .entry(g.name.clone())
                    .or_insert_with(|| Ring::new(HISTORY_LEN))
                    .push((t, pct(g.mem_used_bytes, g.mem_total_bytes)));
            }
        }
        self.prev_metrics_t = Some(t);
        self.metrics = Some(m);
        self.metrics_recv_ms = recv_ms;
        self.last_new_sample_recv_ms = recv_ms;
    }

    fn on_process_detail(&mut self, d: ProcessMetricsResponse, recv_ms: u64) {
        let pid = d.process.pid;
        let cpu = d.process.cpu_usage as f64;
        let mem = d.process.mem_bytes as f64;
        let start = d.process.start_time;
        let t = if d.cached_at == 0 { recv_ms } else { d.cached_at.saturating_mul(1000) };
        // New pid (selection changed) or reused pid (same pid, different
        // start): start a fresh history rather than gluing two processes
        // together. First sample for a pid always counts as new.
        let prev_pid = self.proc_detail.as_ref().map(|x| x.process.pid);
        let prev_start = self.proc_detail.as_ref().map(|x| x.process.start_time);
        let pid_changed = prev_pid != Some(pid) || prev_start != Some(start);
        if pid_changed {
            self.proc_detail_cpu.clear();
            self.proc_detail_mem.clear();
            self.prev_detail_t = None;
        }
        // Agent cache repeat (same sampled_at as before): refresh the
        // snapshot + recv time like on_metrics, but do not plot a duplicate
        // point. Global `repeats` counts metrics only, so overnight logs stay
        // comparable.
        let is_repeat = self.prev_detail_t == Some(t);
        self.proc_detail = Some(d);
        self.proc_detail_recv_ms = recv_ms;
        if !is_repeat {
            self.proc_detail_cpu.push((t, cpu));
            self.proc_detail_mem.push((t, mem));
            self.prev_detail_t = Some(t);
        }
    }

    /// Agent sample time to `now_ms` on the client clock: end-to-end lag.
    /// Includes any clock offset between the machines.
    pub fn sample_age_ms(&self, now_ms: u64) -> Option<i64> {
        self.metrics.as_ref().map(|m| now_ms as i64 - m.sampled_at_ms as i64)
    }

    /// Time since a new sample last arrived, client clock only. Use this
    /// for LIVE/STALE: immune to clock offset between the machines.
    pub fn since_new_sample_ms(&self, now_ms: u64) -> Option<i64> {
        self.metrics.as_ref().map(|_| now_ms as i64 - self.last_new_sample_recv_ms as i64)
    }

    /// Whether the stored kill reply should still show in the status bar.
    /// Successes are transient; refusals stay until a new kill attempt or a
    /// selection change replaces/clears them (see `clear_kill`).
    pub fn kill_visible(&self, now_ms: u64) -> bool {
        match &self.kill {
            None => false,
            Some(r) if r.ok => now_ms.saturating_sub(self.kill_recv_ms) < KILL_OK_TTL_MS,
            Some(_) => true,
        }
    }

    /// Drop the stored kill reply (new attempt supersedes it, or the
    /// selection it belonged to went away).
    pub fn clear_kill(&mut self) {
        self.kill = None;
    }

    /// One-line summary for logs and acceptance evidence.
    pub fn summary_line(&self, now_ms: u64) -> String {
        let o = |v: Option<i64>| v.map(|x| x.to_string()).unwrap_or_else(|| "na".into());
        let conn = match &self.conn {
            Conn::Connected => "connected".to_string(),
            Conn::Connecting { attempt } => format!("connecting#{attempt}"),
            Conn::Down { reason, .. } => format!("down({reason})"),
        };
        format!(
            "conn={conn} since_new_ms={} interval_p50={} interval_p95={} interval_max={} lag_p50={} lag_p95={} \
proc_interval_p95={} proc_lag_p95={} gaps={} repeats={} reconnects={} samples={}",
            o(self.since_new_sample_ms(now_ms)),
            o(self.stats.interval_p(50.0)), o(self.stats.interval_p(95.0)), o(self.stats.interval_p(100.0)),
            o(self.stats.age_p(50.0)), o(self.stats.age_p(95.0)),
            o(self.stats.proc_interval_p(95.0)), o(self.stats.proc_age_p(95.0)),
            self.stats.gaps, self.stats.repeats, self.reconnects, self.cpu.len(),
        )
    }

    /// Interface names, busiest first by cumulative bytes, `lo` last.
    pub fn interfaces(&self) -> Vec<String> {
        let mut v: Vec<(String, u64)> = self
            .metrics
            .as_ref()
            .map(|m| m.networks.iter().map(|n| (n.name.clone(), n.received + n.transmitted)).collect())
            .unwrap_or_default();
        v.sort_by(|a, b| (a.0 == "lo").cmp(&(b.0 == "lo")).then(b.1.cmp(&a.1)));
        v.into_iter().map(|x| x.0).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{GpuMetrics, NetworkInfo};

    fn metrics(t: u64, rx: u64) -> Metrics {
        Metrics {
            sampled_at_ms: t,
            cpu_total: 10.0,
            cpu_per_core: vec![10.0],
            mem_total: 100,
            mem_used: 40,
            swap_total: 0,
            swap_used: 0,
            hostname: "k".into(),
            cpu_temp_c: None,
            disks: vec![],
            networks: vec![NetworkInfo { name: "eth0".into(), received: rx, transmitted: 0 }],
            gpus: None,
            load_avg: None,
            uptime_s: None,
            disk_io: vec![crate::wire::DiskIo { name: "nvme0n1".into(), read_bytes: rx * 2, written_bytes: rx }],
        }
    }

    #[test]
    fn ring_caps() {
        let mut r = Ring::new(3);
        for i in 0..5 {
            r.push(i);
        }
        assert_eq!(r.iter().copied().collect::<Vec<_>>(), vec![2, 3, 4]);
    }

    #[test]
    fn percentile_nearest_rank() {
        let v: Vec<i64> = (1..=100).collect();
        assert_eq!(percentile(&v, 50.0), Some(50));
        assert_eq!(percentile(&v, 95.0), Some(95));
        assert_eq!(percentile(&[7], 95.0), Some(7));
        assert_eq!(percentile(&[], 50.0), None);
    }

    #[test]
    fn rates_intervals_gaps_and_repeats() {
        let mut s = State::default();
        s.apply(Event::Metrics { m: metrics(1_000, 0), recv_ms: 1_030 }, 0);
        s.apply(Event::Metrics { m: metrics(2_000, 5_000), recv_ms: 2_020 }, 0);
        s.apply(Event::Metrics { m: metrics(2_000, 5_000), recv_ms: 2_500 }, 0); // cache repeat
        s.apply(Event::Metrics { m: metrics(4_500, 5_000), recv_ms: 4_510 }, 0); // gap
        let net: Vec<_> = s.net["eth0"].iter().map(|r| r.rx_bps).collect();
        assert_eq!(net, vec![5_000.0, 0.0]);
        let disk: Vec<_> = s.disk_io["nvme0n1"].iter().map(|r| (r.rx_bps, r.tx_bps)).collect();
        assert_eq!(disk, vec![(10_000.0, 5_000.0), (0.0, 0.0)]);
        assert_eq!(s.stats.interval_ms.iter().copied().collect::<Vec<_>>(), vec![1_000, 2_500]);
        assert_eq!(s.stats.gaps, 1);
        assert_eq!(s.stats.repeats, 1);
        assert_eq!(s.cpu.len(), 3);
        assert_eq!(s.stats.age_p(100.0), Some(500));
        // Staleness keys off the last *new* sample (recv 4_510), not the repeat.
        assert_eq!(s.since_new_sample_ms(5_000), Some(490));
    }

    #[test]
    fn gpu_history_kept_per_name_and_ignores_cache_repeats() {
        let mut s = State::default();
        let gpu = |pct| GpuMetrics {
            name: "NVIDIA GeForce GTX 1650".into(),
            utilization_gpu_pct: pct,
            mem_used_bytes: 1 << 30,
            mem_total_bytes: 4 << 30,
            temperature_c: Some(56.0),
        };
        let mut a = metrics(1_000, 0);
        a.gpus = Some(vec![gpu(10)]);
        s.apply(Event::Metrics { m: a, recv_ms: 1_010 }, 0);
        let mut b = metrics(2_000, 0);
        b.gpus = Some(vec![gpu(50)]);
        s.apply(Event::Metrics { m: b.clone(), recv_ms: 2_010 }, 0);
        s.apply(Event::Metrics { m: b, recv_ms: 2_400 }, 0); // cache repeat
        let ring = &s.gpu_pct["NVIDIA GeForce GTX 1650"];
        assert_eq!(ring.iter().map(|p| p.1).collect::<Vec<_>>(), vec![10.0, 50.0]);
        let mring = &s.gpu_mem_pct["NVIDIA GeForce GTX 1650"];
        assert_eq!(mring.iter().map(|p| p.1).collect::<Vec<_>>(), vec![25.0, 25.0]);
    }

    #[test]
    fn staleness_ignores_clock_offset() {
        let mut s = State::default();
        // Agent clock 10 s ahead of client: lag looks negative, freshness is still right.
        s.apply(Event::Metrics { m: metrics(20_000, 0), recv_ms: 10_005 }, 0);
        assert_eq!(s.sample_age_ms(10_100), Some(-9_900));
        assert_eq!(s.since_new_sample_ms(10_100), Some(95));
        assert!(s.summary_line(10_100).contains("since_new_ms=95"));
    }

    #[test]
    fn reconnect_counted_only_after_connected() {
        let mut s = State::default();
        let down = || Event::Disconnected { reason: "x".into(), retry_in: Duration::ZERO };
        s.apply(down(), 0);
        assert_eq!(s.reconnects, 0);
        s.apply(Event::Connected, 0);
        s.apply(down(), 0);
        assert_eq!(s.reconnects, 1);
    }

    fn detail(pid: u32, cpu: f32, cached_at: u64) -> crate::wire::ProcessMetricsResponse {
        use crate::wire::{DetailedProcessInfo, ProcessMetricsResponse};
        ProcessMetricsResponse {
            process: DetailedProcessInfo {
                pid,
                name: format!("p{pid}"),
                command: String::new(),
                cpu_usage: cpu,
                mem_bytes: 4096,
                virtual_mem_bytes: 8192,
                shared_mem_bytes: None,
                thread_count: 1,
                fd_count: None,
                status: "Sleeping".into(),
                parent_pid: None,
                user_id: 1000,
                group_id: 1000,
                start_time: 1_762_500_000,
                cpu_time_user: 0,
                cpu_time_system: 0,
                read_bytes: None,
                write_bytes: None,
                working_directory: None,
                executable_path: None,
                child_processes: vec![],
                threads: vec![],
            },
            cached_at,
        }
    }

    #[test]
    fn process_detail_history_skips_cache_repeats() {
        let mut s = State::default();
        s.apply(Event::ProcessDetail { d: detail(42, 10.0, 1_000), recv_ms: 1_010_000 }, 0);
        s.apply(Event::ProcessDetail { d: detail(42, 20.0, 1_001), recv_ms: 1_011_000 }, 0);
        s.apply(Event::ProcessDetail { d: detail(42, 20.0, 1_001), recv_ms: 1_011_500 }, 0); // cache repeat
        let vals: Vec<_> = s.proc_detail_cpu.iter().map(|p| p.1).collect();
        assert_eq!(vals, vec![10.0, 20.0]);
        assert_eq!(s.proc_detail.as_ref().unwrap().process.pid, 42);
        // Metrics repeats counter is untouched by detail repeats.
        assert_eq!(s.stats.repeats, 0);
    }

    #[test]
    fn process_detail_history_resets_on_pid_change() {
        let mut s = State::default();
        s.apply(Event::ProcessDetail { d: detail(42, 10.0, 1_000), recv_ms: 1_000_000 }, 0);
        s.apply(Event::ProcessDetail { d: detail(43, 30.0, 1_001), recv_ms: 1_001_000 }, 0);
        let pts: Vec<_> = s.proc_detail_cpu.iter().copied().collect();
        assert_eq!(pts, vec![(1_001_000, 30.0)]);
        let mem: Vec<_> = s.proc_detail_mem.iter().copied().collect();
        assert_eq!(mem, vec![(1_001_000, 4096.0)]);
        assert_eq!(s.proc_detail.as_ref().unwrap().process.pid, 43);
    }

    #[test]
    fn process_detail_mem_history_tracks_rss() {
        let mut s = State::default();
        let mut a = detail(42, 10.0, 1_000);
        a.process.mem_bytes = 1000;
        s.apply(Event::ProcessDetail { d: a, recv_ms: 1_000_000 }, 0);
        let mut b = detail(42, 20.0, 1_001);
        b.process.mem_bytes = 2000;
        s.apply(Event::ProcessDetail { d: b, recv_ms: 1_001_000 }, 0);
        let pts: Vec<_> = s.proc_detail_mem.iter().copied().collect();
        assert_eq!(pts, vec![(1_000_000, 1000.0), (1_001_000, 2000.0)]);
    }

    #[test]
    fn kill_result_kept_and_does_not_touch_metrics() {
        let mut s = State::default();
        assert_eq!(s.kill, None);
        s.apply(Event::KillResult {
            r: KillResult { ok: true, pid: 4242, signal: "TERM".into(), error: None },
            recv_ms: 9_999,
        }, 9_999);
        assert_eq!(s.kill.as_ref().map(|r| r.pid), Some(4242));
        assert_eq!(s.kill_recv_ms, 9_999);
        assert!(s.metrics.is_none()); // no sample arrived
        // A later refusal replaces it.
        s.apply(Event::KillResult {
            r: KillResult { ok: false, pid: 1, signal: "INT".into(), error: Some("pid 1".into()) },
            recv_ms: 10_000,
        }, 10_000);
        assert_eq!(s.kill.as_ref().map(|r| r.ok), Some(false));
    }

    #[test]
    fn kill_success_expires_but_refusal_persists() {
        let mut s = State::default();
        s.apply(Event::KillResult {
            r: KillResult { ok: true, pid: 4242, signal: "TERM".into(), error: None },
            recv_ms: 10_000,
        }, 10_000);
        assert!(s.kill_visible(10_000));
        assert!(s.kill_visible(10_000 + KILL_OK_TTL_MS - 1));
        assert!(!s.kill_visible(10_000 + KILL_OK_TTL_MS), "success should expire");
        // A refusal stays visible until cleared.
        s.apply(Event::KillResult {
            r: KillResult { ok: false, pid: 4242, signal: "TERM".into(), error: Some("stale start_time".into()) },
            recv_ms: 20_000,
        }, 20_000);
        assert!(s.kill_visible(20_000 + 60_000));
        s.clear_kill();
        assert!(!s.kill_visible(20_000 + 60_000));
    }
}
