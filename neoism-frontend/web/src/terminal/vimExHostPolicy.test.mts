import assert from "node:assert/strict";
import test from "node:test";
import {
  VimExSaveCloseGate,
  activeIndexAfterClose,
  resolveVimExHostAction,
} from "./vimExHostPolicy.ts";

test("clean and modified code/Markdown buffers follow q/q! desktop semantics", () => {
  for (const document of [true]) {
    assert.deepEqual(
      resolveVimExHostAction("close", { document, modified: false, workspaceModified: false }),
      { kind: "close_buffer" },
    );
    assert.deepEqual(
      resolveVimExHostAction("close", { document, modified: true, workspaceModified: true }),
      { kind: "refuse_modified", all: false },
    );
    assert.deepEqual(
      resolveVimExHostAction("close_force", { document, modified: true, workspaceModified: true }),
      { kind: "close_buffer" },
    );
  }
});

test("write is save-only and wq waits for the matching Saved acknowledgement", () => {
  const context = { document: true, modified: true, workspaceModified: true };
  assert.deepEqual(resolveVimExHostAction("write", context), { kind: "save" });
  assert.deepEqual(resolveVimExHostAction("write_close", context), {
    kind: "save_then_close",
  });

  const gate = new VimExSaveCloseGate();
  gate.arm({ tabKey: "/workspace/a.md", bufferId: "file:///workspace/a.md" });
  assert.equal(gate.acknowledge("file:///workspace/other.md"), null);
  assert.equal(gate.peek()?.tabKey, "/workspace/a.md");
  assert.equal(gate.acknowledge("file:///workspace/a.md"), "/workspace/a.md");
  assert.equal(gate.peek(), null);
});

test("host writes also release only their originating tab", () => {
  const gate = new VimExSaveCloseGate();
  gate.arm({ tabKey: "/workspace/main.rs", bufferId: null });
  assert.equal(gate.acknowledgeHostWrite("/workspace/lib.rs"), null);
  assert.equal(gate.acknowledgeHostWrite("/workspace/main.rs"), "/workspace/main.rs");
});

test("qall refuses dirty workspace unless forced and has no app-quit action", () => {
  assert.deepEqual(
    resolveVimExHostAction("close_all", {
      document: true,
      modified: false,
      workspaceModified: true,
    }),
    { kind: "refuse_modified", all: true },
  );
  assert.deepEqual(
    resolveVimExHostAction("close_all_force", {
      document: true,
      modified: true,
      workspaceModified: true,
    }),
    { kind: "close_workspace_buffers" },
  );
  const possibleKinds = [
    resolveVimExHostAction("close", {
      document: false,
      modified: false,
      workspaceModified: false,
    }).kind,
    resolveVimExHostAction("close_all_force", {
      document: false,
      modified: false,
      workspaceModified: false,
    }).kind,
  ];
  assert.ok(!possibleKinds.includes("quit" as never));
});

test("active tab fallback stays in range for first, middle, last, and only tabs", () => {
  assert.equal(activeIndexAfterClose(3, 0), 0);
  assert.equal(activeIndexAfterClose(3, 1), 1);
  assert.equal(activeIndexAfterClose(3, 2), 1);
  assert.equal(activeIndexAfterClose(1, 0), 0);
});

test("terminal :w is unavailable while terminal :q remains a buffer close", () => {
  const terminal = { document: false, modified: false, workspaceModified: false };
  assert.deepEqual(resolveVimExHostAction("write", terminal), { kind: "unavailable" });
  assert.deepEqual(resolveVimExHostAction("close", terminal), { kind: "close_buffer" });
});