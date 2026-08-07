# Attachments

Attachments add files or media to a user message. Neoism stores them as typed message parts and converts them into the selected provider's attachment format when that model supports the input.

## Add an attachment

Use the agent composer's attachment control or a supported paste/drop action. An attachment includes:

- A URL, commonly a `data:` URL for local content.
- A filename.
- A MIME type.

The file part may also carry a source range, text snippet, symbol reference, or diagnostic reference when it was created from editor context.

## Supported input depends on the model

Models.dev metadata tells Neoism whether a model accepts text, images, audio, video, or PDF input. The provider ultimately enforces the format and size limits.

If a model does not accept an attached modality, switch to a capable model or describe the relevant content as text.

## Images

Local images are normally encoded into the request and rendered in the agent timeline. Large images increase request size and provider cost. Crop screenshots to the relevant area where possible.

## File and editor references

A file part can identify a path or selection without requiring you to paste the whole file into the composer. Neoism's provider context builder reads supported attachments and sends the appropriate content or metadata to the model.

Editor references may include:

- File paths.
- Line/column ranges.
- Selected text.
- Symbols.
- LSP diagnostics.

## Tool-generated attachments

Tools can return attachments as part of tool results. This lets a subsequent model step inspect generated images or other media without creating a separate user message.

## Storage and privacy

Attachment metadata is stored with session history. Inline data may also be retained with the message. The selected provider receives the attachment content needed for inference.

Do not attach credential files, private keys, production database exports, or unrelated personal data. File-read permissions do not make an intentional user attachment private from the provider.

## Limits and failures

An attachment can fail because the provider rejects its MIME type, the model lacks that modality, the payload exceeds a provider limit, the local URL cannot be resolved, or an OAuth/subscription adapter does not support that input path.

See [[Models]], [[Sessions and Sharing]], and [[Formatters, LSP, and References]].