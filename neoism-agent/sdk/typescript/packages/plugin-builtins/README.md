# @neoism/sdk-plugin-builtins

Typed capability clients for Neoism Agent Server's optional built-in plugins.

Applications should install `@neoism/sdk`, which re-exports these clients and
the core HTTP SDK from one package:

```sh
npm install @neoism/sdk
```

Each client is a `PluginSdk` and must be bound with
`client.plugins.use()` or `client.plugins.tryUse()` so disabled plugins are
handled explicitly.

The `workflows` client includes definition administration (`create`, `update`,
`patch`, `remove`) plus run lookup/retry. Definition mutations require the
server management capability and authenticated local credentials; pass the
returned `revision` back on update/delete to avoid lost writes.