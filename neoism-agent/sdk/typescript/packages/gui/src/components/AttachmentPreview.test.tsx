// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createHttpClient, type NeoismClient, type Part } from "@neoism/sdk";
import { AttachmentPreview } from "./AttachmentPreview";
import { artifactId, MAX_ATTACHMENT_BYTES, rasterData } from "./attachmentPreview";
(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
let root: Root, host: HTMLDivElement;
let intersect: (entries: { isIntersecting: boolean }[]) => void;
const create = vi.fn(() => "blob:preview"), revoke = vi.fn();
const part = (url = "/v2/artifacts/art_123/content", mime = "image/png") => ({ id: "p", type: "file", filename: "photo.png", mime, url }) as Extract<Part, { type: "file" }>;
const sdk = () => ({ artifacts: { get: vi.fn(async () => ({ size: 3 })), download: vi.fn(async () => new Uint8Array([1, 2, 3])) } }) as unknown as NeoismClient;
const show = async () => act(async () => intersect([{ isIntersecting: true }]));
beforeEach(() => {
    host = document.createElement("div"); document.body.append(host); root = createRoot(host);
    vi.stubGlobal("IntersectionObserver", class { constructor(cb: typeof intersect) { intersect = cb; } observe() {} disconnect() {} });
    vi.spyOn(URL, "createObjectURL").mockImplementation(create);
    vi.spyOn(URL, "revokeObjectURL").mockImplementation(revoke);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); create.mockClear(); revoke.mockClear(); });
it("accepts only canonical server artifact paths and safe bounded raster data", () => {
    expect(artifactId("/v2/artifacts/art_123/content")).toBe("art_123");
    for (const url of ["https://evil.test/v2/artifacts/a/content", "//evil.test/v2/artifacts/a/content", "/v2/artifacts/../content", "/v2/artifacts/a/content?token=x", "/v2/artifacts/%2f/content"]) expect(artifactId(url)).toBeUndefined();
    expect(rasterData("data:image/png;base64,AQID", "image/png")?.size).toBe(3);
    expect(rasterData("data:image/svg+xml;base64,AQID", "image/svg+xml")).toBeUndefined();
});
it("lazily downloads real SDK Uint8Array bytes, zooms and revokes on removal", async () => {
    const client = sdk();
    await act(async () => root.render(<AttachmentPreview part={part()} client={client} />));
    expect(client.artifacts.download).not.toHaveBeenCalled(); expect(host.textContent).toContain("Loading");
    await show();
    expect(client.artifacts.download).toHaveBeenCalledWith("art_123", expect.any(AbortSignal));
    expect(host.querySelector("img")?.src).toBe("blob:preview"); expect(host.textContent).not.toContain("1,2,3");
    expect(host.querySelector("img")?.alt).toBe("photo.png");
    expect(host.querySelector(".attachment-preview-name")).toBeNull();
    const image = host.querySelector("img")!;
    Object.defineProperties(image, { naturalWidth: { value: 640 }, naturalHeight: { value: 480 } });
    await act(async () => image.dispatchEvent(new Event("load")));
    expect(host.textContent).not.toContain("photo.png");
    await act(async () => host.querySelector("button")!.click());
    expect(document.querySelector("dialog img")).not.toBeNull();
    await act(async () => document.querySelector("dialog")!.dispatchEvent(new Event("cancel", { cancelable: true })));
    expect(document.querySelector("dialog")).toBeNull();
    await act(async () => root.render(null)); expect(revoke).toHaveBeenCalledWith("blob:preview");
});
it("never downloads cross-origin or unsupported parts", async () => {
    const client = sdk();
    await act(async () => root.render(<AttachmentPreview part={part("https://tracker.test/a.png")} client={client} />)); await show();
    expect(client.artifacts.get).not.toHaveBeenCalled(); expect(client.artifacts.download).not.toHaveBeenCalled(); expect(host.querySelector("img")).toBeNull();
    await act(async () => root.render(<AttachmentPreview part={part(undefined, "image/svg+xml")} client={client} />)); await show();
    expect(client.artifacts.download).not.toHaveBeenCalled();
});
it("previews local Files and data without any SDK network and falls back on image error", async () => {
    const file = new File(["png"], "selected.png", { type: "image/png" });
    await act(async () => root.render(<AttachmentPreview file={file} />)); await show();
    expect(create).toHaveBeenCalledWith(file);
    expect(host.querySelector(".attachment-preview-name")).toBeNull();
    expect(host.querySelector("img")?.alt).toBe("selected.png");
    await act(async () => host.querySelector("img")!.dispatchEvent(new Event("error")));
    expect(host.querySelector("img")).toBeNull(); expect(host.textContent).toContain("selected.png");
    await act(async () => root.render(<AttachmentPreview part={part("data:image/png;base64,AQID")} />)); await show();
    expect(host.querySelector("img")).not.toBeNull(); expect(host.textContent).not.toContain("AQID");
});
it("aborts stale download promises without allocating or leaking blob URLs", async () => {
    const client = sdk(); let resolve!: (b: Uint8Array) => void;
    vi.mocked(client.artifacts.download).mockImplementation(() => new Promise(r => { resolve = r; }));
    await act(async () => root.render(<AttachmentPreview part={part()} client={client} />)); await show();
    const signal = vi.mocked(client.artifacts.download).mock.calls[0][1]!;
    await act(async () => root.render(null)); expect(signal.aborted).toBe(true);
    await act(async () => resolve(new Uint8Array([1]))); expect(create).not.toHaveBeenCalled();
});
it.each([undefined, "protected-secret"])("uses existing authenticated SDK binary transport (token=%s)", async token => {
    const fetcher = vi.fn(async (input: RequestInfo | URL) => String(input).endsWith("/content")
        ? new Response(new Uint8Array([1, 2, 3]), { headers: { "content-type": "application/octet-stream" } })
        : new Response(JSON.stringify({ size: 3 }), { headers: { "content-type": "application/json" } }));
    const client = createHttpClient({ baseUrl: "http://localhost:7980", token, fetch: fetcher });
    await act(async () => root.render(<AttachmentPreview part={part()} client={client} />)); await show();
    expect(host.querySelector("img")?.src).toBe("blob:preview");
    expect(fetcher).toHaveBeenCalledTimes(2);
    for (const [input, init] of fetcher.mock.calls as unknown as [string, RequestInit][]) {
        expect(String(input)).toMatch(/^http:\/\/localhost:7980\/v2\/artifacts\/art_123/);
        expect(String(input)).not.toContain("protected-secret");
        expect(new Headers(init.headers).get("authorization")).toBe(token ? `Bearer ${token}` : null);
    }
});
it("rejects oversized metadata before fetching file bytes", async () => {
    const client = sdk(); vi.mocked(client.artifacts.get).mockResolvedValue({ size: MAX_ATTACHMENT_BYTES + 1 } as never);
    await act(async () => root.render(<AttachmentPreview part={part()} client={client} />)); await show();
    expect(client.artifacts.download).not.toHaveBeenCalled(); expect(host.textContent).toContain("Preview unavailable");
});
