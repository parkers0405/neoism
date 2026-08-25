use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::time::{Duration, Instant};

use clap::{Args, Parser, Subcommand};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};

const DEFAULT_SERVER: &str = "http://127.0.0.1:4096";
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);
const POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Parser)]
#[command(name = "neoism", disable_help_subcommand = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Manage model-provider credentials.
    Auth(AuthArgs),
    /// Manage Model Context Protocol servers.
    Mcp(McpArgs),
}

#[derive(Debug, Args)]
struct AuthArgs {
    #[command(subcommand)]
    command: AuthCommand,
}

#[derive(Debug, Subcommand)]
enum AuthCommand {
    /// Connect a model provider.
    Login(AuthLoginArgs),
    /// List provider authentication status.
    List(OutputArgs),
    /// Show one provider's authentication status.
    Status(AuthProviderArgs),
    /// Remove a provider's saved credentials.
    Logout(AuthProviderArgs),
}

#[derive(Debug, Args)]
struct AuthLoginArgs {
    /// Provider ID or alias. `codex` selects OpenAI ChatGPT OAuth.
    provider: String,
    /// Authentication method index, kind, or label.
    #[arg(short, long)]
    method: Option<String>,
    /// API key for API-key methods.
    #[arg(long, hide_env_values = true)]
    key: Option<String>,
    /// Authorization code for OAuth flows that require one.
    #[arg(long, hide_env_values = true)]
    code: Option<String>,
    /// Do not try to open the authorization URL in a browser.
    #[arg(long)]
    no_open: bool,
    /// Start OAuth but do not wait for completion.
    #[arg(long)]
    no_wait: bool,
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Debug, Args)]
struct AuthProviderArgs {
    /// Provider ID or alias.
    provider: String,
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Debug, Args)]
struct McpArgs {
    #[command(subcommand)]
    command: McpCommand,
}

#[derive(Debug, Subcommand)]
enum McpCommand {
    /// List configured MCP servers and their status.
    List(OutputArgs),
    /// Authenticate an MCP server, or list MCP authentication status.
    Auth(McpAuthArgs),
    /// Remove an MCP server's saved OAuth credentials.
    Logout(McpServerArgs),
}

#[derive(Debug, Args)]
struct McpAuthArgs {
    #[command(subcommand)]
    command: Option<McpAuthCommand>,
    /// MCP server name. Omit it to use `neoism mcp auth list`.
    name: Option<String>,
    /// Do not try to open the authorization URL in a browser.
    #[arg(long)]
    no_open: bool,
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Debug, Subcommand)]
enum McpAuthCommand {
    /// List MCP authentication status.
    List(OutputArgs),
}

#[derive(Debug, Args)]
struct McpServerArgs {
    name: String,
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Clone, Debug, Args)]
struct OutputArgs {
    /// Agent server URL.
    #[arg(
        long,
        env = "NEOISM_SERVER",
        default_value = DEFAULT_SERVER,
        hide = true
    )]
    server: String,
    /// Print JSON instead of human-readable output.
    #[arg(long)]
    json: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum AuthKind {
    Api,
    OAuth,
}

