use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

use super::paths::display_path;

#[derive(Clone, Debug)]
struct FormatterCommand {
    name: String,
    extensions: Vec<String>,
    command: Vec<String>,
}

pub(super) fn format_paths(
    services: &neoism_agent_service_api::AgentServices,
    cwd: &Path,
    config: Option<&Value>,
    paths: impl IntoIterator<Item = PathBuf>,
) -> Vec<String> {
    let Some(config) = config.filter(formatting_enabled) else {
        return Vec::new();
    };
    let mut formatted = Vec::new();
    let mut seen = BTreeSet::new();
    let commands = formatter_commands(services, cwd, config);

    for path in paths {
        let path = if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        };
        if !path.is_file() || !seen.insert(path.clone()) {
            continue;
        }
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| format!(".{ext}"))
            .unwrap_or_default();
        if extension.is_empty() {
            continue;
        }
        let mut changed = false;
        for formatter in commands
            .iter()
            .filter(|formatter| formatter.extensions.iter().any(|ext| ext == &extension))
        {
            if run_formatter(services, cwd, &path, formatter) {
                changed = true;
            }
        }
        if changed {
            formatted.push(display_path(cwd, &path));
        }
    }

    formatted
}

pub(super) fn attach_formatted(metadata: &mut Value, formatted: &[String]) {
    if formatted.is_empty() {
        return;
    }
    if let Some(object) = metadata.as_object_mut() {
        object.insert("formatted".to_string(), json!(formatted));
    }
}

