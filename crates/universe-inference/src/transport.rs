//! Byte transports. Mechanism with zero policy: move these bytes to that
//! endpoint, return exactly what came back.
//!
//! A transport selects no model, writes no prompt, sets no decoding parameter,
//! parses nothing, and retries nothing. Host, path, method, headers, body and
//! timeout all arrive from the authored routing data.
//!
//! Two real transports, because `http://` and `https://` are genuinely
//! different problems:
//!
//! * [`TcpHttpTransport`] — HTTP/1.1 over `std::net::TcpStream`, for local
//!   endpoints such as Ollama.
//! * [`CurlHttpsTransport`] — delegates TLS to the platform's `curl` binary.
//!   This workspace has no TLS crate and adding one would churn the shared
//!   `Cargo.lock`; an authorized external transport binary is a legitimate
//!   member of the trusted computing base (CLAUDE.md permits "authorized
//!   transports for real external effects"). The credential is passed through
//!   a config on **stdin**, never on the command line, so it is not visible in
//!   the process table.
//!
//! Both return the RAW wire bytes (status line + headers + body), so the layer
//! above sees one shape regardless of which transport carried it.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::time::Duration;

/// One outbound request, fully determined by routing data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WireRequest {
    pub endpoint: String,
    pub method: String,
    /// Header name/value pairs. Values may be secret; nothing here is ever
    /// recorded into a receipt (only names are, by the layer above).
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timeout: Duration,
}

/// Whether a transport can run at all, decided without running it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransportReadiness {
    Ready,
    /// A precondition of the transport itself is absent (e.g. no TLS-capable
    /// binary). Nothing about the remote provider has been measured.
    Unavailable { reason: String },
}

pub trait WireTransport: Send {
    fn transport_id(&self) -> &str;
    fn readiness(&self) -> TransportReadiness;
    /// Send and return the RAW wire response. `Err` is measured transport
    /// failure with a concrete reason — never an empty success.
    fn send(&mut self, request: &WireRequest) -> Result<Vec<u8>, String>;
}

// ===========================================================================
// http:// over a plain socket
// ===========================================================================

pub struct TcpHttpTransport;

impl TcpHttpTransport {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TcpHttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl WireTransport for TcpHttpTransport {
    fn transport_id(&self) -> &str {
        "tcp-http/1.1"
    }

    fn readiness(&self) -> TransportReadiness {
        TransportReadiness::Ready
    }

    fn send(&mut self, request: &WireRequest) -> Result<Vec<u8>, String> {
        // Validate BEFORE opening a socket. A malformed header must fail
        // closed without making any external contact at all — refusing after
        // connecting would already have touched the outside world.
        check_headers(&request.headers)?;
        let (host, port, path) = split_endpoint(&request.endpoint, "http://")?;
        let authority = format!("{host}:{port}");
        let address = authority
            .to_socket_addrs()
            .map_err(|error| format!("endpoint did not resolve: {error}"))?
            .next()
            .ok_or_else(|| format!("endpoint resolved to no address: {authority}"))?;
        let mut stream = TcpStream::connect_timeout(&address, request.timeout)
            .map_err(|error| format!("connect failed: {error}"))?;
        stream
            .set_read_timeout(Some(request.timeout))
            .map_err(|error| format!("read timeout not set: {error}"))?;
        stream
            .set_write_timeout(Some(request.timeout))
            .map_err(|error| format!("write timeout not set: {error}"))?;

        let mut head = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
            request.method,
            path,
            authority,
            request.body.len()
        );
        for (name, value) in &request.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");

        stream
            .write_all(head.as_bytes())
            .and_then(|()| stream.write_all(&request.body))
            .and_then(|()| stream.flush())
            .map_err(|error| format!("request write failed: {error}"))?;

        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .map_err(|error| format!("response read failed: {error}"))?;
        if response.is_empty() {
            return Err("server closed the connection with no response bytes".to_string());
        }
        Ok(response)
    }
}

// ===========================================================================
// https:// via an authorized external transport binary
// ===========================================================================

pub struct CurlHttpsTransport {
    binary: String,
}

impl CurlHttpsTransport {
    pub fn new() -> Self {
        Self {
            binary: "curl".to_string(),
        }
    }

    pub fn with_binary(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
        }
    }
}

impl Default for CurlHttpsTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl WireTransport for CurlHttpsTransport {
    fn transport_id(&self) -> &str {
        "curl-https"
    }

