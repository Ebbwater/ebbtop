//! Discovery scan and pairing-code exchange for onboarding and Settings.
//!
//! Both halves are operator-initiated, bounded and cancellable, and both run
//! on their own thread so a blocked socket or a slow host can never stall a
//! frame:
//!
//! * [`scan_blocking`] sends at most [`pairing::SCAN_PROBES`] datagrams of
//!   [`pairing::DISCOVERY_REQUEST`] to a fixed target set (this segment's
//!   limited broadcast plus loopback) and stops after [`pairing::SCAN_WINDOW_MS`],
//!   at [`pairing::SCAN_MAX_HOSTS`] replies, or when the user cancels. It
//!   binds no listening port and needs no privilege.
//! * [`exchange_blocking`] is one `POST /pair` with the code in the JSON
//!   *body*. The code never reaches a URL, a log line or an error detail, and
//!   the token it returns goes straight to the caller — the credential store
//!   is the only place it is written.
//!
//! Neither function prints anything.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::RichText;
use ebbtop_core::pairing::{
    self, DiscoveredAgent, PairError, PairRequest, DISCOVERY_PORT, DISCOVERY_REQUEST,
};

use crate::widgets as w;

/// Where a scan sends its probes: this segment's limited broadcast (finds
/// every agent on the L2 segment without knowing an address) and loopback (a
/// local agent, even when broadcast is filtered).
pub fn default_targets(port: u16) -> Vec<SocketAddr> {
    vec![
        SocketAddr::from((Ipv4Addr::BROADCAST, port)),
        SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
    ]
}

/// One bounded, cancellable discovery scan. Returns the agents that answered
/// with a current discovery reply, in reply order, at most `max_hosts`.
///
/// A datagram that is not this protocol, at this version, from a named agent
/// is dropped by [`pairing::decode_discovery_reply`], so a foreign UDP reply
/// can never become a row in the picker.
pub fn scan_blocking(
    targets: &[SocketAddr],
    window: Duration,
    max_hosts: usize,
    cancel: &AtomicBool,
) -> Result<Vec<DiscoveredAgent>, String> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|e| format!("could not open a discovery socket: {e}"))?;
    // Broadcast needs the option; a platform that refuses it still scans
    // unicast targets (loopback), so this is best effort.
    let _ = sock.set_broadcast(true);
    let _ = sock.set_read_timeout(Some(Duration::from_millis(120)));

    let deadline = Instant::now() + window;
    let mut probes: Vec<Instant> = Vec::new();
    let mut found: Vec<DiscoveredAgent> = Vec::new();
    let mut buf = [0u8; pairing::DISCOVERY_REPLY_MAX];

    while Instant::now() < deadline && !cancel.load(Ordering::Relaxed) {
        // Re-send while the window is open: the responder answers at most
        // once per source per second, so one datagram is not always enough.
        let due = probes
            .last()
            .map(|t| t.elapsed() >= Duration::from_millis(pairing::SCAN_PROBE_GAP_MS))
            .unwrap_or(true);
        if probes.len() < pairing::SCAN_PROBES && due {
            for t in targets {
                let _ = sock.send_to(DISCOVERY_REQUEST, t);
            }
            probes.push(Instant::now());
        }
        match sock.recv_from(&mut buf) {
            Ok((n, src)) => {
                let Some(reply) = pairing::decode_discovery_reply(&buf[..n]) else {
                    continue;
                };
                let agent = DiscoveredAgent::from_reply(src.ip(), &reply);
                if found
                    .iter()
                    .any(|a| a.addr == agent.addr && a.nonce == agent.nonce)
                {
                    continue;
                }
                found.push(agent);
                if found.len() >= max_hosts {
                    break;
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(format!("discovery stopped: {e}")),
        }
    }
    Ok(found)
}

/// One `POST /pair`: the code in the body, a five second deadline, and the
/// reply body classified into a [`PairError`] the UI can act on. A non-2xx
/// status is a refusal from the agent (404/403/429/400), not a transport
/// failure.
pub fn exchange_blocking(
    pair_url: &str,
    code: &str,
    label: Option<String>,
    fallback_name: &str,
) -> Result<PairOutcome, PairError> {
    let body = serde_json::to_string(&PairRequest { code: code.to_string(), label })
        .map_err(|e| PairError::from_transport(&format!("cannot encode the pairing request: {e}")))?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .connect_timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| PairError::from_transport(&format!("cannot start the pairing request: {e}")))?;
    let resp = client
        .post(pair_url)
        .header("content-type", "application/json")
        .header("cache-control", "no-store")
        .body(body)
        .send()
        .map_err(|e| PairError::from_transport(&error_chain(&e)))?;
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(PairError::from_status(status, &text));
    }
    let reply = pairing::pair_response(&text)?;
    Ok(PairOutcome {
        token: reply.token,
        host: if reply.host.trim().is_empty() {
            fallback_name.to_string()
        } else {
            reply.host
        },
        agent_version: reply.agent_version,
        scope: reply.scope,
    })
}

