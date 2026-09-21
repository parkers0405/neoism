use super::*;
use std::io::Write;

impl NotebookBinding {
    pub fn create_untitled_in(dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
        for index in 1..10_000 {
            let name = if index == 1 {
                "Untitled Notebook".to_string()
            } else {
                format!("Untitled Notebook {index}")
            };
            let path = dir.join(&name);
            match std::fs::create_dir(&path) {
                Ok(()) => return Self::create(&path, Some(&name)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    continue
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        Err("Could not allocate a new notebook folder".into())
    }

    /// Shared by the desktop and its Notes MCP server. Never overwrites an
    /// existing manifest or page, and does not import nested notebooks/symlinks.
    pub fn create(folder: &Path, title: Option<&str>) -> Result<Self, String> {
        if title.is_some_and(|title| title.trim().is_empty()) {
            return Err("Notebook title cannot be empty".into());
        }
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        let folder = std::fs::canonicalize(folder).map_err(|error| error.to_string())?;
        let manifest_path = folder.join(MANIFEST_NAME);
        if manifest_path.exists() {
            return Err("This folder already contains notebook.json".into());
        }
        let mut paths = Vec::new();
        collect_pages(&folder, &folder, &mut paths, &mut 20_000)?;
        paths.sort();
        if paths.is_empty() {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(folder.join("overview.md"))
                .map_err(|error| error.to_string())?;
            file.write_all(b"# Overview\n\nWelcome to your notebook.\n")
                .map_err(|error| error.to_string())?;
            paths.push("overview.md".into());
        }
        let title = title.map(str::trim).map(str::to_string).unwrap_or_else(|| {
            folder
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        let pages = paths
            .into_iter()
            .map(|path| {
                let section = path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .map(|parent| parent.to_string_lossy().replace(['/', '\\'], " / "));
                if section.is_some() {
                    NotebookPage::Named {
                        path,
                        title: None,
                        section,
                    }
                } else {
                    NotebookPage::Path(path)
                }
            })
            .collect();
        let book = DocumentationNotebook::new(
            manifest_path.clone(),
            NotebookManifest { title, pages },
        )?;
        let bytes = serde_json::to_vec_pretty(&book.manifest)
            .map_err(|error| error.to_string())?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&manifest_path)
            .map_err(|error| error.to_string())?;
        let result = file.write_all(&bytes).and_then(|_| file.sync_all());
        drop(file);
        if let Err(error) = result {
            let _ = std::fs::remove_file(&manifest_path);
            return Err(error.to_string());
        }
        Self::load(&manifest_path)
    }
}

fn collect_pages(
    root: &Path,
    folder: &Path,
    pages: &mut Vec<PathBuf>,
    remaining: &mut usize,
) -> Result<(), String> {
    if folder
        .components()
        .count()
        .saturating_sub(root.components().count())
        > 64
    {
        return Err("Notebook directory nesting exceeds 64 levels".into());
    }
    if folder != root && folder.join(MANIFEST_NAME).exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(folder).map_err(|error| error.to_string())? {
        *remaining = remaining.checked_sub(1).ok_or(
            "This folder is too large to import; choose a smaller documentation folder",
        )?;
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if pages.len() >= 10_000 {
            return Err("Notebook exceeds 10,000 pages".into());
        }
        if kind.is_dir() {
            collect_pages(root, &path, pages, remaining)?;
        } else if crate::editor::markdown::is_markdown_path(&path) {
            pages.push(
                path.strip_prefix(root)
                    .map_err(|error| error.to_string())?
                    .to_path_buf(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn create_imports_existing_pages_without_replacing_them() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("internals")).unwrap();
        std::fs::write(root.path().join("internals/api.md"), "# Original\n").unwrap();
        let book = NotebookBinding::create(root.path(), Some("Code Notes")).unwrap();
        let session = book.session.lock().unwrap();
        assert_eq!(session.manifest.title, "Code Notes");
        assert_eq!(session.manifest.pages.len(), 1);
        assert_eq!(session.manifest.pages[0].section(), Some("internals"));
        assert_eq!(
            std::fs::read_to_string(root.path().join("internals/api.md")).unwrap(),
            "# Original\n"
        );
        assert!(NotebookBinding::create(root.path(), None).is_err());
    }
    #[test]
    fn direct_creation_uses_clicked_parent_and_never_reuses_an_existing_folder() {
        let root = tempfile::tempdir().unwrap();
        let clicked = root.path().join("Clicked Folder");
        let first = NotebookBinding::create_untitled_in(&clicked).unwrap();
        let second = NotebookBinding::create_untitled_in(&clicked).unwrap();
        let clicked = std::fs::canonicalize(clicked).unwrap();
        assert_eq!(first.path, clicked.join("Untitled Notebook/notebook.json"));
        assert_eq!(
            second.path,
            clicked.join("Untitled Notebook 2/notebook.json")
        );
        assert_eq!(
            first.session.lock().unwrap().manifest.title,
            "Untitled Notebook"
        );
        assert!(first.path.parent().unwrap().join("overview.md").is_file());
    }

    #[test]
    fn empty_folder_gets_an_overview() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("New Notebook");
        let binding = NotebookBinding::create(&folder, None).unwrap();
        let book = binding.session.lock().unwrap();
        assert_eq!(book.manifest.title, "New Notebook");
        assert_eq!(book.manifest.pages[0].path(), Path::new("overview.md"));
        assert!(folder.join("overview.md").is_file());
    }
}
