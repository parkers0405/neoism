// Production command palette and config-binding matcher in an isolated window.
import { createRenderer, render } from "@gpuix/solid";
import { startFrameLoop } from "@gpuix/native/runtime";
import { createSignal, Show } from "solid-js";
import { loadConfig } from "../../src/config";
import { CommandController } from "../../src/commands/CommandController";
import { CommandView } from "../../src/commands/CommandView";
import { KeyboardBindings } from "../../src/commands/KeyboardBindings";
const renderer = createRenderer();
renderer.init({ title: "Neoism command verification", appId: "ai.neoism.gpuix.command-check", width: 1200, height: 800 });
const loop = startFrameLoop(renderer), controller = new CommandController({ surface: "file" });
const bindings = new KeyboardBindings([{ key: "p", with: "control | shift", action: "OpenCommandPalette" }]);
const root = render(() => {
  const [open, setOpen] = createSignal(true), [action, setAction] = createSignal("");
  return <div style={{ width: "100%", height: "100%", backgroundColor: "#111" }}>
    <div testId="key-target" tabIndex={0} onKeyDown={(event) => {
      const matches = bindings.match(event); if (matches.length) { event.preventDefault(); event.stopPropagation(); }
      if (matches.some((m) => m.kind === "action" && m.action === "OpenCommandPalette")) setOpen(true);
    }} style={{ height: 40, width: 250, backgroundColor: "#222", color: "#fff" }}><text style={{ pointerEvents: "none" }}>Keyboard target</text></div>
    <text testId="command-selection" style={{ color: "#fff" }}>{action()}</text>
    <Show when={open()}><CommandView controller={controller} config={loadConfig(["--config", "/nonexistent", "--font-size", "14"], {})}
      onAccept={(target) => { setAction(target.action); setOpen(false); }} onClose={() => setOpen(false)} /></Show>
  </div>;
}, { renderer });
const stop = () => { root.unmount(); controller.dispose(); loop.stop(); process.exit(0); };
process.once("SIGINT", stop); process.once("SIGTERM", stop);
