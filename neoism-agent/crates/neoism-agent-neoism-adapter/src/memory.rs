use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

use neoism_agent_service_api::{
    MemoryEntry, MemoryLocation, MemoryRequest, MemoryService, MemoryWriteRequest,
    ScopeChoice, SemanticMemoryIndex, ServiceError, ServiceFuture, SystemContextFragment,
};
use sha2::{Digest, Sha256};

const INDEX_FILE: &str = "MEMORY.md";
const PROJECT_DIR: &str = ".neoism/memory";
const USER_DIR: &str = "user";
const MAX_INDEX_CHARS: usize = 12_000;
const MAX_EMBED_CHARS: usize = 8_000;

#[derive(Clone)]
struct Root {
    scope: &'static str,
    label: String,
    path: PathBuf,
}

pub(crate) struct NeoismMemoryService {
    semantic: RwLock<Option<Arc<dyn SemanticMemoryIndex>>>,
}

impl NeoismMemoryService {
    pub(crate) fn new() -> Self {
        Self { semantic: RwLock::new(None) }
    }

    fn roots(&self, request: &MemoryRequest, include_missing: bool) -> Result<Vec<Root>, ServiceError> {
        roots_for_scope(
            &request.working_directory,
            request.scope_id.as_deref().unwrap_or("project"),
            include_missing,
        )
    }

    async fn semantic_recall(&self, request: &MemoryRequest, query: &str, limit: usize) -> Result<Option<Vec<MemoryEntry>>, ServiceError> {
        let index = self.semantic.read().unwrap().clone();
        let Some(index) = index.filter(|index| index.available()) else { return Ok(None) };
        let Some(model) = index.model() else { return Ok(None) };
        if query.trim().is_empty() { return Ok(None); }
        let roots = self.roots(request, false)?;
        for root in &roots { sync_semantic(index.as_ref(), root, &model).await?; }
        let vectors = index.embed(&[query.to_string()]).await?;
        let Some(vector) = vectors.into_iter().next().filter(|vector| !vector.is_empty()) else { return Ok(None) };
        let root_keys = roots.iter().map(|root| root.path.to_string_lossy().to_string()).collect::<Vec<_>>();
        let ranked = index.search(&root_keys, &vector, &model, limit).await?;
        let mut entries = Vec::new();
        for hit in ranked {
            let absolute = PathBuf::from(&hit.key);
            let Ok(content) = std::fs::read_to_string(&absolute) else {
                let _ = index.delete(&hit.key).await;
                continue;
            };
            if let Some(root) = roots.iter().find(|root| absolute.starts_with(&root.path)) {
                let mut value = entry(root, &absolute, &content, Some(content.clone()));
                value.snippet = Some(snippet(&content));
                value.semantic_distance = Some(hit.distance);
                entries.push(value);
            }
        }
        Ok(Some(entries))
    }
}

impl MemoryService for NeoismMemoryService {
    fn scope_choices(&self) -> Vec<ScopeChoice> {
        vec![
            choice("auto", "Project and user", "Search workspace memory and personal memory."),
            choice("project", "Project", "Use agent-owned memory for this workspace."),
            choice("user", "User", "Use personal Neoism Agent memory."),
            choice("all", "All", "Search project and personal memory."),
        ]
    }

    fn default_scope_id(&self) -> &str { "project" }

    fn init(&self, request: &MemoryRequest) -> Result<Vec<MemoryLocation>, ServiceError> {
        let roots = self.roots(request, true)?;
        for root in &roots { ensure_root(root)?; }
        Ok(roots.iter().map(location).collect())
    }

    fn list(&self, request: &MemoryRequest, limit: usize) -> Result<Vec<MemoryEntry>, ServiceError> {
        let mut entries = Vec::new();
        for root in self.roots(request, false)? {
            for path in files(&root)?.into_iter().take(limit) {
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                entries.push(entry(&root, &path, &content, None));
            }
        }
        Ok(entries)
    }

    fn read(&self, request: &MemoryRequest, path: &str) -> Result<MemoryEntry, ServiceError> {
        let roots = self.roots(request, false)?;
        let (root, absolute) = existing_file(&roots, path)?;
        let content = std::fs::read_to_string(&absolute)?;
        Ok(entry(root, &absolute, &content, Some(content.clone())))
    }

