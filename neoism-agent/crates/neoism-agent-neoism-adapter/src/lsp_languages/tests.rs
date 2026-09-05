use super::*;

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
fn representative_catalog_contracts_match_executable_argv_and_routes() {
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
        assert!(adapter.catalog_packages.iter().any(|package| {
            package.package_id == package_id && package.executable == executable
        }));
        let command = stdio_command(adapter).expect("stdio adapter");
        assert!(command[0].eq_ignore_ascii_case(executable));
        assert_eq!(&command[1..], args);
    }
}

#[test]
fn omnisharp_catalog_binary_is_started_in_language_server_mode() {
    let adapter = adapter_by_id("csharp").unwrap();
    assert!(adapter.catalog_packages.iter().any(|package| {
        package.package_id == "omnisharp" && package.executable == "OmniSharp"
    }));
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
