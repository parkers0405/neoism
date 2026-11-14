use super::*;

fn scratch(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("rune-{name}-{}-{nanos}", std::process::id()))
}

/// The first `on_ready` writes `<bundle id>.<pid>` as one line of JSON into a directory it
/// makes private; later ones write nothing; exiting removes it.
#[test]
fn a_launcher_can_wait_for_the_ready_file() {
    let root = scratch("ready");
    let dir = root.join("ready");
    ready_in(Some(dir.as_os_str()));
    let path = dir.join(format!("{}.{}", bundle_id(), std::process::id()));
    let text = std::fs::read_to_string(&path).expect("the ready file");
    assert!(text.ends_with('\n') && text.lines().count() == 1, "{text:?}");
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["pid"], std::process::id());
    assert_eq!(json["bundle_id"], bundle_id());
    assert_eq!(json["rune"], env!("CARGO_PKG_VERSION"));
    assert!(json["ready_ms"].is_u64());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary file is left");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    }

    let other = root.join("other");
    ready_in(Some(other.as_os_str()));
    assert!(!other.exists(), "once per process: another window's on_ready writes nothing");

    clear();
    assert!(!path.exists(), "exiting removes it");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn without_the_variable_nothing_is_written() {
    let signal = Signal { pid: 7, bundle_id: "com.example.app".into(), ready_ms: 412 };
    assert!(signal.write_in(None).is_none());
    assert!(signal.write_in(Some(OsStr::new(""))).is_none());
    assert_eq!(signal.json(), format!(r#"{{"pid":7,"bundle_id":"com.example.app","ready_ms":412,"rune":"{}"}}"#, env!("CARGO_PKG_VERSION")));
}
