use super::*;
use std::sync::Mutex;

struct CatalogClient {
    capabilities: Value,
    methods: Mutex<Vec<String>>,
    initialized: AtomicBool,
    barrier: Option<tokio::sync::Barrier>,
    fail: Option<&'static str>,
}

impl CatalogClient {
    fn new(capabilities: Value) -> Self {
        Self {
            capabilities,
            methods: Mutex::new(Vec::new()),
            initialized: AtomicBool::new(false),
            barrier: None,
            fail: None,
        }
    }
}

impl JsonRpcClient for CatalogClient {
    async fn request(&self, method: &str, _: Value) -> anyhow::Result<Value> {
        self.methods.lock().unwrap().push(method.to_string());
        if method == "initialize" {
            return Ok(json!({"capabilities": self.capabilities}));
        }
        assert!(self.initialized.load(Ordering::SeqCst));
        if let Some(barrier) = &self.barrier {
            barrier.wait().await;
        }
        if self.fail == Some(method) {
            return Err(anyhow!("catalog unavailable"));
        }
        Ok(match method {
            "tools/list" => {
                json!({"tools": [{"name":"read", "description":"Read", "inputSchema":{"type":"object"}}]})
            }
            "resources/list" => {
                json!({"resources": [{"name":"reference", "uri":"fixture://reference"}]})
            }
            "prompts/list" => {
                json!({"prompts": [{"name":"summary", "description":"Summarize"}]})
            }
            _ => panic!("unexpected request: {method}"),
        })
    }

    async fn notify(&self, method: &str, _: Value) -> anyhow::Result<()> {
        assert_eq!(method, "notifications/initialized");
        self.methods.lock().unwrap().push(method.to_string());
        self.initialized.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn discovery_and_refresh_only_request_advertised_catalogs() {
    for (capability, method) in [
        ("tools", "tools/list"),
        ("resources", "resources/list"),
        ("prompts", "prompts/list"),
    ] {
        let client = CatalogClient::new(json!({capability: {}}));
        let (capabilities, snapshot) =
            load_snapshot("fixture", &client, true).await.unwrap();
        assert_eq!(snapshot.0.len(), usize::from(capability == "tools"));
        assert_eq!(snapshot.1.len(), usize::from(capability == "resources"));
        assert_eq!(snapshot.2.len(), usize::from(capability == "prompts"));
        assert_eq!(
            *client.methods.lock().unwrap(),
            ["initialize", "notifications/initialized", method]
        );
        client.methods.lock().unwrap().clear();
        read_snapshot("fixture", &client, capabilities)
            .await
            .unwrap();
        assert_eq!(*client.methods.lock().unwrap(), [method]);
    }
    for capabilities in [
        json!({}),
        json!({"tools":null,"resources":false,"prompts":[]}),
    ] {
        let client = CatalogClient::new(capabilities);
        let (_, snapshot) = load_snapshot("fixture", &client, false).await.unwrap();
        assert!(snapshot.0.is_empty() && snapshot.1.is_empty() && snapshot.2.is_empty());
        assert_eq!(
            *client.methods.lock().unwrap(),
            ["initialize", "notifications/initialized"]
        );
    }
}

#[tokio::test]
async fn advertised_catalogs_are_read_concurrently_after_initialization() {
    let mut client = CatalogClient::new(json!({"tools":{},"resources":{},"prompts":{}}));
    // Serial discovery cannot cross this barrier. No provider, process, or clock
    // performance assumption is needed to prove the three requests overlap.
    client.barrier = Some(tokio::sync::Barrier::new(3));
    let (_, snapshot) = tokio::time::timeout(
        Duration::from_secs(2),
        load_snapshot("fixture", &client, true),
    )
    .await
    .expect("catalog requests were serialized")
    .unwrap();
    assert_eq!(
        (snapshot.0.len(), snapshot.1.len(), snapshot.2.len()),
        (1, 1, 1)
    );
}

#[tokio::test]
async fn required_tools_fail_closed_but_optional_catalog_errors_remain_tolerated() {
    for method in ["tools/list", "resources/list", "prompts/list"] {
        let mut client =
            CatalogClient::new(json!({"tools":{},"resources":{},"prompts":{}}));
        client.fail = Some(method);
        let result = load_snapshot("fixture", &client, true).await;
        if method == "tools/list" {
            assert!(result.is_err());
        } else {
            let (_, snapshot) = result.unwrap();
            assert_eq!(snapshot.0.len(), 1);
            assert_eq!(snapshot.1.len(), usize::from(method != "resources/list"));
            assert_eq!(snapshot.2.len(), usize::from(method != "prompts/list"));
        }
    }
}
