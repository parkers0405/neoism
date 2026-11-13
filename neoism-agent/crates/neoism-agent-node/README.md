# Native Node binding

This crate exports a Node-API addon for `neoism-agent-server::EmbeddedAgent`. It is a transport/lifecycle binding, not an agent implementation. The public TypeScript entry point is `createEmbeddedNeoism` in the existing `@neoism/sdk-node` package.

From `neoism-agent/sdk/typescript`, run `npm run native:build` and `npm run test:embedded`. The build script compiles a development cdylib and places it under `packages/node/native/<platform>-<arch>/neoism_agent.node`. Importing the SDK does not start a process, open a listener, or load native code; creating the host loads the addon. Platform artifacts are not downloaded automatically.

Binding ABI version 1 uses synchronous request admission/cancellation plus async request headers, pull-based body reads, and close. Admission precedes async dispatch to avoid losing an abort delivered before the Rust future starts. Bodies are read in chunks up to 64 KiB, requests are limited to 16 MiB, and each host admits at most 64 active exchanges. Native requests accept only origin-relative paths. Closed/cancelled exchanges release body streams and pending futures.

The addon uses the existing native/local services and authentication middleware. Native module code, local tools, process plugins, configuration, and credentials remain within the embedding application's authority. It is not a shared-tenant sandbox or a restricted-Worker/Wasm module. Tests must run in Node, which supplies the Node-API symbols; Rust lib test/doctest harnesses are disabled for this cdylib.