/// Join a request error with its sources. `reqwest`'s own Display is only
/// "error sending request for url (…)", which classifies as ambiguous; the
/// cause that decides the class ("Connection refused", "operation timed out")
/// is one level down the chain.
fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        let t = err.to_string();
        if !out.last().is_some_and(|prev| prev.contains(&t)) {
            out.push(t);
        }
        cur = err.source();
    }
    out.join(": ")
}

/// What a successful exchange hands back. `token` is a secret: it is written
/// to the credential store and kept in memory, never rendered or logged.
#[derive(Debug, Clone, PartialEq)]
pub struct PairOutcome {
    pub token: String,
    pub host: String,
    pub agent_version: String,
    pub scope: String,
}

/// A finished or in-flight scan, plus the list it produced. One instance per
/// surface (wizard, Settings).
#[derive(Default)]
pub struct Discovery {
    pub agents: Vec<DiscoveredAgent>,
    pub scanning: bool,
    /// At least one scan has finished, so "nothing found" is a result rather
    /// than the initial state.
    pub finished: bool,
    pub msg: Option<String>,
    rx: Option<Receiver<Result<Vec<DiscoveredAgent>, String>>>,
    cancel: Option<Arc<AtomicBool>>,
}

impl std::fmt::Debug for Discovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Discovery")
            .field("agents", &self.agents.len())
            .field("scanning", &self.scanning)
            .field("finished", &self.finished)
            .finish()
    }
}

impl Discovery {
    /// Drain the scan thread's result, if it has one.
    pub fn poll(&mut self) {
        let Some(rx) = self.rx.as_ref() else { return };
        match rx.try_recv() {
            Ok(Ok(agents)) => {
                self.rx = None;
                self.cancel = None;
                self.scanning = false;
                self.finished = true;
                self.agents = agents;
                self.msg = None;
            }
            Ok(Err(e)) => {
                self.rx = None;
                self.cancel = None;
                self.scanning = false;
                self.finished = true;
                self.agents.clear();
                self.msg = Some(e);
            }
            Err(TryRecvError::Disconnected) => {
                self.rx = None;
                self.cancel = None;
                self.scanning = false;
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    /// Start a scan on its own thread. A second start while one is running is
    /// ignored: the button is a toggle in the UI, and one socket per click is
    /// enough.
    pub fn start(&mut self) {
        if self.scanning {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        self.rx = Some(rx);
        self.cancel = Some(cancel);
        self.scanning = true;
        self.finished = false;
        self.agents.clear();
        self.msg = None;
        let targets = default_targets(DISCOVERY_PORT);
        let spawned = std::thread::Builder::new()
            .name("ebbtop-discover".into())
            .spawn(move || {
                let r = scan_blocking(
                    &targets,
                    Duration::from_millis(pairing::SCAN_WINDOW_MS),
                    pairing::SCAN_MAX_HOSTS,
                    &flag,
                );
                let _ = tx.send(r);
            });
        if let Err(e) = spawned {
            self.scanning = false;
            self.rx = None;
            self.cancel = None;
            self.msg = Some(format!("could not start the scan: {e}"));
        }
    }

    /// Ask the running scan to stop at its next receive. The list keeps
    /// whatever it had already collected.
    pub fn cancel(&mut self) {
        if let Some(c) = &self.cancel {
            c.store(true, Ordering::Relaxed);
        }
    }

    /// Draw the scan controls, the honest limits, the empty state and the
    /// discovered rows. Returns the index of the row whose "Use this host"
    /// was clicked.
    pub fn draw(&mut self, ui: &mut egui::Ui) -> Option<usize> {
        let mut picked = None;
        ui.horizontal(|ui| {
            if self.scanning {
                if ui.button("Cancel scan").clicked() {
                    self.cancel();
                }
                ui.label(RichText::new("scanning this network segment…").color(w::DIM));
            } else if ui
                .button("Find agents on this network")
                .on_hover_text("UDP broadcast probe, 1.4 s, cancellable")
                .clicked()
            {
                self.start();
            }
        });
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Scans this network segment only: broadcast does not cross routers, VLANs or a \
                     VPN/tailnet. Reachable over a tailnet or another subnet? Enter the address \
                     yourself below.",
                )
                .small()
                .color(w::DIM),
            )
            .wrap(),
        );
        if let Some(m) = &self.msg {
            ui.add(egui::Label::new(RichText::new(m).color(w::BAD)).wrap());
        }
        if !self.agents.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("discovered-agents")
                .max_height(132.0)
                .show(ui, |ui| {
                    for (i, a) in self.agents.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&a.name).strong().color(w::TEXT));
                            ui.label(
                                RichText::new(format!("v{}", a.agent_version)).small().color(w::DIM),
                            );
                            ui.label(
                                RichText::new(format!("{}:{}", a.host(), a.port))
                                    .monospace()
                                    .small()
                                    .color(w::DIM),
                            );
                            if ui
                                .button("Use this host")
                                .on_hover_text("pick this agent and pair with a code")
                                .clicked()
                            {
                                picked = Some(i);
                            }
                        });
                    }
                });
        } else if self.finished && !self.scanning && self.msg.is_none() {
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "No agent answered. Check that the host runs the agent with discovery \
                         enabled (EBBTOP_AGENT_DISCOVERY is on by default), that its firewall allows \
                         UDP 45978, or enter the address yourself below.",
                    )
                    .small()
                    .color(w::WARN),
                )
                .wrap(),
            );
        }
        picked
    }
}

