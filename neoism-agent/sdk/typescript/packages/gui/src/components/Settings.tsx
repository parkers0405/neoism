import type { NeoismClient } from "@neoism/sdk";
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { ThemePreview } from "./ThemePreview";
import {
    X,
    SlidersHorizontal,
    Palette,
    Server,
    Plug,
    ArrowLeft,
    Check,
    ChevronRight,
} from "lucide-react";
import { ServerConnections } from "./ServerConnections";
import { joinedDaemon } from "../serverConnections";
import { ProviderConnections } from "./ProviderConnections";
import type { ProviderConnectionPickerProps } from "../providerConnections";
import { fonts, systemFontOptions } from "../generated/fonts";
import { themeOptions } from "../appearance";
import type { Preferences } from "../types";
import { version } from "../../package.json";
import "./settings-provider.css";

export function ThemePicker({
    selected,
    query,
    search,
    choose,
    back,
    showBack = true,
}: {
    selected: string;
    query: string;
    search(value: string): void;
    choose(id: string): void;
    back(): void;
    showBack?: boolean;
}) {
    const matches = themeOptions.filter((theme) =>
        `${theme.name} ${theme.id}`
            .toLowerCase()
            .includes(query.trim().toLowerCase()),
    );
    const [candidateId, setCandidateId] = useState(selected);
    const results = useRef<HTMLDivElement>(null);
    // A filtered-out candidate must never leave a stale preview or Enter target.
    const candidate = matches.find(theme => theme.id === candidateId) || matches[0];
    const navigate = (event: KeyboardEvent<HTMLElement>, focusRow: boolean) => {
        if (event.nativeEvent.isComposing || event.altKey || event.ctrlKey || event.metaKey) return;
        if (event.key === "Enter") {
            event.preventDefault();
            if (candidate) choose(candidate.id);
            return;
        }
        if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
        event.preventDefault();
        if (!matches.length) return;
        const current = matches.findIndex(theme => theme.id === candidate?.id);
        const next = (current + (event.key === "ArrowDown" ? 1 : -1) + matches.length) % matches.length;
        setCandidateId(matches[next].id);
        const row = results.current?.querySelectorAll<HTMLButtonElement>("button")[next];
        if (focusRow) row?.focus({ preventScroll: true });
        row?.scrollIntoView({ block: "nearest", inline: "nearest" });
    };
    return (
        <section className="settings-theme-picker" aria-label="Theme picker">
            <input
                autoFocus
                type="search"
                aria-label="Search themes"
                placeholder="Search themes…"
                value={query}
                onChange={(e) => {
                    setCandidateId("");
                    search(e.target.value);
                }}
                onKeyDown={(e) => navigate(e, false)}
            />
            {showBack && <button type="button" className="theme-back" onClick={back}>
                <ArrowLeft size={16} aria-hidden="true" />
                Back to Appearance
            </button>}
            <div className={`settings-theme-browser${candidate ? "" : " settings-theme-browser-empty"}`}>
                <div ref={results} className="settings-theme-results" aria-label="Themes"
                    onKeyDown={(e) => navigate(e, true)}>
                    {matches.map((theme) => (
                        <button
                            type="button"
                            key={theme.id}
                            aria-pressed={selected === theme.id}
                            data-preview={candidate?.id === theme.id}
                            onMouseEnter={() => setCandidateId(theme.id)}
                            onFocus={() => setCandidateId(theme.id)}
                            onClick={() => choose(theme.id)}
                        >
                            <span>{theme.name}</span>
                            {selected === theme.id && (
                                <Check size={16} aria-label="Selected" />
                            )}
                        </button>
                    ))}
                    {!matches.length && <p role="status">No matching themes.</p>}
                </div>
                {candidate && <ThemePreview theme={candidate} />}
            </div>
        </section>
    );
}

