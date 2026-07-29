use std::collections::VecDeque;
use std::time::Duration;

use neoism_agent_core::{PromptRequest, UserModel};

use crate::chat_blockers::{
    parse_permission_reply_and_id, submit_permission_reply, submit_question_answer,
    submit_question_reject, submit_stream_blocker, StreamBlocker,
};
use crate::chat_input::{
    accept_streaming_completion, update_streaming_completion, ChatPromptInput,
    CompletionMenu,
};
use crate::chat_picker::fetch_subagent_sessions;
use crate::chat_render::ChatRenderState;
use crate::chat_session::{
    ensure_empty_response, ensure_success_response, fetch_permission_requests,
    fetch_question_requests, latest_assistant_text,
};
use crate::chat_sse::handle_chat_sse_event;
use crate::chat_status::{first_pending_id, pending_id_or_first};
use crate::chat_ui::BottomPrompt;
use crate::{BOLD, RESET};

pub(crate) struct StreamOutcome {
    pub(crate) queued: VecDeque<String>,
    pub(crate) leftover_input: String,
    pub(crate) switch_session: Option<String>,
}

pub(crate) async fn stream_chat_prompt(
    client: &reqwest::Client,
    server: &str,
    session_id: &str,
    model: Option<UserModel>,
    agent: Option<String>,
    prompt: ChatPromptInput,
    ui: Option<&mut BottomPrompt>,
) -> anyhow::Result<StreamOutcome> {
    let event_response =
        ensure_success_response(client.get(format!("{server}/event")).send().await?)
            .await?;
    let prompt_parts = prompt.into_parts();
    ensure_empty_response(
        client
            .post(format!("{server}/session/{session_id}/prompt_async"))
            .json(&PromptRequest {
                message_id: None,
                model,
                agent,
                no_reply: false,
                system: None,
                tools: None,
                author: None,
                parts: prompt_parts,
            })
            .send()
            .await?,
    )
    .await?;

    stream_session_events(client, server, session_id, event_response, ui, false).await
}

pub(crate) async fn attach_chat_session(
    client: &reqwest::Client,
    server: &str,
    session_id: &str,
    ui: Option<&mut BottomPrompt>,
) -> anyhow::Result<StreamOutcome> {
    let event_response =
        ensure_success_response(client.get(format!("{server}/event")).send().await?)
            .await?;
    stream_session_events(client, server, session_id, event_response, ui, true).await
}