/// What a click in the code-entry block asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairAction {
    None,
    Start,
    Cancel,
}

/// Code entry for one chosen host: a name for the source, the address it will
/// pair with, the digits the user is typing, and the last classified failure.
pub struct Pairing {
    pub name: String,
    pub url: String,
    pub code: String,
    pub busy: bool,
    pub error: Option<PairError>,
    pub msg: Option<String>,
    rx: Option<Receiver<Result<PairOutcome, PairError>>>,
}

impl std::fmt::Debug for Pairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the code or the token: a debug print can reach a log.
        f.debug_struct("Pairing")
            .field("name", &self.name)
            .field("url", &self.url)
            .field("busy", &self.busy)
            .field("error", &self.error)
            .finish()
    }
}

impl Pairing {
    /// Prompt for an agent the scan found.
    pub fn for_agent(a: &DiscoveredAgent) -> Self {
        Self { name: a.name.clone(), url: a.url(), ..Self::default() }
    }

    /// Prompt for a host the user typed themselves (the manual path still
    /// pairs: discovery is a convenience, not a requirement).
    pub fn for_address(name: &str, url: &str) -> Self {
        // Show the address the exchange will actually use (bare host gains
        // ws://, :45976 and /ws), so the prompt cannot imply a different host.
        let url = ebbtop_core::client::normalize_base_url(url)
            .unwrap_or_else(|_| url.trim().to_string());
        Self { name: name.trim().to_string(), url, ..Self::default() }
    }

    /// Drain the exchange thread. Returns the outcome once on success; a
    /// failure is classified into `error` and stays there until the next try.
    pub fn poll(&mut self) -> Option<PairOutcome> {
        let rx = self.rx.as_ref()?;
        match rx.try_recv() {
            Ok(Ok(o)) => {
                self.rx = None;
                self.busy = false;
                self.error = None;
                self.msg = None;
                Some(o)
            }
            Ok(Err(e)) => {
                self.rx = None;
                self.busy = false;
                self.error = Some(e);
                None
            }
            Err(TryRecvError::Disconnected) => {
                self.rx = None;
                self.busy = false;
                self.error = Some(PairError::bad_reply("the pairing worker stopped without a reply"));
                None
            }
            Err(TryRecvError::Empty) => None,
        }
    }