type Page = "General" | "Appearance" | "Servers" | "Providers";
export function Settings({
    client,
    value,
    token,
    connected,
    forgetServer,
    serverCredential,
    save,
    close,
    onSelectConnection,
    selectedConnection,
    workspaceId,
    initialProviderId,
}: {
    client: NeoismClient;
    value: Preferences;
    token: string;
    connected?: boolean;
    forgetServer?(server: string): void;
    serverCredential?(server: string): string;
    save(p: Preferences, t: string): void;
    close(): void;
} & ProviderConnectionPickerProps) {
    const [draft, set] = useState(value);

    const [filter, setFilter] = useState("");
    const [pickingTheme, setPickingTheme] = useState(false);
    const themeTrigger = useRef<HTMLButtonElement>(null);
    const restoreThemeFocus = useRef(false);
    const leaveThemePicker = () => {
        restoreThemeFocus.current = true;
        setPickingTheme(false);
    };
    useEffect(() => {
        if (!pickingTheme && restoreThemeFocus.current) {
            restoreThemeFocus.current = false;
            themeTrigger.current?.focus();
        }
    }, [pickingTheme]);
    // null shows the mobile category landing. Desktop keeps the last detail
    // visible; CSS alone switches layouts, so resizing never resets drafts/flows.
    const [page, setPage] = useState<Page | null>(
        initialProviderId ? "Providers" : null,
    );
    const lastPage = useRef<Page | null>(null);
    const categoryTrigger = useRef<HTMLButtonElement>(null);
    const pageTitle = useRef<HTMLHeadingElement>(null);
    const content = useRef<HTMLElement>(null);
    const providerEntry = useRef(initialProviderId);
    useEffect(() => {
        if (content.current) content.current.scrollTop = 0;
        if (page) pageTitle.current?.focus();
        else if (lastPage.current) categoryTrigger.current?.focus();
    }, [page]);
    const backToCategories = () => {
        lastPage.current = page;
        providerEntry.current = undefined;
        setConnecting(false);
        setPage(null);
    };
    const selectedPage = page || lastPage.current || "General";
    const [connecting, setConnecting] = useState(!!initialProviderId);
    const ref = useRef<HTMLDialogElement>(null);
    useEffect(() => {
        const dialog = ref.current!;
        const previous = document.activeElement;
        dialog.showModal();
        return () => {
            dialog.close();
            if (previous instanceof HTMLElement) previous.focus();
        };
    }, []);
    const field = (key: keyof Preferences, value: string) =>
        set((d) => ({ ...d, [key]: value }));
    const icons = {
        General: SlidersHorizontal,
        Appearance: Palette,
        Servers: Server,
        Providers: Plug,
    };
    const descriptions = {
        General: "Profile and display name",
        Appearance: "Interface font, code font, and theme",
        Servers: "Join shared workspaces and configure agent access",
        Providers: "Connect providers and manage accounts",
    };
    const nav = (name: Page) => {
        const Icon = icons[name];
        return (
            <button
                type="button"
                key={name}
                ref={lastPage.current === name ? categoryTrigger : undefined}
                aria-label={name}
                aria-current={selectedPage === name ? "page" : undefined}
                onClick={() => {
                    setPickingTheme(false);
                    setPage(name);
                    setConnecting(false);
                }}
            >
                <Icon size={20} aria-hidden="true" />
                <span>{name}<small>{descriptions[name]}</small></span>
                <ChevronRight size={18} aria-hidden="true" />
            </button>
        );
    };
    return (
        <dialog
            ref={ref}
            className={`settings-dialog${connecting && selectedPage === "Providers" ? " settings-connecting" : ""}`}
            aria-label={connecting ? "Connect provider" : "Settings"}
            onKeyDown={(e) => {
                if (pickingTheme && e.key === "Escape") {
                    e.preventDefault();
                    e.stopPropagation();
                    leaveThemePicker();
                }
            }}
            onCancel={(e) => {
                e.preventDefault();
                if (pickingTheme) leaveThemePicker();
                else close();
            }}
            onClick={(e) => {
                if (e.target === e.currentTarget) {
                    const r = e.currentTarget.getBoundingClientRect();
                    if (
                        e.clientX < r.left ||
                        e.clientX > r.right ||
                        e.clientY < r.top ||
                        e.clientY > r.bottom
                    )
                        close();
                }
            }}
        >
            <div className={`settings-layout${page ? " settings-detail" : " settings-landing"}`}>
                <nav className="settings-nav" aria-label="Settings categories">
                    <header className="settings-landing-header">
                        <h2>Settings</h2>
                        <button type="button" className="settings-close" aria-label="Close settings" onClick={close}>
                            <X size={20} aria-hidden="true" />
                        </button>
                    </header>
                    <div>
                        <div className="settings-category-group">{nav("General")}{nav("Appearance")}{nav("Servers")}{nav("Providers")}</div>
                    </div>
                    <footer><span>Neoism</span><span>v{version}</span></footer>
                </nav>
                <main className="settings-content" ref={content}>
                    <header className="settings-page-header">
                        {(page || pickingTheme) && !connecting && (
                            <button type="button" className={`settings-back${pickingTheme ? " settings-theme-back" : ""}`}
                                aria-label={pickingTheme ? "Back to Appearance" : "Back to settings categories"}
                                onClick={pickingTheme ? leaveThemePicker : backToCategories}>
                                <ArrowLeft size={20} aria-hidden="true" />
                            </button>
                        )}
                        <h2 ref={pageTitle} tabIndex={-1}>{connecting ? "Connect provider" : pickingTheme ? "Theme" : selectedPage}</h2>
                        <button type="button" className="settings-close" aria-label="Close settings" onClick={close}>
                            <X size={20} aria-hidden="true" />
                        </button>
                    </header>
                    <div className="settings-page-body">
                        {pickingTheme ? (
                            <ThemePicker
                                selected={draft.theme}
                                query={filter}
                                search={setFilter}
                                back={leaveThemePicker}
                                showBack={false}
                                choose={(id) => {
                                    field("theme", id);
                                    leaveThemePicker();
                                }}
                            />
                        ) : selectedPage === "Providers" ? (
                            <ProviderConnections
                                shared={!!joinedDaemon(value.server)}
                                client={client}
                                directory={value.directory}
                                onSelectConnection={onSelectConnection}
                                selectedConnection={selectedConnection}
                                workspaceId={workspaceId}
                                initialProviderId={providerEntry.current}
                                onFlowChange={setConnecting}
                            />
                        ) : selectedPage === "Servers" ? (
                            <ServerConnections value={value} token={token} connected={connected} forget={forgetServer} credentialFor={serverCredential}
                                join={(server, credential, directory = "") => {
                                    save({ ...draft, server, directory }, credential);
                                    close();
                                }} />
                        ) : (
                            <form
                                id="settings-preferences"
                                onSubmit={(e) => {
                                    e.preventDefault();
                                    save(draft, token);
                                    close();
                                }}
                            >
                                {selectedPage === "General" && (
                                    <section className="settings-section">
                                        <h3>Profile</h3>
                                        <div className="settings-list">
                                            <label className="settings-field-row">
                                                <span>
                                                    Display name
                                                    <small>
                                                        Your identity on this
                                                        device.
                                                    </small>
                                                </span>
                                                <input
                                                    value={draft.name}
                                                    maxLength={80}
                                                    onChange={(e) =>
                                                        field(
                                                            "name",
                                                            e.target.value,
                                                        )
                                                    }
                                                />
                                            </label>
                                        </div>
                                    </section>
                                )}
                                {selectedPage === "Appearance" && (
                                    <section className="settings-section">
                                        <h3>Interface</h3>
                                        <div className="settings-list">
                                            <label className="settings-field-row">
                                                <span>
                                                    UI font
                                                    <small>Default: Geist</small>
                                                </span>
                                                <select
                                                    value={draft.font}
                                                    onChange={(e) =>
                                                        field(
                                                            "font",
                                                            e.target.value,
                                                        )
                                                    }
                                                >
                                                    {[
                                                        ...systemFontOptions,
                                                        ...fonts,
                                                    ].map((font) => (
                                                        <option
                                                            key={font.id}
                                                            value={font.id}
                                                        >
                                                            {font.name}
                                                        </option>
                                                    ))}
                                                </select>
                                            </label>
                                            <label className="settings-field-row">
                                                <span>Code font<small>Default: JetBrains Mono</small></span>
                                                <select aria-label="Code font" value={draft.codeFont || "jetbrains-mono"}
                                                    onChange={event => field("codeFont", event.target.value)}>
                                                    {[...fonts, ...systemFontOptions].filter(font => font.kind === "monospace").map(font =>
                                                        <option key={font.id} value={font.id}>{font.name}</option>
                                                    )}
                                                </select>
                                            </label>
                                            <button
                                                type="button"
                                                ref={themeTrigger}
                                                className="settings-field-row settings-theme-trigger"
                                                onClick={() => {
                                                    setFilter("");
                                                    setPickingTheme(true);
                                                }}
                                            >
                                                <span>Theme</span>
                                                <span>
                                                    {themeOptions.find(
                                                        (t) =>
                                                            t.id ===
                                                            draft.theme,
                                                    )?.name || draft.theme}
                                                </span>
                                                <ChevronRight size={16} />
                                            </button>
                                        </div>
                                    </section>
                                )}

                                <footer className="settings-save">
                                    <button type="button" onClick={close}>
                                        Cancel
                                    </button>
                                    <button type="submit">Save settings</button>
                                </footer>
                            </form>
                        )}
                    </div>
                </main>
            </div>
        </dialog>
    );
}
