use super::*;

const CHILD_ROOT: &str = "NEOISM_TEST_WORKSPACE_REGISTRY_ROOT";
const CHILD_TEST: &str = "workspace::tests::registry_isolation::ephemeral_registry_child";

fn project(root: &Path, id: &str) -> ProjectRootSummary {
    ProjectRootSummary {
        id: id.into(),
        name: id.into(),
        path: root.join(id),
        last_opened: 0,
    }
}

fn run_child(seed_private_registry: bool) {
    let td = TempDir::new().unwrap();
    let root = td.path();
    // Stand-in for the host's registry. Never seed or mutate real user data.
    let user_data = root.join("user-data");
    let host_registry = user_data.join("neoism/workspaces.json");
    save_registry(&host_registry, &PersistedRegistry {
        workspaces: vec![project(root, "unrelated-host-project")],
        preferences: HashMap::from([("host-preferences".into(), sample_prefs())]),
    }).unwrap();
    let original_host = std::fs::read(&host_registry).unwrap();
    let state_dir = root.join("fast/state");
    let private_registry = state_dir.join("workspaces.json");
    if seed_private_registry {
        save_registry(&private_registry, &PersistedRegistry {
            workspaces: vec![project(root, "existing-private-project")],
            preferences: HashMap::from([("existing-private-project".into(), sample_prefs())]),
        }).unwrap();
    }
    // A fresh snapshot must still be ignored in generic ephemeral mode.
    persistence::save_snapshot(&state_dir, Snapshot {
        version: 1,
        workspaces: vec![project(root, "unrelated-snapshot-project")],
        ..Snapshot::default()
    }).unwrap();
    let original_snapshot = std::fs::read(state_dir.join("state.json")).unwrap();

    // Per-command environment avoids racing other tests or modifying the
    // launcher's environment. Even an inherited host override must not leak
    // unrelated workspaces into an ephemeral manager.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env(CHILD_ROOT, root)
        .env("NEOISM_WORKSPACE_REGISTRY", &host_registry)
        .env("XDG_DATA_HOME", &user_data)
        .env("APPDATA", &user_data)
        .env("NEOISM_NOTES_HOME", root.join("notes"))
        .output().unwrap();
    assert!(output.status.success(), "child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert_eq!(std::fs::read(&host_registry).unwrap(), original_host);
    assert_eq!(std::fs::read(state_dir.join("state.json")).unwrap(), original_snapshot);
    let persisted = load_registry_full(&private_registry);
    assert!(persisted.workspaces.is_empty(), "child completed create/rename/delete round trip");
    assert_eq!(persisted.preferences["fast-project"], sample_prefs());
}

#[test]
fn ephemeral_registry_isolates_bootstrap_and_real_mutations_without_env_override() {
    run_child(false);
}

#[test]
fn ephemeral_registry_loads_only_state_local_registry_and_keeps_mutations_local() {
    run_child(true);
}

#[test]
fn ephemeral_registry_child() {
    let Some(root) = std::env::var_os(CHILD_ROOT).map(PathBuf::from) else { return; };
    let state_dir = root.join("fast/state");
    let private_registry = state_dir.join("workspaces.json");
    // Check the deliberately inherited host override before bootstrap. Normal
    // bootstrap must retain its legacy selected-registry behavior.
    let host_registry = root.join("user-data/neoism/workspaces.json");
    assert_eq!(persistence_path(), host_registry);
    let normal = WorkspaceManager::bootstrap();
    assert_eq!(normal.persistence_path, host_registry);
    assert_eq!(normal.list_workspaces()[0].id, "unrelated-host-project");
    let seeded = private_registry.exists();
    let manager = WorkspaceManager::bootstrap_with_state_dir(state_dir, true);
    assert_eq!(manager.persistence_path, private_registry);
    assert!(!manager.snapshot_writer().is_persistent());
    let workspaces = manager.list_workspaces();
    assert_eq!(workspaces.len(), usize::from(seeded));
    assert!(manager.inner.lock().preferences.get("host-preferences").is_none());
    if seeded {
        assert_eq!(workspaces[0].id, "existing-private-project");
        assert_eq!(manager.get_preferences("existing-private-project"), sample_prefs());
        assert!(manager.forget_workspace("existing-private-project"));
    }

    let clone = manager.clone();
    clone.upsert_workspace(project(&root, "fast-project"));
    let persisted = load_registry_full(&private_registry);
    assert_eq!(persisted.workspaces.len(), 1);
    assert_eq!(persisted.workspaces[0].id, "fast-project");
    clone.set_preferences("fast-project".into(), sample_prefs());
    assert_eq!(load_registry_full(&private_registry).preferences["fast-project"], sample_prefs());
    assert!(clone.rename_workspace("fast-project", "renamed".into()));
    assert_eq!(load_registry_full(&private_registry).workspaces[0].name, "renamed");
    assert!(clone.forget_workspace("fast-project"));
    assert!(load_registry_full(&private_registry).workspaces.is_empty());
}