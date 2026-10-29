import { createEffect,createSignal, onCleanup, onMount } from "solid-js";
import { createWindowSize,useGpuixRequired, type HostElement, type KeyEvent } from "@gpuix/solid";
import type { AppConfig } from "../config";
import { coreCall } from "../native-core";
import { PaintLayer, type PaintCommand } from "../ui/PaintLayer";
import { ANIMATION_FRAME_MS, AnimationFrameScheduler } from "../animation/frame-scheduler";
import { focusNativeElement } from "../native-focus";
import {useDialogViewport,overlayPoint} from "../ui/overlay-viewport";
import {handleApplicationShortcut} from "../commands/application-shortcuts";

interface BrowserFrame { commands: PaintCommand[]; animating: boolean; active: boolean; selection?: string | null }

/** The desktop directory chooser (shared `FileBrowserModal`): locations rail,
 * recents, path entry, folder listing and keyboard navigation, painted by the
 * original renderer over the whole window. */
export function DirectoryBrowserView(props: { config: AppConfig; start: string; root: string; onDone: (path: string | undefined) => void }) {
  const config = props.config, renderer = useGpuixRequired();
  const viewport=useDialogViewport(renderer,createWindowSize());
  const { id } = coreCall<{ id: number }>("dirbrowser.new", { start: props.start, root: props.root, theme: config.themeName, scale: config.scale, fontFamily: config.fontFamily, fontWeight: config.fontWeight });
  const [commands, setCommands] = createSignal<readonly PaintCommand[]>([]);
  let size = { width: 0, height: 0 }, host: HostElement | undefined, done = false;
  const animation = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  const finish = (path: string | undefined) => { if (done) return; done = true; props.onDone(path); };
  const call = (op: string, args: Record<string, unknown> = {}) => {
    const started = performance.now(); animation.cancel();
    const next = coreCall<BrowserFrame>(`dirbrowser.${op}`, { id, width: size.width, height: size.height, ...args });
    setCommands(next.commands);
    if (next.selection) { finish(next.selection); return; }
    if (!next.active) { finish(undefined); return; }
    if (next.animating) animation.request(() => { if (!done) call("frame"); }, started);
  };
  createEffect(()=>{const next=viewport();if(next.width!==size.width||next.height!==size.height){size={width:next.width,height:next.height};call("frame");}});
  onMount(() => { if (host) focusNativeElement(renderer, host.id); });
  onCleanup(() => { animation.dispose(); coreCall("dirbrowser.dispose", { id }); });
  const keyDown = (event: KeyEvent) => {
    if(handleApplicationShortcut(renderer,event))return;
    event.preventDefault(); event.stopPropagation();
    call("key", { key: event.key || "", char: event.keyChar || "" });
  };
  return <anchored fill="window" deferred={true} priority={3} occlude={true} style={{ backgroundColor: "transparent" }}>
    <div ref={host} tabIndex={0} testId="directory-browser" aria-label="Choose directory" onKeyDown={keyDown}
      onLayout={(event) => { const next = { width: event.width || 0, height: event.height || 0 }; if (next.width !== size.width || next.height !== size.height) { size = next; call("frame"); } }}
      onMouseDown={(event) => call("pointer", { ...overlayPoint(viewport(),event), clicks: event.clickCount || 1 })}
      onScroll={(event) => call("scroll", { delta: -(event.deltaY || 0) * (event.precise ? 1 : 40) })}
      style={{ position: "absolute", left: viewport().x, top: viewport().y, width: viewport().width, height: viewport().height, userSelect: "none" }}>
      <PaintLayer commands={commands()} config={config} />
    </div>
  </anchored>;
}