    fn write(&self, request: &MemoryWriteRequest) -> Result<MemoryEntry, ServiceError> {
        let kind = request.kind.as_deref().unwrap_or("project");
        let root = write_root(&request.request, kind)?;
        ensure_root(&root)?;
        let file_name = request.file_name.as_deref().map(safe_file_name).filter(|name| !name.is_empty())
            .unwrap_or_else(|| memory_file_name(kind, &request.name));
        let path = root.path.join(file_name);
        if let Some(existing) = find_similar(&root, &path, &request.name, &request.description)? {
            return Err(ServiceError::new(format!("a memory covering this already exists at {}; read and update that file", relative(&root, &existing))));
        }
        let today = today_utc();
        let content = render(
            &request.name, &request.description, kind, root.scope,
            request.origin.as_deref().unwrap_or("neoism-agent"),
            request.created.as_deref().unwrap_or(&today),
            request.updated.as_deref().unwrap_or(&today),
            request.body.as_deref().unwrap_or(""),
        );
        std::fs::write(&path, &content)?;
        update_index(&root, &path, &request.name, &request.description)?;
        Ok(entry(&root, &path, &content, Some(content.clone())))
    }

    fn search(&self, request: &MemoryRequest, query: &str, limit: usize) -> Result<Vec<MemoryEntry>, ServiceError> {
        let mut all = Vec::new();
        for root in self.roots(request, false)? { all.extend(keyword_search(&root, query, limit)?); }
        all.truncate(limit);
        Ok(all)
    }

    fn recall<'a>(&'a self, request: &'a MemoryRequest, query: &'a str, limit: usize) -> ServiceFuture<'a, Result<Vec<MemoryEntry>, ServiceError>> {
        Box::pin(async move {
            match self.semantic_recall(request, query, limit).await {
                Ok(Some(entries)) => Ok(entries),
                Ok(None) => self.search(request, query, limit),
                Err(error) => {
                    tracing::warn!(%error, "semantic memory recall failed; falling back to keyword recall");
                    self.search(request, query, limit)
                }
            }
        })
    }

    fn context_fragments(&self, working_directory: &Path) -> Vec<SystemContextFragment> {
        let request = MemoryRequest::new(working_directory).with_scope("project");
        let Ok(roots) = self.roots(&request, false) else { return Vec::new() };
        roots.into_iter().filter_map(|root| {
            let text = std::fs::read_to_string(root.path.join(INDEX_FILE)).ok()?;
            if !text.lines().any(|line| line.trim_start().starts_with("- [")) { return None; }
            let text = truncate(text.trim(), MAX_INDEX_CHARS);
            Some(SystemContextFragment {
                id: format!("memory:{}:{}", root.scope, root.path.display()),
                content: format!(
                    "Persistent {} memory index ({}), stored in {}. Access it only with the native memory tool. Use memory with operation=recall before repeating project discovery, operation=read for linked topic files, and operation=write for new durable facts.\n{}",
                    root.scope, root.label, root.path.display(), text
                ),
            })
        }).collect()
    }

    fn set_semantic_index(&self, index: Option<Arc<dyn SemanticMemoryIndex>>) {
        *self.semantic.write().unwrap() = index;
    }
}

async fn sync_semantic(index: &dyn SemanticMemoryIndex, root: &Root, model: &str) -> Result<(), ServiceError> {
    let root_key = root.path.to_string_lossy().to_string();
    let indexed = index.hashes(&root_key, model).await?.into_iter().collect::<HashMap<_,_>>();
    let mut disk = HashSet::new();
    let mut stale = Vec::new();
    for path in files(root)? {
        let key = path.to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let name = path.file_stem().and_then(|value| value.to_str()).unwrap_or_default();
        let input = format!("{}\n{}\n{}", name, frontmatter(&text,"description").unwrap_or_default(), text).chars().take(MAX_EMBED_CHARS).collect::<String>();
        let hash = format!("{:x}", Sha256::digest(input.as_bytes()));
        disk.insert(key.clone());
        if indexed.get(&key) != Some(&hash) { stale.push((key,hash,input)); }
    }
    for key in indexed.keys().filter(|key| !disk.contains(*key)) { index.delete(key).await?; }
    for chunk in stale.chunks(16) {
        let inputs = chunk.iter().map(|(_,_,input)|input.clone()).collect::<Vec<_>>();
        for ((key,hash,_),vector) in chunk.iter().zip(index.embed(&inputs).await?) {
            if !vector.is_empty() { index.upsert(key,&root_key,hash,model,unix_millis(),&vector).await?; }
        }
    }
    Ok(())
}

