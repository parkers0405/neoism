import { useCallback, useLayoutEffect, useRef, useState } from "react";
import { ChatTabs } from "./components/ChatTabs";
import { FolderPicker } from "./components/FolderPicker";
import { serverScope } from "./types";
import { ProviderConnections } from "./components/ProviderConnections";
import { McpPicker } from "./components/McpPicker";
import { FxScene } from "./components/FxScene";
import { isFxKind } from "./fx";
import { ArrowLeft, Menu, PanelLeft, PanelRight, X } from "lucide-react";
import { subagentView } from "./subagent-view";
import { ConversationSkeleton } from "./components/Skeleton";
import "./subagent-view.css";
import { useAppController } from "./useAppController";
import { Navigation } from "./components/Navigation";
import { ChatDetails } from "./components/ChatDetails";
import { Wordmark } from "./components/Identity";
import { Settings } from "./components/Settings";
import { Composer, ComposerFooter } from "./components/Composer";
import { ComposerPanel } from "./components/ComposerPanel";
import { Picker } from "./components/Picker";
import { Timeline } from "./components/Timeline";
import { Modal } from "./components/Modal";
import { Library } from "./components/Library";
import { Interactions } from "./components/Interactions";
export interface DockPosition {
    top: number; left: number; width: number; viewportHeight: number; viewportWidth: number;
    home: boolean; tabKey: string;
}
/** Only dock a newly sent home tab; navigation/viewport changes are not motion. */
export function dockingTranslation(before: DockPosition | undefined, after: DockPosition, reducedMotion: boolean) {
    if (reducedMotion || !before?.home || after.home || before.tabKey !== after.tabKey ||
        before.viewportWidth !== after.viewportWidth || before.viewportHeight !== after.viewportHeight ||
        ![before.viewportWidth, before.viewportHeight, before.width, after.width].every(Number.isFinite) ||
        before.width <= 0 || after.width <= 0) return undefined;
    const x = before.left - after.left, y = before.top - after.top;
    return Number.isFinite(x) && Number.isFinite(y) && Math.hypot(x, y) > 1 ? { x, y } : undefined;
}

