import { ProviderDirectory } from "./ProviderDirectory";
import "./settings-provider.css";
import { useEffect, useRef, useState } from "react";
import type { NeoismClient, OperationResponse } from "@neoism/sdk";
import {
    authChoices,
    promptOptions,
    visiblePrompt,
    providerAccounts,
    providerOperationError,
    type Authorization,
    type ProviderConnection,
    type ProviderConnectionPickerProps,
} from "../providerConnections";

export function ProviderConnections({
    client,
    directory,
    initialProviderId,
    workspaceId,
    selectedConnection,
    onSelectConnection,
    onFlowChange,
}: {
    client: NeoismClient;
    directory: string;
    onFlowChange?(active: boolean): void;
} & ProviderConnectionPickerProps) {
    // Remount scoped state on server/workspace changes; stale requests must not update the next picker.
    const [scope, setScope] = useState({
        client,
        directory,
        workspaceId,
        initialProviderId,
        generation: 0,
    });
    if (
        scope.client !== client ||
        scope.directory !== directory ||
        scope.workspaceId !== workspaceId ||
        scope.initialProviderId !== initialProviderId
    ) {
        setScope({
            client,
            directory,
            workspaceId,
            initialProviderId,
            generation: scope.generation + 1,
        });
    }
    return (
        <ProviderDirectory
            key={scope.generation}
            client={client}
            directory={directory}
            initialProviderId={initialProviderId}
            onFlowChange={onFlowChange}
        >
            {(id, accounts) => (
                <AccountPicker
                    key={id}
                    onStopWaiting={() =>
                        setScope((s) => ({
                            ...s,
                            generation: s.generation + 1,
                        }))
                    }
                    client={client}
                    directory={directory}
                    initialProviderId={id}
                    workspaceId={workspaceId}
                    selectedConnection={selectedConnection}
                    onSelectConnection={onSelectConnection}
                    initialAccounts={accounts}
                />
            )}
        </ProviderDirectory>
    );
}

