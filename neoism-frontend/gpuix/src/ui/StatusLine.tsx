import { AnimationFrameScheduler, ANIMATION_FRAME_MS } from "../animation/frame-scheduler";
import { createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import { createWindowSize, useGpuixRequired } from "@gpuix/solid";
import type { AppConfig } from "../config";
import type { DaemonConnection } from "../daemon/client";
import { isRecord } from "../daemon/protocol";
import {cachedGitStatus,watchGitStatus,type GitStatusSnapshot} from "../daemon/git-status";
import { coreCall } from "../native-core";
import { sharePaintCommands } from "../editor/paint-command";
import { PaintLayer, type PaintCommand } from "./PaintLayer";
import { measuredFrameRate, onFrameActivity } from "../chrome/frame-meter";

interface Frame { commands: PaintCommand[]; height: number; animating: boolean }
/** Uses the original StatusLine pill geometry, colors, scramble and hit tests. */
export function StatusLine(props: { config: AppConfig; info: Record<string, unknown>; client: DaemonConnection; root?: string | null; connected: boolean;
  splitEnabled: boolean; splitHidden: boolean; activity?: unknown; editorActivity?: unknown; onAction: (action: Record<string, unknown>) => void; report: (error: unknown) => void }) {
  const config = props.config, size = createWindowSize();
  const renderer = useGpuixRequired(); let paintedFps: number | undefined;
  const id = coreCall<{ id: number }>("chrome.new", { kind: "status", family: config.fontFamily, weight: config.fontWeight, fonts: [config.iconFontFamily] }).id;
  const [git, setGit] = createSignal<{ branch: string | null; git_changes: { added: number; deleted: number } | null }>({ branch: null, git_changes: null });
  const repository = createMemo(() => props.root);
  createEffect(() => {
    const root = repository();
    const publish=(snapshot:GitStatusSnapshot)=>{
      if(snapshot.error)return;
      let added=0,deleted=0;
      for(const file of snapshot.files)if(isRecord(file)){added+=typeof file.additions==="number"?file.additions:0;deleted+=typeof file.deletions==="number"?file.deletions:0;}
      setGit({branch:snapshot.branch,git_changes:snapshot.repo_root?{added,deleted}:null});
    };
    const cached=root?cachedGitStatus(props.client,root):undefined;
    if(cached)publish(cached);else setGit({branch:null,git_changes:null});
    if(root)onCleanup(watchGitStatus(props.client,root,publish));
  });
  const parameters = createMemo(() => ({ id, width: size().width, scale: config.scale, theme: config.themeName,
    splitEnabled: props.splitEnabled, splitHidden: props.splitHidden, info: { ...props.info, ...git() } }), undefined, { equals: (a, b) => JSON.stringify(a) === JSON.stringify(b) });
  const [frame, setFrame] = createSignal<Frame>({ commands: [], height: 22 * config.scale, animating: false });
  const animation = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  const draw = (branchHover?: boolean) => {
    try { const started = performance.now(); animation.cancel(); const data = parameters(); paintedFps = config.statusFps === false ? undefined : measuredFrameRate(renderer); const next = coreCall<Frame>("chrome.frame", { ...data, info: { ...data.info, fps: paintedFps ?? null }, ...(branchHover === undefined ? {} : { branchHover }) }); setFrame(previous => ({ ...next, commands: sharePaintCommands(previous.commands, next.commands) })); if (next.animating) animation.request(() => draw(), started); }
    catch (error) { props.report(error); }
  };
  createEffect(() => { parameters(); draw(); });
  onCleanup(onFrameActivity(renderer, () => { if (config.statusFps !== false && measuredFrameRate(renderer) !== paintedFps) draw(); }));
  createEffect(() => { props.activity; props.editorActivity; if (config.statusFps !== false && measuredFrameRate(renderer) !== paintedFps) draw(); });
  onCleanup(() => { animation.dispose(); coreCall("chrome.dispose", { id }); });
  let hovered = false, origin = { x: 0, y: 0 };
  const local = (x = 0, y = 0) => ({ x: x - origin.x, y: y - origin.y });
  return <div testId="original-statusline" onLayout={(event) => { origin = { x: event.x || 0, y: event.y || 0 }; }}
    onMouseMove={(event) => { const next = coreCall<{ branch: boolean }>("chrome.hit", { id, ...local(event.x, event.y) }).branch; if (next !== hovered) { hovered = next; draw(next); } }}
    onMouseLeave={() => { if (hovered) { hovered = false; draw(false); } }}
    onMouseDown={(event) => { if (event.button !== undefined && event.button !== 0) return; const hit = coreCall<{ action?: Record<string, unknown> }>("chrome.hit", { id, ...local(event.x, event.y) }); if (hit.action) props.onAction(hit.action); }}
    style={{ position: "relative", height: frame().height, minHeight: frame().height, flexShrink: 0 }}>
    <PaintLayer commands={frame().commands} config={config} />
  </div>;
}
