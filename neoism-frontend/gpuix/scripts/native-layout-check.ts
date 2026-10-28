import { launchNative } from "../tests/support/native-launch";
const app = await launchNative({ command: process.execPath, args: ["tests/support/layout-window.tsx"], cwd: process.cwd() });
let stage = "initial";
try {
  await app.getByTestId("pane:3").waitFor({ timeoutMs: 15000 });
  stage="tab click"; await app.getByTestId("pane-tab:1").click();
  await app.getByTestId("surface:a").waitFor();
  if (await app.getByTestId("layout-focus").textContent() !== "a") throw new Error("Local tab click did not change focused surface");
  stage="divider"; const pane = app.getByTestId("pane:3"), divider = app.getByTestId("pane-divider::0");
  const before = (await pane.bounds()).width;
  const center = await divider.center();
  await app.mouse.down(center);
  await app.getByTestId("pane-drag-layer").waitFor();
  await app.getByTestId("pane-drag-layer").bounds();
  await app.mouse.move({ x: center.x + 80, y: center.y }, { pressedButton: 0 });
  await app.mouse.up({ x: center.x + 80, y: center.y });
  let after = before;
  const deadline = Date.now() + 3000;
  while (after > before - 55 && Date.now() < deadline) { await Bun.sleep(16); after = (await pane.bounds()).width; }
  if (after > before - 55) throw new Error(`Native divider drag did not resize its pane: ${before} → ${after}`);
  stage="close first";
  await app.getByTestId("pane-close:1").click();
  await app.getByTestId("surface:b").waitFor();
  stage="tab drag";
  const tab = await app.getByTestId("pane-tab:2").center();
  const target = await app.getByTestId("pane:3").center();
  await app.mouse.down(tab);
  await app.mouse.move({ x: tab.x + 12, y: tab.y }, { pressedButton: 0 });
  await app.getByTestId("pane-drag-layer").waitFor();
  await app.getByTestId("pane-drag-layer").bounds();
  await app.mouse.move(target, { pressedButton: 0 });
  await app.mouse.up(target);
  const collapseDeadline = Date.now() + 3000;
  while (await app.getByTestId("pane-divider::0").count()) { if (Date.now() > collapseDeadline) throw new Error("Tab center drop did not adopt the source into the target pane"); await Bun.sleep(16); }
  stage="close second"; await app.getByTestId("pane-close:2").click();
  await app.getByTestId("surface:shell").waitFor();
  if (await app.getByTestId("pane-divider::0").count()) throw new Error("Last split divider survived pane close");
  console.log("Native local-tab clicks, divider drag, tab adoption, focus, and close/collapse passed.");
} catch(error) {console.error("layout stage",stage);console.error("Layout failure snapshot", JSON.stringify(await app.getByTestId("pane-layout").all()));throw error;} finally { await app.close(); }