async fn stream_session_events(
    client: &reqwest::Client,
    server: &str,
    session_id: &str,
    mut event_response: reqwest::Response,
    mut ui: Option<&mut BottomPrompt>,
    replay_existing: bool,
) -> anyhow::Result<StreamOutcome> {
    let mut streaming_input = String::new();
    let mut streaming_menu: Option<CompletionMenu> = None;
    let mut queued: VecDeque<String> = VecDeque::new();
    let mut switch_session: Option<String> = None;
    let mut last_esc: Option<std::time::Instant> = None;
    let mut interrupt = false;
    let mut sent_abort = false;
    let mut abort_at: Option<std::time::Instant> = None;
    let mut blocker: Option<StreamBlocker> = None;
    if ui.is_none() {
        println!("{BOLD}assistant{RESET}");
    }
    let mut buffer = String::new();
    let mut data_lines = Vec::new();
    let mut render_state = ChatRenderState::default();
    if replay_existing {
        if let Some(text) = latest_assistant_text(client, server, session_id).await? {
            render_state.text_delta(&text)?;
        }
    }
    let mut tick = tokio::time::interval(Duration::from_millis(90));
    loop {
        let next = tokio::select! {
            chunk = event_response.chunk() => chunk?,
            _ = tick.tick() => {
                if ui.is_some() {
                    drain_streaming_input(
                        client,
                        server,
                        session_id,
                        &mut streaming_input,
                        &mut streaming_menu,
                        &mut queued,
                        &mut last_esc,
                        &mut interrupt,
                        &mut blocker,
                        &mut switch_session,
                        ui.as_deref_mut(),
                        &mut render_state,
                    ).await?;
                    if switch_session.is_some() {
                        if let Some(ui) = ui.as_deref_mut() {
                            ui.add_pending_expansions(render_state.take_pending_truncated());
                            ui.clear_overlay()?;
                        }
                        return Ok(StreamOutcome {
                            queued,
                            leftover_input: streaming_input,
                            switch_session,
                        });
                    }
                }
                if interrupt && !sent_abort {
                    let _ = client
                        .post(format!("{server}/session/{session_id}/abort"))
                        .send()
                        .await;
                    sent_abort = true;
                    abort_at = Some(std::time::Instant::now());
                    continue;
                }
                if blocker.is_some() {
                    continue;
                }
                // Safety gate: if the server doesn't emit `session.status: idle`
                // within 3s of abort, force-return so the queued prompt still
                // fires and the user isn't stuck.
                if let Some(t) = abort_at {
                    if std::time::Instant::now().duration_since(t)
                        > Duration::from_secs(3)
                    {
                        if let Some(ui) = ui.as_deref_mut() {
                            ui.add_pending_expansions(render_state.take_pending_truncated());
                            ui.clear_overlay()?;
                        }
                        return Ok(StreamOutcome {
                            queued,
                            leftover_input: streaming_input,
                            switch_session,
                        });
                    }
                }
                continue;
            }
        };
        let Some(chunk) = next else {
            break;
        };
        let chunk = std::str::from_utf8(&chunk)?;
        buffer.push_str(chunk);
        while let Some(index) = buffer.find('\n') {
            let mut line = buffer[..index].to_string();
            buffer.drain(..=index);
            if line.ends_with('\r') {
                line.pop();
            }
            if line.is_empty() {
                let event = handle_chat_sse_event(
                    session_id,
                    &data_lines,
                    &mut render_state,
                    ui.as_deref_mut(),
                )?;
                if let Some(next_blocker) = event.blocker {
                    blocker = Some(next_blocker);
                    streaming_input.clear();
                    streaming_menu = None;
                }
                if event.clear_blocker {
                    blocker = None;
                    streaming_input.clear();
                    streaming_menu = None;
                }
                if event.done {
                    if render_state.printed_text {
                        render_state.finish()?;
                    } else if let Some(text) =
                        latest_assistant_text(client, server, session_id).await?
                    {
                        render_state.text_delta(&text)?;
                        render_state.finish()?;
                    } else {
                        println!();
                    }
                    if let Some(ui) = ui.as_deref_mut() {
                        ui.add_pending_expansions(render_state.take_pending_truncated());
                        ui.clear_overlay()?;
                    }
                    return Ok(StreamOutcome {
                        queued,
                        leftover_input: streaming_input,
                        switch_session,
                    });
                }
                if ui.is_some() {
                    drain_streaming_input(
                        client,
                        server,
                        session_id,
                        &mut streaming_input,
                        &mut streaming_menu,
                        &mut queued,
                        &mut last_esc,
                        &mut interrupt,
                        &mut blocker,
                        &mut switch_session,
                        ui.as_deref_mut(),
                        &mut render_state,
                    )
                    .await?;
                    if switch_session.is_some() {
                        if let Some(ui) = ui.as_deref_mut() {
                            ui.add_pending_expansions(
                                render_state.take_pending_truncated(),
                            );
                            ui.clear_overlay()?;
                        }
                        return Ok(StreamOutcome {
                            queued,
                            leftover_input: streaming_input,
                            switch_session,
                        });
                    }
                }
                if interrupt && !sent_abort {
                    let _ = client
                        .post(format!("{server}/session/{session_id}/abort"))
                        .send()
                        .await;
                    sent_abort = true;
                    abort_at = Some(std::time::Instant::now());
                }
                if blocker.is_some() && !sent_abort {
                    data_lines.clear();
                    continue;
                }
                data_lines.clear();
            } else if let Some(data) = line.strip_prefix("data:") {
                data_lines.push(data.trim_start().to_string());
            }
        }
    }
    if render_state.printed_text {
        render_state.finish()?;
    }
    if let Some(ui) = ui.as_deref_mut() {
        ui.add_pending_expansions(render_state.take_pending_truncated());
        ui.clear_overlay()?;
    }
    Ok(StreamOutcome {
        queued,
        leftover_input: streaming_input,
        switch_session,
    })
}

