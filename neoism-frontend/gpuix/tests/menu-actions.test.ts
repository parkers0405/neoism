import {canonicalPath} from "./support/canonical-path";
import { afterEach, expect, test } from "bun:test";
import { Effect } from "effect";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync, symlinkSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { parse } from "jsonc-parser";
import { coreCall } from "../src/native-core";
import { loadConfig } from "../src/config";
import { DaemonConnection } from "../src/daemon/client";
import { WorkspaceModel } from "../src/model";
import { decodeSession } from "../src/session-store";
import { editConfigDocument, themeUpdates } from "../src/menu-actions/config-document";
import { localDaemon } from "../src/menu-actions/web-frontend";
import { ChromeActionsController, type MenuFrame } from "../src/menu-actions/ChromeActionsController";
const cleanups: Array<() => void> = [];
afterEach(() => cleanups.splice(0).reverse().forEach((cleanup) => cleanup()));
function menu(kind: string, data: Record<string, unknown> = {}) {
  const { id, frame } = coreCall<{ id: number; frame: MenuFrame }>("menu.new", { kind, width: 1200, height: 800, ...data });
  cleanups.push(() => coreCall("menu.dispose", { id }));
  return { id, frame, call: (op: string, data: Record<string, unknown> = {}) => coreCall<MenuFrame>(`menu.${op}`, { id, ...data }) };
}
test("About and appearance pickers run the original Rust painters and selection engines", () => {
  const about = menu("about");
  expect(about.frame.commands.some((op) => op.kind === "text" && op.text === "About Neoism")).toBe(true);
  expect(about.frame.commands.some((op) => op.kind === "text" && op.text.includes("terminal-first"))).toBe(true);
  expect(about.call("key", { key: "enter" }).active).toBe(false);
  const themes = menu("themes"), selected = themes.call("query", { query: "tokyo_night" });
  expect(selected.target).toEqual({ kind: "theme", name: "tokyo_night" });
  expect(selected.commands.some((op) => op.kind === "text" && op.text.includes("Tokyo"))).toBe(true);
  expect(themes.call("key", { key: "enter" }).action).toEqual({ kind: "accept" });
});
test("Settings draws actual descriptor controls and returns real writes from native hit tests", () => {
  const settings = menu("settings", { values: { editor: { "vim-mode": false } }, descriptors: [{ path: "editor.vim-mode", label: "Modal editing", description: "Original engine switch", value_kind: "boolean", default: false, static_suggestions: [], runtime_suggestions: [], options: [], constraints: { nullable: false }, accepted_kinds: ["boolean"], extensible: false, category: "general", control: { kind: "toggle" } }] });
  const toggle = settings.frame.commands.find((op) => op.kind === "rect" && Math.abs(op.rect[2] - 40) < 0.1 && Math.abs(op.rect[3] - 22) < 0.1);
  expect(toggle?.kind).toBe("rect"); if (toggle?.kind !== "rect") throw new Error("Original settings toggle did not paint");
  expect(settings.call("click", { x: toggle.rect[0] + 10, y: toggle.rect[1] + 10 }).action).toEqual({ kind: "setting", key: "editor.vim-mode", value: true });
});
test("GPUIX Settings omits retired renderer controls and the removed minimap while preserving daemon settings data", async () => {
  const config = loadConfig(["--config", "/nonexistent/menu-minimap-test"], {}), client = new DaemonConnection(config);
  const descriptor = { path: "editor.minimap", label: "Minimap", description: "Desktop minimap", value_kind: "boolean", default: false, static_suggestions: [], runtime_suggestions: [], options: [], constraints: { nullable: false }, accepted_kinds: ["boolean"], extensible: false, category: "general", control: { kind: "toggle" } };
  const original = menu("settings", { values: { editor: { minimap: true } }, descriptors: [descriptor] });
  expect(original.frame.commands.some(command => command.kind === "text" && command.text === "Minimap")).toBe(true);
  client.request = ((_family: string, message: unknown) => Effect.succeed(message === "GetConfig" ? { Config: { value: { editor: { minimap: true } } } } : { ConfigSchema: { descriptors: [descriptor, {...descriptor,path:"renderer.use-cpu",label:"Old CPU renderer"}, {...descriptor,path:"platform.windows.renderer.backend",label:"Old renderer override"}] } })) as typeof client.request;
  const model = new WorkspaceModel(client, config), controller = new ChromeActionsController({ model, config, reload: () => {} });
  cleanups.push(() => { controller.dispose(); model.dispose(); });
  await controller.open("OpenSettings");
  expect(controller.frame?.kind).toBe("settings");
  for(const label of ["Minimap","Old CPU renderer","Old renderer override"]){
    controller.call("query", { query: label });
    const commands = controller.frame?.commands || [];
    expect(commands.some(command => command.kind === "text" && command.text === label)).toBe(false);
  }
  expect(descriptor.path).toBe("editor.minimap");
});
test("extensions preserve actual inventory status while withholding unsupported install actions", () => {
  const extensions = menu("extensions", { entries: [{ id: "rust", name: "Rust Analyzer", version: "1", description: "Rust language server", author: "Neoism", categories: ["Language Servers"], languages: ["Rust"], status: "installed", installed_version: "1" }] });
  const labels = extensions.frame.commands.filter((op) => op.kind === "text").map((op) => op.kind === "text" ? op.text : "");
  expect(labels).toContain("Installed"); expect(labels).not.toContain("Uninstall");
  expect(extensions.call("key", { key: "enter" }).action).toBeNull();
});
test("Extensions follows the same compositor scroll direction as Settings and Code",async()=>{
  const extensions=menu("extensions",{entries:Array.from({length:30},(_,index)=>({id:`fixture-${index}`,name:`Fixture extension ${index}`,version:"1",description:"Scroll direction fixture",author:"Neoism",categories:["Language Servers"],languages:["Rust"],status:"installed",installed_version:"1"}))});
  const y=(frame:MenuFrame)=>frame.commands.find(command=>command.kind==="text"&&command.text==="Fixture extension 0");
  const before=y(extensions.frame);expect(before?.kind).toBe("text");if(before?.kind!=="text")throw new Error("Fixture extension did not paint");
  let frame=extensions.call("scroll",{delta:-40});const limit=performance.now()+1000;
  while(performance.now()<limit){const label=y(frame);if(label?.kind==="text"&&label.y<before.y)break;await Bun.sleep(8);frame=extensions.call("frame");}
  const after=y(frame);expect(after?.kind).toBe("text");if(after?.kind!=="text")throw new Error("Scrolled fixture extension did not paint");expect(after.y).toBeLessThan(before.y);
});
test("background discovery reuses backend format, explicit-selection precedence and alias dedup", () => {
  const base = mkdtempSync(join(tmpdir(), "neoism-bg-catalog-")); cleanups.push(() => rmSync(base, { recursive: true, force: true }));
  mkdirSync(join(base, "bg")); writeFileSync(join(base, "bg", "one.PNG"), "fixture"); writeFileSync(join(base, "bg", "ignore.txt"), "fixture");
  symlinkSync(join(base, "bg", "one.PNG"), join(base, "alias.png"));
  const result = coreCall<{ entries: Array<{ name: string; path: string; opacity: number }> }>("menu.background-catalog", { base, image: { path: "alias.png", opacity: 0.23 }, packs: [] });
  expect(result.entries).toHaveLength(1); expect(result.entries[0].name).toBe("alias"); expect(result.entries[0].opacity).toBeCloseTo(0.23);
  expect(canonicalPath(result.entries[0].path)).toBe(canonicalPath(join(base, "bg", "one.PNG")));
});
test("theme/background transactions retain JSONC comments and optimistic revision", () => {
  const document = { content: '{\n // keep me\n "appearance":{"mashup-pack":"my-pack","mashup-baseline":{"theme":"old","font-family":"Mono"}},\n "editor":{"vim-mode":true}\n}', display_path: "/config.json", revision: "revision-1", writable: true };
  const update = editConfigDocument(document, themeUpdates(document, "tokyo_night"));
  expect(update.expected_revision).toBe("revision-1"); expect(update.content).toContain("// keep me");
  expect(parse(update.content).appearance["mashup-baseline"]).toEqual({ theme: "tokyo_night", "font-family": "Mono" });
  expect(parse(update.content).editor["vim-mode"]).toBe(true);
  expect(localDaemon("wss://remote.example/session")).toBe(false); expect(localDaemon("ws+unix:///run/test.sock:/session")).toBe(true);
});
test("config editor uses revision-checked Config transport and preserves dirty text on conflict", async () => {
  const config = loadConfig(["--config", "/nonexistent"], {}), client = new DaemonConnection(config);
  let revision = "r1", written = "", family = "";
  client.request = ((kind: string, message: any) => Effect.suspend(() => {
    family = kind; if (message.SaveConfigDocument.expected_revision !== revision) return Effect.fail(new Error("revision conflict"));
    written = message.SaveConfigDocument.content; revision = "r2";
    return Effect.succeed({ ConfigDocumentSaved: { document: { content: written, revision, writable: true, display_path: "/config/config.json" } } });
  })) as typeof client.request;
  const model = new WorkspaceModel(client, config); cleanups.push(() => model.dispose());
  const workspace = { id: "workspace", host_id: "host", title: "Project", root_dir: "/project" };
  model.update({ workspaces: [workspace], activeWorkspace: workspace });
  model.openConfigDocument({ content: "{}", display_path: "/config/config.json", revision, writable: true });
  const surface = model.state.surfaces[0], editor = model.getEditor(surface); editor.call("paste", { text: "edited " });
  await model.saveFile(surface); expect(family).toBe("Config"); expect(written).toBe(editor.text()); expect(editor.frame.dirty).toBe(false);
  const savedSurface = model.state.surfaces[0]; editor.call("paste", { text: "unsaved " }); const dirty = editor.text(); revision = "external-change";
  await expect(model.saveFile(savedSurface)).rejects.toThrow("revision conflict"); expect(editor.frame.dirty).toBe(true); expect(editor.text()).toBe(dirty);
  model.openConfigDocument({ content: "external", display_path: "/config/config.json", revision, writable: true }); expect(editor.text()).toBe(dirty);
  expect(model.state.activeWorkspace?.root_dir).toBe("/project");
  model.openPage("extensions"); model.openPage("extensions");
  expect(model.state.surfaces.filter((surface) => surface.kind === "page")).toHaveLength(1);
  expect(decodeSession(model.captureSession())?.surfaces.some((surface) => surface.page === "extensions")).toBe(true);
});
