use std::time::Duration;

use futures::{stream, StreamExt};
use neoism_agent_core::AuthInfo;
use neoism_agent_service_api::ProviderConnectionSummary;
use serde::Serialize;
use serde_json::Value;

use super::provider_openai::openai_oauth_credentials;
use crate::auth_store::AuthStore;

#[derive(Debug, Serialize)]
struct UsageWindow {
    label: String,
    used_percent: f64,
    reset_at: Option<i64>,
    limit_window_seconds: Option<i64>,
}

#[derive(Debug, Serialize)]
struct AccountUsage {
    connection_id: String,
    label: String,
    is_default: bool,
    auth_type: String,
    plan_type: Option<String>,
    windows: Vec<UsageWindow>,
    error: Option<String>,
}

const MAX_USAGE_BODY_BYTES: usize = 256 * 1024;
const BODY_TOO_LARGE: &str = "OpenAI usage response exceeded the size limit";

pub(crate) async fn openai_usage(auth: &AuthStore) -> anyhow::Result<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(35);
    let connections = tokio::time::timeout_at(
        deadline,
        auth.service().list(Some("openai"), auth.scope()),
    )
    .await
    .map_err(|_| anyhow::anyhow!("OpenAI usage request timed out"))?
    .map_err(|_| anyhow::anyhow!("Unable to list OpenAI connections"))?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(10))
        .build()?;
    // Bound both per-account work and the aggregate lookup, including queued
    // accounts. Unordered completion avoids head-of-line blocking; restore
    // credential-store order after collecting every account's result.
    let mut accounts: Vec<_> =
        stream::iter(connections.into_iter().enumerate().map(|(index, summary)| {
            let client = &client;
            async move {
                let scoped = auth
                    .scoped(auth.scope().clone(), Some(summary.connection_id.clone()));
                let mut account = AccountUsage {
                    connection_id: summary.connection_id.clone(),
                    label: summary.label.clone(),
                    is_default: summary.is_default,
                    auth_type: summary.auth_type.clone(),
                    plan_type: None,
                    windows: Vec::new(),
                    error: None,
                };
                if summary.auth_type != "oauth" {
                    account.error =
                        Some("ChatGPT usage is unavailable for API accounts".into());
                } else {
                    match fetch_before_deadline(
                        deadline,
                        fetch_usage(client, &scoped, &summary),
                    )
                    .await
                    {
                        Ok((plan, windows)) => {
                            account.plan_type = plan;
                            account.windows = windows;
                        }
                        Err(error) => account.error = Some(error.into()),
                    }
                }
                (index, account)
            }
        }))
        .buffer_unordered(4)
        .collect()
        .await;
    accounts.sort_by_key(|(index, _)| *index);
    let accounts: Vec<_> = accounts.into_iter().map(|(_, account)| account).collect();
    Ok(serde_json::json!({ "accounts": accounts }))
}

async fn fetch_before_deadline<T>(
    deadline: tokio::time::Instant,
    fetch: impl std::future::Future<Output = Result<T, &'static str>>,
) -> Result<T, &'static str> {
    let now = tokio::time::Instant::now();
    // Do not start upstream work for accounts still queued at the deadline.
    if now >= deadline {
        return Err("OpenAI usage request timed out");
    }
    tokio::time::timeout_at(deadline.min(now + Duration::from_secs(22)), fetch)
        .await
        .map_err(|_| "OpenAI usage request timed out")?
}

async fn fetch_usage(
    client: &reqwest::Client,
    auth: &AuthStore,
    summary: &ProviderConnectionSummary,
) -> Result<(Option<String>, Vec<UsageWindow>), &'static str> {
    if summary.auth_type != "oauth" {
        return Err("ChatGPT usage is unavailable for API accounts");
    }
    let credential = auth
        .get("openai")
        .await
        .map_err(|_| "Unable to read OpenAI account credentials")?
        .ok_or("OpenAI account is no longer connected")?;
    if !matches!(credential, AuthInfo::OAuth { .. }) {
        return Err("ChatGPT usage is unavailable for this authentication type");
    }
    let (access, account_id) = openai_oauth_credentials(client, auth, credential)
        .await
        .map_err(|_| {
        "Unable to refresh OpenAI account; reconnect if this persists"
    })?;
    let account_id = account_id
        .filter(|id| !id.trim().is_empty())
        .ok_or("OpenAI account ID is unavailable; reconnect this account")?;
    let response = client
        .get("https://chatgpt.com/backend-api/wham/usage")
        .bearer_auth(access)
        .header("ChatGPT-Account-Id", account_id)
        .header("originator", "neoism")
        .header("accept", "application/json")
        .header(
            "user-agent",
            super::provider_openai_stream::neoism_user_agent(),
        )
        .send()
        .await
        .map_err(|_| "Unable to reach OpenAI usage service")?;
    if !response.status().is_success() {
        // Never forward upstream bodies, URLs, credentials, or raw errors.
        return Err("OpenAI usage service rejected the request");
    }
    let body = read_usage_body(response).await?;
    let value = serde_json::from_slice::<Value>(&body)
        .map_err(|_| "Invalid response from OpenAI usage service")?;
    if !value.is_object() {
        return Err("Invalid response from OpenAI usage service");
    }
    Ok(normalize_usage(&value))
}

