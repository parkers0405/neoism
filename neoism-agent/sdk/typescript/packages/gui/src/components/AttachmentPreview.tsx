import { memo, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { NeoismClient, Part } from "@neoism/sdk";
import { Skeleton } from "./Skeleton";
import { artifactId, downloadAttachment, MAX_ATTACHMENT_BYTES, MAX_IMAGE_PIXELS, MAX_IMAGE_SIDE, rasterData, rasterMime } from "./attachmentPreview";
import "./attachment-preview.css";

type Props = { file?: File; part?: Extract<Part, { type: "file" }>; client?: NeoismClient };
export const AttachmentPreview = memo(function AttachmentPreview({ file, part, client }: Props) {
    // An identity boundary immediately hides old server URLs, before passive cleanup.
    return <PreviewScope key={part?.url ?? "local"} file={file} part={part} client={client} />;
});
function PreviewScope({ file, part, client }: Props) {
    const name = file?.name || part?.filename || "Attachment";
    const mime = file?.type || part?.mime || "application/octet-stream";
    const id = artifactId(part?.url ?? "");
    const image = rasterMime(mime);
    const eligible = image && (file ? file.size > 0 && file.size <= MAX_ATTACHMENT_BYTES : !!(id && client) || !!part?.url.startsWith(`data:${mime};base64,`));
    const host = useRef<HTMLDivElement>(null);
    const [visible, setVisible] = useState(false);
    const [state, setState] = useState<{ url?: string; failed?: boolean; file?: File; client?: NeoismClient }>({});
    const [zoom, setZoom] = useState(false);
    const [loaded, setLoaded] = useState(false);
    const dialog = useRef<HTMLDialogElement>(null);
    const current = state.file === file && state.client === client;
    const url = current ? state.url : undefined;
    const failed = current && state.failed;
    useEffect(() => {
        if (!host.current) return;
        if (typeof IntersectionObserver === "undefined") { setVisible(true); return; }
        const observer = new IntersectionObserver(entries => {
            if (entries.some(e => e.isIntersecting)) { setVisible(true); observer.disconnect(); }
        });
        observer.observe(host.current);
        return () => observer.disconnect();
    }, []);
    useEffect(() => {
        setZoom(false);
        setLoaded(false);
        if (!visible || !eligible) return;
        const abort = new AbortController();
        let owned: string | undefined;
        setState({ file, client });
        void (async () => {
            try {
                const blob = file ?? (id && client ? await downloadAttachment(client, id, mime, abort.signal) : rasterData(part?.url ?? "", mime));
                if (abort.signal.aborted) return;
                if (!blob) throw new Error("Preview unavailable");
                owned = URL.createObjectURL(blob);
                setState({ url: owned, file, client });
            } catch { if (!abort.signal.aborted) setState({ failed: true, file, client }); }
        })();
        return () => { abort.abort(); if (owned) URL.revokeObjectURL(owned); };
    }, [file, part?.url, client, mime, id, eligible, visible]);
    useEffect(() => {
        if (!zoom || !url) return;
        const previous = document.activeElement as HTMLElement | null;
        dialog.current?.showModal();
        return () => { dialog.current?.close(); previous?.focus(); };
    }, [zoom, url]);
    const fail = () => { setZoom(false); setState({ failed: true, file, client }); };
    return <div ref={host} className={`attachment-preview ${file ? "attachment-preview-local" : ""}`}>
        {eligible && !failed && <div className="attachment-preview-frame">
            {(!url || !loaded) && <Skeleton label={`Loading ${name} preview`}><span className="skeleton-block" /></Skeleton>}
            {url &&
                <button type="button" className="attachment-preview-open" aria-label={`View ${name}`} onClick={() => setZoom(true)}>
                    <img src={url} alt={name} onError={fail} onLoad={e => {
                        const img = e.currentTarget;
                        if (!img.naturalWidth || !img.naturalHeight || img.naturalWidth > MAX_IMAGE_SIDE || img.naturalHeight > MAX_IMAGE_SIDE || img.naturalWidth * img.naturalHeight > MAX_IMAGE_PIXELS) fail();
                        else setLoaded(true);
                    }} />
                </button>}
        </div>}
        {(!eligible || failed) && <span className="attachment-preview-name">{name}</span>}
        {failed && <small>Preview unavailable</small>}
        {zoom && url && createPortal(<dialog ref={dialog} className="attachment-preview-dialog" aria-label={`Image: ${name}`} onCancel={e => { e.preventDefault(); setZoom(false); }} onClick={e => { if (e.target === e.currentTarget) setZoom(false); }}>
            <button type="button" autoFocus aria-label="Close image" onClick={() => setZoom(false)}>Close ×</button>
            <img src={url} alt={name} onError={fail} />
        </dialog>, document.body)}
    </div>;
}
