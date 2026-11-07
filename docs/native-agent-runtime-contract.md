# Native agent runtime implementation contract

Codex uses its installed `app-server` stdio API, Claude Code uses the Claude Agent SDK, and OpenCode uses its installed native HTTP/SSE server. These are native integrations, not ACP shims. Rust owns Neoism sessions, inherited policy, task orchestration, normalized transcript storage, permission/question interaction, cancellation, and process lifetime. ACP remains a separate legacy/native-ACP driver.

## Internal Node host

The implementation lives in `neoism-agent/crates/neoism-agent-server/src/external_agent/native_runtime/`. Node is used for SDK interoperability and native JSON/HTTP adapters, not as a second orchestrator. `host.mjs` speaks private JSON-RPC 2.0 NDJSON with Rust; this is not ACP. Adapter files export `createAdapter(context)` and have no top-level process startup.

Context fields are `cwd`, `executable` (absolute host Codex/Claude/OpenCode executable, nullable only for SDK-bundled Claude), `sdk` (injected Claude SDK module), `emit(event)` (runtime metadata only), and `request(method, params)` (Rust-owned interactions). Never read Neoism API credentials or forward them to providers. Preserve the host environment; use literal argument arrays and bind native servers to loopback with private authentication.

Each adapter returns these async methods:

- `describe({selection?})` returns `{capabilities, models, options, commands, auth}` without sending a model turn or creating a native conversation. An optional requested model narrows discovered model-specific options; it is not an effective selection.
- `open({sessionId?, selection?})` creates or resumes the native conversation and returns `{sessionId, model?, options?, capabilities?}`. Claude may return `sessionId:null` for a new SDK conversation whose real ID is assigned on the first prompt; it must then emit `conversation.updated` before persisting an identity. A failed requested continuation must not silently create a new session.
- `start({sessionId, text, selection?, emit, request})` sends a turn, emits normalized events through the provided turn-bound callbacks, and resolves `{finish, model?, usage?}` only after its terminal events. The callbacks supplied here retain their operation identity; never redirect late callbacks to a later turn.
- `interrupt({sessionId})` requests and confirms interruption where the native API permits it.
- `steer({sessionId, text})` is optional and must be advertised only when implemented.
- `configure({sessionId, selection})` applies provider-confirmed model/options and returns effective selection. Unsupported choices fail rather than silently defaulting.
- `list({cwd, cursor?})` returns `{sessions:[{sessionId,cwd,title?,updatedAt?}],nextCursor?}`. Do not claim workspace ownership without a provider-confirmed cwd.
- `history({sessionId, cwd})` returns `{sessionId,cwd,messages:[{id,role,events:[]}],complete:boolean}`. Events use the normalized vocabulary below; importing history must not execute tools or replay a prompt.
- `close()` closes SDK streams/native connections and owned subprocesses. It is idempotent.

Selection is `{model?:string, options?:Record<string,string|boolean|number>}`. Models are `{id,name?,contextWindow?}`. Options are `{id,name,type:"select"|"boolean",category?,currentValue,options?:[{value,name}]}`. Capability fields include `resume`, `history`, `list`, `modelSelection`, `steer`, `toolStreaming`, `reasoning`, and `nativeChildren` (`"none"|"observed"|"controllable"`). Unsupported operations return explicit errors; capability declarations must describe actual implemented behavior.

## Normalized events

Every event has `type` and optional `sessionId`, `turnId`, `scope`. Root scope may be omitted. Native child scope is `{id,parentId?,parentToolCallId?,agent?,title?,sessionId?}`; its ID is provider-owned, never a fabricated resumable session ID.

