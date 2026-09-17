import { useEffect, useState } from 'react';
import { QrCode } from 'lucide-react';
import { Modal } from './Modal';
import { requestPhoneShare } from '../phoneShare';
import type { AgentShareResult } from '../serverConnections';

export function PhoneShareButton({ onOpen }: { onOpen(): void }) {
    return <button type="button" aria-label="Open this chat on a phone" title="Open on phone" onClick={onOpen}><QrCode size={19} /></button>;
}

export function PhoneShareModal({
    server, workspaceId, sessionId, directory, close,
}: {
    server: string; workspaceId?: string; sessionId?: string; directory: string; close(): void;
}) {
    const [result, setResult] = useState<AgentShareResult>();
    const [busy, setBusy] = useState(true);
    const [error, setError] = useState('');
    const load = (shareWorkspace = false) => {
        const controller = new AbortController();
        setBusy(true); setError('');
        void requestPhoneShare({ server, workspaceId, sessionId, directory, shareWorkspace }, controller.signal)
            .then(next => { if (!controller.signal.aborted) { setResult(next); setBusy(false); } })
            .catch(e => { if (!controller.signal.aborted) { setError(e instanceof Error ? e.message : 'Could not create a phone link.'); setBusy(false); } });
        return () => controller.abort();
    };
    useEffect(() => load(false), [server, workspaceId, sessionId, directory]);
    useEffect(() => {
        if (result?.status !== 'ready' || !result.expires_at) return;
        let cancel: (() => void) | undefined;
        const timer = window.setTimeout(() => { cancel = load(false); }, Math.max(1000, result.expires_at * 1000 - Date.now() + 100));
        return () => { window.clearTimeout(timer); cancel?.(); };
    }, [result, server, workspaceId, sessionId, directory]);
    return (
        <Modal title="Open on phone" close={close}>
            {busy && <p role="status">Preparing a Tailscale link…</p>}
            {error && <p className="server-error" role="alert">{error}</p>}
            {result && !busy && <>
                {result.status !== 'ready' && <p>{result.hint}</p>}
                {result.status === 'not_shared' && <footer>
                    <button type="button" onClick={close}>Cancel</button>
                    <button type="button" className="primary" onClick={() => load(true)}>Share this workspace</button>
                </footer>}
                {result.qr_svg && <div className="phone-share-qr" role="img" aria-label="QR code for phone pairing" dangerouslySetInnerHTML={{ __html: result.qr_svg }} />}
                {result.status === 'ready' && <footer><button type="button" onClick={close}>Done</button></footer>}
            </>}
        </Modal>
    );
}
