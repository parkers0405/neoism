pub mod config;
pub mod custom_tools;

pub mod agents;
pub mod artifacts;
pub mod commands;
pub mod documentation_tools;
pub mod goals;
pub mod interactions;
pub mod lsp;
pub mod mcp;
pub mod memory_tools;
pub mod providers;
pub mod pty;
pub mod semantic;
pub mod skills;
pub mod subagents;
pub mod system_prompt;
pub mod vcs;
pub mod websearch;
pub mod workflows;
pub mod workspace_tools;

pub use agents::AgentsPlugin;
pub use artifacts::ArtifactsPlugin;
pub use commands::CommandsPlugin;
pub use config::ConfigPlugin;
pub use custom_tools::CustomToolsPlugin;
pub use documentation_tools::DocumentationToolsPlugin;
pub use goals::GoalsPlugin;
pub use interactions::InteractionsPlugin;
pub use lsp::LspPlugin;
pub use mcp::McpPlugin;
pub use memory_tools::MemoryToolsPlugin;
pub use providers::ProvidersPlugin;
pub use pty::PtyPlugin;
pub use semantic::SemanticPlugin;
pub use skills::SkillsPlugin;
pub use subagents::SubagentsPlugin;
pub use system_prompt::SystemPromptPlugin;
pub use vcs::VcsPlugin;
pub use websearch::WebsearchPlugin;
pub use workflows::WorkflowsPlugin;
pub use workspace_tools::WorkspaceToolsPlugin;

#[cfg(test)]
mod lifecycle_conformance_tests {
    use neoism_agent_plugin_api::{
        CapabilityGrants, HostCapability, PluginContext, PluginFactory, RuntimeScope,
        WorkspaceIdentity,
    };

    fn context() -> PluginContext {
        let grants = [
            HostCapability::ConfigRead,
            HostCapability::ConfigWrite,
            HostCapability::WorkspaceRead,
            HostCapability::WorkspaceWrite,
            HostCapability::EventPublish,
            HostCapability::Network,
            HostCapability::ProcessSpawn,
            HostCapability::SecretRead,
        ]
        .into_iter()
        .fold(CapabilityGrants::default(), CapabilityGrants::allow);
        PluginContext::new(
            RuntimeScope::Workspace(WorkspaceIdentity {
                id: "test".into(),
                root: ".".into(),
            }),
            grants,
        )
    }

    #[tokio::test]
    async fn representative_first_party_factories_cover_services_sources_tools_and_routes(
    ) {
        let config = neoism_agent_core::AgentConfigDocument::default();
        let factories: Vec<Box<dyn PluginFactory>> = vec![
            Box::new(super::SystemPromptPlugin),
            Box::new(super::CommandsPlugin::new(&config)),
            Box::new(super::AgentsPlugin::new(&config)),
            Box::new(super::WebsearchPlugin),
        ];
        for factory in factories {
            let descriptor = factory.descriptor();
            let policy = descriptor.manifest.api_prefix.as_ref().map_or_else(
                neoism_agent_plugin_api::RoutePrefixPolicy::default,
                |prefix| {
                    neoism_agent_plugin_api::RoutePrefixPolicy::default()
                        .allow_legacy(prefix)
                },
            );
            let report = neoism_agent_plugin_api::testkit::check_factory_with_policy(
                factory.as_ref(),
                context(),
                policy,
            )
            .await
            .unwrap();
            assert!(
                report.is_conformant(),
                "{}: {:?}",
                factory.descriptor().manifest.id,
                report.errors
            );
        }
        let installed = neoism_agent_plugin_api::PluginHost::default()
            .install(vec![Box::new(super::SystemPromptPlugin)], &[], context())
            .await
            .unwrap();
        let snapshot = installed.snapshot();
        let metadata = snapshot
            .service_metadata
            .values()
            .next()
            .expect("system prompt service metadata");
        assert_eq!(metadata.owner.plugin_id, super::system_prompt::ID);
    }
}
