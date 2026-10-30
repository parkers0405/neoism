// Real native-window startup check: the JS event loop must remain available
// for the daemon handshake while graphics initialization is pending.
import assert from "node:assert/strict";
import { GpuixRenderer } from "@gpuix/native";

const renderer = new GpuixRenderer() as GpuixRenderer & {
  initAsync(options?: Parameters<GpuixRenderer["init"]>[0]): Promise<void>;
};
let pending = true, turns = 0;
const timer = setInterval(() => { if (pending) turns++; }, 1);
try {
  const ready = renderer.initAsync({ title: "Neoism startup verification", width: 720, height: 480 });
  await assert.rejects(renderer.initAsync(), /already initialized/);
  await ready;
  pending = false;
  assert.equal(renderer.isInitialized(), true);
  if (process.platform !== "darwin") assert.ok(turns > 0, "native startup blocked JavaScript");
  await assert.rejects(renderer.initAsync(), /already initialized/);
  console.log(JSON.stringify({ initialized: true, eventLoopTurnsDuringInit: turns, duplicateInitRejected: true }));
} finally {
  clearInterval(timer);
}
process.exit(0);