export function App() {
    const a = useAppController();
    const { canCompose, metadataLoading, isChild, returnId, backLabel } = subagentView(a.id, a.active, a.chat.state.runtime);
    const sessionOpener = useRef(a.openSession);
    useLayoutEffect(() => { sessionOpener.current = a.openSession; }, [a.openSession]);
    const openTranscriptSession = useCallback((id: string) => sessionOpener.current(id), []);
    // Controller nav is the mobile drawer; desktop collapse must survive chat navigation.
    const [desktopNavVisible, setDesktopNavVisible] = useState(() => {
        try { return localStorage.getItem("neoism.desktop-nav-visible") !== "false"; }
        catch { return true; }
    });
    const toggleDesktopNav = () => {
        const visible = !desktopNavVisible;
        setDesktopNavVisible(visible);
        try { localStorage.setItem("neoism.desktop-nav-visible", String(visible)); }
        catch { /* Storage may be unavailable in private/embedded contexts. */ }
    };
    const appRoot = useRef<HTMLDivElement>(null);
    const chatMain = useRef<HTMLDivElement>(null);
    useLayoutEffect(() => {
        const viewport = window.visualViewport;
        const root = appRoot.current;
        if (!viewport || !root) return;
        const resize = () => {
            if (viewport.scale === 1) root.style.setProperty("--app-viewport-height", `${viewport.height}px`);
        };
        resize();
        viewport.addEventListener("resize", resize);
        return () => viewport.removeEventListener("resize", resize);
    }, []);
    const composerDock = useRef<HTMLDivElement>(null);
    const composerContent = useRef<HTMLDivElement>(null);
    const dockPosition = useRef<DockPosition | undefined>(undefined);
    const dockAnimation = useRef<Animation | undefined>(undefined);
    const sentFromHome = useRef(false);
    const position = (): DockPosition | undefined => {
        const dock = composerDock.current, main = chatMain.current;
        if (!dock || !main) return undefined;
        const rect = dock.getBoundingClientRect();
        return { top: rect.top, left: rect.left, width: rect.width,
            viewportHeight: window.visualViewport?.height ?? window.innerHeight,
            viewportWidth: window.visualViewport?.width ?? window.innerWidth,
            home: !a.id, tabKey: a.tabKey };
    };
    useLayoutEffect(() => {
        const cancel = () => {
            dockAnimation.current?.cancel(); dockAnimation.current = undefined;
            sentFromHome.current = false;
        };
        const motion = window.matchMedia?.("(prefers-reduced-motion: reduce)");
        window.addEventListener("resize", cancel);
        window.visualViewport?.addEventListener("resize", cancel);
        motion?.addEventListener("change", cancel);
        return () => {
            cancel();
            window.removeEventListener("resize", cancel);
            window.visualViewport?.removeEventListener("resize", cancel);
            motion?.removeEventListener("change", cancel);
        };
    }, []);
    useLayoutEffect(() => {
        const main = chatMain.current, dock = composerDock.current, content = composerContent.current;
        if (!main || !dock) return;
        const measure = () => {
            // Cap only the input/interaction child, never the external picker anchor.
            const footer = main.querySelector<HTMLElement>(".composer-footer-dock");
            const footerHeight = footer ? footer.getBoundingClientRect().height + 10 : 0;
            main.style.setProperty("--composer-footer-height", `${footerHeight}px`);
            const heading = main.querySelector<HTMLElement>(".home-heading");
            const homeReserve = heading ? heading.getBoundingClientRect().height + 44 + 64 : 0;
            const cap = Math.max(0, Math.min(main.clientHeight * 0.65 - 20,
                main.clientHeight - homeReserve - 40) - footerHeight);
            main.style.setProperty("--composer-max-height", `${cap}px`);
            content?.classList.toggle("is-overflowing", content.scrollHeight > cap + 1);
            if (!a.id && !sentFromHome.current) dockPosition.current = position();
            main.style.setProperty("--composer-clearance", `${Math.ceil(dock.getBoundingClientRect().height + footerHeight)}px`);
        };
        measure();
        const observer = typeof ResizeObserver !== "undefined" ? new ResizeObserver(measure) : undefined;
        observer?.observe(main);
        observer?.observe(dock);
        if (content) observer?.observe(content);
        const footer = main.querySelector(".composer-footer-dock");
        if (footer) observer?.observe(footer);
        // Observe natural content too, including changes while the scroll child is capped.
        for (const child of content?.children ?? []) observer?.observe(child);
        window.addEventListener("resize", measure);
        window.visualViewport?.addEventListener("resize", measure);
        return () => {
            observer?.disconnect();
            window.removeEventListener("resize", measure);
            window.visualViewport?.removeEventListener("resize", measure);
            main.style.removeProperty("--composer-clearance");
            main.style.removeProperty("--composer-max-height");
            main.style.removeProperty("--composer-footer-height");
            content?.classList.remove("is-overflowing");
        };
    }, [a.id, a.tabKey, a.view, a.skipPermissions, canCompose, metadataLoading]);
    useLayoutEffect(() => {
        // Measure actual laid-out endpoints. No remount, timeout, synthetic layout,
        // or height/scale animation touching the textarea's caret and IME.
        dockAnimation.current?.cancel();
        dockAnimation.current = undefined;
        const next = position();
        if (next && sentFromHome.current && canCompose) {
            const delta = dockingTranslation(dockPosition.current, next,
                window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false);
            if (delta && composerDock.current?.animate) {
                dockAnimation.current = composerDock.current.animate([
                    { transform: `translate(${delta.x}px, ${delta.y}px)` },
                    { transform: "translate(0, 0)" },
                ], { duration: 260, easing: "cubic-bezier(.2,.8,.2,1)" });
            }
        }
        sentFromHome.current = false;
        dockPosition.current = next;
    }, [a.id, a.tabKey, a.view, canCompose]);
    return (
        <div ref={appRoot} className={`app ${a.nav ? "nav-open" : ""} ${desktopNavVisible ? "" : "nav-hidden"} effect-${a.effect}`}>
            {isFxKind(a.effect) && <FxScene key={`${a.effect}:${a.effectRevision}:${a.id}:${a.prefs.server}:${a.directory}`} kind={a.effect} />}
            <header className="app-chrome">
                <div className="navigation-chrome">
                    <button type="button" className="desktop-nav-toggle"
                        aria-label={desktopNavVisible ? "Hide navigation" : "Show navigation"}
                        aria-expanded={desktopNavVisible} aria-controls="app-navigation"
                        onClick={toggleDesktopNav}>
                        <PanelLeft size={19} />
                    </button>
                    <button
                        className="mobile-menu"
                        aria-label="Navigation"
                        aria-expanded={a.nav}
                        aria-controls="app-navigation"
                        onClick={() => a.setNav((x) => !x)}
                    >
                        <Menu size={20} />
                    </button>
                </div>
                <div className="topbar">
                    {a.view === "chat" ? <ChatTabs tabs={a.tabs} active={a.tabKey} activate={a.activateTab} close={a.closeTab} add={a.newChat} /> : <><span>{a.view === "skills" ? "Skills" : "Workflows"}</span><span className="spacer" /></>}
                    {a.id && a.view === "chat" && (
                        <button
                            aria-label="Toggle chat details"
                            onClick={() => a.setSidebar((x) => !x)}
                        >
                            <PanelRight size={19} />
                        </button>
                    )}
                </div>
            </header>
            <Navigation app={a} />
            {a.nav && (
                <button className="nav-scrim" aria-label="Close navigation" onClick={() => a.setNav(false)} />
            )}
            <main>
                {(a.error || a.chat.state.error) && (
                    <div className="error-banner" role="alert">
                        <span>{a.error || a.chat.state.error}</span>
                        <button onClick={() => a.setSettings(true)}>
                            Settings
                        </button>
                        <button
                            aria-label="Dismiss error"
                            onClick={() => {
                                a.setError("");
                                a.chat.setState((s) => ({
                                    ...s,
                                    error: undefined,
                                }));
                            }}
                        >
                            <X size={15} />
                        </button>
                    </div>
                )}
                {a.view !== "chat" ? (
                    <Library
                        key={[
                            a.view,
                            a.prefs.server,
                            a.token,
                        ].join(":")}
                        kind={a.view}
                        client={a.client}
                        openSession={a.openSession}
                    />
                ) : (
                    <div className="chat-layout" role="tabpanel" id={`chat-panel-${a.tabKey}`} aria-labelledby={`chat-tab-${a.tabKey}`}>
                        <div ref={chatMain} className={`chat-main ${!a.id ? "home" : ""} ${canCompose ? "" : "without-composer"}`}>
                            {isChild && returnId && <div className="subagent-view-hint">
                                <button type="button" onClick={() => openTranscriptSession(returnId)}>
                                    <ArrowLeft size={15} aria-hidden="true" />{backLabel}
                                </button>
                                <span>Subagent</span>
                            </div>}
                            {!a.id ? (
                                <div className="home-heading">
                                    <Wordmark />
                                </div>
                            ) : metadataLoading ? (
                                <div className="timeline chat-timeline"><div className="transcript"><ConversationSkeleton /></div></div>
                            ) : (
                                <Timeline client={a.client}
                                    activityBusy={a.chat.activityBusy ?? false}
                                    sessionId={a.id}
                                    runtime={a.chat.state.runtime}
                                    sessionActivity={a.chat.sessionActivity}
                                    activityPalette={a.activityPalette}
                                    messages={a.chat.state.messages}
                                    busy={a.chat.state.busy}
                                    older={a.chat.older}
                                    loading={a.chat.loading}
                                    liveParts={a.chat.liveParts}
                                    onOpenSession={openTranscriptSession}
                                    loadOlder={a.chat.loadOlder}
                                />
                            )}
                             {canCompose && <div ref={composerDock} className="composer-dock">
                                  <div className="composer-anchor">
                                {a.picker && a.picker !== "connect" && a.picker !== "directory" && (
                                    <Picker
                                        key={a.picker}
                                        title={a.picker === "skill" ? "Skills" : a.picker === "model" ? "Models" : a.picker === "thinking" ? "Thinking effort" : a.picker === "agent" ? "Agents" : a.picker === "subagents" ? "Subagents" : "Sessions"}
                                        choices={a.choices}
                                        loading={a.pickerLoading}
                                        close={() => a.setPicker(undefined)}
                                        choose={a.choose}
                                    />
                                )}
                                {a.info?.title === "MCP servers" && !a.picker && (
                                    <ComposerPanel title="MCP servers" close={() => a.setInfo(undefined)}>
                                        <McpPicker key={`${a.prefs.server}:${a.directory}:${a.token}:${a.id}`} client={a.client} directory={a.directory} />
                                    </ComposerPanel>
                                )}
                                {a.picker === "connect" && (
                                    <ComposerPanel title="Connect provider account" close={() => a.setPicker(undefined)}>
                                        <ProviderConnections
                                            key={`${a.prefs.server}:${a.token}:${a.id}:${a.directory}`}
                                            client={a.client}
                                            directory={a.directory}
                                            workspaceId={a.active?.workspaceId}
                                            initialProviderId={a.model.split("/")[0] || undefined}
                                            selectedConnection={a.selectedConnection}
                                            onSelectConnection={a.onSelectConnection}
                                        />
                                    </ComposerPanel>
                                )}
                                <div ref={composerContent} className="composer-content">
                                <div className="composer-content-inner">
                                {a.skipPermissions && (
                                    <div className="error-banner">
                                        Permission checks are bypassed for this open chat.
                                        <button onClick={() => a.setSkipPermissions(false)}>Re-enable checks</button>
                                    </div>
                                )}
                                {a.id && (
                                    <Interactions client={a.client} id={a.id} notify={a.notify} skipPermissions={a.skipPermissions} />
                                )}
                                <Composer
                                    key={`${a.prefs.server}:${a.tabKey}`}
                                    showFooter={false}
                                    tabKey={a.tabKey}
                                    draft={a.draft}
                                    onDraftChange={a.onDraftChange}
                                    files={a.files}
                                    onFilesChange={a.onFilesChange}
                                    directory={a.directory}
                                    client={a.client}
                                    sessionId={a.id}
                                    recentDirectories={a.recentDirectories}
                                    projectStorageScope={serverScope(a.prefs.server)}
                                    onDirectoryChange={a.setDirectory}
                                    onCycleAgent={a.onCycleAgent}
                                    showHints={a.hints}
                                    busy={a.chat.state.busy}
                                    commands={a.catalog}
                                    send={(text, files) => {
                                        // Snapshot before the controller creates the session/sidebar.
                                        // Never measure the starting rect in an after-commit effect.
                                        const before = !a.id ? position() : undefined;
                                        if (before) { dockPosition.current = before; sentFromHome.current = true; }
                                        return a.send(text, files).catch(error => {
                                            if (dockPosition.current === before) sentFromHome.current = false;
                                            throw error;
                                        });
                                    }}
                                    abort={() =>
                                        void a.perform(() =>
                                            a.id
                                                ? a.client.sessions.abort(a.id)
                                                : Promise.resolve(),
                                        )
                                    }
                                    model={a.model}
                                    agent={
                                        a.agent.charAt(0).toUpperCase() +
                                        a.agent.slice(1)
                                    }
                                    thinking={a.thinking}
                                    openPicker={a.togglePicker}
                                />
                                </div>
                                </div>
                                 </div>
                             </div>}
                             {canCompose && !a.id && <div className="composer-footer-dock">
                                <ComposerFooter
                                    showProject={!a.id}
                                    busy={!!a.id && a.chat.state.busy}
                                    key={`${a.prefs.server}:${a.tabKey}`}
                                    tabKey={a.tabKey}
                                    client={a.client}
                                    sessionId={a.id}
                                    directory={a.directory}
                                    recentDirectories={a.recentDirectories}
                                    projectStorageScope={serverScope(a.prefs.server)}
                                    onDirectoryChange={a.setDirectory}
                                    showHints={a.hints}
                                />
                            </div>}
                        </div>
                        {a.id && a.sidebar && <ChatDetails app={a} />}
                    </div>
                )}
            </main>
            {canCompose && a.picker === "directory" && (
                <FolderPicker
                    key={`${a.prefs.server}:${a.tabKey}`}
                    client={a.client}
                    directory={a.directory}
                    recentDirectories={a.recentDirectories}
                    projectStorageScope={serverScope(a.prefs.server)}
                    select={a.setDirectory}
                    close={() => a.setPicker(undefined)}
                />
            )}
            {a.settings && (
                <Settings
                    client={a.client}
                    value={a.prefs}
                    token={a.token}
                    onSelectConnection={a.onSelectConnection}
                    selectedConnection={a.selectedConnection}
                    workspaceId={a.active?.workspaceId}
                    save={a.saveSettings}
                    close={() => a.setSettings(false)}
                />
            )}
            {a.info && a.info.title !== "MCP servers" && (
                <Modal title={a.info.title} close={() => a.setInfo(undefined)}>
                    <pre className="result">{a.info.body || "Command completed."}</pre>
                </Modal>
            )}
        </div>
    );
}
