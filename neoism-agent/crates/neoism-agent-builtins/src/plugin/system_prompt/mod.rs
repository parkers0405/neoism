use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_core::{GoalStatus, SessionGoal};
use neoism_agent_plugin_api::{
    PluginContributions, PluginDefinition, PluginHostError, PluginManifest,
    PluginRuntimeError, PromptRequest, PromptService, RenderedPrompt, ServiceRequest,
    SystemContextSection, SystemContextService,
};

pub const ID: &str = "dev.neoism.system-prompt";

pub struct SystemPromptPlugin;

impl PluginDefinition for SystemPromptPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(),
            name: "System prompts".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            internal: true,
            disableable: true,
            capabilities: vec!["neoism.system-prompt".into()],
            requires: Vec::new(),
            event_namespaces: Vec::new(),
            api_prefix: None,
            config: BTreeMap::new(),
        }
    }

    fn required_capabilities(&self) -> Vec<neoism_agent_plugin_api::HostCapability> {
        use neoism_agent_plugin_api::HostCapability::*;
        vec![ConfigRead, WorkspaceRead]
    }
    fn contributions(
        &self,
        registrar: &mut PluginContributions,
    ) -> Result<(), PluginHostError> {
        registrar.system_context_service_runtime("workspace", Arc::new(WorkspaceContext));
        registrar.prompt_service_runtime("runtime", Arc::new(RuntimePrompts));
        Ok(())
    }
}

struct WorkspaceContext;

impl SystemContextService for WorkspaceContext {
    fn sections(
        &self,
        request: &ServiceRequest,
    ) -> Result<Vec<SystemContextSection>, PluginRuntimeError> {
        let directory = request.directory.as_deref().unwrap_or_default();
        let editing = request
            .options
            .get("editingTool")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("edit");
        let editing = if editing == "apply_patch" {
            "Use apply_patch with a patchText V4A envelope for every file mutation."
        } else {
            "Use edit for targeted replacements and write only for brand-new files or intentional full replacements."
        };
        let mut content = format!(
            "You are an interactive coding agent running in a real workspace.\nWorkspace directory: {directory}\nYou can inspect and modify this workspace with tools. grep searches file contents and glob finds files with fuzzy path and query constraints. Search before reading large files, and issue independent searches or reads together so they execute in parallel. read also lists directories. {editing} Use bash for project commands, and ask before risky or unclear actions. Keep CLI responses concise and directly useful."
        );
        for key in ["instructions", "serviceFragments"] {
            for fragment in request
                .options
                .get(key)
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
            {
                if !fragment.trim().is_empty() {
                    content.push_str("\n\n");
                    content.push_str(fragment);
                }
            }
        }
        let mut sections = vec![SystemContextSection {
            id: "workspace".into(),
            title: None,
            content,
        }];
        if let Some(goal) = request.options.get("goal") {
            if !goal.is_null() {
                let goal: SessionGoal = serde_json::from_value(goal.clone())
                    .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
                if let Some(content) = goal_context(&goal) {
                    sections.push(SystemContextSection {
                        id: "goal".into(),
                        title: None,
                        content,
                    });
                }
            }
        }
        Ok(sections)
    }
}

struct RuntimePrompts;

impl PromptService for RuntimePrompts {
    fn render(
        &self,
        request: &PromptRequest,
    ) -> Result<RenderedPrompt, PluginRuntimeError> {
        let content = match request.prompt_id.as_str() {
            "active-run" => request
                .variables
                .get("instructions")
                .and_then(serde_json::Value::as_str)
                .map(|value| format!("Active agent instructions for this run:\n{}", value.trim()))
                .unwrap_or_default(),
            "active-goal-continuation" => "Continue working toward the active persistent goal. Do not stop just because one batch of work is done; keep going until the goal is genuinely accomplished. When it is fully done, call the complete_goal tool (status=complete) with a thorough summary instead of replying with plain text. If you are truly stuck and need the user, call complete_goal with status=blocked and explain exactly what you need.".into(),
            other => return Err(PluginRuntimeError::new(format!("unknown prompt {other}"))),
        };
        Ok(RenderedPrompt {
            content,
            system: true,
        })
    }
}

fn goal_context(goal: &SessionGoal) -> Option<String> {
    let text = goal.text.trim();
    if text.is_empty() || goal.status == GoalStatus::Complete {
        return None;
    }
    let mut content = match goal.status {
        GoalStatus::Complete => unreachable!(),
        GoalStatus::Blocked => format!("Persistent goal for this session (currently marked BLOCKED). Re-evaluate whether the latest message unblocks you.\n\nGoal: {text}"),
        GoalStatus::Active => format!("Persistent goal for this session. Keep working toward it across every turn, even if the latest message does not restate it. If a request conflicts with the goal, flag the conflict before proceeding. When the goal is fully accomplished, call the complete_goal tool (status=complete); if blocked, call it with status=blocked.\n\nGoal: {text}"),
    };
    if !goal.summary.trim().is_empty() {
        content.push_str(&format!(
            "\n\nYour last status note ({}): {}",
            goal.status.label(),
            goal.summary.trim()
        ));
    }
    for note in &goal.research {
        if !note.content.trim().is_empty() {
            content.push_str(&format!(
                "\n\nSource: {}\n{}",
                note.source,
                note.content.trim()
            ));
        }
    }
    Some(content)
}
