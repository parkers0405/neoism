use super::*;

struct MockShell {
    config: Value,
    calls: Vec<Vec<String>>,
    scanned: bool,
    scan_delay: usize,
    unavailable: bool,
    reject: Option<&'static str>,
    corrupt_hidden: bool,
    stale_stored: bool,
}
impl Default for MockShell {
    fn default() -> Self {
        Self {
            config: json!({"version":1,"unrelated":{"preserve":true},"bar":{"position":"bottom","layout":{
                "left":[],"center":[],"right":[{"id":"omarchy.tray","hidden":["dropbox"],"pinned":["keep"],"custom":42}]
            }}}),
            calls: vec![],
            scanned: false,
            scan_delay: 0,
            unavailable: false,
            reject: None,
            corrupt_hidden: false,
            stale_stored: false,
        }
    }
}
impl Shell for MockShell {
    fn call(&mut self, args: &[&str]) -> Result<String> {
        self.calls
            .push(args.iter().map(|s| s.to_string()).collect());
        if self.unavailable {
            return Err(fail("offline"));
        }
        if self.reject == Some(args[0]) {
            return Ok("not ready".into());
        }
        match args[0] {
            "ping" => Ok("ok".into()),
            "listPlugins" => {
                if self.scanned && self.scan_delay == 0 {
                    Ok(json!([{"id":ID}]).to_string())
                } else {
                    self.scan_delay = self.scan_delay.saturating_sub(1);
                    Ok("[]".into())
                }
            }
            "rescanPlugins" => {
                self.scanned = true;
                Ok(String::new())
            }
            "listShellConfig" => Ok(self.config.to_string()),
            "putBarWidget" => {
                let layout = self.config["bar"]["layout"].as_object_mut().unwrap();
                if !layout
                    .values()
                    .any(|a| a.as_array().unwrap().iter().any(|e| e["id"] == ID))
                {
                    layout
                        .get_mut("right")
                        .unwrap()
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"id":ID}));
                }
                Ok("ok".into())
            }
            "setBarWidget" => {
                assert_eq!(&args[1..3], &["omarchy.tray", "hidden"]);
                let selector: Value = serde_json::from_str(args[4])?;
                let entry = &mut self.config["bar"]["layout"]
                    [selector["section"].as_str().unwrap()]
                    [selector["index"].as_u64().unwrap() as usize];
                if entry.is_string() {
                    *entry = json!({"id":"omarchy.tray"});
                }
                // Model qs/CLI11 bracket expansion, not nonexistent QML string
                // coercion. Whitespace prevents the argv-list special case.
                let mut value: Value = serde_json::from_str(args[3])?;
                if args[3].starts_with('[') && args[3].ends_with(']') {
                    let names = value.as_array().unwrap();
                    if names.len() != 1 {
                        return Err(fail("too many IPC arguments"));
                    }
                    value = names[0].clone();
                }
                entry["hidden"] = if self.corrupt_hidden {
                    json!("neoism")
                } else {
                    value
                };
                Ok("ok".into())
            }
            _ => panic!("unexpected IPC: {args:?}"),
        }
    }
    fn stored_config(&mut self) -> Result<Option<Value>> {
        let mut stored = self.config.clone();
        if self.stale_stored {
            stored["bar"]["layout"]["right"][0]["hidden"] = json!("neoism");
        }
        Ok(Some(stored))
    }
    fn wait(&mut self) {}
}
fn plugin(home: &Path) -> PathBuf {
    home.join(".config/omarchy/plugins").join(ID)
}
fn activated(home: &Path) -> PathBuf {
    home.join(".config/neoism/bootstrap/omarchy/activated-v1.json")
}

#[test]
fn placement_failure_never_hides_generic_item() {
    let home = tempfile::tempdir().unwrap();
    let mut shell = MockShell {
        reject: Some("putBarWidget"),
        ..Default::default()
    };
    let before = shell.config.clone();
    assert!(install(home.path(), &mut shell).is_err());
    assert_eq!(shell.config, before);
    assert!(!shell.calls.iter().any(|args| args[0] == "setBarWidget"));
    assert!(!activated(home.path()).exists());
}

#[test]
fn invalid_hidden_setting_is_not_clobbered_and_string_entry_is_supported() {
    let home = tempfile::tempdir().unwrap();
    let mut shell = MockShell::default();
    shell.config["bar"]["layout"]["right"][0]["hidden"] = json!({"invalid":true});
    assert!(install(home.path(), &mut shell).is_err());
    assert_eq!(
        shell.config["bar"]["layout"]["right"][0]["hidden"],
        json!({"invalid":true})
    );
    assert!(!activated(home.path()).exists());
    shell.config["bar"]["layout"]["right"][0] = json!("omarchy.tray");
    install(home.path(), &mut shell).unwrap();
    assert_eq!(
        shell.config["bar"]["layout"]["right"][0],
        json!({"id":"omarchy.tray", "hidden":["neoism"]})
    );
}

