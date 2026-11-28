//! Connection loop: connect, poll on a fixed tick, reconnect with backoff.
//!
//! Never returns. Every outcome is reported as an [`Event`]. The token is
//! never placed in an event or error string.
//!
//! Phase 2 kill jobs arrive on a channel and are sent between ticks on the
//! *same* connection (one request in flight, exactly like metrics), so the
//! 1 s poll loop is untouched. A kill reply that times out drops the
//! connection: the next frame could otherwise be misread as a metrics
//! reply, and the reconnect re-establishes frame alignment.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{MissedTickBehavior, timeout};
use tokio_tungstenite::tungstenite::Message;

/// Inbound message cap for the client's WebSocket. Real agent frames are
/// 10-100 KB compressed (inflate is capped at 32 MiB in `wire`), and the
/// largest legitimate request is a handful of bytes; tungstenite's 64 MiB
/// default would let a hostile agent park a 64 MiB buffer per message
/// before the decoder ever sees it. 4 MiB is ~40x the largest real frame.
const MAX_WS_MESSAGE: usize = 4 * 1024 * 1024;

// The struct is #[non_exhaustive] upstream, so it cannot be built with a
// struct expression from here; mutate the default instead.
#[allow(clippy::field_reassign_with_default)]
fn ws_config() -> tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
    let mut cfg = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default();
    cfg.max_message_size = Some(MAX_WS_MESSAGE);
    cfg
}

use crate::wire::{
    self, DiskInfo, Frame, KillJob, KillResult, Metrics, ProcessMetricsResponse, Processes,
};

/// `detail_pid` value meaning "no pid selected". PIDs are below 2^22 on
/// Linux, so u32::MAX can never be a real pid.
pub const NO_DETAIL_PID: u32 = u32::MAX;

#[derive(Clone)]
pub struct ClientConfig {
    /// e.g. `ws://agent-host:45976/ws`
    pub url: String,
    pub token: Option<String>,
    /// Poll period. 1000 ms is the design target.
    pub tick: Duration,
    /// Request disks every N ticks.
    pub disks_every: u32,
    /// Per-request reply deadline. A miss drops the connection.
    pub reply_timeout: Duration,
    pub connect_timeout: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    /// Selected pid for `get_process_metrics`, shared with the GUI:
    /// `NO_DETAIL_PID` means "not selected, do not poll detail". The GUI
    /// stores here when its `selected` changes; the session loop reads it
    /// once per 1 s tick, so there is no second poll loop and no lock on the
    /// hot path.
    pub detail_pid: Arc<AtomicU32>,
}

impl std::fmt::Debug for ClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientConfig")
            .field("url", &self.url)
            .field("token", &self.token.as_ref().map(|_| "<set>"))
            .field("tick", &self.tick)
            .finish_non_exhaustive()
    }
}

impl ClientConfig {
    pub fn new(url: impl Into<String>, token: Option<String>) -> Self {
        Self {
            url: url.into(),
            token,
            tick: Duration::from_millis(1000),
            disks_every: 5,
            reply_timeout: Duration::from_millis(3000),
            connect_timeout: Duration::from_millis(5000),
            backoff_min: Duration::from_millis(500),
            // Short cap: a monitor should be back within a few seconds of the
            // agent returning. A refused connect on the tailnet costs nothing.
            backoff_max: Duration::from_secs(4),
            detail_pid: Arc::new(AtomicU32::new(NO_DETAIL_PID)),
        }
    }

