use super::*;
use neoism_ui::editor::documentation_notebook::{NotebookBinding, MANIFEST_NAME};

pub(super) fn tools() -> Vec<Value> {
    vec![
        tool("notebookList", "List documentation notebooks in the linked Notes vault (folder-backed Markdown collections, not Jupyter notebooks)", json!({"type":"object","properties":{"limit":{"type":"integer","minimum":1,"maximum":10000}},"additionalProperties":false})),
        tool("notebookRead", "Read a documentation notebook manifest and its ordered page references. Does not read page contents; use notes.read for vault-relative Markdown files.", json!({"type":"object","properties":{"path":{"type":"string","description":"Notebook folder or notebook.json, relative to the linked vault"}},"required":["path"],"additionalProperties":false})),
        tool("notebookCreate", "Create a GitBook-style documentation notebook folder in the linked Notes vault. Creates notebook.json and an ordinary overview.md page; path is relative to the vault.", json!({"type":"object","properties":{"path":{"type":"string","description":"New notebook folder relative to the linked vault"},"title":{"type":"string","description":"Optional display title; defaults to the folder name"}},"required":["path"],"additionalProperties":false})),
        tool("notebookAddPage", "Add a page to a documentation notebook. Supply title (and optional content) to create a new Markdown file, OR existing_path to reference an existing file anywhere within the linked vault without copying it.", json!({"type":"object","properties":{"notebook":{"type":"string","description":"Notebook folder or notebook.json, relative to the linked vault"},"title":{"type":"string"},"content":{"type":"string"},"existing_path":{"type":"string","description":"Existing Markdown file relative to the vault, not the notebook"}},"required":["notebook"],"oneOf":[{"required":["title"],"not":{"required":["existing_path"]}},{"required":["existing_path"],"not":{"anyOf":[{"required":["title"]},{"required":["content"]}]}}],"additionalProperties":false})),
        tool("notebookMovePage", "Move a notebook page up or down in reading order without moving or modifying its Markdown file", json!({"type":"object","properties":{"notebook":{"type":"string"},"page":{"type":"integer","minimum":1,"description":"One-based page position from notebookRead"},"direction":{"type":"string","enum":["up","down"]}},"required":["notebook","page","direction"],"additionalProperties":false})),
    ]
}