async fn drain_streaming_input(
    client: &reqwest::Client,
    server: &str,
    session_id: &str,
    input: &mut String,
    menu: &mut Option<CompletionMenu>,
    queued: &mut VecDeque<String>,
    last_esc: &mut Option<std::time::Instant>,
    interrupt: &mut bool,
    blocker: &mut Option<StreamBlocker>,
    switch_session: &mut Option<String>,
    mut ui: Option<&mut BottomPrompt>,
    render_state: &mut ChatRenderState,
) -> anyhow::Result<()> {
    use crate::chat_ui::{try_read_key, Key};
    refresh_streaming_menu(input, menu);
    while let Some(key) = try_read_key()? {
        match key {
            Key::Char(ch) => {
                input.push(ch);
                refresh_streaming_menu(input, menu);
                *last_esc = None;
            }
            Key::Backspace => {
                input.pop();
                refresh_streaming_menu(input, menu);
                *last_esc = None;
            }
            Key::Enter => {
                if menu.as_ref().is_some_and(|menu| !menu.options.is_empty())
                    && input.starts_with('/')
                    && !input.contains(char::is_whitespace)
                {
                    if let Some(command) = accept_streaming_completion(input, menu, true)
                    {
                        let handled = handle_stream_control_command(
                            client,
                            server,
                            session_id,
                            command.trim(),
                            queued,
                            interrupt,
                            blocker,
                            switch_session,
                        )
                        .await?;
                        if !handled && blocker.is_none() {
                            queued.push_back(command);
                        }
                        input.clear();
                    }
                    refresh_streaming_menu(input, menu);
                    *last_esc = None;
                    continue;
                }
                let text = std::mem::take(input);
                *menu = None;
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    // Keep a blocker active until the user submits a real answer.
                } else if trimmed.starts_with('/') {
                    let handled = handle_stream_control_command(
                        client,
                        server,
                        session_id,
                        trimmed,
                        queued,
                        interrupt,
                        blocker,
                        switch_session,
                    )
                    .await?;
                    if !handled && blocker.is_none() {
                        queued.push_back(text);
                    }
                } else if let Some(active) = blocker.take() {
                    submit_stream_blocker(client, server, active, trimmed).await?;
                } else if !trimmed.is_empty() {
                    let handled = handle_stream_control_command(
                        client,
                        server,
                        session_id,
                        trimmed,
                        queued,
                        interrupt,
                        blocker,
                        switch_session,
                    )
                    .await?;
                    if !handled {
                        queued.push_back(text);
                    }
                }
                *last_esc = None;
            }
            Key::Esc => {
                if menu.is_some() {
                    *menu = None;
                    *last_esc = None;
                    continue;
                }
                let now = std::time::Instant::now();
                let double = last_esc
                    .map(|t| now.duration_since(t) < Duration::from_millis(1500))
                    .unwrap_or(false);
                if double {
                    *interrupt = true;
                    *last_esc = None;
                } else {
                    *last_esc = Some(now);
                }
            }
            Key::CtrlC | Key::CtrlD => {
                input.clear();
                *menu = None;
                *last_esc = None;
            }
            Key::CtrlO => {
                if let Some(ui) = ui.as_deref_mut() {
                    ui.add_pending_expansions(render_state.take_pending_truncated());
                    ui.expand_pending()?;
                }
                *last_esc = None;
            }
            Key::Tab | Key::Right => {
                if menu.is_some() {
                    accept_streaming_completion(input, menu, false);
                    refresh_streaming_menu(input, menu);
                }
                *last_esc = None;
            }
            Key::Up => {
                if let Some(menu) = menu.as_mut() {
                    menu.move_previous();
                }
                *last_esc = None;
            }
            Key::Down => {
                if let Some(menu) = menu.as_mut() {
                    menu.move_next();
                }
                *last_esc = None;
            }
            _ => {}
        }
    }
    Ok(())
}

