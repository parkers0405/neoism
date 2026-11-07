//! Native picker and question policy adapters. Layout, filtering, navigation,
//! scroll springs, and answer progression are the existing desktop engines.
use neoism_ui::panels::agent_pane::{
    question_policy::{
        question_request_from_event, NeoismAgentPendingQuestion, QuestionCommit,
    },
    state::picker::{NeoismAgentPicker, NeoismAgentPickerKind, NeoismAgentPickerOption},
};
use neoism_ui::widgets::inline_picker;
use serde_json::{json, Value};
use std::{cell::RefCell, collections::HashMap};

#[derive(Default)]
struct Store {
    next: u32,
    pickers: HashMap<u32, NeoismAgentPicker>,
    /// Original inline-picker renderer output, per open picker.
    painters: HashMap<u32, neoism_ui::primitives::paint_record::RecordingPainter>,
    questions: HashMap<u32, NeoismAgentPendingQuestion>,
}
thread_local! { static STORE: RefCell<Store> = RefCell::new(Store::default()); }
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn number(v: &Value, key: &str, fallback: f32) -> f32 {
    v[key].as_f64().map(|n| n as f32).unwrap_or(fallback)
}
fn options(v: &Value) -> Vec<NeoismAgentPickerOption> {
    v["options"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|o| {
            let mut option = NeoismAgentPickerOption::new(
                text(o, "title"),
                text(o, "description"),
                text(o, "footer"),
                text(o, "value"),
            );
            option.section = text(o, "section").to_owned();
            option.is_header = o["header"].as_bool().unwrap_or(false);
            option.is_current = o["current"].as_bool().unwrap_or(false);
            option.pinned = o["pinned"].as_bool().unwrap_or(false);
            option
        })
        .collect()
}
fn rect(v: &Value, count: usize, footer: bool) -> ([f32; 4], usize) {
    let s = number(v, "scale", 1.0).clamp(0.5, 3.0);
    let mut anchor = v["anchor"]
        .as_array()
        .filter(|a| a.len() == 4)
        .map(|a| std::array::from_fn(|i| a[i].as_f64().unwrap_or(0.0) as f32))
        .unwrap_or([20.0, 600.0, 600.0, 100.0]);
    let extra = number(v, "headerExtra", 0.0).max(0.0);
    anchor[1] -= extra;
    let min_y = number(v, "minY", 8.0 * s);
    let limit = inline_picker::row_limit_for_space(anchor[1], min_y, s, footer, 8);
    let mut rect = inline_picker::layout_limited(count, anchor, s, footer, limit, min_y)
        .unwrap_or([anchor[0], anchor[1], anchor[2], 54.0 * s]);
    rect[3] += extra;
    (rect, limit)
}
/// `/mcp` rows carry the server catalog entry; the original picker resolves
/// the service artwork from it (desktop/web `McpServiceIcon::from_entry`).
fn sync_mcp_icons(p: &mut NeoismAgentPicker, v: &Value) {
    use neoism_ui::panels::agent_pane::mcp_icon::McpServiceIcon;
    for o in v["options"].as_array().into_iter().flatten() {
        if o["mcp"].is_object() {
            let name = text(o, "title").to_owned();
            let icon = McpServiceIcon::from_entry(&name, &o["mcp"], p.mcp_icons.get(&name));
            p.mcp_icons.insert(name, icon);
        }
    }
    if let Some(server) = v["mcpServer"].as_str() {
        p.mcp_server = Some(server.to_owned());
        if v["mcpEntry"].is_object() {
            let icon = McpServiceIcon::from_entry(server, &v["mcpEntry"], p.mcp_icons.get(server));
            p.mcp_icons.insert(server.to_owned(), icon);
        }
    }
}