#[derive(Clone, Debug, Deserialize)]
struct AuthMethod {
    #[serde(rename = "type")]
    kind: AuthKind,
    label: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Authorization {
    url: String,
    instructions: String,
}

#[derive(Debug, Deserialize)]
struct McpAuthStart {
    #[serde(rename = "authorizationUrl")]
    authorization_url: String,
}

#[derive(Debug, Deserialize)]
struct McpAuthRemove {
    success: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum McpStatus {
    Connected,
    Disabled,
    NeedsAuth,
    NeedsClientRegistration { error: String },
    Failed { error: String },
}

pub fn maybe_run(
    args: &[OsString],
    ensure_server_started: impl FnOnce(),
    services: neoism_agent_service_api::AgentServices,
) -> Option<Result<(), String>> {
    let command = args.first()?.to_string_lossy();
    if command != "auth" && command != "mcp" {
        return None;
    }

    let argv = std::iter::once(OsString::from("neoism"))
        .chain(args.iter().cloned())
        .collect::<Vec<_>>();
    Some(match Cli::try_parse_from(argv) {
        Ok(cli) => {
            ensure_server_started();
            run(cli.command, services)
        }
        Err(error) => {
            let exit_code = error.exit_code();
            let failed = error.use_stderr();
            match error.print() {
                Err(error) => Err(error.to_string()),
                Ok(()) if failed => std::process::exit(exit_code),
                Ok(()) => Ok(()),
            }
        }
    })
}

fn run(command: Command, services: neoism_agent_service_api::AgentServices) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to start command runtime: {error}"))?;
    runtime.block_on(async move {
        match command {
            Command::Auth(args) => run_auth(args.command, &services).await,
            Command::Mcp(args) => run_mcp(args.command, &services).await,
        }
    })
}

async fn run_auth(command: AuthCommand, services: &neoism_agent_service_api::AgentServices) -> Result<(), String> {
    match command {
        AuthCommand::Login(args) => auth_login(args, services).await,
        AuthCommand::List(output) => auth_list(output).await,
        AuthCommand::Status(args) => auth_status(args).await,
        AuthCommand::Logout(args) => auth_logout(args).await,
    }
}

async fn auth_login(
    mut args: AuthLoginArgs,
    services: &neoism_agent_service_api::AgentServices,
) -> Result<(), String> {
    let codex = args.provider.eq_ignore_ascii_case("codex");
    let provider = provider_id(&args.provider).to_string();
    let methods: BTreeMap<String, Vec<AuthMethod>> =
        get_json(&args.output.server, "/v2/providers/auth-methods").await?;
    let available = methods
        .get(&provider)
        .ok_or_else(|| format!("unknown provider {provider}"))?;
    let selector = if codex && args.method.is_none() {
        Some("0".to_string())
    } else {
        args.method.take()
    };
    let (index, method) = select_method(available, selector.as_deref())?;

    match method.kind {
        AuthKind::Api => {
            let key =
                match args.key {
                    Some(key) => key,
                    None if io::stdin().is_terminal() => prompt_secret("API key: ")?,
                    None => return Err(
                        "an API key is required; pass --key when stdin is not a terminal"
                            .into(),
                    ),
                };
            let saved: bool = put_json(
                &args.output.server,
                &format!("/v2/providers/{provider}/auth"),
                &json!({ "type": "api", "key": key }),
            )
            .await?;
            print_result(
                &args.output,
                json!({ "provider": provider, "saved": saved }),
                || format!("Connected {provider} with {}.", method.label),
            )
        }
        AuthKind::OAuth => {
            let authorization: Authorization = post_json::<Option<Authorization>>(
                &args.output.server,
                &format!("/v2/providers/{provider}/oauth/authorize"),
                &json!({ "method": index, "inputs": {} }),
            )
            .await?
            .ok_or_else(|| format!("provider {provider} did not start OAuth authorization"))?;
            println!("{}\n\n{}", authorization.instructions, authorization.url);
            if !args.no_open {
                if let Err(error) = open_browser(services, &authorization.url) {
                    eprintln!("Could not open a browser ({error}). Open the URL above to continue.");
                }
            }
            if args.no_wait {
                return Ok(());
            }
            eprintln!("\nWaiting for authorization...");
            let saved: bool = post_json(
                &args.output.server,
                &format!("/v2/providers/{provider}/oauth/callback"),
                &json!({ "method": index, "code": args.code }),
            )
            .await?;
            print_result(
                &args.output,
                json!({ "provider": provider, "saved": saved }),
                || format!("Connected {provider} with {}.", method.label),
            )
        }
    }
}

async fn auth_list(output: OutputArgs) -> Result<(), String> {
    let methods: BTreeMap<String, Vec<AuthMethod>> =
        get_json(&output.server, "/v2/providers/auth-methods").await?;
    let mut statuses = BTreeMap::new();
    for provider in methods.keys() {
        let auth: Option<Value> =
            get_json(&output.server, &format!("/v2/providers/{provider}/auth")).await?;
        statuses.insert(provider.clone(), auth_kind(auth.as_ref()));
    }
    if output.json {
        return print_json(&statuses);
    }
    println!("Provider authentication\n");
    for (provider, status) in statuses {
        println!("  {provider:<28} {status}");
    }
    Ok(())
}

async fn auth_status(args: AuthProviderArgs) -> Result<(), String> {
    let provider = provider_id(&args.provider);
    let auth: Option<Value> =
        get_json(&args.output.server, &format!("/v2/providers/{provider}/auth")).await?;
    if args.output.json {
        return print_json(&json!({ "provider": provider, "auth": auth }));
    }
    println!("{provider}\t{}", auth_kind(auth.as_ref()));
    Ok(())
}

async fn auth_logout(args: AuthProviderArgs) -> Result<(), String> {
    let provider = provider_id(&args.provider);
    let removed: bool =
        delete_json(&args.output.server, &format!("/v2/providers/{provider}/auth")).await?;
    print_result(
        &args.output,
        json!({ "provider": provider, "removed": removed }),
        || format!("Removed credentials for {provider}."),
    )
}

async fn run_mcp(
    command: McpCommand,
    services: &neoism_agent_service_api::AgentServices,
) -> Result<(), String> {
    match command {
        McpCommand::List(output) => mcp_list(output, false).await,
        McpCommand::Auth(args) => match args.command {
            Some(McpAuthCommand::List(output)) => mcp_list(output, true).await,
            None => {
                let name = args.name.ok_or_else(|| {
                    "an MCP server name is required; use `neoism mcp auth list` to list servers"
                        .to_string()
                })?;
                mcp_authenticate(name, args.no_open, args.output, services).await
            }
        },
        McpCommand::Logout(args) => mcp_logout(args).await,
    }
}

async fn mcp_list(output: OutputArgs, auth_only: bool) -> Result<(), String> {
    let status = mcp_status(&output.server).await?;
    if output.json {
        return print_json(&status);
    }
    println!("MCP servers\n");
    for (name, state) in status {
        if auth_only && matches!(state, McpStatus::Disabled) {
            continue;
        }
        println!("  {name:<28} {}", mcp_status_label(&state));
    }
    Ok(())
}

async fn mcp_authenticate(
    name: String,
    no_open: bool,
    output: OutputArgs,
    services: &neoism_agent_service_api::AgentServices,
) -> Result<(), String> {
    let auth_path = mcp_auth_path(&name);
    let started: McpAuthStart = post_json(&output.server, &auth_path, &json!({})).await?;
    println!(
        "Authorize {name} in your browser:\n\n{}",
        started.authorization_url
    );
    if !no_open {
        if let Err(error) = open_browser(services, &started.authorization_url) {
            eprintln!(
                "Could not open a browser ({error}). Open the URL above to continue."
            );
        }
    }
    eprintln!("\nWaiting for authorization...");

    let deadline = Instant::now() + AUTH_TIMEOUT;
    loop {
        let status = mcp_status(&output.server).await?;
        match status.get(&name) {
            Some(McpStatus::Connected) => {
                return print_result(
                    &output,
                    json!({ "name": name, "status": "connected" }),
                    || format!("Connected {name}."),
                )
            }
            Some(McpStatus::Failed { error }) => {
                return Err(format!("{name} failed: {error}"))
            }
            _ if Instant::now() >= deadline => {
                return Err(format!("timed out waiting for {name} authorization"))
            }
            _ => tokio::time::sleep(POLL_INTERVAL).await,
        }
    }
}

async fn mcp_status(server: &str) -> Result<BTreeMap<String, McpStatus>, String> {
    get_json(server, "/v2/plugins/dev.neoism.mcp").await
}

async fn mcp_logout(args: McpServerArgs) -> Result<(), String> {
    let removed: McpAuthRemove =
        delete_json(&args.output.server, &mcp_auth_path(&args.name)).await?;
    print_result(
        &args.output,
        json!({ "name": args.name, "removed": removed.success }),
        || format!("Removed OAuth credentials for {}.", args.name),
    )
}

fn select_method<'a>(
    methods: &'a [AuthMethod],
    selector: Option<&str>,
) -> Result<(usize, &'a AuthMethod), String> {
    if methods.is_empty() {
        return Err("provider does not expose an authentication method".into());
    }
    if let Some(selector) = selector {
        if let Ok(index) = selector.parse::<usize>() {
            return methods
                .get(index)
                .map(|method| (index, method))
                .ok_or_else(|| format!("authentication method {index} does not exist"));
        }
        let needle = selector.to_ascii_lowercase();
        return methods
            .iter()
            .enumerate()
            .find(|(_, method)| {
                let kind = match method.kind {
                    AuthKind::Api => "api",
                    AuthKind::OAuth => "oauth",
                };
                kind == needle || method.label.to_ascii_lowercase().contains(&needle)
            })
            .ok_or_else(|| format!("authentication method {selector} does not exist"));
    }
    if methods.len() == 1 {
        return Ok((0, &methods[0]));
    }
    let choices = methods
        .iter()
        .enumerate()
        .map(|(index, method)| format!("  {index}: {}", method.label))
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!(
        "choose an authentication method with --method:\n{choices}"
    ))
}

