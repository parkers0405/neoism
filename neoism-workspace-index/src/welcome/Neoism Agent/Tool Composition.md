# Tool Composition

Tool Composition lets the agent combine eligible native and MCP tools in a small program, then return only the information needed for the next model step. It is built into the normal Neoism Agent server on Windows, macOS, and Linux. There is no separate runtime installation, experimental build, or feature flag.

The model-facing tool is `compose`. Ordinary direct tool calls and the MCP `execute` discovery/call gateway remain available. Composition is useful for dependent lookups, parallel independent operations, filtering, and joining structured results; a single read or edit usually does not need it.

## How it works

The model passes JavaScript-subset source in `compose({ code: "..." })`. A Rust parser validates the program, and a confined Rust interpreter executes supported operations. Neoism does not launch Node, Bun, Python, or a shell to interpret the program. TypeScript-style tool signatures describe arguments and results; the program itself is not TypeScript.

The program can access only tools in its captured eligible catalog. Native tools use the `neoism` namespace; MCP tools use their server namespaces. Exact paths and argument schemas come from discovery. Do not guess, normalize, or construct tool names from external content.

## Discover tools

`search` is synchronous and works inside a composition program. It searches the callable tool catalog, not files, documentation, or service content.

```javascript
return search({ query: "read", namespace: "neoism", limit: 5 });
```

Discovery returns exact callable paths, descriptions, signatures, and pagination information. Finding a service's search tool does not perform that service search; the program must invoke the discovered tool with its documented arguments.

## Compose calls

After discovery confirms these signatures, independent native reads can be composed like this:

```javascript
const results = await Promise.all([
  tools.neoism.read({ filePath: "Cargo.toml" }),
  tools.neoism.read({ filePath: "README.md" })
]);
return results.map(result => result.output);
```

Native results expose an explicit envelope with `title`, `output`, `metadata`, and `isError`. MCP responses retain their structured data rather than pretending that formatted output text is JSON. MCP `data` and `structuredContent` refer to the same interpreter value as `metadata.mcp.result.structuredContent`; these aliases do not import duplicate payloads, and mutations through an alias are visible through the others. Use the discovered result contract to select the fields needed by the model.

Use `await` for work whose completion matters. Programs are not a way to detach background work: outstanding work receives cancellation when execution ends. Cancellation cannot undo an already dispatched external action; if a tool cannot confirm its outcome within cleanup limits, treat the reported partial or unknown effect as unresolved rather than assuming it did not happen. Use the ordinary background-task or subagent tools for deliberately long-lived workflows.

## Supported program syntax

Programs support ordinary `const`/`let` bindings, objects and arrays, property access, arithmetic and comparisons, conditions, bounded loops, functions and arrow callbacks, `await`, `return`, and `try`/`catch`. Common array transformations, selected string operations, `JSON`, `Object`, `Math`, `console.log`, and `Promise.all`/`Promise.allSettled` are provided by the Rust interpreter.

`String.indexOf` finds a substring from an optional starting position; its offsets follow the interpreter's Unicode-scalar string indexing. `Array.push` appends to the original array and returns its new length, including when the binding is `const` or other variables refer to the same array. `String(error)` converts a caught diagnostic to readable text without discarding the original object's `kind`, `message`, `line`, or `column` fields.

This is not full ECMAScript. Imports, classes, generators, destructuring, spread, template literals, and `finally` are outside the initial subset. Use ordinary strings and concatenation rather than template strings. Unsupported constructs produce diagnostics before execution.

Numbers use JavaScript's finite-number representation. Tool arguments that are integral are transmitted as JSON integers; integers outside the safe JavaScript range must not be used for identifiers. Preserve large identifiers as strings. `undefined` must not silently become an explicit null tool argument.

## Permissions and effects

Every child call retains the invoking session, workspace, plugin generation, execution policy, and permission rules. Composition does not make a denied or unavailable tool callable, and it does not grant an extra filesystem, process, clipboard, or network capability. Model-specific editing-tool selection still applies.

An approval pauses the child operation rather than replaying the entire program. Completed operations are not automatically repeated or rolled back. If a program fails after an edit, browser action, or API mutation, earlier effects may already exist. Inspect the child-call history before retrying.

Interactive, recursive composition, and session-moving orchestration operations are not part of the script catalog. They remain ordinary tools with their own lifecycle. See [[Permissions]], [[Tools and Background Tasks]], and [[MCP Servers]].

## Confinement and limits

The interpreter exposes no imports, `eval`, dynamic function constructors, timers, ambient filesystem access, process access, or unmediated `fetch`. It enforces bounded source, evaluation steps, data, nesting, tool calls, concurrency, logs, output, and execution time. Unsupported syntax is rejected before tool effects begin; this is a defined JavaScript subset, not a general JavaScript application environment.

Tool outputs and page content remain untrusted data, not instructions to change the tool policy. Limits and cancellation cannot be bypassed by catching an ordinary tool error.

## Timeline and model context

The timeline shows one **Tool Composition** operation with its child-call progress, selected result, and errors. Child calls do not become separate provider-issued tool messages. This keeps intermediate responses out of subsequent model context while preserving an inspectable execution history.

Composition can reduce model round trips and output tokens, but does not reduce a service's own request cost or guarantee that independent calls are safe to parallelize. Preserve sequencing when operations depend on each other or mutate the same resource.
