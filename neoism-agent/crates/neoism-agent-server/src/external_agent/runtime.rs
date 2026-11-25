use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExternalRuntime {
    OpenCode,
    Codex,
    Claude,
    Grok,
    Kiro,
    Antigravity,
    Fx,
    Cursor,
}

impl ExternalRuntime {
    pub(crate) fn native_driver_id(self) -> &'static str {
        match self {
            Self::Codex => "codex-app-server",
            Self::Claude => "claude-agent-sdk",
            Self::OpenCode => "opencode-server",
            Self::Grok | Self::Kiro | Self::Antigravity | Self::Fx | Self::Cursor => "acp",
        }
    }

    pub(crate) fn acp_only(self) -> bool {
        self.native_driver_id() == "acp"
    }

    pub(crate) fn from_provider_id(name: &str) -> Option<Self> {
        Self::resolve(name).filter(|runtime| runtime.provider_id() == name)
    }

    /// Ownership is explicit. Observed provider children are never resumable tasks.
    pub(crate) fn owns_session(self, session: &SessionInfo) -> bool {
        session.extra.get("externalAgent").is_some_and(|extra| {
            extra["provider"] == self.provider_id()
                && extra["nested"] != true
                && extra["nativeProjection"] != true
                && extra["runtime"].as_str().is_some_and(|driver| {
                    driver == self.native_driver_id() || driver == "acp"
                })
        })
    }

    pub(crate) fn is_legacy_session(self, session: &SessionInfo) -> bool {
        self.owns_session(session) && session.extra["externalAgent"]["runtime"] == "acp"
    }
    pub(crate) fn resolve(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "opencode" | "open-code" => Some(Self::OpenCode),
            "codex" | "openai-codex" => Some(Self::Codex),
            "claude" | "claude-code" | "claude-agent" => Some(Self::Claude),
            "grok" | "grok-build" => Some(Self::Grok),
            "kiro" | "kiro-cli" => Some(Self::Kiro),
            "antigravity" | "agy" => Some(Self::Antigravity),
            "fx" => Some(Self::Fx),
            "cursor" | "cursor-agent" => Some(Self::Cursor),
            _ => None,
        }
    }

    pub(crate) fn agent_name(self) -> &'static str {
        self.provider_id()
    }

    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::OpenCode => "OpenCode",
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Grok => "Grok Build",
            Self::Kiro => "Kiro",
            Self::Antigravity => "Antigravity",
            Self::Fx => "fx",
            Self::Cursor => "Cursor",
        }
    }

    pub(super) fn is_sign_in_error(self, message: &str) -> bool {
        let message = message.to_ascii_lowercase();
        [
            "auth_required", "authrequired", "authentication_required", "authentication required",
            "not authenticated", "not logged in", "login required", "log in required",
            "please log in", "please login", "sign in required", "sign-in required",
            "please sign in", "run /login",
        ]
        .iter()
        .any(|marker| message.contains(marker))
            || (self == Self::Claude && (message.contains("claude code authentication_failed")
                || message.contains("oauth session expired and could not be refreshed")))
    }

    pub(super) fn task_failure_message(self, error: String) -> String {
        if !self.is_sign_in_error(&error) {
            return error;
        }
        let provider = self.display_name();
        let login = match self {
            Self::Claude => "Sign in using `claude auth login`",
            Self::Codex => "Sign in with ChatGPT using `codex login`",
            _ => "Sign in using the provider's CLI",
        };
        format!("{provider} sign-in required: Neoism cannot run this {provider} task yet. {login} on the host running the agent, then retry the task. This is separate from Neoism's /connect.")
    }

    pub(crate) fn provider_id(self) -> &'static str {
        match self {
            Self::OpenCode => "opencode",
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Grok => "grok",
            Self::Kiro => "kiro",
            Self::Antigravity => "antigravity",
            Self::Fx => "fx",
            Self::Cursor => "cursor",
        }
    }

    fn installed_acp_command(self) -> Option<(&'static str, &'static [&'static str])> {
        match self {
            Self::Grok => Some((
                "grok",
                &["--permission-mode", "default", "agent", "stdio"],
            )),
            Self::Kiro => Some((
                "kiro-cli",
                &["acp", "--agent-engine=v3", "--auth-method=cli"],
            )),
            Self::Antigravity => Some(("agy", &["acp"])),
            Self::Fx => Some(("fx", &["acp"])),
            Self::Cursor => Some(("cursor-agent", &["acp"])),
            _ => None,
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
            Self::Codex => {
                let mut config = package_acp_config(
                    services,
                    "codex",
                    "Codex",
                    "@agentclientprotocol/codex-acp@1.13.1",
                    cwd,
                )?;
                if std::env::var_os("CODEX_PATH").is_none() {
                    if let Some(path) = resolve_runtime_path(services, "codex") {
                        config.env.push(("CODEX_PATH".to_string(), path));
                    }
                }
                config
            }
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
            installed => {
                let (command, args) = installed
                    .installed_acp_command()
                    .ok_or_else(|| "Missing installed ACP command".to_string())?;
                AcpServerConfig::new(
                    installed.provider_id(),
                    installed.display_name(),
                    resolve_runtime(services, command)?,
                    PathBuf::from(cwd),
                )
                .args(args.iter().copied())
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

/// Only explicitly owned roots are routed here; never infer a protocol from a model.
pub(crate) fn root_runtime(session: &SessionInfo) -> Option<ExternalRuntime> {
    if session.parent_id.is_some() {
        return None;
    }
    let extra = session.extra.get("externalAgent")?;
    let runtime = ExternalRuntime::from_provider_id(extra.get("provider")?.as_str()?)?;
    runtime.owns_session(session).then_some(runtime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_sign_in_errors_name_the_harness_and_keep_other_failures_distinct() {
        let claude = ExternalRuntime::Claude.task_failure_message(
            "adapter turn failed: Claude Code authentication_failed; authentication remains native-owned".into(),
        );
        assert!(claude.contains("Claude Code sign-in required"));
        assert!(ExternalRuntime::Claude.is_sign_in_error("OAuth session expired and could not be refreshed"));
        assert!(claude.contains("claude auth login"));
        assert!(!claude.contains("native-owned"));
        let codex = ExternalRuntime::Codex.task_failure_message("Not logged in".into());
        assert!(codex.contains("ChatGPT using `codex login`"));
        for error in ["Claude Code billing_error", "Permission denied", "Session aborted"] {
            assert_eq!(ExternalRuntime::Claude.task_failure_message(error.into()), error);
        }
    }

    #[test]
    fn installed_harnesses_launch_directly_via_host_resolver() {
        for (provider, executable, args) in [
            ("grok", "grok", &["--permission-mode", "default", "agent", "stdio"][..]),
            ("kiro", "kiro-cli", &["acp", "--agent-engine=v3", "--auth-method=cli"][..]),
            ("antigravity", "agy", &["acp"][..]),
            ("fx", "fx", &["acp"][..]),
            ("cursor", "cursor-agent", &["acp"][..]),
        ] {
            let mut services = crate::standard_services();
            let resolved = if cfg!(windows) {
                r"C:\agents\harness.exe"
            } else {
                "/agents/harness"
            };
            services.executables = Arc::new(
                crate::executable::test_support::FakeExecutableService::with(executable, resolved),
            );
            let config = ExternalRuntime::from_provider_id(provider)
                .unwrap()
                .acp_config(".", &services)
                .unwrap();
            assert_eq!(config.command, resolved, "{provider}");
            assert!(config.args.iter().map(String::as_str).eq(args.iter().copied()), "{provider}: {:?}", config.args);
        }
    }

    #[test]
    fn codex_adapter_prefers_host_cli_without_overriding_explicit_path() {
        let mut services = crate::standard_services();
        let codex = if cfg!(windows) { r"C:\neoism\codex\codex.exe" } else { "/neoism/codex/bin/codex" };
        let npx = if cfg!(windows) { r"C:\neoism\node\npx.cmd" } else { "/neoism/node/bin/npx" };
        services.executables = Arc::new(
            crate::executable::test_support::FakeExecutableService::with("npx", npx)
                .with_path("codex", codex),
        );
        let config = ExternalRuntime::Codex.acp_config(".", &services).unwrap();
        assert_eq!(config.command, npx);
        let override_path = config.env.iter().find_map(|(name, value)| (name == "CODEX_PATH").then_some(value.as_str()));
        if std::env::var_os("CODEX_PATH").is_some() {
            assert_eq!(override_path, None, "preserve the inherited explicit override");
        } else {
            assert_eq!(override_path, Some(codex));
        }
    }

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
