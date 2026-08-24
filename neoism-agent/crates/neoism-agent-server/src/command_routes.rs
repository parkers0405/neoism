use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use neoism_agent_core::CommandInfo;

use crate::error::ApiError;
use crate::state::AppState;
use crate::{config, resolve_directory, InstanceQuery};

pub(crate) async fn command_list(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<CommandInfo>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    let mut commands = Vec::new();
    if crate::plugins::enabled(&directory, "dev.neoism.commands") {
        for source in state.inner.plugin_host.snapshot().command_sources.values() {
            commands.extend(
                source
                    .list(&directory)
                    .map_err(|error| ApiError::internal(error.to_string()))?,
            );
        }
    }
    commands.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(Json(commands))
}

fn builtin_commands() -> Vec<CommandInfo> {
    vec![
        CommandInfo {
            name: "init".to_string(),
            description: Some("Create or refresh project agent instructions".to_string()),
            template: Some(
                "Analyze this project and write AGENTS.md guidance.".to_string(),
            ),
            agent: None,
            model: None,
            subtask: None,
        },
        CommandInfo {
            name: "summarize".to_string(),
            description: Some("Summarize the current session".to_string()),
            template: None,
            agent: None,
            model: None,
            subtask: None,
        },
    ]
}

pub(crate) fn load_commands(directory: &str) -> anyhow::Result<Vec<CommandInfo>> {
    let mut commands = builtin_commands();
    commands.extend(config::load(directory)?.info.command.into_values());
    Ok(commands)
}

pub(crate) fn find_command(
    state: &AppState,
    directory: &str,
    name: &str,
) -> anyhow::Result<Option<CommandInfo>> {
    if !crate::plugins::enabled(directory, "dev.neoism.commands") {
        return Ok(None);
    }
    for source in state.inner.plugin_host.snapshot().command_sources.values() {
        if let Some(command) = source
            .list(directory)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?
            .into_iter()
            .find(|command| command.name == name)
        {
            return Ok(Some(command));
        }
    }
    Ok(None)
}

pub(crate) fn expand_command_template(template: &str, arguments: &str) -> String {
    let args = command_arguments(arguments);
    let mut output = String::new();
    let mut chars = template.chars().peekable();
    let mut last_placeholder = 0usize;
    while let Some(ch) = chars.next() {
        if ch == '$' {
            let mut digits = String::new();
            while matches!(chars.peek(), Some(next) if next.is_ascii_digit()) {
                digits.push(chars.next().expect("peeked digit exists"));
            }
            if let Ok(position) = digits.parse::<usize>() {
                last_placeholder = last_placeholder.max(position);
            }
        }
    }

    let mut chars = template.chars().peekable();
    let mut used_index_placeholder = false;
    while let Some(ch) = chars.next() {
        if ch != '$' {
            output.push(ch);
            continue;
        }

        let mut digits = String::new();
        while matches!(chars.peek(), Some(next) if next.is_ascii_digit()) {
            digits.push(chars.next().expect("peeked digit exists"));
        }
        if digits.is_empty() {
            output.push('$');
            continue;
        }
        used_index_placeholder = true;
        let position = digits.parse::<usize>().unwrap_or(0);
        let index = position.saturating_sub(1);
        if index >= args.len() {
            continue;
        }
        if position == last_placeholder {
            output.push_str(&args[index..].join(" "));
        } else {
            output.push_str(&args[index]);
        }
    }

    let used_arguments_placeholder = output.contains("$ARGUMENTS");
    let mut output = output.replace("$ARGUMENTS", arguments);
    if !used_index_placeholder
        && !used_arguments_placeholder
        && !arguments.trim().is_empty()
    {
        output.push_str("\n\n");
        output.push_str(arguments);
    }
    output.trim().to_string()
}

pub(crate) fn command_arguments(arguments: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut chars = arguments.chars().peekable();
    while chars.peek().is_some() {
        while matches!(chars.peek(), Some(ch) if ch.is_whitespace()) {
            let _ = chars.next();
        }
        let Some(ch) = chars.peek().copied() else {
            break;
        };
        if ch == '[' {
            let mut token = String::new();
            for next in chars.by_ref() {
                token.push(next);
                if next == ']' {
                    break;
                }
            }
            if token.starts_with("[Image ") && token.ends_with(']') {
                args.push(token);
                continue;
            }
            args.push(token);
            continue;
        }
        if ch == '"' || ch == '\'' {
            let quote = chars.next().expect("peeked quote exists");
            let mut token = String::new();
            for next in chars.by_ref() {
                if next == quote {
                    break;
                }
                token.push(next);
            }
            args.push(token);
            continue;
        }

        let mut token = String::new();
        while let Some(next) = chars.peek().copied() {
            if next.is_whitespace() {
                break;
            }
            token.push(chars.next().expect("peeked char exists"));
        }
        if !token.is_empty() {
            args.push(token);
        }
    }
    args
}
