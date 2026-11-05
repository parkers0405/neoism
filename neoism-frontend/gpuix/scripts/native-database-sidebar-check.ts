import assert from "node:assert/strict";
import { launchNative } from "../tests/support/native-launch";
import type { TreeNode } from "@gpuix/solid/automation";

const app = await launchNative({ command: process.execPath, args: ["tests/support/database-sidebar-window.tsx"], cwd: process.cwd(), env: { NEOISM_GPUIX_DISPLAY_DEBUG: "1" } });
const flatten = (nodes: TreeNode[]): TreeNode[] => nodes.flatMap(node => [node, ...flatten(node.children || [])]);
const nodes = async () => { const tree = (await app.call("getTree", {})).tree; return flatten(tree ? [tree] : []); };
const until = async (check: () => Promise<boolean>) => { const end = Date.now() + 8000; while (!await check()) { if (Date.now() > end) throw Error("Database sidebar did not settle"); await Bun.sleep(30); } };
const press = async (keys: string) => { await app.call("keystrokes", { keys }); await Bun.sleep(100); };
const noSidebarCursor = async () => { await until(async () => !(await nodes()).some(node => node.testId === "chrome-focus-cursor:sidebar")); };
const aligned = async (id: string) => {
  // Automation tree bounds are native painted snapshots: no synchronous frame
  // bounds query or timeout changes are needed for row/cursor geometry checks.
  await until(async () => {
    const tree = await nodes(), row = tree.find(node => node.testId === id)?.bounds;
    const cursors = tree.filter(node => node.testId?.startsWith("chrome-focus-cursor:"));
    if (!row || cursors.length !== 1 || cursors[0]!.testId !== "chrome-focus-cursor:sidebar") return false;
    const ink = cursors[0]!.bounds;
    return !!ink && ink.width > 0 && ink.width < 12 && ink.height > 0 && ink.y >= row.y - 1 && ink.y + ink.height <= row.y + row.height + 1;
  });
};
const activeTable = async (name: string) => {
  await app.getByTestId("database-workbench").waitFor();
  assert.equal(await app.getByTestId("database-sidebar-active-table").textContent(), name, "Activation must select the real model's table surface");
};
let stage = "mount";
try {
  await app.getByTestId("database-table:table_0:row").waitFor({ timeoutMs: 15000 });
  stage = "native sidebar hides its scrollbar but retains a scroll viewport";
  await until(async () => (await app.getByTestId("database-sidebar-native-style").textContent()).startsWith("{"));
  const style = JSON.parse(await app.getByTestId("database-sidebar-native-style").textContent());
  assert.equal(style.overflowY, "scroll");
  assert.ok(style.scrollbarColor === undefined || style.scrollbarWidth === 0, "Sidebar must not opt into a visible native scrollbar");
  await app.call("focus", { elementId: (await app.getByTestId("database-sidebar").element()).id });
  await aligned("database-connection:fruit:row");
  stage = "Alt-arrow entry and selected-row cursor movement";
  await press("alt-right"); await noSidebarCursor();
  await press("alt-left"); await aligned("database-connection:fruit:row");
  await press("down"); await aligned("database-group:Tables:row");
  await press("down"); await aligned("database-table:table_0:row");
  await press("enter"); await activeTable("table_0"); await noSidebarCursor();
  stage = "Alt-left reentry retains selection; Down moves and activates";
  await press("alt-left"); await aligned("database-table:table_0:row");
  await press("down"); await aligned("database-table:table_1:row");
  await press("enter"); await activeTable("table_1"); await noSidebarCursor();
  stage = "mouse expansion publishes clicked-row geometry";
  await app.getByTestId("database-table:table_1:expand").click();
  await app.getByTestId("database-section:table_1:Columns").waitFor(); await aligned("database-table:table_1:row");
  await press("left");
  assert.equal(await app.getByTestId("database-section:table_1:Columns").count(), 0);
  stage = "End reveals the selected bottom row with one shared cursor";
  await press("end"); await aligned("database-group:Views:row");
  const tree = await app.getByTestId("database-schema").element();
  const offset = (await app.call("getScrollOffset", { elementId: tree.id })).offset;
  assert.ok(offset && offset[1]! < 0, "End must reveal the selected bottom row");
  stage = "native scrolling clips and restores the selected-row cursor";
  await app.call("scrollTo", { elementId: tree.id, x: 0, y: 0 }); await noSidebarCursor();
  await app.call("scrollTo", { elementId: tree.id, x: 0, y: offset[1]! }); await aligned("database-group:Views:row");
  stage = "Home restores root geometry; table clicks activate content";
  await press("home"); await aligned("database-connection:fruit:row");
  await press("down right enter"); await activeTable("table_0");
  await app.getByTestId("database-table:table_2").click(); await activeTable("table_2"); await noSidebarCursor();
  console.log(JSON.stringify({ actualApp: true, oneSharedChromeCursor: true, hiddenSidebarScrollbar: true, altArrowEntryReentry: true, selectedRowGeometry: true, nativeScrollClipping: true, clickAndKeyboardActivation: true }));
} catch (cause) {
  console.error("Database sidebar stage:", stage);
  const tree=await nodes(),schema=tree.find(node=>node.testId==="database-schema"),views=tree.find(node=>node.testId==="database-group:Views:row");
  console.error(JSON.stringify(tree.filter(node => node.testId?.startsWith("chrome-focus") || ["database-schema", "database-group:Views:row"].includes(node.testId || "")).map(node => ({ id: node.id, testId: node.testId, bounds: node.bounds }))));
  console.error(JSON.stringify({offset:schema?(await app.call("getScrollOffset",{elementId:schema.id})).offset:null,viewSnapshot:views?(await app.call("getBounds",{elementId:views.id})).bounds:null,rows:tree.filter(node=>node.testId?.endsWith(":row")).map(node=>({testId:node.testId,y:node.bounds?.y})).slice(-5)}));
  throw cause;
}
finally { await app.close(); }