    fn readiness(&self) -> TransportReadiness {
        match Command::new(&self.binary)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(status) if status.success() => TransportReadiness::Ready,
            Ok(status) => TransportReadiness::Unavailable {
                reason: format!("{} --version exited with {status}", self.binary),
            },
            Err(error) => TransportReadiness::Unavailable {
                reason: format!(
                    "https transport binary {:?} is not runnable: {error}",
                    self.binary
                ),
            },
        }
    }

    fn send(&mut self, request: &WireRequest) -> Result<Vec<u8>, String> {
        // Validate before staging anything or spawning anything.
        check_headers(&request.headers)?;
        if !request.endpoint.starts_with("https://") {
            return Err(format!(
                "curl-https transport refuses non-https endpoint {} (no silent downgrade)",
                request.endpoint
            ));
        }

        // The body goes to a temp file so the config on stdin stays small and
        // free of escaping hazards. The body carries no credential.
        let body_path = std::env::temp_dir().join(format!(
            "universe-inference-body-{}-{}.json",
            std::process::id(),
            now_nanos()
        ));
        std::fs::write(&body_path, &request.body)
            .map_err(|error| format!("could not stage request body: {error}"))?;
        let body_arg = body_path.to_string_lossy().replace('\\', "/");

        // Credentials travel in this config, delivered on stdin. They never
        // appear in argv, so they are not visible in the process table.
        let mut config = String::new();
        config.push_str(&format!("url = \"{}\"\n", escape_config(&request.endpoint)));
        config.push_str(&format!("request = \"{}\"\n", escape_config(&request.method)));
        for (name, value) in &request.headers {
            config.push_str(&format!(
                "header = \"{}: {}\"\n",
                escape_config(name),
                escape_config(value)
            ));
        }
        config.push_str(&format!("data-binary = \"@{}\"\n", escape_config(&body_arg)));
        config.push_str(&format!(
            "max-time = \"{}\"\n",
            request.timeout.as_secs_f64().max(0.1)
        ));

        let spawn = Command::new(&self.binary)
            // -s silent, -S still report errors, -i include response headers so
            // the raw shape matches the TCP transport, -K - read config (and
            // therefore the credential) from stdin.
            .args(["-sS", "-i", "-K", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        let mut child = match spawn {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_file(&body_path);
                return Err(format!("could not start {:?}: {error}", self.binary));
            }
        };
        if let Some(stdin) = child.stdin.as_mut() {
            if let Err(error) = stdin.write_all(config.as_bytes()) {
                let _ = std::fs::remove_file(&body_path);
                return Err(format!("could not write transport config: {error}"));
            }
        }
        let output = child.wait_with_output();
        let _ = std::fs::remove_file(&body_path);
        let output = output.map_err(|error| format!("transport did not complete: {error}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "transport exited with {} : {}",
                output.status,
                stderr.trim()
            ));
        }
        if output.stdout.is_empty() {
            return Err("transport returned no response bytes".to_string());
        }
        Ok(output.stdout)
    }
}

// ===========================================================================
// A stub, for proving wiring without a network or a credential
// ===========================================================================

/// Returns a canned wire response. Used ONLY to exercise a provider's wiring
/// when a real call cannot be made, and every run that uses it says so
/// explicitly in its evidence — a stubbed attempt is never reported as a real
/// measurement of the remote provider.
pub struct StubTransport {
    pub id: String,
    pub status: u16,
    pub body: Vec<u8>,
    /// Requests this stub actually received, so a test can prove the rendered
    /// body and headers were correct without any network.
    pub seen: Vec<WireRequest>,
}

impl StubTransport {
    pub fn new(id: impl Into<String>, status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            id: id.into(),
            status,
            body: body.into(),
            seen: Vec::new(),
        }
    }
}

impl WireTransport for StubTransport {
    fn transport_id(&self) -> &str {
        &self.id
    }

    fn readiness(&self) -> TransportReadiness {
        TransportReadiness::Ready
    }

    fn send(&mut self, request: &WireRequest) -> Result<Vec<u8>, String> {
        self.seen.push(request.clone());
        let mut raw = format!(
            "HTTP/1.1 {} STUB\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
            self.status,
            self.body.len()
        )
        .into_bytes();
        raw.extend_from_slice(&self.body);
        Ok(raw)
    }
}

// ===========================================================================
// Shared helpers
// ===========================================================================

/// Splits a raw HTTP response into (status code, body). Missing or unparseable
/// pieces are `None` — never defaulted to 200 and never to an empty body.
pub fn split_http(raw: &[u8]) -> (Option<u16>, Option<&[u8]>) {
    let text = String::from_utf8_lossy(raw);
    let status = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok());
    let body = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| &raw[index + 4..]);
    (status, body)
}

/// Refuse headers that could forge additional headers on the wire.
///
/// Called by every real transport BEFORE it opens a socket, stages a file, or
/// spawns a process — a malformed header must never produce external contact.
fn check_headers(headers: &[(String, String)]) -> Result<(), String> {
    for (name, value) in headers {
        if name.is_empty()
            || name.contains(':')
            || name.contains('\r')
            || name.contains('\n')
            || name.contains(' ')
        {
            return Err(format!("header name {name:?} is not transportable"));
        }
        if value.contains('\r') || value.contains('\n') {
            return Err(format!("header {name} is not transportable (control bytes)"));
        }
    }
    Ok(())
}

