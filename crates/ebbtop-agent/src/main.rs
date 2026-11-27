//! ebbtop agent entrypoint: serves host metrics over WebSocket at /ws.
//! Owned replacement for the vendored socktop_agent. Wire protocol is
//! intentionally unchanged so existing ebbtop clients keep working.
//!
//! P3a adds two control-plane surfaces to the same binary, neither of which
//! changes the WebSocket data path:
//! * `--pair` mints a short-lived pairing code and exits (the running agent
//!   consumes it at `POST /pair`);
//! * a UDP discovery responder on `EBBTOP_DISCOVERY_PORT` (default 45978)
//!   that answers an exact probe with name, version and per-boot nonce.
//!
//! macOS (Apple Silicon) additions, all inert on Linux:
//! * `--env-file <path>` reads `EBBTOP_*` settings from a 0600 file, because
//!   launchd has no `EnvironmentFile`. The LaunchAgent therefore carries a
//!   path, never a token, in the plist and in the process argument list;
//! * [`platform::startup_policy`] in `Enforce` mode refuses to serve without
//!   a nonempty token or with a bind address that is not loopback/private;
//! * `--probe <host:port> [--token-file <path>]` performs one authenticated
//!   WebSocket exchange and prints `probe: ok|denied|failed`, which is how
//!   the installer proves readiness instead of assuming it.

use std::net::SocketAddr;
use std::path::PathBuf;

use ebbtop_agent::discovery::{self, DiscoveryConfig};
use ebbtop_agent::pairing::{self, CODE_TTL_MS, PairingStore};
use ebbtop_agent::platform::{GatePolicy, PolicyError};
use ebbtop_agent::probe::{self, ProbeConfig};
use ebbtop_agent::server;
use ebbtop_agent::state::AppState;

fn arg_flag(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}
fn arg_value(name: &str) -> Option<String> {
    let mut it = std::env::args();
    while let Some(a) = it.next() {
        if a == name {
            return it.next();
        }
    }
    None
}

/// The gate this build applies.
///
/// * **macOS (Apple Silicon): always [`GatePolicy::Enforce`].** There is no
///   environment override, by design: a laptop that answers on an untrusted
///   network with no credential is the exact failure this release exists to
///   prevent, and a variable that can turn that off is a variable an attacker
///   or a stray `agent.env` line can set. `EBBTOP_AGENT_REQUIRE_AUTH` is
///   *ignored* on Darwin, so a Linux fixture can never be mistaken for
///   evidence about the Mac build.
/// * **Linux: [`GatePolicy::Permissive`]** — the historical behaviour the
///   installed units, installers and README describe, unchanged. On Linux the
///   variable is an opt-in that lets the test harness exercise the enforced
///   gate (`=1`) and restores the default (`=0`); see
///   `deploy/linux/test-*.sh` and the agent's own tests.
fn gate_policy() -> GatePolicy {
    ebbtop_agent::platform::resolve_gate(
        cfg!(target_os = "macos"),
        std::env::var("EBBTOP_AGENT_REQUIRE_AUTH").ok().as_deref(),
    )
}

/// Listen address from --bind, EBBTOP_BIND, or SOCKTOP_BIND (compat).
fn bind_ip() -> anyhow::Result<std::net::IpAddr> {
    let s = arg_value("--bind")
        .or_else(|| std::env::var("EBBTOP_BIND").ok())
        .or_else(|| std::env::var("SOCKTOP_BIND").ok())
        .unwrap_or_else(|| "0.0.0.0".to_string());
    s.parse()
        .map_err(|e| anyhow::anyhow!("invalid bind address {s:?}: {e}"))
}

/// Port from --port/-p, EBBTOP_PORT, or SOCKTOP_PORT (compat). Default 3000.
fn port() -> u16 {
    arg_value("--port")
        .or_else(|| arg_value("-p"))
        .or_else(|| std::env::var("EBBTOP_PORT").ok())
        .or_else(|| std::env::var("SOCKTOP_PORT").ok())
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(3000)
}

fn main() -> anyhow::Result<()> {
    // --env-file must be applied before anything reads the environment. The
    // runtime is not built yet, so this is still the only thread.
    if let Some(path) = arg_value("--env-file") {
        match ebbtop_agent::envfile::load(std::path::Path::new(&path)) {
            Ok(n) => println!("ebbtop-agent: applied {n} setting(s) from {path}"),
            Err(e) => {
                eprintln!("ebbtop-agent: {e}");
                std::process::exit(2);
            }
        }
    }

    #[cfg(feature = "logging")]
    tracing_subscriber::fmt::init();

    // Small fixed thread pool: the agent is I/O-bound (/proc reads, sysinfo,
    // one WebSocket at a time) with no CPU-intensive work.
    let worker_threads = std::env::var("EBBTOP_WORKER_THREADS")
        .or_else(|_| std::env::var("SOCKTOP_WORKER_THREADS"))
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(2)
        .clamp(1, 16);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .thread_name("ebbtop-agent")
        .enable_all()
        .build()?;

    runtime.block_on(async_main())
}