    /// Currently selected pid for per-process detail, if any.
    pub fn detail_selected(&self) -> Option<u32> {
        let v = self.detail_pid.load(Ordering::Relaxed);
        (v != NO_DETAIL_PID).then_some(v)
    }
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)] // a few events per second
pub enum Event {
    Connecting {
        attempt: u32,
    },
    Connected,
    Disconnected {
        reason: String,
        retry_in: Duration,
    },
    Metrics {
        m: Metrics,
        recv_ms: u64,
    },
    Processes {
        p: Processes,
        recv_ms: u64,
    },
    Disks {
        d: Vec<DiskInfo>,
        recv_ms: u64,
    },
    ProcessDetail {
        d: ProcessMetricsResponse,
        recv_ms: u64,
    },
    /// Phase 2 kill reply (ok, or the agent's refusal reason).
    KillResult {
        r: KillResult,
        recv_ms: u64,
    },
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Agent TCP port. The wizard assumes it when the user types a bare host.
pub const AGENT_PORT: u16 = 45976;

/// Agent token shape: 32 random bytes hex (64 hex chars) in `ebbtop.token`.
pub const AGENT_TOKEN_HEX_LEN: usize = 64;

/// Normalize what the user typed into a full `ws(s)://host:port/path` URL.
///
/// Accepts a bare hostname/IP (`k-server`), `host:port`, or a full URL.
/// Missing scheme defaults to `ws://`, missing port to [`AGENT_PORT`],
/// and an empty/`/` path to `/ws`. An explicit `wss://`, port, or path is
/// kept. Leading/trailing whitespace is ignored.
pub fn normalize_base_url(raw: &str) -> anyhow::Result<String> {
    let t = raw.trim();
    anyhow::ensure!(
        !t.is_empty(),
        "URL is empty: enter a host like 192.168.1.10 or ws://host:45976/ws"
    );
    let with_scheme = if t.contains("://") {
        t.to_string()
    } else {
        format!("ws://{t}")
    };
    let mut u =
        url::Url::parse(&with_scheme).map_err(|e| anyhow::anyhow!("bad URL {raw:?}: {e}"))?;
    match u.scheme() {
        "ws" | "wss" => {}
        s => anyhow::bail!("bad URL: scheme {s:?} must be ws:// or wss://"),
    }
    if u.host_str().unwrap_or("").is_empty() {
        anyhow::bail!("bad URL {raw:?}: missing host");
    }
    if u.port().is_none() {
        u.set_port(Some(AGENT_PORT))
            .map_err(|_| anyhow::anyhow!("bad URL {raw:?}: cannot set port"))?;
    }
    if u.path().is_empty() || u.path() == "/" {
        u.set_path("/ws");
    }
    Ok(u.as_str().to_string())
}

/// Strip surrounding whitespace (including pasted newlines) from a token.
/// Internal whitespace is dropped too: tokens are one hex word.
pub fn clean_token(raw: &str) -> String {
    raw.split_whitespace().collect()
}

/// Advisory shape check for a cleaned token. `None` means it looks like an
/// agent token; `Some(msg)` is a warning, never an error (old tokens and
/// test values still connect).
pub fn token_warning(cleaned: &str) -> Option<String> {
    if cleaned.is_empty() {
        return None;
    }
    let hex =
        cleaned.len() == AGENT_TOKEN_HEX_LEN && cleaned.bytes().all(|b| b.is_ascii_hexdigit());
    if hex {
        None
    } else {
        Some(
            "token looks unusual: the agent token is 64 hex characters from ebbtop.token"
                .to_string(),
        )
    }
}

/// Build the connect URL with the token as a query parameter (agent contract).
/// The base is normalized first (see [`normalize_base_url`]), so a bare
/// hostname works everywhere a full URL does.
pub fn connect_url(base: &str, token: Option<&str>) -> anyhow::Result<url::Url> {
    let normalized = normalize_base_url(base)?;
    let mut u = url::Url::parse(&normalized)?;
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        u.query_pairs_mut().append_pair("token", t);
    }
    Ok(u)
}

/// One-shot connection check: connect, fetch one metrics snapshot, close.
/// Used by the Settings test button and onboarding. Errors never contain
/// the token.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeOk {
    pub hostname: String,
}

pub async fn probe_once(
    base: &str,
    token: Option<&str>,
    deadline: Duration,
) -> Result<ProbeOk, String> {
    let r: anyhow::Result<ProbeOk> = async {
        let url = connect_url(base, token)?;
        let (mut ws, _) = timeout(
            deadline,
            tokio_tungstenite::connect_async_with_config(url.as_str(), Some(ws_config()), false),
        )
        .await
        .map_err(|_| anyhow::anyhow!("connect timed out"))??;
        let m = request(
            &mut ws,
            "get_metrics",
            "get_metrics",
            deadline,
            wire::decode_metrics,
        )
        .await?;
        Ok(ProbeOk {
            hostname: m.hostname,
        })
    }
    .await;
    r.map_err(|e| redact(error_text(&e), token))
}

/// User-facing probe failure: separates token rejection from unreachable
/// agents. Matches the close message `request()` emits for a kicked
/// connection. Kept for callers that only have one line to spend; the
/// structured form is [`Diagnosis`].
pub fn probe_hint(err: &str) -> String {
    if err.contains("wrong token") {
        "token rejected: check the token and try again".to_string()
    } else {
        format!("cannot reach agent: {err}")
    }
}

