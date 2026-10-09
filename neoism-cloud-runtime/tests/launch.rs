use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use neoism_cloud_runtime::*;

fn expiry() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        + 3600
}
fn descriptor(root: &str, state: &str) -> Result<WorkerLaunchDescriptor> {
    WorkerLaunchDescriptor::new(
        "runtime-opaque",
        root,
        state,
        expiry(),
        URL_SAFE_NO_PAD.encode([7; 32]),
    )
}
#[test]
fn launch_paths_are_vm_namespace_normalized_and_disjoint() {
    for (root, state) in [
        ("/workspace/project", "/worker-state"),
        ("C:/workspace", "D:/worker-state"),
    ] {
        let d = descriptor(root, state).unwrap();
        assert_eq!(d.version(), 1);
        assert_eq!(d.root(), root);
        d.validate_active().unwrap();
        let mut encoded = serde_json::to_value(&d).unwrap();
        encoded["private_seed"] = "secret".into();
        assert!(serde_json::from_value::<WorkerLaunchDescriptor>(encoded).is_err());
    }
    for root in [
        "/",
        "C:/",
        "c:/workspace",
        "C:\\workspace",
        "relative",
        "/a/../b",
        "/a/./b",
        "//host/a",
        "/a//b",
        "/a/",
        "C:workspace",
        "/a\n",
    ] {
        assert!(descriptor(root, "/state").is_err(), "{root}");
    }
    for (root, state) in [
        ("/a", "/a/state"),
        ("/a", "/a"),
        ("C:/Work", "C:/work/state"),
        ("/a/project", "/a"),
    ] {
        assert!(descriptor(root, state).is_err());
    }
    assert!(descriptor("/a", "/ab").is_ok());
}
#[test]
fn key_version_and_expiry_validation_are_explicit() {
    let d = descriptor("/workspace", "/state").unwrap();
    for (field, value) in [
        ("version", serde_json::json!(2)),
        ("verification_key", serde_json::json!("bad=")),
        ("expires_at", serde_json::json!(0)),
    ] {
        let mut wire = serde_json::to_value(&d).unwrap();
        wire[field] = value;
        let invalid: WorkerLaunchDescriptor = serde_json::from_value(wire).unwrap();
        assert!(invalid.validate().is_err());
    }
    let mut wire = serde_json::to_value(&d).unwrap();
    wire["expires_at"] = 1.into();
    let expired: WorkerLaunchDescriptor = serde_json::from_value(wire).unwrap();
    expired.validate().unwrap();
    assert!(matches!(
        expired.validate_active(),
        Err(Error::ExpiredLaunch)
    ));
    let mut wire = serde_json::to_value(&d).unwrap();
    // Same decoded bytes but noncanonical final base64 bits.
    let canonical = d.verification_key();
    let mut noncanonical = canonical[..42].to_string();
    noncanonical.push('d');
    wire["verification_key"] = noncanonical.into();
    let invalid: WorkerLaunchDescriptor = serde_json::from_value(wire).unwrap();
    assert!(invalid.validate().is_err());
}