| Type | Fields |
| --- | --- |
| `conversation.updated` | Confirmed `sessionId`, optional `model`, `selection` |
| `text.delta` | `id`, `text` |
| `text.completed` | `id`, optional full `text` |
| `reasoning.delta` | `id`, `text` |
| `reasoning.completed` | `id`, optional full `text` |
| `tool.started` | `id`, `name`, `title?`, `input`, `kind?` |
| `tool.updated` | `id`, optional `name`, `title`, `input`, `kind`, `metadata` |
| `tool.output` | `id`, `text`, `mode:"append"|"replace"`, optional `stream:"stdout"|"stderr"` |
| `tool.completed` | `id`, optional `input`, `output`, `outputMode`, `metadata`, `exitCode` |
| `tool.failed` | `id`, `error`, optional `output`, `metadata` |
| `plan.updated` | `entries:[{content,status,priority?}]` |
| `usage.updated` | `usage:{input,output,reasoning?,cacheRead?,cacheWrite?,total?,contextWindow?}` |
| `model.updated` | `model`, optional `contextWindow` |
| `child.updated` | `scope`, `status:"running"|"completed"|"error"|"interrupted"`, optional `summary` |
| `runtime.status` | `status`, optional `message` |

Emit tool starts before output, preserve sparse updates, and distinguish initial output snapshots from later deltas. In particular, Codex `item/started.item.aggregatedOutput` is an initial snapshot and must not be discarded; later native `outputDelta` bytes append. Do not duplicate initial bytes in the final aggregate. Do not resurrect terminal tools or report unconfirmed cancellation as stopped.

## Interaction requests

`request("permission.request", {id,toolCallId?,toolName,title?,input,options?})` resolves `{decision:"allow_once"|"allow_session"|"deny"}`. Rust checks the current child policy and only asks the user when needed. An approval is not an OS sandbox guarantee.

Codex app-server approvals do not intercept every native auto-approved tool. The driver refuses inherited denials that its native sandbox cannot enforce instead of claiming a full tool-policy sandbox. Blanket edit/write denials use read-only mode without approval-based escalation; unsupported path-specific or tool-specific denials fail before native startup. Claude uses a pre-tool hook to force Rust decisions; OpenCode starts owned sessions with ask rules.

`request("question.request", {id,toolCallId?,questions:[{id,question,header?,options?:[{label,description?}],multiple?,custom?}]})` resolves `{answers:Record<string,string[]>}` or throws cancellation. Do not auto-answer questions, broaden permission modes, or silently pass native authentication failures through another provider.

## Host RPC methods

`runtime.initialize` selects provider, cwd, executable and policy, then creates the adapter. Remaining methods are `runtime.describe`, `conversation.open`, `turn.start`, `turn.interrupt`, `turn.steer`, `selection.apply`, `conversation.list`, `conversation.history`, and `runtime.close`. Turn events are notifications `runtime.event` with `{operationId,event}`. Interaction requests carry that operation ID as well. Errors preserve a readable message and optional native code/details.

## Ownership and compatibility

New sessions persist driver identities `codex-app-server`, `claude-agent-sdk`, and `opencode-server` independently of provider/model. Existing `runtime:"acp"` sessions retain their legacy driver; they are not silently rebound to a different protocol or conversation. Native session IDs are persisted only after provider confirmation. New direct external roots and delegated children use the same driver/session runner. Existing parent completion delivery and sidebar/timeline branding are reused.

Rust's native facade owns `options::session_lock` for managed sessions; native route branches must occur before acquiring the legacy ACP gate. Native selection may be validated and scheduled for the next turn: `selection.apply` then returns `applied:"next-turn"`, not a fabricated currently-effective model.

Option previews cache only discovery and validated pending choices, never create provider conversations, and never claim a pending choice is effective. History discovery and complete import are separate capabilities: Claude advertises `completeHistory:false`, so its partial SDK replay is not offered as a complete import. Failed imports remove their hidden importing root so retries are possible.

Verification must exercise real transcript data, initial snapshots plus deltas, tool outputs, sparse completion, approvals/questions, explicit continuation, cancellation, and native history. No-tools responses and painted logos alone do not establish interoperability.