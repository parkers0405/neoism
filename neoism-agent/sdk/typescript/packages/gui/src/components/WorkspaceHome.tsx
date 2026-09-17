import { ArrowUpRight, Folder, Plus, RefreshCw } from 'lucide-react';
import { createHttpTransport } from '@neoism/sdk';
import { useEffect, useRef, useState } from 'react';
import { Wordmark } from './Identity';
import { DaemonConnection, scopedAgentFetch, workspaceAgentUrl, type LocalWorkspace, type SharedWorkspace } from '../serverConnections';
import { credentialKey, localServer, matchesServer, NativeServerRegistry, serverAddress, serverCredentials, type SavedServer } from '../serverRegistry';
import type { Preferences } from '../types';
import { ServerConnections } from './ServerConnections';
import { Modal } from './Modal';
import './WorkspaceHome.css';

const isLocalWorkspace = (workspace: LocalWorkspace | SharedWorkspace): workspace is LocalWorkspace =>
    typeof (workspace as LocalWorkspace).directory === 'string';

export function WorkspaceHome({ workspaces, loading, error, select, refresh, value, token, join, credentialFor, forget }: {
    workspaces: LocalWorkspace[]; loading: boolean; error: string;
    select(workspace: LocalWorkspace, server: SavedServer, credential: string): void; refresh(): void;
    value: Preferences; token: string;
    join(server: string, token: string, directory?: string): void;
    credentialFor?(server: string): string; forget?(server: string): void;
}) {
    const [servers, setServers] = useState<SavedServer[]>([]);
    const [serverId, setServerId] = useState('local');
    const [remoteRows, setRemoteRows] = useState<SharedWorkspace[]>([]);
    const [remoteLoading, setRemoteLoading] = useState(false);
    const [remoteError, setRemoteError] = useState('');
    const [serverDialog, setServerDialog] = useState<'add' | 'connect'>();
    const [registryRevision, setRegistryRevision] = useState(0);
    const choseServer = useRef(false);
    const request = useRef<AbortController | undefined>(undefined);
    useEffect(() => {
        // Let the operator workspace request finish first. Besides producing a
        // faster first useful row, this avoids competing with bootstrap fetches.
        if (loading) return;
        const controller = new AbortController();
        void new NativeServerRegistry().list(controller.signal).then(found => {
            if (controller.signal.aborted) return;
            setServers(found);
            if (!choseServer.current) {
                const active = found.find(row => matchesServer(row, value));
                setServerId(active?.id || 'local');
            }
        }).catch(() => { if (!controller.signal.aborted) setServers([]); });
        return () => controller.abort();
    }, [loading, registryRevision, value.server, value.directory]);
    const selected = serverId === 'local' ? localServer() : servers.find(row => row.id === serverId);
    const credential = selected ? (matchesServer(selected, value) ? token : serverCredentials.get(credentialKey(selected)) || credentialFor?.(selected.address) || '') : '';
    useEffect(() => {
        request.current?.abort();
        const controller = new AbortController(); request.current = controller;
        setRemoteRows([]); setRemoteError('');
        if (!selected || selected.id === 'local' || selected.kind === 'agent') { setRemoteLoading(false); return () => controller.abort(); }
        setRemoteLoading(true);
        void new DaemonConnection(selected.address).workspaces(credential, controller.signal)
            .then(rows => { if (!controller.signal.aborted) setRemoteRows(rows); })
            .catch(e => { if (!controller.signal.aborted) setRemoteError(e instanceof Error ? e.message : 'Could not load workspaces.'); })
            .finally(() => { if (!controller.signal.aborted) setRemoteLoading(false); });
        return () => controller.abort();
    }, [selected?.id, selected?.address, selected?.kind, credential]);
    const chooseRemote = (workspace: SharedWorkspace) => {
        if (!selected || selected.kind !== 'daemon') return;
        request.current?.abort(); const controller = new AbortController(); request.current = controller;
        setRemoteLoading(true); setRemoteError('');
        void new DaemonConnection(selected.address).verify(workspace.id, credential, controller.signal).then(() => {
            if (!controller.signal.aborted) join(workspaceAgentUrl(selected.address, workspace.id), credential, '');
        }).catch(e => { if (!controller.signal.aborted) setRemoteError(e instanceof Error ? e.message : 'Could not connect.'); })
            .finally(() => { if (!controller.signal.aborted) setRemoteLoading(false); });
    };
    const chooseAgent = () => {
        if (!selected || selected.kind !== 'agent') return;
        request.current?.abort(); const controller = new AbortController(); request.current = controller;
        setRemoteLoading(true); setRemoteError('');
        const base = serverAddress(selected.address, 'agent');
        void createHttpTransport({ baseUrl: base, token: credential || undefined, fetch: scopedAgentFetch(base) })
            .request({ path: '/v2/health', signal: controller.signal }).then(() => {
                if (!controller.signal.aborted) join(base, credential, selected.directory);
            }).catch(e => { if (!controller.signal.aborted) setRemoteError((e as { status?: number }).status === 401 ? 'Connect this server to enter its credential.' : 'Could not connect to this Agent API.'); })
            .finally(() => { if (!controller.signal.aborted) setRemoteLoading(false); });
    };
    const visibleRows = selected?.id === 'local' ? workspaces : remoteRows;
    const visibleLoading = selected?.id === 'local' ? loading : remoteLoading;
    const visibleError = selected?.id === 'local' ? error : remoteError;
    return <section className="workspace-home" aria-label="Workspaces">
        <div className="workspace-home-content">
            <Wordmark />
            <div className="workspace-home-label"><h1>Workspaces</h1>
                <button type="button" aria-label="Refresh workspaces" onClick={() => { refresh(); setRegistryRevision(n => n + 1); }} disabled={visibleLoading}><RefreshCw size={15} /></button>
            </div>
            {visibleLoading && <p className="workspace-home-status" role="status">Loading…</p>}
            {visibleError && <p className="workspace-home-status" role="alert">{visibleError}</p>}
            {!visibleLoading && selected?.id === 'local' && !visibleError && !visibleRows.length && <p className="workspace-home-status">No open workspaces</p>}
            {!visibleLoading && selected?.kind === 'daemon' && !credential && <button type="button" className="workspace-home-connect" onClick={() => setServerDialog('connect')}>Connect to {selected.name}</button>}
            {!visibleLoading && !visibleError && selected?.kind === 'daemon' && !visibleRows.length && <p className="workspace-home-status">No shared workspaces</p>}
            <div className="workspace-home-list">
                {visibleRows.map(workspace => <button type="button" key={workspace.id} className="workspace-home-row"
                    onClick={() => selected?.id === 'local' ? select(workspace as LocalWorkspace, selected, credential) : chooseRemote(workspace)} title={isLocalWorkspace(workspace) ? workspace.directory : workspace.title}>
                    <Folder size={18} aria-hidden="true" />
                    <span><strong>{workspace.title || (isLocalWorkspace(workspace) ? workspace.directory.split(/[\\/]/).filter(Boolean).at(-1) : workspace.id)}</strong>
                        {isLocalWorkspace(workspace) && <small>{workspace.directory}</small>}</span>
                    <ArrowUpRight size={16} aria-hidden="true" />
                </button>)}
                {!visibleLoading && selected?.kind === 'agent' && selected.id !== 'local' && <button type="button" className="workspace-home-row" onClick={credential ? chooseAgent : () => setServerDialog('connect')}>
                    <Plus size={18} aria-hidden="true" /><span><strong>{selected.directory || `Connect to ${selected.name}`}</strong><small>{selected.address}</small></span><ArrowUpRight size={16} aria-hidden="true" />
                </button>}
            </div>
            <footer className="workspace-home-footer"><label><select aria-label="Server" value={serverId} onChange={event => {
                if (event.target.value === 'add') { setServerDialog('add'); return; }
                choseServer.current = true; setServerId(event.target.value);
            }}><option value="local">{localServer().name}</option>{servers.map(server => <option key={server.id} value={server.id}>{server.name}</option>)}<option value="add">Add server…</option></select></label></footer>
        </div>
        {serverDialog && <Modal title={serverDialog === 'add' ? 'Add server' : 'Servers'} close={() => setServerDialog(undefined)}><ServerConnections initialAdd={serverDialog === 'add'} value={value} token={token}
            join={(server, nextToken, directory) => { join(server, nextToken, directory); setServerDialog(undefined); }} forget={forget} credentialFor={credentialFor}
            registryChanged={() => setRegistryRevision(n => n + 1)} /></Modal>}
    </section>;
}
