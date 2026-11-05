import { createEffect, createSignal, Show, untrack } from "solid-js";
import { TerminalView, type TerminalViewProps } from "../terminal/TerminalView";
import { SplashView } from "./SplashView";
import type { WordmarkHosts } from "./AnimatedWordmark";

export function HomeTerminalView(props: Omit<TerminalViewProps, "sessionId" | "historyHeader"> & {
  sessionId?: string;
  contextId: string;
  wordmarkHosts?: WordmarkHosts;
  ensureSession: () => Promise<void>;
  onAction: (action: string) => void;
}) {
  const [starting, setStarting] = createSignal(false);
  const start = () => {
    if (starting()) return;
    setStarting(true);
    void props.ensureSession().catch(props.report).finally(() => setStarting(false));
  };
  createEffect(() => { if (!props.sessionId) untrack(start); });
  const splash = (viewport?: { width: number; height: number }) => <SplashView
    contextId={props.contextId} config={props.config} wordmarkHosts={props.wordmarkHosts} embedded={!!viewport}
    initialViewport={viewport ? { width: Math.max(0, viewport.width - 80 * props.config.scale), height: Math.max(0, viewport.height - 16 * props.config.scale) } : undefined}
    active={props.active} focusToken={props.focusToken} onCustomKey={props.onCustomKey} onFocus={props.onFocus} onAction={props.onAction}/>;
  return <div testId="home-terminal" style={{ position: "relative", flexGrow: 1, minWidth: 0, minHeight: 0, display: "flex" }}>
    <Show when={props.sessionId} keyed fallback={<>
      {splash()}
      <Show when={!starting()}><div role="button" testId="home-start-shell" tabIndex={0} onClick={start} onKeyDown={event => { if (event.key === "enter") start(); }}
        style={{ position: "absolute", bottom: 16 * props.config.scale, left: 16 * props.config.scale, cursor: "pointer" }}>
        <text style={{ color: props.config.theme.dim, fontFamily: props.config.fontFamily, fontSize: props.config.fontSize }}>Start shell</text>
      </div></Show>
    </>}>{sessionId => <TerminalView {...props} sessionId={sessionId} historyHeader={splash}/>}</Show>
  </div>;
}