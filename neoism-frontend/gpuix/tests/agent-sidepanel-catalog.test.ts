import { expect, test } from "bun:test";
import { coreCall } from "../src/native-core";

const thread = (id: string, directory: string, updated: number) => ({ session_id: id, title: id, directory, updated_at: updated });
const panel = (directory: string) => coreCall<{ id: number }>("agentsidepanel.new", { directory, theme: "pastel_dark", scale: 1, fontFamily: "Cascadia Code NF", fontWeight: 400 }).id;
const frame = (id: number) => coreCall<{ send: Record<string, any>[] }>("agentsidepanel.frame", { id, width: 300, height: 600, viewedSession: null });
const debug = (id: number) => coreCall<{ sessions: number; scrollTop: number; nextCursor: string | null }>("agentsidepanel.debug", { id });

test("a settled catalog does not animate the pane's invisible home wordmark", () => {
  const id = panel("/proj/app");
  try {
    const loading = coreCall<{ animating: boolean }>("agentsidepanel.frame", { id, width: 300, height: 600 });
    expect(loading.animating).toBe(true);
    coreCall("agentsidepanel.event", { id, message: { ThreadList: { threads: [thread("a", "/proj/app", 2)], requested_cursor: null, next_cursor: null } } });
    const settled = coreCall<{ animating: boolean }>("agentsidepanel.frame", { id, width: 300, height: 600 });
    expect(settled.animating).toBe(false);
  } finally { coreCall("agentsidepanel.dispose", { id }); }
});

test("a declared-workspace sidebar ignores another consumer's ThreadList and keeps its own reply", () => {
  const id = panel("/proj/app");
  try {
    expect(frame(id).send).toEqual([{ ListThreads: { directory: "/proj/app", limit: 50, cursor: null } }]);
    // The model's startup listing for a different workspace arrives first.
    coreCall("agentsidepanel.event", { id, message: { ThreadList: { threads: [thread("other", "/proj/other", 9)], requested_cursor: null, next_cursor: null } } });
    expect(debug(id).sessions).toBe(0);
    coreCall("agentsidepanel.event", { id, message: { ThreadList: { threads: [thread("a", "/proj/app", 2), thread("b", "/proj/app", 1)], requested_cursor: null, next_cursor: "c1" } } });
    expect(debug(id)).toMatchObject({ sessions: 2, nextCursor: "c1" });
  } finally { coreCall("agentsidepanel.dispose", { id }); }
});

test("the filesystem root and home directory are the all-workspace dashboard", () => {
  for (const directory of ["/", process.env.HOME!]) {
    const id = panel(directory);
    try { expect(Object.keys(frame(id).send[0] || {})).toEqual(["ListSessionProjects"]); }
    finally { coreCall("agentsidepanel.dispose", { id }); }
  }
});

test("an unknown workspace directory waits instead of showing the dashboard, then scopes when known", () => {
  const id = panel("");
  try {
    expect(frame(id).send).toEqual([]);
    const scoped = coreCall<{ send: Record<string, any>[] }>("agentsidepanel.scope", { id, directory: "/proj/app" });
    expect(scoped.send).toEqual([{ ListThreads: { directory: "/proj/app", limit: 50, cursor: null } }]);
  } finally { coreCall("agentsidepanel.dispose", { id }); }
});
