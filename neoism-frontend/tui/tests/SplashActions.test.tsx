import { afterEach, expect, test } from "bun:test";
import { testRender, useTerminalDimensions } from "@opentui/solid";
import { SplashActions } from "../src/SplashActions";
import { palettes } from "../../gpuix/src/design/generated";

const cleanups: Array<() => Promise<void>> = [];
afterEach(async () => { for (const cleanup of cleanups.splice(0)) await cleanup(); });

test("splash quick actions have equal columns and aligned keys, then become one centered column", async () => {
  const screen = await testRender(() => {
    const dimensions = useTerminalDimensions();
    return <box width="100%" height="100%" alignItems="center" justifyContent="center"><SplashActions width={dimensions().width - 8} theme={palettes.pastel_dark} onAction={() => {}} onCommands={() => {}} /></box>;
  }, { width: 100, height: 30 });
  cleanups.push(screen.dispose); await screen.flush();
  const action = (id: string) => screen.renderer.root.findDescendantById(`splash-action-${id}`)!;
  expect(action("files").width).toBe(action("commands").width);
  expect(action("files").x).toBe(action("search").x);
  expect(action("commands").x).toBe(action("directory").x);
  expect(action("files").y).toBe(action("commands").y);
  const lines = screen.captureCharFrame().split("\n").filter(line => line.includes("[Alt"));
  expect(lines).toHaveLength(2);
  expect(lines[0]!.indexOf("[Alt + E]")).toBe(lines[1]!.indexOf("[Alt + S]"));
  expect(lines[0]!.indexOf("[Alt + P]")).toBe(lines[1]!.indexOf("[Alt + D]"));
  screen.resize(50, 20); await screen.flush();
  expect(action("files").x).toBe(action("commands").x);
  expect(action("files").width).toBe(action("directory").width);
  expect(screen.captureCharFrame().split("\n").filter(line => line.includes("[Alt"))).toHaveLength(4);
});