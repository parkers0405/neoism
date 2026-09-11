import type { NeoismClient } from "@neoism/sdk";
import { useEffect, useRef, useState } from "react";
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
}: {
    selected: string;
    query: string;
    search(value: string): void;
    choose(id: string): void;
    back(): void;
}) {
    const matches = themeOptions.filter((theme) =>
        `${theme.name} ${theme.id}`
            .toLowerCase()
            .includes(query.trim().toLowerCase()),
    );
    return (
        <section className="settings-theme-picker" aria-label="Theme picker">
            <input
                autoFocus
                type="search"
                aria-label="Search themes"
                placeholder="Search themes…"
                value={query}
                onChange={(e) => search(e.target.value)}
            />
            <button type="button" className="theme-back" onClick={back}>
                <ArrowLeft size={16} />
                Back to Appearance
            </button>
            <div className="settings-theme-results" aria-label="Themes">
                {matches.map((theme) => (
                    <button
                        type="button"
                        key={theme.id}
                        aria-pressed={selected === theme.id}
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
        </section>
    );
}

type Page = "General" | "Appearance" | "Servers" | "Providers";
export function Settings({
    client,
    value,
    token,
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
    save(p: Preferences, t: string): void;
    close(): void;
} & ProviderConnectionPickerProps) {
    const [draft, set] = useState(value);
    const [secret, setSecret] = useState(token);
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
    const [page, setPage] = useState<Page>(
        initialProviderId ? "Providers" : "General",
    );
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
    const nav = (name: Page) => {
        const Icon = icons[name];
        return (
            <button
                type="button"
                key={name}
                aria-current={page === name ? "page" : undefined}
                onClick={() => {
                    setPickingTheme(false);
                    setPage(name);
                    setConnecting(false);
                }}
            >
                <Icon size={16} />
                {name}
            </button>
        );
    };
    return (
        <dialog
            ref={ref}
            className={`settings-dialog${connecting && page === "Providers" ? " settings-connecting" : ""}`}
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
            <div className="settings-layout">
                <nav className="settings-nav" aria-label="Settings">
                    <div>
                        <h3>Desktop</h3>
                        {nav("General")}
                        {nav("Appearance")}
                        <h3>Server</h3>
                        {nav("Servers")}
                        {nav("Providers")}
                    </div>
                    <footer>
                        <span>Neoism</span>
                        <span>v{version}</span>
                    </footer>
                </nav>
                <main className="settings-content">
                    <button
                        type="button"
                        className="settings-close"
                        aria-label="Close settings"
                        onClick={close}
                    >
                        <X size={16} />
                    </button>
                    <header className="settings-page-header">
                        <h2>{pickingTheme ? "Theme" : page}</h2>
                    </header>
                    <div className="settings-page-body">
                        {pickingTheme ? (
                            <ThemePicker
                                selected={draft.theme}
                                query={filter}
                                search={setFilter}
                                back={leaveThemePicker}
                                choose={(id) => {
                                    field("theme", id);
                                    leaveThemePicker();
                                }}
                            />
                        ) : page === "Providers" ? (
                            <ProviderConnections
                                client={client}
                                directory={value.directory}
                                onSelectConnection={onSelectConnection}
                                selectedConnection={selectedConnection}
                                workspaceId={workspaceId}
                                initialProviderId={initialProviderId}
                                onFlowChange={setConnecting}
                            />
                        ) : (
                            <form
                                id="settings-preferences"
                                onSubmit={(e) => {
                                    e.preventDefault();
                                    save(draft, secret);
                                    close();
                                }}
                            >
                                {page === "General" && (
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
                                {page === "Appearance" && (
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
                                {page === "Servers" && (
                                    <section className="settings-section">
                                        <h3>Connection</h3>
                                        <div className="settings-list">
                                            <label className="settings-field-row">
                                                <span>Server URL</span>
                                                <input
                                                    required
                                                    type="url"
                                                    value={draft.server}
                                                    onChange={(e) => {
                                                        if (e.target.value !== draft.server) setSecret("");
                                                        field("server", e.target.value);
                                                    }}
                                                />
                                            </label>
                                            <label className="settings-field-row">
                                                <span>
                                                    Bearer token
                                                    <small>
                                                        Kept only in memory
                                                    </small>
                                                </span>
                                                <input
                                                    type="password"
                                                    autoComplete="off"
                                                    value={secret}
                                                    placeholder="Optional"
                                                    onChange={(e) =>
                                                        setSecret(
                                                            e.target.value,
                                                        )
                                                    }
                                                />
                                            </label>
                                            <label className="settings-field-row">
                                                <span>Workspace directory</span>
                                                <input
                                                    value={draft.directory}
                                                    placeholder="Server default"
                                                    onChange={(e) =>
                                                        field(
                                                            "directory",
                                                            e.target.value,
                                                        )
                                                    }
                                                />
                                            </label>
                                        </div>
                                        <p className="settings-note">
                                            Use HTTPS for remote servers.
                                            Cross-origin servers must allow this
                                            app’s origin.
                                        </p>
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
