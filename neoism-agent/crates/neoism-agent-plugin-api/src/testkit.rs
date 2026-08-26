//! Public conformance helpers for first-party and third-party plugin tests.

use std::collections::BTreeSet;

use crate::{
    ContributionMetadata, PluginContext, PluginDescriptor, PluginFactory, PluginFuture,
    PluginInstance, PluginRuntimeError, PluginScope, ReadinessState, RoutePrefixPolicy, RuntimeScope,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConformanceReport {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ConformanceReport {
    pub fn is_conformant(&self) -> bool {
        self.errors.is_empty()
    }
    pub fn into_result(self) -> Result<(), PluginRuntimeError> {
        if self.is_conformant() {
            Ok(())
        } else {
            Err(PluginRuntimeError::new(self.errors.join("; ")))
        }
    }
}

pub fn check_factory<'a>(
    factory: &'a dyn PluginFactory,
    context: PluginContext,
) -> PluginFuture<'a, ConformanceReport> {
    check_factory_with_policy(factory, context, RoutePrefixPolicy::default())
}

pub fn check_factory_with_policy<'a>(
    factory: &'a dyn PluginFactory,
    context: PluginContext,
    route_prefix_policy: RoutePrefixPolicy,
) -> PluginFuture<'a, ConformanceReport> {
    Box::pin(async move {
        let descriptor = factory.descriptor();
        let mut report = ConformanceReport::default();
        if let Err(error) = crate::validate_host_descriptor(&descriptor, &context, &route_prefix_policy) {
            report.errors.push(error.to_string());
            return Ok(report);
        }

        let instance = factory.create(context.restricted_to(&descriptor.required_capabilities)).await?;
        if let Err(primary) = instance.start().await {
            return match shutdown_retries(instance.as_ref()).await {
                Ok(()) => Err(primary),
                Err(cleanup) => Err(PluginRuntimeError::new(format!("{primary}; cleanup also failed: {cleanup}"))),
            };
        }
        check_instance_with_policy(
            &descriptor,
            context.scope(),
            instance.as_ref(),
            &mut report,
            &route_prefix_policy,
        );
        shutdown_retries(instance.as_ref()).await?;
        // Verify the documented idempotent shutdown contract.
        shutdown_retries(instance.as_ref()).await?;
        Ok(report)
    })
}

async fn shutdown_retries(instance: &dyn PluginInstance) -> Result<(), PluginRuntimeError> {
    let mut last_error = None;
    for _ in 0..3 {
        match instance.shutdown().await {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| PluginRuntimeError::new("plugin cleanup failed")))
}

pub fn check_instance(
    descriptor: &PluginDescriptor,
    runtime_scope: &RuntimeScope,
    instance: &dyn PluginInstance,
    report: &mut ConformanceReport,
) {
    check_instance_with_policy(descriptor, runtime_scope, instance, report, &RoutePrefixPolicy::default());
}

fn check_instance_with_policy(
    descriptor: &PluginDescriptor,
    runtime_scope: &RuntimeScope,
    instance: &dyn PluginInstance,
    report: &mut ConformanceReport,
    route_prefix_policy: &RoutePrefixPolicy,
) {
    match instance.readiness().state {
        ReadinessState::Ready | ReadinessState::Degraded => {}
        state => report
            .errors
            .push(format!("instance is not usable after start: {state:?}")),
    }
    let plugin_id = descriptor.manifest.id.as_str();
    let scope = descriptor.scope;
    let mut contributions = instance.contributions();
    let workspace_id = match runtime_scope { RuntimeScope::Workspace(workspace) => Some(workspace.id.clone()) };
    macro_rules! stamp_services {
        ($items:expr) => { for item in &mut $items { stamp_metadata(&mut item.metadata, plugin_id, scope, workspace_id.as_deref()); } };
    }
    stamp_services!(contributions.config);
    stamp_services!(contributions.agents);
    stamp_services!(contributions.commands);
    stamp_services!(contributions.skills);
    stamp_services!(contributions.providers);
    stamp_services!(contributions.system_context);
    stamp_services!(contributions.prompts);
    for route in &mut contributions.routes { stamp_metadata(&mut route.metadata, plugin_id, scope, workspace_id.as_deref()); }
    for route in &mut contributions.websocket_routes { stamp_metadata(&mut route.metadata, plugin_id, scope, workspace_id.as_deref()); }
    if let Err(error) = crate::validate_contributions(descriptor, &contributions, route_prefix_policy) {
        report.errors.push(error.to_string());
    }
    let mut ids = BTreeSet::new();
    macro_rules! check_services {
        ($items:expr) => {
            for contribution in &$items {
                check_metadata(
                    plugin_id,
                    scope,
                    runtime_scope,
                    &contribution.metadata,
                    &mut ids,
                    report,
                );
            }
        };
    }
    check_services!(contributions.config);
    check_services!(contributions.agents);
    check_services!(contributions.commands);
    check_services!(contributions.skills);
    check_services!(contributions.providers);
    check_services!(contributions.system_context);
    check_services!(contributions.prompts);
    for contribution in &contributions.routes {
        check_metadata(
            plugin_id,
            scope,
            runtime_scope,
            &contribution.metadata,
            &mut ids,
            report,
        );
    }
    for contribution in &contributions.websocket_routes {
        check_metadata(plugin_id, scope, runtime_scope, &contribution.metadata, &mut ids, report);
    }
}

