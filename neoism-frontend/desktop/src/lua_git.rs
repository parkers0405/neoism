use neoism_lua::PluginJobSpawnRequest;

/// Translate typed Git operations to a fixed git argv. No shell is involved;
/// path operands are separated with `--` so repository data cannot become an
/// option. The managed-job host supplies containment, limits and teardown.
pub(crate) fn request(action: &str, value: &serde_json::Value) -> Result<PluginJobSpawnRequest, String> {
    let string = |name: &str| value.get(name).and_then(serde_json::Value::as_str).filter(|value| value.len() <= 16 * 1024).map(str::to_owned);
    let args = match action {
        "status" => vec!["status".into(), "--porcelain=v2".into(), "--branch".into()],
        "diff" => { let mut args = vec!["diff".into(), "--no-ext-diff".into(), "--".into()]; if let Some(path) = string("path") { safe_relative(&path)?; args.push(path); } args }
        "blame" => { let path = string("path").ok_or("git blame requires path")?; safe_relative(&path)?; vec!["blame".into(), "--porcelain".into(), "--".into(), path] },
        "branches" => vec!["branch".into(), "--format=%(refname:short)%00%(objectname)%00%(upstream:short)".into()],
        "history" => vec!["log".into(), "--date=iso-strict".into(), "--format=%H%x00%P%x00%an%x00%ad%x00%s".into(), format!("-n{}", value.get("limit").and_then(serde_json::Value::as_u64).unwrap_or(100).min(1_000))],
        "worktrees" => vec!["worktree".into(), "list".into(), "--porcelain".into()],
        "stage_hunk" | "unstage_hunk" => return Err("hunk mutation requires the native retained-diff capability and is not available through argv brokerage".into()),
        "checkout" => { let branch = string("branch").ok_or("git checkout requires branch")?; safe_ref(&branch)?; vec!["switch".into(), branch] },
        "branch" => { let name = string("name").ok_or("git branch requires name")?; safe_ref(&name)?; vec!["branch".into(), name] },
        "worktree" => return Err("worktree mutation requires a host-owned destination picker".into()),
        _ => return Err(format!("unsupported Git operation `{action}`")),
    };
    if args.iter().any(|argument| argument.contains('\0')) { return Err("Git argument contains NUL".into()); }
    Ok(PluginJobSpawnRequest { program: "git".into(), arguments: args, cwd: Some(".".into()), env: Default::default(), timeout_millis: Some(120_000), max_output_bytes: Some(4 * 1024 * 1024) })
}

fn safe_relative(value: &str) -> Result<(), String> { let path = std::path::Path::new(value); if value.is_empty() || path.is_absolute() || path.components().any(|part| matches!(part, std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_))) { Err("Git path must be workspace-relative".into()) } else { Ok(()) } }
fn safe_ref(value: &str) -> Result<(), String> { if value.is_empty() || value.starts_with('-') || value.contains([' ', '\0', '~', '^', ':', '?', '*', '[', '\\']) || value.contains("..") || value.ends_with('.') || value.ends_with('/') { Err("Git ref name is invalid".into()) } else { Ok(()) } }