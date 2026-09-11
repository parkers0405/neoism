import type { NeoismClient } from "@neoism/sdk";

export const MAX_ATTACHMENT_BYTES = 20 * 1024 * 1024;
export const MAX_IMAGE_SIDE = 16384;
export const MAX_IMAGE_PIXELS = 40_000_000;
export const rasterMime = (mime: string) => /^image\/(png|jpeg|webp|gif|avif)$/.test(mime);
// Upload's downloadUrl is server-relative. Do not normalize URLs: this rejects
// origins, credentials, query tokens, escaped separators and dot traversal.
export function artifactId(url: string): string | undefined {
    return /^\/v2\/artifacts\/([A-Za-z0-9_-]+)\/content$/.exec(url)?.[1];
}
export function rasterData(url: string, mime: string): Blob | undefined {
    if (!rasterMime(mime) || url.length > Math.ceil(MAX_ATTACHMENT_BYTES / 3) * 4 + 64) return;
    const match = /^data:(image\/(?:png|jpeg|webp|gif|avif));base64,([A-Za-z0-9+/]*={0,2})$/.exec(url);
    if (!match || match[1] !== mime) return;
    try {
        const raw = atob(match[2]);
        if (!raw.length || raw.length > MAX_ATTACHMENT_BYTES) return;
        return new Blob([Uint8Array.from(raw, c => c.charCodeAt(0))], { type: mime });
    } catch { return; }
}
export async function downloadAttachment(client: NeoismClient, id: string, mime: string, signal: AbortSignal): Promise<Blob> {
    const metadata = await client.artifacts.get(id);
    if (signal.aborted) throw new Error("Aborted");
    if (!Number.isFinite(metadata.size) || metadata.size <= 0 || metadata.size > MAX_ATTACHMENT_BYTES) throw new Error("Attachment exceeds preview limit");
    const bytes = await client.artifacts.download(id, signal);
    if (signal.aborted) throw new Error("Aborted");
    if (!(bytes instanceof Uint8Array) || !bytes.byteLength || bytes.byteLength > MAX_ATTACHMENT_BYTES) throw new Error("Invalid attachment bytes");
    return new Blob([new Uint8Array(bytes)], { type: mime });
}
