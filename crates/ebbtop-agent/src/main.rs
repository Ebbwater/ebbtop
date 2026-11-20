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

use std::net::SocketAddr;
use std::path::PathBuf;

use ebbtop_agent::discovery::{self, DiscoveryConfig};
use ebbtop_agent::pairing::{self, CODE_TTL_MS, PairingStore};
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

async fn async_main() -> anyhow::Result<()> {
    if arg_flag("--version") || arg_flag("-V") {
        println!("ebbtop-agent {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if arg_flag("--pair") {
        return pair_command();
    }

    let state = AppState::new();
    let bind = bind_ip()?;
    let ws_port = port();

    // Discovery is a convenience: a bind failure must not stop monitoring.
    // It listens on every IPv4 interface (a socket bound to one address never
    // receives broadcasts) and answers only exact probes from private
    // sources; see README "Why discovery is UDP broadcast".
    let cfg = DiscoveryConfig::from_env();
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
