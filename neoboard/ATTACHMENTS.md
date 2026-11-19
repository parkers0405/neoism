# Attachments

Attachments are opaque, download-only SQLite BLOBs attached to an existing message. Every operation authenticates a current, unexpired, non-revoked board credential and authorizes the message's channel. Any credential permitted in that channel may upload or read attachments; an admin credential alone is not a board credential. Hidden messages/attachments and missing messages/attachments return 404. The server never accepts a filesystem path or reads/writes an attachment file on disk.

## Desktop/client contract

`neochat::protocol::Attachment { id: u64, message_id: u64, name: String, size: u64 }` derives Clone, Debug, Serialize, Deserialize, PartialEq and Eq. The blocking client exposes exactly these methods:

```rust
Client::attach(&self, id: u64, name: &str, bytes: Vec<u8>) -> Result<Attachment, ClientError>
Client::attachments(&self, id: u64) -> Result<Vec<Attachment>, ClientError>
Client::download_attachment(&self, id: u64) -> Result<Vec<u8>, ClientError>
```

For attach/list, `id` is the message ID; for download, it is the attachment ID. Successful uploads atomically emit an existing `EventKind::Message` upsert for that message. Consumers should refresh the attachment list on this event, and fetch it when opening a message. There are no new event variants or fields on Message/Snapshot. Upload retries are not idempotent: each accepted upload creates an attachment. Deletion is not implemented.

## Wire contract and caps

- `POST /v1/messages/{message_id}/attachments?name=<URL-encoded filename>` accepts a raw byte body and returns Attachment JSON.
- `GET /v1/messages/{message_id}/attachments` returns an ID-ordered Attachment JSON array.
- `GET /v1/attachments/{attachment_id}` returns raw bytes, never inline content: `Content-Type: application/octet-stream`, `Content-Disposition: attachment`, `X-Content-Type-Options: nosniff`, `Cache-Control: no-store`, and `Content-Security-Policy: sandbox; default-src 'none'`.
- Each attachment is at most 8 MiB (8,388,608 bytes); each message allows at most 16 attachments totaling at most 32 MiB. Empty files are permitted and count toward the count quota. Caps are checked transactionally. HTTP request bodies are bounded as well. Oversized uploads and message quotas return 413.
- Names are 1–255 UTF-8 bytes, without surrounding whitespace, separators, colons, control characters, bidi override/isolate characters, or standalone `.`/`..`. They are metadata only, not download destinations. Invalid names return 400. Executable/HTML bytes remain opaque: clients must never render, open, or execute them automatically.

The daemon stores metadata and bytes in the same SQLite transaction as the refresh event, with foreign-key linkage to the message. No MIME inference or executable preview is provided. Limits are per-message, not global disk quotas; deployments must separately provision/limit database storage.

## CLI

```text
neochat attach MESSAGE_ID LOCAL_FILE [--name DISPLAY_NAME]
neochat attachments MESSAGE_ID
neochat download-attachment ATTACHMENT_ID EXPLICIT_OUTPUT_FILE
```

Attach and list support `--json`. Download requires an explicit user-selected output path, never uses the server-provided name, never overwrites an existing file, and creates non-executable private files (0600 on Unix). Downloads are not printed into a terminal or opened automatically. Both CLI file reads and client downloads enforce the per-file byte cap.

Verification: `cargo test --manifest-path neoboard/Cargo.toml --test attachments` exercises backend quotas, invalid names, authorization, cross-channel isolation, revocation, binary fidelity, SQLite reopen, raw HTTP limits/headers, client methods, CLI upload/list/download, overwrite refusal, file permissions, and abrupt daemon restart durability.