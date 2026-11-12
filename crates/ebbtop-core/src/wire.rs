//! Wire format of the patched socktop_agent (see agent/socktop_agent).
//!
//! Request/response over one WebSocket, one request in flight at a time:
//! - `get_metrics`   -> JSON `Metrics`, gzipped as a binary frame when large
//! - `get_disks`     -> JSON `Vec<DiskInfo>`, same framing
//! - `get_processes` -> protobuf `Processes`, gzipped when large
//! - `get_process_metrics:<pid>` -> JSON `ProcessMetricsResponse` (or
//!   `{"error": ..., "request": "get_process_metrics", "pid": N}` when the
//!   pid is gone), same JSON framing. Client polls it at 1 s only while a
//!   pid is selected (see client.rs); the decode maps the error shape to
//!   `None` so a dying process does not drop the connection.
//! - `kill:<pid>:<start_time_ms>:<TERM|INT>` (Phase 2, on demand only, queued
//!   on the same connection) -> JSON `KillResult`
//!
//! Fields marked "ebbtop" extend the original socktop protocol. They are
//! `#[serde(default)]` / proto3 defaults so a legacy agent still decodes
//! (with empty user/state, `sampled_at_ms == 0` and `start_time_ms == 0`).

use std::io::Read;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DiskInfo {
    pub name: String,
    pub total: u64,
    pub available: u64,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub is_partition: bool,
}

/// Cumulative byte counters since boot, per interface.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct NetworkInfo {
    pub name: String,
    pub received: u64,
    pub transmitted: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct GpuMetrics {
    pub name: String,
    pub utilization_gpu_pct: u32,
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
    /// ebbtop: edge temperature in °C. Optional so an older agent's
    /// payload (without the field) still decodes.
    #[serde(default)]
    pub temperature_c: Option<f32>,
}