/// Why a one-shot probe failed, as far as the client can tell.
///
/// The agent answers a wrong token by completing the WebSocket upgrade and
/// closing the socket at once (`crates/ebbtop-agent/src/ws.rs`: the token
/// gate upgrades, then `socket.close()`), so "the endpoint took the
/// handshake and closed before replying" is how this product refuses a
/// token. That is an inference, not proof: the wire carries no
/// auth-specific close code or reason, so any other WebSocket service at
/// the same host and port can close the same way. Everything else is
/// classified from the transport error: a refused/DNS/no-route failure never
/// reached the agent, a deadline miss proves nothing about the agent, and a
/// handshake error means something other than the agent answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeFailure {
    /// The endpoint accepted the handshake and closed before answering.
    /// Possible token refusal, but the close alone cannot confirm it.
    AuthRefused,
    /// No path to the endpoint: refused, DNS failure, no route, unreachable.
    Unreachable,
    /// Nothing came back inside the deadline. A firewall drop, a VPN that is
    /// down and a hung agent all look like this, so it stays ambiguous.
    Timeout,
    /// Something answered, but not the EbbTop agent protocol (wrong port or
    /// path, a plain HTTP server, a proxy).
    NotAgent,
    /// Any other failure: the detail is all the evidence there is.
    Ambiguous,
}

impl ProbeFailure {
    /// Stable tag for log lines and tests.
    pub fn tag(self) -> &'static str {
        match self {
            Self::AuthRefused => "auth-refused",
            Self::Unreachable => "unreachable",
            Self::Timeout => "timeout",
            Self::NotAgent => "not-agent",
            Self::Ambiguous => "ambiguous",
        }
    }

    /// Short label for the UI: what happened, in the user's terms.
    pub fn headline(self) -> &'static str {
        match self {
            Self::AuthRefused => "Closed right after the handshake",
            Self::Unreachable => "No route to the agent",
            Self::Timeout => "No answer in time",
            Self::NotAgent => "Not the EbbTop agent",
            Self::Ambiguous => "Connection failed",
        }
    }

    /// One concrete next action, distinct per class so a user is never told
    /// to "check everything" for a failure that already has an answer.
    pub fn next_action(self) -> &'static str {
        match self {
            Self::AuthRefused => {
                "EbbTop infers this from one observation: the endpoint accepted the WebSocket \
                 handshake and closed at once. The EbbTop agent closes that way to refuse a \
                 token. Re-copy the token on the agent host \
                 (`ssh user@host cat ~/services/ebbtop-agent/ebbtop.token`), paste it again and \
                 Test. The close is not proof: another WebSocket service at that host and port \
                 can close the same way, so also confirm the host and port point at the EbbTop \
                 agent (default port 45976). If both are right and the token is definitely the \
                 current one, the agent may have been reinstalled with a new token: run \
                 `systemctl --user status ebbtop-agent` on the agent host."
            }
            Self::Unreachable => {
                "Nothing is reachable at that address. Check the IP or hostname, that this \
                 computer is on the same private network or VPN as the agent, the port \
                 (default 45976), and that the agent is running \
                 (`systemctl --user is-active ebbtop-agent` on the agent host)."
            }
            Self::Timeout => {
                "The address did not answer in time. A firewall rule, a VPN that is down \
                 and a stopped agent all look like this. Confirm the IP, the private \
                 network or VPN, and the port; check \
                 `systemctl --user is-active ebbtop-agent` on the agent host; then Test \
                 again."
            }
            Self::NotAgent => {
                "Something answered, but it is not the EbbTop agent. Check the port \
                 (default 45976) and the path (`/ws`), and that nothing else is listening \
                 there. A bare hostname gets `ws://`, `:45976` and `/ws` added for you."
            }
            Self::Ambiguous => {
                "The connection failed without a clear cause. Check the IP, the private \
                 network or VPN, the port, and the agent status \
                 (`systemctl --user is-active ebbtop-agent` on the agent host), then Test \
                 again."
            }
        }
    }

    /// Classify one probe error string. Order matters: a message can name
    /// more than one symptom, and the most specific evidence wins.
    pub fn classify(err: &str) -> Self {
        let e = err.to_ascii_lowercase();
        if e.contains("wrong token") {
            return Self::AuthRefused;
        }
        if e.contains("timed out") || e.contains("no reply within") || e.contains("deadline") {
            return Self::Timeout;
        }
        if e.contains("connection refused")
            || e.contains("failed to lookup address")
            || e.contains("dns error")
            || e.contains("no route to host")
            || e.contains("network is unreachable")
            || e.contains("host is unreachable")
        {
            return Self::Unreachable;
        }
        if e.contains("http error")
            || e.contains("http format")
            || e.contains("missing, duplicated or incorrect header")
            || e.contains("protocol error")
            || e.contains("not a websocket")
            || e.contains("invalid status")
            || e.contains("sec-websocket")
        {
            return Self::NotAgent;
        }
        Self::Ambiguous
    }
}

/// A classified failure plus the scrubbed detail it was derived from.
/// Everything a caller may show, log or copy comes from here, so the token
/// and any token-bearing URL are removed once, in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnosis {
    pub kind: ProbeFailure,
    /// Error chain, token-scrubbed. Never contains the token.
    pub detail: String,
}

