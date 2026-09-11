use std::time::Duration;

use anyhow::Context;
use neoism_agent_server::{gui::GuiRoot, ServerOptions};

fn server_url(server: &str) -> anyhow::Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(server).context("invalid server URL")?;
    anyhow::ensure!(matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
        && url.username().is_empty() && url.password().is_none()
        && url.query().is_none() && url.fragment().is_none()
        && matches!(url.path(), "" | "/"),
        "--server must be an HTTP(S) origin without credentials, path, query, or fragment");
    url.set_path("/");
    Ok(url)
}

// Only connection refusal permits starting a server. A timeout, foreign HTTP
// service or unhealthy agent is not evidence that the endpoint is unoccupied.
async fn health(client: &reqwest::Client, url: &reqwest::Url) -> anyhow::Result<bool> {
    let response = match client.get(url.join("v2/health")?).send().await {
        Ok(response) => response,
        Err(error) if refused(&error) => return Ok(false),
        Err(error) => {
            return Err(error).context("could not probe agent health; no server started")
        }
    };
    anyhow::ensure!(
        response.status().is_success(),
        "existing endpoint rejected /v2/health ({}); no server started",
        response.status()
    );
    let value: serde_json::Value = response
        .json()
        .await
        .context("existing endpoint is not an agent health API")?;
    anyhow::ensure!(
        value["healthy"] == true && value["version"].is_string(),
        "existing endpoint is not a healthy Neoism agent; no server started"
    );
    Ok(true)
}

fn refused(error: &reqwest::Error) -> bool {
    use std::error::Error;
    let mut source = error.source();
    while let Some(error) = source {
        if let Some(io) = error.downcast_ref::<std::io::Error>() {
            if io.kind() == std::io::ErrorKind::ConnectionRefused {
                return true;
            }
        }
        source = error.source();
    }
    false
}

async fn verify_gui(client: &reqwest::Client, url: &reqwest::Url) -> anyhow::Result<()> {
    let response = client
        .head(url.clone())
        .send()
        .await
        .context("could not probe GUI")?;
    anyhow::ensure!(response.status().is_success()
        && response.headers().get("x-neoism-agent-gui").and_then(|h| h.to_str().ok()) == Some("1")
        && response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|h| h.to_str().ok()).is_some_and(|s| s.starts_with("text/html")),
        "agent is running at {url}, but it does not serve the standalone GUI. Restart that server with `neoism-agent serve --web` and a built GUI dist. No duplicate server was started");
    Ok(())
}

fn report(url: &reqwest::Url, no_open: bool) {
    println!("Neoism agent GUI: {url}");
    if no_open {
        return;
    }
    // No shell interpolation or tokens in the URL. Missing desktop launchers
    // are non-fatal: the verified URL remains usable manually.
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url.as_str()).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("rundll32.exe")
        .arg("url.dll,FileProtocolHandler")
        .arg(url.as_str())
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let result = std::process::Command::new("xdg-open")
        .arg(url.as_str())
        .spawn();
    match result {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(error) => eprintln!(
            "Browser launcher unavailable ({error}); open the URL above manually."
        ),
    }
}

pub(crate) async fn run(
    server: Option<String>,
    hostname: String,
    port: u16,
    no_open: bool,
) -> anyhow::Result<()> {
    let explicit_server = server.is_some();
    let url = server_url(&server.unwrap_or_else(|| {
        let host = if hostname.contains(':') {
            format!("[{hostname}]")
        } else {
            hostname.clone()
        };
        format!("http://{host}:{port}")
    }))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()?;
    if health(&client, &url).await? {
        verify_gui(&client, &url).await?;
        report(&url, no_open);
        return Ok(());
    }
    anyhow::ensure!(!explicit_server, "no agent is listening at {url}; --server attaches only. Start it with `neoism-agent serve --web`");
    anyhow::ensure!(port != 0, "web requires a nonzero --port");
    let ip: std::net::IpAddr = hostname
        .parse()
        .context("--hostname must be a loopback IP address")?;
    anyhow::ensure!(ip.is_loopback(), "web auto-start is loopback-only; use serve --web for remote hosting, then web --server to attach");
    let root = GuiRoot::discover()?;
    let serving = neoism_agent_server::listen_with_gui(
        ServerOptions {
            hostname,
            port,
            cors: Vec::new(),
        },
        crate::standalone_services(),
        Some(root),
    );
    tokio::pin!(serving);
    let ready = async {
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            // Binding precedes state initialization, so our own queued probe
            // may time out. The outer deadline bounds these startup retries.
            if matches!(health(&client, &url).await, Ok(true)) {
                verify_gui(&client, &url).await?;
                return Ok::<_, anyhow::Error>(());
            }
        }
        anyhow::bail!("agent startup timed out; GUI was not opened")
    };
    tokio::select! {
        result = &mut serving => { result?; anyhow::bail!("agent exited before GUI became ready"); }
        result = tokio::time::timeout(Duration::from_secs(30), ready) => {
            result.context("agent startup timed out; GUI was not opened")??;
        }
    }
    report(&url, no_open);
    println!("Serving in foreground; Ctrl-C to stop.");
    serving.await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn attach_requires_both_agent_health_and_gui_marker() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        async fn endpoint(
            body: &str,
            headers: &str,
        ) -> (reqwest::Url, tokio::task::JoinHandle<()>) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = server_url(&format!("http://{}", listener.local_addr().unwrap()))
                .unwrap();
            let response = format!("HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}", body.len());
            let task = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                stream.read(&mut request).await.unwrap();
                stream.write_all(response.as_bytes()).await.unwrap();
            });
            (url, task)
        }
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let (url, task) = endpoint(
            r#"{"healthy":true,"version":"test"}"#,
            "Content-Type: application/json\r\n",
        )
        .await;
        assert!(health(&client, &url).await.unwrap());
        task.await.unwrap();
        let (url, task) =
            endpoint("foreign service", "Content-Type: text/plain\r\n").await;
        assert!(health(&client, &url).await.is_err());
        task.await.unwrap();
        let (url, task) = endpoint("", "Content-Type: text/html\r\n").await;
        assert!(verify_gui(&client, &url).await.is_err());
        task.await.unwrap();
        let (url, task) =
            endpoint("", "Content-Type: text/html\r\nX-Neoism-Agent-Gui: 1\r\n").await;
        verify_gui(&client, &url).await.unwrap();
        task.await.unwrap();
    }

    #[test]
    fn origin_only_no_secret_urls() {
        assert_eq!(
            server_url("http://127.0.0.1:4096").unwrap().as_str(),
            "http://127.0.0.1:4096/"
        );
        for url in [
            "file:///tmp/gui",
            "http://token@localhost",
            "http://localhost/?token=secret",
            "http://localhost/#secret",
            "http://localhost/v2",
        ] {
            assert!(server_url(url).is_err());
        }
    }
}