fn provider_id(provider: &str) -> &str {
    if provider.eq_ignore_ascii_case("codex") {
        "openai"
    } else {
        provider
    }
}

fn auth_kind(auth: Option<&Value>) -> &'static str {
    match auth
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
    {
        Some("api") => "API key",
        Some("oauth") => "OAuth",
        Some("wellknown") => "environment",
        _ => "not configured",
    }
}

fn mcp_status_label(status: &McpStatus) -> String {
    match status {
        McpStatus::Connected => "connected".into(),
        McpStatus::Disabled => "disabled".into(),
        McpStatus::NeedsAuth => "authentication required".into(),
        McpStatus::NeedsClientRegistration { error } => {
            format!("client registration required: {error}")
        }
        McpStatus::Failed { error } => format!("failed: {error}"),
    }
}

fn prompt_secret(prompt: &str) -> Result<String, String> {
    eprint!("{prompt}");
    io::stderr().flush().map_err(|error| error.to_string())?;
    let mut value = String::new();
    io::stdin()
        .read_line(&mut value)
        .map_err(|error| error.to_string())?;
    let value = value.trim().to_string();
    if value.is_empty() {
        Err("API key cannot be empty".into())
    } else {
        Ok(value)
    }
}

fn open_browser(
    services: &neoism_agent_service_api::AgentServices,
    url: &str,
) -> Result<(), String> {
    let mut command = browser_command(services, url)?;
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn browser_command(
    services: &neoism_agent_service_api::AgentServices,
    url: &str,
) -> Result<std::process::Command, String> {
    #[cfg(target_os = "macos")]
    let launcher = "open";
    #[cfg(target_os = "windows")]
    let launcher = "cmd";
    #[cfg(all(unix, not(target_os = "macos")))]
    let launcher = "xdg-open";
    let launcher = crate::executable::resolve(
        services,
        launcher,
        neoism_agent_service_api::ExecutablePurpose::Browser,
        "browser launcher",
    )
    .map_err(|error| error.to_string())?;
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new(&launcher);
        command.arg(url);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new(&launcher);
        command.args(["/C", "start", "", url]);
        crate::windows_process::hide_std_command(&mut command);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    #[allow(unused_mut)]
    let mut command = {
        let mut command = std::process::Command::new(&launcher);
        command.arg(url);
        command
    };
    Ok(command)
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            byte => format!("%{byte:02X}"),
        })
        .collect()
}

