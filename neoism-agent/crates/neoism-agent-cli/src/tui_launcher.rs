use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;

pub(crate) struct TuiOptions {
    pub(crate) server: String,
    pub(crate) session: Option<String>,
    pub(crate) dir: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) variant: Option<String>,
}

pub(crate) async fn run(
    options: TuiOptions,
    services: &neoism_agent_service_api::AgentServices,
) -> anyhow::Result<()> {
    let app_dir = opentui_app_dir()?;
    let entry = app_dir.join("src/index.ts");
    if !entry.exists() {
        anyhow::bail!("Neoism OpenTUI entrypoint is missing: {}", entry.display());
    }
    let bun = resolve_bun(services)?;
    if !app_dir.join("node_modules/@opentui/core").exists() {
        install_dependencies(&bun, &app_dir)?;
    }

    let mut command = Command::new(bun);
    command
        .current_dir(&app_dir)
        .env("BUN_TMPDIR", "/tmp/neoism-bun")
        .arg("run")
        .arg("src/index.ts")
        .arg("--server")
        .arg(options.server);
    push_opt(&mut command, "--session", options.session.as_deref());
    push_opt(&mut command, "--dir", options.dir.as_deref());
    push_opt(&mut command, "--model", options.model.as_deref());
    push_opt(&mut command, "--provider", options.provider.as_deref());
    push_opt(&mut command, "--agent", options.agent.as_deref());
    push_opt(&mut command, "--variant", options.variant.as_deref());

    let status = command.status().with_context(|| {
        format!(
            "failed to start Bun for Neoism OpenTUI app in {}",
            app_dir.display()
        )
    })?;
    if !status.success() {
        anyhow::bail!("Neoism OpenTUI exited with status {status}");
    }
    Ok(())
}

fn resolve_bun(
    services: &neoism_agent_service_api::AgentServices,
) -> anyhow::Result<PathBuf> {
    let bun = std::env::var_os("BUN").unwrap_or_else(|| "bun".into());
    resolve_bun_program(services, bun)
}

fn resolve_bun_program(
    services: &neoism_agent_service_api::AgentServices,
    bun: std::ffi::OsString,
) -> anyhow::Result<PathBuf> {
    services
        .executables
        .resolve(&neoism_agent_service_api::ExecutableRequest::new(
            &bun,
            neoism_agent_service_api::ExecutablePurpose::Other("cli-runtime".to_string()),
        ))
        .map(|result| result.path)
        .map_err(|error| {
            anyhow::anyhow!(
                "Bun executable `{}` is unavailable: {error}; set BUN or install Bun",
                bun.to_string_lossy()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_service_api::{
        ExecutableError, ExecutableRequest, ExecutableResult, ExecutableService,
        ExecutableSource,
    };
    use std::sync::Arc;

    struct Fake(Option<PathBuf>);

    impl ExecutableService for Fake {
        fn resolve(
            &self,
            request: &ExecutableRequest,
        ) -> Result<ExecutableResult, ExecutableError> {
            self.0
                .clone()
                .map(|path| ExecutableResult {
                    path,
                    source: ExecutableSource::Managed {
                        provider: "test".into(),
                    },
                })
                .ok_or_else(|| ExecutableError::NotFound {
                    program: request.program.clone(),
                })
        }
    }

    #[test]
    fn bun_runtime_honors_injected_path_and_reports_missing_bun() {
        let injected = PathBuf::from("/injected/bun");
        let mut services = neoism_agent_server::standard_services();
        services.executables = Arc::new(Fake(Some(injected.clone())));
        assert_eq!(
            resolve_bun_program(&services, "bun".into()).unwrap(),
            injected
        );

        services.executables = Arc::new(Fake(None));
        let error = resolve_bun_program(&services, "bun".into())
            .unwrap_err()
            .to_string();
        assert!(error.contains("Bun executable `bun` is unavailable"));
        assert!(error.contains("install Bun"));
    }
}

fn install_dependencies(bun: &Path, app_dir: &Path) -> anyhow::Result<()> {
    eprintln!(
        "installing Neoism OpenTUI dependencies in {}",
        app_dir.display()
    );
    let status = Command::new(bun)
        .current_dir(app_dir)
        .env("BUN_TMPDIR", "/tmp/neoism-bun")
        .arg("install")
        .status()
        .with_context(|| {
            format!("failed to run `bun install` in {}", app_dir.display())
        })?;
    if !status.success() {
        anyhow::bail!("`bun install` failed with status {status}");
    }
    Ok(())
}

fn push_opt(command: &mut Command, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        command.arg(key).arg(value);
    }
}

fn opentui_app_dir() -> anyhow::Result<PathBuf> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let app_dir = manifest_dir.join("../../apps/neoism-opentui");
    app_dir
        .canonicalize()
        .with_context(|| format!("failed to locate {}", app_dir.display()))
}
