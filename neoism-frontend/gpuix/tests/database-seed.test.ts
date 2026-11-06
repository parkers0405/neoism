import { expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import { rm } from "node:fs/promises";
import { appleColumns, catalogColumns, checkDatabaseSeed, databaseFixtureObjects, seedDatabaseWorkspace } from "../scripts/database-debug";

test("preview offers real populated sidebar categories and a wide catalog without changing fruit contracts", async () => {
  const fixture = seedDatabaseWorkspace();
  try {
    checkDatabaseSeed(fixture.database);
    const db = new Database(fixture.database, { readonly: true });
    try {
      expect(databaseFixtureObjects).toHaveLength(22);
      expect(db.query("PRAGMA table_info(apples)").all()).toHaveLength(appleColumns);
      expect(db.query<{sku:string;weight_g:number;variety:string},[]>("SELECT sku, weight_g, variety FROM apples WHERE id=1").get()).toEqual({sku:"APL-00001",weight_g:141,variety:"Gala"});
      expect(db.query("PRAGMA table_info(product_catalog)").all()).toHaveLength(catalogColumns);
      for (const name of databaseFixtureObjects) {
        expect(db.query<{ count: number }, []>(`SELECT count(*) AS count FROM "${name}"`).get()!.count).toBeGreaterThan(0);
      }
      for (const [name, count] of Object.entries(fixture.rows)) {
        expect(db.query<{ count: number }, []>(`SELECT count(*) AS count FROM "${name}"`).get()!.count).toBe(count);
      }
    } finally { db.close(); }
  } finally {
    // SQLite's deferred-close statements and Windows file scanners may retain
    // a scratch handle briefly after close. Release unreachable statements and
    // await bounded cleanup rather than failing successful SQL assertions.
    Bun.gc(true);
    await rm(fixture.workspace, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});