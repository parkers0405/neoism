//! Agent HTTP transport: pooled reqwest for bounded requests, polling sockets
//! for SSE (whose callers rely on WouldBlock to check cancellation).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

// A dedicated, permanently driven runtime avoids blocking-client/nested block_on
// panics when a synchronous caller happens to be inside a Tokio runtime. Cloned
// clients share their pool; credentials are attached per request, never defaults.
struct RequestTransport {
    runtime: tokio::runtime::Runtime,
    local_client: reqwest::Client,
    remote_client: reqwest::Client,
}

// Classify the URL itself, without DNS: remote names must keep their configured
// proxy behavior. Include all of 127/8 and IPv4-mapped IPv6 loopback literals.
fn is_loopback_endpoint(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(host)) => {
            host.eq_ignore_ascii_case("localhost")
                || host.eq_ignore_ascii_case("localhost.")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => {
            ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback())
        }
        None => false,
    }
}

fn proxy_policy(builder: reqwest::ClientBuilder, local: bool) -> reqwest::ClientBuilder {
    if local {
        builder.no_proxy()
    } else {
        builder
    }
}

impl RequestTransport {
    fn new() -> Result<Self, String> {
        Self::with_clients(|| {
            let build = |local| {
                proxy_policy(reqwest::Client::builder(), local)
                    .connect_timeout(Duration::from_secs(10))
                    .pool_idle_timeout(Duration::from_secs(90))
                    .pool_max_idle_per_host(8)
                    // Never forward bearer tokens or replay writes via redirects.
                    .redirect(reqwest::redirect::Policy::none())
                    .retry(reqwest::retry::never())
                    .build()
                    .map_err(|_| "Could not initialize agent HTTP client".to_string())
            };
            Ok((build(true)?, build(false)?))
        })
    }

    fn with_clients(
        build: impl FnOnce() -> Result<(reqwest::Client, reqwest::Client), String>,
    ) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("agent-http")
            .build()
            .map_err(|_| "Could not initialize agent HTTP runtime".to_string())?;
        let clients = {
            let _guard = runtime.enter();
            // Do not propagate this error with `?`: Runtime::drop blocks and
            // panics if initialization was called inside another Tokio runtime.
            build()
        };
        match clients {
            Ok((local_client, remote_client)) => Ok(Self {
                runtime,
                local_client,
                remote_client,
            }),
            Err(error) => {
                // The enter guard is gone; shutdown must remain nonblocking even
                // if the caller itself is running in an async context.
                runtime.shutdown_background();
                Err(error)
            }
        }
    }
}

pub(super) const RESPONSE_BODY_LIMIT: usize = 32 * 1024 * 1024;