/// ebbtop: cumulative bytes since boot for one whole disk.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DiskIo {
    pub name: String,
    pub read_bytes: u64,
    pub written_bytes: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Metrics {
    /// Agent epoch ms when this snapshot was collected.
    pub sampled_at_ms: u64,
    pub cpu_total: f32,
    pub cpu_per_core: Vec<f32>,
    pub mem_total: u64,
    pub mem_used: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub hostname: String,
    #[serde(default)]
    pub cpu_temp_c: Option<f32>,
    #[serde(default)]
    pub disks: Vec<DiskInfo>,
    #[serde(default)]
    pub networks: Vec<NetworkInfo>,
    #[serde(default)]
    pub gpus: Option<Vec<GpuMetrics>>,
    /// ebbtop.
    #[serde(default)]
    pub load_avg: Option<[f64; 3]>,
    /// ebbtop.
    #[serde(default)]
    pub uptime_s: Option<u64>,
    /// ebbtop.
    #[serde(default)]
    pub disk_io: Vec<DiskIo>,
    /// ebbtop: CPU model string the agent read from `/proc/cpuinfo`
    /// (`model name`). None on a legacy agent, a non-Linux agent or an
    /// unreadable /proc — the GUI then shows the type label alone.
    #[serde(default)]
    pub cpu_model: Option<String>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Process {
    #[prost(uint32, tag = "1")]
    pub pid: u32,
    #[prost(string, tag = "2")]
    pub name: String,
    /// Share of total machine CPU, 0..100 (btop default, not per core).
    #[prost(float, tag = "3")]
    pub cpu_usage: f32,
    /// RSS bytes.
    #[prost(uint64, tag = "4")]
    pub mem_bytes: u64,
    /// ebbtop: owner name.
    #[prost(string, tag = "5")]
    pub user: String,
    /// ebbtop: state, e.g. "Sleeping".
    #[prost(string, tag = "6")]
    pub state: String,
    /// ebbtop: agent epoch ms when the process started (PID-reuse guard
    /// for Phase 2 kill). 0 = unpatched agent.
    #[prost(uint64, tag = "7")]
    pub start_time_ms: u64,
    /// ebbtop: cgroup path the process runs in, e.g.
    /// "/user.slice/user-1000.slice/user@1000.service/app.slice/davy-pg.service/libpod-payload-<id>".
    /// Empty on a legacy agent or a platform without cgroups. The client
    /// decides how to group; see `view::workload_key`.
    #[prost(string, tag = "8")]
    pub cgroup: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Processes {
    #[prost(uint64, tag = "1")]
    pub process_count: u64,
    #[prost(message, repeated, tag = "2")]
    pub rows: Vec<Process>,
    /// ebbtop: agent epoch ms when rows were collected. 0 = legacy agent.
    #[prost(uint64, tag = "3")]
    pub sampled_at_ms: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ThreadInfo {
    pub tid: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub cpu_time_user: u64,
    #[serde(default)]
    pub cpu_time_system: u64,
    #[serde(default)]
    pub status: String,
}

/// One process from `get_process_metrics:<pid>`. Mirrors the agent's
/// `DetailedProcessInfo` (agent/socktop_agent/src/types.rs). `child_processes`
/// is one level only and `threads` is one entry per task; the detail pane
/// shows the top entries of each.
/// All but `pid` default so an older agent's payload still decodes.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct DetailedProcessInfo {
    pub pid: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub cpu_usage: f32,
    #[serde(default)]
    pub mem_bytes: u64,
    #[serde(default)]
    pub virtual_mem_bytes: u64,
    #[serde(default)]
    pub shared_mem_bytes: Option<u64>,
    #[serde(default)]
    pub thread_count: u32,
    #[serde(default)]
    pub fd_count: Option<u32>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub parent_pid: Option<u32>,
    #[serde(default)]
    pub user_id: u32,
    #[serde(default)]
    pub group_id: u32,
    /// Unix seconds when the process started (sysinfo).
    #[serde(default)]
    pub start_time: u64,
    /// CPU microseconds.
    #[serde(default)]
    pub cpu_time_user: u64,
    /// CPU microseconds.
    #[serde(default)]
    pub cpu_time_system: u64,
    #[serde(default)]
    pub read_bytes: Option<u64>,
    #[serde(default)]
    pub write_bytes: Option<u64>,
    #[serde(default)]
    pub working_directory: Option<String>,
    #[serde(default)]
    pub executable_path: Option<String>,
    #[serde(default)]
    pub child_processes: Vec<DetailedProcessInfo>,
    #[serde(default)]
    pub threads: Vec<ThreadInfo>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ProcessMetricsResponse {
    pub process: DetailedProcessInfo,
    /// Agent epoch seconds when this snapshot was collected (cache hits keep
    /// it). The client multiplies by 1000 for the history ring clock.
    #[serde(default)]
    pub cached_at: u64,
}

/// Text request the client sends to poll one pid. Built from a `u32` so the
/// agent's `pid_str.parse::<u32>()` can never fail.
pub fn encode_process_metrics_request(pid: u32) -> String {
    format!("get_process_metrics:{pid}")
}

/// Raw frame payload as received.
pub enum Frame<'a> {
    Text(&'a str),
    Binary(&'a [u8]),
}

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
/// Refuse to inflate past this; a full process list is ~10-100 KB.
const MAX_INFLATED: u64 = 32 * 1024 * 1024;

fn maybe_gunzip(b: &[u8]) -> anyhow::Result<std::borrow::Cow<'_, [u8]>> {
    if b.len() >= 2 && b[..2] == GZIP_MAGIC {
        let mut out = Vec::with_capacity(b.len() * 4);
        flate2::read::GzDecoder::new(b)
            .take(MAX_INFLATED + 1)
            .read_to_end(&mut out)?;
        anyhow::ensure!(out.len() as u64 <= MAX_INFLATED, "inflated frame too large");
        Ok(out.into())
    } else {
        Ok(b.into())
    }
}

fn json<T: serde::de::DeserializeOwned>(f: Frame<'_>) -> anyhow::Result<T> {
    match f {
        Frame::Text(s) => Ok(serde_json::from_str(s)?),
        Frame::Binary(b) => Ok(serde_json::from_slice(&maybe_gunzip(b)?)?),
    }
}

fn raw_value(f: Frame<'_>) -> anyhow::Result<serde_json::Value> {
    match f {
        Frame::Text(s) => Ok(serde_json::from_str(s)?),
        Frame::Binary(b) => Ok(serde_json::from_slice(&maybe_gunzip(b)?)?),
    }
}

pub fn decode_metrics(f: Frame<'_>) -> anyhow::Result<Metrics> {
    json(f)
}

pub fn decode_disks(f: Frame<'_>) -> anyhow::Result<Vec<DiskInfo>> {
    json(f)
}

pub fn decode_processes(f: Frame<'_>) -> anyhow::Result<Processes> {
    match f {
        Frame::Text(s) => anyhow::bail!("expected protobuf process frame, got text: {:.80}", s),
        Frame::Binary(b) => Ok(<Processes as prost::Message>::decode(&*maybe_gunzip(b)?)?),
    }
}

/// Decode a `get_process_metrics` reply. `Ok(None)` is the agent's
/// `{"error": "Process N not found", ...}` shape (pid died between list and
/// detail poll): callers treat it as "no detail for this pid", not an error,
/// so it never drops the connection.
pub fn decode_process_detail(f: Frame<'_>) -> anyhow::Result<Option<ProcessMetricsResponse>> {
    let v = raw_value(f)?;
    if let Ok(resp) = serde_json::from_value::<ProcessMetricsResponse>(v.clone()) {
        return Ok(Some(resp));
    }
    if v.get("error").is_some() {
        return Ok(None);
    }
    anyhow::bail!("unexpected process detail response: {:.200}", v.to_string())
}

/// Signal for a Phase 2 kill request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillSignal {
    Term,
    Int,
}

impl KillSignal {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Term => "TERM",
            Self::Int => "INT",
        }
    }
}

