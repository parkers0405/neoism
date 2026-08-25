use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::chat_command_handlers::{
    apply_session_state, handle_chat_command, CommandOutcome,
};
use crate::chat_header::{print_chat_header, print_session_bootstrap};
use crate::chat_input::{
    chat_footer_label, cycle_agent, fetch_agent_choices, read_prompt_raw, AgentChoice,
    ChatPromptInput, PromptRead,
};
use crate::chat_session::{
    create_cli_session, fetch_context_usage_label, normalize_model_ref,
    user_model_from_cli_model,
};
use crate::chat_stream::{attach_chat_session, stream_chat_prompt, StreamOutcome};
use crate::chat_ui::print_user_prompt;
use crate::chat_ui::{stdin_is_tty, BottomPrompt, RawTerminal};
use crate::{normalize_server, response_json, split_model_ref, BOLD, CYAN, DIM, RESET};

pub(crate) async fn chat(
    server: String,
    session: Option<String>,
    dir: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    agent: Option<String>,
    variant: Option<String>,
) -> anyhow::Result<()> {
    let server = normalize_server(&server);
    let client = reqwest::Client::new();
    let mut current_model = match (provider, model) {
        (None, None) => None,
        (None, Some(model)) => Some(normalize_model_ref(&model, "openai")),
        (Some(provider), None) => Some(format!("{provider}/gpt-5.5")),
        (Some(provider), Some(model)) => {
            let model = split_model_ref(&model)
                .map(|(_, model)| model)
                .unwrap_or(model);
            Some(format!("{provider}/{model}"))
        }
    };
    let mut current_agent = agent;
    let mut current_variant = variant;
    let mut session_id = match session {
        Some(session) => session,
        None => {
            create_cli_session(
                &client,
                &server,
                dir.as_deref(),
                current_model.as_deref(),
                current_agent.as_deref(),
                current_variant.as_deref(),
            )
            .await?
        }
    };

    let agents = fetch_agent_choices(&client, &server, dir.as_deref())
        .await
        .unwrap_or_default();
    let cwd_display = dir.clone().or_else(|| {
        std::env::current_dir()
            .ok()
            .map(|path| path.to_string_lossy().to_string())
    });
    if stdin_is_tty() {
        print_chat_header(
            &session_id,
            current_model.as_deref(),
            current_agent.as_deref(),
            current_variant.as_deref(),
            cwd_display.as_deref(),
        );
        let bootstrap = print_session_bootstrap(&client, &server, &session_id).await?;
        if bootstrap.should_attach {
            attach_chat_session(&client, &server, &session_id, None).await?;
        }
        chat_raw_loop(
            &client,
            &server,
            dir.as_deref(),
            &mut session_id,
            &mut current_model,
            &mut current_agent,
            &mut current_variant,
            &agents,
        )
        .await?;
        return Ok(());
    }

    print_chat_header(
        &session_id,
        current_model.as_deref(),
        current_agent.as_deref(),
        current_variant.as_deref(),
        cwd_display.as_deref(),
    );
    let bootstrap = print_session_bootstrap(&client, &server, &session_id).await?;
    if bootstrap.should_attach {
        attach_chat_session(&client, &server, &session_id, None).await?;
    }
    chat_line_loop(
        &client,
        &server,
        dir.as_deref(),
        &mut session_id,
        &mut current_model,
        &mut current_agent,
        &mut current_variant,
    )
    .await
}