/// `ebbtop-agent --pair [--state-dir DIR]`: mint a pairing code, write it to
/// the pending-code file the running agent reads, print it once, exit.
///
/// Prints only the code and instructions — never the agent token — and the
/// daemon never writes a code to the journal.
fn pair_command() -> anyhow::Result<()> {
    let dir = arg_value("--state-dir").map(PathBuf::from);
    let store = PairingStore::new(dir.unwrap_or_else(pairing::state_dir));
    let pending = store.mint()?;
    println!("Pairing code: {}", pending.code);
    println!(
        "Valid for {} minutes, single use. Enter it in EbbTop to add this host.",
        CODE_TTL_MS / 60_000
    );
    println!("(State directory: {})", store.dir().display());
    Ok(())
}

/// `ebbtop-agent --probe <host:port> [--token-file PATH] [--timeout-ms N]`.
///
/// Reads the token from a file (or an already-set `EBBTOP_TOKEN`), never from
/// the argument list, and prints exactly one line. Exit 0 = an authenticated
/// metrics reply arrived, 3 = refused (missing/wrong token), 1 = no usable
/// listener.
///
/// `--dump-metrics`, `--dump-disks` and `--dump-process <pid>` print the raw
/// reply body instead of the status line. They exist for the native macOS
/// verification, which compares the agent's own numbers against `sysctl`,
/// `df` and a known workload on the same host; the probe is the agent's own
/// client, so no second protocol implementation is involved.
fn probe_command() -> anyhow::Result<()> {
    let Some(addr) = arg_value("--probe") else {
        unreachable!("probe_command is only reached with --probe");
    };
    let (host, port) = match probe::split_host_port(&addr) {
        Ok(v) => v,
        Err(e) => {
            println!("probe: failed: {e}");
            std::process::exit(1);
        }
    };
    let token = match arg_value("--token-file") {
        Some(p) => match std::fs::read_to_string(&p) {
            Ok(t) => Some(t.trim().to_string()),
            Err(e) => {
                // The path is fine to print; the contents are not read.
                println!("probe: failed: cannot read --token-file {p}: {e}");
                std::process::exit(1);
            }
        },
        None => std::env::var("EBBTOP_TOKEN").ok().filter(|t| !t.is_empty()),
    };
    let timeout_ms: u64 = arg_value("--timeout-ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3_000);

    // Which frame to ask for, and whether to print it raw.
    let (request, dump) = if arg_flag("--dump-metrics") {
        (None, true)
    } else if arg_flag("--dump-disks") {
        (Some("get_disks".to_string()), true)
    } else if arg_flag("--dump-process") {
        match arg_value("--dump-process") {
            Some(pid) if pid.chars().all(|c| c.is_ascii_digit()) && !pid.is_empty() => {
                (Some(format!("get_process_metrics:{pid}")), true)
            }
            _ => {
                println!("probe: failed: --dump-process needs a numeric pid");
                std::process::exit(1);
            }
        }
    } else {
        (None, false)
    };

    let outcome = probe::run(&ProbeConfig {
        host,
        port,
        token,
        timeout: std::time::Duration::from_millis(timeout_ms),
        request,
        dump,
    });
    if dump {
        println!("{}", outcome.dumped());
    } else {
        println!("{}", outcome.line());
    }
    let code = outcome.exit_code();
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

async fn async_main() -> anyhow::Result<()> {
    if arg_flag("--version") || arg_flag("-V") {
        println!("ebbtop-agent {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if arg_flag("--pair") {
        return pair_command();
    }

    if arg_flag("--probe") {
        return probe_command();
    }

    let policy = gate_policy();
    let state = AppState::new();
    let bind = bind_ip()?;
    let ws_port = port();

    // Fail closed before binding anything: no listener, no discovery socket,
    // no served frame. On Linux this is a no-op (GatePolicy::Permissive).
    match ebbtop_agent::platform::startup_policy(policy, bind, state.auth_token.as_deref()) {
        Ok(()) => {}
        Err(PolicyError(msg)) => {
            eprintln!("ebbtop-agent: {msg}");
            std::process::exit(2);
        }
    }

    // Discovery is a convenience: a bind failure must not stop monitoring.
    // It listens on every IPv4 interface (a socket bound to one address never
    // receives broadcasts) and answers only exact probes from private
    // sources; see README "Why discovery is UDP broadcast". The default is
    // per-platform: off for the Mac pilot, on for Linux as before.
    let cfg =
        DiscoveryConfig::from_env_or_default(ebbtop_agent::platform::discovery_default(policy));
    match discovery::spawn(cfg, state.clone(), ws_port).await {
        Ok(Some(_task)) => println!(
            "ebbtop-agent: discovery responder on udp://{}/{}",
            discovery::bind_addr(cfg.port).ip(),
            cfg.port
        ),
        Ok(None) => println!("ebbtop-agent: discovery disabled (EBBTOP_AGENT_DISCOVERY)"),
        Err(e) => eprintln!(
            "ebbtop-agent: discovery responder unavailable on udp://0.0.0.0:{}: {e}",
            cfg.port
        ),
    }

    // Plain ws:// only. Tailnet provides the transport security; the token
    // in the query string is the gate. No TLS listener by design.
    let addr = SocketAddr::new(bind, ws_port);
    println!("ebbtop-agent: Listening on ws://{addr}/ws");
    axum_server::bind(addr)
        .serve(server::router(state).into_make_service())
        .await?;
    Ok(())
}
