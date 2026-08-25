use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

use neoism_agent_service_api::{
    BuiltinMcpCallResult, BuiltinMcpContent, BuiltinMcpService, BuiltinMcpTool,
    MemoryEntry, MemoryLocation, MemoryRequest, MemoryService, MemoryWriteRequest,
    ScopeChoice, SemanticMemoryIndex, ServiceError, ServiceFuture, SystemContextFragment,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const MCP_ID: &str = "neoism-memory";
const INDEX_FILE: &str = "MEMORY.md";
const PROJECT_DIR: &str = "Memory";
const USER_DIR: &str = "Memory/Personal";
const MAX_INDEX_CHARS: usize = 12_000;
const MAX_EMBED_CHARS: usize = 8_000;

#[derive(Clone)]
struct Root {
    scope: &'static str,
    label: String,
    path: PathBuf,
    workspace: neoism_workspace_index::config::NeoismWorkspace,
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
            request.scope_id.as_deref().unwrap_or("auto"),
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
            choice("auto", "Project and user", "Search project memory and personal memory."),
            choice("project", "Project", "Use the most-specific linked project memory."),
            choice("user", "User", "Use personal memory in the default vault."),
            choice("all", "All", "Search project and personal memory."),
        ]
    }

    fn default_scope_id(&self) -> &str { "auto" }

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
        let request = MemoryRequest::new(working_directory).with_scope("auto");
        let Ok(roots) = self.roots(&request, false) else { return Vec::new() };
        roots.into_iter().filter_map(|root| {
            let text = std::fs::read_to_string(root.path.join(INDEX_FILE)).ok()?;
            if !text.lines().any(|line| line.trim_start().starts_with("- [")) { return None; }
            let text = truncate(text.trim(), MAX_INDEX_CHARS);
            Some(SystemContextFragment {
                id: format!("memory:{}:{}", root.scope, root.path.display()),
                content: format!(
                    "Persistent {} memory index (vault {}), stored in {}. That folder is OUTSIDE the workspace directory. Access it only with the {} memory tools. Use memory.recall before repeating project discovery, memory.read for linked topic files, and memory.write for new durable facts.\n{}",
                    root.scope, root.label, root.path.display(), MCP_ID, text
                ),
            })
        }).collect()
    }

    fn set_semantic_index(&self, index: Option<Arc<dyn SemanticMemoryIndex>>) {
        *self.semantic.write().unwrap() = index;
    }
}

impl BuiltinMcpService for NeoismMemoryService {
    fn id(&self) -> &str { MCP_ID }

    fn tools(&self) -> Vec<BuiltinMcpTool> {
        let description = "Scope defaults to project and user memory. Use user only for facts about the user; use project for codebase and workflow facts.";
        let scope = json!({"type":"string","enum":["auto","project","user","all"]});
        vec![
            tool("memory.init", "Create Neoism memory folders and compact indexes", json!({"type":"object","properties":{"scope":scope.clone()},"description":description})),
            tool("memory.list", "List Neoism memory files", json!({"type":"object","properties":{"scope":scope.clone(),"limit":{"type":"integer"}},"description":description})),
            tool("memory.recall", "Search Neoism memory by meaning with keyword fallback", json!({"type":"object","properties":{"query":{"type":"string"},"scope":scope.clone(),"limit":{"type":"integer"}},"required":["query"],"description":description})),
            tool("memory.search", "Search Neoism memory text", json!({"type":"object","properties":{"query":{"type":"string"},"scope":scope.clone(),"limit":{"type":"integer"}},"required":["query"],"description":description})),
            tool("memory.read", "Read a memory file by memory-relative path", json!({"type":"object","properties":{"path":{"type":"string"},"scope":scope.clone()},"required":["path"]})),
            tool("memory.write", "Write or update a Neoism memory topic and compact index", json!({"type":"object","properties":{"name":{"type":"string"},"description":{"type":"string"},"type":{"type":"string","description":"project, feedback, bug, feature, reference, perf, preference, workflow, or personal"},"scope":scope,"body":{"type":"string"},"content":{"type":"string"},"fileName":{"type":"string"},"created":{"type":"string"},"updated":{"type":"string"},"origin":{"type":"string"}},"required":["name","description"]})),
        ]
    }