async fn chat_line_loop(
    client: &reqwest::Client,
    server: &str,
    dir: Option<&str>,
    session_id: &mut String,
    current_model: &mut Option<String>,
    current_agent: &mut Option<String>,
    current_variant: &mut Option<String>,
) -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    loop {
        print!("{CYAN} >>> {RESET} ");
        std::io::stdout().flush()?;
        let Some(line) = lines.next() else {
            println!();
            break;
        };
        let line = line?;
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        if input.starts_with('/') {
            match handle_chat_command(
                client,
                server,
                dir,
                session_id,
                current_model,
                current_agent,
                current_variant,
                input,
                None,
            )
            .await?
            {
                CommandOutcome::Continue => {}
                CommandOutcome::Exit => break,
                CommandOutcome::Attach => {
                    attach_chat_session(client, server, session_id, None).await?;
                }
                CommandOutcome::Prefill(text) => {
                    if !text.trim().is_empty() {
                        println!("prefill: {text}");
                    }
                }
            }
            continue;
        }

        let prompt_model = current_model
            .as_deref()
            .map(|model| user_model_from_cli_model(model, current_variant.as_deref()))
            .transpose()?;
        stream_chat_prompt(
            client,
            server,
            session_id,
            prompt_model,
            current_agent.clone(),
            ChatPromptInput::text(input.to_string()),
            None,
        )
        .await?;
    }
    Ok(())
}

async fn chat_raw_loop(
    client: &reqwest::Client,
    server: &str,
    dir: Option<&str>,
    session_id: &mut String,
    current_model: &mut Option<String>,
    current_agent: &mut Option<String>,
    current_variant: &mut Option<String>,
    agents: &[AgentChoice],
) -> anyhow::Result<()> {
    let _raw = RawTerminal::enter()?;
    let root = dir
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let mut ui = BottomPrompt::new();
    let mut footer = chat_footer(
        client,
        server,
        session_id,
        current_model,
        current_agent,
        current_variant,
        &root,
    )
    .await;
    ui.render_prompt("", None, &footer)?;
    let mut next_initial = String::new();

    loop {
        let initial = std::mem::take(&mut next_initial);
        match read_prompt_raw(&mut ui, &root, agents, &footer, initial)? {
            PromptRead::Quit => break,
            PromptRead::CycleAgent => {
                cycle_agent(current_agent, agents);
                footer = chat_footer(
                    client,
                    server,
                    session_id,
                    current_model,
                    current_agent,
                    current_variant,
                    &root,
                )
                .await;
                ui.render_prompt("", None, &footer)?;
            }
            PromptRead::Command(command) => {
                ui.before_output()?;
                match handle_chat_command(
                    client,
                    server,
                    dir,
                    session_id,
                    current_model,
                    current_agent,
                    current_variant,
                    command.trim(),
                    Some(&mut ui),
                )
                .await?
                {
                    CommandOutcome::Continue => {}
                    CommandOutcome::Exit => break,
                    CommandOutcome::Attach => {
                        let outcome = attach_chat_session(
                            client,
                            server,
                            session_id,
                            Some(&mut ui),
                        )
                        .await?;
                        let outcome = follow_stream_switches(
                            client,
                            server,
                            session_id,
                            current_model,
                            current_agent,
                            current_variant,
                            &mut ui,
                            outcome,
                        )
                        .await?;
                        if !outcome.leftover_input.trim().is_empty() {
                            next_initial = outcome.leftover_input;
                        }
                    }
                    CommandOutcome::Prefill(text) => {
                        next_initial = text;
                    }
                }
                footer = chat_footer(
                    client,
                    server,
                    session_id,
                    current_model,
                    current_agent,
                    current_variant,
                    &root,
                )
                .await;
                ui.render_prompt(&next_initial, None, &footer)?;
            }
            PromptRead::Submit(prompt) => {
                ui.before_output()?;
                print_user_prompt(prompt.visible_text());
                let mut prompt_queue = VecDeque::from([prompt]);
                let mut leftover = String::new();
                let mut exit_after_queue = false;
                while let Some(prompt) = prompt_queue.pop_front() {
                    let visible = prompt.visible_text().trim().to_string();
                    if visible.starts_with('/') {
                        match handle_chat_command(
                            client,
                            server,
                            dir,
                            session_id,
                            current_model,
                            current_agent,
                            current_variant,
                            &visible,
                            Some(&mut ui),
                        )
                        .await?
                        {
                            CommandOutcome::Continue => {}
                            CommandOutcome::Exit => {
                                exit_after_queue = true;
                                break;
                            }
                            CommandOutcome::Attach => {
                                let outcome = attach_chat_session(
                                    client,
                                    server,
                                    session_id,
                                    Some(&mut ui),
                                )
                                .await?;
                                let outcome = follow_stream_switches(
                                    client,
                                    server,
                                    session_id,
                                    current_model,
                                    current_agent,
                                    current_variant,
                                    &mut ui,
                                    outcome,
                                )
                                .await?;
                                leftover = outcome.leftover_input;
                                prompt_queue.extend(
                                    outcome.queued.into_iter().map(ChatPromptInput::text),
                                );
                                if let Some(queued_input) = prompt_queue.front() {
                                    print_user_prompt(queued_input.visible_text());
                                }
                            }
                            CommandOutcome::Prefill(text) => {
                                leftover = text;
                            }
                        }
                        continue;
                    }
                    let prompt_model = current_model
                        .as_deref()
                        .map(|model| {
                            user_model_from_cli_model(model, current_variant.as_deref())
                        })
                        .transpose()?;
                    let outcome = stream_chat_prompt(
                        client,
                        server,
                        session_id,
                        prompt_model,
                        current_agent.clone(),
                        prompt,
                        Some(&mut ui),
                    )
                    .await?;
                    let outcome = follow_stream_switches(
                        client,
                        server,
                        session_id,
                        current_model,
                        current_agent,
                        current_variant,
                        &mut ui,
                        outcome,
                    )
                    .await?;
                    leftover = outcome.leftover_input;
                    prompt_queue
                        .extend(outcome.queued.into_iter().map(ChatPromptInput::text));
                    if let Some(queued_input) = prompt_queue.front() {
                        print_user_prompt(queued_input.visible_text());
                    }
                }
                if exit_after_queue {
                    break;
                }
                footer = chat_footer(
                    client,
                    server,
                    session_id,
                    current_model,
                    current_agent,
                    current_variant,
                    &root,
                )
                .await;
                next_initial = leftover;
                ui.render_prompt(&next_initial, None, &footer)?;
            }
        }
    }
    ui.clear_overlay()?;
    println!();
    Ok(())
}