fn mcp_auth_path(name: &str) -> String {
    format!(
        "/v2/plugins/dev.neoism.mcp/{}/auth",
        percent_encode(name)
    )
}

fn print_result<T: serde::Serialize>(
    output: &OutputArgs,
    value: T,
    human: impl FnOnce() -> String,
) -> Result<(), String> {
    if output.json {
        print_json(&value)
    } else {
        println!("{}", human());
        Ok(())
    }
}

fn print_json(value: &impl serde::Serialize) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|error| error.to_string())?
    );
    Ok(())
}

async fn get_json<T: DeserializeOwned>(server: &str, path: &str) -> Result<T, String> {
    response_json(
        reqwest::Client::new()
            .get(format!("{}{path}", normalize_server(server)))
            .send()
            .await,
    )
    .await
}

async fn post_json<T: DeserializeOwned>(
    server: &str,
    path: &str,
    body: &Value,
) -> Result<T, String> {
    response_json(
        reqwest::Client::new()
            .post(format!("{}{path}", normalize_server(server)))
            .json(body)
            .send()
            .await,
    )
    .await
}

async fn put_json<T: DeserializeOwned>(
    server: &str,
    path: &str,
    body: &Value,
) -> Result<T, String> {
    response_json(
        reqwest::Client::new()
            .put(format!("{}{path}", normalize_server(server)))
            .json(body)
            .send()
            .await,
    )
    .await
}

