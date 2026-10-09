#![cfg(target_os = "linux")]
// Compile the owned module even while the parent is wiring its public export.
#[path = "../src/docker.rs"]
mod docker;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use docker::{DockerConfig, DockerDevelopmentProvider};
use neoism_cloud_runtime::*;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

static TEST_STATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Fixture {
    dir: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir()
            .join(format!("neoism-docker-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(dir.join("docker"), FAKE).unwrap();
        fs::set_permissions(dir.join("docker"), fs::Permissions::from_mode(0o700))
            .unwrap();
        Self { dir }
    }
    fn config(&self) -> DockerConfig {
        DockerConfig {
            id: "docker-test".into(),
            controller_directory: self.dir.clone(),
            image_catalog: BTreeMap::from([(
                "worker".into(),
                "trusted/worker:test".into(),
            )]),
            allow_unenforced_disk: true,
        }
    }
    fn provider(&self) -> DockerDevelopmentProvider {
        DockerDevelopmentProvider::with_cli(self.config(), self.dir.join("docker"))
            .unwrap()
    }
    fn calls(&self) -> Vec<Vec<String>> {
        fs::read_to_string(self.dir.join("calls"))
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn allocation() -> Allocation {
    Allocation {
        owner: WorkspaceKey::new("tenant", "workspace").unwrap(),
        generation: 1,
        provider: "docker-test".into(),
        spec: WorkspaceSpec {
            image: "worker".into(),
            region: "local".into(),
            vcpus: 2,
            memory_mib: 512,
            disk_gib: 10,
        },
        launch: Some(
            WorkerLaunchDescriptor::new(
                "runtime-test",
                "/workspace",
                "/var/lib/neoism",
                (SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    + 3600) as i64,
                URL_SAFE_NO_PAD.encode([42u8; 32]),
            )
            .unwrap(),
        ),
    }
}
const FAKE: &str = r#"#!/usr/bin/python3
import sys,json,pathlib
base=pathlib.Path(__file__).parent
a=sys.argv[1:]
with (base/'calls').open('a') as f: f.write(json.dumps(a)+'\n')
p=base/'fake-state'
s=json.loads(p.read_text()) if p.exists() else {'containers':{},'volumes':{}}
kind,op=a[:2]
def labels(): return dict(x.split('=',1) for i,x in enumerate(a) if i and a[i-1]=='--label')
def save(): p.write_text(json.dumps(s))
if op=='ls':
    print('\n'.join(s['containers' if kind=='container' else 'volumes']))
elif kind=='volume':
    name=a[-1]
    if op=='create': s['volumes'].setdefault(name,{'Name':name,'Driver':'local','Options':None,'Labels':labels()});save();print(name)
    else: print(json.dumps([s['volumes'][name]]))
elif op=='create':
    name=a[a.index('--name')+1]
    if name in s['containers']: sys.exit(1)
    s['containers'][name]={'Name':'/'+name,'Config':{'Labels':labels()},'State':{'Running':False,'Status':'created'},'NetworkSettings':{'Ports':{'4096/tcp':[{'HostIp':'127.0.0.1','HostPort':'23456'}]}}};save()
    if (base/'pause-create').exists():
        import time
        (base/'create-entered').write_text('')
        time.sleep(20)
    if (base/'lose-reply').exists(): (base/'lose-reply').unlink();sys.stderr.write('SECRET daemon diagnostics');sys.exit(1)
    print(name)
elif op=='inspect': print(json.dumps([s['containers'][a[-1]]]))
elif op=='start': s['containers'][a[-1]]['State']={'Running':True,'Status':'running'};save()
elif op=='stop': s['containers'][a[-1]]['State']={'Running':False,'Status':'exited'};save()
elif op=='rm': del s['containers'][a[-1]];save()
else: sys.exit(2)
"#;

#[test]
fn opt_in_and_truthful_caps() {
    let _serial = TEST_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let f = Fixture::new();
    let mut c = f.config();
    c.allow_unenforced_disk = false;
    assert!(DockerDevelopmentProvider::with_cli(c, f.dir.join("docker")).is_err());
    assert!(
        DockerDevelopmentProvider::with_cli(f.config(), PathBuf::from("docker")).is_err()
    );
    let p = f.provider();
    let caps = p.capabilities();
    assert_eq!(caps.isolation, IsolationKind::Container);
    assert!(!caps.disk_limit);
    assert!(
        caps.cpu_limit && caps.memory_limit && caps.stop_start && caps.durable_workspace
    );
    // Keep the default constructor covered without executing its CLI.
    assert!(DockerDevelopmentProvider::new(f.config()).is_ok());
}
#[tokio::test]
async fn launch_arguments_public_bootstrap_and_durable_lifecycle() {
    let _serial = TEST_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let f = Fixture::new();
    let p = f.provider();
    let a = allocation();
    let s = p.ensure(&a).await.unwrap();
    assert_eq!(s.state, MachineState::Running);
    assert!(!s.ready);
    assert!(s.connection.is_some());
    s.validate(&a, None).unwrap();
    let calls = f.calls();
    let create = calls
        .iter()
        .find(|c| c.get(1).is_some_and(|x| x == "create") && c[0] == "container")
        .unwrap();
    for pair in [
        ["--user", "10001:10001"],
        ["--cap-drop", "ALL"],
        ["--publish", "127.0.0.1::4096"],
        ["--network", "bridge"],
        ["--security-opt", "no-new-privileges:true"],
        ["--cpus", "2"],
        ["--memory", "512m"],
        ["--pids-limit", "512"],
    ] {
        assert!(create
            .windows(2)
            .any(|w| w[0] == pair[0] && w[1] == pair[1]));
    }
    assert!(create.contains(&"--read-only".into()));
    assert_eq!(create.last().unwrap(), "trusted/worker:test");
    assert!(!create.iter().any(|s| s.contains("docker.sock")
        || s.contains("privileged")
        || s.contains("signing")));
    let dir = f.dir.join(&s.handle.machine_id);
    let b: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("bootstrap.json")).unwrap()).unwrap();
    assert_eq!(b["tenantId"], "tenant");
    assert_eq!(b["runtimeGeneration"], 1);
    assert_eq!(b["version"], 1);
    assert_eq!(b.as_object().unwrap().len(), 7);
    assert_eq!(
        fs::read(dir.join("verification.key")).unwrap(),
        vec![42u8; 32]
    );
    let mut foreign = s.handle.clone();
    foreign.owner.tenant = "other".into();
    assert_eq!(
        p.destroy(&a, &foreign).await.unwrap_err().code,
        FailureCode::Identity
    );
    assert_eq!(
        p.stop(&a, &s.handle).await.unwrap().state,
        MachineState::Stopped
    );
    assert!(p
        .inspect(&a, Some(&s.handle))
        .await
        .unwrap()
        .connection
        .is_none());
    assert_eq!(p.start(&a, &s.handle).await.unwrap().handle, s.handle);
    let mut next = a.clone();
    next.generation = 2;
    assert_eq!(
        p.ensure(&next).await.unwrap_err().code,
        FailureCode::Conflict
    );
    p.destroy(&a, &s.handle).await.unwrap();
    drop(p);
    let p = f.provider();
    assert_eq!(p.ensure(&a).await.unwrap_err().code, FailureCode::Conflict);
    assert_eq!(
        p.start(&a, &s.handle).await.unwrap_err().code,
        FailureCode::Conflict
    );
    assert_eq!(
        p.destroy(&a, &s.handle).await.unwrap().state,
        MachineState::Destroyed
    );
    let second = p.ensure(&next).await.unwrap();
    assert_ne!(second.handle.machine_id, s.handle.machine_id);
    let calls = f.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c[0] == "volume" && c[1] == "create")
            .count(),
        2
    );
    assert!(!calls.iter().any(|c| c[0] == "volume" && c[1] == "rm"));
}
#[tokio::test]
async fn lost_create_reply_recovers_name_and_immutable_digest() {
    let _serial = TEST_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let f = Fixture::new();
    let a = allocation();
    fs::write(f.dir.join("lose-reply"), "").unwrap();
    let error = f.provider().ensure(&a).await.unwrap_err();
    assert!(!format!("{error:?}").contains("SECRET"));
    let p = f.provider();
    let s = p.ensure(&a).await.unwrap();
    assert_eq!(
        f.calls()
            .iter()
            .filter(|c| c[0] == "container" && c[1] == "create")
            .count(),
        1
    );
    let mut changed = a.clone();
    changed.spec.memory_mib = 1024;
    assert_eq!(
        p.ensure(&changed).await.unwrap_err().code,
        FailureCode::Identity
    );
    let mut changed = a.clone();
    changed.launch = Some(
        WorkerLaunchDescriptor::new(
            "different",
            "/workspace",
            "/var/lib/neoism",
            a.launch.as_ref().unwrap().expires_at(),
            URL_SAFE_NO_PAD.encode([42u8; 32]),
        )
        .unwrap(),
    );
    assert_eq!(
        p.ensure(&changed).await.unwrap_err().code,
        FailureCode::Identity
    );
    let mut fake = s.handle.clone();
    fake.machine_id.push_str("-wrong");
    assert_eq!(
        p.inspect(&a, Some(&fake)).await.unwrap_err().code,
        FailureCode::Identity
    );
    // A confirmed machine that disappears must not be recreated under its old ID.
    let path = f.dir.join("fake-state");
    let mut v: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    v["containers"].as_object_mut().unwrap().clear();
    fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
    assert_eq!(p.ensure(&a).await.unwrap_err().code, FailureCode::NotFound);
    p.destroy(&a, &s.handle).await.unwrap();
}
#[tokio::test]
async fn unrelated_catalog_changes_do_not_change_generation_identity() {
    let _serial = TEST_STATE.lock().unwrap_or_else(|e| e.into_inner());
    for change in ["add", "change", "remove"] {
        let f = Fixture::new();
        let a = allocation();
        let mut original = f.config();
        if change != "add" {
            original
                .image_catalog
                .insert("unrelated".into(), "trusted/other:v1".into());
        }
        let p =
            DockerDevelopmentProvider::with_cli(original.clone(), f.dir.join("docker"))
                .unwrap();
        let s = p.ensure(&a).await.unwrap();
        drop(p);
        let before: serde_json::Value =
            serde_json::from_slice(&fs::read(f.dir.join("fake-state")).unwrap()).unwrap();
        let digest = before["containers"][&s.handle.machine_id]["Config"]["Labels"]
            ["dev.neoism.digest"]
            .clone();
        match change {
            "add" => {
                original
                    .image_catalog
                    .insert("unrelated".into(), "trusted/other:v1".into());
            }
            "change" => {
                original
                    .image_catalog
                    .insert("unrelated".into(), "trusted/other:v2".into());
            }
            "remove" => {
                original.image_catalog.remove("unrelated");
            }
            _ => unreachable!(),
        }
        let p =
            DockerDevelopmentProvider::with_cli(original, f.dir.join("docker")).unwrap();
        assert_eq!(
            p.inspect(&a, Some(&s.handle)).await.unwrap().handle,
            s.handle
        );
        assert_eq!(
            p.stop(&a, &s.handle).await.unwrap().state,
            MachineState::Stopped
        );
        assert_eq!(
            p.start(&a, &s.handle).await.unwrap().state,
            MachineState::Running
        );
        assert_eq!(p.ensure(&a).await.unwrap().handle, s.handle);
        let after: serde_json::Value =
            serde_json::from_slice(&fs::read(f.dir.join("fake-state")).unwrap()).unwrap();
        assert_eq!(
            after["containers"][&s.handle.machine_id]["Config"]["Labels"]
                ["dev.neoism.digest"],
            digest
        );
        p.destroy(&a, &s.handle).await.unwrap();
        assert_eq!(
            f.calls()
                .iter()
                .filter(|c| c[0] == "container" && c[1] == "create")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn selected_image_upgrade_fences_launch_but_preserves_cleanup_and_new_generation() {
    let _serial = TEST_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let f = Fixture::new();
    let mut a = allocation();
    let p = f.provider();
    let first = p.ensure(&a).await.unwrap();
    drop(p);
    let mut changed = f.config();
    changed
        .image_catalog
        .insert("worker".into(), "trusted/worker:v2".into());
    let p = DockerDevelopmentProvider::with_cli(changed.clone(), f.dir.join("docker"))
        .unwrap();
    let call_count = f.calls().len();
    assert_eq!(p.ensure(&a).await.unwrap_err().code, FailureCode::Conflict);
    assert_eq!(
        p.start(&a, &first.handle).await.unwrap_err().code,
        FailureCode::Conflict
    );
    assert_eq!(
        f.calls().len(),
        call_count,
        "changed image must reject before Docker side effects"
    );
    assert_eq!(
        p.inspect(&a, None).await.unwrap().state,
        MachineState::Running
    );
    assert_eq!(
        p.stop(&a, &first.handle).await.unwrap().state,
        MachineState::Stopped
    );
    p.destroy(&a, &first.handle).await.unwrap();
    a.generation = 2;
    let second = p.ensure(&a).await.unwrap();
    drop(p);
    let calls = f.calls();
    let creates: Vec<_> = calls
        .iter()
        .filter(|c| c[0] == "container" && c[1] == "create")
        .collect();
    assert_eq!(creates.len(), 2);
    assert_eq!(creates[0].last().unwrap(), "trusted/worker:test");
    assert_eq!(creates[1].last().unwrap(), "trusted/worker:v2");
    // Removing the selected alias also cannot strand the old generation.
    changed.image_catalog.remove("worker");
    assert!(changed.image_catalog.is_empty());
    let p = DockerDevelopmentProvider::with_cli(changed, f.dir.join("docker")).unwrap();
    assert_eq!(p.ensure(&a).await.unwrap_err().code, FailureCode::Rejected);
    assert_eq!(
        p.start(&a, &second.handle).await.unwrap_err().code,
        FailureCode::Rejected
    );
    p.inspect(&a, Some(&second.handle)).await.unwrap();
    p.stop(&a, &second.handle).await.unwrap();
    p.destroy(&a, &second.handle).await.unwrap();
    assert_eq!(
        p.destroy(&a, &second.handle).await.unwrap().state,
        MachineState::Destroyed
    );
}

#[tokio::test]
async fn cancellation_and_cross_instance_lock_recover_durable_intent() {
    let _serial = TEST_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let f = Fixture::new();
    let p = f.provider();
    let a = allocation();
    fs::write(f.dir.join("pause-create"), "").unwrap();
    let cancelled = async {
        let mut pending = Box::pin(p.ensure(&a));
        loop {
            tokio::select! {
                r=&mut pending => panic!("create should still be paused: {r:?}"),
                _=tokio::time::sleep(std::time::Duration::from_millis(10)) => {
                    if f.dir.join("create-entered").exists() { break; }
                }
            }
        }
        let other = f.provider();
        assert_eq!(
            other.ensure(&a).await.unwrap_err().code,
            FailureCode::Unavailable
        );
        // Dropping the pending future kills its CLI process, but retains intent.
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), cancelled)
        .await
        .unwrap();
    fs::remove_file(f.dir.join("pause-create")).unwrap();
    let other = f.provider();
    let s = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match other.ensure(&a).await {
                Ok(s) => break s,
                Err(e) if e.retryable => {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await
                }
                Err(e) => panic!("recovery failed: {e:?}"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(s.state, MachineState::Running);
    assert_eq!(
        f.calls()
            .iter()
            .filter(|c| c[0] == "container" && c[1] == "create")
            .count(),
        1
    );
    // Docker health, not a running flag or endpoint, is the readiness source.
    let path = f.dir.join("fake-state");
    let mut v: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    v["containers"][&s.handle.machine_id]["State"]["Health"] =
        serde_json::json!({"Status":"healthy"});
    fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    assert!(other.inspect(&a, None).await.unwrap().ready);
    // Other tenants cannot overlap this workspace's named volumes or containers.
    let mut another = a.clone();
    another.owner.tenant = "different-tenant".into();
    let second = other.ensure(&another).await.unwrap();
    assert_ne!(second.handle.machine_id, s.handle.machine_id);
    let v: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(v["volumes"].as_object().unwrap().len(), 4);
}

#[tokio::test]
async fn expired_descriptor_cannot_restart_but_can_be_destroyed() {
    let _serial = TEST_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let f = Fixture::new();
    let p = f.provider();
    let mut a = allocation();
    let deadline = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3) as i64;
    a.launch = Some(
        WorkerLaunchDescriptor::new(
            "runtime-test",
            "/workspace",
            "/var/lib/neoism",
            deadline,
            URL_SAFE_NO_PAD.encode([42u8; 32]),
        )
        .unwrap(),
    );
    let s = p.ensure(&a).await.unwrap();
    p.stop(&a, &s.handle).await.unwrap();
    while SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        < deadline as u64
    {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(
        p.start(&a, &s.handle).await.unwrap_err().code,
        FailureCode::Rejected
    );
    assert_eq!(p.ensure(&a).await.unwrap_err().code, FailureCode::Rejected);
    p.destroy(&a, &s.handle).await.unwrap();
}

#[tokio::test]
async fn foreign_container_volume_and_invalid_paths_rejected() {
    let _serial = TEST_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let f = Fixture::new();
    let p = f.provider();
    let a = allocation();
    assert_eq!(
        p.inspect(&a, None).await.unwrap_err().code,
        FailureCode::NotFound
    );
    let mut invalid = a.clone();
    invalid.launch = Some(
        WorkerLaunchDescriptor::new(
            "runtime-test",
            "C:/workspace",
            "C:/state",
            a.launch.as_ref().unwrap().expires_at(),
            URL_SAFE_NO_PAD.encode([42u8; 32]),
        )
        .unwrap(),
    );
    assert_eq!(
        p.ensure(&invalid).await.unwrap_err().code,
        FailureCode::Rejected
    );
    let s = p.ensure(&a).await.unwrap();
    let path = f.dir.join("fake-state");
    let mut v: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    v["containers"][&s.handle.machine_id]["Config"]["Labels"]["dev.neoism.digest"] =
        "foreign".into();
    fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    assert_eq!(
        p.destroy(&a, &s.handle).await.unwrap_err().code,
        FailureCode::Identity
    );
    assert_eq!(p.ensure(&a).await.unwrap_err().code, FailureCode::Conflict); // destroy intent fences late ensure
                                                                             // New owner keeps its own volumes; an arbitrary preexisting volume is never adopted.
    let f2 = Fixture::new();
    fs::write(f2.dir.join("lose-reply"), "").unwrap();
    let p2 = f2.provider();
    let _ = p2.ensure(&a).await;
    let path = f2.dir.join("fake-state");
    let mut v: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    v["containers"].as_object_mut().unwrap().clear();
    for volume in v["volumes"].as_object_mut().unwrap().values_mut() {
        volume["Labels"]["dev.neoism.owner"] = "foreign".into();
    }
    fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
    assert_eq!(p2.ensure(&a).await.unwrap_err().code, FailureCode::Identity);
}

async fn await_healthy(p: &DockerDevelopmentProvider, a: &Allocation) {
    tokio::time::timeout(std::time::Duration::from_secs(25), async {
        loop {
            let s = p.inspect(a, None).await.unwrap();
            assert_eq!(
                s.state,
                MachineState::Running,
                "worker exited before healthy"
            );
            if s.ready {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("worker never became Docker-healthy");
}
async fn test_output(cli: &std::path::Path, args: &[String]) -> std::process::Output {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::process::Command::new(cli)
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap()
}
async fn cleanup_owned_test(cli: &std::path::Path, owner: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(35), async {
        for (kind, suffix) in [
            ("container", "g1"),
            ("container", "g2"),
            ("volume", "workspace"),
            ("volume", "state"),
        ] {
            let name = format!("neoism-{owner}-{suffix}");
            let output =
                test_output(cli, &[kind.into(), "inspect".into(), name.clone()]).await;
            if !output.status.success() {
                continue;
            }
            let values: Vec<serde_json::Value> =
                serde_json::from_slice(&output.stdout).unwrap();
            let labels = if kind == "container" {
                &values[0]["Config"]["Labels"]
            } else {
                &values[0]["Labels"]
            };
            assert_eq!(labels["dev.neoism.owner"], owner);
            assert_eq!(labels["dev.neoism.provider"], "docker-test");
            let args = if kind == "container" {
                vec![kind.into(), "rm".into(), "--force".into(), name]
            } else {
                vec![kind.into(), "rm".into(), name]
            };
            assert!(test_output(cli, &args).await.status.success());
        }
    })
    .await
    .expect("owned Docker test cleanup timed out");
}

/// Explicit operator opt-in only. No prune, no discovery of user containers, no credentials.
#[tokio::test]
#[ignore = "requires NEOISM_DOCKER_TEST_IMAGE trusted worker image and a local Docker daemon"]
async fn real_docker_named_volume_survives_stop_and_generation() {
    let image =
        std::env::var("NEOISM_DOCKER_TEST_IMAGE").expect("set NEOISM_DOCKER_TEST_IMAGE");
    let _serial = TEST_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let f = Fixture::new();
    let mut c = f.config();
    c.image_catalog.insert("worker".into(), image);
    let cli = PathBuf::from("/usr/bin/docker");
    let p = DockerDevelopmentProvider::with_cli(c, cli.clone()).unwrap();
    let mut a = allocation();
    a.owner = WorkspaceKey::new(format!("test-{}", uuid::Uuid::new_v4()), "durability")
        .unwrap();
    use sha2::{Digest, Sha256};
    let owner = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(p.id(), &a.owner)).unwrap())
    );
    let test_cli = cli.clone();
    let mut task = tokio::spawn(async move {
        let s = p.ensure(&a).await.unwrap();
        await_healthy(&p, &a).await;
        let exec = |name: String, args: Vec<&'static str>| {
            let cli = cli.clone();
            async move {
                let status = tokio::process::Command::new(cli)
                    .arg("exec")
                    .arg(name)
                    .args(args)
                    .kill_on_drop(true)
                    .status()
                    .await
                    .unwrap();
                assert!(status.success());
            }
        };
        exec(
            s.handle.machine_id.clone(),
            vec!["touch", "/workspace/neoism-durability-marker"],
        )
        .await;
        exec(
            s.handle.machine_id.clone(),
            vec!["touch", "/var/lib/neoism/state/neoism-durability-marker"],
        )
        .await;
        p.stop(&a, &s.handle).await.unwrap();
        p.start(&a, &s.handle).await.unwrap();
        exec(
            s.handle.machine_id.clone(),
            vec!["test", "-f", "/workspace/neoism-durability-marker"],
        )
        .await;
        p.destroy(&a, &s.handle).await.unwrap();
        a.generation = 2;
        let s2 = p.ensure(&a).await.unwrap();
        await_healthy(&p, &a).await;
        exec(
            s2.handle.machine_id.clone(),
            vec![
                "test",
                "-f",
                "/var/lib/neoism/state/neoism-durability-marker",
            ],
        )
        .await;
        exec(
            s2.handle.machine_id.clone(),
            vec!["test", "-f", "/workspace/neoism-durability-marker"],
        )
        .await;
        p.destroy(&a, &s2.handle).await.unwrap();
        // The provider never removes volumes; label-verified test cleanup below
        // explicitly removes only this test's own namespace.
    });
    let result =
        tokio::time::timeout(std::time::Duration::from_secs(140), &mut task).await;
    if result.is_err() {
        task.abort();
        let _ = task.await;
    }
    cleanup_owned_test(&test_cli, &owner).await;
    result
        .expect("Docker test exceeded lifecycle deadline")
        .expect("Docker lifecycle assertion failed");
}