pub(super) fn request(
    url: url::Url,
    method: reqwest::Method,
    credential: Option<String>,
    body: Option<String>,
    deadline: Duration,
) -> Result<(u16, String, Vec<u8>), String> {
    static TRANSPORT: std::sync::OnceLock<Result<RequestTransport, String>> =
        std::sync::OnceLock::new();
    let transport = TRANSPORT
        .get_or_init(RequestTransport::new)
        .as_ref()
        .map_err(Clone::clone)?;
    let client = if is_loopback_endpoint(&url) {
        &transport.local_client
    } else {
        &transport.remote_client
    }
    .clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    transport.runtime.spawn(async move {
        let result = tokio::time::timeout(deadline, async move {
            let attempts = if method == reqwest::Method::GET { 2 } else { 1 };
            for attempt in 0..attempts {
                let mut request = client
                    .request(method.clone(), url.clone())
                    .header(reqwest::header::ACCEPT, "application/json");
                if let Some(token) = credential.as_ref() {
                    request = request.bearer_auth(token);
                }
                if let Some(body) = body.as_ref() {
                    request = request
                        .header(reqwest::header::CONTENT_TYPE, "application/json")
                        .body(body.clone());
                }
                let response = request.send().await;
                let mut response = match response {
                    Ok(response) => response,
                    Err(error) => {
                        if attempt + 1 < attempts
                            && (error.is_connect()
                                || error.is_request()
                                || error.is_body())
                        {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            continue;
                        }
                        // reqwest errors can contain URLs, including query secrets.
                        return Err(if error.is_timeout() {
                            "Neoism request timed out waiting for the server response"
                        } else {
                            "Neoism Agent HTTP connection failed"
                        }
                        .to_string());
                    }
                };
                let status = response.status();
                if attempt + 1 < attempts && matches!(status.as_u16(), 502 | 503 | 504) {
                    // Drop rather than drain an unbounded failure response.
                    drop(response);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                if response
                    .content_length()
                    .is_some_and(|len| len > RESPONSE_BODY_LIMIT as u64)
                {
                    return Err("Neoism Agent response exceeds body limit".to_string());
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|_| "Failed to read Neoism Agent response".to_string())?
                {
                    if chunk.len() > RESPONSE_BODY_LIMIT - bytes.len() {
                        return Err(
                            "Neoism Agent response exceeds body limit".to_string()
                        );
                    }
                    bytes.extend_from_slice(&chunk);
                }
                return Ok((
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("").to_string(),
                    bytes,
                ));
            }
            unreachable!()
        })
        .await
        .unwrap_or_else(|_| {
            Err("Neoism request timed out waiting for the server response".to_string())
        });
        let _ = sender.send(result);
    });
    receiver
        .recv_timeout(deadline.saturating_add(Duration::from_secs(1)))
        .map_err(|_| "Neoism Agent HTTP worker timed out or stopped".to_string())?
}

#[cfg(test)]
mod request_tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    fn server() -> (TcpListener, url::Url) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url =
            url::Url::parse(&format!("http://{}/test", listener.local_addr().unwrap()))
                .unwrap();
        (listener, url)
    }

    fn read_request(stream: &mut TcpStream) {
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            bytes.push(byte[0]);
            assert!(bytes.len() < 64 * 1024);
        }
    }

    #[test]
    fn loopback_policy_does_not_capture_remote_endpoints() {
        for endpoint in [
            "http://localhost:4096/agent",
            "http://LOCALHOST./agent",
            "http://127.0.0.1:4096",
            "http://127.23.45.67",
            "http://[::1]:4096",
            "http://[::ffff:127.0.0.1]:4096",
        ] {
            assert!(
                is_loopback_endpoint(&url::Url::parse(endpoint).unwrap()),
                "{endpoint}"
            );
        }
        for endpoint in [
            "https://agent.example/agent",
            "http://localhost.example",
            "http://not-localhost",
            "http://192.168.1.2:4096",
            "http://[2001:db8::1]",
            "http://[::ffff:192.168.1.2]",
        ] {
            assert!(
                !is_loopback_endpoint(&url::Url::parse(endpoint).unwrap()),
                "{endpoint}"
            );
        }
    }

    #[test]
    fn local_policy_bypasses_proxies_and_remote_policy_preserves_them() {
        // Use explicit proxies rather than mutating global environment variables
        // underneath parallel tests. no_proxy disables both explicit and automatic
        // environment/system proxy discovery in reqwest.
        for local in [true, false] {
            let (listener, server_url) = server();
            let proxy_url = if local {
                "http://127.0.0.1:1".to_string()
            } else {
                server_url.to_string()
            };
            let endpoint = if local {
                server_url
            } else {
                url::Url::parse("http://remote.invalid/test").unwrap()
            };
            let worker = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                read_request(&mut stream);
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                    .unwrap();
            });
            let client = proxy_policy(
                reqwest::Client::builder().proxy(reqwest::Proxy::all(proxy_url).unwrap()),
                local,
            )
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                assert_eq!(
                    client
                        .get(endpoint)
                        .send()
                        .await
                        .unwrap()
                        .bytes()
                        .await
                        .unwrap()
                        .as_ref(),
                    b"{}"
                );
            });
            worker.join().unwrap();
        }
    }

    #[test]
    fn failed_client_initialization_inside_tokio_shuts_down_without_panicking() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let result = RequestTransport::with_clients(|| {
                // Exercise an actual ClientBuilder::build error, not a panic.
                let client = reqwest::Client::builder()
                    .user_agent("invalid\nheader")
                    .build()
                    .map_err(|_| "expected client build failure".to_string())?;
                Ok((client.clone(), client))
            });
            assert!(
                matches!(result, Err(error) if error == "expected client build failure")
            );
            // The failed initialization must also restore the caller's runtime.
            tokio::time::sleep(Duration::from_millis(1)).await;
        });
    }

    #[test]
    fn content_length_finishes_without_eof_and_reuses_connection() {
        let (listener, url) = server();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // Both requests must arrive on this socket. Keeping it open also
            // proves response completion is based on framing, not EOF.
            for _ in 0..2 {
                read_request(&mut stream);
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                    .unwrap();
            }
        });
        for _ in 0..2 {
            assert_eq!(
                request(
                    url.clone(),
                    reqwest::Method::GET,
                    None,
                    None,
                    Duration::from_secs(2)
                )
                .unwrap()
                .2,
                b"{}"
            );
        }
        worker.join().unwrap();
    }

    #[test]
    fn transient_get_status_is_retried() {
        let (listener, url) = server();
        let worker = thread::spawn(move || {
            for status in ["503 Unavailable", "200 OK"] {
                let (mut stream, _) = listener.accept().unwrap();
                read_request(&mut stream);
                write!(stream, "HTTP/1.1 {status}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").unwrap();
            }
        });
        assert_eq!(
            request(
                url,
                reqwest::Method::GET,
                None,
                None,
                Duration::from_secs(3)
            )
            .unwrap()
            .0,
            200
        );
        worker.join().unwrap();
    }

    #[test]
    fn transient_get_disconnect_is_retried_but_post_is_not() {
        for method in [reqwest::Method::GET, reqwest::Method::POST] {
            let (listener, url) = server();
            let is_get = method == reqwest::Method::GET;
            let worker = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                read_request(&mut stream);
                drop(stream); // ambiguous outcome: a POST may already be applied
                if is_get {
                    let (mut stream, _) = listener.accept().unwrap();
                    read_request(&mut stream);
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                        .unwrap();
                }
                listener.set_nonblocking(true).unwrap();
                thread::sleep(Duration::from_millis(300));
                assert!(
                    matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
                );
            });
            let result = request(url, method, None, None, Duration::from_secs(2));
            assert_eq!(result.is_ok(), is_get);
            worker.join().unwrap();
        }
    }

    #[test]
    fn post_service_unavailable_is_not_retried() {
        let (listener, url) = server();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            stream.write_all(b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            listener.set_nonblocking(true).unwrap();
            thread::sleep(Duration::from_millis(300));
            assert!(
                matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
            );
        });
        assert_eq!(
            request(
                url,
                reqwest::Method::POST,
                None,
                None,
                Duration::from_secs(2)
            )
            .unwrap()
            .0,
            503
        );
        worker.join().unwrap();
    }

    #[test]
    fn delayed_admission_can_exceed_old_five_second_timeout() {
        let (listener, url) = server();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            thread::sleep(Duration::from_millis(5200));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                .unwrap();
        });
        assert!(request(
            url,
            reqwest::Method::POST,
            None,
            None,
            Duration::from_secs(8)
        )
        .is_ok());
        worker.join().unwrap();
    }

    #[test]
    fn deadline_and_body_limit_are_enforced() {
        let (listener, url) = server();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            thread::sleep(Duration::from_millis(250));
        });
        assert!(request(
            url,
            reqwest::Method::POST,
            None,
            None,
            Duration::from_millis(50)
        )
        .unwrap_err()
        .contains("timed out"));
        worker.join().unwrap();
        let (listener, url) = server();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                RESPONSE_BODY_LIMIT + 1
            )
            .unwrap();
        });
        assert!(request(
            url,
            reqwest::Method::GET,
            None,
            None,
            Duration::from_secs(2)
        )
        .unwrap_err()
        .contains("body limit"));
        worker.join().unwrap();
    }

    #[test]
    fn chunked_response_finishes_without_eof() {
        let (listener, url) = server();
        let (release, wait) = std::sync::mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n").unwrap();
            let _ = wait.recv_timeout(Duration::from_secs(10));
        });
        let result = request(
            url,
            reqwest::Method::GET,
            None,
            None,
            Duration::from_secs(2),
        );
        release.send(()).unwrap();
        assert_eq!(result.unwrap().2, b"{}");
        worker.join().unwrap();
    }

    #[test]
    fn sse_socket_remains_pollable_and_delivers_later_events() {
        let (listener, _) = server();
        let addr = listener.local_addr().unwrap();
        let (release, wait) = std::sync::mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            wait.recv_timeout(Duration::from_secs(10)).unwrap();
            stream.write_all(b"data: {}\n\n").unwrap();
        });
        let mut stream = AgentTransport::connect(
            &addr,
            "127.0.0.1",
            false,
            Duration::from_secs(1),
            Duration::from_millis(20),
            Duration::from_secs(1),
        )
        .unwrap();
        stream
            .write_all(b"GET /events HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut byte = [0];
        assert!(
            matches!(stream.read(&mut byte), Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
        );
        release.send(()).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(1)));
        let mut event = [0; 10];
        stream.read_exact(&mut event).unwrap();
        assert_eq!(&event, b"data: {}\n\n");
        worker.join().unwrap();
    }

    #[test]
    fn sync_request_inside_tokio_runtime_does_not_panic() {
        let (listener, url) = server();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
                .unwrap();
        });
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                assert_eq!(
                    request(
                        url,
                        reqwest::Method::GET,
                        None,
                        None,
                        Duration::from_secs(2)
                    )
                    .unwrap()
                    .0,
                    204
                );
            });
        worker.join().unwrap();
    }
}