fn split_endpoint<'a>(
    endpoint: &'a str,
    scheme: &str,
) -> Result<(String, u16, &'a str), String> {
    let rest = endpoint
        .strip_prefix(scheme)
        .ok_or_else(|| format!("endpoint {endpoint} does not use {scheme} (no silent downgrade)"))?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (
            host.to_string(),
            port.parse::<u16>()
                .map_err(|error| format!("endpoint port is not a port: {error}"))?,
        ),
        None => (authority.to_string(), 80u16),
    };
    Ok((host, port, path))
}

fn escape_config(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_http_reports_absence_rather_than_defaulting() {
        let (status, body) = split_http(b"HTTP/1.1 200 OK\r\nx: y\r\n\r\n{\"a\":1}");
        assert_eq!(status, Some(200));
        assert_eq!(body, Some(&b"{\"a\":1}"[..]));

        // Garbage in: no invented 200, no invented empty body.
        let (status, body) = split_http(b"not http at all");
        assert_eq!(status, None);
        assert_eq!(body, None);
    }

    #[test]
    fn split_http_handles_the_http2_status_line_curl_emits() {
        let (status, body) = split_http(b"HTTP/2 200 \r\ncontent-type: application/json\r\n\r\n{}");
        assert_eq!(status, Some(200));
        assert_eq!(body, Some(&b"{}"[..]));
    }

    #[test]
    fn a_non_https_endpoint_is_refused_by_the_tls_transport() {
        let mut transport = CurlHttpsTransport::new();
        let error = transport
            .send(&WireRequest {
                endpoint: "http://example.invalid/x".into(),
                method: "POST".into(),
                headers: vec![],
                body: b"{}".to_vec(),
                timeout: Duration::from_millis(100),
            })
            .unwrap_err();
        assert!(error.contains("no silent downgrade"), "{error}");
    }

    #[test]
    fn header_injection_through_authored_data_is_refused_before_any_contact() {
        // The endpoint is unroutable on purpose: if this test ever passes by
        // *connecting first*, it would hang on the connect rather than return
        // instantly. Returning fast is part of what is being asserted.
        let started = std::time::Instant::now();
        let mut transport = TcpHttpTransport::new();
        let error = transport
            .send(&WireRequest {
                endpoint: "http://192.0.2.1:81/x".into(),
                method: "POST".into(),
                headers: vec![("x-evil".into(), "a\r\nx-forged: 1".into())],
                body: b"{}".to_vec(),
                timeout: Duration::from_secs(30),
            })
            .unwrap_err();
        assert!(error.contains("control bytes"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "refusal must happen before any socket is opened"
        );

        // The TLS transport refuses on the same terms, also before staging a
        // body file or spawning anything.
        let mut tls = CurlHttpsTransport::new();
        let error = tls
            .send(&WireRequest {
                endpoint: "https://example.invalid/x".into(),
                method: "POST".into(),
                headers: vec![("x-api-key".into(), "a\nx-forged: 1".into())],
                body: b"{}".to_vec(),
                timeout: Duration::from_secs(30),
            })
            .unwrap_err();
        assert!(error.contains("control bytes"), "{error}");
    }

    #[test]
    fn a_malformed_header_name_is_refused_too() {
        let mut transport = TcpHttpTransport::new();
        for name in ["", "x evil", "x:evil", "x\revil"] {
            let error = transport
                .send(&WireRequest {
                    endpoint: "http://192.0.2.1:81/x".into(),
                    method: "POST".into(),
                    headers: vec![(name.into(), "v".into())],
                    body: b"{}".to_vec(),
                    timeout: Duration::from_secs(30),
                })
                .unwrap_err();
            assert!(error.contains("not transportable"), "name {name:?}: {error}");
        }
    }

    #[test]
    fn a_refused_local_port_is_measured_failure_not_silence() {
        let mut transport = TcpHttpTransport::new();
        // Port 1 on loopback: nothing listens, so this is a real, fast failure.
        let result = transport.send(&WireRequest {
            endpoint: "http://127.0.0.1:1/api/generate".into(),
            method: "POST".into(),
            headers: vec![],
            body: b"{}".to_vec(),
            timeout: Duration::from_millis(500),
        });
        let error = result.expect_err("nothing listens on 127.0.0.1:1");
        assert!(!error.is_empty(), "a failure must carry a concrete reason");
    }

    #[test]
    fn stub_records_exactly_what_it_was_asked_to_send() {
        let mut stub = StubTransport::new("stub", 200, br#"{"response":"ok"}"#.to_vec());
        let request = WireRequest {
            endpoint: "https://example.invalid/v1/messages".into(),
            method: "POST".into(),
            headers: vec![("x-api-key".into(), "SECRET".into())],
            body: br#"{"model":"m"}"#.to_vec(),
            timeout: Duration::from_secs(1),
        };
        let raw = stub.send(&request).unwrap();
        let (status, body) = split_http(&raw);
        assert_eq!(status, Some(200));
        assert_eq!(body, Some(&br#"{"response":"ok"}"#[..]));
        assert_eq!(stub.seen.len(), 1);
        assert_eq!(stub.seen[0].body, request.body);
    }
}