fn formatting_enabled(config: &&Value) -> bool {
    match config {
        Value::Bool(enabled) => *enabled,
        Value::Object(map) => !map
            .get("disabled")
            .or_else(|| map.get("disable"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        _ => false,
    }
}

fn formatter_commands(
    services: &neoism_agent_service_api::AgentServices,
    cwd: &Path,
    config: &Value,
) -> Vec<FormatterCommand> {
    let mut commands = builtin_formatters(services, cwd, config);
    if let Value::Object(map) = config {
        for (name, value) in map {
            if matches!(name.as_str(), "disabled" | "disable") {
                continue;
            }
            if formatter_disabled(value) {
                continue;
            }
            let Some(command) = custom_command(value) else {
                continue;
            };
            let extensions = value
                .get("extensions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(normalize_extension)
                .filter(|ext| !ext.is_empty())
                .collect::<Vec<_>>();
            if extensions.is_empty() {
                continue;
            }
            commands.push(FormatterCommand {
                name: name.clone(),
                extensions,
                command,
            });
        }
    }
    commands
}

fn builtin_formatters(
    services: &neoism_agent_service_api::AgentServices,
    cwd: &Path,
    config: &Value,
) -> Vec<FormatterCommand> {
    let mut commands = Vec::new();
    push_builtin(
        &mut commands,
        config,
        "rustfmt",
        &[".rs"],
        which(services, "rustfmt").map(|bin| vec![bin, "$FILE".to_string()]),
    );
    push_builtin(
        &mut commands,
        config,
        "gofmt",
        &[".go"],
        which(services, "gofmt")
            .map(|bin| vec![bin, "-w".to_string(), "$FILE".to_string()]),
    );
    push_builtin(
        &mut commands,
        config,
        "zig",
        &[".zig", ".zon"],
        which(services, "zig")
            .map(|bin| vec![bin, "fmt".to_string(), "$FILE".to_string()]),
    );
    if find_up(cwd, ".clang-format").is_some() {
        push_builtin(
            &mut commands,
            config,
            "clang-format",
            &[
                ".c", ".cc", ".cpp", ".cxx", ".c++", ".h", ".hh", ".hpp", ".hxx", ".h++",
                ".ino", ".C", ".H",
            ],
            which(services, "clang-format")
                .map(|bin| vec![bin, "-i".to_string(), "$FILE".to_string()]),
        );
    }
    if project_mentions(cwd, &["package.json"], &["prettier"]) {
        push_builtin(
            &mut commands,
            config,
            "prettier",
            &[
                ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts", ".html",
                ".htm", ".css", ".scss", ".sass", ".less", ".vue", ".svelte", ".json",
                ".jsonc", ".yaml", ".yml", ".toml", ".xml", ".md", ".mdx", ".graphql",
                ".gql",
            ],
            which(services, "prettier")
                .map(|bin| vec![bin, "--write".to_string(), "$FILE".to_string()]),
        );
    }
    if find_up(cwd, "biome.json")
        .or_else(|| find_up(cwd, "biome.jsonc"))
        .is_some()
    {
        push_builtin(
            &mut commands,
            config,
            "biome",
            &[
                ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts", ".html",
                ".htm", ".css", ".scss", ".sass", ".less", ".vue", ".svelte", ".json",
                ".jsonc", ".yaml", ".yml", ".toml", ".xml", ".md", ".mdx", ".graphql",
                ".gql",
            ],
            which(services, "biome").map(|bin| {
                vec![
                    bin,
                    "format".to_string(),
                    "--write".to_string(),
                    "$FILE".to_string(),
                ]
            }),
        );
    }
    if find_up(cwd, "ruff.toml")
        .or_else(|| find_up(cwd, ".ruff.toml"))
        .is_some()
        || project_mentions(
            cwd,
            &["pyproject.toml", "requirements.txt", "Pipfile"],
            &["ruff", "[tool.ruff]"],
        )
    {
        push_builtin(
            &mut commands,
            config,
            "ruff",
            &[".py", ".pyi"],
            which(services, "ruff")
                .map(|bin| vec![bin, "format".to_string(), "$FILE".to_string()]),
        );
    }
    commands
}

fn push_builtin(
    commands: &mut Vec<FormatterCommand>,
    config: &Value,
    name: &str,
    extensions: &[&str],
    command: Option<Vec<String>>,
) {
    if !formatter_enabled_for_name(config, name) {
        return;
    }
    let Some(command) = command else {
        return;
    };
    commands.push(FormatterCommand {
        name: name.to_string(),
        extensions: extensions.iter().map(|ext| (*ext).to_string()).collect(),
        command,
    });
}

fn formatter_enabled_for_name(config: &Value, name: &str) -> bool {
    match config {
        Value::Bool(enabled) => *enabled,
        Value::Object(map) => map.get(name).map(formatter_disabled) != Some(true),
        _ => false,
    }
}

fn formatter_disabled(value: &Value) -> bool {
    match value {
        Value::Bool(enabled) => !enabled,
        Value::Object(map) => map
            .get("disabled")
            .or_else(|| map.get("disable"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        _ => false,
    }
}

fn custom_command(value: &Value) -> Option<Vec<String>> {
    let command = value.get("command")?;
    match command {
        Value::Array(items) => {
            let parts = items
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            (!parts.is_empty()).then_some(parts)
        }
        Value::String(command) => {
            let parts = command
                .split_whitespace()
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            (!parts.is_empty()).then_some(parts)
        }
        _ => None,
    }
}

fn run_formatter(
    services: &neoism_agent_service_api::AgentServices,
    cwd: &Path,
    path: &Path,
    formatter: &FormatterCommand,
) -> bool {
    let Some((program, args)) = formatter.command.split_first() else {
        return false;
    };
    let file = path.display().to_string();
    let request = neoism_agent_service_api::ExecutableRequest::new(
        program,
        neoism_agent_service_api::ExecutablePurpose::Formatter,
    );
    let Ok(resolved) = services.executables.resolve(&request) else {
        return false;
    };
    let mut command = Command::new(resolved.path);
    command
        .args(args.iter().map(|arg| arg.replace("$FILE", &file)))
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::tool::process::set_new_process_group_std(&mut command);
    let status = command.status();
    match status {
        Ok(status) if status.success() => true,
        Ok(status) => {
            tracing::debug!(
                formatter = formatter.name,
                status = status.code(),
                file = %path.display(),
                "formatter exited unsuccessfully"
            );
            false
        }
        Err(error) => {
            tracing::debug!(
                formatter = formatter.name,
                error = %error,
                file = %path.display(),
                "failed to run formatter"
            );
            false
        }
    }
}

fn normalize_extension(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return String::new();
    }
    if value.starts_with('.') {
        value.to_string()
    } else {
        format!(".{value}")
    }
}

fn which(
    services: &neoism_agent_service_api::AgentServices,
    program: &str,
) -> Option<String> {
    let request = neoism_agent_service_api::ExecutableRequest::new(
        program,
        neoism_agent_service_api::ExecutablePurpose::Formatter,
    );
    services
        .executables
        .resolve(&request)
        .ok()
        .map(|result| result.path.display().to_string())
}

fn find_up(start: &Path, name: &str) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn project_mentions(cwd: &Path, names: &[&str], needles: &[&str]) -> bool {
    names.iter().any(|name| {
        find_up(cwd, name)
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|content| needles.iter().any(|needle| content.contains(needle)))
            .unwrap_or(false)
    })
}