/// Reuse the shared, theme-tinted provider atlas rather than shipping picker art.
fn provider_icon(
    paint: &mut dyn neoism_ui::primitives::paint::Paint,
    provider: &str,
    color: [u8; 4],
) -> Option<inline_picker::InlinePickerIcon> {
    use neoism_ui::panels::agent_pane::icon::{register_provider_logo_atlas, ProviderLogo};
    let logo = ProviderLogo::from_provider_id(provider)?;
    Some(inline_picker::InlinePickerIcon::Image {
        // RecordingPainter resets overlays each frame; keep this adapter's
        // artwork namespace independent of the desktop's private panel IDs.
        panel_id: 0x5052_4f56,
        image_id: register_provider_logo_atlas(paint, color)?,
        source_rect: logo.source_rect(),
    })
}

/// The original inline picker card (`view::picker::render_picker` body):
/// shell, rows, current/pinned marks, MCP status and artwork, activity, caret.
fn paint_picker(
    picker: &mut NeoismAgentPicker,
    painter: &mut neoism_ui::primitives::paint_record::RecordingPainter,
    v: &Value,
) -> Value {
    use neoism_ui::primitives::{ide_theme::IdeTheme, paint::Paint};
    use neoism_ui::widgets::inline_picker::{InlinePickerRow, InlinePickerStatus, InlinePickerView};
    let s = number(v, "scale", 1.0).clamp(0.5, 3.0);
    let theme = IdeTheme::by_name(text(v, "theme"));
    let anchor: [f32; 4] = v["anchor"].as_array().filter(|a| a.len() == 4)
        .map(|a| std::array::from_fn(|i| a[i].as_f64().unwrap_or(0.0) as f32))
        .unwrap_or([20.0, 600.0, 600.0, 100.0]);
    let min_y = number(v, "minY", 8.0 * s);
    let max_rows = inline_picker::row_limit_for_space(anchor[1], min_y, s, false, v["maxRows"].as_u64().unwrap_or(8) as usize);
    if picker.activity().is_none() {
        picker.set_visible_row_limit(max_rows);
    }
    if picker.kind==NeoismAgentPickerKind::Usage {
        painter.begin_frame();
        neoism_ui::panels::agent_pane::view::picker::render_usage_picker(painter,picker,anchor,&theme,s,max_rows,min_y);
        let geometry=picker.usage_layout(anchor,s,max_rows,min_y);
        return json!({"commands":painter.commands(),"rect":geometry.rect,"animating":picker.is_animating(),"selectedValue":Value::Null});
    }
    let show_search_caret = v["caret"].as_bool().unwrap_or(true);
    let empty_message = picker.empty_message();
    // Keep the editable value in state while the native card paints only
    // one mask glyph per entered scalar; credentials never enter text paint.
    let masked_query=(picker.kind==NeoismAgentPickerKind::ConnectSecret).then(||"•".repeat(picker.query.chars().count()));
    let display_query=masked_query.unwrap_or_else(||picker.query.clone());
    let list_scroll_offset = picker.tick_list_scroll();
    let cursor_offset = picker.tick_cursor();
    painter.begin_frame();
    let paint: &mut dyn Paint = painter;
    let mcp = picker.kind == NeoismAgentPickerKind::Mcp;
    let back = v["back"].as_bool().unwrap_or(false);
    let provider_rows = v["providerRows"].as_bool().unwrap_or(false);
    let rows = picker.options().iter().map(|option| InlinePickerRow {
        title: &option.title,
        description: &option.description,
        footer: &option.footer,
        is_header: option.is_header,
        is_current: option.is_current,
        is_pinned: option.pinned,
        status: mcp.then(|| match option.footer.as_str() {
            "connected" => InlinePickerStatus::Good,
            "needs auth" | "needs registration" | "ready" => InlinePickerStatus::Warning,
            "failed" => InlinePickerStatus::Error,
            _ => InlinePickerStatus::Muted,
        }),
    }).collect::<Vec<_>>();
    let row_icons = picker.options().iter().map(|option| {
        if option.value == "action:add-provider" {
            return Some(inline_picker::InlinePickerIcon::Glyph("+"));
        }
        if picker.kind == NeoismAgentPickerKind::Model && !option.is_header {
            let provider = option.value.split('/').next().unwrap_or_default();
            return provider_icon(paint, provider, theme.u8(theme.fg));
        }
        if provider_rows && !option.is_header {
            return provider_icon(paint, &option.value, theme.u8(theme.fg));
        }
        if picker.kind == NeoismAgentPickerKind::Session {
            if option.is_header {
                return (option.section == "directory").then(|| inline_picker::InlinePickerIcon::Glyph(neoism_ui::panels::file_tree::icons::folder_icon(true).0));
            }
            return None;
        }
        if !mcp || option.is_header { return None; }
        Some(picker.mcp_icons.get(&option.title).cloned().unwrap_or_default().presentation(paint, theme.u8(theme.fg)))
    }).collect::<Vec<_>>();
    // The icon slot reserves title space and clips long titles before `esc`.
    // Back takes precedence over artwork; the host owns its hit overlay.
    let header_icon = if back {
        Some(inline_picker::InlinePickerIcon::Glyph("←"))
    } else if let Some(provider) = v["provider"].as_str() {
        provider_icon(paint, provider, theme.u8(theme.fg))
    } else {
        (picker.kind == NeoismAgentPickerKind::McpActions).then(|| {
            picker.mcp_server.as_ref().and_then(|name| picker.mcp_icons.get(name)).cloned().unwrap_or_default()
                .presentation(paint, theme.u8(theme.fg))
        })
    };
    let state = inline_picker::render_limited_with_icons(
        paint,
        InlinePickerView {
            title: &picker.title,
            query: &display_query,
            selected: picker.selected,
            scroll_offset: picker.scroll_offset,
            list_scroll_offset,
            cursor_offset,
            rows: &rows,
            footer_hint: None,
            rename: None,
            show_search_caret,
            wrap_prompt: false,
            search_placeholder: picker.search_placeholder.as_deref().unwrap_or("Search"),
            loading: picker.loading,
            empty_message,
            loading_elapsed: picker.loading_elapsed(),
        },
        anchor,
        &theme,
        s,
        max_rows,
        min_y,
        picker.activity(),
        &row_icons,
        header_icon,
    );
    if let Some(state) = state {
        if picker.activity().is_some() {
            picker.set_visible_row_limit(state.visible_rows.max(1));
        }
        picker.set_render_geometry(state, s, list_scroll_offset);
    }
    json!({
        "commands": painter.commands(),
        "rect": state.map(|state| state.rect),
        // Same device-pixel coordinate space as `rect`; includes 4px padding
        // around the shared header's 16px arrow, without covering the title.
        "backRect": state.filter(|_| back).map(|state| [state.rect[0] + 10.0 * s, state.rect[1] + 4.0 * s, 24.0 * s, 24.0 * s]),
        "animating": picker.is_animating(),
        "selectedValue": picker.selected_option().map(|o| &o.value),
    })
}