pub(super) fn call(notes: &Notes, name: &str, args: Value) -> Result<String, String> {
    match name {
        "notebookList" => {
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(100)
                .clamp(1, 10_000) as usize;
            let mut found = Vec::new();
            if notes.root.exists() {
                collect(&notes.root, &notes.root, &mut found, limit, &mut 20_000, 0)?;
            }
            Ok(found.join("\n"))
        }
        "notebookRead" => {
            describe(notes, &load(notes, &required_string(&args, "path")?)?)
        }
        "notebookCreate" => {
            let path = checked_path(notes, &required_string(&args, "path")?)?;
            if path.exists() {
                return Err("notebook path already exists".into());
            }
            let title = args
                .get("title")
                .map(|value| {
                    value
                        .as_str()
                        .map(str::trim)
                        .filter(|title| !title.is_empty())
                        .map(str::to_string)
                        .ok_or("title must be a non-empty string")
                })
                .transpose()?;
            let binding = NotebookBinding::create(&path, title.as_deref())?;
            describe(notes, &binding)
        }
        "notebookAddPage" => {
            let existing = args
                .get("existing_path")
                .map(|_| required_string(&args, "existing_path"))
                .transpose()?;
            if existing.is_some() == args.get("title").is_some()
                || (existing.is_some() && args.get("content").is_some())
            {
                return Err(
                    "Supply either title with optional content, or existing_path".into(),
                );
            }
            let binding = load(notes, &required_string(&args, "notebook")?)?;
            let mut created = false;
            let path = if let Some(existing) = existing {
                checked_path(notes, &existing)?
            } else {
                let title = required_string(&args, "title")?;
                let path = binding
                    .path
                    .parent()
                    .ok_or("Notebook has no folder")?
                    .join(safe_note_file_name(&title));
                let relative = relative_path(notes, &path)?;
                let path = checked_path(notes, &relative)?;
                let content = match args.get("content") {
                    Some(value) => value
                        .as_str()
                        .ok_or("content must be a string")?
                        .to_string(),
                    None => format!("# {}\n", title.trim()),
                };
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map_err(|error| error.to_string())?;
                file.write_all(content.as_bytes())
                    .and_then(|_| file.sync_all())
                    .map_err(|error| error.to_string())?;
                created = true;
                path
            };
            let result = binding
                .session
                .lock()
                .map_err(|_| "Notebook lock failed".to_string())?
                .add_existing(path.clone());
            if let Err(error) = result {
                return Err(if created {
                    format!("{error}. The new page was preserved at {}", path.display())
                } else {
                    error
                });
            }
            describe(notes, &binding)
        }
        "notebookMovePage" => {
            let page = args
                .get("page")
                .and_then(Value::as_u64)
                .and_then(|page| usize::try_from(page).ok())
                .filter(|page| *page > 0)
                .ok_or("page must be a positive one-based position")?;
            let down = match required_string(&args, "direction")?.as_str() {
                "up" => false,
                "down" => true,
                _ => return Err("direction must be up or down".into()),
            };
            let binding = load(notes, &required_string(&args, "notebook")?)?;
            {
                let mut book =
                    binding.session.lock().map_err(|_| "Notebook lock failed")?;
                if page > book.manifest.pages.len() {
                    return Err("page is outside the notebook".into());
                }
                book.current = page - 1;
                book.move_current(down)?;
            }
            describe(notes, &binding)
        }
        _ => Err(format!("unknown Notes notebook tool {name}")),
    }
}

// Notebook metadata and newly-created pages obey the same vault boundary as
// Notes tools. Canonicalize existing ancestors so symlinked folders cannot escape.
fn checked_path(notes: &Notes, raw: &str) -> Result<PathBuf, String> {
    let path = notes.path(raw)?;
    let root = std::fs::canonicalize(&notes.root).map_err(|error| error.to_string())?;
    let mut ancestor = path.as_path();
    while !ancestor.exists() {
        ancestor = ancestor.parent().ok_or("Path has no existing parent")?;
    }
    let real = std::fs::canonicalize(ancestor).map_err(|error| error.to_string())?;
    if !real.starts_with(&root) {
        return Err(
            "notebook paths must stay inside the selected vault (including symlinks)"
                .into(),
        );
    }
    let suffix = path
        .strip_prefix(ancestor)
        .map_err(|error| error.to_string())?;
    if suffix.as_os_str().is_empty() {
        Ok(real)
    } else {
        Ok(real.join(suffix))
    }
}

fn relative_path(notes: &Notes, path: &Path) -> Result<String, String> {
    let root = std::fs::canonicalize(&notes.root).map_err(|error| error.to_string())?;
    Ok(path
        .strip_prefix(root)
        .map_err(|_| "Notebook is outside the selected vault")?
        .to_string_lossy()
        .replace('\\', "/"))
}

fn load(notes: &Notes, raw: &str) -> Result<NotebookBinding, String> {
    let mut path = checked_path(notes, raw)?;
    if path.is_dir() {
        path = path.join(MANIFEST_NAME);
    }
    if path.file_name().is_none_or(|name| name != MANIFEST_NAME) {
        return Err("Expected a notebook folder or notebook.json".into());
    }
    // Validate the manifest itself too, not only its containing directory.
    let relative = relative_path(notes, &path)?;
    NotebookBinding::load(&checked_path(notes, &relative)?)
}

