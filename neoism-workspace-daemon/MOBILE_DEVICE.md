# Pair a phone with the local daemon

The standalone daemon's phone listener is **off by default**. Use a private Tailscale node address and the certificate/key for its MagicDNS name (for example, provision them with `tailscale cert`). Set the following variables before starting the daemon. Do not enable Tailscale Funnel or forward the main daemon listener.

| Variable | Example / meaning |
| --- | --- |
| `NEOISM_MOBILE_LISTEN` | `100.64.1.2:8443` (explicit Tailscale or loopback address) |
| `NEOISM_MOBILE_ORIGIN` | `https://my-host.example.ts.net:8443` (advertised HTTPS origin, matching the certificate) |
| `NEOISM_MOBILE_TLS_CERT` | Path to the PEM certificate chain |
| `NEOISM_MOBILE_TLS_KEY` | Path to the PEM private key |
| `NEOISM_MOBILE_TLS_CA` | Optional private CA certificate in PEM format. The public CA certificate is included in the QR so clients can trust this host without installing a system-wide CA. The server certificate must match the advertised hostname or IP. |
| `NEOISM_MOBILE_OPERATOR_KEY` | 32 random bytes encoded as unpadded base64url; share only with the local operator UI |

Use tailnet ACLs to restrict access. The phone listener serves TLS directly, does not trust forwarded transport or peer headers, and never mounts the main daemon router. It uses the local loopback Agent service. Fast Server, temporary hosts and persistent hosted workspace processes do not enable this feature.

## HTTP and WebSocket

| Listener | Request | Success |
| --- | --- | --- |
| Local daemon (loopback or Unix socket) | `POST /mobile-device/invite`, operator bearer, `{"password":"…"}` | `200 {"invite":"neoism://device?origin=…&code=…&name=…"}` |
| Phone TLS origin | `POST /mobile-device/claim`, `{"code":"…","device_label":"…","password":"…"}` | `200 {"token":"…"}` |
| Phone TLS origin | `GET /mobile-device/session`, WebSocket upgrade with `Authorization: Bearer <token>` | `101`, then Workspace `Hello` / `HelloAck` |
| Local daemon (loopback or Unix socket) | `GET /mobile-device/devices`, operator bearer | `200 [{"device_id":"…","device_label":"…"}]` |
| Local daemon (loopback or Unix socket) | `DELETE /mobile-device/devices/<device_id>`, operator bearer | `204`; active sessions for that device disconnect |

Only the invite code goes in the QR; enter the password on the phone. An invite expires in 60 seconds, permits five incorrect password attempts, and can succeed once. A bad password, expired invite, or replay returns `401`. Registry state is in memory: restarting the daemon invalidates all invites and phone tokens. Use the operator routes to revoke one device without restarting. The operator key and phone bearer are separate credentials; the WebSocket does not accept tokens in URLs.

## Rune bootstrap

Send `{"Workspace":{"request_id":1,"message":{"Hello":{"token":null,"client_name":"neoism-rune"}}}}`. On an accepted `HelloAck` with `workspace_lifecycle:false`, mark chat ready without creating, joining, or switching a daemon workspace. Start with `ListSessionProjects` (also starts the all-project live catalog), then `ListProjectSessions` for the selected directory. Use `SwitchThread` or `ResumeStream` and `GetHistory` when opening a chat; use `StopStream` to release a subscription. On `ThreadCreated`, bind the new stream explicitly. Agent replies use the existing `AgentReply` envelope; asynchronous messages use outer `request_id:0` and existing payload correlation. Workspace `Ping` echoes its request ID.

`ListProviders` and `ListModelCatalog` return host models. `ConnectListProviders` returns provider IDs/names and connected IDs, with no auth methods. `ConnectListConnections` returns only provider/account IDs, label, authentication type and default status, never credentials. Keep the mobile composer's selection locally and send it as `model`, `connection_id`, and `thinking` on `CreateThread` or `SubmitPrompt`, not as standalone settings mutations. The daemon validates the selected model against the configured host catalog, the reasoning level against the model's declared variants (or standard levels for reasoning-capable models without a variant list), and the connection ID against that provider's existing connections. Omit `directory` on `CreateThread` to use the daemon user's home on the host (macOS, Windows, or Linux). An explicit directory must be in that host's chat catalog. Selecting reasoning or an account on a new thread requires an explicit model. For an existing chat, a reasoning or account override may use its saved model.

`SubmitPrompt` accepts up to four inline photos (PNG, JPEG, WebP, GIF, or BMP), at most 4 MiB of raw attachment bytes per prompt. Each attachment carries its MIME in `kind`, a JSON byte array in `bytes`, and optionally a display filename (not a path or URL) in `path`. The mobile WebSocket caps frames and messages at 17 MiB to accommodate JSON byte arrays. The daemon checks the photo type and forwards it to the Agent service as a `data:` image part. The selected model must support image input.

This is a chat client, not an Agent sandbox: prompts run under the host session's existing permissions. Direct file, PTY, generic agent proxy, configuration, collaboration, slash-command and provider-auth operations are not exposed. Mobile clients cannot select agent modes.