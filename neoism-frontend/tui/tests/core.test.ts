import { describe, expect, test } from "bun:test";
import { loadTuiConfig } from "../src/config";
import { matchingCommands, terminalLayout } from "../src/commands";
import { treeRows } from "../src/model";

const env = { NEOISM_CONFIG_HOME: "/nonexistent/neoism-tui-test-config" };

describe("terminal frontend configuration", () => {
  test("uses the exact GUI palette and accepts explicit endpoint and workspace", () => {
    const config = loadTuiConfig(["--daemon=ws://example.test:7878/session", "--workspace", "work", "--theme", "tokyo_night", "--offline"], env);
    expect(config.endpoint).toBe("ws://example.test:7878/session");
    expect(config.workspaceId).toBe("work");
    expect(config.theme.bg).toBe("#1a1b26");
    expect(config.offline).toBe(true);
    expect(config.authToken).toBeUndefined();
  });
  test("does not turn a pairing token into an upgrade token", () => {
    const config = loadTuiConfig(["--daemon", "wss://remote.test/session"], { ...env, NEOISM_PAIRING_TOKEN: "paired-secret" });
    expect(config.pairingToken).toBe("paired-secret");
    expect(config.authToken).toBeUndefined();
  });
  test("validates flags and falls back for an unknown theme", () => {
    expect(() => loadTuiConfig(["--daemon"], env)).toThrow("Missing value");
    expect(() => loadTuiConfig(["--daemon", "https://example.test"], env)).toThrow("must use");
    expect(() => loadTuiConfig(["--unknown"], env)).toThrow("Unknown argument");
    expect(loadTuiConfig(["--theme", "__proto__"], env).theme.bg).toBe("#000000");
    expect(loadTuiConfig(["--theme", "no-such-theme"], env).warnings).toHaveLength(1);
  });
  test("directory arguments are daemon-owned and cannot be mixed with workspace IDs", () => {
    expect(loadTuiConfig([".", "--daemon", "ws://localhost:7878/session"], env).directory).toBe(process.cwd());
    expect(loadTuiConfig(["/remote/project", "--daemon", "wss://remote.test/session"], env).directory).toBe("/remote/project");
    expect(() => loadTuiConfig([".", "--daemon", "wss://remote.test/session"], env)).toThrow("absolute paths");
    expect(() => loadTuiConfig(["/project", "--workspace", "work"], env)).toThrow("not both");
  });
});

test("narrow screens preserve content before showing a sidebar", () => {
  expect(terminalLayout(120, 40, true)).toEqual({ showSidebar: true, sidebarWidth: 27, contentWidth: 93, compact: false });
  expect(terminalLayout(60, 20, true)).toEqual({ showSidebar: false, sidebarWidth: 0, contentWidth: 60, compact: true });
  expect(terminalLayout(120, 8, true).showSidebar).toBe(false);
});

test("command filtering uses every search word", () => {
  expect(matchingCommands("file tree").map(command => command.id)).toEqual(["files"]);
  expect(matchingCommands("imaginary")).toEqual([]);
});

test("tree rows respect the daemon's Windows paths and collapsed directories", () => {
  const directories = {
    "": { path: "", entries: [{ name: "src", is_dir: true }, { name: "README.md", is_dir: false }] },
    src: { path: "src", entries: [{ name: "nested", is_dir: true }] },
    "src\\nested": { path: "src\\nested", entries: [{ name: "code.rs", is_dir: false }] },
  };
  expect(treeRows(directories, new Set(["src", "src\\nested"]), "C:\\work").map(row => row.path))
    .toEqual(["src", "src\\nested", "src\\nested\\code.rs", "README.md"]);
  expect(treeRows(directories, new Set(), "C:\\work")).toHaveLength(2);
});