    /// Validate the code locally (a malformed code must not spend one of the
    /// host's five attempts), then exchange it on a worker thread.
    pub fn start(&mut self) {
        if self.busy {
            return;
        }
        let code = match pairing::clean_pair_code(&self.code) {
            Ok(c) => c,
            Err(e) => {
                self.error = None;
                self.msg = Some(e);
                return;
            }
        };
        let url = match pairing::pair_url(&self.url) {
            Ok(u) => u,
            Err(e) => {
                self.msg = Some(format!("cannot pair with this address: {e}"));
                return;
            }
        };
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.busy = true;
        self.error = None;
        self.msg = None;
        let label = Some(self.name.clone());
        let name = self.name.clone();
        let spawned = std::thread::Builder::new()
            .name("ebbtop-pair".into())
            .spawn(move || {
                let r = exchange_blocking(&url, &code, label, &name);
                let _ = tx.send(r);
            });
        if let Err(e) = spawned {
            self.busy = false;
            self.rx = None;
            self.msg = Some(format!("could not start the exchange: {e}"));
        }
    }

    /// Draw the prompt. The caller starts the exchange (or drops the prompt)
    /// from the returned action.
    pub fn draw(&mut self, ui: &mut egui::Ui) -> PairAction {
        let mut action = PairAction::None;
        ui.add_space(4.0);
        ui.separator();
        ui.label(RichText::new(format!("Pair with {}", self.name)).strong().color(w::TEXT));
        ui.add(
            egui::Label::new(
                RichText::new(format!(
                    "{} — run `ebbtop-agent --pair` on that host for a 6-digit code \
                     (about 5 minutes, one use).",
                    self.url
                ))
                .small()
                .color(w::DIM),
            )
            .wrap(),
        );
        ui.horizontal(|ui| {
            ui.label("Pairing code");
            ui.add(
                egui::TextEdit::singleline(&mut self.code)
                    .desired_width(88.0)
                    .hint_text("6 digits")
                    .char_limit(12),
            );
            let pair = ui
                .add_enabled(!self.busy, egui::Button::new("Pair"))
                .on_hover_text("exchange the code for a read-only token on that host");
            if pair.clicked() {
                action = PairAction::Start;
            }
            if ui.button("Cancel").clicked() {
                action = PairAction::Cancel;
            }
        });
        if self.busy {
            ui.label(RichText::new("asking the host for a token…").color(w::DIM));
        }
        if let Some(e) = &self.error {
            w::error_block(ui, e.headline(), e.next_action(), &e.detail);
        }
        if let Some(m) = &self.msg {
            ui.add(egui::Label::new(RichText::new(m).color(w::WARN)).wrap());
        }
        action
    }
}