fn picker_frame(p: &mut NeoismAgentPicker, v: &Value) -> Value {
    if p.kind==NeoismAgentPickerKind::Usage {
        let scale=number(v,"scale",1.0);let anchor=v["anchor"].as_array().filter(|a|a.len()==4).map(|a|std::array::from_fn(|i|a[i].as_f64().unwrap_or(0.0)as f32)).unwrap_or([20.0,600.0,600.0,100.0]);
        let limit=inline_picker::row_limit_for_space(anchor[1],number(v,"minY",8.0*scale),scale,false,8);
        let geometry=p.usage_layout(anchor,scale,limit,number(v,"minY",8.0*scale));
        return json!({"title":p.title,"query":"","rows":[],"count":p.usage_accounts.len(),"selected":0,"top":0,"offset":p.tick_list_scroll(),"cursorOffset":0,"rect":geometry.rect,"limit":limit,"animating":p.loading||p.is_animating(),"selectedValue":Value::Null,"loading":p.loading});
    }
    let (rect, limit) =
        rect(v, if p.loading && p.options().is_empty() { 3 } else { p.options().len() }, v["footer"].as_bool().unwrap_or(false));
    p.set_visible_row_limit(limit);
    let offset = p.tick_list_scroll();
    let cursor = p.tick_cursor();
    // Only the visible window (plus one row each side for the scroll spring)
    // crosses to the host, like the desktop picker which only draws what is on
    // screen. A 500-model catalog otherwise mounted 500 rows every frame.
    let count=p.options().len();
    let start=p.scroll_offset.saturating_sub(1).min(count);
    let end=(p.scroll_offset+limit+2).min(count);
    let rows:Vec<Value>=p.options()[start..end].iter().enumerate().map(|(i,o)|json!({"index":start+i,"title":o.title,"description":o.description,"footer":o.footer,"value":o.value,"section":o.section,"header":o.is_header,"current":o.is_current,"pinned":o.pinned})).collect();
    json!({"title":p.title,"query":p.query,"rows":rows,"count":count,"selected":p.selected,"top":p.scroll_offset,"offset":offset,"cursorOffset":cursor,
        "rect":rect,"limit":limit,"animating":p.is_animating(),"selectedValue":p.selected_option().map(|o|&o.value),"loading":p.loading})
}
fn question_frame(q: &NeoismAgentPendingQuestion, v: &Value) -> Value {
    let title = q
        .current_item()
        .or_else(|| q.questions.last())
        .map(|item| {
            if q.questions.len() > 1 {
                format!(
                    "({}/{}) {}",
                    (q.current + 1).min(q.questions.len()),
                    q.questions.len(),
                    item.text
                )
            } else {
                item.text.clone()
            }
        })
        .unwrap_or_default();
    let rows:Vec<Value>=q.visible_rows().into_iter().map(|r|json!({"title":r.label,"description":r.description,"footer":if r.is_custom{"enter"}else{""},"value":r.label,"header":false})).collect();
    let entry=v["entry"].as_bool().unwrap_or(false);
    let (rect, limit) = rect(v, if entry {0}else{rows.len()}, !entry);
    json!({"title":title,"query":q.typed,"rows":rows,"selected":q.selected,"top":q.scroll_offset,"offset":0,"cursorOffset":0,"rect":rect,"limit":limit,
        "animating":false,"responding":q.responding,"current":q.current,"count":q.questions.len(),"requestId":q.id,"sessionId":q.session_id})
}
pub fn dispatch(v: &Value) -> Option<Result<Value, String>> {
    let op = text(v, "op");
    if op == "composer.labels" {
        use neoism_ui::panels::agent_pane::view::{
            layout::is_narrow_composer_width,
            user_input::{capitalize_chip_label, model_chip_label},
        };
        let narrow =
            is_narrow_composer_width(number(v, "width", 800.0), number(v, "scale", 1.0));
        return Some(Ok(
            json!({"agent":capitalize_chip_label(text(v,"agent")),"model":model_chip_label(text(v,"model"),narrow),"thinking":capitalize_chip_label(text(v,"thinking")),"narrow":narrow}),
        ));
    }
    if op == "attachment.token" {
        use neoism_ui::panels::agent_pane::attachment_policy::{
            file_attachment_token, unique_attachment_token,
        };
        let base = file_attachment_token(
            text(v, "filename"),
            text(v, "mime"),
            v["index"].as_u64().unwrap_or(1) as usize,
        );
        let tokens: Vec<&str> = v["tokens"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        return Some(Ok(
            json!({"token":unique_attachment_token(text(v,"input"),tokens,&base)}),
        ));
    }
    if !op.starts_with("picker.") && !op.starts_with("question.") {
        return None;
    }
    Some(STORE.with(|store| {
        let mut store = store.borrow_mut();
        let id = v["id"].as_u64().unwrap_or(0) as u32;
        match op {
            // The desktop catalog projection: connected providers only, same
            // provider/model ranking and "Free"/"Nk ctx" footers.
            "picker.connect-options" => Ok(Value::Array(
                neoism_ui::panels::agent_pane::state::connect_options_from_catalog(&v["catalog"])
                    .into_iter()
                    .map(|o| json!({"title":o.title,"description":o.description,"footer":o.footer,"value":o.value,"section":o.section,"header":o.is_header,"current":o.is_current,"pinned":o.pinned}))
                    .collect(),
            )),
            "picker.model-options" => Ok(Value::Array(
                neoism_ui::panels::agent_pane::api_mapping::model_options_from_providers_json(&v["catalog"])
                    .into_iter()
                    .map(|o| json!({"title":o.title,"description":o.description,"footer":o.footer,"value":o.value,"section":o.section,"header":o.is_header}))
                    .collect(),
            )),
            "picker.new" => {
                store.next += 1;
                let id = 0x40000000 + store.next;
                let kind = match text(v, "kind") {
                    "usage" => NeoismAgentPickerKind::Usage,
                    "agent" => NeoismAgentPickerKind::Agent,
                    "model" => NeoismAgentPickerKind::Model,
                    "thinking" => NeoismAgentPickerKind::Thinking,
                    "skill" | "skills" => NeoismAgentPickerKind::Skill,
                    "mcp" => NeoismAgentPickerKind::Mcp,
                    "mcp-action" => NeoismAgentPickerKind::McpActions,
                    "connect" => NeoismAgentPickerKind::Connect,
                    "connect-account" => NeoismAgentPickerKind::ConnectAccount,
                    "connect-account-actions" => NeoismAgentPickerKind::ConnectAccountActions,
                    "connect-confirm" => NeoismAgentPickerKind::ConnectConfirm,
                    "connect-method" | "connect-input" | "connect-authorize" => NeoismAgentPickerKind::ConnectAuth,
                    "secret" => NeoismAgentPickerKind::ConnectSecret,
                    "input" => NeoismAgentPickerKind::ConnectLabel,
                    "account" => NeoismAgentPickerKind::ModelAccount,
                    "sessions" => NeoismAgentPickerKind::Session,
                    "subagents" => NeoismAgentPickerKind::Subagent,
                    "directory" => NeoismAgentPickerKind::Directory,
                    _ => NeoismAgentPickerKind::Slash,
                };
                let mut picker = NeoismAgentPicker::new(
                    kind,
                    text(v, "title"),
                    options(v),
                    v["selected"].as_u64().unwrap_or(0) as usize,
                );
                picker.set_loading(v["loading"].as_bool().unwrap_or(false));
                picker.search_placeholder=v["placeholder"].as_str().map(str::to_owned);
                sync_mcp_icons(&mut picker, v);
                store.pickers.insert(id, picker);
                Ok(json!({"id":id}))
            }
            "question.new" => {
                store.next += 1;
                let id = 0x40000000 + store.next;
                let question = question_request_from_event(&v["request"]);
                store.questions.insert(id, question);
                Ok(json!({"id":id}))
            }
            "picker.dispose" | "question.dispose" => {
                store.pickers.remove(&id);
                store.painters.remove(&id);
                store.questions.remove(&id);
                Ok(Value::Null)
            }
            _ if op.starts_with("picker.") => {
                let Store { pickers, painters, .. } = &mut *store;
                let p = pickers.get_mut(&id).ok_or("Unknown picker")?;
                match op {
                    "picker.query" => p.set_query(text(v, "query").to_owned()),
                    "picker.usage"=>{
                        if v["usage"].is_object(){let payload:neoism_ui::panels::agent_pane::state::picker::NeoismAgentUsagePayload=serde_json::from_value(v["usage"].clone()).map_err(|error|error.to_string())?;p.set_usage_accounts(payload.accounts);}
                        if let Some(error)=v["error"].as_str(){p.set_usage_error(error.to_owned());}
                        p.set_loading(v["loading"].as_bool().unwrap_or(false));
                    }
                    "picker.options" => {
                        if let Some(title) = v["title"].as_str() { p.title = title.to_owned(); }
                        p.replace_options(options(v));
                        p.set_loading(v["loading"].as_bool().unwrap_or(false));
                        if let Some(placeholder)=v["placeholder"].as_str(){p.search_placeholder=Some(placeholder.to_owned());}
                        sync_mcp_icons(p, v);
                    }
                    // Connect / OAuth / MCP stage status, drawn by the original card.
                    "picker.activity" => {
                        if v["body"].is_string() || v["status"].is_string() {
                            p.set_connect_activity(text(v, "body"), text(v, "status"), v["busy"].as_bool().unwrap_or(false));
                            if let Some(url) = v["url"].as_str() { p.set_connect_authorization_url(url); }
                        } else {
                            p.clear_connect_activity();
                        }
                    }
                    "picker.paint" => {
                        let painter = painters.entry(id).or_insert_with(|| {
                            let family = text(v, "fontFamily");
                            neoism_ui::primitives::paint_record::RecordingPainter::new(
                                if family.is_empty() { "Cascadia Code NF" } else { family },
                                v["fontWeight"].as_u64().unwrap_or(400) as u16,
                                vec![],
                            )
                        });
                        return Ok(paint_picker(p, painter, v));
                    }
                    // Original hit test against the painted rows.
                    "picker.hit" => {
                        let (x, y) = (number(v, "x", 0.0), number(v, "y", 0.0));
                        if let Some(url) = p.authorization_url_at(x, y) {
                            return Ok(json!({"url": url}));
                        }
                        let hit = p.activate_row_at(x, y);
                        return Ok(json!({"hit": hit, "inside": p.contains_point(x, y), "value": hit.then(|| p.selected_option().map(|o| o.value.clone())).flatten()}));
                    }
                    "picker.move" => p.move_selection(number(v, "delta", 0.0) as isize),
                    "picker.select" => {
                        let target = number(v, "index", 0.0).max(0.0) as usize;
                        if p.options().get(target).is_some_and(|o| o.is_selectable()) {
                            for _ in 0..p.options().len() {
                                if p.selected == target {
                                    break;
                                }
                                p.move_selection(if target > p.selected {
                                    1
                                } else {
                                    -1
                                });
                            }
                        }
                    }
                    "picker.scroll" => {
                        p.scroll_pixels(number(v, "delta", 0.0));
                    }
                    "picker.frame" => {}
                    _ => return Err("Unknown picker operation".into()),
                }
                if v["deferFrame"].as_bool().unwrap_or(false) { return Ok(Value::Null); }
                Ok(picker_frame(p, v))
            }
            _ => {
                let Store { questions, painters, .. } = &mut *store;
                let q = questions.get_mut(&id).ok_or("Unknown question")?;
                let mut answers = None;
                match op {
                    "question.paint" => {
                        use neoism_ui::primitives::{ide_theme::IdeTheme, paint::Paint, paint_record::RecordingPainter};
                        let painter = painters.entry(id).or_insert_with(|| RecordingPainter::new(
                            text(v, "fontFamily"), v["fontWeight"].as_u64().unwrap_or(400) as u16, vec![],
                        ));
                        let s = number(v, "scale", 1.0).clamp(0.5, 3.0);
                        let anchor = v["anchor"].as_array().filter(|a| a.len() == 4)
                            .map(|a| std::array::from_fn(|i| a[i].as_f64().unwrap_or(0.0) as f32))
                            .unwrap_or([20.0, 600.0, 600.0, 100.0]);
                        painter.begin_frame();
                        let state = neoism_ui::panels::agent_pane::view::prompt_picker::render_question_picker(
                            painter, q, anchor, &IdeTheme::by_name(text(v, "theme")), s,
                            number(v, "minY", 8.0 * s), v["error"].as_str(),
                        );
                        let rows: Vec<Value> = state.as_ref().map(|state| {
                            (0..state.visible_rows).filter_map(|slot| {
                                let index = state.first_row + slot;
                                (index < q.visible_rows().len()).then(|| json!({"index":index,"rect":state.row_rect(slot,s)}))
                            }).collect()
                        }).unwrap_or_default();
                        return Ok(json!({"commands":painter.commands(),"rect":state.map(|s|s.rect),"rows":rows,"animating":false}));
                    }
                    "question.query" if !q.responding => {
                        q.typed = text(v, "query").to_owned();
                        q.selected = 0;
                        q.scroll_offset = 0;
                    }
                    "question.move" if !q.responding => {
                        q.move_selection(number(v, "delta", 0.0) as isize)
                    }
                    "question.select" if !q.responding => {
                        q.selected = (number(v, "index", 0.0).max(0.0) as usize)
                            .min(q.visible_rows().len().saturating_sub(1))
                    }
                    "question.commit"
                        if !q.responding
                            && q.current >= q.questions.len()
                            && !q.answers.is_empty() =>
                    {
                        q.responding = true;
                        answers = Some(q.answers.clone());
                    }
                    "question.commit" if !q.responding => {
                        if let QuestionCommit::Finished(value) = q.commit_selected() {
                            q.responding = true;
                            answers = Some(value);
                        }
                    }
                    "question.retry" => q.responding = false,
                    "question.frame" | "question.commit" | "question.query"
                    | "question.select" | "question.move" => {}
                    _ => return Err("Unknown question operation".into()),
                }
                let mut frame = question_frame(q, v);
                if let Some(answers) = answers {
                    frame["answers"] = json!(answers);
                }
                Ok(frame)
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_picker_paints_active_check_and_add_provider_action() {
        let call = |op: &str, mut args: Value| {
            args["op"] = json!(op);
            dispatch(&args).unwrap().unwrap()
        };
        let id = call("picker.new", json!({"kind":"model", "options":[
            {"title":"Recent model", "value":"openai/recent", "current":true, "footer":"selected"},
            {"title":"Add Provider", "value":"action:add-provider"}
        ]}))["id"].clone();
        let painted = call("picker.paint", json!({"id":id, "anchor":[20,600,600,100], "scale":1, "caret":false}));
        let commands = painted["commands"].as_array().unwrap();
        let check = commands.iter().find(|c| c["text"] == "✓").unwrap();
        let footer = commands.iter().find(|c| c["text"] == "selected").unwrap();
        assert!(check["x"].as_f64().unwrap() > footer["x"].as_f64().unwrap() + footer["width"].as_f64().unwrap());
        let horizontal = commands.iter().find(|c| c["kind"] == "rect" && c["rect"][2] == 12.0 && c["rect"][3] == 2.0).unwrap();
        let vertical = commands.iter().find(|c| c["kind"] == "rect" && c["rect"][2] == 2.0 && c["rect"][3] == 12.0).unwrap();
        let row = &painted["rows"][1]["rect"];
        for bar in [horizontal, vertical] {
            let rect = &bar["rect"];
            assert_eq!(rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap() / 2.0, row[0].as_f64().unwrap() + 31.0);
            assert_eq!(rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap() / 2.0, row[1].as_f64().unwrap() + row[3].as_f64().unwrap() / 2.0);
        }
        call("picker.dispose", json!({"id":id}));
    }

    #[test]
    fn provider_picker_back_header_and_dynamic_title_keep_anchored_geometry() {
        let call = |op: &str, mut args: Value| {
            args["op"] = json!(op);
            dispatch(&args).unwrap().unwrap()
        };
        let options = json!([
            {"title":"Providers", "header":true, "value":"openai"},
            {"title":"Not a provider name", "value":"anthropic"},
            {"title":"Custom", "value":"custom-endpoint"}
        ]);
        let id = call("picker.new", json!({"kind":"connect", "title":"Connect", "options":options}))["id"].clone();
        let updated = call("picker.options", json!({"id":id, "title":"Choose authentication", "options":options}));
        assert_eq!(updated["title"], "Choose authentication");
        for scale in [1.0, 2.0] {
            let mut request = json!({"id":id,"anchor":[20,600,600,100],"minY":8,"fontFamily":"monospace","scale":scale,"caret":false,"providerRows":true,"back":true});
            let painted = call("picker.paint", request.clone());
            let card = painted["rect"].as_array().unwrap();
            let back = painted["backRect"].as_array().unwrap();
            let x = card[0].as_f64().unwrap();
            let y = card[1].as_f64().unwrap();
            assert!(y >= 8.0 && y + card[3].as_f64().unwrap() <= 600.0);
            assert_eq!(back, &vec![json!(x + 10.0 * scale), json!(y + 4.0 * scale), json!(24.0 * scale), json!(24.0 * scale)]);
            let commands = painted["commands"].as_array().unwrap();
            assert!(commands.iter().any(|c| c["text"] == "←"));
            let title = commands.iter().find(|c| c["text"] == "Choose authentication").unwrap();
            assert!(title["x"].as_f64().unwrap() >= back[0].as_f64().unwrap() + back[2].as_f64().unwrap());
            // Only the known non-header value resolves artwork, regardless of label.
            assert_eq!(commands.iter().filter(|c| c["kind"] == "image").count(), 1);
            request["back"] = json!(false);
            request["provider"] = json!("openai");
            let header = call("picker.paint", request);
            assert!(header["backRect"].is_null());
            assert_eq!(header["rect"], painted["rect"]);
            assert_eq!(header["commands"].as_array().unwrap().iter().filter(|c| c["kind"] == "image").count(), 2);
        }
        call("picker.dispose", json!({"id":id}));
    }

    #[test]
    fn question_card_wraps_above_composer_and_keeps_answer_hit_geometry() {
        let call = |op: &str, args: Value| {
            let mut args = args;
            args["op"] = json!(op);
            dispatch(&args).unwrap().unwrap()
        };
        let id = call("question.new", json!({"request":{
            "id":"q", "sessionId":"child", "questions":[{
                "question":"May I launch another controlled diagnostic preview to capture the hang under the debugger from startup?",
                "options":[{"label":"Yes","description":"Launch under debugger"},{"label":"No"}]
            }]
        }}))["id"].clone();
        let paint = call("question.paint", json!({"id":id,"anchor":[20,600,420,100],"minY":40,"fontFamily":"monospace","scale":1}));
        let rect = paint["rect"].as_array().unwrap();
        let y = rect[1].as_f64().unwrap();
        assert!(y >= 40.0);
        assert!(y + rect[3].as_f64().unwrap() <= 600.0);
        let rows = paint["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0]["rect"][1].as_f64().unwrap() > y + 54.0, "Long prompt must wrap before the answer rows");
        assert!(paint["commands"].as_array().unwrap().iter().any(|c| c["text"] == "Yes"));
        call("question.query", json!({"id":id,"query":"Only with a timeout"}));
        let answer = call("question.commit", json!({"id":id}));
        assert_eq!(answer["answers"], json!([["Only with a timeout"]]));
        let sending = call("question.paint", json!({"id":id,"anchor":[20,600,420,100],"minY":40,"fontFamily":"monospace"}));
        assert!(sending["rows"].as_array().unwrap().is_empty());
        assert!(sending["commands"].as_array().unwrap().iter().any(|c| c["text"] == "sending…"));
        call("question.dispose", json!({"id":id}));
    }
}