#[test]
fn csv_hidden_normalizes_to_arrays_preserving_names_pinned_and_ipc_format() {
    for (hidden, merged) in [
        (json!("dropbox,foo"), json!(["dropbox", "foo", "neoism"])),
        (
            json!(" dropbox ,foo*,prefix:bar,,"),
            json!(["dropbox", "foo*", "prefix:bar", "neoism"]),
        ),
        (json!("dropbox"), json!(["dropbox", "neoism"])),
        (json!(""), json!(["neoism"])),
        (
            json!(["dropbox", "foo"]),
            json!(["dropbox", "foo", "neoism"]),
        ),
        (Value::Null, json!(["neoism"])),
    ] {
        let home = tempfile::tempdir().unwrap();
        let mut shell = MockShell::default();
        shell.config["bar"]["layout"]["right"][0]["hidden"] = hidden.clone();
        shell.config["bar"]["layout"]["right"][0]["pinned"] = json!("keep,other");
        let mut expected_tray = shell.config["bar"]["layout"]["right"][0].clone();
        expected_tray["hidden"] = json!(merged);
        install(home.path(), &mut shell).unwrap();
        assert_eq!(shell.config["bar"]["layout"]["right"][0], expected_tray);
        let call = shell
            .calls
            .iter()
            .find(|args| args[0] == "setBarWidget")
            .unwrap();
        let wire: Value = serde_json::from_str(&call[3]).unwrap();
        assert_eq!(wire, merged);
        assert!(
            call[3].starts_with(" ["),
            "JSON must bypass qs bracket-list expansion"
        );
        assert!(activated(home.path()).exists());
        // The production activation marker skips settings entirely, including
        // later manual removal of Neoism from a preexisting custom CSV list.
        shell.config["bar"]["layout"]["right"][0]["hidden"] = json!("dropbox,foo");
        let before = shell.config.clone();
        shell.calls.clear();
        install(home.path(), &mut shell).unwrap();
        assert_eq!(shell.config, before);
        assert!(shell
            .calls
            .iter()
            .all(|args| ["ping", "listPlugins"].contains(&args[0].as_str())));
    }
}

#[test]
fn existing_neoism_csv_is_normalized_without_duplicating_name() {
    for csv in ["neoism", "dropbox,neoism,foo", "dropbox, neoism ,foo"] {
        let home = tempfile::tempdir().unwrap();
        let mut shell = MockShell::default();
        shell.config["bar"]["layout"]["right"][0]["hidden"] = json!(csv);
        install(home.path(), &mut shell).unwrap();
        let expected: Vec<_> = csv.split(',').map(str::trim).collect();
        assert_eq!(
            shell.config["bar"]["layout"]["right"][0]["hidden"],
            json!(expected)
        );
        assert_eq!(
            shell
                .calls
                .iter()
                .filter(|args| args[0] == "setBarWidget")
                .count(),
            1
        );
        assert!(activated(home.path()).exists());
    }
}

#[test]
fn qs_bracket_expansion_regression_singleton_and_multiple_names() {
    let mut shell = MockShell::default();
    shell
        .call(&[
            "setBarWidget",
            "omarchy.tray",
            "hidden",
            r#"["neoism"]"#,
            r#"{"section":"right","index":0}"#,
        ])
        .unwrap();
    assert_eq!(
        shell.config["bar"]["layout"]["right"][0]["hidden"],
        "neoism"
    );
    assert!(shell
        .call(&[
            "setBarWidget",
            "omarchy.tray",
            "hidden",
            r#"["dropbox","neoism"]"#,
            r#"{"section":"right","index":0}"#
        ])
        .is_err());
    set_hidden(&mut shell, "right", 0, &json!(["dropbox", "neoism"])).unwrap();
    assert_eq!(
        shell.config["bar"]["layout"]["right"][0]["hidden"],
        json!(["dropbox", "neoism"])
    );
}

#[test]
fn successful_ipc_reply_requires_effective_and_stored_arrays_before_activation() {
    for disk_only in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let mut shell = MockShell {
            corrupt_hidden: !disk_only,
            stale_stored: disk_only,
            ..Default::default()
        };
        assert!(install(home.path(), &mut shell)
            .unwrap_err()
            .to_string()
            .contains("round-trip"));
        assert!(!activated(home.path()).exists());
        assert!(!home
            .path()
            .join(".config/neoism/bootstrap/omarchy/hidden-array-v1.json")
            .exists());
    }
}

