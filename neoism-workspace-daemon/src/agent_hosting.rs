//! Called only for operator-supplied `--workspace` roots, before accepting joins.
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
        .timeout(std::time::Duration::from_secs(3))
        .build()?;
    let mut last_error = String::new();
    for _ in 0..30 {
        let credential = crate::server::mint_agent_credential(
            "neoism:host-chat-association".into(),
            workspace,
            &root,
        )
        .map_err(|_| anyhow::anyhow!("cannot sign chat hosting association"))?;
        let result = client
            .post(format!("{base}/v2/hosting/associate"))
            .bearer_auth(credential)
            .header("x-neoism-directory", root.to_string_lossy().as_ref())
            .send()
            .await;
        match result {
            Ok(response) if response.status().is_success() => {
                let body: serde_json::Value = response.json().await?;
                let namespace = body
                    .get("workspaceId")
                    .and_then(|v| v.as_str())
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Agent returned no hosting namespace")
                    })?;
                NAMESPACES
                    .get_or_init(Default::default)
                    .lock()
                    .unwrap()
                    .insert(workspace.into(), (root, namespace.into()));
                return Ok(());
            }
            Ok(response) => {
                let status = response.status();
                last_error = response.text().await.unwrap_or_else(|_| status.to_string());
                if status.is_client_error() {
                    anyhow::bail!(
                        "chat hosting association rejected ({status}): {last_error}"
                    );
                }
            }
            Err(error) => last_error = error.to_string(),
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    anyhow::bail!("chat hosting association failed: {last_error}")
}
