use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{ContributionMetadata, PluginContributions, PluginDefinition, PluginFuture, PluginHostError, PluginManifest, PluginScope, RouteContribution, RouteDescriptor, RouteHandler, RouteMethod, RouteRequest, RouteResponse, RouteScope};

pub const ID: &str = "dev.neoism.lsp";

#[derive(Clone, Copy)]
pub enum LspAction { Status, Hover, SignatureHelp, InlayHints, DocumentHighlights, Definition, References, Implementation, PrepareCallHierarchy, IncomingCalls, OutgoingCalls, Diagnostics, DocumentSymbols, Formatting, CodeActions, Touch, Shutdown }

pub trait LspHost: Send + Sync + 'static {
    fn register_tools(&self, registrar: &mut PluginContributions);
    fn execute<'a>(&'a self, action: LspAction, request: RouteRequest) -> PluginFuture<'a, RouteResponse>;
}

pub struct LspPlugin { host: Arc<dyn LspHost> }
impl LspPlugin { pub fn new(host: Arc<dyn LspHost>) -> Self { Self { host } } }

impl PluginDefinition for LspPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest { id: ID.into(), name: "Language servers".into(), version: env!("CARGO_PKG_VERSION").into(), internal: true, disableable: true, capabilities: vec!["neoism.lsp".into()], requires: Vec::new(), event_namespaces: vec!["lsp".into()], api_prefix: Some(format!("/v2/plugins/{ID}")), config: BTreeMap::new() }
    }

    fn required_capabilities(&self) -> Vec<neoism_agent_plugin_api::HostCapability> { use neoism_agent_plugin_api::HostCapability::*; vec![WorkspaceRead, ProcessSpawn] }
    fn contributions(&self, registrar: &mut PluginContributions) -> Result<(), PluginHostError> {
        self.host.register_tools(registrar);
        for (id, method, suffix, action) in [
            ("v2.plugins.lsp.status", RouteMethod::Get, "", LspAction::Status),
            ("v2.plugins.lsp.hover", RouteMethod::Get, "/hover", LspAction::Hover),
            ("v2.plugins.lsp.signatureHelp", RouteMethod::Get, "/signature-help", LspAction::SignatureHelp),
            ("v2.plugins.lsp.inlayHints", RouteMethod::Get, "/inlay-hints", LspAction::InlayHints),
            ("v2.plugins.lsp.documentHighlights", RouteMethod::Get, "/document-highlights", LspAction::DocumentHighlights),
            ("v2.plugins.lsp.definition", RouteMethod::Get, "/definition", LspAction::Definition),
            ("v2.plugins.lsp.references", RouteMethod::Get, "/references", LspAction::References),
            ("v2.plugins.lsp.implementation", RouteMethod::Get, "/implementation", LspAction::Implementation),
            ("v2.plugins.lsp.prepareCallHierarchy", RouteMethod::Get, "/prepare-call-hierarchy", LspAction::PrepareCallHierarchy),
            ("v2.plugins.lsp.incomingCalls", RouteMethod::Get, "/incoming-calls", LspAction::IncomingCalls),
            ("v2.plugins.lsp.outgoingCalls", RouteMethod::Get, "/outgoing-calls", LspAction::OutgoingCalls),
            ("v2.plugins.lsp.diagnostics", RouteMethod::Get, "/diagnostics", LspAction::Diagnostics),
            ("v2.plugins.lsp.documentSymbols", RouteMethod::Get, "/document-symbols", LspAction::DocumentSymbols),
            ("v2.plugins.lsp.formatting", RouteMethod::Get, "/formatting", LspAction::Formatting),
            ("v2.plugins.lsp.codeActions", RouteMethod::Get, "/code-actions", LspAction::CodeActions),
            ("v2.plugins.lsp.touch", RouteMethod::Post, "/touch", LspAction::Touch),
            ("v2.plugins.lsp.shutdown", RouteMethod::Post, "/shutdown", LspAction::Shutdown),
        ] {
            registrar.runtime_route(RouteContribution {
                descriptor: RouteDescriptor { id: id.into(), method, path: format!("/v2/plugins/{ID}{suffix}"), scope: RouteScope::Workspace, request_schema: None, response_schema: None },
                metadata: ContributionMetadata::new(id, ID, PluginScope::Workspace),
                handler: Arc::new(LspRoute { host: self.host.clone(), action }),
            });
        }
        Ok(())
    }
}

struct LspRoute { host: Arc<dyn LspHost>, action: LspAction }
impl RouteHandler for LspRoute {
    fn handle<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, RouteResponse> { self.host.execute(self.action, request) }
}