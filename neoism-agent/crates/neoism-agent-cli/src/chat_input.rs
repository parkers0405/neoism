use std::path::Path;
use std::time::{Duration, Instant};

use base64::Engine;
use neoism_agent_core::PromptPart;
use serde_json::Value;

use crate::chat_commands::{ChatCommandSpec, CHAT_COMMANDS};
use crate::chat_ui::{read_key, BottomPrompt, Key};
use crate::{request_with_dir, response_json, split_model_ref};

#[path = "chat_input_completion.rs"]
mod chat_input_completion;
use chat_input_completion::{
    file_url, update_completion, update_slash_completion, CompletionKind,
};
pub(crate) use chat_input_completion::{CompletionMenu, CompletionMode};

const MAX_INLINE_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct AgentChoice {
    pub(crate) name: String,
    pub(crate) mode: String,
}

pub(crate) async fn fetch_agent_choices(
    client: &reqwest::Client,
    server: &str,
    dir: Option<&str>,
) -> anyhow::Result<Vec<AgentChoice>> {
    let value = response_json(
        request_with_dir(client.get(format!("{server}/v2/agents")), dir)
            .send()
            .await?,
    )
    .await?;
    let mut agents = value
        .as_array()
        .into_iter()
        .flatten()
        .filter(|agent| {
            !agent
                .get("hidden")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|agent| {
            Some(AgentChoice {
                name: agent.get("name").and_then(Value::as_str)?.to_string(),
                mode: agent
                    .get("mode")
                    .and_then(Value::as_str)
                    .unwrap_or("all")
                    .to_string(),
            })
        })
        .collect::<Vec<_>>();
    agents.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(agents)
}

#[derive(Clone, Debug)]
pub(crate) struct ChatPromptInput {
    text: String,
    files: Vec<MentionedFile>,
}

impl ChatPromptInput {
    pub(crate) fn text(text: String) -> Self {
        Self {
            text,
            files: Vec::new(),
        }
    }

    pub(crate) fn visible_text(&self) -> &str {
        &self.text
    }

    pub(crate) fn into_parts(self) -> Vec<PromptPart> {
        let mut parts = vec![PromptPart::Text { text: self.text }];
        parts.extend(self.files.into_iter().map(|file| PromptPart::File {
            url: file.url,
            filename: file.filename,
            mime: file.mime,
        }));
        parts
    }
}

#[derive(Clone, Debug)]
struct MentionedFile {
    token: String,
    filename: String,
    url: String,
    mime: String,
}

struct PromptDraft {
    text: String,
    files: Vec<MentionedFile>,
    menu: Option<CompletionMenu>,
}

impl PromptDraft {
    fn new() -> Self {
        Self {
            text: String::new(),
            files: Vec::new(),
            menu: None,
        }
    }

    fn input(self) -> ChatPromptInput {
        let files = self
            .files
            .into_iter()
            .filter(|file| self.text.contains(&file.token))
            .collect();
        ChatPromptInput {
            text: self.text,
            files,
        }
    }
}

pub(crate) enum PromptRead {
    Submit(ChatPromptInput),
    Command(String),
    CycleAgent,
    Quit,
}

pub(crate) fn chat_footer_label(
    current_model: &Option<String>,
    current_agent: &Option<String>,
    current_variant: &Option<String>,
    root: &Path,
    context_usage: Option<&str>,
) -> String {
    let (provider, model) = current_model
        .as_deref()
        .and_then(split_model_ref)
        .unwrap_or_else(|| ("provider:default".to_string(), "model:default".to_string()));
    let base = format!(
        "{} {} · {} · {} [{}]",
        model,
        current_variant.as_deref().unwrap_or("default"),
        compact_path(root),
        titlecase(current_agent.as_deref().unwrap_or("build")),
        provider
    );
    match context_usage {
        Some(context_usage) if !context_usage.trim().is_empty() => {
            format!("{base} · {context_usage}")
        }
        _ => base,
    }
}

fn compact_path(path: &Path) -> String {
    let display = path.to_string_lossy().replace('\\', "/");
    let Some(home) = std::env::var_os("HOME") else {
        return display;
    };
    let home = home.to_string_lossy().replace('\\', "/");
    if display == home {
        return "~".to_string();
    }
    display
        .strip_prefix(&(home + "/"))
        .map(|rest| format!("~/{rest}"))
        .unwrap_or(display)
}

fn titlecase(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    format!("{}{}", first.to_uppercase(), chars.as_str())
}

pub(crate) fn cycle_agent(current_agent: &mut Option<String>, agents: &[AgentChoice]) {
    let primary = agents
        .iter()
        .filter(|agent| agent.mode == "primary" || agent.mode == "all")
        .map(|agent| agent.name.clone())
        .collect::<Vec<_>>();
    if primary.is_empty() {
        return;
    }
    let current = current_agent.as_deref().unwrap_or("build");
    let next = primary
        .iter()
        .position(|agent| agent == current)
        .map(|index| (index + 1) % primary.len())
        .unwrap_or(0);
    *current_agent = Some(primary[next].clone());
}

pub(crate) fn update_streaming_completion(
    text: &str,
    previous_selected: Option<usize>,
) -> Option<CompletionMenu> {
    update_slash_completion(text, previous_selected)
}

pub(crate) fn accept_streaming_completion(
    text: &mut String,
    menu: &mut Option<CompletionMenu>,
    execute_if_immediate: bool,
) -> Option<String> {
    let selected = menu.as_ref()?.selected_command()?;
    *menu = None;
    *text = selected.replacement;
    if selected.execute_immediately {
        return execute_if_immediate.then(|| text.clone());
    }
    if !text.ends_with(' ') {
        text.push(' ');
    }
    None
}

pub(crate) fn read_prompt_raw(
    ui: &mut BottomPrompt,
    root: &Path,
    agents: &[AgentChoice],
    right: &str,
    initial: String,
) -> anyhow::Result<PromptRead> {
    let mut draft = PromptDraft::new();
    draft.text = initial;
    let mut last_ctrl_c: Option<Instant> = None;
    let mut footer_override: Option<String> = None;
    loop {
        let previous_selected = draft.menu.as_ref().map(|menu| menu.selected);
        draft.menu = update_completion(&draft.text, previous_selected, root, agents);
        let footer = footer_override.as_deref().unwrap_or(right);
        ui.render_prompt(&draft.text, draft.menu.as_ref(), footer)?;
        let key = read_key()?;
        if !matches!(key, Key::CtrlC | Key::CtrlD) {
            last_ctrl_c = None;
            footer_override = None;
        }
        match key {
            Key::Char(ch) => draft.text.push(ch),
            Key::Backspace => {
                draft.text.pop();
            }
            Key::Enter => {
                if let Some(action) = accept_or_submit(&mut draft)? {
                    return Ok(action);
                }
            }
            Key::Tab => {
                if draft.menu.is_some() {
                    accept_completion(&mut draft)?;
                } else {
                    return Ok(PromptRead::CycleAgent);
                }
            }
            Key::Right => accept_completion(&mut draft)?,
            Key::Up => {
                if let Some(menu) = draft.menu.as_mut() {
                    menu.move_previous();
                }
            }
            Key::Down => {
                if let Some(menu) = draft.menu.as_mut() {
                    menu.move_next();
                }
            }
            Key::Esc => draft.menu = None,
            Key::CtrlO => {
                ui.expand_pending()?;
            }
            Key::CtrlP => {
                return Ok(PromptRead::Command("/think".to_string()));
            }
            Key::CtrlC | Key::CtrlD => {
                if !draft.text.is_empty() {
                    draft = PromptDraft::new();
                    last_ctrl_c = None;
                    footer_override = None;
                } else if last_ctrl_c
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
                {
                    return Ok(PromptRead::Quit);
                } else {
                    last_ctrl_c = Some(Instant::now());
                    footer_override = Some("press ctrl+c again to exit".to_string());
                }
            }
            Key::Left => {}
        }
    }
}

fn accept_or_submit(draft: &mut PromptDraft) -> anyhow::Result<Option<PromptRead>> {
    if draft.text.trim().is_empty() {
        return Ok(None);
    }
    if draft.text.starts_with('/') {
        if exact_slash_command(&draft.text).is_some() {
            return Ok(Some(PromptRead::Command(draft.text.clone())));
        }
        if draft.menu.is_some() {
            if let Some(command) = immediate_selected_command(draft) {
                return Ok(Some(PromptRead::Command(command)));
            }
            accept_completion(draft)?;
            return Ok(None);
        }
        return Ok(Some(PromptRead::Command(draft.text.clone())));
    }
    if draft.menu.is_some() {
        accept_completion(draft)?;
        return Ok(None);
    }
    let input = std::mem::replace(draft, PromptDraft::new()).input();
    Ok(Some(PromptRead::Submit(input)))
}

fn immediate_selected_command(draft: &PromptDraft) -> Option<String> {
    let menu = draft.menu.as_ref()?;
    let option = menu.options.get(menu.selected)?;
    match option.completion {
        CompletionKind::Command {
            execute_immediately: true,
        } => Some(option.replacement.clone()),
        _ => None,
    }
}

fn accept_completion(draft: &mut PromptDraft) -> anyhow::Result<()> {
    let Some(menu) = draft.menu.take() else {
        return Ok(());
    };
    let Some(option) = menu.options.get(menu.selected).cloned() else {
        return Ok(());
    };
    match option.completion {
        CompletionKind::Command {
            execute_immediately,
        } => {
            draft.text = option.replacement;
            if !execute_immediately && !draft.text.ends_with(' ') {
                draft.text.push(' ');
            }
        }
        CompletionKind::Agent => {
            draft
                .text
                .replace_range(menu.trigger.., &option.replacement);
        }
        CompletionKind::File { path } => {
            draft
                .text
                .replace_range(menu.trigger.., &option.replacement);
            let mime = mime_for_path(&path);
            draft.files.push(MentionedFile {
                token: option.replacement.trim_end().to_string(),
                filename: option
                    .replacement
                    .trim()
                    .trim_start_matches('@')
                    .to_string(),
                url: attachment_url_for_path(&path, mime),
                mime: mime.to_string(),
            });
        }
    }
    Ok(())
}

fn attachment_url_for_path(path: &Path, mime: &str) -> String {
    if attachment_mime_can_inline(mime) {
        if let Ok(metadata) = std::fs::metadata(path) {
            if metadata.len() <= MAX_INLINE_ATTACHMENT_BYTES {
                if let Ok(bytes) = std::fs::read(path) {
                    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
                    return format!("data:{mime};base64,{encoded}");
                }
            }
        }
    }
    file_url(path)
}

fn attachment_mime_can_inline(mime: &str) -> bool {
    mime.starts_with("image/") || mime == "application/pdf"
}

fn mime_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "toml" => "application/toml",
        "yaml" | "yml" => "application/yaml",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" | "cjs" => "text/javascript",
        "ts" | "tsx" => "text/typescript",
        "rs" | "py" | "go" | "java" | "c" | "h" | "cpp" | "hpp" | "cs" | "rb" | "php"
        | "swift" | "kt" | "kts" | "sh" | "bash" | "zsh" | "fish" | "sql" | "xml"
        | "txt" => "text/plain",
        _ => "text/plain",
    }
}

