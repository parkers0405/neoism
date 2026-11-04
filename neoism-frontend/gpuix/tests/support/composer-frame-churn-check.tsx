import assert from "node:assert/strict";
import { createRoot } from "@gpuix/solid";
import { App } from "../../src/ui/App";
import { AppVisualState } from "../../src/ui/AppVisualState";
import { WorkspaceModel } from "../../src/model";
import { DaemonConnection } from "../../src/daemon/client";
import { loadConfig } from "../../src/config";
import { usePresentationScheduler } from "../../src/animation/frame-scheduler";
import { createAgentDaemon } from "./agent-daemon";
import { createRecordingAppRenderer } from "./full-app-recording-renderer";

const daemon = createAgentDaemon();
const config = { ...loadConfig([], {}), endpoint: daemon.endpoint, authToken: undefined, pairingToken: undefined, reducedMotion: false, cursorBlink: false };
const client = new DaemonConnection(config), model = new WorkspaceModel(client, config), visuals = new AppVisualState();
model.setUI({ openWorkspaceIds: ["a"] });
const host = createRecordingAppRenderer();
const original = host.renderer.applyBatch!.bind(host.renderer);
let counting = false, styles = 0, identicalStyles = 0, creates = 0, bytes = 0;
host.renderer.applyBatch = json => {
  if (counting) {
    bytes += json.length;
    for (const [op, id, value] of JSON.parse(json)) {
      if (op === "createElement") creates++;
      if (op === "setStyle") {
        styles++;
        if (JSON.stringify(host.nodes.get(id)?.style) === JSON.stringify(value)) identicalStyles++;
      }
    }
  }
  return original(json);
};
const root = createRoot(host.renderer, { keyboardFocusDim: false });
const callbacks = new Set<() => void>();
const restore = usePresentationScheduler(callback => { callbacks.add(callback); return () => callbacks.delete(callback); });
async function until(predicate: () => boolean) {
  const deadline = performance.now() + 5000;
  while (!predicate()) {
    await host.pump(root);
    assert.ok(performance.now() < deadline, "App did not settle");
    await Bun.sleep(2);
  }
  await host.pump(root);
}
try {
  model.start(); client.start();
  await until(() => model.state.activeWorkspace?.id === "a" && !model.state.loading);
  model.openThread({ session_id: "one", title: "One", directory: "/project/a" });
  await until(() => model.state.history.one !== undefined);
  root.render(() => <App model={model} config={config} visuals={visuals}/>);
  await host.pump(root);
  daemon.push({ StreamingState: { session_id: "one", state: "Generating", label: null } });
  await until(() => model.state.streaming.one === "Generating" && !!host.find("composer"));
  // Warm the mounted components before counting display-only updates. No
  // native window, user daemon, input events or new transcript packets.
  for (let i = 0; i < 25; i++) {
    if (i === 5) counting = true;
    const frame = [...callbacks]; callbacks.clear();
    for (const callback of frame) callback();
    await host.pump(root);
    await Bun.sleep(9);
  }
  console.log(JSON.stringify({ frames: 20, styles, identicalStyles, creates, bytes }));
  assert.equal(identicalStyles, 0, "Display-only animation must not resend unchanged native styles");
  assert.equal(creates, 0, "Display-only animation must not rebuild host elements");
} finally {
  counting = false; root.unmount(); restore(); visuals.dispose(); model.dispose(); client.stop(); daemon.stop();
}