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
                "@agentclientprotocol/codex-acp@1.13.1",
                cwd,
            )?,
            Self::Claude => {
                let mut config = package_acp_config(
                    services,
                    "claude",
                    "Claude",
                    "@agentclientprotocol/claude-agent-acp@0.81.1",
                    cwd,
                )?;
                if std::env::var_os("CLAUDE_CODE_EXECUTABLE").is_none() {
                    if let Some(path) = resolve_runtime_path(services, "claude") {
                        config
                            .env
                            .push(("CLAUDE_CODE_EXECUTABLE".to_string(), path));
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
    let npx = resolve_runtime(services, "npx")?;
    let mut config = AcpServerConfig::new(id, name, npx.clone(), PathBuf::from(cwd))
        .args(["--yes", package]);
    config.env.push((
        "npm_config_cache".to_string(),
        neoism_agent_builtins::default_cache_dir()
            .join("npm-acp")
            .to_string_lossy()
            .into_owned(),
    ));
    if let Some(bin_dir) = Path::new(&npx).parent() {
        let mut paths = vec![bin_dir.to_path_buf()];
        if let Some(existing) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        if let Ok(path) = std::env::join_paths(paths) {
            config
                .env
                .push(("PATH".to_string(), path.to_string_lossy().into_owned()));
        }
    }
    Ok(config)
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

/// Only root sessions explicitly created as ACP chats are routed here.
pub(crate) fn root_runtime(session: &SessionInfo) -> Option<ExternalRuntime> {
    if session.parent_id.is_some() {
        return None;
    }
    let extra = session.extra.get("externalAgent")?;
    if extra.get("runtime")?.as_str()? != "acp" {
        return None;
    }
    match extra.get("provider")?.as_str()? {
        "opencode" => Some(ExternalRuntime::OpenCode),
        "claude" => Some(ExternalRuntime::Claude),
        "codex" => Some(ExternalRuntime::Codex),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_adapter_uses_resolved_npx_and_prepends_its_node_bin() {
        let mut services = crate::standard_services();
        let managed_npx = if cfg!(windows) {
            r"C:\neoism\node\v22.11.0\npx.cmd"
        } else {
            "/neoism/node/v22.11.0/bin/npx"
        };
        services.executables = Arc::new(
            crate::executable::test_support::FakeExecutableService::with(
                "npx",
                managed_npx,
            ),
        );

        let config = package_acp_config(
            &services,
            "claude",
            "Claude",
            "@agentclientprotocol/claude-agent-acp@0.81.1",
            ".",
        )
        .unwrap();
        assert_eq!(config.command, managed_npx);
        assert!(config.env.iter().any(|(name, value)| {
            name == "npm_config_cache"
                && Path::new(value).file_name().and_then(|name| name.to_str())
                    == Some("npm-acp")
        }));
        let path = config
            .env
            .iter()
            .find_map(|(name, value)| (name == "PATH").then_some(value))
            .expect("managed Node PATH");
        assert_eq!(
            std::env::split_paths(path).next().as_deref(),
            Path::new(managed_npx).parent()
        );
    }
}
