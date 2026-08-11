//! Public lookup-table tests for portable IDE and agent installation metadata.

use std::collections::BTreeSet;

use neoism_protocol::ide_tools::{
    agent_install_spec, treesitter_install_spec, AgentInstallMethod,
};

#[test]
fn every_supported_treesitter_language_has_complete_metadata() {
    let expected = [
        "rust",
        "python",
        "javascript",
        "typescript",
        "tsx",
        "go",
        "lua",
        "json",
        "toml",
        "yaml",
        "markdown",
        "nix",
    ];
    let mut repositories = BTreeSet::new();

    for language in expected {
        let spec = treesitter_install_spec(language)
            .unwrap_or_else(|| panic!("missing Tree-sitter metadata for {language}"));
        assert_eq!(spec.lang, language);
        assert!(!spec.display_name.is_empty());
        assert!(spec.repo.starts_with("https://github.com/"));
        assert!(!spec.subdir.is_empty());
        repositories.insert(spec.repo);
    }

    assert!(
        repositories.len() < expected.len(),
        "TypeScript and TSX should intentionally share a repository"
    );
}

#[test]
fn agent_installers_expose_the_expected_execution_strategy() {
    let claude = agent_install_spec("claude").expect("Claude install metadata");
    assert_eq!(claude.binary, "claude");
    assert_eq!(claude.manager, "npm");
    assert_eq!(
        claude.method,
        AgentInstallMethod::NpmGlobal {
            package: "@anthropic-ai/claude-code",
        }
    );

    let codex = agent_install_spec("codex").expect("Codex install metadata");
    assert_eq!(codex.binary, "codex");
    assert_eq!(
        codex.method,
        AgentInstallMethod::NpmGlobal {
            package: "@openai/codex",
        }
    );

    let opencode = agent_install_spec("opencode").expect("OpenCode install metadata");
    assert_eq!(opencode.binary, "opencode");
    assert_eq!(
        opencode.method,
        AgentInstallMethod::ShellPipe {
            url: "https://opencode.ai/install",
        }
    );
}

#[test]
fn lookup_ids_are_exact_and_unknown_values_are_rejected() {
    for unknown in ["", "Rust", "CLAUDE", "typescriptreact", "../rust"] {
        assert!(treesitter_install_spec(unknown).is_none());
        assert!(agent_install_spec(unknown).is_none());
    }
}
