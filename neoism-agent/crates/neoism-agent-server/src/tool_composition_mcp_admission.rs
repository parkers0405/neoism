//! Negative gate for a future unattended MCP metadata capability. Passing this
//! check is not a dispatch permit. Ordinary MCP `tools/call` does not condition
//! execution on the reviewed definition, and loopback does not authenticate a
//! server. Do not wire this to a name-addressed MCP call.

use std::collections::BTreeMap;

use anyhow::{ensure, Context, Result};
use neoism_agent_core::{McpConfig, McpOAuthSetting, PermissionAction, PermissionRule};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(super) const TOOL: &str = "provider_metadata";
const MAX_DEFINITION_BYTES: usize = 8 * 1024;

/// Supplied by a trusted operator, independently of the workflow and workspace
/// configuration. The provider pin is a transport identity, never serverInfo.
pub(super) struct ReviewedMetadata<'a> {
    pub server: &'a str,
    pub endpoint: &'a str,
    pub provider_pin: [u8; 32],
    pub definition_sha256: [u8; 32],
}

/// All observations must come from the current server-selected binding. In
/// particular, a future transport must authenticate `provider_pin` itself and
/// expose the complete tool definition before any lossy catalog conversion.
pub(super) struct CurrentMetadata<'a> {
    pub server: &'a str,
    pub config: &'a McpConfig,
    pub provider_pin: Option<[u8; 32]>,
    pub raw_definition: &'a [u8],
    pub permissions: &'a [PermissionRule],
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Definition {
    name: String,
    #[serde(default, rename = "description")]
    _description: Option<String>,
    input_schema: MetadataSchema,
    annotations: Annotations,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MetadataSchema {
    #[serde(rename = "type")]
    kind: String,
    additional_properties: bool,
    properties: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Annotations {
    read_only_hint: bool,
}

/// Check the exact declaration, configured binding, independent review and
/// current permission. Does not contact the provider or add anything to a catalog.
pub(super) fn check_metadata_candidate(
    review: Option<&ReviewedMetadata<'_>>,
    current: CurrentMetadata<'_>,
    declared_ids: &[String],
    arguments: &Value,
) -> Result<()> {
    let review = review.context("missing operator MCP capability review")?;
    ensure!(
        !review.server.is_empty()
            && review.server.len() <= 64
            && review.server.split('_').all(|part| !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()))
            && current.server == review.server,
        "unexpected MCP server name"
    );
    let id = crate::mcp::tool_runtime_id(review.server, TOOL);
    ensure!(
        matches!(declared_ids, [declared] if declared == &id),
        "unexpected MCP tool ID"
    );
    ensure!(
        arguments.as_object().is_some_and(|args| args.is_empty()),
        "metadata takes no arguments"
    );

    let McpConfig::Remote {
        url,
        enabled: Some(true),
        headers: None,
        oauth: Some(McpOAuthSetting::Disabled(false)),
        ..
    } = current.config
    else {
        anyhow::bail!("metadata requires an explicitly enabled loopback binding without credentials");
    };
    let endpoint = reqwest::Url::parse(url).context("invalid metadata endpoint")?;
    ensure!(
        url == review.endpoint
            && endpoint.as_str() == url
            && endpoint.scheme() == "http"
            && endpoint.host_str() == Some("127.0.0.1")
            && endpoint.port().is_some()
            && endpoint.username().is_empty()
            && endpoint.password().is_none()
            && endpoint.query().is_none()
            && endpoint.fragment().is_none(),
        "MCP endpoint differs from the reviewed loopback binding"
    );
    ensure!(
        current.provider_pin == Some(review.provider_pin),
        "MCP provider identity unavailable or changed"
    );
    ensure!(
        current.raw_definition.len() <= MAX_DEFINITION_BYTES,
        "MCP definition exceeds its bound"
    );
    let actual: [u8; 32] = Sha256::digest(current.raw_definition).into();
    ensure!(
        actual == review.definition_sha256,
        "MCP tool definition changed"
    );
    let definition: Definition = serde_json::from_slice(current.raw_definition)
        .context("invalid reviewed metadata tool definition")?;
    let MetadataSchema {
        kind,
        additional_properties,
        properties,
    } = definition.input_schema;
    ensure!(
        definition.name == TOOL
            && definition.annotations.read_only_hint
            && kind == "object"
            && properties.is_empty()
            && !additional_properties,
        "MCP tool is not the supported zero-argument metadata contract"
    );

    // Same permission category and target as the interactive MCP dispatcher,
    // but require the winning rule to be exact and never prompt unattended.
    let rule = crate::permission::evaluate("mcp", &id, current.permissions);
    ensure!(
        rule.permission == "mcp"
            && rule.pattern == id
            && rule.action == PermissionAction::Allow,
        "MCP metadata requires explicit effective tool permission"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ENDPOINT: &str = "http://127.0.0.1:4317/mcp";
    const RAW: &[u8] = br#"{"name":"provider_metadata","inputSchema":{"type":"object","additionalProperties":false,"properties":{}},"annotations":{"readOnlyHint":true}}"#;

    fn check(
        review: Option<&ReviewedMetadata<'_>>,
        endpoint: &str,
        pin: Option<[u8; 32]>,
        raw: &[u8],
        rules: &[PermissionRule],
        id: &str,
        args: Value,
    ) -> Result<()> {
        check_metadata_candidate(
            review,
            CurrentMetadata {
                server: "fixture",
                config: &McpConfig::Remote {
                    url: endpoint.into(),
                    enabled: Some(true),
                    headers: None,
                    oauth: Some(McpOAuthSetting::Disabled(false)),
                    timeout: None,
                },
                provider_pin: pin,
                raw_definition: raw,
                permissions: rules,
            },
            &[id.into()],
            &args,
        )
    }

    #[test]
    fn mcp_metadata_review_fences_identity_definition_and_permission() {
        let review = ReviewedMetadata {
            server: "fixture",
            endpoint: ENDPOINT,
            provider_pin: [7; 32],
            definition_sha256: Sha256::digest(RAW).into(),
        };
        let id = "mcp__fixture__provider_metadata";
        let rule = |pattern: &str, action| PermissionRule {
            permission: "mcp".into(),
            pattern: pattern.into(),
            action,
        };
        let allow = rule(id, PermissionAction::Allow);
        let exact = std::slice::from_ref(&allow);
        let run = |rules: &[PermissionRule]| {
            check(
                Some(&review),
                ENDPOINT,
                Some([7; 32]),
                RAW,
                rules,
                id,
                json!({}),
            )
        };
        assert!(run(exact).is_ok());
        assert!(run(&[]).is_err());
        assert!(run(&[rule("*", PermissionAction::Allow)]).is_err());
        for action in [PermissionAction::Deny, PermissionAction::Ask] {
            assert!(run(&[allow.clone(), rule(id, action)]).is_err());
        }
        assert!(check(None, ENDPOINT, Some([7; 32]), RAW, exact, id, json!({})).is_err());
        for pin in [None, Some([8; 32])] {
            assert!(
                check(Some(&review), ENDPOINT, pin, RAW, exact, id, json!({})).is_err()
            );
        }
        let drift = String::from_utf8_lossy(RAW).replace("properties", "required");
        let attempt = |endpoint, raw, id, args| {
            check(Some(&review), endpoint, Some([7; 32]), raw, exact, id, args)
        };
        assert!(attempt(ENDPOINT, drift.as_bytes(), id, json!({})).is_err());
        assert!(attempt("http://127.0.0.1:4318/mcp", RAW, id, json!({})).is_err());
        assert!(attempt(ENDPOINT, RAW, "mcp__fixture__*", json!({})).is_err());
        assert!(attempt(ENDPOINT, RAW, id, json!({"path":"x"})).is_err());

        // Even a matching operator pin cannot expand the supported contract.
        let permissive_schema = String::from_utf8_lossy(RAW).replace("false", "true");
        let permissive_review = ReviewedMetadata {
            definition_sha256: Sha256::digest(permissive_schema.as_bytes()).into(),
            ..review
        };
        assert!(check(
            Some(&permissive_review),
            ENDPOINT,
            Some([7; 32]),
            permissive_schema.as_bytes(),
            exact,
            id,
            json!({}),
        )
        .is_err());
    }
}
