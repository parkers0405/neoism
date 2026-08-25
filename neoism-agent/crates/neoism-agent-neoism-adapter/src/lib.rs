use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use neoism_agent_service_api::{
    AgentServices, BuiltinMcpService, ConfigSourceService, DocumentationService, ExecutableError,
    ExecutableRequest, ExecutableResult, ExecutableService, ExecutableSource, MemoryService, NotesService,
    StandardExecutableService,
};

mod docs;
mod config;
mod memory;
mod notes;

#[cfg(test)]
fn test_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    &LOCK
}

pub fn neoism_services() -> AgentServices {
    let notes = Arc::new(notes::NeoismNotesService);
    let documentation = Arc::new(docs::NeoismDocumentationService);
    let memory = Arc::new(memory::NeoismMemoryService::new());
    AgentServices::new(
        Arc::new(NeoismExecutableService::new()),
        neoism_agent_server::standard_workspace_search(),
    )
        .with_config(Arc::new(config::NeoismConfigSourceService::new()) as Arc<dyn ConfigSourceService>)
        .with_notes(notes.clone() as Arc<dyn NotesService>)
        .with_documentation(documentation.clone() as Arc<dyn DocumentationService>)
        .with_memory(memory.clone() as Arc<dyn MemoryService>)
        .with_builtin_mcp(notes as Arc<dyn BuiltinMcpService>)
        .with_builtin_mcp(documentation as Arc<dyn BuiltinMcpService>)
        .with_builtin_mcp(memory as Arc<dyn BuiltinMcpService>)
}

pub struct NeoismExecutableService {
    managed: BTreeMap<String, String>,
    standard: StandardExecutableService,
}

impl NeoismExecutableService {
    pub fn new() -> Self {
        // TODO: neoism-extensions currently reconciles legacy installs while
        // building this map. Keep that product side effect isolated here until
        // it exposes a side-effect-free installed-binary snapshot API.
        let managed = neoism_extensions::managed_bin::managed_bin_map().unwrap_or_default();
        Self {
            managed,
            standard: StandardExecutableService,
        }
    }

    #[cfg(test)]
    fn from_managed(managed: BTreeMap<String, String>) -> Self {
        Self {
            managed,
            standard: StandardExecutableService,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_service_api::{ExecutablePurpose, ExecutableRequest, ExecutableSource};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn managed_id_resolves_before_path_without_shadowing_missing_files() {
        let root = std::env::temp_dir().join(format!(
            "neoism-agent-adapter-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("pyright-langserver");
        fs::write(&executable, b"test").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let service = NeoismExecutableService::from_managed(BTreeMap::from([(
            "python".to_string(),
            executable.display().to_string(),
        )]));
        let result = service
            .resolve(
                &ExecutableRequest::new(
                    "pyright-langserver",
                    ExecutablePurpose::LanguageServer,
                )
                .with_preferred_id("python"),
            )
            .unwrap();
        assert_eq!(result.path, executable);
        assert_eq!(
            result.source,
            ExecutableSource::Managed {
                provider: "neoism-extensions".to_string()
            }
        );
    }
}

impl Default for NeoismExecutableService {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecutableService for NeoismExecutableService {
    fn resolve(&self, request: &ExecutableRequest) -> Result<ExecutableResult, ExecutableError> {
        let program_key = Path::new(&request.program)
            .file_name()
            .and_then(|name| name.to_str());
        let managed = request
            .preferred_ids
            .iter()
            .filter_map(|id| self.managed.get(id))
            .chain(program_key.into_iter().filter_map(|name| self.managed.get(name)))
            .next();
        if let Some(path) = managed {
            let managed_request = ExecutableRequest::new(path, request.purpose.clone());
            if let Ok(mut result) = self.standard.resolve(&managed_request) {
                result.source = ExecutableSource::Managed {
                    provider: "neoism-extensions".to_string(),
                };
                return Ok(result);
            }
        }
        self.standard.resolve(request)
    }
}