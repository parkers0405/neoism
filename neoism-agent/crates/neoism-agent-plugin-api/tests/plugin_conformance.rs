use std::collections::BTreeMap;
use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use neoism_agent_plugin_api::{
    AgentPlugin, PluginHost, PluginHostError, PluginManifest, PluginRegistrar,
    PluginToolDefinition, PluginToolInvocation, PluginToolResult, RuntimeHook,
    RuntimeTool,
};
use serde_json::{json, Value};

const ID: &str = "dev.neoism.conformance-fixture";

struct ConformingPlugin;

impl AgentPlugin for ConformingPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.to_string(),
            name: "Conformance fixture".to_string(),
            version: "1.0.0".to_string(),
            internal: false,
            disableable: true,
            capabilities: vec!["fixture.echo".to_string()],
            requires: Vec::new(),
            event_namespaces: vec!["fixture".to_string()],
            api_prefix: Some(format!("/v2/plugins/{ID}")),
            config: BTreeMap::new(),
        }
    }

    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
        registrar.route("fixture");
        registrar.event("fixture.changed", Some(json!({ "type": "object" })));
        registrar.runtime_tool(Arc::new(EchoTool));
        registrar.runtime_hook(Arc::new(EchoHook));
        Ok(())
    }
}

struct EchoTool;

impl RuntimeTool for EchoTool {
    fn definition(&self) -> PluginToolDefinition {
        PluginToolDefinition {
            id: "fixture_echo".to_string(),
            description: "Echo conformance input".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "value": { "type": "string" } },
                "required": ["value"],
                "additionalProperties": false
            }),
            output_schema: json!({ "type": "object" }),
            permission: None,
        }
    }

    fn execute<'a>(
        &'a self,
        invocation: PluginToolInvocation,
    ) -> neoism_agent_plugin_api::PluginFuture<'a, PluginToolResult> {
        Box::pin(async move {
            Ok(PluginToolResult {
                title: "Echo".to_string(),
                output: invocation.arguments["value"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                metadata: Some(json!({ "directory": invocation.directory })),
            })
        })
    }
}

struct EchoHook;

impl RuntimeHook for EchoHook {
    fn invoke(
        &self,
        hook: &str,
        _context: Value,
        value: Value,
    ) -> Result<Value, neoism_agent_plugin_api::PluginRuntimeError> {
        assert_eq!(hook, "fixture.echo");
        Ok(value)
    }
}

/// Minimal reusable harness for native plugins. It exercises only public
/// plugin-api contracts, so internal and third-party plugins can copy the same
/// checks without linking the server.
fn assert_plugin_conforms(plugin: Box<dyn AgentPlugin>) {
    let host = PluginHost::default();
    let snapshot = host.install(vec![plugin], &[]).expect("plugin installs");
    let manifest = snapshot
        .manifests
        .iter()
        .find(|item| item.id == ID)
        .expect("manifest is discoverable");
    assert!(manifest.enabled && manifest.active && manifest.disableable);
    assert_eq!(
        manifest.api_prefix.as_deref(),
        Some("/v2/plugins/dev.neoism.conformance-fixture")
    );
    assert_eq!(snapshot.contributions["Route:fixture"].plugin_id, ID);
    assert_eq!(
        snapshot.contributions["Event:fixture.changed"].plugin_id,
        ID
    );
    assert_eq!(snapshot.contributions["Tool:fixture_echo"].plugin_id, ID);

    let tool = &snapshot.runtime_tools["fixture_echo"];
    let result = block_on(tool.execute(PluginToolInvocation {
        directory: "/workspace".to_string(),
        session_id: Some("session-test".to_string()),
        arguments: json!({ "value": "hello" }),
        permission_rules: Vec::new(),
        env: BTreeMap::new(),
        cancel: None,
        formatter: None,
        generation: None,
    }))
    .expect("runtime tool executes through public DTOs");
    assert_eq!(result.output, "hello");
    assert_eq!(result.metadata.unwrap()["directory"], "/workspace");

    let hook = snapshot
        .runtime_hooks
        .first()
        .expect("runtime hook registered");
    assert_eq!(hook.plugin_id, ID);
    assert_eq!(
        hook.invoke("fixture.echo", Value::Null, json!({ "ok": true }))
            .unwrap(),
        json!({ "ok": true })
    );
    assert!(hook.lifecycle().active);

    let disabled = host
        .install(vec![Box::new(ConformingPlugin)], &[ID.to_string()])
        .expect("disableable plugin can be disabled");
    assert!(
        disabled.manifests.is_empty(),
        "disabled plugins are structurally absent"
    );
    assert!(disabled.contributions.is_empty());
    assert!(disabled.runtime_tools.is_empty());
    assert!(disabled.runtime_hooks.is_empty());
}

#[test]
fn native_plugin_conforms_without_server_dependencies() {
    assert_plugin_conforms(Box::new(ConformingPlugin));
}

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => {
            panic!("conformance fixture futures must complete without an async runtime")
        }
    }
}