#[test]
fn migration_repairs_only_our_legacy_singleton_with_plugin_still_placed_once() {
    for (hidden, placed, repair) in [
        (json!("neoism"), true, true),
        (json!("neoism"), false, false),
        (json!("dropbox,foo"), true, false),
        (json!([]), true, false),
        (json!(["neoism"]), true, false),
    ] {
        let home = tempfile::tempdir().unwrap();
        let mut shell = MockShell::default();
        install(home.path(), &mut shell).unwrap();
        let activation_before = fs::read(activated(home.path())).unwrap();
        let fix = home
            .path()
            .join(".config/neoism/bootstrap/omarchy/hidden-array-v1.json");
        fs::remove_file(&fix).unwrap(); // Simulate an old completed activation.
        shell.config["bar"]["layout"]["right"][0]["hidden"] = hidden;
        if !placed {
            shell.config["bar"]["layout"]["right"]
                .as_array_mut()
                .unwrap()
                .pop();
        }
        let mut expected = shell.config.clone();
        if repair {
            expected["bar"]["layout"]["right"][0]["hidden"] = json!(["neoism"]);
        }
        shell.calls.clear();
        install(home.path(), &mut shell).unwrap();
        assert_eq!(shell.config, expected);
        assert_eq!(fs::read(activated(home.path())).unwrap(), activation_before);
        assert_eq!(
            shell
                .calls
                .iter()
                .filter(|a| a[0] == "setBarWidget")
                .count(),
            usize::from(repair)
        );
        assert!(!shell
            .calls
            .iter()
            .any(|a| ["putBarWidget", "rescanPlugins"].contains(&a[0].as_str())));
        assert!(fix.exists());
        shell.config["bar"]["layout"]["right"][0]["hidden"] = json!("neoism");
        shell.calls.clear();
        install(home.path(), &mut shell).unwrap();
        assert!(!shell.calls.iter().any(|a| a[0] == "setBarWidget"));
    }
}

/// Explicit selected-key host round-trip, not part of ordinary cargo test.
/// Requires an already-owned, activated installation and only our singleton
/// array. Writes the SAME hidden value, then checks disk and effective JSON.
#[test]
#[ignore = "writes the existing Neoism-only tray hidden key; requires reviewed host approval"]
fn host_verify_array_transport() {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    verify_owned(&plugin(&home)).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(activated(&home)).unwrap()).unwrap(),
        json!({"activation":1})
    );
    let root = PathBuf::from(
        std::env::var_os("OMARCHY_PATH").unwrap_or_else(|| "/usr/share/omarchy".into()),
    );
    let mut shell = LiveShell {
        root,
        config_path: home.join(".config/omarchy/shell.json"),
    };
    let before: Value =
        serde_json::from_str(&shell.call(&["listShellConfig"]).unwrap()).unwrap();
    let mut count = 0;
    for (section, entries) in before["bar"]["layout"].as_object().unwrap() {
        for (index, entry) in entries.as_array().unwrap().iter().enumerate() {
            if entry["id"] == "omarchy.tray" {
                assert_eq!(
                    entry["hidden"],
                    json!(["neoism"]),
                    "refusing to touch non-Neoism-only hidden state"
                );
                set_hidden(&mut shell, section, index, &entry["hidden"]).unwrap();
                count += 1;
            }
        }
    }
    assert!(count > 0);
    let effective: Value =
        serde_json::from_str(&shell.call(&["listShellConfig"]).unwrap()).unwrap();
    assert_eq!(effective, before);
    assert_eq!(shell.stored_config().unwrap().unwrap(), before);
}

#[test]
fn installs_and_preserves_config_placement_and_idempotence() {
    let home = tempfile::tempdir().unwrap();
    let mut shell = MockShell::default();
    shell.scan_delay = 3;
    shell.config["bar"]["layout"]["left"] = json!([{"id":ID,"custom":99}]);
    let before = shell.config.clone();
    install(home.path(), &mut shell).unwrap();
    let mut expected = before;
    expected["bar"]["layout"]["right"][0]["hidden"] = json!(["dropbox", "neoism"]);
    assert_eq!(shell.config, expected);
    assert!(activated(home.path()).exists());
    assert_eq!(verify_owned(&plugin(home.path())).unwrap(), marker());
    shell.calls.clear();
    install(home.path(), &mut shell).unwrap();
    assert!(shell
        .calls
        .iter()
        .all(|a| ["ping", "listPlugins"].contains(&a[0].as_str())));
}

