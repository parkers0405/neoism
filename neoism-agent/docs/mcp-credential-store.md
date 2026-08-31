# MCP credential-store adapter

`AgentServices::with_mcp_credentials` is the only supported hosted MCP secret-storage boundary. The default `LocalMcpCredentialStore` preserves the standalone `mcp-auth.json` path and legacy decode, but reports `supports_hosted_scopes() == false`; hosted requests therefore fail closed rather than sharing a process-global file.

An external adapter (for example, a host-owned Synapse/Supabase adapter) implements `McpCredentialStore` from `neoism-agent-service-api` and is injected when constructing `AgentServices`. The Agent crates do not depend on that product or database.

## Storage contract

- Partition every operation by the complete `CredentialScope` (`tenant_id` plus optional `workspace_id`).
- Within that partition, key credentials by both `McpConnectionRef.connection_id` and the exact `server_url`.
- Encrypt `McpCredential` and `McpOAuthAttempt.code_verifier` at rest. Never return these records from an HTTP/plugin route or include them in logs. Their `Debug` implementations redact secrets, but adapters must also avoid logging serialized records.
- Make `put`, `delete`, and OAuth-attempt operations durable before resolving their futures.
- Implement `consume_attempt` and `consume_connection_attempt` as atomic read-delete transactions. A callback must succeed at most once. Delete expired attempts while consuming them and periodically purge abandoned attempts.
- Treat an explicit scope/connection miss as final. Never fall back to another tenant, workspace, connection, URL, or a process-global credential.
- Return `supports_hosted_scopes() == true` only when those isolation guarantees are enforced by the backing store.

OAuth attempts are durable through the same interface, so a callback can complete after an Agent process restart. Browser GET callbacks identify an attempt by the unguessable OAuth `state`; authenticated callbacks additionally enforce the caller scope. The legacy POST callback has no state parameter and therefore consumes the one pending attempt for the exact scope and connection.

No SDK or public MCP response contains `McpCredential`, registration secrets, refresh tokens, access tokens, or PKCE verifiers.