impl Diagnosis {
    /// Classify and scrub in one step. Callers that still hold the token
    /// should pass the error through [`redact`] first (as `probe_once` does).
    pub fn from_error(err: &str) -> Self {
        let detail = scrub(err);
        Self {
            kind: ProbeFailure::classify(&detail),
            detail,
        }
    }

    pub fn headline(&self) -> &'static str {
        self.kind.headline()
    }

    pub fn next_action(&self) -> &'static str {
        self.kind.next_action()
    }

    /// Two-block user text: what happened, then what to do.
    pub fn message(&self) -> String {
        format!("{}\n{}", self.headline(), self.next_action())
    }

    /// One log line. Tag + scrubbed detail only: safe to append verbatim.
    pub fn log_line(&self) -> String {
        format!("probe {}: {}", self.kind.tag(), self.detail)
    }
}

/// Replace any `token=<value>` pair with `token=<token>`. Belt and braces
/// for messages built by libraries: it works without knowing the secret.
pub fn scrub(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("token=") {
        out.push_str(&rest[..i]);
        out.push_str("token=<token>");
        let after = &rest[i + "token=".len()..];
        let end = after
            .find(|c: char| c == '&' || c == '#' || c == '"' || c == '\'' || c.is_whitespace())
            .unwrap_or(after.len());
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// Drop the token from an error string. `probe_once` applies this before
/// returning, so probe errors are already clean.
pub fn redact(s: String, token: Option<&str>) -> String {
    match token.filter(|t| !t.is_empty()) {
        Some(t) => s.replace(t, "<token>"),
        None => s,
    }
}

/// Error chain joined with ": ", skipping links already contained in the
/// previous one (tungstenite repeats its source in its own message).
pub fn error_text(e: &anyhow::Error) -> String {
    let mut out: Vec<String> = Vec::new();
    for c in e.chain() {
        let t = c.to_string();
        if out.last().is_some_and(|prev| prev.contains(&t)) {
            continue;
        }
        out.push(t);
    }
    out.join(": ")
}

/// Next backoff: doubles from `min`, capped at `max`.
pub fn next_backoff(cur: Duration, min: Duration, max: Duration) -> Duration {
    if cur.is_zero() {
        min
    } else {
        (cur * 2).min(max)
    }
}

pub async fn run<F: FnMut(Event) + Send>(cfg: ClientConfig, emit: F) -> ! {
    run_with_kill(cfg, None, emit).await
}

/// Like [`run`], but kill jobs sent on `kill_rx` are sent on the same
/// connection between ticks. Pass `None` to keep the metrics-only loop.
pub async fn run_with_kill<F: FnMut(Event) + Send>(
    cfg: ClientConfig,
    mut kill_rx: Option<UnboundedReceiver<KillJob>>,
    mut emit: F,
) -> ! {
    let mut attempt = 0u32;
    let mut backoff = Duration::ZERO;
    loop {
        attempt += 1;
        emit(Event::Connecting { attempt });
        let mut got_data = false;
        let res = session(&cfg, &mut emit, &mut got_data, kill_rx.as_mut()).await;
        if got_data {
            backoff = Duration::ZERO;
            attempt = 0;
        }
        backoff = next_backoff(backoff, cfg.backoff_min, cfg.backoff_max);
        let reason = match res {
            Ok(()) => "closed".to_string(),
            Err(e) => redact(error_text(&e), cfg.token.as_deref()),
        };
        emit(Event::Disconnected {
            reason,
            retry_in: backoff,
        });
        tokio::time::sleep(backoff).await;
    }
}

async fn session<F: FnMut(Event)>(
    cfg: &ClientConfig,
    emit: &mut F,
    got_data: &mut bool,
    kill_rx: Option<&mut UnboundedReceiver<KillJob>>,
) -> anyhow::Result<()> {
    let url = connect_url(&cfg.url, cfg.token.as_deref())?;
    let (mut ws, _) = timeout(
        cfg.connect_timeout,
        tokio_tungstenite::connect_async_with_config(url.as_str(), Some(ws_config()), false),
    )
    .await
    .map_err(|_| anyhow::anyhow!("connect timed out"))??;
    emit(Event::Connected);

    let mut tick = tokio::time::interval(cfg.tick);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut n: u32 = 0;
    let mut kill_rx = kill_rx;
    loop {
        // Between ticks (never mid-reply): serve one queued kill first if
        // one arrived, else wait for the next tick. A job that arrives
        // while a tick is in flight stays queued for the next loop pass.
        let mut job: Option<KillJob> = None;
        match kill_rx.as_deref_mut() {
            Some(rx) => {
                tokio::select! {
                    _ = tick.tick() => {}
                    j = rx.recv() => match j {
                        Some(j) => job = Some(j),
                        None => kill_rx = None, // sender dropped: metrics only
                    },
                }
            }
            None => {
                tick.tick().await;
            }
        }
        if let Some(job) = job {
            let res = kill_roundtrip(&mut ws, &job, cfg.reply_timeout).await;
            match res {
                Ok(r) => emit(Event::KillResult {
                    r,
                    recv_ms: now_ms(),
                }),
                Err(e) => {
                    // Defined error path: report the failure, then drop the
                    // connection so frame alignment is rebuilt on reconnect.
                    let msg = redact(error_text(&e), cfg.token.as_deref());
                    emit(Event::KillResult {
                        r: KillResult {
                            ok: false,
                            pid: job.pid,
                            signal: job.signal.as_str().to_string(),
                            error: Some(msg),
                        },
                        recv_ms: now_ms(),
                    });
                    return Err(e);
                }
            }
            continue;
        }

        let m = request(
            &mut ws,
            "get_metrics",
            "get_metrics",
            cfg.reply_timeout,
            wire::decode_metrics,
        )
        .await?;
        *got_data = true;
        emit(Event::Metrics {
            m,
            recv_ms: now_ms(),
        });

        let p = request(
            &mut ws,
            "get_processes",
            "get_processes",
            cfg.reply_timeout,
            wire::decode_processes,
        )
        .await?;
        emit(Event::Processes {
            p,
            recv_ms: now_ms(),
        });

        // Per-process detail: only while a pid is selected, on the same 1 s
        // tick (no second loop). The pid is read here, after processes, so a
        // selection change is picked up within one tick. A pid that died
        // decodes to None and is skipped, not treated as a disconnect.
        if let Some(pid) = cfg.detail_selected() {
            let req = wire::encode_process_metrics_request(pid);
            if let Some(d) = request(
                &mut ws,
                &req,
                "get_process_metrics",
                cfg.reply_timeout,
                wire::decode_process_detail,
            )
            .await?
            {
                emit(Event::ProcessDetail {
                    d,
                    recv_ms: now_ms(),
                });
            }
        }

        if n.is_multiple_of(cfg.disks_every.max(1)) {
            let d = request(
                &mut ws,
                "get_disks",
                "get_disks",
                cfg.reply_timeout,
                wire::decode_disks,
            )
            .await?;
            emit(Event::Disks {
                d,
                recv_ms: now_ms(),
            });
        }
        n = n.wrapping_add(1);
    }
}

/// Send one kill request on the live connection and await its reply with
/// the usual per-request deadline. One in flight, like every other request.
async fn kill_roundtrip(
    ws: &mut Ws,
    job: &KillJob,
    deadline: Duration,
) -> anyhow::Result<KillResult> {
    let msg = wire::format_kill_request(job.pid, job.start_time_ms, job.signal);
    request(ws, &msg, "kill", deadline, wire::decode_kill).await
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn request<T>(
    ws: &mut Ws,
    send: &str,
    label: &str,
    deadline: Duration,
    decode: impl Fn(Frame<'_>) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    ws.send(Message::text(send)).await?;
    let fut = async {
        loop {
            match ws.next().await {
                None => anyhow::bail!("agent closed the connection (wrong token?)"),
                Some(Err(e)) => return Err(e.into()),
                Some(Ok(Message::Text(t))) => return decode(Frame::Text(t.as_str())),
                Some(Ok(Message::Binary(b))) => return decode(Frame::Binary(&b)),
                Some(Ok(Message::Close(_))) => {
                    anyhow::bail!("agent closed the connection (wrong token?)")
                }
                Some(Ok(_)) => continue, // ping/pong/frame
            }
        }
    };
    timeout(deadline, fut)
        .await
        .map_err(|_| anyhow::anyhow!("{label}: no reply within {} ms", deadline.as_millis()))?
        .map_err(|e| e.context(label.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_query_encoded() {
        let u = connect_url("ws://h:1/ws", Some("a b&c")).unwrap();
        assert_eq!(u.as_str(), "ws://h:1/ws?token=a+b%26c");
        assert_eq!(
            connect_url("ws://h:1/ws", None).unwrap().as_str(),
            "ws://h:1/ws"
        );
        assert!(connect_url("http://h/ws", None).is_err());
    }

    #[test]
    fn normalize_bare_host_gets_defaults() {
        assert_eq!(
            normalize_base_url("myserver").unwrap(),
            "ws://myserver:45976/ws"
        );
        assert_eq!(
            normalize_base_url("  192.168.1.10  ").unwrap(),
            "ws://192.168.1.10:45976/ws"
        );
    }

    #[test]
    fn normalize_keeps_explicit_parts() {
        assert_eq!(
            normalize_base_url("192.168.1.10:1234").unwrap(),
            "ws://192.168.1.10:1234/ws"
        );
        assert_eq!(
            normalize_base_url("wss://h:1/custom").unwrap(),
            "wss://h:1/custom"
        );
        assert_eq!(normalize_base_url("ws://h/ws").unwrap(), "ws://h:45976/ws");
        assert!(normalize_base_url("http://h/ws").is_err());
        assert!(normalize_base_url("   ").is_err());
    }

    #[test]
    fn connect_url_accepts_bare_host() {
        let u = connect_url("myserver", Some("t")).unwrap();
        assert_eq!(u.as_str(), "ws://myserver:45976/ws?token=t");
    }

    #[test]
    fn token_clean_and_warning() {
        assert_eq!(clean_token("  abc\n"), "abc");
        assert_eq!(clean_token("a b c"), "abc");
        let good = "a".repeat(64);
        assert!(token_warning(&good).is_none());
        assert!(token_warning("short").is_some());
        assert!(token_warning("").is_none());
    }

    #[test]
    fn redact_hides_token() {
        assert_eq!(
            redact("bad ws://h/?token=s3cr3t".into(), Some("s3cr3t")),
            "bad ws://h/?token=<token>"
        );
    }

    #[test]
    fn backoff_doubles_and_caps() {
        let (a, b) = (Duration::from_millis(500), Duration::from_secs(10));
        let mut d = Duration::ZERO;
        let seq: Vec<u128> = (0..7)
            .map(|_| {
                d = next_backoff(d, a, b);
                d.as_millis()
            })
            .collect();
        assert_eq!(seq, vec![500, 1000, 2000, 4000, 8000, 10000, 10000]);
        assert_eq!(
            ClientConfig::new("ws://h/ws", None).backoff_max,
            Duration::from_secs(4)
        );
    }

    #[test]
    fn error_text_drops_repeated_source() {
        let io = std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "Connection refused (os error 111)",
        );
        let e = anyhow::Error::new(io)
            .context("IO error: Connection refused (os error 111)")
            .context("get_metrics");
        assert_eq!(
            error_text(&e),
            "get_metrics: IO error: Connection refused (os error 111)"
        );
    }

    #[tokio::test]
    async fn probe_refused_connection_is_an_error_without_token() {
        let r = probe_once(
            "ws://127.0.0.1:1/ws",
            Some("s3cr3t"),
            Duration::from_secs(2),
        )
        .await;
        let e = r.expect_err("closed port must fail");
        assert!(!e.contains("s3cr3t"), "probe error leaked token: {e}");
    }

    #[test]
    fn probe_hint_names_rejection() {
        let h = probe_hint("kill: agent closed the connection (wrong token?)");
        assert!(h.contains("token rejected"), "unhelpful: {h}");
    }

    #[test]
    fn probe_hint_keeps_unreachable_detail() {
        let h = probe_hint("connect timed out");
        assert!(h.contains("cannot reach"), "unhelpful: {h}");
        assert!(h.contains("connect timed out"), "detail lost: {h}");
    }

    #[tokio::test]
    async fn probe_returns_hostname() {
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream).await.expect("ws");
            let msg = ws.next().await.expect("request").expect("msg");
            assert_eq!(msg.to_text().expect("text"), "get_metrics");
            let body = r#"{"sampled_at_ms":1700000000000,"cpu_total":1.0,"cpu_per_core":[1.0],"mem_total":8,"mem_used":4,"swap_total":0,"swap_used":0,"hostname":"fakehost"}"#;
            ws.send(Message::text(body)).await.expect("reply");
        });
        let r = probe_once(
            &format!("ws://{addr}/ws"),
            Some("s3cr3t"),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(r.expect("probe").hostname, "fakehost");
    }

    // ---- E4: actionable pairing diagnostics -------------------------------
    //
    // Every fixture server below is local (127.0.0.1, ephemeral port): no
    // test touches a real agent or the network path a user would use.

    /// Dummy secret used as the probe token. It must never come back out of
    /// a probe, a diagnosis, a log line or a support summary.
    const SENTINEL: &str = "DUMMYSENTINEL0123456789abcdef0123456789abcdef0123456789abcdef0123";

    #[tokio::test]
    async fn probe_classifies_agent_style_close_as_auth_refusal() {
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream).await.expect("ws");
            // The agent reads the request and then closes on a bad token.
            let _ = ws.next().await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = ws.close(None).await;
        });
        let e = probe_once(
            &format!("ws://{addr}/ws"),
            Some(SENTINEL),
            Duration::from_secs(3),
        )
        .await
        .expect_err("an agent-style close must fail the probe");
        let d = Diagnosis::from_error(&e);
        assert_eq!(d.kind, ProbeFailure::AuthRefused, "detail: {e}");
        // F1: the headline describes the observed close, not a confirmed
        // refusal — this wire carries no auth-specific close code.
        assert_eq!(d.headline(), "Closed right after the handshake");
        assert_close_guidance_is_uncertain(d.next_action());
        assert!(!e.contains(SENTINEL), "probe error leaked the token: {e}");
        assert!(
            !d.log_line().contains(SENTINEL),
            "log line leaked the token"
        );
        assert!(!d.message().contains(SENTINEL), "message leaked the token");
    }

    /// F1 regression control: a non-agent WebSocket peer that closes with an
    /// explicit benign code and reason produces the byte-identical error and
    /// the same classification, because the unchanged wire does not carry (or
    /// keep) an auth-specific close signal. The guidance therefore has to
    /// name that possibility instead of asserting a refusal.
    fn assert_close_guidance_is_uncertain(action: &str) {
        for need in [
            "infers",
            "handshake",
            "Re-copy the token",
            "another WebSocket service",
            "host and port point at the EbbTop agent",
            "systemctl --user status ebbtop-agent",
        ] {
            assert!(
                action.contains(need),
                "close guidance misses {need:?}: {action}"
            );
        }
        assert!(
            !action.contains("private network"),
            "a close after the handshake must not be answered with network checks: {action}"
        );
    }

    /// Reviewer's non-agent close fixture (CloseCode::Normal, "service
    /// shutting down"). Run here as a regression case: the candidate must not
    /// claim to have confirmed a token refusal from it.
    #[tokio::test]
    async fn non_agent_normal_close_is_indistinguishable_and_guidance_says_so() {
        use tokio::net::TcpListener;
        use tokio_tungstenite::tungstenite::protocol::CloseFrame;
        use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream).await.expect("ws");
            let _ = ws.next().await;
            let _ = ws
                .close(Some(CloseFrame {
                    code: CloseCode::Normal,
                    reason: "service shutting down".into(),
                }))
                .await;
        });
        let e = probe_once(
            &format!("ws://{addr}/ws"),
            Some(SENTINEL),
            Duration::from_secs(3),
        )
        .await
        .expect_err("a benign non-agent close must fail the probe");
        assert_eq!(
            e, "get_metrics: agent closed the connection (wrong token?)",
            "the wire no longer hides the non-agent close; revisit the guidance"
        );
        let d = Diagnosis::from_error(&e);
        assert_eq!(d.kind, ProbeFailure::AuthRefused);
        assert_close_guidance_is_uncertain(d.next_action());
        assert!(!d.message().contains(SENTINEL));
    }

    /// Reviewer's second non-agent close fixture (CloseCode::Policy, "go
    /// away"): the reason is discarded by the unchanged client, so this is
    /// the same case as far as a user can tell.
    #[tokio::test]
    async fn non_agent_policy_close_is_indistinguishable_and_guidance_says_so() {
        use tokio::net::TcpListener;
        use tokio_tungstenite::tungstenite::protocol::CloseFrame;
        use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream).await.expect("ws");
            let _ = ws.next().await;
            let _ = ws
                .close(Some(CloseFrame {
                    code: CloseCode::Policy,
                    reason: "go away".into(),
                }))
                .await;
        });
        let e = probe_once(
            &format!("ws://{addr}/ws"),
            Some(SENTINEL),
            Duration::from_secs(3),
        )
        .await
        .expect_err("a policy close must fail the probe");
        assert_eq!(e, "get_metrics: agent closed the connection (wrong token?)");
        let d = Diagnosis::from_error(&e);
        assert_eq!(d.kind, ProbeFailure::AuthRefused);
        assert_close_guidance_is_uncertain(d.next_action());
        assert!(!d.message().contains(SENTINEL));
    }

    #[tokio::test]
    async fn probe_classifies_silent_listener_as_timeout() {
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            // Accept, then never answer: a firewall drop or a hung agent.
            let (_stream, _) = listener.accept().await.expect("accept");
            tokio::time::sleep(Duration::from_secs(2)).await;
        });
        let e = probe_once(
            &format!("ws://{addr}/ws"),
            Some(SENTINEL),
            Duration::from_millis(300),
        )
        .await
        .expect_err("a silent listener must time out");
        let d = Diagnosis::from_error(&e);
        assert_eq!(d.kind, ProbeFailure::Timeout, "detail: {e}");
        assert!(!e.contains(SENTINEL), "probe error leaked the token: {e}");
        // Ambiguous by construction: the action must name the checks that
        // separate IP, private network, port and a stopped agent.
        for word in ["IP", "network", "port", "agent"] {
            assert!(
                d.next_action().contains(word),
                "guidance misses {word}: {}",
                d.next_action()
            );
        }
    }

    #[tokio::test]
    async fn probe_classifies_refused_port_as_unreachable() {
        let e = probe_once(
            "ws://127.0.0.1:1/ws",
            Some(SENTINEL),
            Duration::from_secs(2),
        )
        .await
        .expect_err("closed port must fail");
        let d = Diagnosis::from_error(&e);
        assert_eq!(d.kind, ProbeFailure::Unreachable, "detail: {e}");
        assert!(
            d.next_action().contains("private network"),
            "not actionable: {}",
            d.next_action()
        );
        assert!(!e.contains(SENTINEL), "probe error leaked the token: {e}");
    }

    #[tokio::test]
    async fn probe_classifies_plain_http_answer_as_not_agent() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf).await;
            let _ = s
                .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n")
                .await;
        });
        let e = probe_once(
            &format!("ws://{addr}/ws"),
            Some(SENTINEL),
            Duration::from_secs(3),
        )
        .await
        .expect_err("a plain HTTP answer must fail the probe");
        let d = Diagnosis::from_error(&e);
        assert_eq!(d.kind, ProbeFailure::NotAgent, "detail: {e}");
        assert!(
            d.next_action().contains("port"),
            "not actionable: {}",
            d.next_action()
        );
    }

    #[test]
    fn every_class_has_its_own_headline_and_action() {
        let all = [
            ProbeFailure::AuthRefused,
            ProbeFailure::Unreachable,
            ProbeFailure::Timeout,
            ProbeFailure::NotAgent,
            ProbeFailure::Ambiguous,
        ];
        for (i, a) in all.iter().enumerate() {
            assert!(!a.headline().is_empty() && !a.next_action().is_empty());
            for b in &all[i + 1..] {
                assert_ne!(a.headline(), b.headline(), "classes share a headline");
                assert_ne!(a.next_action(), b.next_action(), "classes share an action");
                assert_ne!(a.tag(), b.tag(), "classes share a log tag");
            }
        }
        // The one distinction the card names explicitly.
        assert!(
            !ProbeFailure::AuthRefused
                .next_action()
                .contains("private network"),
            "an auth refusal must not be answered with network checks"
        );
        assert!(
            !ProbeFailure::Unreachable
                .next_action()
                .contains("Re-copy the token"),
            "a failed network path must not be answered with the token"
        );
    }

    #[test]
    fn classification_reads_the_most_specific_evidence() {
        assert_eq!(
            ProbeFailure::classify("get_metrics: agent closed the connection (wrong token?)"),
            ProbeFailure::AuthRefused
        );
        assert_eq!(
            ProbeFailure::classify("get_metrics: no reply within 5000 ms"),
            ProbeFailure::Timeout
        );
        assert_eq!(
            ProbeFailure::classify("connect timed out"),
            ProbeFailure::Timeout
        );
        assert_eq!(
            ProbeFailure::classify("IO error: Connection refused (os error 111)"),
            ProbeFailure::Unreachable
        );
        assert_eq!(
            ProbeFailure::classify(
                "failed to lookup address information: Name or service not known"
            ),
            ProbeFailure::Unreachable
        );
        assert_eq!(
            ProbeFailure::classify("HTTP error: 404 Not Found"),
            ProbeFailure::NotAgent
        );
        assert_eq!(
            ProbeFailure::classify(
                "Protocol error: Missing, duplicated or incorrect header sec-websocket-accept"
            ),
            ProbeFailure::NotAgent
        );
        assert_eq!(
            ProbeFailure::classify("something else entirely"),
            ProbeFailure::Ambiguous
        );
    }

    #[test]
    fn scrub_removes_a_token_bearing_url_without_knowing_the_secret() {
        assert_eq!(
            scrub("bad ws://h:1/ws?token=abc"),
            "bad ws://h:1/ws?token=<token>"
        );
        assert_eq!(scrub("x?token=abc&b=1"), "x?token=<token>&b=1");
        assert_eq!(scrub("no query here"), "no query here");
        let d = Diagnosis::from_error(&format!("bad ws://h:1/ws?token={SENTINEL}"));
        assert!(
            !d.detail.contains(SENTINEL),
            "scrub missed the token: {}",
            d.detail
        );
        assert!(!d.log_line().contains(SENTINEL));
        assert!(!d.message().contains(SENTINEL));
    }

    #[test]
    fn diagnosis_detail_is_scrubbed_even_when_the_upstream_error_is_not() {
        // Defence in depth: a library that echoes the URL must not leak it.
        let d = Diagnosis::from_error(&format!("connect failed ws://h:1/ws?token={SENTINEL}"));
        assert!(!d.detail.contains(SENTINEL), "detail: {}", d.detail);
        assert!(
            d.detail.contains("<token>"),
            "detail lost the marker: {}",
            d.detail
        );
    }
}