fn check_usage_body_length(length: u64) -> Result<(), &'static str> {
    if length > MAX_USAGE_BODY_BYTES as u64 {
        return Err(BODY_TOO_LARGE);
    }
    Ok(())
}

fn append_usage_chunk(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), &'static str> {
    let length = body.len().checked_add(chunk.len()).ok_or(BODY_TOO_LARGE)?;
    check_usage_body_length(length as u64)?;
    body.extend_from_slice(chunk);
    Ok(())
}

async fn read_usage_body(
    mut response: reqwest::Response,
) -> Result<Vec<u8>, &'static str> {
    if let Some(length) = response.content_length() {
        check_usage_body_length(length)?;
    }
    let mut body = Vec::new();
    // The accumulated (decoded) size is authoritative even when the upstream
    // omits Content-Length, uses chunks, or has a misleading length header.
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Unable to read OpenAI usage response")?
    {
        append_usage_chunk(&mut body, &chunk)?;
    }
    Ok(body)
}

fn normalize_usage(value: &Value) -> (Option<String>, Vec<UsageWindow>) {
    let plan = value
        .get("plan_type")
        .and_then(Value::as_str)
        .filter(|plan| !plan.trim().is_empty())
        .map(str::to_owned);
    let mut windows = Vec::new();
    for (key, prefix) in [
        ("rate_limit", ""),
        ("code_review_rate_limit", "Code review · "),
    ] {
        for (slot, fallback) in [
            ("primary_window", "Primary"),
            ("secondary_window", "Secondary"),
        ] {
            let window = &value[key][slot];
            // Missing/null usage must not appear as an invented 0%-used bar.
            let Some(used) = window["used_percent"]
                .as_f64()
                .filter(|used| used.is_finite())
            else {
                continue;
            };
            let seconds = window["limit_window_seconds"]
                .as_i64()
                .filter(|seconds| *seconds > 0);
            let label = match seconds {
                Some(18_000) => "5-hour",
                Some(604_800) => "Weekly",
                _ => fallback,
            };
            windows.push(UsageWindow {
                label: format!("{prefix}{label}"),
                used_percent: used.clamp(0.0, 100.0),
                reset_at: window["reset_at"].as_i64().filter(|reset| *reset >= 0),
                limit_window_seconds: seconds,
            });
        }
    }
    (plan, windows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn advertised_body_length_is_bounded() {
        assert!(check_usage_body_length(0).is_ok());
        assert!(check_usage_body_length(MAX_USAGE_BODY_BYTES as u64).is_ok());
        assert_eq!(
            check_usage_body_length(MAX_USAGE_BODY_BYTES as u64 + 1),
            Err(BODY_TOO_LARGE)
        );
        assert_eq!(check_usage_body_length(u64::MAX), Err(BODY_TOO_LARGE));
    }

    #[test]
    fn chunked_body_limit_is_cumulative_and_does_not_append_overflow() {
        let mut body = Vec::new();
        let half = vec![b'x'; MAX_USAGE_BODY_BYTES / 2];
        append_usage_chunk(&mut body, &half).unwrap();
        append_usage_chunk(&mut body, &half).unwrap();
        assert_eq!(body.len(), MAX_USAGE_BODY_BYTES);
        assert_eq!(
            append_usage_chunk(&mut body, b"secret-upstream-content"),
            Err(BODY_TOO_LARGE)
        );
        assert_eq!(body.len(), MAX_USAGE_BODY_BYTES);
        assert!(!String::from_utf8_lossy(&body).contains("secret"));
        let mut empty = Vec::new();
        assert_eq!(
            append_usage_chunk(&mut empty, &vec![0; MAX_USAGE_BODY_BYTES + 1]),
            Err(BODY_TOO_LARGE)
        );
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn expired_aggregate_deadline_does_not_start_queued_fetch() {
        let started = std::cell::Cell::new(false);
        let result = fetch_before_deadline(tokio::time::Instant::now(), async {
            started.set(true);
            Ok(())
        })
        .await;
        assert_eq!(result, Err("OpenAI usage request timed out"));
        assert!(!started.get());
    }

    #[tokio::test]
    async fn aggregate_deadline_keeps_all_results_across_multiple_batches() {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(10);
        let mut results: Vec<_> = stream::iter((0..12).map(|index| async move {
            let result = fetch_before_deadline(
                deadline,
                std::future::pending::<Result<(), &'static str>>(),
            )
            .await;
            (index, result)
        }))
        .buffer_unordered(4)
        .collect()
        .await;
        results.sort_by_key(|(index, _)| *index);
        assert_eq!(results.len(), 12);
        for (index, (returned_index, result)) in results.into_iter().enumerate() {
            assert_eq!(returned_index, index);
            assert_eq!(result, Err("OpenAI usage request timed out"));
        }
    }

    #[tokio::test]
    async fn stale_refresh_snapshot_re_reads_the_exact_connection() {
        use neoism_agent_service_api::{
            CreateProviderConnection, CredentialScope, LocalProviderCredentialStore,
            ProviderCredential, ProviderCredentialStore,
        };
        use std::sync::Arc;
        let path = std::env::temp_dir().join(format!(
            "neoism-usage-refresh-{}.json",
            rand::random::<u64>()
        ));
        let store = Arc::new(LocalProviderCredentialStore::new(path.clone()));
        let scope = CredentialScope::local();
        let fresh = AuthInfo::OAuth {
            refresh: "rotated-refresh".into(),
            access: "fresh-access".into(),
            expires: u64::MAX,
            account_id: Some("exact-account".into()),
            enterprise_url: None,
        };
        let stale = AuthInfo::OAuth {
            refresh: "stale-refresh".into(),
            access: "expired-access".into(),
            expires: 1,
            account_id: Some("exact-account".into()),
            enterprise_url: None,
        };
        let summary = store
            .create(CreateProviderConnection {
                provider_id: "openai".into(),
                label: "Refreshed account".into(),
                scope: scope.clone(),
                credential: ProviderCredential::OAuth {
                    refresh: "rotated-refresh".into(),
                    access: "fresh-access".into(),
                    expires: u64::MAX,
                    account_id: Some("exact-account".into()),
                    enterprise_url: None,
                },
                set_default: false,
            })
            .await
            .unwrap();
        store
            .create(CreateProviderConnection {
                provider_id: "openai".into(),
                label: "Different default".into(),
                scope: scope.clone(),
                credential: ProviderCredential::Api {
                    key: "default-api-secret".into(),
                    metadata: None,
                },
                set_default: true,
            })
            .await
            .unwrap();
        let auth =
            AuthStore::from_service(store).scoped(scope, Some(summary.connection_id));
        let result = openai_oauth_credentials(&reqwest::Client::new(), &auth, stale)
            .await
            .unwrap();
        assert_eq!(
            result,
            ("fresh-access".into(), Some("exact-account".into()))
        );
        assert_eq!(
            serde_json::to_value(auth.get("openai").await.unwrap().unwrap()).unwrap(),
            serde_json::to_value(fresh).unwrap(),
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn missing_and_null_fields_do_not_invent_usage() {
        for value in [
            json!({}),
            json!({"plan_type": null, "rate_limit": null}),
            json!({"rate_limit": {"primary_window": {"used_percent": null}, "secondary_window": {}}}),
        ] {
            let (plan, windows) = normalize_usage(&value);
            assert!(plan.is_none());
            assert!(windows.is_empty());
        }
        let (_, windows) = normalize_usage(
            &json!({"rate_limit": {"primary_window": {"used_percent": 0}}}),
        );
        assert_eq!(windows[0].used_percent, 0.0);
        assert!(windows[0].reset_at.is_none());
        assert!(windows[0].limit_window_seconds.is_none());
    }

    #[test]
    fn clamps_usage_and_labels_known_windows_by_duration() {
        let (plan, windows) = normalize_usage(&json!({
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": {"used_percent": -5, "limit_window_seconds": 18000, "reset_at": 1234},
                "secondary_window": {"used_percent": 150.5, "limit_window_seconds": 604800, "reset_at": null}
            }
        }));
        assert_eq!(plan.as_deref(), Some("plus"));
        assert_eq!(windows[0].label, "5-hour");
        assert_eq!(windows[0].used_percent, 0.0);
        assert_eq!(windows[0].reset_at, Some(1234));
        assert_eq!(windows[1].label, "Weekly");
        assert_eq!(windows[1].used_percent, 100.0);
        assert!(windows[1].reset_at.is_none());
    }

    #[test]
    fn unknown_durations_and_code_review_have_honest_labels() {
        let (plan, windows) = normalize_usage(&json!({
            "plan_type": "",
            "rate_limit": {
                "primary_window": {"used_percent": 12.5, "limit_window_seconds": 3600, "reset_at": -1},
                "secondary_window": {"used_percent": "30", "limit_window_seconds": 604800}
            },
            "code_review_rate_limit": {"primary_window": {"used_percent": 42, "limit_window_seconds": 0}}
        }));
        assert!(plan.is_none());
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].label, "Primary");
        assert_eq!(windows[0].used_percent, 12.5);
        assert_eq!(windows[0].limit_window_seconds, Some(3600));
        assert!(windows[0].reset_at.is_none());
        assert_eq!(windows[1].label, "Code review · Primary");
        assert!(windows[1].limit_window_seconds.is_none());
    }
}
