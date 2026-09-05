import { test } from "node:test";
import assert from "node:assert/strict";

import {
  classifyWorkspaceHost,
  notesVaultForWorkspace,
} from "./workspaceHostIdentity.ts";
import type { WorkspaceSummary } from "../workspace/types.ts";

const workspace = (hostId: string): WorkspaceSummary => ({
  id: "workspace-1",
  host_id: hostId,
  title: "Notes",
  root_dir: "/project",
  linked_vault_dir: "/vault/shared",
  notes_vault_dir: "/vault/personal",
});

test("same stable host id is own across different URL aliases", () => {
  assert.equal(
    classifyWorkspaceHost(
      { hostId: "machine-a", url: "ws://127.0.0.1:7878/session" },
      { hostId: "machine-a", url: "wss://laptop.tailnet.ts.net/session" },
    ),
    "own",
  );
});

test("different stable host id is foreign even when URLs are identical", () => {
  const url = "wss://gateway.example/session";
  assert.equal(
    classifyWorkspaceHost(
      { hostId: "machine-a", url },
      { hostId: "machine-b", url },
    ),
    "foreign",
  );
});

test("missing id from an older daemon conservatively treats its tree as own", () => {
  assert.equal(
    classifyWorkspaceHost(
      { hostId: null, url: "ws://phone-alias/session" },
      { hostId: "machine-a", url: "ws://desktop-alias/session" },
    ),
    "own",
  );
});

test("own workspace chooses notes vault while foreign chooses linked vault only", () => {
  assert.equal(
    notesVaultForWorkspace(workspace("machine-a"), { hostId: "machine-a" }),
    "/vault/personal",
  );
  assert.equal(
    notesVaultForWorkspace(workspace("machine-b"), { hostId: "machine-a" }),
    "/vault/shared",
  );
  assert.equal(
    notesVaultForWorkspace(
      { ...workspace("machine-b"), linked_vault_dir: null },
      { hostId: "machine-a" },
    ),
    null,
  );
});