//! ebbtop: an authenticated readiness probe for the installers.
//!
//! `ebbtop-agent --probe <host:port> [--token-file <path>] [--timeout-ms N]`
//! opens one WebSocket connection to `/ws`, asks for metrics, and prints one
//! line: `probe: ok frames=N` on success, or `probe: denied` / `probe:
//! failed: <reason>` and a nonzero exit. The installers run it after starting
//! the service, so "the unit is loaded" and "an authenticated client gets
//! data" are two separate facts they can both check.
//!
//! Why this is hand-written: the shipped agent binary must not grow a client
//! stack (that is `ebbtop-core`, deliberately a separate crate and not a
//! dependency of the agent). The handshake needs SHA-1 plus base64, and the
//! read needs one unmasked server frame, both of which are a few lines of
//! std-only code. Gzip frames are inflated with the `flate2` the agent
//! already links.
//!
//! The token is read from a file or `EBBTOP_TOKEN`, never from argv, and it
//! is sent in the request line of a loopback/TCP connection — exactly what
//! the desktop client does. Nothing here prints the token.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use flate2::read::GzDecoder;

/// A probe result, as the installers and operators see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The handshake completed and a metrics-shaped (or requested) frame came
    /// back. `body` is the reply text, which `--dump-*` prints.
    Ok { frames: usize, body: String },
    /// The server accepted the TCP connection and closed the upgrade:
    /// a missing or wrong token.
    Denied,
    /// Anything else: refused connection, timeout, garbage.
    Failed(String),
}

impl ProbeOutcome {
    /// The line the installer parses. Always one line, never a token.
    pub fn line(&self) -> String {
        match self {
            ProbeOutcome::Ok { frames, .. } => format!("probe: ok frames={frames}"),
            ProbeOutcome::Denied => "probe: denied".to_string(),
            ProbeOutcome::Failed(why) => format!("probe: failed: {why}"),
        }
    }
    /// What `--dump-*` prints: the reply body, or the ordinary status line.
    pub fn dumped(&self) -> String {
        match self {
            ProbeOutcome::Ok { body, .. } if !body.is_empty() => body.clone(),
            other => other.line(),
        }
    }
    /// Process exit code: 0 ok, 3 denied, 1 failed. Distinct so a script can
    /// tell "wrong token" from "not listening".
    pub fn exit_code(&self) -> i32 {
        match self {
            ProbeOutcome::Ok { .. } => 0,
            ProbeOutcome::Denied => 3,
            ProbeOutcome::Failed(_) => 1,
        }
    }
}

// ---------------------------------------------------------------- SHA-1

/// FIPS 180-4 SHA-1, over the handshake key only.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];

    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    let mut w = [0u32; 80];
    for chunk in msg.as_chunks::<64>().0 {
        for (i, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

/// Standard base64 with padding, the alphabet the handshake uses.
fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// `Sec-WebSocket-Accept` for a client key, per RFC 6455 §4.2.2.
pub fn accept_key(client_key: &str) -> String {
    const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    base64(&sha1(format!("{client_key}{GUID}").as_bytes()))
}

// ------------------------------------------------------------ handshake

/// Everything the probe needs; the token is deliberately not `Debug`.
pub struct ProbeConfig {
    pub host: String,
    pub port: u16,
    pub token: Option<String>,
    pub timeout: Duration,
    /// When set, the probe sends this protocol request instead of
    /// `get_metrics` and prints the reply body verbatim. Native verification
    /// uses it to compare the agent's own numbers against `sysctl`, `vm_stat`
    /// and `df` without a second client implementation.
    pub request: Option<String>,
    /// Print the reply body to stdout instead of `probe: ok frames=N`.
    pub dump: bool,
}

impl std::fmt::Debug for ProbeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProbeConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("token", &self.token.as_ref().map(|_| "<set>"))
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Parse `host:port`, accepting `[v6]:port` and a bare host.
pub fn split_host_port(s: &str) -> Result<(String, u16), String> {
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let (h, tail) = rest
            .split_once(']')
            .ok_or_else(|| format!("invalid address {s:?}: missing ']'"))?;
        (h.to_string(), tail.strip_prefix(':').unwrap_or(""))
    } else {
        match s.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p),
            None => (s.to_string(), ""),
        }
    };
    let port: u16 = port
        .parse()
        .map_err(|_| format!("invalid address {s:?}: no usable port"))?;
    if host.is_empty() {
        return Err(format!("invalid address {s:?}: empty host"));
    }
    Ok((host, port))
}

