use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{
    LanguageAdapter, ResolvedLanguageRoute, ResolvedLspTransport, WorkspaceRootStrategy,
};

pub(super) fn apply_config(adapter: &mut LanguageAdapter, object: &Map<String, Value>) {
    if let Some(name) = object
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        adapter.name = name.to_string();
    }

    match configured_routes(adapter, object) {
        Ok(Some(routes)) => adapter.routes = routes,
        Ok(None) if adapter.routes.is_empty() => set_error(
            adapter,
            "must declare at least one route or top-level language plus extensions/filenamePatterns",
        ),
        Ok(None) => {}
        Err(error) => set_error(adapter, error),
    }
    let invalid_document_ids = adapter
        .routes
        .iter()
        .filter(|route| {
            route.document_language_id.trim().is_empty()
                || route.document_language_id.eq_ignore_ascii_case("plaintext")
        })
        .map(|route| route.id.clone())
        .collect::<Vec<_>>();
    if !invalid_document_ids.is_empty() {
        set_error(
            adapter,
            format!(
                "route(s) {} require an explicit documentLanguageId other than `plaintext`",
                invalid_document_ids.join(", ")
            ),
        );
    }

    if let Some(markers) = object.get("markers").or_else(|| object.get("rootMarkers")) {
        match string_array(markers, false) {
            Ok(markers) => adapter.markers = markers,
            Err(error) => set_error(adapter, format!("invalid root markers: {error}")),
        }
    }
    if let Some(strategy) = object
        .get("rootStrategy")
        .or_else(|| object.get("root_strategy"))
    {
        match configured_root_strategy(strategy) {
            Ok(strategy) => adapter.root_strategy = strategy,
            Err(error) => set_error(adapter, format!("invalid root strategy: {error}")),
        }
    }

    apply_capabilities(adapter, object.get("capabilities"));
    adapter.initialization_options = object
        .get("initializationOptions")
        .or_else(|| object.get("initialization_options"))
        .or_else(|| object.get("initialization"))
        .cloned()
        .or_else(|| adapter.initialization_options.clone());
    if let Some(settings) = object.get("settings") {
        adapter.settings = Some(settings.clone());
    }

    match configured_transport(adapter, object) {
        Ok(Some(transport)) => adapter.transport = transport,
        Ok(None) if matches!(adapter.transport, ResolvedLspTransport::Invalid) => {
            set_error(adapter, "must declare a stdio command or TCP endpoint")
        }
        Ok(None) => {}
        Err(error) => set_error(adapter, error),
    }
}