fn exact_slash_command(text: &str) -> Option<&'static ChatCommandSpec> {
    let command = text.split_whitespace().next()?;
    CHAT_COMMANDS.iter().find(|spec| {
        spec.names
            .iter()
            .any(|name| *name != "/" && *name == command)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_file_mentions_use_extension_mime_types() {
        assert_eq!(mime_for_path(Path::new("screenshot.png")), "image/png");
        assert_eq!(mime_for_path(Path::new("report.pdf")), "application/pdf");
        assert_eq!(mime_for_path(Path::new("TASK.md")), "text/markdown");
        assert_eq!(mime_for_path(Path::new("src/lib.rs")), "text/plain");
    }

    #[test]
    fn streaming_slash_completion_can_execute_immediate_command() {
        let mut text = "/abo".to_string();
        let mut menu = update_streaming_completion(&text, None);
        let command = accept_streaming_completion(&mut text, &mut menu, true);
        assert_eq!(command.as_deref(), Some("/abort"));
        assert_eq!(text, "/abort");
    }

    #[test]
    fn streaming_slash_completion_fills_commands_with_arguments() {
        let mut text = "/que".to_string();
        let mut menu = update_streaming_completion(&text, None);
        let queue_index = menu
            .as_ref()
            .and_then(|menu| {
                menu.options
                    .iter()
                    .position(|option| option.display.starts_with("/queue"))
            })
            .expect("/queue should be present in slash completion results");
        if let Some(menu) = menu.as_mut() {
            menu.selected = queue_index;
        }
        let command = accept_streaming_completion(&mut text, &mut menu, true);
        assert_eq!(command, None);
        assert_eq!(text, "/queue ");
    }

    #[test]
    fn media_file_mentions_are_inlined_as_data_urls() {
        let path = std::env::temp_dir().join(format!(
            "neoism-agent-chat-input-media-{}.png",
            std::process::id()
        ));
        std::fs::write(&path, b"png bytes").unwrap();

        let url = attachment_url_for_path(&path, "image/png");

        assert!(url.starts_with("data:image/png;base64,"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn text_file_mentions_keep_file_urls() {
        let path = std::env::temp_dir().join(format!(
            "neoism-agent-chat-input-text-{}.rs",
            std::process::id()
        ));
        std::fs::write(&path, b"fn main() {}\n").unwrap();

        let url = attachment_url_for_path(&path, "text/plain");

        assert!(url.starts_with("file://"));
        let _ = std::fs::remove_file(path);
    }
}
