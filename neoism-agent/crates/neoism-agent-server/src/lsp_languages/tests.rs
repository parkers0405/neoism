use super::*;
use neoism_extensions::{
    load_mason_registry,
    mason::{mason_cache_path, parse_mason_registry},
    package_to_manifest,
};
use std::path::Path;

const REPRESENTATIVE_MASON_LSP_ROWS: &str = r#"[
    {
        "name": "typescript-language-server",
        "source": { "id": "pkg:npm/typescript-language-server@5.3.0" },
        "bin": { "typescript-language-server": "npm:typescript-language-server" }
    },
    {
        "name": "pyright",
        "source": { "id": "pkg:npm/pyright@1.1.411" },
        "bin": {
            "pyright": "npm:pyright",
            "pyright-langserver": "npm:pyright-langserver"
        }
    },
    {
        "name": "bash-language-server",
        "source": { "id": "pkg:npm/bash-language-server@5.6.0" },
        "bin": { "bash-language-server": "npm:bash-language-server" }
    },
    {
        "name": "json-lsp",
        "source": { "id": "pkg:npm/vscode-langservers-extracted@4.10.0" },
        "bin": { "vscode-json-language-server": "npm:vscode-json-language-server" }
    },
    {
        "name": "docker-language-server",
        "source": {
            "id": "pkg:github/docker/docker-language-server@v0.20.1",
            "asset": [{
                "target": "linux_x64_gnu",
                "file": "docker-language-server-linux-amd64-{{version}}"
            }]
        },
        "bin": { "docker-language-server": "{{source.asset.file}}" }
    },
    {
        "name": "nil",
        "source": {
            "id": "pkg:cargo/nil@2025-06-13?repository_url=https://github.com/oxalica/nil"
        },
        "bin": { "nil": "cargo:nil" }
    }
]"#;

#[test]
fn filename_globs_are_anchored_and_case_insensitive() {
    assert!(wildcard_filename_matches("Dockerfile.*", "Dockerfile.dev"));
    assert!(wildcard_filename_matches("*.Dockerfile", "ci.Dockerfile"));
    assert!(wildcard_filename_matches(
        "compose.*.yaml",
        "compose.dev.yaml"
    ));
    assert!(!wildcard_filename_matches(
        "Dockerfile.*",
        "xDockerfile.dev"
    ));
}

#[test]
fn every_stdio_adapter_has_an_explicit_catalog_contract_or_opt_out() {
    let stdio = LANGUAGE_SPECS
        .iter()
        .filter(|spec| stdio_command(spec).is_some())
        .collect::<Vec<_>>();
    assert_eq!(stdio.len(), 25, "update this audit when adapters change");

    let without_catalog = stdio
        .iter()
        .filter(|spec| spec.catalog_packages.is_empty())
        .map(|spec| spec.id)
        .collect::<Vec<_>>();
    assert_eq!(
        without_catalog,
        vec!["scala"],
        "Mason currently has no Metals package; every other stdio adapter must declare its exact package"
    );
    assert_eq!(
        stdio
            .iter()
            .map(|spec| spec.catalog_packages.len())
            .sum::<usize>(),
        24
    );
}

