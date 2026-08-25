use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExternalRuntime {
    OpenCode,
    Codex,
    Claude,
}

impl ExternalRuntime {
    pub(crate) fn resolve(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "opencode" | "open-code" => Some(Self::OpenCode),
            "codex" | "openai-codex" => Some(Self::Codex),
            "claude" | "claude-code" | "claude-agent" => Some(Self::Claude),
            _ => None,
        }
    }

    pub(crate) fn agent_name(self) -> &'static str {
        match self {
            Self::OpenCode => "opencode",
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::OpenCode => "OpenCode",
            Self::Codex => "Codex",
            Self::Claude => "Claude",
        }
    }

    pub(crate) fn provider_id(self) -> &'static str {
        match self {
            Self::OpenCode => "opencode",
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    pub(crate) fn acp_config(
        self,
        cwd: &str,
        services: &neoism_agent_service_api::AgentServices,
    ) -> Result<AcpServerConfig, String> {
        Ok(match self {
            Self::OpenCode => AcpServerConfig::new(
                "opencode",
                "OpenCode",
                resolve_runtime(services, "opencode")?,
                PathBuf::from(cwd),
            )
            .args(["acp", "--cwd", cwd]),
            Self::Codex => package_acp_config(
                services,
                "codex",
                "Codex",
                "@zed-industries/codex-acp@latest",
                cwd,
            )?,
            Self::Claude => {
                let mut config = package_acp_config(
                    services,
                    "claude",
                    "Claude",
                    "@agentclientprotocol/claude-agent-acp@latest",
                    cwd,
                )?;
                if std::env::var_os("CLAUDE_CODE_EXECUTABLE").is_none() {
                    if let Some(path) = resolve_runtime_path(services, "claude") {
                        config.env.push(("CLAUDE_CODE_EXECUTABLE".to_string(), path));
                    }
                }
                config
            }
        })
    }
}

pub(crate) fn is_external_agent(name: &str) -> bool {
    ExternalRuntime::resolve(name).is_some()
}

fn package_acp_config(
    services: &neoism_agent_service_api::AgentServices,
    id: &'static str,
    name: &'static str,
    package: &'static str,
    cwd: &str,
) -> Result<AcpServerConfig, String> {
    Ok(AcpServerConfig::new(id, name, resolve_runtime(services, "npx")?, PathBuf::from(cwd))
        .args(["--yes", package])
    )
}

fn resolve_runtime(
    services: &neoism_agent_service_api::AgentServices,
    name: &str,
) -> Result<String, String> {
    resolve_runtime_path(services, name).ok_or_else(|| {
        format!(
            "external Agent executable `{name}` is unavailable; configure the host executable resolver or install it"
        )
    })
}

fn resolve_runtime_path(
    services: &neoism_agent_service_api::AgentServices,
    name: &str,
) -> Option<String> {
    let request = neoism_agent_service_api::ExecutableRequest::new(
        name,
        neoism_agent_service_api::ExecutablePurpose::ExternalAgent,
    );
    services
        .executables
        .resolve(&request)
        .ok()
        .map(|result| result.path.to_string_lossy().into_owned())
}
