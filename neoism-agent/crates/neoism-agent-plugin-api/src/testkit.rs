//! Public conformance helpers for first-party and third-party plugin tests.

use std::collections::BTreeSet;

use crate::{
    ContributionMetadata, PluginContext, PluginFactory, PluginFuture, PluginInstance,
    PluginRuntimeError, PluginScope, ReadinessState, RuntimeScope,
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
    Box::pin(async move {
        let descriptor = factory.descriptor();
        let mut report = ConformanceReport::default();
        if descriptor.manifest.id.trim().is_empty() {
            report.errors.push("plugin id is empty".to_string());
        }
        if descriptor.scope != context.scope().kind() {
            report
                .errors
                .push("factory scope does not match runtime context".to_string());
        }
        for capability in &descriptor.required_capabilities {
            if !context.has(*capability) {
                report.errors.push(format!(
                    "required capability `{capability:?}` was not granted"
                ));
            }
        }
        if !report.errors.is_empty() {
            return Ok(report);
        }

        let instance = factory.create(context.clone()).await?;
        instance.start().await?;
        check_instance(
            &descriptor.manifest.id,
            descriptor.scope,
            context.scope(),
            instance.as_ref(),
            &mut report,
        );
        instance.shutdown().await?;
        // Verify the documented idempotent shutdown contract.
        instance.shutdown().await?;
        Ok(report)
    })
}

pub fn check_instance(
    plugin_id: &str,
    scope: PluginScope,
    runtime_scope: &RuntimeScope,
    instance: &dyn PluginInstance,
    report: &mut ConformanceReport,
) {
    match instance.readiness().state {
        ReadinessState::Ready | ReadinessState::Degraded => {}
        state => report
            .errors
            .push(format!("instance is not usable after start: {state:?}")),
    }
    let contributions = instance.contributions();
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
        if let Err(error) = contribution.descriptor.validate() {
            report.errors.push(error.to_string());
        }
        if contribution.metadata.id != contribution.descriptor.id {
            report.errors.push(format!(
                "route `{}` metadata and descriptor ids differ",
                contribution.metadata.id
            ));
        }
    }
}

fn check_metadata(
    plugin_id: &str,
    scope: PluginScope,
    runtime_scope: &RuntimeScope,
    metadata: &ContributionMetadata,
    ids: &mut BTreeSet<String>,
    report: &mut ConformanceReport,
) {
    if metadata.id.trim().is_empty() {
        report.errors.push("contribution id is empty".to_string());
    }
    if !ids.insert(metadata.id.clone()) {
        report
            .errors
            .push(format!("duplicate contribution id `{}`", metadata.id));
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
        RuntimeScope::Global if metadata.owner.workspace_id.is_some() => {
            report.errors.push(format!(
                "global contribution `{}` has a workspace owner",
                metadata.id
            ))
        }
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
                scope: PluginScope::Global,
                required_capabilities: Vec::new(),
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
            self.0.fetch_add(1, Ordering::SeqCst);
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
            PluginContext::new(RuntimeScope::Global, CapabilityGrants::default()),
        ))
        .unwrap();
        assert!(report.is_conformant(), "{:?}", report.errors);
        assert_eq!(shutdowns.load(Ordering::SeqCst), 2);
    }
}
