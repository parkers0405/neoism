import { expect, test } from "bun:test";
import { fileIcon } from "../src/design/file-icons";
import { isDatabasePath, isDatabaseSidecarPath } from "../src/database/file-path";

test("database files share the database glyph and routing, sidecars are decoration only", () => {
  for (const name of ["agent.db", "agent.sqlite", "agent.sqlite3", "agent.turso.db", "agent.turso", "agent.TursoDB", "old.db3"]) {
    expect(isDatabasePath(name)).toBe(true);
    expect(fileIcon(name).glyph).toBe("\uf1c0");
    for (const suffix of ["-wal", "-shm"]) {
      expect(isDatabasePath(name + suffix)).toBe(false);
      expect(isDatabaseSidecarPath(name + suffix)).toBe(true);
      expect(fileIcon(name + suffix).glyph).toBe("\uf1c0");
      expect(fileIcon(name + suffix).color).not.toBe(fileIcon(name).color);
    }
  }
  for (const name of ["notes.txt", "notes-wal", "notes.txt-shm", "agent.db.bak"]) {
    expect(isDatabasePath(name)).toBe(false);
    expect(isDatabaseSidecarPath(name)).toBe(false);
    expect(fileIcon(name).glyph).not.toBe("\uf1c0");
  }
});