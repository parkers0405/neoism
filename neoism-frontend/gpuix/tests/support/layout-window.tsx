// Isolated native interaction fixture, using the production layout component.
import { createRenderer, render } from "@gpuix/solid";
import { startFrameLoop } from "@gpuix/native/runtime";
import { createSignal } from "solid-js";
import { loadConfig } from "../../src/config";
import { PaneLayoutView } from "../../src/layout/PaneLayoutView";
import { WorkspaceLayout } from "../../src/layout/WorkspaceLayout";
const renderer = createRenderer();
renderer.init({ title: "Neoism layout verification", appId: "ai.neoism.gpuix.layout-check", width: 1200, height: 800 });
const loop = startFrameLoop(renderer);
const layout = new WorkspaceLayout();
layout.open({ id: "a", title: "a.rs", kind: "file" });
layout.open({ id: "b", title: "b.rs", kind: "file" });
layout.split("horizontal", { id: "shell", title: "Terminal", kind: "terminal" });
const root = render(() => {
  const [focused, setFocused] = createSignal("shell");
  return <div style={{ width: "100%", height: "100%", display: "flex", flexDirection: "column", backgroundColor: "#111" }}>
    <text testId="layout-focus" style={{ color: "#fff" }}>{focused()}</text>
    <PaneLayoutView controller={layout} config={loadConfig(["--config", "/nonexistent", "--font-size", "14"], {})}
      renderSurface={(id) => <text testId={`surface:${id}`} style={{ color: "#fff" }}>{id}</text>}
      onFocus={(id) => setFocused(id)} onClose={(_id, leaf) => { layout.close(leaf); setFocused(layout.frame.focusedSurfaceId || "empty"); }} />
  </div>;
}, { renderer });
const stop = () => { root.unmount(); layout.dispose(); loop.stop(); process.exit(0); };
process.once("SIGINT", stop); process.once("SIGTERM", stop);