/// One probe. Blocking; the caller runs it on a thread that does not serve
/// anything (the binary only reaches this from `--probe`).
pub fn run(cfg: &ProbeConfig) -> ProbeOutcome {
    let addr = format!("{}:{}", cfg.host, cfg.port);
    let mut stream = match TcpStream::connect(&addr) {
        Ok(s) => s,
        Err(e) => return ProbeOutcome::Failed(format!("connect {addr}: {e}")),
    };
    let _ = stream.set_read_timeout(Some(cfg.timeout));
    let _ = stream.set_write_timeout(Some(cfg.timeout));

    // A fixed, well-formed key: the response is checked against it, so a
    // wrong Accept header (a proxy, a different server) is a refusal.
    let key = "dGhlIHNhbXBsZSBub25jZQ==";
    let path = match cfg.token.as_deref() {
        Some(t) if !t.is_empty() => format!("/ws?token={t}"),
        _ => "/ws".to_string(),
    };
    let req = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\n\
         Sec-WebSocket-Version: 13\r\n\
         \r\n"
    );
    if let Err(e) = stream.write_all(req.as_bytes()) {
        return ProbeOutcome::Failed(format!("write handshake: {e}"));
    }

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) => return ProbeOutcome::Failed(format!("read handshake: {e}")),
        }
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 16 * 1024 {
            return ProbeOutcome::Failed("handshake response too large".into());
        }
    }
    let head_end = match buf.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(p) => p + 4,
        None => return ProbeOutcome::Failed("no complete handshake response".into()),
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let status = head.lines().next().unwrap_or_default();

    if !status.contains(" 101 ") {
        // The agent refuses a bad or missing token by accepting the upgrade
        // and closing, so a non-101 status here is something else (a proxy,
        // a wrong port). A 4xx/5xx is still "the server said no".
        if status.contains(" 401 ") || status.contains(" 403 ") {
            return ProbeOutcome::Denied;
        }
        return ProbeOutcome::Failed(format!("unexpected handshake status: {status:?}"));
    }
    let accept = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case("sec-websocket-accept")
                .then(|| v.trim().to_string())
        })
        .unwrap_or_default();
    if accept != accept_key(key) {
        return ProbeOutcome::Failed(format!(
            "handshake Accept {accept:?} does not match the request key"
        ));
    }

    // Ask for the requested frame, masked per RFC 6455 as a client must.
    let mask = [0x37u8, 0xfa, 0x21, 0x3d];
    let payload = cfg.request.as_deref().unwrap_or("get_metrics").as_bytes();
    if payload.len() > 125 {
        return ProbeOutcome::Failed("request too long for a single frame".into());
    }
    let mut frame = vec![0x81u8, 0x80 | payload.len() as u8];
    frame.extend_from_slice(&mask);
    frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    if let Err(e) = stream.write_all(&frame) {
        return ProbeOutcome::Failed(format!("write request: {e}"));
    }

    // Read one frame. Anything the agent sends back proves the token was
    // accepted; a close frame without a payload proves it was not.
    let mut pos = head_end;
    let mut frames = 0usize;
    loop {
        while buf.len() < pos + 2 {
            match stream.read(&mut chunk) {
                Ok(0) => return ProbeOutcome::Failed("connection closed before a reply".into()),
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    return ProbeOutcome::Failed("timed out waiting for a reply".into());
                }
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    return ProbeOutcome::Failed("timed out waiting for a reply".into());
                }
                Err(e) => return ProbeOutcome::Failed(format!("read reply: {e}")),
            }
        }
        let opcode = buf[pos] & 0x0f;
        let masked = buf[pos + 1] & 0x80 != 0;
        let len0 = (buf[pos + 1] & 0x7f) as usize;
        let mut cursor = pos + 2;
        let len = match len0 {
            126 => {
                while buf.len() < cursor + 2 {
                    match stream.read(&mut chunk) {
                        Ok(0) => return ProbeOutcome::Failed("truncated frame".into()),
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        Err(e) => return ProbeOutcome::Failed(format!("read reply: {e}")),
                    }
                }
                let v = u16::from_be_bytes([buf[cursor], buf[cursor + 1]]) as usize;
                cursor += 2;
                v
            }
            127 => {
                while buf.len() < cursor + 8 {
                    match stream.read(&mut chunk) {
                        Ok(0) => return ProbeOutcome::Failed("truncated frame".into()),
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        Err(e) => return ProbeOutcome::Failed(format!("read reply: {e}")),
                    }
                }
                let mut b = [0u8; 8];
                b.copy_from_slice(&buf[cursor..cursor + 8]);
                cursor += 8;
                u64::from_be_bytes(b) as usize
            }
            n => n,
        };
        let mask_len = if masked { 4 } else { 0 };
        while buf.len() < cursor + mask_len + len {
            match stream.read(&mut chunk) {
                Ok(0) => return ProbeOutcome::Failed("truncated frame".into()),
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) => return ProbeOutcome::Failed(format!("read reply: {e}")),
            }
        }
        let mask_key: Vec<u8> = buf[cursor..cursor + mask_len].to_vec();
        cursor += mask_len;
        let mut data = buf[cursor..cursor + len].to_vec();
        if masked {
            for (i, b) in data.iter_mut().enumerate() {
                *b ^= mask_key[i % 4];
            }
        }
        pos = cursor + len;

        match opcode {
            0x8 => {
                return if frames == 0 {
                    ProbeOutcome::Denied
                } else {
                    ProbeOutcome::Ok {
                        frames,
                        body: String::new(),
                    }
                };
            }
            0x0..=0x2 => {
                // Only a metrics-shaped or `get_disks`/`get_processes` body
                // counts; an empty frame from some other server must not be
                // reported as a healthy agent. A process-detail or error reply
                // is also a real answer from this agent, so it counts too —
                // anything else is refused rather than assumed.
                let text = if opcode == 0x2 {
                    let mut out = String::new();
                    GzDecoder::new(&data[..])
                        .read_to_string(&mut out)
                        .map(|_| out)
                        .unwrap_or_default()
                } else {
                    String::from_utf8_lossy(&data).to_string()
                };
                let shaped = text.contains("sampled_at_ms")
                    || text.contains("\"error\"")
                    || text.contains("process_count")
                    || text.contains("\"total\"")
                    || text.contains("\"pid\"");
                if shaped {
                    frames += 1;
                    return ProbeOutcome::Ok { frames, body: text };
                }
                return ProbeOutcome::Failed(
                    "a frame arrived but it was not a reply this agent produces".to_string(),
                );
            }
            _ => continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The RFC 6455 §1.3 example, which is the one every implementation is
    /// checked against.
    #[test]
    fn accept_key_matches_rfc6455_example() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn sha1_known_vectors() {
        let hex = |d: [u8; 20]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(hex(sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(sha1(b"The quick brown fox jumps over the lazy dog")),
            "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12"
        );
    }

    #[test]
    fn base64_padding_and_alphabet() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
    }

    #[test]
    fn addresses_split() {
        assert_eq!(
            split_host_port("127.0.0.1:45976").unwrap(),
            ("127.0.0.1".into(), 45976)
        );
        assert_eq!(
            split_host_port("[::1]:45976").unwrap(),
            ("::1".into(), 45976)
        );
        assert!(split_host_port("127.0.0.1").is_err());
        assert!(split_host_port(":45976").is_err());
        assert!(split_host_port("host:notaport").is_err());
    }

    #[test]
    fn probe_outcome_lines_and_codes() {
        let ok = |frames: usize| ProbeOutcome::Ok {
            frames,
            body: String::new(),
        };
        assert_eq!(ok(1).line(), "probe: ok frames=1");
        assert_eq!(ProbeOutcome::Denied.line(), "probe: denied");
        assert_eq!(ok(2).exit_code(), 0);
        assert_eq!(ProbeOutcome::Denied.exit_code(), 3);
        assert_eq!(ProbeOutcome::Failed("x".into()).exit_code(), 1);

        // --dump-* prints the reply body when there is one and the status line
        // otherwise, so a caller never gets an empty document instead of an
        // error.
        let with_body = ProbeOutcome::Ok {
            frames: 1,
            body: r#"{"sampled_at_ms":1,"mem_total":8}"#.into(),
        };
        assert_eq!(with_body.dumped(), r#"{"sampled_at_ms":1,"mem_total":8}"#);
        assert_eq!(ok(1).dumped(), "probe: ok frames=1");
        assert_eq!(ProbeOutcome::Denied.dumped(), "probe: denied");
        assert_eq!(
            ProbeOutcome::Failed("connect refused".into()).dumped(),
            "probe: failed: connect refused"
        );

        // A failure reason never carries a token, and the config's Debug
        // never prints one.
        let cfg = ProbeConfig {
            host: "127.0.0.1".into(),
            port: 1,
            token: Some("supersecret".into()),
            timeout: Duration::from_millis(10),
            request: Some("get_disks".into()),
            dump: true,
        };
        assert!(!format!("{cfg:?}").contains("supersecret"));
    }
}