    fn call_tool(&self, cwd: &Path, tool_name: &str, arguments: Value) -> Result<BuiltinMcpCallResult, ServiceError> {
        if tool_name == "memory.recall" { return Err(ServiceError::new("memory.recall requires asynchronous dispatch")); }
        self.call_sync(cwd, tool_name, arguments)
    }

    fn call_tool_async<'a>(&'a self, cwd: &'a Path, tool_name: &'a str, arguments: Value) -> ServiceFuture<'a, Result<BuiltinMcpCallResult, ServiceError>> {
        Box::pin(async move {
            if tool_name != "memory.recall" { return self.call_sync(cwd, tool_name, arguments); }
            let request = request(cwd, &arguments);
            let query = required(&arguments, "query")?;
            let limit = limit(&arguments);
            match self.semantic_recall(&request, &query, limit).await {
                Ok(Some(entries)) => text_result(json!({"operation":"recall","query":query,"mode":"semantic","hits":entries.iter().map(entry_json).collect::<Vec<_>>() })),
                Ok(None) => {
                    let entries = self.search(&request, &query, limit)?;
                    text_result(json!({"operation":"recall","query":query,"hits":grouped_json(&entries)}))
                }
                Err(error) => {
                    tracing::warn!(%error, "semantic memory recall failed; falling back to keyword recall");
                    let entries = self.search(&request, &query, limit)?;
                    text_result(json!({"operation":"recall","query":query,"hits":grouped_json(&entries)}))
                }
            }
        })
    }
}

impl NeoismMemoryService {
    fn call_sync(&self, cwd: &Path, name: &str, arguments: Value) -> Result<BuiltinMcpCallResult, ServiceError> {
        let request = request(cwd, &arguments);
        let output = match name {
            "memory.init" => json!({"operation":"init","roots":self.init(&request)?.iter().map(location_json).collect::<Vec<_>>() }),
            "memory.list" => json!({"operation":"list","entries":grouped_json(&self.list(&request, limit(&arguments))?) }),
            "memory.search" => { let query = required(&arguments,"query")?; json!({"operation":"search","query":query,"hits":self.search(&request,&query,limit(&arguments))?.iter().map(entry_json).collect::<Vec<_>>()}) },
            "memory.read" => { let path = required(&arguments,"path")?; let value=self.read(&request,&path)?; json!({"operation":"read","scope":value.location.scope_id,"path":value.path,"absolutePath":value.location.storage_key.to_string()+"/"+&value.path,"text":value.content}) },
            "memory.write" => {
                let write = MemoryWriteRequest { request, name: required(&arguments,"name")?, description: required(&arguments,"description")?, kind: optional(&arguments,"type"), body: optional(&arguments,"body").or_else(||optional(&arguments,"content")), file_name: optional(&arguments,"fileName"), created: optional(&arguments,"created"), updated: optional(&arguments,"updated"), origin: optional(&arguments,"origin") };
                let kind = write.kind.as_deref().unwrap_or("project");
                let root = write_root(&write.request, kind)?;
                let target_name = write.file_name.as_deref().map(safe_file_name).filter(|name| !name.is_empty()).unwrap_or_else(||memory_file_name(kind,&write.name));
                if let Some(existing) = find_similar(&root,&root.path.join(target_name),&write.name,&write.description)? {
                    return text_result(json!({"operation":"write","status":"duplicate","scope":root.scope,"existingPath":relative(&root,&existing),"absolutePath":existing,"hint":"a memory covering this already exists; read it and update that file instead of creating a duplicate"}));
                }
                let value = self.write(&write)?;
                json!({"operation":"write","scope":value.location.scope_id,"path":value.path,"absolutePath":value.location.storage_key.to_string()+"/"+&value.path})
            }
            other => return Err(ServiceError::new(format!("unknown memory MCP tool {other}"))),
        };
        text_result(output)
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
        "user" => vec![user_root()],
        "auto" | "all" | "" => { let mut roots=project_roots(cwd,include_missing)?; roots.push(user_root()); roots },
        other => return Err(ServiceError::new(format!("unknown memory scope {other}"))),
    };
    if !include_missing { roots.retain(|root| root.path.is_dir()); }
    Ok(roots)
}

