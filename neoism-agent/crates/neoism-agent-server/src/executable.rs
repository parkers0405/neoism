use std::ffi::OsStr;
use std::path::PathBuf;

use neoism_agent_service_api::{AgentServices, ExecutablePurpose, ExecutableRequest};

pub(crate) fn resolve(
    services: &AgentServices,
    program: impl AsRef<OsStr>,
    purpose: ExecutablePurpose,
    description: &str,
) -> anyhow::Result<PathBuf> {
    let program = program.as_ref();
    services
        .executables
        .resolve(&ExecutableRequest::new(program, purpose))
        .map(|result| result.path)
        .map_err(|error| {
            anyhow::anyhow!(
                "{description} executable `{}` is unavailable: {error}; configure the host executable resolver or install it",
                program.to_string_lossy()
            )
        })
}

pub(crate) fn resolve_command(
    services: &AgentServices,
    program: impl AsRef<OsStr>,
    purpose: ExecutablePurpose,
    description: &str,
) -> anyhow::Result<PathBuf> {
    let program = program.as_ref();
    let path = std::path::Path::new(program);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    resolve(services, program, purpose, description)
}

pub(crate) fn in_directory(
    program: impl AsRef<OsStr>,
    directory: &std::path::Path,
) -> std::ffi::OsString {
    let program = std::path::Path::new(program.as_ref());
    if !program.is_absolute() && program.components().count() > 1 {
        directory.join(program).into_os_string()
    } else {
        program.as_os_str().to_owned()
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use neoism_agent_service_api::{
        ExecutableError, ExecutableRequest, ExecutableResult, ExecutableService,
        ExecutableSource,
    };

    #[derive(Clone, Default)]
    pub(crate) struct FakeExecutableService {
        paths: BTreeMap<OsString, PathBuf>,
        pub(crate) requests: Arc<Mutex<Vec<ExecutableRequest>>>,
    }

    impl FakeExecutableService {
        pub(crate) fn with(
            program: impl Into<OsString>,
            path: impl Into<PathBuf>,
        ) -> Self {
            Self {
                paths: BTreeMap::from([(program.into(), path.into())]),
                requests: Arc::default(),
            }
        }
    }

    impl ExecutableService for FakeExecutableService {
        fn resolve(
            &self,
            request: &ExecutableRequest,
        ) -> Result<ExecutableResult, ExecutableError> {
            self.requests.lock().unwrap().push(request.clone());
            self.paths
                .get(&request.program)
                .cloned()
                .map(|path| ExecutableResult {
                    path,
                    source: ExecutableSource::Managed {
                        provider: "test".to_string(),
                    },
                })
                .ok_or_else(|| ExecutableError::NotFound {
                    program: request.program.clone(),
                })
        }
    }
}
