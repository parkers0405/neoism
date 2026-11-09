//! Associates workspace roots with their local Agent chat namespace.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

static NAMESPACES: OnceLock<Mutex<HashMap<String, (PathBuf, String)>>> = OnceLock::new();

pub(crate) fn namespace(workspace: &str, root: &Path) -> Option<String> {
    NAMESPACES
        .get()?
        .lock()
        .ok()?
        .get(workspace)
        .filter(|(bound_root, _)| bound_root == root)
        .map(|(_, id)| id.clone())
}

pub async fn associate(workspace: &str, root: &Path) -> anyhow::Result<()> {
    let root = crate::path::canonicalize(root)?;
    let base = crate::agent::configured_agent_server();
    // Do not send the local operator capability to a configured remote service.
    let url = reqwest::Url::parse(&base)?;
    anyhow::ensure!(
        url.scheme() == "http"
            && matches!(
                url.host_str(),
                Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
            ),
        "local chat hosting requires a loopback Agent server"
    );
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_proxy()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    // Retry only the read-only readiness probe, using elapsed time rather than
    // an attempt count (Windows can refuse connections immediately while cold).
    crate::agent::wait_for_agent_health(
        &client,
        &base,
        crate::agent::AGENT_SERVER_READY_TIMEOUT,
    )
    .await?;
    let credential = crate::server::mint_agent_credential(
        "neoism:host-chat-association".into(),
        workspace,
        &root,
    )
    .map_err(|_| anyhow::anyhow!("cannot sign chat hosting association"))?;
    let namespace = send_association(&client, &base, &credential, &root).await?;
    NAMESPACES
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(workspace.into(), (root, namespace));
    Ok(())
}

async fn send_association(
    client: &reqwest::Client,
    base: &str,
    credential: &str,
    root: &Path,
) -> anyhow::Result<String> {
    let result = client
        .post(format!("{base}/v2/hosting/associate"))
        .bearer_auth(credential)
        .header("x-neoism-directory", root.to_string_lossy().as_ref())
        .send()
        .await;
    // Once sent, association may have committed even if its response was
    // lost or timed out. Never replay this mutation automatically.
    match result {
        Ok(response) if response.status().is_success() => {
            let body: serde_json::Value = response.json().await?;
            let namespace = body
                .get("workspaceId")
                .and_then(|v| v.as_str())
                .filter(|id| !id.is_empty())
                .ok_or_else(|| anyhow::anyhow!("Agent returned no hosting namespace"))?;
            Ok(namespace.into())
        }
        Ok(response) => {
            let status = response.status();
            anyhow::bail!("chat hosting association rejected ({status})");
        }
        Err(error) => anyhow::bail!(
            "chat hosting association failed (not replayed): {}",
            error.without_url()
        ),
    }
}

#[cfg(test)]
mod startup_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn association_does_not_replay_server_error_or_lost_response() {
        for reply in [Some("HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"), None] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = [0; 4096];
                let n = stream.read(&mut bytes).await.unwrap();
                assert!(String::from_utf8_lossy(&bytes[..n]).starts_with("POST /v2/hosting/associate "));
                if let Some(reply) = reply {
                    stream.write_all(reply.as_bytes()).await.unwrap();
                }
                drop(stream);
                assert!(tokio::time::timeout(std::time::Duration::from_millis(350), listener.accept()).await.is_err());
            });
            let client = reqwest::Client::builder().no_proxy().retry(reqwest::retry::never()).build().unwrap();
            assert!(send_association(&client, &base, "test", Path::new(".")).await.is_err());
            server.await.unwrap();
        }
    }
}