async fn delete_json<T: DeserializeOwned>(server: &str, path: &str) -> Result<T, String> {
    response_json(
        reqwest::Client::new()
            .delete(format!("{}{path}", normalize_server(server)))
            .send()
            .await,
    )
    .await
}

async fn response_json<T: DeserializeOwned>(
    response: Result<reqwest::Response, reqwest::Error>,
) -> Result<T, String> {
    let response =
        response.map_err(|error| format!("could not reach Neoism: {error}"))?;
    let status = response.status();
    let body = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format_http_error(status, &body));
    }
    serde_json::from_str(&body)
        .map_err(|error| format!("invalid Agent response: {error}"))
}

fn normalize_server(server: &str) -> String {
    server.trim_end_matches('/').to_string()
}

fn format_http_error(status: reqwest::StatusCode, body: &str) -> String {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("message").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| body.trim().to_string());
    format!("Neoism returned {status}: {message}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_launcher_honors_injected_path_and_reports_missing_launcher() {
        use crate::executable::test_support::FakeExecutableService;
        use std::path::PathBuf;
        use std::sync::Arc;

        #[cfg(target_os = "macos")]
        let launcher = "open";
        #[cfg(target_os = "windows")]
        let launcher = "cmd";
        #[cfg(all(unix, not(target_os = "macos")))]
        let launcher = "xdg-open";

        let injected = PathBuf::from("/injected/browser-launcher");
        let mut services = crate::standard_services();
        services.executables = Arc::new(FakeExecutableService::with(launcher, &injected));
        let command = browser_command(&services, "https://example.invalid").unwrap();
        assert_eq!(command.get_program(), injected.as_os_str());

        services.executables = Arc::new(FakeExecutableService::default());
        let error = browser_command(&services, "https://example.invalid")
            .unwrap_err()
            .to_string();
        assert!(error.contains("browser launcher executable"));
        assert!(error.contains("install it"));
    }

    #[test]
    fn parses_public_auth_commands() {
        assert!(Cli::try_parse_from(["neoism", "auth", "login", "codex"]).is_ok());
        assert!(Cli::try_parse_from(["neoism", "auth", "logout", "openai"]).is_ok());
        assert!(Cli::try_parse_from(["neoism", "mcp", "auth", "supabase"]).is_ok());
        assert!(Cli::try_parse_from(["neoism", "mcp", "auth", "list"]).is_ok());
    }

    #[test]
    fn builds_current_mcp_auth_route() {
        assert_eq!(
            mcp_auth_path("webflow"),
            "/v2/plugins/dev.neoism.mcp/webflow/auth"
        );
        assert_eq!(
            mcp_auth_path("company tools"),
            "/v2/plugins/dev.neoism.mcp/company%20tools/auth"
        );
    }

    #[test]
    fn formats_canonical_api_error() {
        let message = format_http_error(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"code":"request.invalid","message":"bad input","retryable":false,"details":{}}"#,
        );
        assert!(message.ends_with(": bad input"));
    }

    #[test]
    fn auth_cli_source_contains_no_deleted_http_routes() {
        let source = include_str!("auth_cli.rs");
        for suffix in ["provider", "auth/", "mcp/"] {
            let legacy = format!("\"/{suffix}");
            assert!(!source.contains(&legacy), "legacy auth CLI route remains: {legacy}");
        }
        assert!(!source.contains(&["/data", "/message"].concat()));
    }
}