fn describe(notes: &Notes, binding: &NotebookBinding) -> Result<String, String> {
    let book = binding.session.lock().map_err(|_| "Notebook lock failed")?;
    serde_json::to_string_pretty(&json!({"path":relative_path(notes, &binding.path)?,"title":book.manifest.title,"pages":book.manifest.pages})).map_err(|error| error.to_string())
}

fn collect(
    root: &Path,
    dir: &Path,
    found: &mut Vec<String>,
    limit: usize,
    budget: &mut usize,
    depth: usize,
) -> Result<(), String> {
    if depth > 64 {
        return Err("Notebook scan exceeds 64 directory levels".into());
    }
    let mut entries = std::fs::read_dir(dir)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        if found.len() >= limit {
            break;
        }
        *budget = budget
            .checked_sub(1)
            .ok_or("Notebook scan exceeds 20,000 entries")?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        let path = entry.path();
        if kind.is_dir() {
            collect(root, &path, found, limit, budget, depth + 1)?;
        } else if kind.is_file() && entry.file_name() == MANIFEST_NAME {
            found.push(
                path.strip_prefix(root)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notebook_tools_create_reference_and_reorder_real_markdown() {
        let root = tempfile::tempdir().unwrap();
        let notes = Notes {
            root: root.path().to_path_buf(),
        };
        assert!(tools().iter().any(|tool| tool["name"] == "notebookCreate"));
        let created = call(
            &notes,
            "notebookCreate",
            json!({"path":"Code","title":"Code Notes"}),
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&created).unwrap()["path"],
            "Code/notebook.json"
        );
        assert!(root.path().join("Code/overview.md").is_file());
        call(
            &notes,
            "notebookAddPage",
            json!({"notebook":"Code","title":"API","content":"# API\n"}),
        )
        .unwrap();
        std::fs::write(root.path().join("Shared.md"), "# Shared\n").unwrap();
        call(
            &notes,
            "notebookAddPage",
            json!({"notebook":"Code","existing_path":"Shared.md"}),
        )
        .unwrap();
        let moved = call(
            &notes,
            "notebookMovePage",
            json!({"notebook":"Code","page":2,"direction":"up"}),
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&moved).unwrap()["pages"][0],
            "API.md"
        );
        assert_eq!(
            call(&notes, "notebookList", json!({})).unwrap(),
            "Code/notebook.json"
        );
        assert!(
            call(&notes, "notebookRead", json!({"path":"Code/notebook.json"}))
                .unwrap()
                .contains("Shared.md")
        );
        assert!(call(&notes, "notebookCreate", json!({"path":"Code"})).is_err());
        assert!(call(
            &notes,
            "notebookAddPage",
            json!({"notebook":"Code","title":"API"})
        )
        .is_err());
        assert_eq!(
            std::fs::read_to_string(root.path().join("Code/API.md")).unwrap(),
            "# API\n"
        );
        assert!(call(&notes, "notebookCreate", json!({"path":"../escape"})).is_err());
        assert!(call(
            &notes,
            "notebookMovePage",
            json!({"notebook":"Code","page":99,"direction":"up"})
        )
        .is_err());
        assert!(call(
            &notes,
            "notebookAddPage",
            json!({"notebook":"Code","title":"Bad","existing_path":"Shared.md"})
        )
        .is_err());
    }
    #[test]
    #[cfg(unix)]
    fn notebook_tools_cannot_follow_symlinks_outside_vault() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        let notes = Notes {
            root: root.path().to_path_buf(),
        };
        assert!(call(&notes, "notebookCreate", json!({"path":"escape/Book"})).is_err());
        assert!(!outside.path().join("Book").exists());
        NotebookBinding::create(&root.path().join("Book"), None).unwrap();
        std::fs::write(outside.path().join("page.md"), "private").unwrap();
        assert!(call(
            &notes,
            "notebookAddPage",
            json!({"notebook":"Book","existing_path":"escape/page.md"})
        )
        .is_err());
    }
}