function AccountPicker({
    client,
    directory,
    initialProviderId,
    workspaceId,
    selectedConnection,
    onSelectConnection,
    onStopWaiting,
    initialAccounts = false,
}: {
    client: NeoismClient;
    directory: string;
    onStopWaiting(): void;
    initialAccounts?: boolean;
} & ProviderConnectionPickerProps) {
    const [methods, setMethods] = useState<
        OperationResponse<"v2.providers.authMethods">
    >({});
    const [provider] = useState(
        initialProviderId || selectedConnection?.providerId || "",
    );
    const [accounts, setAccounts] = useState<ProviderConnection[]>([]);
    const [loaded, setLoaded] = useState(false);
    const [reload, setReload] = useState(0);
    const [method, setMethod] = useState(0);
    const [accountView, setAccountView] = useState(initialAccounts);
    const [methodChosen, setMethodChosen] = useState(false);
    const [label, setLabel] = useState("");
    const [key, setKey] = useState("");
    const [code, setCode] = useState("");
    const [inputs, setInputs] = useState<Record<string, string>>({});
    const [target, setTarget] = useState<ProviderConnection>();
    const [authorization, setAuthorization] = useState<Authorization>();
    const [rename, setRename] = useState<ProviderConnection>();
    const [renameLabel, setRenameLabel] = useState("");
    const [deleting, setDeleting] = useState<ProviderConnection>();
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState("");
    const [message, setMessage] = useState("");
    const alive = useRef(true);
    const lock = useRef(false);
    useEffect(() => {
        alive.current = true;
        return () => {
            alive.current = false;
        };
    }, []);
    const api = providerAccounts(client, provider, workspaceId);
    const choices = authChoices(methods[provider]);
    const selected = choices[method];
    const reset = () => {
        setKey("");
        setCode("");
        setInputs({});
        setAuthorization(undefined);
        setTarget(undefined);
        setLabel("");
    };
    const run = async (fn: () => Promise<void>) => {
        if (lock.current) return;
        lock.current = true;
        setBusy(true);
        setError("");
        setMessage("");
        try {
            await fn();
        } catch {
            if (alive.current) setError(providerOperationError);
        } finally {
            lock.current = false;
            if (alive.current) setBusy(false);
        }
    };
    const refresh = async () => {
        try {
            const result = await api.list();
            if (alive.current) {
                setAccounts(result);
                setLoaded(true);
            }
        } catch {
            if (alive.current) {
                setAccounts([]);
                setLoaded(false);
                setError(
                    "Could not refresh accounts. Any completed account change is already saved; retry Refresh accounts, not the account change.",
                );
            }
        }
    };
    useEffect(() => {
        let active = true;
        setLoaded(false);
        setAccounts([]);
        setError("");
        Promise.all([
            client.catalog.providers.authMethods(directory),
            provider ? api.list() : Promise.resolve([]),
        ])
            .then(([m, a]) => {
                if (active) {
                    setMethods(m);
                    setAccounts(a);
                    setLoaded(true);
                }
            })
            .catch(() => {
                if (active) setError(providerOperationError);
            });
        return () => {
            active = false;
        };
    }, [client, directory, workspaceId, provider, reload]);
    const notify = (
        account: ProviderConnection,
        reason: "selected" | "deleted",
    ) => {
        if (alive.current)
            onSelectConnection?.({
                providerId: provider,
                connectionId: account.connectionId,
                label: account.label,
                reason,
            });
    };
    const complete = () =>
        run(async () => {
            if (!authorization) return;
            const secret = code;
            setCode("");
            await api.complete(method, authorization, secret);
            if (!alive.current) return;
            reset();
            setAccountView(true);
            setMessage(
                "Account connected. Choose ‘Use for prompts’ to select its billing account.",
            );
            await refresh();
        });
    useEffect(() => {
        // Auto/device methods block at the callback endpoint until authorization completes.
        // Keep the provider link visible while waiting; retry remains available on failure.
        if (authorization?.method === "auto") void complete();
    }, [authorization]);
    return (
        <section
            className="provider-connections"
            aria-label="Provider accounts"
            onKeyDown={(e) => {
                // Inline hosts may have an outer composer form; credentials must never submit it.
                if (
                    e.key === "Enter" &&
                    (e.target instanceof HTMLInputElement ||
                        e.target instanceof HTMLSelectElement)
                )
                    e.preventDefault();
            }}
        >
            <p className="provider-security">
                Account changes apply immediately on the current server. Secrets
                are sent to that server, never saved in browser storage. Prompt
                billing selection is separate from the server default.
            </p>
            {!onSelectConnection && (
                <p className="provider-security">
                    Prompt account selection is not available in this host.
                </p>
            )}
            <div className="provider-view-tabs">
                <button
                    type="button"
                    disabled={busy}
                    aria-pressed={!accountView}
                    onClick={() => setAccountView(false)}
                >
                    Connect account
                </button>
                <button
                    type="button"
                    disabled={busy}
                    aria-pressed={accountView}
                    onClick={() => setAccountView(true)}
                >
                    Saved accounts
                </button>
            </div>
            <fieldset
                disabled={busy}
                style={{ border: 0, padding: 0, minWidth: 0 }}
            >
                {provider && (
                    <>
                        {accountView && (
                            <>
                                <h4>Saved accounts</h4>
                                {!loaded ? (
                                    <p className="muted">
                                        Accounts not loaded.
                                    </p>
                                ) : !accounts.length ? (
                                    <p className="muted">No saved accounts.</p>
                                ) : (
                                    <ul>
                                        {accounts.map((a) => (
                                            <li key={a.connectionId}>
                                                <strong>{a.label}</strong> ·{" "}
                                                {a.authType}
                                                {a.isDefault
                                                    ? " · server default"
                                                    : ""}
                                                {selectedConnection?.providerId ===
                                                    provider &&
                                                selectedConnection.connectionId ===
                                                    a.connectionId
                                                    ? " · selected for prompts"
                                                    : ""}
                                                <div className="actions">
                                                    {onSelectConnection && (
                                                        <button
                                                            type="button"
                                                            onClick={() => {
                                                                notify(
                                                                    a,
                                                                    "selected",
                                                                );
                                                                setMessage(
                                                                    `Selected ${a.label} for prompt billing.`,
                                                                );
                                                            }}
                                                        >
                                                            Use for prompts
                                                        </button>
                                                    )}
                                                    <button
                                                        type="button"
                                                        onClick={() => {
                                                            setRename(a);
                                                            setRenameLabel(
                                                                a.label,
                                                            );
                                                            setDeleting(
                                                                undefined,
                                                            );
                                                        }}
                                                    >
                                                        Rename
                                                    </button>
                                                    <button
                                                        type="button"
                                                        disabled={a.isDefault}
                                                        onClick={() =>
                                                            void run(
                                                                async () => {
                                                                    await api.setDefault(
                                                                        a.connectionId,
                                                                    );
                                                                    setMessage(
                                                                        "Server default updated; prompt selection unchanged.",
                                                                    );
                                                                    await refresh();
                                                                },
                                                            )
                                                        }
                                                    >
                                                        Make server default
                                                    </button>
                                                    {a.authType === "oauth" &&
                                                        choices.some(
                                                            (m) =>
                                                                m.type ===
                                                                "oauth",
                                                        ) && (
                                                            <button
                                                                type="button"
                                                                onClick={() => {
                                                                    reset();
                                                                    setAccountView(
                                                                        false,
                                                                    );
                                                                    setMethodChosen(
                                                                        false,
                                                                    );
                                                                    setTarget(
                                                                        a,
                                                                    );
                                                                    setLabel(
                                                                        a.label,
                                                                    );
                                                                    setMethod(
                                                                        choices.findIndex(
                                                                            (
                                                                                m,
                                                                            ) =>
                                                                                m.type ===
                                                                                "oauth",
                                                                        ),
                                                                    );
                                                                }}
                                                            >
                                                                Reauthorize
                                                            </button>
                                                        )}
                                                    <button
                                                        type="button"
                                                        onClick={() => {
                                                            setDeleting(a);
                                                            setRename(
                                                                undefined,
                                                            );
                                                        }}
                                                    >
                                                        Delete account…
                                                    </button>
                                                </div>
                                            </li>
                                        ))}
                                    </ul>
                                )}
                                <button
                                    type="button"
                                    onClick={() => void run(refresh)}
                                >
                                    Refresh accounts
                                </button>
                                {rename && (
                                    <div className="notice">
                                        <label>
                                            New account label{" "}
                                            <input
                                                value={renameLabel}
                                                onChange={(e) =>
                                                    setRenameLabel(
                                                        e.target.value,
                                                    )
                                                }
                                            />
                                        </label>
                                        <button
                                            type="button"
                                            disabled={!renameLabel.trim()}
                                            onClick={() =>
                                                void run(async () => {
                                                    await api.rename(
                                                        rename.connectionId,
                                                        renameLabel,
                                                    );
                                                    setRename(undefined);
                                                    setMessage(
                                                        "Account renamed.",
                                                    );
                                                    await refresh();
                                                })
                                            }
                                        >
                                            Save account label
                                        </button>
                                        <button
                                            type="button"
                                            onClick={() => setRename(undefined)}
                                        >
                                            Cancel rename
                                        </button>
                                    </div>
                                )}
                                {deleting && (
                                    <div className="notice" role="alert">
                                        <p>
                                            Delete “{deleting.label}” from the
                                            server? Only this account’s
                                            credentials will be removed. Prompts
                                            using it must select another
                                            account.
                                            {deleting.isDefault
                                                ? " This is the current server default."
                                                : ""}
                                        </p>
                                        <button
                                            type="button"
                                            onClick={() =>
                                                void run(async () => {
                                                    await api.remove(
                                                        deleting.connectionId,
                                                    );
                                                    notify(deleting, "deleted");
                                                    setDeleting(undefined);
                                                    if (
                                                        target?.connectionId ===
                                                        deleting.connectionId
                                                    )
                                                        reset();
                                                    setAccountView(true);
                                                    setMessage(
                                                        "Account deleted.",
                                                    );
                                                    await refresh();
                                                })
                                            }
                                        >
                                            Confirm delete account
                                        </button>
                                        <button
                                            type="button"
                                            onClick={() =>
                                                setDeleting(undefined)
                                            }
                                        >
                                            Keep account
                                        </button>
                                    </div>
                                )}
                            </>
                        )}
                        {!accountView && (
                            <>
                                <h4>
                                    {target
                                        ? `Reauthorize ${target.label}`
                                        : "Add account"}
                                </h4>
                                {!authorization &&
                                choices.length > 1 &&
                                !methodChosen ? (
                                    <div className="provider-methods">
                                        <p>Choose how to connect</p>
                                        {choices.map((m, i) => (
                                            <button
                                                type="button"
                                                key={i}
                                                disabled={
                                                    !!target &&
                                                    m.type !== "oauth"
                                                }
                                                onClick={() => {
                                                    setMethod(i);
                                                    setMethodChosen(true);
                                                }}
                                            >
                                                {m.label}
                                                <span>→</span>
                                            </button>
                                        ))}
                                    </div>
                                ) : null}
                                {(choices.length === 1 || methodChosen) && (
                                    <>
                                        {choices.length > 1 &&
                                            !authorization && (
                                                <button
                                                    type="button"
                                                    onClick={() => {
                                                        setKey("");
                                                        setCode("");
                                                        setInputs({});
                                                        setMethodChosen(false);
                                                    }}
                                                >
                                                    Change authentication method
                                                </button>
                                            )}
                                        <label>
                                            Account label{" "}
                                            <input
                                                value={label}
                                                disabled={!!authorization}
                                                autoComplete="off"
                                                onChange={(e) =>
                                                    setLabel(e.target.value)
                                                }
                                                placeholder="Personal, work, project…"
                                            />
                                        </label>
                                        {selected?.type === "oauth" ? (
                                            <>
                                                {!authorization && (
                                                    <>
                                                        {selected.prompts
                                                            ?.filter((p) =>
                                                                visiblePrompt(
                                                                    p,
                                                                    inputs,
                                                                ),
                                                            )
                                                            .map((p) => (
                                                                <label
                                                                    key={p.key}
                                                                    className="form-field"
                                                                >
                                                                    {p.message}
                                                                    {p.type ===
                                                                    "select" ? (
                                                                        <select
                                                                            value={
                                                                                inputs[
                                                                                    p
                                                                                        .key
                                                                                ] ||
                                                                                ""
                                                                            }
                                                                            onChange={(
                                                                                e,
                                                                            ) =>
                                                                                setInputs(
                                                                                    (
                                                                                        s,
                                                                                    ) => ({
                                                                                        ...s,
                                                                                        [p.key]:
                                                                                            e
                                                                                                .target
                                                                                                .value,
                                                                                    }),
                                                                                )
                                                                            }
                                                                        >
                                                                            <option value="">
                                                                                Choose…
                                                                            </option>
                                                                            {promptOptions(
                                                                                p,
                                                                            ).map(
                                                                                (
                                                                                    o,
                                                                                ) => (
                                                                                    <option
                                                                                        key={
                                                                                            o.value
                                                                                        }
                                                                                        value={
                                                                                            o.value
                                                                                        }
                                                                                    >
                                                                                        {
                                                                                            o.label
                                                                                        }
                                                                                    </option>
                                                                                ),
                                                                            )}
                                                                        </select>
                                                                    ) : (
                                                                        <input
                                                                            type="password"
                                                                            autoComplete="off"
                                                                            value={
                                                                                inputs[
                                                                                    p
                                                                                        .key
                                                                                ] ||
                                                                                ""
                                                                            }
                                                                            onChange={(
                                                                                e,
                                                                            ) =>
                                                                                setInputs(
                                                                                    (
                                                                                        s,
                                                                                    ) => ({
                                                                                        ...s,
                                                                                        [p.key]:
                                                                                            e
                                                                                                .target
                                                                                                .value,
                                                                                    }),
                                                                                )
                                                                            }
                                                                        />
                                                                    )}
                                                                </label>
                                                            ))}
                                                        <button
                                                            type="button"
                                                            disabled={
                                                                !label.trim() ||
                                                                !loaded
                                                            }
                                                            onClick={() =>
                                                                void run(
                                                                    async () => {
                                                                        const activeInputs =
                                                                            Object.fromEntries(
                                                                                (
                                                                                    selected.prompts ||
                                                                                    []
                                                                                )
                                                                                    .filter(
                                                                                        (
                                                                                            p,
                                                                                        ) =>
                                                                                            visiblePrompt(
                                                                                                p,
                                                                                                inputs,
                                                                                            ),
                                                                                    )
                                                                                    .map(
                                                                                        (
                                                                                            p,
                                                                                        ) => [
                                                                                            p.key,
                                                                                            inputs[
                                                                                                p
                                                                                                    .key
                                                                                            ] ||
                                                                                                "",
                                                                                        ],
                                                                                    ),
                                                                            );
                                                                        const result =
                                                                            await api.authorize(
                                                                                method,
                                                                                label,
                                                                                activeInputs,
                                                                                target?.connectionId,
                                                                            );
                                                                        if (
                                                                            alive.current
                                                                        ) {
                                                                            setInputs(
                                                                                {},
                                                                            );
                                                                            setAuthorization(
                                                                                result,
                                                                            );
                                                                        }
                                                                    },
                                                                )
                                                            }
                                                        >
                                                            Continue
                                                        </button>
                                                    </>
                                                )}
                                                {authorization && (
                                                    <div className="notice">
                                                        <p>
                                                            {
                                                                authorization.instructions
                                                            }
                                                        </p>
                                                        {/^https?:\/\//i.test(
                                                            authorization.url,
                                                        ) && (
                                                            <a
                                                                href={
                                                                    authorization.url
                                                                }
                                                                target="_blank"
                                                                rel="noopener noreferrer"
                                                                referrerPolicy="no-referrer"
                                                            >
                                                                Open provider
                                                                authorization ↗
                                                            </a>
                                                        )}
                                                        {authorization.method ===
                                                        "code" ? (
                                                            <label>
                                                                Authorization
                                                                code{" "}
                                                                <input
                                                                    type="password"
                                                                    autoComplete="off"
                                                                    value={code}
                                                                    onChange={(
                                                                        e,
                                                                    ) =>
                                                                        setCode(
                                                                            e
                                                                                .target
                                                                                .value,
                                                                        )
                                                                    }
                                                                />
                                                            </label>
                                                        ) : (
                                                            <p>
                                                                Complete
                                                                authorization in
                                                                the provider
                                                                tab. Waiting for
                                                                the provider
                                                                callback
                                                                automatically;
                                                                if it fails,
                                                                retry completion
                                                                below.
                                                            </p>
                                                        )}
                                                        <button
                                                            type="button"
                                                            disabled={
                                                                authorization.method ===
                                                                    "code" &&
                                                                !code.trim()
                                                            }
                                                            onClick={() =>
                                                                void complete()
                                                            }
                                                        >
                                                            Complete connection
                                                        </button>
                                                    </div>
                                                )}
                                            </>
                                        ) : (
                                            <div
                                                className="actions provider-key"
                                                title="Add API-key account"
                                            >
                                                <input
                                                    type="password"
                                                    autoComplete="off"
                                                    aria-label="Provider API key"
                                                    placeholder="Provider API key"
                                                    value={key}
                                                    onChange={(e) =>
                                                        setKey(e.target.value)
                                                    }
                                                />
                                                <button
                                                    type="button"
                                                    disabled={
                                                        !label.trim() ||
                                                        !key.trim() ||
                                                        !loaded
                                                    }
                                                    onClick={() =>
                                                        void run(async () => {
                                                            const secret = key;
                                                            setKey("");
                                                            await api.create(
                                                                label,
                                                                secret,
                                                            );
                                                            if (!alive.current)
                                                                return;
                                                            reset();
                                                            setAccountView(
                                                                true,
                                                            );
                                                            setMessage(
                                                                "Account added. Choose ‘Use for prompts’ to select its billing account.",
                                                            );
                                                            await refresh();
                                                        })
                                                    }
                                                >
                                                    Continue
                                                </button>
                                            </div>
                                        )}
                                        {(target || authorization) && (
                                            <button
                                                type="button"
                                                onClick={reset}
                                            >
                                                Cancel authorization / add
                                                another account
                                            </button>
                                        )}
                                    </>
                                )}
                            </>
                        )}
                    </>
                )}
            </fieldset>
            {busy && (
                <p className="muted" role="status">
                    Contacting server…
                </p>
            )}
            {busy && authorization && (
                <div className="notice">
                    <button type="button" onClick={onStopWaiting}>
                        Stop waiting
                    </button>
                    <p>
                        This only stops waiting in this picker, not the server
                        authorization. It may still complete; refresh accounts
                        before starting another connection.
                    </p>
                </div>
            )}
            {error && (
                <p className="error" role="alert">
                    {error}{" "}
                    <button
                        type="button"
                        disabled={busy}
                        onClick={() => setReload((n) => n + 1)}
                    >
                        Retry loading accounts and methods
                    </button>
                </p>
            )}
            {message && (
                <p className="muted" role="status">
                    {message}
                </p>
            )}
        </section>
    );
}
