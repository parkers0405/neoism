use crate::*;
use serde_json::{json, Map, Value};

/// Route identity shared by the HTTP client, validation, and canonical contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeAction {
    Ensure,
    Start,
    Inspect,
    Stop,
    Destroy,
}
impl RuntimeAction {
    pub const ALL: [Self; 5] = [
        Self::Ensure,
        Self::Start,
        Self::Inspect,
        Self::Stop,
        Self::Destroy,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ensure => "ensure",
            Self::Start => "start",
            Self::Inspect => "inspect",
            Self::Stop => "stop",
            Self::Destroy => "destroy",
        }
    }
}

/// Owned v2 wire request. Validate against its route before any native provider call.
/// No authentication material belongs in this DTO.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProtocolRequest {
    pub version: u32,
    pub allocation: Allocation,
    pub handle: Option<MachineHandle>,
}
impl ProtocolRequest {
    pub fn new(
        action: RuntimeAction,
        allocation: Allocation,
        handle: Option<MachineHandle>,
    ) -> std::result::Result<Self, ProviderError> {
        let request = Self {
            version: 2,
            allocation,
            handle,
        };
        request.validate(action)?;
        Ok(request)
    }
    pub fn validate(
        &self,
        action: RuntimeAction,
    ) -> std::result::Result<(), ProviderError> {
        if self.version != 2
            || (action == RuntimeAction::Ensure && self.handle.is_some())
            || (!matches!(action, RuntimeAction::Ensure | RuntimeAction::Inspect)
                && self.handle.is_none())
        {
            return Err(ProviderError::new(FailureCode::Protocol, false));
        }
        let a = &self.allocation;
        if a.owner.validate().is_err()
            || a.spec.validate().is_err()
            || a.generation == 0
            || !valid_id(&a.provider)
            || self.handle.as_ref().is_some_and(|h| {
                h.owner != a.owner
                    || h.provider != a.provider
                    || h.generation != a.generation
                    || !valid_id(&h.machine_id)
            })
        {
            return Err(ProviderError::new(FailureCode::Identity, false));
        }
        if let Some(launch) = &a.launch {
            launch
                .validate()
                .map_err(|_| ProviderError::new(FailureCode::Protocol, false))?;
            if matches!(action, RuntimeAction::Ensure | RuntimeAction::Start) {
                launch
                    .validate_active()
                    .map_err(|_| ProviderError::new(FailureCode::Rejected, false))?;
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProtocolResponse {
    pub version: u32,
    pub status: MachineStatus,
}
impl ProtocolResponse {
    pub fn new(status: MachineStatus) -> Self {
        Self { version: 2, status }
    }
    pub fn validate(
        &self,
        allocation: &Allocation,
        expected: Option<&MachineHandle>,
    ) -> std::result::Result<(), ProviderError> {
        if self.version != 2 {
            return Err(ProviderError::new(FailureCode::Protocol, false));
        }
        self.status.validate(allocation, expected)
    }
}

fn reference(name: &str) -> Value {
    json!({"$ref": format!("#/components/schemas/{name}")})
}
fn nullable(name: &str) -> Value {
    json!({"anyOf": [reference(name), {"type": "null"}]})
}
fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "additionalProperties": false, "properties": properties, "required": required})
}
fn id() -> Value {
    json!({"type": "string", "minLength": 1, "maxLength": 128, "pattern": "^[A-Za-z0-9_-]+$"})
}
fn owner_id() -> Value {
    json!({"type": "string", "minLength": 1, "maxLength": 128, "pattern": "^[A-Za-z0-9_.:-]+$"})
}
fn integer(min: u64, max: u64) -> Value {
    json!({"type": "integer", "minimum": min, "maximum": max})
}
fn version() -> Value {
    json!({"type": "integer", "enum": [1]})
}

/// Authoritative OpenAPI 3.1 bridge contract, generated entirely inside this crate.
/// Hand-schema fallback avoids new dependencies/root lockfile changes. DTO parity
/// and operation constraints are tested. Cross-object equality, URL trust, actual
/// readiness, authorization and durable upstream fencing still require validation.
pub fn canonical_openapi() -> Value {
    let mut schemas = Map::new();
    schemas.insert(
        "WorkspaceKey".into(),
        object(
            json!({"tenant": owner_id(), "workspace": owner_id()}),
            &["tenant", "workspace"],
        ),
    );
    schemas.insert("WorkspaceSpec".into(), object(json!({"image": id(), "region": id(), "vcpus": integer(1, 1024), "memory_mib": integer(128, 4_194_304), "disk_gib": integer(1, 65_536)}), &["image", "region", "vcpus", "memory_mib", "disk_gib"]));
    schemas.insert("Allocation".into(), object(json!({"owner": reference("WorkspaceKey"), "generation": integer(1, u64::MAX), "provider": id(), "spec": reference("WorkspaceSpec"), "launch": nullable("WorkerLaunchDescriptor")}), &["owner", "generation", "provider", "spec"]));
    schemas.insert("WorkerLaunchDescriptor".into(), object(json!({"version": version(), "runtime_id": id(), "root": {"type":"string", "minLength":2, "maxLength":4096, "description":"Normalized absolute VM POSIX or uppercase drive-root path with forward slashes; no volume root or traversal."}, "state_root": {"type":"string", "minLength":2, "maxLength":4096, "description":"Normalized VM path outside root; validated independently of coordinator filesystem."}, "expires_at": integer(1, i64::MAX as u64), "verification_key": {"type":"string", "minLength":43, "maxLength":43, "pattern":"^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$"}}), &["version","runtime_id","root","state_root","expires_at","verification_key"]));
    schemas.insert("MachineHandle".into(), object(json!({"owner": reference("WorkspaceKey"), "generation": integer(1, u64::MAX), "provider": id(), "machine_id": id()}), &["owner", "generation", "provider", "machine_id"]));
    schemas.insert("MachineState".into(), json!({"type": "string", "enum": ["provisioning", "starting", "running", "stopping", "stopped", "failed", "destroyed"]}));
    schemas.insert("FailureCode".into(), json!({"type": "string", "enum": ["transport", "timeout", "unauthorized", "not_found", "conflict", "rejected", "unavailable", "protocol", "identity"]}));
    schemas.insert(
        "ProviderError".into(),
        object(
            json!({"code": reference("FailureCode"), "retryable": {"type": "boolean"}}),
            &["code", "retryable"],
        ),
    );
    schemas.insert(
        "WorkerTransport".into(),
        json!({"type": "string", "enum": ["https", "development_loopback_http"]}),
    );
    let mut worker = object(
        json!({"version": version(), "handle": reference("MachineHandle"), "agent_api_base_url": {"type": "string", "format": "uri", "minLength": 1, "maxLength": 2048, "description": "Canonical absolute API base URL ending in /. No credentials, query, fragment, controls, whitespace or backslash. Verified by the trusted provider for this exact handle; not a user-selected URL."}, "transport": reference("WorkerTransport")}),
        &["version", "handle", "agent_api_base_url", "transport"],
    );
    worker["allOf"] = json!([
        {"if": {"properties": {"transport": {"const": "https"}}, "required": ["transport"]}, "then": {"properties": {"agent_api_base_url": {"pattern": "^https://[^/?#@\\s\\\\]+/[^?#\\s\\\\]*$"}}}},
        {"if": {"properties": {"transport": {"const": "development_loopback_http"}}, "required": ["transport"]}, "then": {"properties": {"agent_api_base_url": {"pattern": "^http://(127\\.[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}|\\[::1\\])(:[0-9]+)?/[^?#\\s\\\\]*$"}}}}
    ]);
    schemas.insert("WorkerConnection".into(), worker);
    let mut status = object(
        json!({"handle": reference("MachineHandle"), "state": reference("MachineState"), "ready": {"type": "boolean"}, "connection": nullable("WorkerConnection"), "failure": nullable("ProviderError")}),
        &["handle", "state", "ready"],
    );
    status["description"] = json!("Ready running requires a scoped connection. Connection handle must equal status handle, including tenant, workspace, generation, provider and machine ID. JSON Schema cannot express that equality; use MachineStatus::validate. Optional properties may be absent or null on the wire.");
    status["allOf"] = json!([
        {"if": {"properties": {"ready": {"const": true}}, "required": ["ready"]}, "then": {"properties": {"state": {"const": "running"}, "connection": reference("WorkerConnection")}, "required": ["connection"]}},
        {"if": {"properties": {"state": {"const": "running"}}, "required": ["state"]}, "else": {"properties": {"connection": {"type": "null"}}}},
        {"if": {"properties": {"state": {"const": "failed"}}, "required": ["state"]}, "then": {"properties": {"failure": reference("ProviderError")}, "required": ["failure"]}, "else": {"properties": {"failure": {"type": "null"}}}}
    ]);
    schemas.insert("MachineStatus".into(), status);
    schemas.insert("ProtocolRequest".into(), object(json!({"version": {"type":"integer", "const":2}, "allocation": reference("Allocation"), "handle": nullable("MachineHandle")}), &["version", "allocation"]));
    schemas.insert(
        "ProtocolResponse".into(),
        object(
            json!({"version": {"type":"integer", "const":2}, "status": reference("MachineStatus")}),
            &["version", "status"],
        ),
    );
    schemas.insert(
        "Intent".into(),
        json!({"type": "string", "enum": ["ensure", "start", "stop", "destroy"]}),
    );
    schemas.insert(
        "IsolationKind".into(),
        json!({"type":"string", "enum":["virtual_machine","container"]}),
    );
    schemas.insert("Capabilities".into(), object(json!({"isolation": reference("IsolationKind"), "cpu_limit": {"type":"boolean"}, "memory_limit": {"type":"boolean"}, "disk_limit": {"type":"boolean"}, "durable_workspace": {"type":"boolean"}, "stop_start": {"type": "boolean"}}), &["isolation", "cpu_limit", "memory_limit", "disk_limit", "durable_workspace", "stop_start"]));
    schemas.insert("Binding".into(), object(json!({"allocation": reference("Allocation"), "revision": integer(1, u64::MAX), "status": nullable("MachineStatus"), "pending": nullable("Intent"), "last_error": nullable("ProviderError"), "retired": {"type": "boolean"}}), &["allocation", "revision", "retired"]));

    let mut paths = Map::new();
    for action in RuntimeAction::ALL {
        let request = json!({"allOf": [reference("ProtocolRequest"), if action == RuntimeAction::Ensure {
            json!({"properties": {"handle": {"type": "null"}}})
        } else if action == RuntimeAction::Inspect {
            json!({})
        } else {
            json!({"properties": {"handle": reference("MachineHandle")}, "required": ["handle"]})
        }]});
        let success = json!({"description": "Identity-validated current machine status, including transitional states. Not an empty acceptance receipt.", "content": {"application/json": {"schema": reference("ProtocolResponse")}}});
        paths.insert(format!("/v2/runtime/{}", action.as_str()), json!({"post": {
            "operationId": format!("cloud.runtime.{}", action.as_str()), "tags": ["cloud-runtime"],
            "security": [{"RuntimeBearer": []}],
            "requestBody": {"required": true, "content": {"application/json": {"schema": request}}},
            "responses": {"200": success, "202": success,
                "401": {"description": "Unauthorized; no public error-body contract"},
                "403": {"description": "Forbidden tenant/workspace"},
                "404": {"description": "Missing is not proof of owned destruction; return an owned tombstone for idempotent destroy"},
                "409": {"description": "Immutable spec conflict, stale generation, or foreign machine"},
                "429": {"description": "Retryable rate limit"},
                "500": {"description": "Retryable bridge failure"}}
        }}));
    }
    json!({"openapi": "3.1.0", "info": {"title": "Neoism Cloud Runtime Bridge", "version": "2.0.0", "description": "Provider-neutral whole-machine runtime bridge v2. Native provisioning requires an authoritative durable ownership/generation/idempotency ledger. No invented vendor endpoint or default host."}, "paths": paths, "components": {"schemas": schemas, "securitySchemes": {"RuntimeBearer": {"type": "http", "scheme": "bearer", "description": "Coordinator credential supplied only through the Authorization header; never in a serialized DTO or worker connection."}}}})
}