fn roots_for_scope(cwd: &Path, scope: &str, include_missing: bool) -> Result<Vec<Root>, ServiceError> {
    let mut roots = match scope {
        "project" => project_roots(cwd, include_missing)?,
        "user" => vec![user_root()?],
        "auto" | "all" | "" => { let mut roots=project_roots(cwd,include_missing)?; roots.push(user_root()?); roots },
        other => return Err(ServiceError::new(format!("unknown memory scope {other}"))),
    };
    if !include_missing { roots.retain(|root| root.path.is_dir()); }
    Ok(roots)
}

fn project_roots(cwd: &Path, include_missing: bool) -> Result<Vec<Root>, ServiceError> {
    let root = project_root(cwd)?;
    if include_missing || root.path.is_dir() { Ok(vec![root]) } else { Ok(Vec::new()) }
}

fn project_root(cwd: &Path) -> Result<Root, ServiceError> {
    let workspace = crate::config::NeoismConfigSourceService::workspace_root(cwd);
    let label = workspace.file_name().and_then(|value| value.to_str()).unwrap_or("workspace").to_string();
    let root = Root { scope:"project", label, path:workspace.join(PROJECT_DIR) };
    move_vault_memory(&root, &vault_memory_roots(cwd)?)?;
    Ok(root)
}

fn user_root() -> Result<Root, ServiceError> {
    let root = Root { scope:"user", label:"Neoism user".to_string(), path:memory_home().join(USER_DIR) };
    let old = neoism_workspace_index::default_notes_workspace().notes_workspace_dir().join("Memory/Personal");
    move_vault_memory(&root, &[old])?;
    Ok(root)
}

fn write_root(request: &MemoryRequest, kind: &str) -> Result<Root, ServiceError> {
    match request.scope_id.as_deref().unwrap_or("project") {
        "user" => user_root(), "project" | "all" => project_root(&request.working_directory),
        "auto" | "" if matches!(kind,"personal"|"preference") => user_root(),
        "auto" | "" => project_root(&request.working_directory), other => Err(ServiceError::new(format!("unknown memory scope {other}"))),
    }
}

fn memory_home() -> PathBuf {
    if let Some(path) = std::env::var_os("NEOISM_AGENT_MEMORY_HOME") { return PathBuf::from(path); }
    if let Some(path) = std::env::var_os("XDG_DATA_HOME") { return PathBuf::from(path).join("neoism/memory"); }
    if let Some(home) = std::env::var_os("HOME") { return PathBuf::from(home).join(".local/share/neoism/memory"); }
    PathBuf::from(".neoism-user-memory")
}

fn vault_memory_roots(cwd: &Path) -> Result<Vec<PathBuf>, ServiceError> {
    let Some(workspace) = neoism_workspace_index::linked_project_for_code_dir(cwd).map_err(error)? else {
        return Ok(Vec::new());
    };
    let scoped = workspace.notes_workspace_dir().join("Memory");
    let vault = workspace.as_vault_workspace().notes_workspace_dir().join("Memory");
    Ok(if scoped == vault { vec![scoped] } else { vec![scoped, vault] })
}

fn move_vault_memory(root: &Root, sources: &[PathBuf]) -> Result<(), ServiceError> {
    if !sources.iter().any(|path| path.is_dir()) { return Ok(()); }
    std::fs::create_dir_all(&root.path)?;
    for source_root in sources {
        let Ok(entries) = std::fs::read_dir(source_root) else { continue };
        for entry in entries.filter_map(Result::ok) {
            let source = entry.path();
            if !source.is_file() || source.extension().and_then(|value| value.to_str()) != Some("md") { continue; }
            let target = root.path.join(entry.file_name());
            if target.exists() {
                if source.file_name().and_then(|value| value.to_str()) == Some(INDEX_FILE) {
                    merge_memory_index(&target, &source)?;
                    std::fs::remove_file(source)?;
                    continue;
                }
                if std::fs::read(&target)? == std::fs::read(&source)? {
                    std::fs::remove_file(source)?;
                    continue;
                }
            }
            let target = available_import_path(&target);
            if std::fs::rename(&source, &target).is_err() {
                std::fs::copy(&source, &target)?;
                std::fs::remove_file(source)?;
            }
        }
        let _ = std::fs::remove_dir(source_root);
    }
    Ok(())
}

