import { expect, test } from "bun:test";
import { fileURLToPath } from "node:url";

test("actual full App handles editor close then global AltA and independent local composers without a native window", async () => {
  const child = Bun.spawn([process.execPath, "run", "tests/support/full-app-focus-check.tsx"], {
    cwd: fileURLToPath(new URL("..", import.meta.url)), stdout: "pipe", stderr: "pipe",
  });
  const [status, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  expect({ status, stderr }).toEqual({ status: 0, stderr: "" });
  expect(JSON.parse(stdout)).toEqual({ noWindow: true, fullProductionAppComponents: true, actualCodeAndMarkdownLeaderClose: true,
    postCloseWindowAltACreatesOneAgent: true, focusedComposerShortcutRunsOnce: true, threeIndependentAgentTabs: true,
    actualComposerDraftsIndependent: true, workspaceAOnlySubscribedBeforeCreation: true, noDaemonPromptsOrPty: true, persistentRoot: true,
    agentQClosesOnlyCurrentTab:true,ctrlShiftWCreatesExactlyOneWorkspace:true,newWorkspaceOpensWelcomeWithoutPrompt:true,qallReturnsToAWithoutExiting:true,notificationsRespectBodyBounds:true,actualAgentCaretPaint:true });
},15_000);