#[test]
fn workspace_root_policy_is_declared_by_each_builtin() {
    let cargo_metadata = LANGUAGE_SPECS
        .iter()
        .filter_map(|adapter| match adapter.root_strategy {
            WorkspaceRootStrategySpec::CargoMetadata { manifest } => {
                Some((adapter.id, manifest))
            }
            WorkspaceRootStrategySpec::NearestMarker => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(cargo_metadata, vec![("rust", "Cargo.toml")]);
    assert_eq!(
        LANGUAGE_SPECS
            .iter()
            .filter(|adapter| {
                adapter.root_strategy == WorkspaceRootStrategySpec::NearestMarker
            })
            .count(),
        LANGUAGE_SPECS.len() - 1
    );
}

#[test]
fn representative_catalog_rows_match_package_executable_argv_and_routes() {
    let registry = parse_mason_registry(REPRESENTATIVE_MASON_LSP_ROWS.as_bytes())
        .expect("representative Mason registry");
    let expected = [
        (
            "typescript",
            "typescript-language-server",
            "typescript-language-server",
            &["--stdio"][..],
        ),
        ("python", "pyright", "pyright-langserver", &["--stdio"][..]),
        (
            "bash",
            "bash-language-server",
            "bash-language-server",
            &["start"][..],
        ),
        (
            "json",
            "json-lsp",
            "vscode-json-language-server",
            &["--stdio"][..],
        ),
        (
            "docker",
            "docker-language-server",
            "docker-language-server",
            &["start", "--stdio"][..],
        ),
        ("nix", "nil", "nil", &[][..]),
    ];

    for (adapter_id, package_id, executable, args) in expected {
        let adapter = adapter_by_id(adapter_id).expect("declared adapter");
        assert!(adapter.supports_catalog_package(package_id, executable));
        let command = stdio_command(adapter).expect("stdio adapter");
        assert!(command[0].eq_ignore_ascii_case(executable));
        assert_eq!(&command[1..], args);

        let package = registry
            .iter()
            .find(|package| package.name == package_id)
            .expect("representative package");
        assert!(package
            .bin
            .keys()
            .any(|bin| bin.eq_ignore_ascii_case(executable)));
        let manifest = package_to_manifest(package).expect("translatable package");
        assert!(manifest
            .run
            .as_ref()
            .and_then(|run| run.command.first())
            .is_some_and(|command| command.eq_ignore_ascii_case(executable)));
    }

    let route_cases = [
        ("src/app.tsx", "typescript", "typescriptreact"),
        ("src/app.mjs", "typescript", "javascript"),
        ("src/main.py", "python", "python"),
        ("tools/check.ksh", "bash", "shellscript"),
        ("tools/check.csh", "bash", "shellscript"),
        ("settings.jsonc", "json", "jsonc"),
        ("Dockerfile.dev", "docker", "dockerfile"),
        ("compose.test.yaml", "docker", "dockercompose"),
        ("docker-bake.hcl", "docker", "dockerbake"),
        ("flake.nix", "nix", "nix"),
        ("go.mod", "go", "gomod"),
        ("go.work", "go", "gowork"),
        ("templates/page.heex", "elixir", "heex"),
        ("generated/value.zir", "zig", "zir"),
    ];
    for (path, adapter_id, language_id) in route_cases {
        let adapter = best_adapter_for_path(Path::new(path)).expect("routed path");
        assert_eq!(adapter.id, adapter_id, "wrong adapter for {path}");
        assert_eq!(
            adapter.language_id_for_path(Path::new(path)),
            Some(language_id),
            "wrong languageId for {path}"
        );
    }
}

#[test]
fn omnisharp_catalog_binary_is_started_in_language_server_mode() {
    let adapter = adapter_by_id("csharp").unwrap();
    assert!(adapter.supports_catalog_package("omnisharp", "OmniSharp"));
    let command = stdio_command(adapter).unwrap();
    assert_eq!(command.first().copied(), Some("omnisharp"));
    assert!(command.contains(&"--languageserver"));
    assert!(command
        .windows(2)
        .any(|pair| pair == ["--encoding", "utf-8"]));
}

fn stdio_command(spec: &LanguageSpec) -> Option<&'static [&'static str]> {
    match spec.transport {
        LspTransportSpec::Stdio { command } => Some(command),
        LspTransportSpec::Tcp { .. } => None,
    }
}

/// Developer audit against the current cached Mason snapshot. CI does not
/// need a network/cache; the representative regression above is stable.
#[test]
fn cached_mason_rows_satisfy_every_declared_catalog_contract_when_present() {
    let path = mason_cache_path();
    if !path.is_file() {
        return;
    }
    let registry = load_mason_registry(&path).expect("cached Mason registry");
    for adapter in LANGUAGE_SPECS {
        for contract in adapter.catalog_packages {
            let package = registry
                .iter()
                .find(|package| package.name == contract.package_id)
                .unwrap_or_else(|| {
                    panic!(
                        "adapter `{}` declares missing Mason package `{}`",
                        adapter.id, contract.package_id
                    )
                });
            assert!(
                package
                    .bin
                    .keys()
                    .any(|bin| { bin.eq_ignore_ascii_case(contract.executable) }),
                "Mason package `{}` no longer exposes `{}` for adapter `{}`; bins={:?}",
                contract.package_id,
                contract.executable,
                adapter.id,
                package.bin.keys().collect::<Vec<_>>()
            );

            // Unsupported source kinds are intentionally left visible but
            // non-installable. Any row we *can* translate must select the
            // exact executable consumed by the adapter.
            if let Ok(manifest) = package_to_manifest(package) {
                let command = manifest
                    .run
                    .as_ref()
                    .and_then(|run| run.command.first())
                    .expect("translated LSP run command");
                assert!(
                    command.eq_ignore_ascii_case(contract.executable),
                    "translated package `{}` selected `{command}`, expected `{}`",
                    contract.package_id,
                    contract.executable
                );
            }
        }
    }
}
