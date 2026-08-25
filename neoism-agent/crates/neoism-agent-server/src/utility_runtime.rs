use std::sync::Arc;

pub(crate) struct UtilityRuntime {
    pub(crate) shell: crate::platform_shell::ShellRuntime,
    pub(crate) login_shell_environment: crate::tool::bash::LoginShellEnvironment,
    pub(crate) file_locks: crate::tool::locks::FileLockRegistry,
}

impl UtilityRuntime {
    pub(crate) fn new(services: &neoism_agent_service_api::AgentServices) -> Arc<Self> {
        Arc::new(Self {
            shell: crate::platform_shell::ShellRuntime::resolve(services),
            login_shell_environment: Default::default(),
            file_locks: Default::default(),
        })
    }
}
