import {
    useEffect,
    useLayoutEffect,
    useRef,
    useState,
    type ReactNode,
} from "react";
import type { NeoismClient, OperationResponse } from "@neoism/sdk";
import { Plus, ArrowLeft } from "lucide-react";
import { SkeletonRows, SkeletonActivity } from "./Skeleton";
import "./settings-provider.css";
import providerIcons from "./providers/icons.svg";

type Catalog = OperationResponse<"v2.providers.list">;
const popular = [
    "anthropic",
    "openai",
    "google",
    "github-copilot",
    "openrouter",
    "vercel",
    "opencode",
];
const notes: Record<string, string> = {
    anthropic: "Claude models",
    openai: "GPT and reasoning models",
    google: "Gemini models",
    "github-copilot": "GitHub Copilot",
    openrouter: "Models from multiple providers",
    vercel: "Vercel AI Gateway",
};
// Provider artwork from OpenCode at the pinned commit; see providers/LICENSE.opencode.
export function ProviderMark({
    id,
    large = false,
}: {
    id: string;
    large?: boolean;
}) {
    const icon = popular.includes(id) ? id : "synthetic";
    return (
        <span
            className={large ? "provider-mark large" : "provider-mark"}
            aria-hidden="true"
        >
            <svg width={large ? 32 : 16} height={large ? 32 : 16}>
                <use href={`${providerIcons}#${icon}`} />
            </svg>
        </span>
    );
}
export const PROVIDER_WINDOW = 24;
const description = (p: Catalog["all"][number]) =>
    (p as { description?: string }).description || notes[p.id] || "";

export function providerGroups(
    catalog: Catalog,
    query = "",
    limit = PROVIDER_WINDOW,
) {
    const seen = new Set<string>();
    const needle = query.trim().toLowerCase();
    const matches = catalog.all.filter((p) => {
        if (seen.has(p.id)) return false;
        seen.add(p.id);
        return `${p.name} ${p.id} ${description(p)}`
            .toLowerCase()
            .includes(needle);
    });
    const connected = matches.filter((p) => catalog.connected.includes(p.id));
    const available = matches.filter((p) => !catalog.connected.includes(p.id));
    const remainder = available.filter((p) => !popular.includes(p.id));
    return {
        groups: [
            { title: "Connected", items: connected },
            {
                title: "Popular",
                items: available.filter((p) => popular.includes(p.id)),
            },
            { title: "Other providers", items: remainder.slice(0, limit) },
        ].filter((group) => group.items.length),
        hasMore: limit < remainder.length,
    };
}

