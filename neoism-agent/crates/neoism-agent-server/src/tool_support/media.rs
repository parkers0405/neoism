use std::collections::BTreeMap;

use neoism_agent_plugin_api::{MediaGenerationRequest, MediaKind};
use serde_json::{json, Value};

use super::{ToolContext, ToolExecutionResult};

pub(crate) async fn generate_image_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    generate_media(context, arguments, MediaKind::Image).await
}

pub(crate) async fn generate_video_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    generate_media(context, arguments, MediaKind::Video).await
}

async fn generate_media(
    context: ToolContext,
    arguments: Value,
    kind: MediaKind,
) -> anyhow::Result<ToolExecutionResult> {
    let prompt = crate::tool::args::required_string(&arguments, "prompt")?
        .trim()
        .to_string();
    if prompt.is_empty() {
        anyhow::bail!("prompt must not be empty");
    }
    let state = context
        .state()
        .ok_or_else(|| anyhow::anyhow!("media generation requires session state"))?;
    let session_id = context
        .session_id()
        .ok_or_else(|| anyhow::anyhow!("media generation requires a session id"))?;
    let session = context
        .session_scope
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("media generation requires session scope"))?;
    let snapshot = context.plugin_snapshot()?;
    let configured_model = match kind {
        MediaKind::Image => snapshot.config().image_model.as_deref(),
        MediaKind::Video => snapshot.config().video_model.as_deref(),
    }
    .map(str::trim)
    .filter(|model| !model.is_empty())
    .ok_or_else(|| {
        anyhow::anyhow!(
            "agent.{}Model is not configured",
            match kind {
                MediaKind::Image => "image",
                MediaKind::Video => "video",
            }
        )
    })?;
    let model = crate::model_selection::model_ref_from_config(configured_model)
        .ok_or_else(|| anyhow::anyhow!("media model must use provider/model format"))?;
    let provider = snapshot
        .provider_services_by_priority()
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("provider service is unavailable"))?;
    let options = arguments
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, value)| key.as_str() != "prompt" && !value.is_null())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<BTreeMap<_, _>>();
    let generated = provider
        .generate_media(MediaGenerationRequest {
            kind,
            provider_id: model.provider_id,
            model_id: model.id,
            connection_id: model.connection_id,
            tenant_id: crate::caller::session_tenant(session).to_string(),
            workspace_id: session.workspace_id.as_ref().map(ToString::to_string),
            prompt,
            options,
            cancel: context.cancel.clone(),
        })
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let max_bytes = match kind {
        MediaKind::Image => crate::artifact_routes::MAX_ARTIFACT_BYTES,
        MediaKind::Video => crate::artifact_routes::MAX_GENERATED_VIDEO_BYTES,
    };
    let artifact = crate::artifact_routes::store_generated_artifact(
        state,
        crate::caller::session_tenant(session),
        session_id,
        generated.filename,
        generated.mime,
        &generated.bytes,
        max_bytes,
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let kind_name = match kind {
        MediaKind::Image => "image",
        MediaKind::Video => "video",
    };
    let mut metadata = json!({
        "generatedFiles": [{
            "mime": artifact.media_type,
            "url": artifact.download_url,
            "filename": artifact.filename,
        }],
        "artifactID": artifact.id,
    });
    if let Some(revised_prompt) = generated.revised_prompt {
        metadata["revisedPrompt"] = Value::String(revised_prompt);
    }
    Ok(ToolExecutionResult {
        title: format!("Generated {kind_name}"),
        output: format!("Generated {kind_name}: {}", artifact.download_url),
        metadata: Some(metadata),
    })
}
