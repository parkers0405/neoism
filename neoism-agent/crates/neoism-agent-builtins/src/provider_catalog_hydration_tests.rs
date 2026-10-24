use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const FIXTURE: &str = r#"{"openai":{"id":"openai","name":"OpenAI","npm":"@ai-sdk/openai","api":"https://api.openai.com/v1","models":{"gpt-5":{"id":"gpt-5","name":"GPT-5","release_date":"2025-08-07","limit":{"context":1050000,"output":128000},"cost":{"input":5,"output":30}}}}}"#;

async fn status(
    catalog: &ProviderCatalog,
    access: &OpenAiModelAccess,
    oauth: (&str, u64),
) -> Value {
    catalog.catalog_status("fixture-scope", access, oauth).await
}

#[test]
fn bundled_snapshot_is_full_public_metadata_not_an_entitlement_list() {
    use sha2::{Digest, Sha256};
    assert_eq!(
        format!("{:x}", Sha256::digest(include_bytes!("provider_catalog_fallback.json"))),
        "09659a0eeb6685e6554b93a4897b7dede514b52fd6f5d9731dc2e241359074d7",
        "update the versioned snapshot provenance when replacing the catalog",
    );
    let providers = parse_models(include_str!("provider_catalog_fallback.json")).unwrap();
    assert!(providers.len() > 20);
    let openai = providers.iter().find(|p| p.id == "openai").unwrap();
    assert!(openai.models.len() > 10);
    assert!(openai
        .models
        .values()
        .any(|m| m.api.id != "gpt-5" && m.limit.context > 0));
    assert!(providers
        .iter()
        .any(|p| p.id == "anthropic" && p.models.len() > 5));
}