fn stamp_metadata(metadata: &mut ContributionMetadata, plugin_id: &str, scope: PluginScope, workspace_id: Option<&str>) {
    metadata.owner.plugin_id = plugin_id.to_string();
    metadata.owner.scope = scope;
    metadata.owner.workspace_id = workspace_id.map(str::to_string);
}

fn check_metadata(
    plugin_id: &str,
    scope: PluginScope,
    runtime_scope: &RuntimeScope,
    metadata: &ContributionMetadata,
    _ids: &mut BTreeSet<String>,
    report: &mut ConformanceReport,
) {
    if metadata.id.trim().is_empty() {
        report.errors.push("contribution id is empty".to_string());
    }
    if metadata.owner.plugin_id != plugin_id {
        report
            .errors
            .push(format!("contribution `{}` has foreign owner", metadata.id));
    }
    if metadata.owner.scope != scope {
        report.errors.push(format!(
            "contribution `{}` has the wrong scope",
            metadata.id
        ));
    }
    match runtime_scope {
        RuntimeScope::Workspace(workspace)
            if metadata.owner.workspace_id.as_deref() != Some(&workspace.id) =>
        {
            report.errors.push(format!(
                "workspace contribution `{}` is not owned by workspace `{}`",
                metadata.id, workspace.id
            ))
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    use crate::{
        CapabilityGrants, PluginContributions, PluginDescriptor, PluginManifest,
        PluginReadiness,
    };

    use super::*;

    const PLUGIN_ID: &str = "dev.neoism.conformance-test";

    struct Factory(Arc<AtomicUsize>);
    struct Instance(Arc<AtomicUsize>);

    impl PluginFactory for Factory {
        fn descriptor(&self) -> PluginDescriptor {
            PluginDescriptor {
                manifest: PluginManifest {
                    id: PLUGIN_ID.to_string(),
                    name: "Conformance test".to_string(),
                    version: "1.0.0".to_string(),
                    internal: true,
                    disableable: true,
                    capabilities: Vec::new(),
                    requires: Vec::new(),
                    event_namespaces: Vec::new(),
                    api_prefix: None,
                    config: BTreeMap::new(),
                },
                scope: PluginScope::Workspace,
                required_capabilities: Vec::new(),
                plugin_api_major: crate::PLUGIN_API_MAJOR,
            }
        }

        fn create<'a>(
            &'a self,
            _context: PluginContext,
        ) -> PluginFuture<'a, Box<dyn PluginInstance>> {
            let shutdowns = self.0.clone();
            Box::pin(async move {
                Ok(Box::new(Instance(shutdowns)) as Box<dyn PluginInstance>)
            })
        }
    }

    impl PluginInstance for Instance {
        fn readiness(&self) -> PluginReadiness {
            PluginReadiness::ready()
        }

        fn contributions(&self) -> PluginContributions {
            PluginContributions::default()
        }

        fn shutdown<'a>(&'a self) -> PluginFuture<'a, ()> {
            let _ = self.0.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    struct NoopWake;
    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        let waker = Waker::from(Arc::new(NoopWake));
        let mut context = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("test future unexpectedly pending"),
        }
    }

    #[test]
    fn factory_check_exercises_start_readiness_and_idempotent_shutdown() {
        let shutdowns = Arc::new(AtomicUsize::new(0));
        let report = block_on(check_factory(
            &Factory(shutdowns.clone()),
            PluginContext::new(RuntimeScope::Workspace(crate::WorkspaceIdentity { id: "workspace".into(), root: ".".into() }), CapabilityGrants::default()),
        ))
        .unwrap();
        assert!(report.is_conformant(), "{:?}", report.errors);
        assert_eq!(shutdowns.load(Ordering::SeqCst), 1);
    }

    struct InvalidIdFactory;
    impl PluginFactory for InvalidIdFactory {
        fn descriptor(&self) -> PluginDescriptor {
            let mut descriptor = Factory(Arc::new(AtomicUsize::new(0))).descriptor();
            descriptor.manifest.id = "not-reverse-dns".into();
            descriptor
        }

        fn create<'a>(&'a self, _context: PluginContext) -> PluginFuture<'a, Box<dyn PluginInstance>> {
            panic!("descriptor preflight must run before create")
        }
    }

    #[test]
    fn factory_preflight_matches_the_host_descriptor_validator() {
        let context = PluginContext::new(
            RuntimeScope::Workspace(crate::WorkspaceIdentity { id: "workspace".into(), root: ".".into() }),
            CapabilityGrants::default(),
        );
        let host_error = match block_on(crate::PluginHost::default().install(
            vec![Box::new(InvalidIdFactory)],
            &[],
            context.clone(),
        )) {
            Ok(_) => panic!("invalid descriptor installed"),
            Err(error) => error.to_string(),
        };
        let report = block_on(check_factory(&InvalidIdFactory, context)).unwrap();
        assert_eq!(report.errors, vec![host_error]);
    }

    struct Hook;
    impl crate::RuntimeHook for Hook {
        fn invoke(&self, _: &str, _: serde_json::Value, value: serde_json::Value) -> Result<serde_json::Value, PluginRuntimeError> { Ok(value) }
    }
    struct WebSocketHandler;
    impl crate::WebSocketRouteHandler for WebSocketHandler {
        fn prepare<'a>(&'a self, _: crate::RouteRequest) -> PluginFuture<'a, Arc<dyn crate::WebSocketSession>> {
            Box::pin(async { Err(PluginRuntimeError::new("not invoked by conformance")) })
        }
    }
    struct CoverageInstance;
    impl PluginInstance for CoverageInstance {
        fn readiness(&self) -> PluginReadiness { PluginReadiness::ready() }
        fn contributions(&self) -> PluginContributions {
            let mut contributions = PluginContributions::default();
            contributions.hook("transform");
            contributions.runtime_hook(Arc::new(Hook));
            contributions.runtime_websocket_route(crate::WebSocketRouteContribution {
                metadata: ContributionMetadata::new("socket", PLUGIN_ID, PluginScope::Workspace),
                descriptor: crate::RouteDescriptor { id: "socket".into(), method: crate::RouteMethod::Get, path: format!("/v2/plugins/{PLUGIN_ID}/socket"), scope: crate::RouteScope::Workspace, request_schema: None, response_schema: None },
                handler: Arc::new(WebSocketHandler),
            });
            contributions
        }
        fn shutdown<'a>(&'a self) -> PluginFuture<'a, ()> { Box::pin(async { Ok(()) }) }
    }

    #[test]
    fn instance_check_covers_websockets_hooks_and_declarations_after_host_stamping() {
        let scope = RuntimeScope::Workspace(crate::WorkspaceIdentity { id: "workspace".into(), root: ".".into() });
        let mut report = ConformanceReport::default();
        check_instance(&Factory(Arc::new(AtomicUsize::new(0))).descriptor(), &scope, &CoverageInstance, &mut report);
        assert!(report.is_conformant(), "{:?}", report.errors);
    }

    struct MismatchedRouteInstance;
    impl PluginInstance for MismatchedRouteInstance {
        fn readiness(&self) -> PluginReadiness { PluginReadiness::ready() }
        fn contributions(&self) -> PluginContributions {
            let mut contributions = PluginContributions::default();
            contributions.runtime_websocket_route(crate::WebSocketRouteContribution {
                metadata: ContributionMetadata::new("metadata-id", PLUGIN_ID, PluginScope::Workspace),
                descriptor: crate::RouteDescriptor { id: "descriptor-id".into(), method: crate::RouteMethod::Get, path: format!("/v2/plugins/{PLUGIN_ID}/socket"), scope: crate::RouteScope::Workspace, request_schema: None, response_schema: None },
                handler: Arc::new(WebSocketHandler),
            });
            contributions
        }
        fn shutdown<'a>(&'a self) -> PluginFuture<'a, ()> { Box::pin(async { Ok(()) }) }
    }

    #[test]
    fn instance_check_uses_host_route_identity_validation() {
        let scope = RuntimeScope::Workspace(crate::WorkspaceIdentity { id: "workspace".into(), root: ".".into() });
        let mut report = ConformanceReport::default();
        check_instance(&Factory(Arc::new(AtomicUsize::new(0))).descriptor(), &scope, &MismatchedRouteInstance, &mut report);
        assert!(report.errors.iter().any(|error| error.contains("metadata and descriptor ids differ")));
    }
}
