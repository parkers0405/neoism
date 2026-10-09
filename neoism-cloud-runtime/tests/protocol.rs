use neoism_cloud_runtime::*;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

fn allocation() -> Allocation {
    Allocation {
        owner: WorkspaceKey::new("tenant", "workspace").unwrap(),
        generation: 1,
        provider: "bridge".into(),
        spec: WorkspaceSpec {
            image: "ubuntu".into(),
            region: "east".into(),
            vcpus: 2,
            memory_mib: 2048,
            disk_gib: 20,
        },
        launch: None,
    }
}
fn handle() -> MachineHandle {
    let a = allocation();
    MachineHandle {
        owner: a.owner,
        generation: 1,
        provider: a.provider,
        machine_id: "vm-1".into(),
    }
}
fn status() -> MachineStatus {
    MachineStatus {
        handle: handle(),
        state: MachineState::Running,
        ready: true,
        connection: Some(
            WorkerConnection::new(handle(), "https://worker.example/agent/").unwrap(),
        ),
        failure: None,
    }
}
// Compare actual DTO serde field names, optionality, and unknown-field handling
// against the component schema. This deliberately does not replace a JSON Schema
// validator: equality of two handles and trusted URL semantics need Rust validation.
fn parity<T: Serialize + DeserializeOwned>(name: &str, dto: T) {
    let document = canonical_openapi();
    let schema = &document["components"]["schemas"][name];
    let encoded = serde_json::to_value(&dto).unwrap();
    let actual: BTreeSet<_> = encoded.as_object().unwrap().keys().cloned().collect();
    let expected: BTreeSet<_> = schema["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(actual, expected, "{name} serialized fields drifted");
    assert_eq!(schema["additionalProperties"], false);
    let _: T = serde_json::from_value(encoded.clone()).unwrap();
    for field in actual {
        let mut missing = encoded.clone();
        missing.as_object_mut().unwrap().remove(&field);
        let is_required = schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!(field));
        assert_eq!(
            serde_json::from_value::<T>(missing).is_err(),
            is_required,
            "{name}.{field} required drift"
        );
    }
    let mut extra = encoded;
    extra["unknown_field"] = json!(true);
    assert!(
        serde_json::from_value::<T>(extra).is_err(),
        "{name} silently accepts unknown fields"
    );
}