fn project_roots(cwd: &Path, include_missing: bool) -> Result<Vec<Root>, ServiceError> {
    let scoped = project_root(cwd)?;
    if include_missing { return Ok(vec![scoped]); }
    let vault = scoped.workspace.as_vault_workspace();
    let path = vault.notes_workspace_dir().join(PROJECT_DIR);
    if path == scoped.path { return Ok(vec![scoped]); }
    Ok(vec![scoped, Root { scope:"project", label:vault.config.notes.workspace.clone(), path, workspace:vault }])
}

fn project_root(cwd: &Path) -> Result<Root, ServiceError> {
    let workspace = neoism_workspace_index::linked_project_for_code_dir(cwd).map_err(error)?.unwrap_or_else(neoism_workspace_index::default_notes_workspace);
    let path = workspace.notes_workspace_dir().join(PROJECT_DIR);
    let relative = workspace.notes_scope_relative();
    let label = if relative == Path::new(".") { workspace.config.notes.workspace.clone() } else { format!("{}/{}",workspace.config.notes.workspace,relative.display()) };
    Ok(Root { scope:"project",label,path,workspace })
}

fn user_root() -> Root {
    let workspace = neoism_workspace_index::default_notes_workspace();
    Root { scope:"user", label:format!("{}/Memory/Personal",workspace.config.notes.workspace), path:workspace.notes_workspace_dir().join(USER_DIR), workspace }
}

fn write_root(request: &MemoryRequest, kind: &str) -> Result<Root, ServiceError> {
    match request.scope_id.as_deref().unwrap_or("auto") {
        "user" => Ok(user_root()), "project" | "all" => project_root(&request.working_directory),
        "auto" | "" if matches!(kind,"personal"|"preference"|"workflow") => Ok(user_root()),
        "auto" | "" => project_root(&request.working_directory), other => Err(ServiceError::new(format!("unknown memory scope {other}"))),
    }
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

fn safe_path(root:&Root,raw:&str)->Result<PathBuf,ServiceError>{let path=Path::new(raw);if path.is_absolute(){return path.starts_with(&root.path).then(||path.to_path_buf()).ok_or_else(||ServiceError::new("memory path is outside the selected memory root"));}if path.components().any(|c|matches!(c,Component::ParentDir|Component::RootDir|Component::Prefix(_))){return Err(ServiceError::new("memory path must be relative"));}let mut relative=path;for prefix in [USER_DIR,PROJECT_DIR]{if root.path.ends_with(prefix){if let Ok(rest)=path.strip_prefix(prefix){relative=rest;break}}}Ok(root.path.join(relative))}
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
fn request(cwd:&Path,args:&Value)->MemoryRequest{let mut request=MemoryRequest::new(cwd);request.scope_id=args.get("scope").and_then(Value::as_str).map(str::to_string);request}
fn required(args:&Value,key:&str)->Result<String,ServiceError>{args.get(key).and_then(Value::as_str).map(str::to_string).ok_or_else(||ServiceError::new(format!("{key} is required")))}
fn optional(args:&Value,key:&str)->Option<String>{args.get(key).and_then(Value::as_str).map(str::to_string)}
fn limit(args:&Value)->usize{args.get("limit").and_then(Value::as_u64).unwrap_or(40).max(1)as usize}
fn choice(id:&str,label:&str,description:&str)->ScopeChoice{ScopeChoice{id:id.to_string(),label:label.to_string(),description:Some(description.to_string())}}
fn tool(name:&str,description:&str,input_schema:Value)->BuiltinMcpTool{BuiltinMcpTool{name:name.to_string(),description:Some(description.to_string()),input_schema,annotations:None}}
fn text_result(value:Value)->Result<BuiltinMcpCallResult,ServiceError>{Ok(BuiltinMcpCallResult{content:vec![BuiltinMcpContent::Text{text:serde_json::to_string_pretty(&value).map_err(error)?,annotations:None}],is_error:None})}
fn entry_json(value:&MemoryEntry)->Value{let mut object=serde_json::Map::new();object.insert("scope".into(),json!(value.location.scope_id));object.insert("vault".into(),json!(value.location.label));object.insert("path".into(),json!(value.path));object.insert("description".into(),json!(value.description));object.insert("type".into(),json!(value.kind));if let Some(snippet)=&value.snippet{object.insert("snippet".into(),json!(snippet));}if let Some(distance)=value.semantic_distance{object.insert("distance".into(),json!(distance));}Value::Object(object)}
fn grouped_json(entries:&[MemoryEntry])->Vec<Value>{let mut groups:Vec<(MemoryLocation,Vec<Value>)>=Vec::new();for entry in entries{if let Some((_,items))=groups.iter_mut().find(|(location,_)|location.storage_key==entry.location.storage_key){items.push(entry_json(entry));}else{groups.push((entry.location.clone(),vec![entry_json(entry)]));}}groups.into_iter().map(|(location,result)|json!({"scope":location.scope_id,"vault":location.label,"memoryRoot":location.storage_key,"result":result})).collect()}
fn location_json(value:&MemoryLocation)->Value{json!({"scope":value.scope_id,"vault":value.label,"memoryRoot":value.storage_key,"index":format!("{}/{}",value.storage_key,INDEX_FILE)})}
fn error(error:impl std::fmt::Display)->ServiceError{ServiceError::new(error.to_string())}
fn unix_millis()->i64{std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis()as i64).unwrap_or(0)}
fn snippet(value:&str)->String{value.chars().take(240).collect::<String>().replace('\n'," ")}
fn today_utc()->String{let days=unix_millis()/86_400_000;let z=days+719468;let era=if z>=0{z}else{z-146096}/146097;let doe=z-era*146097;let yoe=(doe-doe/1460+doe/36524-doe/146096)/365;let y=yoe+era*400;let doy=doe-(365*yoe+yoe/4-yoe/100);let mp=(5*doy+2)/153;let d=doy-(153*mp+2)/5+1;let m=mp+if mp<10{3}else{-9};let year=y+if m<=2{1}else{0};format!("{year:04}-{m:02}-{d:02}")}