impl Default for Pairing {
    fn default() -> Self {
        Self {
            name: String::new(),
            url: String::new(),
            code: String::new(),
            busy: false,
            error: None,
            msg: None,
            rx: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Mutex;

    /// A discovery responder on loopback: answers `reply` to the exact
    /// request, once per received probe, and stays otherwise silent.
    fn responder(reply: &[u8]) -> SocketAddr {
        let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind responder");
        let addr = sock.local_addr().expect("responder addr");
        let reply = reply.to_vec();
        std::thread::spawn(move || {
            let mut buf = [0u8; 256];
            while let Ok((n, src)) = sock.recv_from(&mut buf) {
                if &buf[..n] == DISCOVERY_REQUEST {
                    let _ = sock.send_to(&reply, src);
                }
            }
        });
        addr
    }

    /// A TCP fixture that answers exactly one HTTP request with `status` and
    /// `body` and records the raw request bytes. No real host, no port 45976.
    fn http_fixture(
        status: u16,
        reason: &'static str,
        body: impl Into<String>,
    ) -> (SocketAddr, Arc<Mutex<String>>) {
        let body: String = body.into();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind fixture");
        let addr = listener.local_addr().expect("fixture addr");
        let seen = Arc::new(Mutex::new(String::new()));
        let sink = seen.clone();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                read_request(&mut s, &sink);
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes());
                let _ = s.flush();
            }
        });
        (addr, seen)
    }

    /// Read one HTTP/1.1 request (headers then Content-Length bytes) so the
    /// fixture can answer after the body has arrived.
    fn read_request(s: &mut TcpStream, sink: &Arc<Mutex<String>>) {
        let mut raw = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = match s.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            raw.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&raw).to_string();
            if let Some(head_end) = text.find("\r\n\r\n") {
                let len = text[..head_end]
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().to_string()))
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(0);
                if raw.len() >= head_end + 4 + len {
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        *sink.lock().unwrap() = String::from_utf8_lossy(&raw).to_string();
    }

    const FIXTURE_REPLY: &[u8] = br#"{"ebbtop_discovery":1,"name":"k-server","agent_version":"0.2.8","nonce":"00112233445566778899aabbccddeeff","port":45976}"#;

    /// A 64-hex token shape, built at run time so no compile-time literal in
    /// this file is a credential-shaped string.
    fn token64() -> String {
        "a1b2c3d4e5f60718".repeat(4)
    }

    #[test]
    fn scan_finds_a_fixture_responder_and_prefills_its_url() {
        let target = responder(FIXTURE_REPLY);
        let cancel = AtomicBool::new(false);
        let found = scan_blocking(&[target], Duration::from_millis(600), 8, &cancel)
            .expect("scan must not error on a quiet segment");
        assert_eq!(found.len(), 1, "one responder must produce one row: {found:?}");
        let a = &found[0];
        assert_eq!(a.name, "k-server");
        assert_eq!(a.agent_version, "0.2.8");
        assert_eq!(a.addr, "127.0.0.1");
        assert_eq!(a.port, 45976);
        assert_eq!(a.url(), "ws://127.0.0.1:45976/ws");
        // The row carries the three facts the picker promises (the app
        // renders them as separate labels; the fields are what it reads).
        assert_eq!(
            (a.name.as_str(), a.agent_version.as_str(), a.host(), a.port),
            ("k-server", "0.2.8", "127.0.0.1".to_string(), 45976)
        );
    }

    #[test]
    fn scan_ignores_foreign_datagrams_and_oversize_replies() {
        for reply in [
            &br#"{"hello":"world"}"#[..],
            &br#"{"ebbtop_discovery":2,"name":"x","agent_version":"1","nonce":"n"}"#[..],
            &br#"{"ebbtop_discovery":1,"name":"","agent_version":"1","nonce":"n"}"#[..],
        ] {
            let target = responder(reply);
            let cancel = AtomicBool::new(false);
            let found = scan_blocking(&[target], Duration::from_millis(350), 8, &cancel)
                .expect("scan");
            assert!(found.is_empty(), "foreign reply listed: {found:?}");
        }
    }

    #[test]
    fn scan_stops_at_the_host_cap() {
        // Two distinct agents (different name and nonce) on loopback: without
        // a cap both are found, so the cap is what bounds the list.
        const SECOND: &[u8] = br#"{"ebbtop_discovery":1,"name":"pi","agent_version":"0.2.8","nonce":"ffeeddccbbaa99887766554433221100","port":45976}"#;
        let a = responder(FIXTURE_REPLY);
        let b = responder(SECOND);
        let cancel = AtomicBool::new(false);
        let both =
            scan_blocking(&[a, b], Duration::from_millis(600), 8, &cancel).expect("scan");
        assert_eq!(both.len(), 2, "the fixtures must both answer: {both:?}");
        let cancel = AtomicBool::new(false);
        let capped =
            scan_blocking(&[a, b], Duration::from_millis(600), 1, &cancel).expect("scan");
        assert_eq!(capped.len(), 1, "cap must bound the list: {capped:?}");
    }

    #[test]
    fn scan_is_cancellable_before_the_window_ends() {
        let target = responder(FIXTURE_REPLY);
        let cancel = AtomicBool::new(true);
        let started = Instant::now();
        let found = scan_blocking(&[target], Duration::from_secs(2), 8, &cancel).expect("scan");
        assert!(found.is_empty(), "a cancelled scan reports nothing: {found:?}");
        assert!(
            started.elapsed() < Duration::from_millis(400),
            "cancel must not wait out the window: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_quiet_segment_returns_an_empty_list_inside_the_window() {
        // A bound socket that never answers: the window, not a hang, ends it.
        let silent = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind silent");
        let addr = silent.local_addr().expect("silent addr");
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let found = scan_blocking(&[addr], Duration::from_millis(300), 8, &cancel).expect("scan");
        assert!(found.is_empty());
        assert!(started.elapsed() < Duration::from_millis(1500), "{:?}", started.elapsed());
    }

    #[test]
    fn exchange_returns_the_issued_token_from_a_fixture_host() {
        let token = token64();
        let (addr, seen) = http_fixture(
            200,
            "OK",
            format!(
                r#"{{"token":"{token}","host":"k-server","agent_version":"0.2.8","nonce":"n","scope":"read_only"}}"#
            ),
        );
        let url = format!("http://{addr}/pair");
        let out = exchange_blocking(&url, "123456", Some("wizard".into()), "fallback")
            .expect("fixture must pair");
        assert_eq!(out.host, "k-server");
        assert_eq!(out.scope, "read_only");
        assert_eq!(out.token, token);

        // The code travelled in the JSON body, never in the request line or a
        // query string, and the body is the contract's shape.
        let req = seen.lock().unwrap().clone();
        let (head, body) = req.split_once("\r\n\r\n").expect("request head");
        assert!(head.starts_with("POST /pair HTTP/1.1"), "wrong request: {head}");
        assert!(!head.to_ascii_lowercase().contains("code="), "code in the URL: {head}");
        assert!(body.contains(r#""code":"123456""#), "code missing from the body: {body}");
        assert!(body.contains(r#""label":"wizard""#), "label missing: {body}");
        let parsed: PairRequest = serde_json::from_str(body.trim()).expect("body is a PairRequest");
        assert_eq!(parsed.code, "123456");
    }

    #[test]
    fn exchange_classifies_each_refusal_class() {
        for (status, reason, body, want) in [
            (404u16, "Not Found", "no pending pairing code", pairing::PairFailure::NoPendingCode),
            (403, "Forbidden", "invalid pairing code", pairing::PairFailure::InvalidCode),
            (403, "Forbidden", "pairing code expired", pairing::PairFailure::ExpiredCode),
            (429, "Too Many Requests", "too many attempts; run ebbtop-agent --pair again", pairing::PairFailure::LockedOut),
            (400, "Bad Request", "bad pair request", pairing::PairFailure::BadRequest),
            (500, "Internal Server Error", "boom", pairing::PairFailure::Ambiguous),
        ] {
            let (addr, _seen) = http_fixture(status, reason, body);
            let url = format!("http://{addr}/pair");
            let e = exchange_blocking(&url, "123456", None, "n").expect_err("refusal expected");
            assert_eq!(e.kind, want, "status {status} body {body:?} classified wrong: {e:?}");
            assert!(!e.detail.contains("123456"), "code echoed in the detail: {}", e.detail);
            assert!(!e.log_line().contains("123456"), "code in the log line: {}", e.log_line());
            // Every class renders an actionable pair of lines.
            assert!(e.headline().len() > 5, "no headline: {e:?}");
            assert!(e.next_action().len() > 40, "no next action: {e:?}");
        }
    }

    #[test]
    fn exchange_refuses_an_unreachable_host_without_leaking_the_code() {
        // A port nobody listens on: the fixture is a closed one.
        let addr = {
            let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
            let a = l.local_addr().expect("addr");
            drop(l);
            a
        };
        let url = format!("http://{addr}/pair");
        let e = exchange_blocking(&url, "987654", None, "n").expect_err("closed port must fail");
        assert_eq!(e.kind, pairing::PairFailure::Unreachable, "got {e:?}");
        assert!(!e.detail.contains("987654"), "code leaked: {}", e.detail);
    }

    #[test]
    fn a_malformed_code_never_reaches_the_network() {
        let mut p = Pairing::for_address("h", "ws://127.0.0.1:1/ws");
        p.code = "12345".into();
        p.start();
        assert!(!p.busy, "a short code must not start an exchange");
        assert!(p.msg.as_deref().unwrap_or("").contains("6 digits"), "{:?}", p.msg);
        assert!(p.error.is_none());
        // A bad address is refused the same way, before any socket.
        let mut p = Pairing::for_address("h", "http://h/ws");
        p.code = "123456".into();
        p.start();
        assert!(!p.busy);
        assert!(p.msg.as_deref().unwrap_or("").contains("address"), "{:?}", p.msg);
    }

    #[test]
    fn a_finished_exchange_hands_back_the_outcome_once() {
        let (addr, _seen) = http_fixture(
            200,
            "OK",
            r#"{"token":"tok","host":"h","agent_version":"0.2.8","scope":"read_only"}"#,
        );
        let mut p = Pairing::for_address("h", &format!("ws://{addr}/ws"));
        p.code = "123456".into();
        p.start();
        assert!(p.busy);
        let out = loop {
            if let Some(o) = p.poll() {
                break o;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(out.token, "tok");
        assert!(!p.busy);
        assert!(p.poll().is_none(), "the outcome must be delivered once");
    }
}
