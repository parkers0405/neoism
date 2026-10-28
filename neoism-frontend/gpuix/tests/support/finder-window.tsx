// Native interaction fixture for the production Finder and shared Rust engine.
import { createRenderer, render } from "@gpuix/solid";
import { startFrameLoop } from "@gpuix/native/runtime";
import { createSignal, Show } from "solid-js";
import { loadConfig } from "../../src/config";
import { FinderController } from "../../src/finder/FinderController";
import { FinderView } from "../../src/finder/FinderView";
import { variant } from "../../src/daemon/protocol";
const renderer = createRenderer();
renderer.init({ title: "Neoism Finder verification", appId: "ai.neoism.gpuix.finder-check", width: 1200, height: 800 });
const loop = startFrameLoop(renderer);
const finder = new FinderController({ cwd: "/project", search: async (message) => {
  const [tag, payload] = variant(message)!; const query = (payload as { query?: string }).query || "file";
  if (tag === "CollectFiles") return { CollectFilesResult: { req_id: 0, paths: ["src/file.rs"] } };
  return { SearchFilesResult: { req_id: 0, hits: Array.from({ length: 50 }, (_, n) => ({ path: `src/${query}-${n}.rs`, score: 100 - n })) } };
} });
const root = render(() => {
  const [open, setOpen] = createSignal(true), [selected, setSelected] = createSignal("");
  return <div style={{ width: "100%", height: "100%", backgroundColor: "#111" }}>
    <text testId="finder-selection" style={{ color: "#fff" }}>{selected()}</text>
    <Show when={open()}><FinderView controller={finder} config={loadConfig(["--config", "/nonexistent", "--font-size", "14"], {})}
      onAccept={(target) => { setSelected(target.path || ""); setOpen(false); }} onClose={() => setOpen(false)} /></Show>
  </div>;
}, { renderer });
const stop = () => { root.unmount(); finder.dispose(); loop.stop(); process.exit(0); };
process.once("SIGINT", stop); process.once("SIGTERM", stop);