#[test]
fn absent_account_wire_ids_materialize_without_api_or_other_account_leakage() {
    let raw = parse_models(FIXTURE).unwrap();
    let access = OpenAiModelAccess::Codex(BTreeMap::from([(
        "account-only-future".into(),
        CodexModelMetadata {
            context_window: Some(300_000),
            ..Default::default()
        },
    )]));
    let mut configured = BTreeMap::new();
    configured.insert(
        "openai".into(),
        ProviderConfig {
            models: BTreeMap::from([(
                "account-only-future".into(),
                ProviderModelConfig {
                    tool_call: Some(true),
                    limit: Some(ModelLimit {
                        context: 200_000,
                        input: Some(180_000),
                        output: 4096,
                    }),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        },
    );
    let resolved = effective_catalog_with_config(&raw, &access, &configured, true);
    let account = resolved.iter().find(|p| p.id == "openai").unwrap();
    assert_eq!(account.models.len(), 1);
    let model = &account.models["account-only-future"];
    assert_eq!(model.api.id, "account-only-future");
    assert_eq!(model.limit.context, 200_000);
    assert_eq!(model.limit.input, Some(180_000));
    assert_eq!(model.limit.output, 4096);
    assert_eq!(model.cost.input, 0.0);
    assert!(model.capabilities.tool_call);
    let user_model = UserModel {
        provider_id: "openai".into(),
        model_id: model.id.clone(),
        connection_id: None,
        variant: None,
    };
    assert_eq!(
        generation_metadata(&resolved, &user_model)
            .limit
            .unwrap()
            .context,
        200_000
    );
    assert_eq!(
        usable_provider_catalog(&resolved, &["openai".into()], &OpenAiModelAccess::Api)
            [0]
        .models
        .len(),
        1
    );
    let api = effective_provider_catalog(&raw, &OpenAiModelAccess::Api);
    let api = api.iter().find(|p| p.id == "openai").unwrap();
    assert!(!api.models.contains_key("account-only-future"));
    assert_eq!(api.models["gpt-5"].limit.context, 1_050_000);
    assert_eq!(api.models["gpt-5"].cost.input, 5.0);
    let other =
        effective_provider_catalog(&raw, &OpenAiModelAccess::Codex(BTreeMap::new()));
    assert!(other
        .iter()
        .find(|p| p.id == "openai")
        .unwrap()
        .models
        .is_empty());
    let unknown = effective_provider_catalog(&[], &access);
    let unknown = &unknown[0].models["account-only-future"];
    assert_eq!(unknown.limit.output, 0);
    assert!(!unknown.capabilities.attachment);
    assert!(!unknown.capabilities.reasoning);
    assert_eq!(unknown.limit.input, Some(285_000));
}

#[tokio::test]
async fn hung_source_recovers_on_warm_reads_singleflight_even_with_unwritable_cache() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let source = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let recovered = Arc::new(AtomicBool::new(false));
    let (h, r) = (hits.clone(), recovered.clone());
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (h, r) = (h.clone(), r.clone());
            tokio::spawn(async move {
                let mut request = [0; 2048];
                let _ = socket.read(&mut request).await;
                h.fetch_add(1, Ordering::SeqCst);
                if !r.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_secs(11)).await;
                    return;
                }
                let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", FIXTURE.len(), FIXTURE);
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    let root =
        std::env::temp_dir().join(format!("neoism-hydration-{}", rand::random::<u64>()));
    // A regular file used as a parent fails reliably even when tests run as root.
    tokio::fs::write(&root, "not a directory").await.unwrap();
    let catalog = ProviderCatalog::with_test_source(source, root.join("models.json"));
    tokio::time::timeout(Duration::from_millis(500), catalog.providers())
        .await
        .unwrap()
        .unwrap();
    for _ in 0..20 {
        tokio::time::timeout(Duration::from_millis(500), catalog.providers())
            .await
            .unwrap()
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            if status(&catalog, &OpenAiModelAccess::Api, ("ready", 0)).await["state"]
                == "retryable-error"
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let failed = status(&catalog, &OpenAiModelAccess::Api, ("ready", 0)).await;
    assert_eq!(failed["source"], "bundled");
    assert!(failed["retryAfterMs"].as_u64().unwrap() > 0);
    for _ in 0..4 {
        catalog.providers().await.unwrap();
    }
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    recovered.store(true, Ordering::SeqCst);
    catalog.hydration.write().await.next = std::time::Instant::now();
    catalog.providers().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if status(&catalog, &OpenAiModelAccess::Api, ("ready", 0)).await["source"]
                == "network"
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let ready = status(&catalog, &OpenAiModelAccess::Api, ("ready", 0)).await;
    assert_eq!(ready["state"], "ready");
    assert!(ready["revision"].as_u64().unwrap() > failed["revision"].as_u64().unwrap());
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert_eq!(
        catalog
            .providers()
            .await
            .unwrap()
            .iter()
            .find(|p| p.id == "openai")
            .unwrap()
            .models
            .len(),
        1
    );
    // A subsequent failed forced refresh must retain the good network result.
    server.abort();
    assert!(catalog.refresh(true).await.is_err());
    assert_eq!(
        catalog
            .providers()
            .await
            .unwrap()
            .iter()
            .find(|p| p.id == "openai")
            .unwrap()
            .models
            .len(),
        1
    );
    let failed_again = status(&catalog, &OpenAiModelAccess::Api, ("ready", 0)).await;
    assert_eq!(failed_again["source"], "network");
    assert_eq!(failed_again["state"], "retryable-error");
    tokio::fs::remove_file(root).await.unwrap();
}

#[tokio::test]
async fn hydration_empty_is_ready_failure_is_retryable_and_pending_is_not_negative() {
    let catalog =
        ProviderCatalog::with_test_source("http://127.0.0.1:1".into(), PathBuf::new());
    *catalog.cached.write().await = Some(parse_models(FIXTURE).unwrap());
    {
        let mut base = catalog.hydration.write().await;
        base.source = "disk";
        base.state = "ready";
    }
    let empty = OpenAiModelAccess::Codex(BTreeMap::new());
    let ready = status(&catalog, &empty, ("ready", 0)).await;
    assert_eq!(ready["state"], "ready");
    let failed = status(
        &catalog,
        &OpenAiModelAccess::CodexPending,
        ("retryable-error", 1234),
    )
    .await;
    assert_eq!(failed["state"], "retryable-error");
    assert_eq!(failed["retryAfterMs"], 1234);
    let pending =
        status(&catalog, &OpenAiModelAccess::CodexPending, ("loading", 500)).await;
    assert_eq!(pending["state"], "loading");
    assert!(pending["revision"].as_u64().unwrap() > failed["revision"].as_u64().unwrap());
    assert_eq!(
        status(&catalog, &empty, ("ready", 0)).await["state"],
        "ready"
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let fixture = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 2048];
        let _ = socket.read(&mut request).await;
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"data\":[]}").await.unwrap();
    });
    let mut configured = BTreeMap::new();
    configured.insert(
        "fixture-local".into(),
        ProviderConfig {
            discover_models: true,
            auth: Some(ProviderAuthMode::None),
            options: neoism_agent_core::ProviderConfigOptions {
                base_url: Some(url.clone()),
            },
            ..Default::default()
        },
    );
    let mut local = catalog.clone();
    local.configured = Arc::new(configured);
    assert_eq!(
        status(&local, &empty, ("ready", 0)).await["state"],
        "loading"
    );
    assert!(local
        .discover_model_ids(&url, ProviderAuthMode::None)
        .await
        .is_empty());
    fixture.await.unwrap();
    assert_eq!(status(&local, &empty, ("ready", 0)).await["state"], "ready");
    local.discovered.write().await.clear();
    assert!(local
        .discover_model_ids(&url, ProviderAuthMode::None)
        .await
        .is_empty());
    assert_eq!(
        status(&local, &empty, ("ready", 0)).await["state"],
        "retryable-error"
    );
}