async fn follow_stream_switches(
    client: &reqwest::Client,
    server: &str,
    session_id: &mut String,
    current_model: &mut Option<String>,
    current_agent: &mut Option<String>,
    current_variant: &mut Option<String>,
    ui: &mut BottomPrompt,
    mut outcome: StreamOutcome,
) -> anyhow::Result<StreamOutcome> {
    while let Some(next_session_id) = outcome.switch_session.take() {
        let value = response_json(
            client
                .get(format!("{server}/v2/sessions/{next_session_id}"))
                .send()
                .await?,
        )
        .await?;
        *session_id = next_session_id;
        apply_session_state(&value, current_model, current_agent, current_variant);
        ui.before_output()?;
        println!();
        println!("{BOLD}{CYAN}›{RESET} {DIM}session{RESET} {BOLD}{session_id}{RESET}");
        let mut next = attach_chat_session(client, server, session_id, Some(ui)).await?;
        if !outcome.leftover_input.trim().is_empty()
            && next.leftover_input.trim().is_empty()
        {
            next.leftover_input = outcome.leftover_input;
        }
        if !outcome.queued.is_empty() {
            let mut queued = outcome.queued;
            queued.extend(next.queued);
            next.queued = queued;
        }
        outcome = next;
    }
    Ok(outcome)
}

async fn chat_footer(
    client: &reqwest::Client,
    server: &str,
    session_id: &str,
    current_model: &Option<String>,
    current_agent: &Option<String>,
    current_variant: &Option<String>,
    root: &PathBuf,
) -> String {
    let context_usage = fetch_context_usage_label(client, server, session_id)
        .await
        .unwrap_or(None);
    chat_footer_label(
        current_model,
        current_agent,
        current_variant,
        root,
        context_usage.as_deref(),
    )
}

#[cfg(test)]
#[path = "chat_tests.rs"]
mod tests;