export function ProviderRows({
    catalog,
    query = "",
    limit = PROVIDER_WINDOW,
    connect,
    manage,
}: {
    catalog: Catalog;
    query?: string;
    limit?: number;
    connect(id: string): void;
    manage(id: string): void;
}) {
    const { groups } = providerGroups(catalog, query, limit);
    return (
        <>
            {groups.map((group) => (
                <section className="settings-section" key={group.title}>
                    <h3>{group.title}</h3>
                    <div className="settings-list">
                        {group.items.map((p) => (
                            <div
                                className="settings-provider-row"
                                data-provider-id={p.id}
                                key={p.id}
                            >
                                <ProviderMark id={p.id} />
                                <div className="provider-copy">
                                    <span className="provider-name">
                                        {p.name}
                                    </span>
                                    {description(p) && <p>{description(p)}</p>}
                                </div>
                                <div className="provider-row-actions">
                                    {catalog.connected.includes(p.id) && (
                                        <button
                                            type="button"
                                            onClick={() => manage(p.id)}
                                        >
                                            Accounts
                                        </button>
                                    )}
                                    <button
                                        type="button"
                                        onClick={() => connect(p.id)}
                                        aria-label={`Connect ${p.name}`}
                                    >
                                        <Plus size={14} />
                                        Connect
                                    </button>
                                </div>
                            </div>
                        ))}
                    </div>
                </section>
            ))}
        </>
    );
}
// A scope change remounts the directory so neither old data nor in-flight auth UI
// can leak across clients/workspaces, even before the next effect runs.
export function ProviderDirectory(props: DirectoryProps) {
    const [scope, setScope] = useState({
        client: props.client,
        directory: props.directory,
        key: 0,
    });
    if (scope.client !== props.client || scope.directory !== props.directory) {
        setScope({
            client: props.client,
            directory: props.directory,
            key: scope.key + 1,
        });
    }
    return <ScopedProviderDirectory key={scope.key} {...props} />;
}
type DirectoryProps = {
    client: NeoismClient;
    directory: string;
    initialProviderId?: string;
    onFlowChange?(active: boolean): void;
    children(id: string, accounts: boolean): ReactNode;
};
function ScopedProviderDirectory({
    client,
    directory,
    initialProviderId,
    onFlowChange,
    children,
}: DirectoryProps) {
    const [catalog, setCatalog] = useState<Catalog>();
    const [error, setError] = useState(false);
    const [revision, reload] = useState(0);
    const [fetchedRevision, setFetchedRevision] = useState(-1);
    const pending = fetchedRevision !== revision;
    // Reuse the same request when StrictMode replays an effect. A refresh gets
    // a new revision; a different scope gets a new component/ref entirely.
    const request = useRef<
        { revision: number; promise: Promise<Catalog> } | undefined
    >(undefined);
    const [flow, setFlow] = useState(
        initialProviderId
            ? { id: initialProviderId, accounts: false }
            : undefined,
    );
    const [query, setQuery] = useState("");
    const [limit, setLimit] = useState(PROVIDER_WINDOW);
    const sentinel = useRef<HTMLDivElement>(null);
    const directoryRef = useRef<HTMLDivElement>(null);
    const returnPosition = useRef<
        { top: number; id: string; accounts: boolean } | undefined
    >(undefined);
    const hasMore = catalog
        ? providerGroups(catalog, query, limit).hasMore
        : false;
    const reveal = () => setLimit((n) => n + PROVIDER_WINDOW);
    const enterFlow = (id: string, accounts: boolean) => {
        const root = directoryRef.current?.closest(
            ".settings-content, .composer-panel-content",
        );
        returnPosition.current = { top: root?.scrollTop || 0, id, accounts };
        setFlow({ id, accounts });
    };
    useLayoutEffect(() => {
        if (flow || !returnPosition.current) return;
        const { top, id, accounts } = returnPosition.current;
        const directory = directoryRef.current;
        const row = Array.from(
            directory?.querySelectorAll<HTMLElement>(
                ".settings-provider-row",
            ) || [],
        ).find((row) => row.dataset.providerId === id);
        row?.querySelectorAll<HTMLButtonElement>("button")[
            accounts ? 0 : row.querySelectorAll("button").length - 1
        ]?.focus({ preventScroll: true });
        const root = directory?.closest(
            ".settings-content, .composer-panel-content",
        );
        if (root) root.scrollTop = top;
        returnPosition.current = undefined;
    }, [flow]);
    useEffect(() => {
        const target = sentinel.current;
        if (
            flow ||
            error ||
            !hasMore ||
            !target ||
            typeof IntersectionObserver === "undefined"
        )
            return;
        const root = target.closest(
            ".settings-content, .composer-panel-content",
        );
        let active = true;
        const observer = new IntersectionObserver(
            (entries) => {
                if (active && entries.some((entry) => entry.isIntersecting)) {
                    active = false;
                    observer.disconnect();
                    reveal();
                }
            },
            { root },
        );
        observer.observe(target);
        return () => {
            active = false;
            observer.disconnect();
        };
    }, [flow, error, hasMore, limit, query]);
    useEffect(() => {
        onFlowChange?.(!!flow);
    }, [!!flow, onFlowChange]);
    useEffect(() => {
        let active = true;
        setError(false);
        if (!request.current || request.current.revision !== revision) {
            request.current = {
                revision,
                promise: client.catalog.providers.list(directory),
            };
        }
        request.current.promise
            .then((result) => {
                if (active) setCatalog(result);
            })
            .catch(() => {
                if (active) setError(true);
            })
            .finally(() => {
                if (active) setFetchedRevision(revision);
            });
        return () => {
            active = false;
        };
    }, [client, directory, revision]);
    if (flow)
        return (
            <div className="provider-flow">
                <button
                    className="provider-back"
                    type="button"
                    onClick={() => {
                        setFlow(undefined);
                        reload((n) => n + 1);
                    }}
                >
                    <ArrowLeft size={16} />
                    Providers
                </button>
                <div className="provider-identity">
                    <ProviderMark id={flow.id} large />
                    <h2>
                        {catalog?.all.find((p) => p.id === flow.id)?.name ||
                            flow.id}
                    </h2>
                </div>
                {children(flow.id, flow.accounts)}
            </div>
        );
    return (
        <div className="provider-directory" ref={directoryRef}>
            <input
                type="search"
                aria-label="Search providers"
                placeholder="Search providers…"
                value={query}
                onChange={(e) => {
                    setQuery(e.target.value);
                    setLimit(PROVIDER_WINDOW);
                }}
            />
            {catalog && pending && <SkeletonActivity label="Refreshing providers…" />}
            {error && (
                <p role="alert">
                    Could not load providers.{" "}
                    <button type="button" onClick={() => reload((n) => n + 1)}>
                        Retry
                    </button>
                </p>
            )}
            {!catalog ? (!error && <SkeletonRows kind="provider" count={6} header />) : (
                <ProviderRows
                    catalog={catalog}
                    query={query}
                    limit={limit}
                    connect={(id) => enterFlow(id, false)}
                    manage={(id) => enterFlow(id, true)}
                />
            )}
            {catalog && !error && (
                <>
                    {!providerGroups(catalog, query, limit).groups.length && (
                        <p role="status">No matching providers.</p>
                    )}
                    {hasMore ? (
                        <div ref={sentinel} className="provider-load-more">
                            <button type="button" onClick={reveal}>
                                Load more providers
                            </button>
                        </div>
                    ) : (
                        <p className="provider-end" role="status">
                            All matching providers shown.
                        </p>
                    )}
                </>
            )}
        </div>
    );
}