fn merge_memory_index(target: &Path, source: &Path) -> Result<(), ServiceError> {
    let mut current = std::fs::read_to_string(target)?;
    let incoming = std::fs::read_to_string(source)?;
    for line in incoming.lines().filter(|line| line.trim_start().starts_with("- [")) {
        if !current.lines().any(|existing| existing == line) {
            if !current.ends_with('\n') { current.push('\n'); }
            current.push_str(line);
            current.push('\n');
        }
    }
    std::fs::write(target, current)?;
    Ok(())
}

fn available_import_path(path: &Path) -> PathBuf {
    if !path.exists() { return path.to_path_buf(); }
    let stem = path.file_stem().and_then(|value| value.to_str()).unwrap_or("memory");
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    (1..).map(|index| parent.join(format!("{stem}_imported_{index}.md")))
        .find(|candidate| !candidate.exists()).expect("memory import path")
}

fn ensure_root(root: &Root) -> Result<(), ServiceError> {
    std::fs::create_dir_all(&root.path)?;
    let index=root.path.join(INDEX_FILE);
    if !index.exists() { std::fs::write(index,format!("# Memory\n\nCompact {} memory index for Neoism. Keep this file short: one link and one-line summary per topic.\n\n",root.scope))?; }
    Ok(())
}

fn files(root: &Root) -> Result<Vec<PathBuf>, ServiceError> {
    let Ok(entries)=std::fs::read_dir(&root.path) else{return Ok(Vec::new())};
    let mut paths=entries.filter_map(Result::ok).map(|e|e.path()).filter(|p|p.is_file()&&p.extension().and_then(|v|v.to_str())==Some("md")&&p.file_name().and_then(|v|v.to_str())!=Some(INDEX_FILE)).collect::<Vec<_>>(); paths.sort(); Ok(paths)
}

