import { afterEach, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { daemonTokenPath, loadConfig, parseLocalServiceEndpoint } from "../src/config";
import { easeOutCubic, revealProgress } from "../src/design/animation";
const dirs: string[] = [];
afterEach(() => dirs.splice(0).forEach((path) => rmSync(path, { recursive: true, force: true })));
test("service discovery accepts only versioned, concrete loopback endpoints",()=>{
 const record=(address:string)=>JSON.stringify({protocol:1,pid:123,nonce:"owner",address});
 expect(parseLocalServiceEndpoint(record("127.0.0.1:54321"))).toBe("ws://127.0.0.1:54321/session");
 expect(parseLocalServiceEndpoint(record("[::1]:54321"))).toBe("ws://[::1]:54321/session");
 for(const address of ["0.0.0.0:54321","example.com:54321","127.0.0.1:0","127.0.0.1:65536","127.0.0.1:1234?token=secret"])
  expect(parseLocalServiceEndpoint(record(address))).toBeUndefined();
 expect(parseLocalServiceEndpoint("{}")).toBeUndefined();
 expect(parseLocalServiceEndpoint("not json")).toBeUndefined();
});
test("local discovery survives config loads, ignores dead owners and preserves explicit endpoints",async()=>{
 if(process.platform==="win32")return;
 const dir=mkdtempSync(join(tmpdir(),"neoism-service-discovery-"));dirs.push(dir);mkdirSync(join(dir,"neoism"));
 writeFileSync(join(dir,"neoism","daemon-endpoint.json"),JSON.stringify({protocol:1,pid:process.pid,nonce:"owner",address:"127.0.0.1:54321"}));
 const env={XDG_RUNTIME_DIR:dir};
 expect(loadConfig([],env,{}).endpoint).toBe("ws://127.0.0.1:54321/session");
 expect(loadConfig([],{...env,NEOISM_DAEMON_ADDR:"127.0.0.1:9000"},{}).endpoint).toBe("ws://127.0.0.1:9000/session");
 expect(loadConfig(["--daemon","wss://remote.test/session"],env,{}).endpoint).toBe("wss://remote.test/session");
 const exited=Bun.spawn([process.execPath,"-e","process.exit(0)"],{stdout:"ignore",stderr:"ignore"});await exited.exited;
 writeFileSync(join(dir,"neoism","daemon-endpoint.json"),JSON.stringify({protocol:1,pid:exited.pid,nonce:"old-owner",address:"127.0.0.1:54321"}));
 expect(loadConfig([],env,{}).endpoint).toBe("ws://127.0.0.1:7878/session");
});
test("local credentials use the daemon's platform-specific token location",()=>{
  const temp=tmpdir();
  expect(daemonTokenPath({USERNAME:"chand"},"win32",temp)).toBe(join(temp,"neoism-chand","daemon-token"));
  expect(daemonTokenPath({USERNAME:"A B_é😀",XDG_RUNTIME_DIR:"ignored"},"win32",temp)).toBe(join(temp,"neoism-A-B---","daemon-token"));
  expect(daemonTokenPath({},"win32",temp)).toBe(join(temp,"neoism-default","daemon-token"));
  expect(daemonTokenPath({XDG_RUNTIME_DIR:"/runtime"},"linux",temp)).toBe(join("/runtime","neoism","daemon-token"));
});
test("reads actual Neoism JSONC appearance and ui.window paths without writing config", () => {
  const dir = mkdtempSync(join(tmpdir(), "neoism-gpuix-config-")); dirs.push(dir);
  const path = join(dir, "config.json");
  writeFileSync(path, `{
    // same JSONC format as desktop
    "appearance": {"theme":"tokyo_night","fonts":{"family":"Geist Mono","size":20,"weight":700,"symbol-map":[{"font-family":"JetBrainsMono Nerd Font"}]}},
    "look":{"markdown":{"font-family":"Monocraft"}},
    "terminal":{"scroll":{"multiplier":4,"divider":2}},
    "ui":{"window":{"background-image":{"path":${JSON.stringify(join(dir,"image.png"))},"opacity":0.35}}},
  }`);
  const config = loadConfig(["--config", path], {});
  expect(config.fontSize).toBe(20); expect(config.scale).toBeCloseTo(20 / 14);
  expect(config.fontFamily).toBe("Geist Mono"); expect(config.fontWeight).toBe(700);
  expect(config.iconFontFamily).toBe("JetBrainsMono Nerd Font"); expect(config.markdownFontFamily).toBe("Monocraft");
  expect(config.theme.bg).toBe("#1a1b26"); expect(config.background?.opacity).toBe(0.35);
  expect(config.terminalScrollMultiplier).toBe(4); expect(config.terminalScrollDivider).toBe(2);
});
test("does not automatically send a local secret to an explicit remote endpoint", () => {
  const config = loadConfig(["--config", "/nonexistent/neoism-gpuix-test", "--daemon", "wss://example.test/session"], {});
  expect(config.authToken).toBeUndefined(); expect(config.pairingToken).toBeUndefined();
});
test("VI editing follows Neoism's enabled default and explicit opt out", () => {
  expect(loadConfig([], {}, {}).vim).toBe(true);
  expect(loadConfig([], {}, { editor: {} }).vim).toBe(true);
  expect(loadConfig([], {}, { editor: { "vim-mode": true } }).vim).toBe(true);
  expect(loadConfig([], {}, { editor: { "vim-mode": false } }).vim).toBe(false);
});
test("cursor blink reads the original local half-period and desktop bounds", () => {
  expect(loadConfig([], {}, {}).cursorBlinkInterval).toBe(530);
  const cursor = (value: number) => loadConfig([], {}, {terminal:{cursor:{"blinking-interval":value}}}).cursorBlinkInterval;
  expect(cursor(900)).toBe(900); expect(cursor(1)).toBe(350); expect(cursor(5000)).toBe(1200);
  expect(loadConfig([], {}, {terminal:{cursor:{blinking:false}}}).cursorBlink).toBe(false);
});
test("wordmark autoplay follows the original opt-in preference without changing reduced motion", () => {
  expect(loadConfig([], {}, {}).splashAutoAnimate).toBe(false);
  expect(loadConfig([], {}, { appearance: { "splash-auto-animate": true } }).splashAutoAnimate).toBe(true);
  expect(loadConfig([], {}, { appearance: { "splash-auto-animate": "on" } }).splashAutoAnimate).toBe(false);
  const reduced = loadConfig(["--reduced-motion"], {}, { appearance: { "splash-auto-animate": true } });
  expect(reduced.splashAutoAnimate).toBe(true);
  expect(reduced.reducedMotion).toBe(true);
});
test("Agent stars default on and can be disabled independently of reduced motion",()=>{
 expect(loadConfig([],{},{}).agentStars).toBe(true);
 expect(loadConfig([],{}, {appearance:{"agent-stars":false}}).agentStars).toBe(false);
 const reduced=loadConfig(["--reduced-motion"],{},{});
 expect(reduced.agentStars).toBe(true);expect(reduced.reducedMotion).toBe(true);
});
test("tree reveal matches the production ease-out cubic and per-row delay", () => {
  expect(easeOutCubic(0)).toBe(0); expect(easeOutCubic(0.5)).toBe(0.875); expect(easeOutCubic(1)).toBe(1);
  expect(revealProgress(12, 1, 320, 12)).toBe(0);
  expect(revealProgress(172, 1, 320, 12)).toBe(0.875);
  expect(revealProgress(332, 1, 320, 12)).toBe(1);
});

test("composer material retains configured selector opacity without fading text or changing wallpaper", () => {
  const config = (styles: Record<string, unknown>) => loadConfig([], {}, {ui: {styles}});
  expect(config({}).composerOpacity).toBe(0.85);
  expect(config({composer: {opacity: 0.4}}).composerOpacity).toBe(0.4);
  expect(config({composer: {opacity: 0.4}, "composer.agent": {opacity: 0.7}}).composerOpacity).toBe(0.7);
  expect(config({"composer.agent": {opacity: 0}}).composerOpacity).toBe(0);
  expect(config({"composer.agent": {opacity: 1}}).composerOpacity).toBe(1);
  expect(config({"composer.agent": {opacity: 5}}).composerOpacity).toBe(1);
  expect(config({"composer.agent": {opacity: -2}}).composerOpacity).toBe(0);
  expect(config({"composer.agent": {opacity: NaN}}).composerOpacity).toBe(0.85);
});

test("custom configuration home matches the daemon and an explicit file takes precedence", () => {
  const dir = mkdtempSync(join(tmpdir(), "neoism-config-home-")); dirs.push(dir);
  writeFileSync(join(dir, "config.json"), JSON.stringify({appearance:{fonts:{size:19}}}));
  const explicit = join(dir, "override.json");
  writeFileSync(explicit, JSON.stringify({appearance:{fonts:{size:22}}}));
  expect(loadConfig([], {NEOISM_CONFIG_HOME:dir, XDG_CONFIG_HOME:"/nonexistent"}).fontSize).toBe(19);
  expect(loadConfig(["--config", explicit], {NEOISM_CONFIG_HOME:dir}).fontSize).toBe(22);
});

test("equals-form launch flags retain explicit endpoint isolation and preferences", () => {
  const config = loadConfig(["--daemon=wss://example.test/session", "--workspace=work", "--font-size=19", "--theme=tokyo_night"], {}, {});
  expect(config.endpoint).toBe("wss://example.test/session");
  expect(config.workspaceId).toBe("work");
  expect(config.fontSize).toBe(19);
  expect(config.themeName).toBe("tokyo_night");
  expect(config.authToken).toBeUndefined();
});

test("terminal shell config preserves program paths and literal argv including explicit empty arguments",()=>{
 const program="/home/parker/.nix-profile/bin/zsh";
 expect(loadConfig([],{}, {terminal:{shell:{program,args:["--login","argument with spaces"]}}}).shell).toEqual({program,args:["--login","argument with spaces"]});
 expect(loadConfig([],{}, {terminal:{shell:{program,args:[]}}}).shell?.args).toEqual([]);
 expect(loadConfig([],{}, {terminal:{shell:{program}}}).shell?.args).toBeUndefined();
 expect(loadConfig([],{}, {terminal:{shell:{program,args:["--login"]}},shell:{program:"/bin/bash"}}).shell).toEqual({program,args:["--login"]});
 expect(loadConfig([],{}, {shell:{program,args:[]}}).shell).toEqual({program,args:[]});
 expect(loadConfig([],{}, {}).shell).toBeUndefined();
 const invalid=loadConfig([],{}, {terminal:{shell:{program,args:["--login",4]}}});
 expect(invalid.shell).toEqual({program,args:undefined});expect(invalid.warnings).toContain("Shell arguments must be an array of strings; using default shell arguments.");
});

test("block terminal defaults on and preserves an explicit boolean opt out", () => {
  expect(loadConfig([], {}, {}).terminalBlockUi).toBe(true);
  expect(loadConfig([], {}, {terminal:{}}).terminalBlockUi).toBe(true);
  expect(loadConfig([], {}, {terminal:{"block-ui":false}}).terminalBlockUi).toBe(false);
  for (const value of [true, "false", 1, null]) {
    expect(loadConfig([], {}, {terminal:{"block-ui":value}}).terminalBlockUi).toBe(true);
  }
  const snapshot = JSON.parse(JSON.stringify({
    terminal:{"block-ui":true,shell:{program:"/bin/sh",args:["-l"]},scroll:{multiplier:4,divider:2}},
    appearance:{theme:"tokyo_night",fonts:{family:"Geist Mono",size:18}},
  }));
  const config = loadConfig([], {}, snapshot);
  expect(config.terminalBlockUi).toBe(true);
  expect(config.shell).toEqual({program:"/bin/sh",args:["-l"]});
  expect(config.terminalScrollMultiplier).toBe(4);
  expect(config.terminalScrollDivider).toBe(2);
  expect(config.themeName).toBe("tokyo_night");
  expect(config.fontFamily).toBe("Geist Mono");
  expect(config.fontSize).toBe(18);
});

test("block terminal reads the grouped JSONC key with comments and trailing commas", () => {
  const dir = mkdtempSync(join(tmpdir(), "neoism-block-ui-config-")); dirs.push(dir);
  const path = join(dir, "config.json");
  writeFileSync(path, `{
    // New sessions use the chosen presentation.
    "terminal": {"block-ui": true,},
  }`);
  expect(loadConfig(["--config", path], {}).terminalBlockUi).toBe(true);
});

test("terminal input position defaults to bottom with blocks enabled", () => {
  for (const snapshot of [{}, {terminal:{}}]) {
    const config = loadConfig([], {}, snapshot);
    expect(config.terminalInputPosition).toBe("bottom");
    expect(config.terminalBlockUi).toBe(true);
  }
});

test("terminal input positions survive config snapshots independently of block opt in", () => {
  for (const position of ["bottom", "top", "flow"] as const) {
    for (const enabled of [false, true]) {
      const snapshot = JSON.parse(JSON.stringify({terminal:{"input-position":position,"block-ui":enabled}}));
      const config = loadConfig([], {}, snapshot);
      expect(config.terminalInputPosition).toBe(position);
      expect(config.terminalBlockUi).toBe(enabled);
    }
  }
});

test("invalid terminal input positions fall back to bottom", () => {
  for (const position of ["left", "TOP", "", 1, true, null, [], {}]) {
    expect(loadConfig([], {}, {terminal:{"input-position":position}}).terminalInputPosition).toBe("bottom");
  }
});

test("terminal input position reads grouped JSONC with comments and trailing commas", () => {
  const dir = mkdtempSync(join(tmpdir(), "neoism-input-position-config-")); dirs.push(dir);
  const path = join(dir, "config.json");
  writeFileSync(path, `{
    // Input placement only affects the block layout.
    "terminal": {"input-position": "flow",},
  }`);
  const config = loadConfig(["--config", path], {});
  expect(config.terminalInputPosition).toBe("flow");
  expect(config.terminalBlockUi).toBe(true);
});
