use serde_json::{json, Value};

use crate::chat_session::ensure_success_response;
use crate::chat_status::permission_reply_alias;

pub(crate) enum StreamBlocker {
    Permission {
        id: String,
        title: String,
    },
    Question {
        id: String,
        label: String,
        count: usize,
    },
}

impl StreamBlocker {
    pub(crate) fn status(&self) -> String {
        match self {
            Self::Permission { title, .. } => {
                format!("{title} · type y once, a always, n reject")
            }
            Self::Question { label, count, .. } if *count > 1 => {
                format!("Answer question 1/{count}: {label}")
            }
            Self::Question { label, .. } => format!("Answer: {label}"),
        }
    }
}

pub(crate) async fn submit_stream_blocker(
    client: &reqwest::Client,
    server: &str,
    blocker: StreamBlocker,
    input: &str,
) -> anyhow::Result<()> {
    match blocker {
        StreamBlocker::Permission { id, .. } => {
            let reply = permission_reply_alias(&input.trim().to_ascii_lowercase());
            submit_permission_reply(client, server, &id, reply).await?;
        }
        StreamBlocker::Question { id, count, .. } => {
            submit_question_answer(client, server, &id, input, count).await?;
        }
    }
    Ok(())
}

pub(crate) fn parse_permission_reply_and_id<'a>(
    first: Option<&'a str>,
    second: Option<&'a str>,
) -> (&'static str, Option<&'a str>) {
    match first {
        Some(value) if is_permission_reply(value) => {
            (permission_reply_alias(value), second)
        }
        Some(value) => ("once", Some(value)),
        None => ("once", None),
    }
}

pub(crate) fn is_permission_reply(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "once" | "always" | "reject" | "y" | "a" | "n" | "yes" | "no"
    )
}

pub(crate) async fn submit_permission_reply(
    client: &reqwest::Client,
    server: &str,
    id: &str,
    reply: &str,
) -> anyhow::Result<()> {
    ensure_success_response(
        client
            .post(format!("{server}/v2/interactions/permissions/{id}/reply"))
            .json(&json!({ "reply": reply }))
            .send()
            .await?,
    )
    .await
    .map(|_| ())
}

pub(crate) async fn submit_question_answer(
    client: &reqwest::Client,
    server: &str,
    id: &str,
    input: &str,
    count: usize,
) -> anyhow::Result<()> {
    let answers = if count <= 1 {
        vec![vec![input.trim().to_string()]]
    } else {
        input
            .split(';')
            .map(|answer| vec![answer.trim().to_string()])
            .collect::<Vec<_>>()
    };
    ensure_success_response(
        client
            .post(format!("{server}/v2/interactions/questions/{id}/reply"))
            .json(&json!({ "answers": answers }))
            .send()
            .await?,
    )
    .await
    .map(|_| ())
}

pub(crate) async fn submit_question_reject(
    client: &reqwest::Client,
    server: &str,
    id: &str,
) -> anyhow::Result<()> {
    ensure_success_response(
        client
            .post(format!("{server}/v2/interactions/questions/{id}/reject"))
            .send()
            .await?,
    )
    .await
    .map(|_| ())
}

pub(crate) fn permission_blocker(properties: &Value) -> Option<StreamBlocker> {
    Some(StreamBlocker::Permission {
        id: properties.get("id").and_then(Value::as_str)?.to_string(),
        title: properties
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Allow tool?")
            .to_string(),
    })
}

pub(crate) fn question_blocker(properties: &Value) -> Option<StreamBlocker> {
    let questions = properties.get("questions").and_then(Value::as_array)?;
    let label = questions
        .first()
        .and_then(|question| {
            question
                .get("question")
                .or_else(|| question.get("label"))
                .and_then(Value::as_str)
        })
        .unwrap_or("Question")
        .to_string();
    Some(StreamBlocker::Question {
        id: properties.get("id").and_then(Value::as_str)?.to_string(),
        label,
        count: questions.len().max(1),
    })
}