fn refresh_streaming_menu(input: &str, menu: &mut Option<CompletionMenu>) {
    let previous_selected = menu.as_ref().map(|menu| menu.selected);
    *menu = update_streaming_completion(input, previous_selected);
}

async fn handle_stream_control_command(
    client: &reqwest::Client,
    server: &str,
    session_id: &str,
    trimmed: &str,
    queued: &mut VecDeque<String>,
    interrupt: &mut bool,
    blocker: &mut Option<StreamBlocker>,
    switch_session: &mut Option<String>,
) -> anyhow::Result<bool> {
    let mut parts = trimmed.split_whitespace();
    match parts.next() {
        Some("/abort") => {
            *interrupt = true;
            Ok(true)
        }
        Some("/queue") => match parts.next() {
            Some("clear") => {
                queued.clear();
                let _ = client
                    .delete(format!("{server}/session/{session_id}/queue"))
                    .send()
                    .await;
                Ok(true)
            }
            Some("pop") => {
                if queued.pop_front().is_none() {
                    let _ = client
                        .post(format!("{server}/session/{session_id}/queue/pop"))
                        .send()
                        .await;
                }
                Ok(true)
            }
            _ => Ok(false),
        },
        Some("/permit") => {
            let first = parts.next();
            let (reply, id_arg) = parse_permission_reply_and_id(first, parts.next());
            let id = if let Some(id) = id_arg {
                Some(id.to_string())
            } else if let Some(StreamBlocker::Permission { id, .. }) = blocker.as_ref() {
                Some(id.clone())
            } else {
                let permissions = fetch_permission_requests(client, server).await?;
                first_pending_id(&permissions, session_id)
            };
            let Some(id) = id else {
                return Ok(true);
            };
            submit_permission_reply(client, server, &id, reply).await?;
            if matches!(blocker.as_ref(), Some(StreamBlocker::Permission { id: active, .. }) if active == &id)
            {
                *blocker = None;
            }
            Ok(true)
        }
        Some("/answer") => {
            let answer = parts.collect::<Vec<_>>().join(" ");
            if answer.trim().is_empty() {
                return Ok(true);
            }
            if let Some(StreamBlocker::Question { id, count, .. }) = blocker.take() {
                submit_question_answer(client, server, &id, &answer, count).await?;
                return Ok(true);
            }
            let questions = fetch_question_requests(client, server).await?;
            let Some(id) = first_pending_id(&questions, session_id) else {
                return Ok(true);
            };
            submit_question_answer(client, server, &id, &answer, 0).await?;
            Ok(true)
        }
        Some("/reject") | Some("/deny") => {
            let id_arg = parts.next();
            if id_arg.is_none() {
                if let Some(active) = blocker.take() {
                    match active {
                        StreamBlocker::Permission { id, .. } => {
                            submit_permission_reply(client, server, &id, "reject")
                                .await?;
                        }
                        StreamBlocker::Question { id, .. } => {
                            submit_question_reject(client, server, &id).await?;
                        }
                    }
                    return Ok(true);
                }
            }
            let questions = fetch_question_requests(client, server).await?;
            if let Some(id) = pending_id_or_first(&questions, session_id, id_arg) {
                submit_question_reject(client, server, &id).await?;
                return Ok(true);
            }
            let permissions = fetch_permission_requests(client, server).await?;
            if let Some(id) = pending_id_or_first(&permissions, session_id, id_arg) {
                submit_permission_reply(client, server, &id, "reject").await?;
            }
            Ok(true)
        }
        Some("/permissions") | Some("/questions") => Ok(true),
        Some("/sub-agent") | Some("/subagents") | Some("/sub") => {
            if let Some(id) = parts.next() {
                *switch_session = Some(id.to_string());
                return Ok(true);
            }
            let entries = fetch_subagent_sessions(client, server, session_id).await?;
            if let Some(entry) = entries
                .into_iter()
                .find(|entry| matches!(entry.status.as_str(), "working" | "retrying"))
            {
                *switch_session = Some(entry.id);
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}