/// An on-demand kill job. The client sends it between ticks on the same
/// connection (one request in flight), never per-tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillJob {
    pub pid: u32,
    /// Agent epoch ms when the process started, as last seen in the table.
    pub start_time_ms: u64,
    pub signal: KillSignal,
}

/// Exact request grammar: `kill:<pid>:<start_time_ms>:<TERM|INT>`.
pub fn format_kill_request(pid: u32, start_time_ms: u64, signal: KillSignal) -> String {
    format!("kill:{pid}:{start_time_ms}:{}", signal.as_str())
}

/// Agent reply to a kill request (JSON text frame, same framing rules as
/// metrics; `error` is a short stable reason, token never included).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct KillResult {
    pub ok: bool,
    pub pid: u32,
    #[serde(default)]
    pub signal: String,
    #[serde(default)]
    pub error: Option<String>,
}

pub fn decode_kill(f: Frame<'_>) -> anyhow::Result<KillResult> {
    json(f)

}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use prost::Message;
    use std::io::Write;

    fn gz(b: &[u8]) -> Vec<u8> {
        let mut e = GzEncoder::new(Vec::new(), Compression::fast());
        e.write_all(b).unwrap();
        e.finish().unwrap()
    }

    const METRICS: &str = r#"{"sampled_at_ms":1762500000123,"cpu_total":12.5,"cpu_per_core":[10.0,15.0],
      "mem_total":1000,"mem_used":250,"swap_total":0,"swap_used":0,"hostname":"k-server",
      "cpu_temp_c":null,"disks":[],"networks":[{"name":"eth0","received":10,"transmitted":20}],
      "top_processes":[],"gpus":null}"#;

    #[test]
    fn metrics_text_and_gzip_decode_identically() {
        let a = decode_metrics(Frame::Text(METRICS)).unwrap();
        let b = decode_metrics(Frame::Binary(&gz(METRICS.as_bytes()))).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.sampled_at_ms, 1762500000123);
        assert_eq!(a.networks[0].transmitted, 20);
        assert_eq!(a.load_avg, None); // unpatched agent
        assert!(a.disk_io.is_empty());
    }

    /// The `METRICS` fixture above predates `cpu_model`; it must keep
    /// decoding, with the field defaulting to None (legacy agent).
    #[test]
    fn metrics_without_cpu_model_decodes_legacy() {
        assert!(!METRICS.contains("cpu_model"), "fixture must stay legacy-shaped");
        let m = decode_metrics(Frame::Text(METRICS)).unwrap();
        assert_eq!(m.cpu_model, None);
        // Same for an agent that explicitly sends null.
        let explicit = METRICS.replace(r#""gpus":null}"#, r#""gpus":null,"cpu_model":null}"#);
        assert_eq!(decode_metrics(Frame::Text(&explicit)).unwrap().cpu_model, None);
    }

    #[test]
    fn cpu_model_decodes_when_present() {
        let s = METRICS.replace(
            r#""gpus":null}"#,
            r#""gpus":null,"cpu_model":"Intel(R) Core(TM) i7-9750H CPU @ 2.60GHz"}"#,
        );
        let m = decode_metrics(Frame::Text(&s)).unwrap();
        assert_eq!(m.cpu_model.as_deref(), Some("Intel(R) Core(TM) i7-9750H CPU @ 2.60GHz"));
    }

    #[test]
    fn patched_metrics_fields() {
        let s = METRICS.replace(r#""gpus":null}"#,
            r#""gpus":null,"load_avg":[0.5,0.6,0.7],"uptime_s":3600,"disk_io":[{"name":"nvme0n1","read_bytes":1024,"written_bytes":2048}]}"#);
        let m = decode_metrics(Frame::Text(&s)).unwrap();
        assert_eq!(m.load_avg, Some([0.5, 0.6, 0.7]));
        assert_eq!(m.uptime_s, Some(3600));
        assert_eq!(m.disk_io[0].written_bytes, 2048);
    }

    #[test]
    fn gpu_metrics_temp_is_optional() {
        let with = METRICS.replace(r#""gpus":null}"#,
            r#""gpus":[{"name":"NVIDIA GeForce GTX 1650","utilization_gpu_pct":37,"mem_used_bytes":1073741824,"mem_total_bytes":4294967296,"temperature_c":56.0}]}"#);
        let m = decode_metrics(Frame::Text(&with)).unwrap();
        let g = &m.gpus.as_ref().unwrap()[0];
        assert_eq!(g.utilization_gpu_pct, 37);
        assert_eq!(g.temperature_c, Some(56.0));

        // An older agent omits the field entirely: still decodes.
        let without = METRICS.replace(r#""gpus":null}"#,
            r#""gpus":[{"name":"NVIDIA GeForce GTX 1650","utilization_gpu_pct":37,"mem_used_bytes":1,"mem_total_bytes":2}]}"#);
        let m = decode_metrics(Frame::Text(&without)).unwrap();
        assert_eq!(m.gpus.unwrap()[0].temperature_c, None);
    }

    #[test]
    fn processes_round_trip_plain_and_gzip() {
        let p = Processes {
            process_count: 1,
            rows: vec![Process {
                pid: 42,
                name: "btop".into(),
                cpu_usage: 3.5,
                mem_bytes: 4096,
                user: "kyler".into(),
                state: "Sleeping".into(),
                start_time_ms: 1_700_000_002_500,
                cgroup: "/user.slice/user-1000.slice/user@1000.service/app.slice/davy-pg.service".into(),
            }],
            sampled_at_ms: 99,
        };
        let raw = p.encode_to_vec();
        assert_eq!(decode_processes(Frame::Binary(&raw)).unwrap(), p);
        assert_eq!(decode_processes(Frame::Binary(&gz(&raw))).unwrap(), p);
    }

    /// Bytes as an unpatched agent would send them (fields 1-4 only).
    #[test]
    fn unpatched_agent_processes_decode_with_defaults() {
        #[derive(Clone, PartialEq, prost::Message)]
        struct OldProcess {
            #[prost(uint32, tag = "1")]
            pid: u32,
            #[prost(string, tag = "2")]
            name: String,
            #[prost(float, tag = "3")]
            cpu_usage: f32,
            #[prost(uint64, tag = "4")]
            mem_bytes: u64,
        }
        #[derive(Clone, PartialEq, prost::Message)]
        struct OldProcesses {
            #[prost(uint64, tag = "1")]
            process_count: u64,
            #[prost(message, repeated, tag = "2")]
            rows: Vec<OldProcess>,
        }
        let old = OldProcesses {
            process_count: 1,
            rows: vec![OldProcess { pid: 7, name: "x".into(), cpu_usage: 1.0, mem_bytes: 2 }],
        };
        let p = decode_processes(Frame::Binary(&old.encode_to_vec())).unwrap();
        assert_eq!(p.rows[0].pid, 7);
        assert_eq!(p.rows[0].user, "");
        assert_eq!(p.sampled_at_ms, 0);
        assert_eq!(p.rows[0].start_time_ms, 0); // unpatched agent: kill unsupported
    }

    #[test]
    fn kill_grammar_and_reply_decode() {
        assert_eq!(format_kill_request(4242, 1_700_000_002_500, KillSignal::Term), "kill:4242:1700000002500:TERM");
        assert_eq!(format_kill_request(7, 0, KillSignal::Int), "kill:7:0:INT");
        let ok = decode_kill(Frame::Text(r#"{"ok":true,"pid":4242,"signal":"TERM","error":null}"#)).unwrap();
        assert!(ok.ok && ok.pid == 4242 && ok.signal == "TERM" && ok.error.is_none());
        // Large-error gzip variant decodes the same way.
        let err = decode_kill(Frame::Binary(&gz(br#"{"ok":false,"pid":1,"signal":"","error":"pid 1"}"#))).unwrap();
        assert!(!err.ok && err.error.as_deref() == Some("pid 1"));
        assert!(decode_kill(Frame::Text("nope")).is_err());
    }

    #[test]
    fn text_process_frame_is_an_error() {
        assert!(decode_processes(Frame::Text("{}")).is_err());
    }

    const DETAIL: &str = r#"{"process":{"pid":4242,"name":"hottest-proc","command":"hottest-proc --flag","cpu_usage":87.5,"mem_bytes":3221225472,"virtual_mem_bytes":4294967296,"shared_mem_bytes":null,"thread_count":3,"fd_count":null,"status":"Running","parent_pid":1,"user_id":1000,"group_id":1000,"start_time":1762500000,"cpu_time_user":123456,"cpu_time_system":654,"read_bytes":1024,"write_bytes":2048,"working_directory":"/tmp","executable_path":"/usr/bin/hottest-proc","child_processes":[],"threads":[{"tid":4242,"name":"hottest-proc","cpu_time_user":123456,"cpu_time_system":654,"status":"Running"}]},"cached_at":1762500000}"#;

    #[test]
    fn process_detail_text_and_gzip_decode_identically() {
        let a = decode_process_detail(Frame::Text(DETAIL)).unwrap().expect("detail");
        let b = decode_process_detail(Frame::Binary(&gz(DETAIL.as_bytes()))).unwrap().expect("detail gzip");
        assert_eq!(a, b);
        assert_eq!(a.process.pid, 4242);
        assert_eq!(a.process.name, "hottest-proc");
        assert!((a.process.cpu_usage - 87.5).abs() < 1e-6);
        assert_eq!(a.process.mem_bytes, 3 << 30);
        assert_eq!(a.process.status, "Running");
        assert_eq!(a.process.user_id, 1000);
        assert_eq!(a.process.thread_count, 3);
        assert_eq!(a.process.threads.len(), 1);
        assert_eq!(a.process.threads[0].tid, 4242);
        assert_eq!(a.cached_at, 1762500000);
        assert_eq!(encode_process_metrics_request(4242), "get_process_metrics:4242");
    }

    #[test]
    fn process_detail_missing_pid_maps_to_none() {
        let err = r#"{"error":"Process 999999 not found","request":"get_process_metrics","pid":999999}"#;
        assert_eq!(decode_process_detail(Frame::Text(err)).unwrap(), None);
    }

    #[test]
    fn process_detail_unexpected_shape_is_an_error() {
        assert!(decode_process_detail(Frame::Text("{}")).is_err());
    }

    #[test]
    fn process_detail_old_agent_payload_with_defaults() {
        // An older agent omits the optional scalars and children/threads.
        let s = r#"{"process":{"pid":7,"name":"x","cpu_usage":1.5,"mem_bytes":4096},"cached_at":99}"#;
        let d = decode_process_detail(Frame::Text(s)).unwrap().expect("detail old");
        assert_eq!(d.process.pid, 7);
        assert_eq!(d.process.status, "");
        assert_eq!(d.process.user_id, 0);
        assert!(d.process.child_processes.is_empty());
        assert!(d.process.threads.is_empty());
        assert_eq!(d.cached_at, 99);
    }
}