#[test]
fn component_schemas_match_actual_owned_dto_encoding() {
    let a = allocation();
    parity("WorkspaceKey", a.owner.clone());
    parity("WorkspaceSpec", a.spec.clone());
    parity("Allocation", a.clone());
    parity("MachineHandle", handle());
    parity(
        "WorkerConnection",
        WorkerConnection::new(handle(), "https://worker.example/agent/").unwrap(),
    );
    parity("MachineStatus", status());
    use base64::Engine;
    parity(
        "WorkerLaunchDescriptor",
        WorkerLaunchDescriptor::new(
            "runtime-1",
            "/workspace",
            "/state",
            i64::MAX,
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0; 32]),
        )
        .unwrap(),
    );
    parity(
        "ProviderError",
        ProviderError::new(FailureCode::Unavailable, true),
    );
    parity(
        "ProtocolRequest",
        ProtocolRequest::new(RuntimeAction::Ensure, a.clone(), None).unwrap(),
    );
    parity("ProtocolResponse", ProtocolResponse::new(status()));
    parity(
        "Capabilities",
        Capabilities {
            isolation: IsolationKind::VirtualMachine,
            cpu_limit: true,
            memory_limit: true,
            disk_limit: true,
            durable_workspace: true,
            stop_start: true,
        },
    );
    parity(
        "Binding",
        Binding {
            allocation: a,
            revision: 1,
            status: Some(status()),
            pending: None,
            last_error: None,
            retired: false,
        },
    );
}
#[test]
fn enum_schemas_match_serde_wire_names() {
    let document = canonical_openapi();
    let schemas = &document["components"]["schemas"];
    fn values<T: Serialize>(items: &[T]) -> Value {
        serde_json::to_value(items).unwrap()
    }
    assert_eq!(
        schemas["MachineState"]["enum"],
        values(&[
            MachineState::Provisioning,
            MachineState::Starting,
            MachineState::Running,
            MachineState::Stopping,
            MachineState::Stopped,
            MachineState::Failed,
            MachineState::Destroyed
        ])
    );
    assert_eq!(
        schemas["FailureCode"]["enum"],
        values(&[
            FailureCode::Transport,
            FailureCode::Timeout,
            FailureCode::Unauthorized,
            FailureCode::NotFound,
            FailureCode::Conflict,
            FailureCode::Rejected,
            FailureCode::Unavailable,
            FailureCode::Protocol,
            FailureCode::Identity
        ])
    );
    assert_eq!(
        schemas["Intent"]["enum"],
        values(&[Intent::Ensure, Intent::Start, Intent::Stop, Intent::Destroy])
    );
    assert_eq!(
        schemas["WorkerTransport"]["enum"],
        values(&[
            WorkerTransport::Https,
            WorkerTransport::DevelopmentLoopbackHttp
        ])
    );
}
#[test]
fn protocol_version_and_handles_are_validated_for_each_operation() {
    for action in RuntimeAction::ALL {
        let h = if action == RuntimeAction::Ensure {
            None
        } else {
            Some(handle())
        };
        let request = ProtocolRequest::new(action, allocation(), h).unwrap();
        let encoded = serde_json::to_value(&request).unwrap();
        assert_eq!(encoded["version"], 2);
        let decoded: ProtocolRequest = serde_json::from_value(encoded.clone()).unwrap();
        decoded.validate(action).unwrap();
        let mut missing = encoded.clone();
        missing.as_object_mut().unwrap().remove("handle");
        let decoded: ProtocolRequest = serde_json::from_value(missing).unwrap();
        assert_eq!(
            decoded.validate(action).is_ok(),
            matches!(action, RuntimeAction::Ensure | RuntimeAction::Inspect)
        );
        let mut wrong = request.clone();
        wrong.version = 1;
        assert_eq!(
            wrong.validate(action).unwrap_err().code,
            FailureCode::Protocol
        );
        wrong = request.clone();
        wrong.allocation.generation = 0;
        assert_eq!(
            wrong.validate(action).unwrap_err().code,
            FailureCode::Identity
        );
        wrong = request;
        wrong.handle = if action == RuntimeAction::Ensure {
            Some(handle())
        } else {
            None
        };
        if action == RuntimeAction::Inspect {
            wrong.validate(action).unwrap();
        } else {
            assert_eq!(
                wrong.validate(action).unwrap_err().code,
                FailureCode::Protocol
            );
        }
    }
    let mut foreign = handle();
    foreign.owner.tenant = "foreign".into();
    assert_eq!(
        ProtocolRequest::new(RuntimeAction::Inspect, allocation(), Some(foreign))
            .unwrap_err()
            .code,
        FailureCode::Identity
    );
    let encoded = serde_json::to_value(ProtocolResponse::new(status())).unwrap();
    let mut response: ProtocolResponse = serde_json::from_value(encoded).unwrap();
    response.validate(&allocation(), Some(&handle())).unwrap();
    response.version = 1;
    assert_eq!(
        response
            .validate(&allocation(), Some(&handle()))
            .unwrap_err()
            .code,
        FailureCode::Protocol
    );
}
#[test]
fn canonical_routes_security_and_constraints_are_stable() {
    let d = canonical_openapi();
    assert_eq!(d, canonical_openapi(), "generation must be deterministic");
    assert_eq!(d["openapi"], "3.1.0");
    assert_eq!(d["paths"].as_object().unwrap().len(), 5);
    for action in RuntimeAction::ALL {
        let operation = &d["paths"][format!("/v2/runtime/{}", action.as_str())]["post"];
        assert_eq!(
            operation["operationId"],
            format!("cloud.runtime.{}", action.as_str())
        );
        assert_eq!(operation["security"], json!([{"RuntimeBearer": []}]));
        assert_eq!(operation["requestBody"]["required"], true);
        let schema = &operation["requestBody"]["content"]["application/json"]["schema"];
        assert_eq!(
            schema["allOf"][0]["$ref"],
            "#/components/schemas/ProtocolRequest"
        );
        if action == RuntimeAction::Ensure {
            assert_eq!(schema["allOf"][1]["properties"]["handle"]["type"], "null");
        } else if action != RuntimeAction::Inspect {
            assert_eq!(schema["allOf"][1]["required"], json!(["handle"]));
            assert_eq!(
                schema["allOf"][1]["properties"]["handle"]["$ref"],
                "#/components/schemas/MachineHandle"
            );
        }
        for code in ["200", "202"] {
            assert_eq!(
                operation["responses"][code]["content"]["application/json"]["schema"]
                    ["$ref"],
                "#/components/schemas/ProtocolResponse"
            );
        }
    }
    let schemas = &d["components"]["schemas"];
    for name in ["ProtocolRequest", "ProtocolResponse"] {
        assert_eq!(schemas[name]["properties"]["version"]["const"], json!(2));
    }
    assert_eq!(
        schemas["WorkerConnection"]["properties"]["version"]["enum"],
        json!([1])
    );
    for name in ["Allocation", "MachineHandle"] {
        assert_eq!(schemas[name]["properties"]["generation"]["minimum"], 1);
    }
    assert_eq!(
        schemas["WorkspaceKey"]["properties"]["tenant"]["pattern"],
        "^[A-Za-z0-9_.:-]+$"
    );
    assert_eq!(
        schemas["WorkspaceKey"]["properties"]["workspace"]["maxLength"],
        128
    );
    let mut namespaced = allocation();
    namespaced.owner = WorkspaceKey::new("workspace:daemon.id-123", "notes.v1").unwrap();
    ProtocolRequest::new(RuntimeAction::Ensure, namespaced, None).unwrap();
    assert_eq!(
        schemas["Allocation"]["properties"]["provider"]["pattern"],
        "^[A-Za-z0-9_-]+$"
    );
    assert_eq!(
        schemas["WorkerConnection"]["properties"]["agent_api_base_url"]["maxLength"],
        2048
    );
    assert_eq!(
        d["components"]["securitySchemes"]["RuntimeBearer"]["scheme"],
        "bearer"
    );
    // All references resolve locally. No server host or credential defaults.
    fn references(v: &Value, document: &Value) {
        match v {
            Value::Object(o) => {
                if let Some(r) = o.get("$ref") {
                    assert!(
                        document
                            .pointer(r.as_str().unwrap().strip_prefix('#').unwrap())
                            .is_some(),
                        "{r}"
                    );
                }
                for value in o.values() {
                    references(value, document);
                }
            }
            Value::Array(a) => {
                for value in a {
                    references(value, document);
                }
            }
            _ => {}
        }
    }
    references(&d, &d);
    assert!(d.get("servers").is_none());
    for schema in schemas.as_object().unwrap().values() {
        if let Some(props) = schema.get("properties") {
            for secret in ["bearer", "token", "password", "credential"] {
                assert!(props.get(secret).is_none());
            }
        }
    }
}

#[test]
fn expired_launch_is_inspectable_and_destroyable_but_never_ensured_or_started() {
    use base64::Engine;
    let launch = WorkerLaunchDescriptor::new(
        "runtime-1",
        "/workspace",
        "/state",
        i64::MAX,
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7; 32]),
    )
    .unwrap();
    let mut wire = serde_json::to_value(launch).unwrap();
    wire["expires_at"] = 1.into();
    let mut a = allocation();
    a.launch = Some(serde_json::from_value(wire).unwrap());
    for action in RuntimeAction::ALL {
        let h = if action == RuntimeAction::Ensure {
            None
        } else {
            Some(handle())
        };
        let result = ProtocolRequest::new(action, a.clone(), h);
        assert_eq!(
            result.is_ok(),
            matches!(
                action,
                RuntimeAction::Inspect | RuntimeAction::Stop | RuntimeAction::Destroy
            )
        );
    }
    ProtocolRequest::new(RuntimeAction::Inspect, a, None).unwrap();
}