pub(crate) enum AgentTransport {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl AgentTransport {
    pub(crate) fn connect(
        addr: &SocketAddr,
        host: &str,
        tls: bool,
        connect_timeout: Duration,
        read_timeout: Duration,
        write_timeout: Duration,
    ) -> Result<Self, String> {
        let stream = TcpStream::connect_timeout(addr, connect_timeout)
            .map_err(|error| format!("not reachable: {error}"))?;
        let _ = stream.set_write_timeout(Some(write_timeout));
        if !tls {
            let _ = stream.set_read_timeout(Some(read_timeout));
            return Ok(Self::Plain(stream));
        }
        // Drive the handshake with a forgiving poll timeout, then hand the
        // caller's read timeout to the finished session.
        let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
        let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
            .map_err(|_| format!("invalid TLS server name '{host}'"))?;
        let mut connection =
            rustls::ClientConnection::new(tls_client_config(), server_name)
                .map_err(|error| format!("failed to start TLS: {error}"))?;
        let mut stream = stream;
        let deadline = Instant::now() + Duration::from_secs(5);
        while connection.is_handshaking() {
            match connection.complete_io(&mut stream) {
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    if Instant::now() > deadline {
                        return Err("TLS handshake timed out".to_string());
                    }
                }
                Err(error) => return Err(format!("TLS handshake failed: {error}")),
            }
        }
        let _ = stream.set_read_timeout(Some(read_timeout));
        Ok(Self::Tls(Box::new(rustls::StreamOwned::new(
            connection, stream,
        ))))
    }

    pub(crate) fn set_read_timeout(&self, timeout: Option<Duration>) {
        let socket = match self {
            Self::Plain(stream) => stream,
            Self::Tls(stream) => stream.get_ref(),
        };
        let _ = socket.set_read_timeout(timeout);
    }
}

impl Read for AgentTransport {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buf),
            Self::Tls(stream) => stream.read(buf),
        }
    }
}

impl Write for AgentTransport {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buf),
            Self::Tls(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

fn tls_client_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: std::sync::OnceLock<Arc<rustls::ClientConfig>> =
        std::sync::OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            };
            Arc::new(
                rustls::ClientConfig::builder_with_provider(Arc::new(
                    rustls::crypto::ring::default_provider(),
                ))
                .with_safe_default_protocol_versions()
                .expect("rustls default protocol versions")
                .with_root_certificates(roots)
                .with_no_client_auth(),
            )
        })
        .clone()
}
