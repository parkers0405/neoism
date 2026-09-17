import { useEffect, useRef, useState, type ReactNode } from 'react';
import { agentGuiOrigin } from '../serverConnections';
import { bootstrapPhonePairing, type PhoneConnection } from '../phoneShare';
import { loadPreferences } from '../types';

/** Mount no chat clients until the one-use pairing credential has been claimed. */
export function PhoneEntry({ children }: { children(connection?: PhoneConnection): ReactNode }) {
    const phone = !!agentGuiOrigin();
    const [connection, setConnection] = useState<PhoneConnection>();
    const [error, setError] = useState('');
    const pending = useRef<Promise<PhoneConnection> | undefined>(undefined);
    useEffect(() => {
        if (!phone) return;
        let alive = true;
        // StrictMode may replay the effect; the code must only be consumed once.
        pending.current ??= (async () => {
            let joined: PhoneConnection | undefined;
            await bootstrapPhonePairing({ name: loadPreferences().name,
                join: (server, token, _directory, sessionId) => { joined = { server, token, sessionId }; },
            });
            if (!joined) throw new Error('Scan a new QR code to connect.');
            return joined;
        })();
        void pending.current.then(value => { if (alive) setConnection(value); })
            .catch(() => { if (alive) setError('Scan a new QR code to connect.'); });
        return () => { alive = false; };
    }, [phone]);
    if (!phone) return children();
    if (connection) return children(connection);
    return <main className="workspace-home"><p role={error ? 'alert' : 'status'}>{error || 'Connecting...'}</p></main>;
}
