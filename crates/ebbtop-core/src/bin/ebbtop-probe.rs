//! Headless measurement client. Measures what the GUI would see.
//!
//! ebbtop-probe [--url ws://HOST:PORT/ws] [--seconds 60] [--token-file PATH]
//! Token: --token-file, else env EBBTOP_TOKEN, else none.

use std::time::Duration;

use ebbtop_core::client::{self, ClientConfig, Event};
use ebbtop_core::history::State;

fn arg(name: &str) -> Option<String> {
    let mut it = std::env::args();
    while let Some(a) = it.next() {
        if a == name {
            return it.next();
        }
    }
    None
}

fn fmt_opt(v: Option<i64>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "n/a".into())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let url = arg("--url").unwrap_or_else(|| "ws://127.0.0.1:45976/ws".into());
    let secs: u64 = arg("--seconds").and_then(|s| s.parse().ok()).unwrap_or(60);
    let token = match arg("--token-file") {
        Some(p) => Some(std::fs::read_to_string(p)?.trim().to_string()),
        None => std::env::var("EBBTOP_TOKEN").ok(),
    };
    let cfg = ClientConfig::new(url.clone(), token);
    println!("probe url={url} seconds={secs} token={}", if cfg.token.is_some() { "set" } else { "none" });

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(client::run(cfg, move |e| {
        let _ = tx.send(e);
    }));

    let mut st = State::default();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    let mut metrics_frames = 0u64;
    // Whole-run counters; State keeps only the last 300 samples for percentiles.
    let (mut last_t, mut distinct, mut max_iv, mut gaps) = (0u64, 0u64, 0u64, 0u64);
    loop {
        let ev = tokio::select! {
            e = rx.recv() => match e { Some(e) => e, None => break },
            _ = tokio::time::sleep_until(deadline) => break,
        };
        match &ev {
            Event::Metrics { m, recv_ms } => {
                metrics_frames += 1;
                if m.sampled_at_ms != last_t {
                    distinct += 1;
                    if last_t != 0 {
                        let iv = m.sampled_at_ms.saturating_sub(last_t);
                        max_iv = max_iv.max(iv);
                        if iv > 2000 {
                            gaps += 1;
                        }
                    }
                    last_t = m.sampled_at_ms;
                }
                println!("metrics sampled_at={} recv={} age_ms={} cpu={:.1}", m.sampled_at_ms, recv_ms,
                    *recv_ms as i64 - m.sampled_at_ms as i64, m.cpu_total);
            }
            Event::Processes { p, recv_ms } => println!("procs   sampled_at={} recv={} age_ms={} n={}",
                p.sampled_at_ms, recv_ms, *recv_ms as i64 - p.sampled_at_ms as i64, p.rows.len()),
            Event::Disks { d, .. } => println!("disks   n={}", d.len()),
            other => println!("{other:?}"),
        }
        st.apply(ev, client::now_ms());
    }

    let s = &st.stats;
    println!("--- summary");
    println!("whole run: metrics_frames={metrics_frames} distinct_samples={distinct} repeats={} gaps_over_2000ms={gaps} max_interval_ms={max_iv} reconnects={}",
        s.repeats, st.reconnects);
    println!("percentiles below: last {} samples", s.interval_ms.len());
    println!("metrics interval_ms p50={} p95={} max={}", fmt_opt(s.interval_p(50.0)), fmt_opt(s.interval_p(95.0)), fmt_opt(s.interval_p(100.0)));
    println!("metrics age_ms      p50={} p95={} max={}", fmt_opt(s.age_p(50.0)), fmt_opt(s.age_p(95.0)), fmt_opt(s.age_p(100.0)));
    println!("procs   interval_ms p50={} p95={} max={}", fmt_opt(s.proc_interval_p(50.0)), fmt_opt(s.proc_interval_p(95.0)), fmt_opt(s.proc_interval_p(100.0)));
    println!("procs   age_ms      p50={} p95={} max={}", fmt_opt(s.proc_age_p(50.0)), fmt_opt(s.proc_age_p(95.0)), fmt_opt(s.proc_age_p(100.0)));
    if let Some(m) = &st.metrics {
        match m.gpus.as_deref().filter(|g| !g.is_empty()) {
            Some(gpus) => {
                for g in gpus {
                    let temp = g.temperature_c.map(|t| format!("{t:.0}C")).unwrap_or_else(|| "n/a".into());
                    let used = ebbtop_core::view::bytes(g.mem_used_bytes);
                    let total = ebbtop_core::view::bytes(g.mem_total_bytes);
                    println!("gpu: {} util={}% vram={used}/{total} temp={temp}",
                        g.name, g.utilization_gpu_pct);
                }
            }
            None => println!("gpu: none"),
        }
    }
    if let (Some(p), Some(m)) = (&st.procs, &st.metrics) {
        let mut rows = p.rows.clone();
        rows.sort_by(|a, b| b.cpu_usage.total_cmp(&a.cpu_usage));
        println!("top by cpu at procs sampled_at={} (mem_total={}):", p.sampled_at_ms, m.mem_total);
        for r in rows.iter().take(5) {
            println!("  {:>7} {:<20} {:<10} {:<10} cpu={:5.1} mem={:5.1}%", r.pid, r.name, r.user, r.state,
                r.cpu_usage, r.mem_bytes as f64 * 100.0 / m.mem_total.max(1) as f64);
        }
    }
    Ok(())
}