#[cfg(test)]
mod tests {
    use super::*;

    struct NotesHome {
        root: PathBuf,
        previous: Option<std::ffi::OsString>,
    }

    impl NotesHome {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "neoism-memory-adapter-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
            ));
            std::fs::create_dir_all(&root).unwrap();
            let previous = std::env::var_os("NEOISM_NOTES_HOME");
            unsafe { std::env::set_var("NEOISM_NOTES_HOME", &root); }
            Self { root, previous }
        }
    }

    impl Drop for NotesHome {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => unsafe { std::env::set_var("NEOISM_NOTES_HOME", value) },
                None => unsafe { std::env::remove_var("NEOISM_NOTES_HOME") },
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
    fn canonical_default_and_user_layout_is_the_only_runtime_layout() {
        let _lock = crate::test_env_lock().lock().unwrap_or_else(|error| error.into_inner());
        let home = NotesHome::new("canonical");
        let cwd = home.root.join("code");
        std::fs::create_dir_all(&cwd).unwrap();
        let legacy = home.root.join("Default/Personal/Memory");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("personal_legacy.md"), "legacy data must remain untouched").unwrap();

        let service = NeoismMemoryService::new();
        service.init(&MemoryRequest::new(&cwd).with_scope("all")).unwrap();
        service.write(&write_request(&cwd, "project", "feature", "project fact")).unwrap();
        service.write(&write_request(&cwd, "user", "personal", "user fact")).unwrap();

        assert!(home.root.join("Default/Memory/feature_project_fact.md").is_file());
        assert!(home.root.join("Default/Memory/Personal/personal_user_fact.md").is_file());
        assert!(legacy.join("personal_legacy.md").is_file());
        assert!(service.search(&MemoryRequest::new(&cwd).with_scope("user"), "legacy", 10).unwrap().is_empty());
    }

    #[test]
    fn linked_folder_reads_its_memory_and_the_owning_vault_memory() {
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
        assert!(vault.join("Projects/Specific/Memory/feature_specific_fact.md").is_file());
    }
}