fn safe_path(root:&Root,raw:&str)->Result<PathBuf,ServiceError>{let path=Path::new(raw);if path.is_absolute(){return path.starts_with(&root.path).then(||path.to_path_buf()).ok_or_else(||ServiceError::new("memory path is outside the selected memory root"));}if path.components().any(|c|matches!(c,Component::ParentDir|Component::RootDir|Component::Prefix(_))){return Err(ServiceError::new("memory path must be relative"));}let relative=path.strip_prefix(PROJECT_DIR).or_else(|_|path.strip_prefix(USER_DIR)).unwrap_or(path);Ok(root.path.join(relative))}
fn existing_file<'a>(roots:&'a[Root],raw:&str)->Result<(&'a Root,PathBuf),ServiceError>{for root in roots{if let Ok(path)=safe_path(root,raw){if path.is_file(){return Ok((root,path))}}}Err(ServiceError::new(format!("no memory file {raw}")))}
fn location(root:&Root)->MemoryLocation{MemoryLocation{scope_id:root.scope.to_string(),label:root.label.clone(),storage_key:root.path.to_string_lossy().to_string()}}
fn entry(root:&Root,path:&Path,text:&str,content:Option<String>)->MemoryEntry{MemoryEntry{location:location(root),path:relative(root,path),description:frontmatter(text,"description"),kind:frontmatter(text,"type"),content,snippet:None,semantic_distance:None}}
fn relative(root:&Root,path:&Path)->String{path.strip_prefix(&root.path).unwrap_or(path).components().filter_map(|c|if let Component::Normal(v)=c{v.to_str()}else{None}).collect::<Vec<_>>().join("/")}

fn keyword_search(root:&Root,query:&str,limit:usize)->Result<Vec<MemoryEntry>,ServiceError>{let needle=query.trim().to_ascii_lowercase();let tokens=needle.split_whitespace().collect::<Vec<_>>();let mut hits=Vec::new();for path in files(root)?{let text=std::fs::read_to_string(&path).unwrap_or_default();let name=path.file_name().and_then(|v|v.to_str()).unwrap_or_default().to_ascii_lowercase();let description=frontmatter(&text,"description").unwrap_or_default().to_ascii_lowercase();let haystack=format!("{name}\n{description}\n{}",text.to_ascii_lowercase());let matched=tokens.iter().filter(|token|haystack.contains(**token)).count();if tokens.is_empty()||matched>0{let mut value=entry(root,&path,&text,None);value.snippet=Some(snippet(&haystack));hits.push((usize::MAX-matched,value));}}hits.sort_by_key(|(rank,_)|*rank);Ok(hits.into_iter().take(limit).map(|(_,entry)|entry).collect())}

fn update_index(root:&Root,path:&Path,name:&str,description:&str)->Result<(),ServiceError>{let index=root.path.join(INDEX_FILE);let rel=relative(root,path);let mut lines=std::fs::read_to_string(&index).unwrap_or_default().lines().filter(|line|!line.contains(&format!("]({rel})"))).map(str::to_string).collect::<Vec<_>>();lines.push(format!("- [{}]({}) - {}",name.trim(),rel,description.trim()));std::fs::write(index,format!("{}\n",lines.join("\n")))?;Ok(())}
fn render(name:&str,description:&str,kind:&str,scope:&str,origin:&str,created:&str,updated:&str,body:&str)->String{format!("---\nname: {}\ndescription: {}\ntype: {}\nscope: {}\norigin: {}\ncreated: {}\nupdated: {}\n---\n\n{}\n",yaml(name),yaml(description),yaml(kind),yaml(scope),yaml(origin),yaml(created),yaml(updated),body.trim())}
fn frontmatter(source:&str,key:&str)->Option<String>{let mut lines=source.lines();if lines.next()?!="---"{return None}for line in lines{if line=="---"{break}if let Some((candidate,value))=line.split_once(':'){if candidate.trim()==key{return Some(value.trim().trim_matches('"').to_string())}}}None}
fn find_similar(root:&Root,target:&Path,name:&str,description:&str)->Result<Option<PathBuf>,ServiceError>{let incoming=tokens(&format!("{name} {description}"));if incoming.len()<3{return Ok(None)}for path in files(root)?{if path==target{continue}let text=std::fs::read_to_string(&path).unwrap_or_default();let existing=tokens(&format!("{} {}",frontmatter(&text,"name").unwrap_or_default(),frontmatter(&text,"description").unwrap_or_default()));let shared=incoming.intersection(&existing).count();let smaller=incoming.len().min(existing.len());if smaller>0&&shared*10>=smaller*7{return Ok(Some(path))}}Ok(None)}
fn tokens(value:&str)->BTreeSet<String>{value.to_lowercase().split(|c:char|!c.is_ascii_alphanumeric()).filter(|v|v.len()>2).map(str::to_string).collect()}
fn memory_file_name(kind:&str,name:&str)->String{let kind=slug(kind);let name=slug(name);format!("{}.md",if name.starts_with(&format!("{kind}_")){name}else{format!("{kind}_{name}")}.trim_matches('_'))}
fn safe_file_name(value:&str)->String{let value=value.trim().strip_suffix(".md").unwrap_or(value.trim());let value=slug(value);if value.is_empty(){value}else{format!("{value}.md")}}
fn slug(value:&str)->String{let mut out=String::new();let mut separator=false;for ch in value.trim().chars(){if ch.is_ascii_alphanumeric(){out.push(ch.to_ascii_lowercase());separator=false}else if !separator{out.push('_');separator=true}}out.trim_matches('_').to_string()}
fn yaml(value:&str)->String{format!("\"{}\"",value.replace('\\',"\\\\").replace('"',"\\\""))}
fn truncate(text:&str,max:usize)->String{if text.len()<=max{return text.to_string()}let mut out=String::new();for line in text.lines(){if out.len()+line.len()+1>max{break}out.push_str(line);out.push('\n')}out.push_str("(index truncated - use memory.list or memory.recall for omitted entries)");out}
fn choice(id:&str,label:&str,description:&str)->ScopeChoice{ScopeChoice{id:id.to_string(),label:label.to_string(),description:Some(description.to_string())}}
fn error(error:impl std::fmt::Display)->ServiceError{ServiceError::new(error.to_string())}
fn unix_millis()->i64{std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis()as i64).unwrap_or(0)}
fn snippet(value:&str)->String{value.chars().take(240).collect::<String>().replace('\n'," ")}
fn today_utc()->String{let days=unix_millis()/86_400_000;let z=days+719468;let era=if z>=0{z}else{z-146096}/146097;let doe=z-era*146097;let yoe=(doe-doe/1460+doe/36524-doe/146096)/365;let y=yoe+era*400;let doy=doe-(365*yoe+yoe/4-yoe/100);let mp=(5*doy+2)/153;let d=doy-(153*mp+2)/5+1;let m=mp+if mp<10{3}else{-9};let year=y+if m<=2{1}else{0};format!("{year:04}-{m:02}-{d:02}")}

#[cfg(test)]
mod tests {
    use super::*;

    struct NotesHome {
        root: PathBuf,
        previous_notes: Option<std::ffi::OsString>,
        previous_memory: Option<std::ffi::OsString>,
    }

