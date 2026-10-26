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
            "Use apply_patch with a patchText V4A envelope for every file mutation. Batch related edits to one or multiple files into one patch."
        } else {
            "Use edit for targeted replacements and write only for brand-new files or intentional full replacements."
        };
        let shell = if cfg!(windows) {
            "The bash tool runs PowerShell on this Windows host (pwsh.exe if available), not bash. Commands are UTF-16 encoded, so JSON, quotes, and braces survive. Use PowerShell syntax: curl.exe instead of curl (curl is often Invoke-WebRequest), and --data-raw or here-strings for JSON bodies"
        } else {
            "Use bash for project commands"
        };
        let mut content = format!(
            "You are an interactive coding agent running in a real workspace.\nWorkspace directory: {directory}\nYou can inspect and modify this workspace with tools. grep searches file contents and glob finds files with fuzzy path and query constraints. Start with relevant paths or symbols, search before reading large files, and issue independent searches or reads together so they execute in parallel. read also lists directories. Gather the context needed for the requested task; when the relevant behavior, implementation, and verification are clear, proceed with the authorized work. Reuse prior findings and working commands. {editing} {shell}, and ask before risky or unclear actions. Keep CLI responses concise and directly useful.\n\n## Native file tools first\n\nIMPORTANT: bash and background_task are for terminal operations such as git, builds, tests, package managers, and docker. DO NOT use shell commands for file operations when a dedicated tool supports the operation.\n- File discovery: use glob, not find, ls, or Get-ChildItem.\n- Content search: use grep, not shell grep, rg, or Select-String.\n- Read files and list directories: use read, not cat, head, tail, sed, or Get-Content.\n- File mutations: use the selected native mutation tool, not shell redirection or scripts. {editing}\n- Fall back to shell only when the required native tool is unavailable or cannot support the operation. A familiar shell command or combining file operations into one pipeline is not a reason to bypass native tools. This rule applies to primary agents and subagents alike."
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
        if request
            .options
            .get("htmlArtifactsEnabled")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            sections.push(SystemContextSection {
                id: "html-artifacts".into(),
                title: None,
                content: include_str!("html_artifacts.md").into(),
            });
        }
        Ok(sections)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_context_always_routes_file_operations_to_native_tools() {
        for editing in ["edit", "apply_patch"] {
            let request = ServiceRequest {
                workspace_id: None,
                directory: Some("/workspace".into()),
                options: BTreeMap::from([
                    ("editingTool".into(), serde_json::json!(editing)),
                    (
                        "instructions".into(),
                        serde_json::json!(["Custom agent instructions"]),
                    ),
                ]),
            };
            let sections = WorkspaceContext.sections(&request).expect("valid context");
            let workspace = sections
                .iter()
                .find(|section| section.id == "workspace")
                .unwrap();
            for required in [
                "Native file tools first",
                "bash and background_task are for terminal operations",
                "File discovery: use glob",
                "Content search: use grep",
                "Read files and list directories: use read",
                "File mutations: use the selected native mutation tool",
                "required native tool is unavailable or cannot support the operation",
                "primary agents and subagents alike",
                "Custom agent instructions",
            ] {
                assert!(workspace.content.contains(required), "missing {required}");
            }
            let mutation_rule = if editing == "apply_patch" {
                "Use apply_patch with a patchText V4A envelope for every file mutation."
            } else {
                "Use edit for targeted replacements and write only for brand-new files or intentional full replacements."
            };
            assert!(workspace.content.contains(mutation_rule));
        }
    }

    #[test]
    fn artifact_guidance_requires_explicit_boolean_opt_in() {
        for enabled in [
            None,
            Some(serde_json::json!(false)),
            Some(serde_json::json!("true")),
            Some(serde_json::json!(true)),
        ] {
            let mut request = ServiceRequest {
                workspace_id: None,
                directory: None,
                options: Default::default(),
            };
            if let Some(value) = &enabled {
                request
                    .options
                    .insert("htmlArtifactsEnabled".into(), value.clone());
            }
            let sections = WorkspaceContext.sections(&request).expect("valid context");
            let guidance = sections
                .iter()
                .find(|section| section.id == "html-artifacts");
            assert_eq!(guidance.is_some(), enabled == Some(serde_json::json!(true)));
            if let Some(guidance) = guidance {
                for required in [
                    "neoism-html",
                    "--background",
                    "--chart-6",
                    "--font-mono",
                    "neoism-theme-changed",
                ] {
                    assert!(guidance.content.contains(required), "missing {required}");
                }
            }
        }
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
