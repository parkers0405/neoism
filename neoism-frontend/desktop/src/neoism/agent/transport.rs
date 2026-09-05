//! Synchronous agent-server transport: plain TCP for local daemons, rustls
//! for HTTPS-fronted hosted servers. Keeps the blocking-socket semantics the
//! agent client threads rely on (bounded read timeouts polled as WouldBlock).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