#[test]
fn respect_manual_layout_removal_and_tray_unhide() {
    let home = tempfile::tempdir().unwrap();
    let mut shell = MockShell::default();
    install(home.path(), &mut shell).unwrap();
    shell.config = MockShell::default().config;
    let before = shell.config.clone();
    install(home.path(), &mut shell).unwrap();
    assert_eq!(shell.config, before);
    fs::remove_dir_all(plugin(home.path())).unwrap();
    shell.calls.clear();
    install(home.path(), &mut shell).unwrap();
    assert!(!plugin(home.path()).exists());
    assert!(shell.calls.is_empty());
}

#[test]
fn refuses_unowned_custom_git_modified_and_symlinks() {
    for variant in [
        "unowned",
        "extra",
        "git",
        "edited",
        "symlink",
        "parent-symlink",
    ] {
        let home = tempfile::tempdir().unwrap();
        let dir = plugin(home.path());
        install_assets(&dir).unwrap();
        match variant {
            "unowned" => fs::remove_file(dir.join(OWNER)).unwrap(),
            "extra" => fs::write(dir.join("custom.qml"), "custom").unwrap(),
            "git" => fs::create_dir(dir.join(".git")).unwrap(),
            "edited" => fs::write(dir.join("BarWidget.qml"), "user edit").unwrap(),
            "symlink" => {
                fs::remove_file(dir.join("BarWidget.qml")).unwrap();
                std::os::unix::fs::symlink("manifest.json", dir.join("BarWidget.qml"))
                    .unwrap();
            }
            "parent-symlink" => {
                let parent = dir.parent().unwrap();
                let moved = home.path().join("other");
                fs::rename(parent, &moved).unwrap();
                std::os::unix::fs::symlink(moved, parent).unwrap();
            }
            _ => unreachable!(),
        }
        let mut shell = MockShell::default();
        assert!(install(home.path(), &mut shell).is_err(), "{variant}");
        assert!(!activated(home.path()).exists());
        assert!(!shell.calls.iter().any(|a| a[0] == "putBarWidget"));
        if variant == "edited" {
            assert_eq!(
                fs::read_to_string(dir.join("BarWidget.qml")).unwrap(),
                "user edit"
            );
        }
    }
}

#[test]
fn offline_or_partial_activation_retries_without_success_marker() {
    let home = tempfile::tempdir().unwrap();
    let mut shell = MockShell {
        unavailable: true,
        ..Default::default()
    };
    assert!(install(home.path(), &mut shell).is_err());
    assert!(!plugin(home.path()).exists());
    shell.unavailable = false;
    shell.reject = Some("setBarWidget");
    assert!(install(home.path(), &mut shell).is_err());
    assert!(!activated(home.path()).exists());
    shell.reject = None;
    install(home.path(), &mut shell).unwrap();
    assert!(activated(home.path()).exists());
    assert_eq!(
        shell.config["bar"]["layout"]["right"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn scan_timeout_does_not_activate_and_each_tray_gets_own_union() {
    let home = tempfile::tempdir().unwrap();
    let mut shell = MockShell {
        scan_delay: 100,
        ..Default::default()
    };
    assert!(install(home.path(), &mut shell).is_err());
    assert!(!activated(home.path()).exists());
    shell.scan_delay = 0;
    shell.config["bar"]["layout"]["left"] =
        json!([{"id":"omarchy.tray","hidden":["other","neoism"],"pinned":["x"]}]);
    install(home.path(), &mut shell).unwrap();
    assert_eq!(
        shell.config["bar"]["layout"]["left"][0]["hidden"],
        json!(["other", "neoism"])
    );
    assert_eq!(
        shell.config["bar"]["layout"]["left"][0]["pinned"],
        json!(["x"])
    );
}

#[test]
fn atomic_upgrade_only_for_pristine_owned_assets() {
    let home = tempfile::tempdir().unwrap();
    let dir = plugin(home.path());
    install_assets(&dir).unwrap();
    let old = b"old bundled version";
    fs::write(dir.join("BarWidget.qml"), old).unwrap();
    let mut saved = marker();
    saved["hashes"]["BarWidget.qml"] = json!(hash(old));
    fs::write(dir.join(OWNER), saved.to_string()).unwrap();
    install_assets(&dir).unwrap();
    assert_eq!(fs::read(dir.join("BarWidget.qml")).unwrap(), ASSETS[1].1);
    assert_eq!(fs::read_dir(dir.parent().unwrap()).unwrap().count(), 1);
}