    impl NotesHome {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "neoism-memory-adapter-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
            ));
            std::fs::create_dir_all(&root).unwrap();
            let previous_notes = std::env::var_os("NEOISM_NOTES_HOME");
            let previous_memory = std::env::var_os("NEOISM_AGENT_MEMORY_HOME");
            unsafe { std::env::set_var("NEOISM_NOTES_HOME", &root); }
            unsafe { std::env::set_var("NEOISM_AGENT_MEMORY_HOME", root.join("agent-memory")); }
            Self { root, previous_notes, previous_memory }
        }
    }

    impl Drop for NotesHome {
        fn drop(&mut self) {
            match self.previous_notes.take() {
                Some(value) => unsafe { std::env::set_var("NEOISM_NOTES_HOME", value) },
                None => unsafe { std::env::remove_var("NEOISM_NOTES_HOME") },
            }
            match self.previous_memory.take() {
                Some(value) => unsafe { std::env::set_var("NEOISM_AGENT_MEMORY_HOME", value) },
                None => unsafe { std::env::remove_var("NEOISM_AGENT_MEMORY_HOME") },
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn write_request(cwd: &Path, scope: &str, kind: &str, name: &str) -> MemoryWriteRequest {
        MemoryWriteRequest {
            request: MemoryRequest::new(cwd).with_scope(scope),
            name: name.to_string(),
            description: format!("description for {name}"),
            kind: Some(kind.to_string()),
            body: Some(format!("body for {name}")),
            file_name: None,
            created: Some("2026-08-24".to_string()),
            updated: Some("2026-08-24".to_string()),
            origin: Some("test".to_string()),
        }
    }

    #[test]
    fn memory_is_agent_owned_and_moves_old_user_memory() {
        let _lock = crate::test_env_lock().lock().unwrap_or_else(|error| error.into_inner());
        let home = NotesHome::new("canonical");
        let cwd = home.root.join("code");
        std::fs::create_dir_all(&cwd).unwrap();
        let legacy_user = home.root.join("Default/Memory/Personal");
        std::fs::create_dir_all(&legacy_user).unwrap();
        std::fs::write(legacy_user.join("personal_legacy.md"), "legacy user fact").unwrap();

        let service = NeoismMemoryService::new();
        service.init(&MemoryRequest::new(&cwd).with_scope("all")).unwrap();
        service.write(&write_request(&cwd, "project", "feature", "project fact")).unwrap();
        service.write(&write_request(&cwd, "user", "personal", "user fact")).unwrap();

        assert!(cwd.join(".neoism/memory/feature_project_fact.md").is_file());
        assert!(home.root.join("agent-memory/user/personal_user_fact.md").is_file());
        assert!(home.root.join("agent-memory/user/personal_legacy.md").is_file());
        assert!(!legacy_user.join("personal_legacy.md").exists());
    }

    #[test]
    fn linked_vault_memory_is_moved_into_workspace_memory() {
        let _lock = crate::test_env_lock().lock().unwrap_or_else(|error| error.into_inner());
        let home = NotesHome::new("linked");
        let cwd = home.root.join("code/project");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(cwd.join(".neoism")).unwrap();
        let mut workspace = neoism_workspace_index::default_notes_workspace();
        workspace.root = cwd.clone();
        workspace.config.notes.workspace = "Linked".to_string();
        workspace.config.notes.vault_id = None;
        workspace.config.notes.scope = PathBuf::from("Projects/Specific");
        neoism_workspace_index::config::link_workspace_to_vault_project(&mut workspace, &cwd).unwrap();

        let vault = home.root.join("Linked");
        std::fs::create_dir_all(vault.join("Memory")).unwrap();
        std::fs::write(vault.join("Memory/shared.md"), "---\ndescription: shared owning vault fact\ntype: project\n---\n").unwrap();
        let service = NeoismMemoryService::new();
        service.write(&write_request(&cwd, "project", "feature", "specific fact")).unwrap();

        let entries = service.search(&MemoryRequest::new(&cwd).with_scope("project"), "fact", 10).unwrap();
        assert!(entries.iter().any(|entry| entry.path == "feature_specific_fact.md"));
        assert!(entries.iter().any(|entry| entry.path == "shared.md"));
        assert!(cwd.join(".neoism/memory/feature_specific_fact.md").is_file());
        assert!(cwd.join(".neoism/memory/shared.md").is_file());
        assert!(!vault.join("Memory/shared.md").exists());
    }
}
