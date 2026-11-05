import assert from "node:assert/strict";
import { join } from "node:path";
import { tmpdir } from "node:os";
import type { ElementBounds, TreeNode } from "@gpuix/solid/automation";
import { launchNative } from "../tests/support/native-launch";

const widths = process.argv.includes("--narrow") ? [720] : [1200, 720];
const failures: string[] = [];
const flatten = (nodes: TreeNode[]): TreeNode[] => nodes.flatMap(node => [node, ...flatten(node.children || [])]);
function inside(inner: ElementBounds, outer: ElementBounds, label: string) {
  assert.ok(inner.width > 0 && inner.height > 0, `${label} has empty bounds: ${JSON.stringify(inner)}`);
  assert.ok(inner.x >= outer.x - 1 && inner.y >= outer.y - 1 && inner.x + inner.width <= outer.x + outer.width + 1 && inner.y + inner.height <= outer.y + outer.height + 1,
    `${label} escapes pane: inner=${JSON.stringify(inner)}, pane=${JSON.stringify(outer)}`);
}
for (const width of widths) {
  const app = await launchNative({ command: process.execPath, args: ["tests/support/database-window.tsx"], cwd: process.cwd(), env: { DATABASE_CHECK_WIDTH: String(width) } });
  const check = async (label: string, work: () => Promise<void>) => {
    try { await work(); console.log(`PASS ${width}: ${label}`); }
    catch (cause) {
      const message = `${width}: ${label}: ${cause instanceof Error ? cause.message : String(cause)}`; failures.push(message); console.error(`FAIL ${message}`);
      console.error("Native failure state", JSON.stringify({ status: await status(), offset: await paintedOffset(),
        origin: await app.getByTestId("database-filter:origin").bounds().catch(() => null), grid: await app.getByTestId("database-grid").bounds() }));
    }
  };
  const until = async (predicate: () => Promise<boolean>) => { const deadline = Date.now() + 5000; while (!await predicate()) { if (Date.now() > deadline) throw new Error("Native fixture input did not settle"); await Bun.sleep(30); } };
  const status = () => app.getByTestId("database-fixture-status").textContent();
  const paintedOffset=async()=>{const grid=await app.getByTestId("database-grid").bounds(),plane=await app.getByTestId("database-data-plane").bounds();return[plane.x-grid.x,plane.y-grid.y] as const;};
  const firstRow = async () => {
    const offset=await paintedOffset();
    await app.getByTestId("database-grid").wheel(-offset[0],-offset[1]);
    await app.getByTestId("database-cell:row:0:0").waitFor();
    await until(async () => {
      const cell = await app.getByTestId("database-cell:row:0:0").bounds(), bounds = await app.getByTestId("database-grid").bounds();
      return Math.abs(cell.y - bounds.y) < 1;
    });
  };
  const clickPopupText = async (testId: string, text: string) => {
    // Native text locators include matching ancestors; resolve a painted leaf
    // to avoid an ambiguous click on a dialog or its surrounding workbench.
    const node = flatten(await app.getByTestId(testId).all()).find(node => node.text === text && !node.children?.length);
    assert.ok(node, `Missing popup text ${text}`);
    const bounds = (await app.call("getBounds", { elementId: node.id })).bounds; assert.ok(bounds);
    await app.mouse.click({ x: bounds.x + bounds.width / 2, y: bounds.y + bounds.height / 2 });
  };
  const capture = async (phase: string) => {
    try { const path = join(tmpdir(), `neoism-database-${process.pid}-${width}-${phase}.png`); await app.screenshot({ path }); console.log(`Test-window screenshot: ${path}`); }
    catch (cause) { console.log(`Test-window screenshot unavailable (${width}/${phase}): ${cause instanceof Error ? cause.message : String(cause)}. No desktop capture attempted.`); }
  };
  try {
    await app.getByTestId("database-workbench").waitFor({ timeoutMs: 15000 });
    const info = await app.call("initialize", { protocolVersion: 1, client: "native-database-check" });
    if (process.env.HYPRLAND_INSTANCE_SIGNATURE) {
      // This compositor tiles new windows and ignores init dimensions. Only
      // manipulate the freshly launched fixture, with a PID check before each
      // active-window operation. Never resize or float another application.
      await Bun.$`hyprctl dispatch ${`hl.dsp.focus({window="pid:${info.pid}"})`}`.quiet();
      const active = async () => JSON.parse(await Bun.$`hyprctl activewindow -j`.text()) as { pid: number; floating: boolean };
      let window = await active(); assert.equal(window.pid, info.pid, "Refuse to manipulate a non-fixture window");
      if (!window.floating) await Bun.$`hyprctl dispatch ${'hl.dsp.window.float({action="toggle"})'}`.quiet();
      window = await active(); assert.equal(window.pid, info.pid, "Refuse to resize a non-fixture window");
      await Bun.$`hyprctl dispatch ${`hl.dsp.window.resize({x=${width},y=740,relative=false})`}`.quiet();
    }
    await Bun.sleep(500);
    if (process.argv.includes("--input-only")) {
      // Reuse painted IDs: locator resolution itself waits for a platform frame
      // in this GPUIX build, so it must not sit in the input/scroll timing loop.
      const tree = await app.call("getTree", {});
      const nodes = flatten(tree.tree ? [tree.tree] : []);
      const find = (id: string) => { const node = nodes.find(node => node.testId === id); assert.ok(node, `Missing ${id}`); return node; };
      const bounds = async (id: string) => { const result = await app.call("getBounds", { elementId: find(id).id }); assert.ok(result.bounds, `No painted bounds for ${id}`); return result.bounds; };
      const text = async () => (await app.call("getAllText", {})).text.join("\n");
      const grid = await bounds("database-grid");
      const verticalRail = await bounds("database-scrollbar-vertical");
      assert.ok(Math.abs(verticalRail.y-grid.y)<1.1&&Math.abs(verticalRail.height-grid.height)<1.1,"Vertical rail shares the grid's top and bottom edges");
      const footer = await bounds("database-footer");
      for(const id of ["database-page-size","database-page-prev","database-page-number","database-page-next"])inside(await bounds(id),footer,`visible pagination ${id}`);
      await app.call("focus", { elementId: find("database-sidebar").id });
      await Bun.sleep(50);
      const first = await bounds("database-cell:row:0:0");
      const newRow = await bounds("database-new-row"), rowCount = await bounds("database-rows-label");
      assert.ok(Math.abs(rowCount.x-newRow.x-7*first.height/25)<1.1,"Edit labels and row count share the same left text inset");
      await app.mouse.click({ x: first.x + first.width / 2, y: first.y + first.height / 2 });
      await until(async () => (await text()).includes("selected=row:0:0;"));
      // No elementId: keys must reach the host restored by the mouse click.
      await app.call("keystrokes", { keys: "right down" });
      await until(async () => (await text()).includes("selected=row:1:1;"));
      const started = performance.now();
      await app.call("keystrokes", { keys: `${Array.from({ length: 20 }, () => "right left").join(" ")} right` });
      await until(async () => (await text()).includes("selected=row:1:2;"));
      const inputMs = Math.round(performance.now() - started);
      const anchor = await bounds("database-cell:row:20:1");
      await app.call("scrollWheel", { x: grid.x + grid.width / 2, y: grid.y + grid.height / 2, deltaX: 0, deltaY: -200 });
      await until(async () => (await bounds("database-cell:row:20:1")).y < anchor.y - 100);
      const after = await bounds("database-cell:row:20:1");
      assert.ok(after.y < anchor.y, "The same native cell scrolls without remounting");
      const headerTop=(await bounds("database-header")).y,gutterLeft=(await bounds("database-gutter:row:20")).x;
      for(let step=0;step<12;step++){
        const before=await bounds("database-cell:row:20:2"),sign=step%2?1:-1;
        await app.call("scrollWheel",{x:grid.x+grid.width/2,y:grid.y+grid.height/2,deltaX:sign*16,deltaY:sign*12});
        await until(async()=>Math.abs((await bounds("database-cell:row:20:2")).y-before.y-sign*12)<1.1);
        const cell=await bounds("database-cell:row:20:2"),number=await bounds("database-gutter:row:20"),header=await bounds("database-filter:origin");
        assert.ok(Math.abs(cell.y-number.y)<1.1,"Trackpad pan keeps row numbers pixel-aligned with cells");
        assert.ok(Math.abs(cell.x+cell.width-header.x-header.width)<2.1,"Trackpad pan keeps header and cells pixel-aligned");
        assert.ok(Math.abs(number.x-gutterLeft)<1.1&&Math.abs((await bounds("database-header")).y-headerTop)<1.1,"Frozen panes do not move along their locked axis");
      }
      if (width === 720) {
        await app.call("scrollWheel", { x: grid.x + grid.width / 2, y: grid.y + grid.height / 2, deltaX: -150, deltaY: 0 });
        await until(async () => (await bounds("database-cell:row:20:1")).x < after.x - 50);
        const rail = await bounds("database-scrollbar-horizontal"), thumb = await bounds("database-scrollbar-thumb-horizontal");
        assert.ok(Math.abs(rail.x-first.x)<1.1,"Horizontal rail starts at the data columns, not the row-number gutter");
        assert.ok(Math.abs(rail.x+rail.width-grid.x-grid.width)<1.1,"Horizontal rail ends at the same grid edge");
        const beforeDrag=await bounds("database-cell:row:20:2");
        await app.mouse.drag({x:thumb.x+thumb.width/2,y:thumb.y+thumb.height/2},{x:rail.x+rail.width-2,y:rail.y-40},{steps:8});
        await until(async()=> (await bounds("database-cell:row:20:2")).x < beforeDrag.x-50);
        const afterDrag=await bounds("database-cell:row:20:2");
        assert.ok(Math.abs(afterDrag.y-beforeDrag.y)<1.1,"Dragging horizontally preserves the vertical position");
        const header=await bounds("database-filter:origin");
        assert.ok(Math.abs(header.x+header.width-afterDrag.x-afterDrag.width)<2.1,"Dragged grid and frozen header remain aligned");
        await app.mouse.move({x:rail.x,y:rail.y-40});
        await Bun.sleep(80);
        assert.ok(Math.abs((await bounds("database-cell:row:20:2")).x-afterDrag.x)<1.1,"Mouse release ends scrollbar capture");
      }
      const beforeVertical=await bounds("database-cell:row:30:2"), verticalThumb=await bounds("database-scrollbar-thumb-vertical");
      await app.mouse.drag({x:verticalThumb.x+verticalThumb.width/2,y:verticalThumb.y+verticalThumb.height/2},{x:verticalRail.x-20,y:verticalThumb.y+verticalThumb.height/2+20},{steps:8});
      await until(async()=> (await bounds("database-cell:row:30:2")).y < beforeVertical.y-15);
      assert.ok(Math.abs((await bounds("database-cell:row:30:2")).x-beforeVertical.x)<1.1,"Vertical dragging preserves the horizontal position");
      console.log(JSON.stringify({ width, clickRestoresGridFocus: true, arrowBurstKeys: 41, inputMs, retainedCellScroll: true, horizontalScroll: width === 720 }));
      continue;
    }
    const pane = await app.getByTestId("database-fixture-body").bounds();
    console.log(JSON.stringify({ requestedWidth: width, window: info.window, pane }));
    const schema = await app.getByTestId("database-sidebar").bounds();
    const right = await app.getByTestId("database-workbench").bounds();
    await check("external sidebar, header, rows and visible cells geometry; no nested tabs", async () => {
      assert.ok(Math.abs(schema.width - 210) <= 1, `Schema width ${schema.width}, expected 210`);
      assert.ok(Math.abs(pane.width - width) <= 1, `Pane width ${pane.width}, expected ${width}`);
      inside(schema, pane, "schema");
      const header = await app.getByTestId("database-header").bounds();
      assert.ok(Math.abs(header.height - 30) <= 1, `Header height ${header.height}`);
      assert.equal(await app.getByTestId("database-tabs").count(), 0, "No nested database tabs");
      assert.ok(!flatten(await app.getByTestId("database-workbench").all()).some(node => node.testId === "database-sidebar" || node.testId === "database-schema"), "No nested database sidebar");
      assert.ok(Math.abs(right.x - schema.x - schema.width) <= 1 && Math.abs(right.width + schema.width - pane.width) <= 1, "External sidebar and buffer must share the full pane width");
      inside(header, right, "header");
      const row = await app.getByTestId("database-row:row:0").bounds(), cell = await app.getByTestId("database-cell:row:0:0").bounds();
      assert.ok(Math.abs(row.height - 25) <= 1, `Row height ${row.height}`); assert.ok(Math.abs(cell.height - 25) <= 1, `Cell height ${cell.height}`);
      inside(cell, await app.getByTestId("database-grid").bounds(), "first cell");
      const mounted = flatten(await app.getByTestId("database-grid").all()).filter(node => node.testId?.startsWith("database-row:"));
      assert.ok(mounted.length > 1 && mounted.length < 60, `Mounted ${mounted.length} of 500 rows`);
      console.log(JSON.stringify({ schema, header, row, cell, mountedRows: mounted.length }));
    });
    await capture("grid");
    await check("external sidebar connection and table activation", async () => {
      await app.getByTestId("database-connection:fruit").click();
      await app.getByTestId("database-table:bananas").click(); await until(async () => (await status()).includes("table=bananas"));
      await app.getByTestId("database-table:apples").click(); await until(async () => (await status()).includes("table=apples"));
      await Bun.sleep(200);
    });
    await check("filter popup and search fit pane and receive input", async () => {
      await app.getByTestId("database-filter:origin").click(); await app.getByTestId("database-filter-popover").waitFor();
      inside(await app.getByTestId("database-filter-popover").bounds(), right, "filter popup");
      assert.ok(Math.abs((await app.getByTestId("database-filter-popover").bounds()).y - right.y - 30) <= 1, "Filter must anchor immediately below the sole grid header");
      inside(await app.getByTestId("database-filter-search").bounds(), await app.getByTestId("database-filter-popover").bounds(), "filter search");
      console.log("Filter bounds", JSON.stringify(await app.getByTestId("database-filter-popover").bounds()));
      await app.getByTestId("database-filter-search").fill("Italy");
      await until(async () => await app.getByTestId("database-filter-popover").getByText("Italy").count() > 0 && await app.getByTestId("database-filter-popover").getByText("France").count() === 0);
      await capture("filter");
      await clickPopupText("database-filter-popover", "Italy"); await until(async () => (await status()).includes("filters=origin"));
      await clickPopupText("database-filter-popover", "Clear all"); await until(async () => (await status()).includes("filters=;"));
      await Bun.sleep(100);
    });
    if (await app.getByTestId("database-filter-search").count()) await app.getByTestId("database-filter-search").press("escape");
    await check("native cell editor and controls fit pane", async () => {
      await firstRow();
      await app.getByTestId("database-cell:row:0:1").click(); await app.getByTestId("database-workbench").press("enter");
      await app.getByTestId("database-cell-editor").waitFor();
      const popup = await app.getByTestId("database-cell-editor").bounds(); inside(popup, right, "cell editor");
      console.log("Editor bounds", JSON.stringify(popup));
      inside(await app.getByTestId("database-cell-value").bounds(), popup, "editor input");
      for (const node of flatten(await app.getByTestId("database-cell-editor").all()).filter(node => node.events?.includes("click"))) {
        const bounds = (await app.call("getBounds", { elementId: node.id })).bounds; if (bounds) inside(bounds, popup, `editor control ${node.id}`);
      }
      await capture("editor");
      await app.getByTestId("database-cell-value").fill("Native edited apple"); await app.getByTestId("database-cell-value").press("enter");
      await until(async () => (await status()).includes("pending=1"));
      assert.equal(await app.getByTestId("database-cell:row:0:1").textContent(), "Native edited apple");
    });
    if (await app.getByTestId("database-cell-editor").count()) { await capture("editor"); await app.getByTestId("database-cell-value").press("escape"); }
    await check("table activation leaves independent buffer drafts intact", async () => {
      await app.getByTestId("database-table:bananas").click(); await until(async () => (await status()).includes("table=bananas"));
      assert.ok((await status()).includes("pending=0"));
      await app.getByTestId("database-table:apples").click(); await until(async () => (await status()).includes("table=apples"));
      assert.ok((await status()).includes("pending=1"));
      await firstRow(); assert.equal(await app.getByTestId("database-cell:row:0:1").textContent(), "Native edited apple");
    });
    await check("native keyboard navigation", async () => {
      await firstRow();
      await app.getByTestId("database-cell:row:0:0").click(); await app.getByTestId("database-workbench").press("right");
      await until(async () => (await status()).includes("selected=row:0:1"));
    });
    await check("native wheel scroll virtualizes later rows", async () => {
      await app.getByTestId("database-grid").wheel(0, -600);
      console.log("Native wheel offset",await paintedOffset());
      await until(async () => (await paintedOffset())[1] < -100);
      await until(async () => !(await app.getByTestId("database-row:row:0").count()));
    });
    if (width === 720) await check("native horizontal wheel scroll synchronizes header and grid", async () => {
      await firstRow();
      await app.getByTestId("database-grid").wheel(-300, 0);
      await until(async () => (await paintedOffset())[0] < -100);
      const cell = await app.getByTestId("database-cell:row:0:4").bounds(), filter = await app.getByTestId("database-filter:price").bounds();
      inside(cell, await app.getByTestId("database-grid").bounds(), "rightmost horizontally scrolled cell");
      assert.ok(Math.abs(cell.x + cell.width - filter.x - filter.width) <= 2, "Header/filter edge does not follow horizontally scrolled cell");
    });
  } finally { await app.close(); }
}
if (failures.length) throw new Error(`Native database defects (${failures.length}):\n${failures.join("\n")}`);
console.log("Native database layout/input checks passed at all requested pane widths.");