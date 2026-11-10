//! ebbtop agent entrypoint: serves host metrics over WebSocket at /ws.
//! Owned replacement for the vendored socktop_agent. Wire protocol is
//! intentionally unchanged so existing ebbtop clients keep working.

mod gpu;
mod kill;
mod proc_util;
mod metrics;
mod proto;
mod state;
mod types;
mod ws;

use axum::{Router, http::StatusCode, routing::get};
use std::net::SocketAddr;

use state::AppState;

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

async fn async_main() -> anyhow::Result<()> {
    if arg_flag("--version") || arg_flag("-V") {
        println!("ebbtop-agent {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let state = AppState::new();

    async fn healthz() -> StatusCode {
        StatusCode::OK
    }
    let app = Router::new()
        .route("/ws", get(ws::ws_handler))
        .route("/healthz", get(healthz))
        .with_state(state.clone());

    // Plain ws:// only. Tailnet provides the transport security; the token
    // in the query string is the gate. No TLS listener by design.
    let addr = SocketAddr::new(bind_ip()?, port());
    println!("ebbtop-agent: Listening on ws://{addr}/ws");
    axum_server::bind(addr)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}
