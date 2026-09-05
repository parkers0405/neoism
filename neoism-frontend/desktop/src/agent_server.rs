use std::io::{Read, Write};
use std::net::ToSocketAddrs;
use std::time::{Duration, Instant};

const DEFAULT_SERVER: &str = "http://127.0.0.1:4096";
const DEFAULT_PORT: u16 = 4096;
const HEALTH_PATH: &str = "/v2/health";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(8);

pub(crate) fn ensure_started() {
    ensure_started_inner(false);
}

pub(crate) fn ensure_started_for_request() {
    ensure_started_inner(true);
}

fn ensure_started_inner(wait_for_health: bool) {
    let server = configured_server();
    std::env::set_var("NEOISM_SERVER", &server);
    std::env::set_var("NEOISM_AGENT_SERVER", &server);

    if is_healthy(&server) {
        tracing::info!(target: "neoism::agent_server", server, "using existing Neoism Agent server");
        return;
    }

    if wait_for_health && !wait_until_healthy(&server, STARTUP_TIMEOUT) {
        tracing::warn!(
            target: "neoism::agent_server",
            server,
            "daemon-owned Neoism Agent server did not become healthy before request"
        );
    }
}

fn configured_server() -> String {
    std::env::var("NEOISM_AGENT_SERVER")
        .ok()
        .or_else(|| std::env::var("NEOISM_SERVER").ok())
        .map(|server| server.trim().trim_end_matches('/').to_string())
        .filter(|server| !server.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVER.to_string())
}

fn is_healthy(server: &str) -> bool {
    let Ok(response) = http_get(server, HEALTH_PATH, Duration::from_millis(250)) else {
        return false;
    };
    response.starts_with("HTTP/1.1 200 ") || response.starts_with("HTTP/1.0 200 ")
}

fn wait_until_healthy(server: &str, timeout: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if is_healthy(server) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    is_healthy(server)
}

fn http_get(server: &str, path: &str, timeout: Duration) -> Result<String, String> {
    let (tls, host, port, base_path) = parse_http_server(server)?;
    let addr = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|error| format!("failed to resolve Neoism Agent server: {error}"))?
        .next()
        .ok_or_else(|| "failed to resolve Neoism Agent server".to_string())?;
    let mut stream = crate::neoism::agent::transport::AgentTransport::connect(
        &addr, &host, tls, timeout, timeout, timeout,
    )
    .map_err(|error| format!("Neoism Agent is not reachable at {server}: {error}"))?;

    let request_path = request_path(&base_path, path);
    let request = format!(
        "GET {request_path} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).map_err(|error| {
        format!("failed to write Neoism Agent health request: {error}")
    })?;
    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|error| {
        format!("failed to read Neoism Agent health response: {error}")
    })?;
    Ok(response)
}

fn parse_http_server(server: &str) -> Result<(bool, String, u16, String), String> {
    let (tls, rest) = if let Some(rest) = server.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = server.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(format!(
            "unsupported Neoism Agent server '{server}'; expected http:// or https://"
        ));
    };
    let (host_port, base_path) = rest.split_once('/').unwrap_or((rest, ""));
    let default_port = if tls { 443 } else { DEFAULT_PORT };
    let (host, port) = host_port
        .rsplit_once(':')
        .map(|(host, port)| {
            let port = port
                .parse::<u16>()
                .map_err(|_| format!("invalid Neoism Agent port '{port}'"))?;
            Ok::<_, String>((host.to_string(), port))
        })
        .transpose()?
        .unwrap_or_else(|| (host_port.to_string(), default_port));
    if host.is_empty() {
        return Err("Neoism Agent server host is empty".to_string());
    }
    Ok((tls, host, port, base_path.trim_end_matches('/').to_string()))
}

fn request_path(base_path: &str, path: &str) -> String {
    if base_path.is_empty() {
        return path.to_string();
    }
    format!(
        "/{}/{}",
        base_path.trim_matches('/'),
        path.trim_start_matches('/')
    )
}