fn configured_root_strategy(value: &Value) -> Result<WorkspaceRootStrategy, String> {
    let (kind, manifest) = match value {
        Value::String(kind) => (kind.as_str(), None),
        Value::Object(object) => {
            let kind = object
                .get("kind")
                .or_else(|| object.get("type"))
                .or_else(|| object.get("strategy"))
                .and_then(Value::as_str)
                .ok_or_else(|| "object must declare `kind`".to_string())?;
            let manifest = object.get("manifest").and_then(Value::as_str);
            (kind, manifest)
        }
        _ => {
            return Err(
                "must be `nearestMarker`, `cargoMetadata`, or an object".to_string()
            )
        }
    };

    let normalized = kind
        .chars()
        .filter(|character| !matches!(character, '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect::<String>();
    match normalized.as_str() {
        "nearestmarker" => Ok(WorkspaceRootStrategy::NearestMarker),
        "cargometadata" => {
            let manifest = manifest.unwrap_or("Cargo.toml").trim();
            if manifest.is_empty() {
                return Err("cargo metadata `manifest` cannot be empty".to_string());
            }
            let path = std::path::Path::new(manifest);
            if path.is_absolute()
                || path.components().any(|component| {
                    !matches!(component, std::path::Component::Normal(_))
                })
            {
                return Err(
                    "cargo metadata `manifest` must be a relative path without `..`"
                        .to_string(),
                );
            }
            Ok(WorkspaceRootStrategy::CargoMetadata {
                manifest: manifest.to_string(),
            })
        }
        _ => Err(format!(
            "unknown strategy `{kind}`; expected `nearestMarker` or `cargoMetadata`"
        )),
    }
}

fn configured_routes(
    adapter: &LanguageAdapter,
    object: &Map<String, Value>,
) -> Result<Option<Vec<ResolvedLanguageRoute>>, String> {
    if let Some(routes) = object.get("routes") {
        let parsed = match routes {
            Value::Array(routes) => routes
                .iter()
                .enumerate()
                .map(|(index, route)| {
                    parse_route(None, route)
                        .map_err(|error| format!("route {}: {error}", index + 1))
                })
                .collect::<Result<Vec<_>, _>>()?,
            Value::Object(routes) => routes
                .iter()
                .map(|(id, route)| {
                    parse_route(Some(id), route)
                        .map_err(|error| format!("route `{id}`: {error}"))
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => return Err("`routes` must be an array or object".to_string()),
        };
        if parsed.is_empty() {
            return Err("`routes` cannot be empty".to_string());
        }
        return Ok(Some(parsed));
    }

    let has_route_matchers = ["extensions", "filenamePatterns", "filenames"]
        .iter()
        .any(|key| object.contains_key(*key));
    // `language`/`languageId` historically selected a built-in adapter. They
    // do not replace that adapter's complete route table unless matchers are
    // also supplied. New adapters, which have no inherited routes, still use
    // these fields to declare their first route.
    let has_top_level_route = has_route_matchers || adapter.routes.is_empty();
    if !has_top_level_route {
        return Ok(None);
    }
    let logical_id = object
        .get("language")
        .and_then(Value::as_str)
        .or_else(|| adapter.routes.first().map(|route| route.id.as_str()))
        .unwrap_or(adapter.id.as_str());
    let document_id = object
        .get("documentLanguageId")
        .or_else(|| object.get("document_language_id"))
        .or_else(|| object.get("languageId"))
        .and_then(Value::as_str)
        .unwrap_or(logical_id);
    let extensions = object
        .get("extensions")
        .map(|value| string_array(value, true))
        .transpose()?
        .unwrap_or_default();
    let filename_patterns = object
        .get("filenamePatterns")
        .or_else(|| object.get("filename_patterns"))
        .or_else(|| object.get("filenames"))
        .map(|value| string_array(value, false))
        .transpose()?
        .unwrap_or_default();
    validate_route(logical_id, document_id, extensions, filename_patterns)
        .map(|route| Some(vec![route]))
}

fn parse_route(
    map_id: Option<&str>,
    value: &Value,
) -> Result<ResolvedLanguageRoute, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "must be an object".to_string())?;
    let logical_id = object
        .get("id")
        .or_else(|| object.get("language"))
        .and_then(Value::as_str)
        .or(map_id)
        .ok_or_else(|| "missing logical `id`".to_string())?;
    let document_id = object
        .get("documentLanguageId")
        .or_else(|| object.get("document_language_id"))
        .or_else(|| object.get("languageId"))
        .and_then(Value::as_str)
        .unwrap_or(logical_id);
    let extensions = object
        .get("extensions")
        .map(|value| string_array(value, true))
        .transpose()?
        .unwrap_or_default();
    let filename_patterns = object
        .get("filenamePatterns")
        .or_else(|| object.get("filename_patterns"))
        .or_else(|| object.get("filenames"))
        .map(|value| string_array(value, false))
        .transpose()?
        .unwrap_or_default();
    validate_route(logical_id, document_id, extensions, filename_patterns)
}

fn validate_route(
    logical_id: &str,
    document_id: &str,
    extensions: Vec<String>,
    filename_patterns: Vec<String>,
) -> Result<ResolvedLanguageRoute, String> {
    let logical_id = logical_id.trim();
    let document_id = document_id.trim();
    if logical_id.is_empty() {
        return Err("logical id cannot be empty".to_string());
    }
    if extensions.is_empty() && filename_patterns.is_empty() {
        return Err("must declare `extensions` or `filenamePatterns`".to_string());
    }
    Ok(ResolvedLanguageRoute {
        id: logical_id.to_string(),
        document_language_id: document_id.to_string(),
        extensions,
        filename_patterns,
    })
}

fn configured_transport(
    adapter: &LanguageAdapter,
    object: &Map<String, Value>,
) -> Result<Option<ResolvedLspTransport>, String> {
    let transport_object = object.get("transport").and_then(Value::as_object);
    let transport_kind = object.get("transport").and_then(Value::as_str).or_else(|| {
        transport_object.and_then(|transport| {
            transport
                .get("kind")
                .or_else(|| transport.get("type"))
                .and_then(Value::as_str)
        })
    });
    let field = |name: &str| {
        transport_object
            .and_then(|transport| transport.get(name))
            .or_else(|| object.get(name))
    };
    let wants_tcp = transport_kind.is_some_and(|kind| kind.eq_ignore_ascii_case("tcp"))
        || field("endpoint").is_some()
        || field("host").is_some()
        || field("port").is_some();
    if wants_tcp {
        let fallback = match &adapter.transport {
            ResolvedLspTransport::Tcp { host, port, .. } => (host.clone(), *port),
            _ => ("127.0.0.1".to_string(), 0),
        };
        let (mut host, mut port) = fallback;
        if let Some(endpoint) = field("endpoint").and_then(Value::as_str) {
            let endpoint = endpoint
                .trim()
                .strip_prefix("tcp://")
                .unwrap_or(endpoint.trim());
            let (endpoint_host, endpoint_port) = endpoint
                .rsplit_once(':')
                .ok_or_else(|| "TCP `endpoint` must include host and port".to_string())?;
            host = endpoint_host.trim_matches(['[', ']']).to_string();
            port = endpoint_port
                .parse::<u16>()
                .map_err(|_| "TCP endpoint port must be 1-65535".to_string())?;
        }
        if let Some(configured_host) = field("host").and_then(Value::as_str) {
            host = configured_host.trim().to_string();
        }
        if let Some(configured_port) = field("port") {
            port = configured_port
                .as_u64()
                .and_then(|port| u16::try_from(port).ok())
                .or_else(|| {
                    configured_port
                        .as_str()
                        .and_then(|port| port.parse::<u16>().ok())
                })
                .ok_or_else(|| {
                    "TCP `port` must be an integer from 1-65535".to_string()
                })?;
        }
        if host.is_empty() || port == 0 {
            return Err(
                "TCP transport requires a non-empty host and non-zero port".to_string()
            );
        }
        return Ok(Some(ResolvedLspTransport::Tcp {
            host,
            port,
            built_in: false,
        }));
    }

    if transport_kind.is_some_and(|kind| !kind.eq_ignore_ascii_case("stdio")) {
        return Err(format!(
            "unsupported transport `{}`; expected `stdio` or `tcp`",
            transport_kind.unwrap_or_default()
        ));
    }
    let command = field("command").map(configured_command).transpose()?;
    let env = field("env")
        .map(configured_env)
        .transpose()?
        .unwrap_or_default();
    if let Some(command) = command {
        return Ok(Some(ResolvedLspTransport::Stdio { command, env }));
    }
    if !env.is_empty() {
        if let ResolvedLspTransport::Stdio { command, .. } = &adapter.transport {
            return Ok(Some(ResolvedLspTransport::Stdio {
                command: command.clone(),
                env,
            }));
        }
        return Err("stdio `env` requires a command".to_string());
    }
    Ok(None)
}

fn configured_command(value: &Value) -> Result<Vec<String>, String> {
    let command = if let Some(command) = value.as_str() {
        command
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>()
    } else if let Some(parts) = value.as_array() {
        parts
            .iter()
            .map(|part| {
                part.as_str()
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| {
                        "command entries must be non-empty strings".to_string()
                    })
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        return Err("stdio `command` must be a string or string array".to_string());
    };
    if command.is_empty() {
        return Err("stdio `command` cannot be empty".to_string());
    }
    Ok(command)
}

fn configured_env(value: &Value) -> Result<BTreeMap<String, String>, String> {
    value
        .as_object()
        .ok_or_else(|| "stdio `env` must be an object".to_string())?
        .iter()
        .map(|(key, value)| {
            value
                .as_str()
                .map(|value| (key.clone(), value.to_string()))
                .ok_or_else(|| format!("environment variable `{key}` must be a string"))
        })
        .collect()
}

fn string_array(
    value: &Value,
    normalize_extensions: bool,
) -> Result<Vec<String>, String> {
    let values = value
        .as_array()
        .ok_or_else(|| "must be an array of strings".to_string())?;
    values
        .iter()
        .map(|value| {
            let value = value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "entries must be non-empty strings".to_string())?;
            Ok(if normalize_extensions {
                value.trim_start_matches('.').to_ascii_lowercase()
            } else {
                value.to_string()
            })
        })
        .collect()
}

fn apply_capabilities(adapter: &mut LanguageAdapter, value: Option<&Value>) {
    let Some(capabilities) = value.and_then(Value::as_object) else {
        return;
    };
    let enabled = |camel: &str, snake: &str, current: bool| {
        capabilities
            .get(camel)
            .or_else(|| capabilities.get(snake))
            .and_then(Value::as_bool)
            .unwrap_or(current)
    };
    adapter.workspace_symbols = enabled(
        "workspaceSymbols",
        "workspace_symbols",
        adapter.workspace_symbols,
    );
    adapter.completion = enabled("completion", "completion", adapter.completion);
    adapter.hover = enabled("hover", "hover", adapter.hover);
    adapter.definition = enabled("definition", "definition", adapter.definition);
    adapter.references = enabled("references", "references", adapter.references);
    adapter.implementation =
        enabled("implementation", "implementation", adapter.implementation);
    adapter.call_hierarchy =
        enabled("callHierarchy", "call_hierarchy", adapter.call_hierarchy);
    adapter.diagnostics = enabled("diagnostics", "diagnostics", adapter.diagnostics);
    adapter.document_symbols = enabled(
        "documentSymbols",
        "document_symbols",
        adapter.document_symbols,
    );
    adapter.formatting = enabled("formatting", "formatting", adapter.formatting);
    adapter.code_actions = enabled("codeActions", "code_actions", adapter.code_actions);
    adapter.rename = enabled("rename", "rename", adapter.rename);
}

fn set_error(adapter: &mut LanguageAdapter, error: impl Into<String>) {
    let error = error.into();
    adapter.configuration_error = Some(match adapter.configuration_error.take() {
        Some(existing) => format!("{existing}; {error}"),
        None => format!("invalid LSP adapter `{}`: {error}", adapter.id),
    });
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::lsp::lsp_languages::LANGUAGE_SPECS;

    fn rust_adapter() -> LanguageAdapter {
        LanguageAdapter::from_builtin(
            LANGUAGE_SPECS
                .iter()
                .find(|spec| spec.id == "rust")
                .expect("built-in Rust adapter"),
        )
    }

    #[test]
    fn configured_adapter_inherits_builtin_root_strategy() {
        let mut adapter = rust_adapter();
        apply_config(
            &mut adapter,
            json!({ "name": "Configured Rust" })
                .as_object()
                .expect("object"),
        );

        assert_eq!(
            adapter.root_strategy,
            WorkspaceRootStrategy::CargoMetadata {
                manifest: "Cargo.toml".to_string()
            }
        );
    }

    #[test]
    fn configured_adapter_can_override_root_strategy() {
        let mut adapter = rust_adapter();
        apply_config(
            &mut adapter,
            json!({ "rootStrategy": "nearestMarker" })
                .as_object()
                .expect("object"),
        );
        assert_eq!(adapter.root_strategy, WorkspaceRootStrategy::NearestMarker);

        apply_config(
            &mut adapter,
            json!({
                "rootStrategy": {
                    "kind": "cargoMetadata",
                    "manifest": "workspace/Cargo.toml"
                }
            })
            .as_object()
            .expect("object"),
        );
        assert_eq!(
            adapter.root_strategy,
            WorkspaceRootStrategy::CargoMetadata {
                manifest: "workspace/Cargo.toml".to_string()
            }
        );
    }
}
