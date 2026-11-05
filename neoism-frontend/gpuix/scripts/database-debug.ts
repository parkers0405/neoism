import assert from "node:assert/strict";
import { Database } from "bun:sqlite";
import { chmodSync, existsSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { frontendRoot, verifiedDisplayBinding } from "../src/display-binding";

/** Real sidebar objects: open any of these as ordinary buffer tabs. */
export const databaseFixtureObjects = [
  "apples", "bananas", "grapes", "fruit_audit", "stock_summary", "grape_origins",
  "growers", "orchards", "suppliers", "warehouses", "customers", "employees",
  "product_catalog", "purchase_orders", "purchase_order_items", "shipments",
  "shipment_items", "inventory_lots", "quality_inspections", "price_history",
  "warehouse_stock", "order_totals",
] as const;
export const catalogColumns = 24;
export const appleColumns = 25;

/** Supply-chain fixture, independent of the original fruit/audit SQL contracts. */
function seedSupplyChain(db: Database) {
  db.exec(`
    CREATE TABLE growers (id INTEGER PRIMARY KEY, name TEXT NOT NULL, country TEXT, certified_organic INTEGER NOT NULL, contact_email TEXT);
    CREATE TABLE orchards (id INTEGER PRIMARY KEY, grower_id INTEGER REFERENCES growers(id), name TEXT NOT NULL, hectares REAL, elevation_m INTEGER);
    CREATE TABLE suppliers (id INTEGER PRIMARY KEY, name TEXT NOT NULL, payment_terms TEXT, lead_days INTEGER);
    CREATE TABLE warehouses (id INTEGER PRIMARY KEY, name TEXT NOT NULL, city TEXT, capacity_pallets INTEGER, refrigerated INTEGER);
    CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT NOT NULL, segment TEXT, city TEXT, credit_limit REAL);
    CREATE TABLE employees (id INTEGER PRIMARY KEY, name TEXT NOT NULL, warehouse_id INTEGER REFERENCES warehouses(id), role TEXT, shift TEXT);
    -- Deliberately wide: mixed types, long headers/text, NULLs, and 24 columns.
    CREATE TABLE product_catalog (
      id INTEGER PRIMARY KEY, sku TEXT NOT NULL UNIQUE, display_name TEXT NOT NULL,
      fruit_family TEXT, variety TEXT, country_of_origin TEXT, grower_id INTEGER REFERENCES growers(id),
      supplier_id INTEGER REFERENCES suppliers(id), organic INTEGER, grade TEXT, package_type TEXT,
      units_per_case INTEGER, case_weight_kg REAL, unit_cost REAL, retail_price REAL,
      currency TEXT, minimum_order_cases INTEGER, reorder_point INTEGER, shelf_life_days INTEGER,
      storage_temperature_c REAL, available_from TEXT, discontinued INTEGER, tasting_notes TEXT, certification_reference TEXT
    );
    CREATE TABLE purchase_orders (id INTEGER PRIMARY KEY, supplier_id INTEGER REFERENCES suppliers(id), ordered_at TEXT, status TEXT, expected_at TEXT);
    CREATE TABLE purchase_order_items (id INTEGER PRIMARY KEY, order_id INTEGER REFERENCES purchase_orders(id), product_id INTEGER REFERENCES product_catalog(id), cases INTEGER, case_cost REAL);
    CREATE TABLE shipments (id INTEGER PRIMARY KEY, customer_id INTEGER REFERENCES customers(id), warehouse_id INTEGER REFERENCES warehouses(id), shipped_at TEXT, status TEXT, tracking_reference TEXT);
    CREATE TABLE shipment_items (id INTEGER PRIMARY KEY, shipment_id INTEGER REFERENCES shipments(id), product_id INTEGER REFERENCES product_catalog(id), cases INTEGER, unit_price REAL);
    CREATE TABLE inventory_lots (id INTEGER PRIMARY KEY, product_id INTEGER REFERENCES product_catalog(id), warehouse_id INTEGER REFERENCES warehouses(id), orchard_id INTEGER REFERENCES orchards(id), lot_code TEXT, cases_on_hand INTEGER, harvested_at TEXT, expires_at TEXT);
    CREATE TABLE quality_inspections (id INTEGER PRIMARY KEY, lot_id INTEGER REFERENCES inventory_lots(id), employee_id INTEGER REFERENCES employees(id), inspected_at TEXT, brix REAL, passed INTEGER, notes TEXT);
    CREATE TABLE price_history (id INTEGER PRIMARY KEY, product_id INTEGER REFERENCES product_catalog(id), effective_at TEXT, previous_price REAL, new_price REAL, reason TEXT);
    CREATE INDEX idx_inventory_warehouse ON inventory_lots(warehouse_id, product_id);
    CREATE INDEX idx_order_items_order ON purchase_order_items(order_id);
    CREATE VIEW warehouse_stock AS
      SELECT w.name AS warehouse, p.fruit_family, count(*) AS lots, sum(l.cases_on_hand) AS cases
      FROM inventory_lots l JOIN warehouses w ON w.id = l.warehouse_id JOIN product_catalog p ON p.id = l.product_id
      GROUP BY w.id, p.fruit_family;
    CREATE VIEW order_totals AS
      SELECT o.id, s.name AS supplier, o.status, sum(i.cases) AS cases, round(sum(i.cases * i.case_cost), 2) AS total_cost
      FROM purchase_orders o JOIN suppliers s ON s.id = o.supplier_id JOIN purchase_order_items i ON i.order_id = o.id
      GROUP BY o.id;

    INSERT INTO growers VALUES (1, 'Valley Cooperative', 'Italy', 1, 'harvest@valley.example'), (2, 'Loire Orchards', 'France', 0, NULL), (3, 'Pacific Fruit', 'USA', 1, 'orders@pacific.example');
    INSERT INTO orchards VALUES (1, 1, 'South terraces', 24.5, 430), (2, 2, 'River meadow', 18.2, 110), (3, 3, 'Highland grove', 42.8, 820);
    INSERT INTO suppliers VALUES (1, 'Fresh Routes', 'Net 30', 3), (2, 'Harvest Direct', 'Net 14', 5), (3, 'Orchard Partners', 'Net 45', 7);
    INSERT INTO warehouses VALUES (1, 'North cold store', 'Lyon', 450, 1), (2, 'Central distribution', 'Milan', 720, 1), (3, 'Coastal cross-dock', 'Genoa', 180, 0);
    INSERT INTO customers VALUES (1, 'Market Square', 'Retail', 'Paris', 15000), (2, 'School Kitchens', 'Wholesale', 'Turin', 28000), (3, 'Weekend Markets', 'Independent', 'Nice', 4500), (4, 'Juice Works', 'Processing', 'Lyon', 32000);
    INSERT INTO employees VALUES (1, 'Alex Morgan', 1, 'Quality lead', 'Morning'), (2, 'Sam Rivera', 2, 'Receiver', 'Evening'), (3, 'Robin Chen', 3, 'Dispatcher', 'Morning');
    WITH RECURSIVE series(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM series WHERE n < 48)
    INSERT INTO product_catalog
      SELECT n, printf('FRUIT-%04d', n), printf('%s harvest selection %02d', CASE n % 3 WHEN 0 THEN 'Apple' WHEN 1 THEN 'Grape' ELSE 'Banana' END, n),
        CASE n % 3 WHEN 0 THEN 'Apples' WHEN 1 THEN 'Grapes' ELSE 'Bananas' END,
        CASE n % 3 WHEN 0 THEN 'Gala' WHEN 1 THEN 'Muscat' ELSE 'Cavendish' END,
        CASE n % 3 WHEN 0 THEN 'Italy' WHEN 1 THEN 'France' ELSE 'USA' END,
        1 + n % 3, 1 + n % 3, n % 2, CASE n % 2 WHEN 0 THEN 'Premium' ELSE 'Standard' END,
        CASE n % 2 WHEN 0 THEN 'Reusable crate' ELSE 'Cardboard case' END,
        12 + n % 4, 4.5 + n % 6 * 0.5, round(0.8 + n * 0.03, 2), round(1.5 + n * 0.07, 2),
        'EUR', 2 + n % 5, 20 + n, 7 + n % 14, 2.0 + n % 4,
        date('2026-09-01', printf('+%d days', n)), n % 17 = 0,
        CASE WHEN n % 4 = 0 THEN NULL ELSE 'Fresh harvest · crisp texture; keep chilled and protect from direct sunlight during transport.' END,
        CASE WHEN n % 2 = 0 THEN printf('EU-ORGANIC-2026-%04d', n) ELSE NULL END
      FROM series;
    INSERT INTO purchase_orders SELECT id, supplier_id, date('2026-09-01', printf('+%d days', id)), CASE id % 3 WHEN 0 THEN 'received' WHEN 1 THEN 'confirmed' ELSE 'in transit' END, date('2026-09-08', printf('+%d days', id)) FROM product_catalog WHERE id <= 12;
    INSERT INTO purchase_order_items SELECT id, 1 + (id - 1) % 12, id, 5 + id % 20, round(unit_cost * units_per_case, 2) FROM product_catalog;
    INSERT INTO shipments SELECT id, 1 + id % 4, 1 + id % 3, date('2026-10-01', printf('+%d days', id)), CASE id % 2 WHEN 0 THEN 'delivered' ELSE 'in transit' END, printf('TRACK-%06d', id) FROM product_catalog WHERE id <= 16;
    INSERT INTO shipment_items SELECT id, 1 + (id - 1) % 16, id, 2 + id % 8, retail_price FROM product_catalog;
    INSERT INTO inventory_lots SELECT id, id, 1 + id % 3, 1 + id % 3, printf('LOT-2026-%04d', id), 10 + id * 3, date('2026-10-01', printf('+%d days', id % 10)), date('2026-10-20', printf('+%d days', id % 10)) FROM product_catalog;
    INSERT INTO quality_inspections SELECT id, id, 1 + id % 3, '2026-10-12T08:30:00Z', 10.5 + id % 9 * 0.4, id % 7 != 0, CASE WHEN id % 7 = 0 THEN 'Bruising found; hold for review' ELSE NULL END FROM inventory_lots;
    INSERT INTO price_history SELECT id, id, '2026-10-01', round(retail_price - 0.15, 2), retail_price, CASE id % 2 WHEN 0 THEN 'Seasonal adjustment' ELSE 'Freight cost update' END FROM product_catalog;
  `);
}

/** Always creates a fresh scratch workspace; never accepts an existing DB path.
 * Exported for future full-App native checks without duplicating seed SQL. */
export function seedDatabaseWorkspace() {
  const workspace = mkdtempSync(join(tmpdir(), "neoism-database-preview-"));
  const path = join(workspace, "fruit.db");
  const db = new Database(path, { create: true });
  try {
    db.exec(`
      PRAGMA journal_mode = DELETE;
      PRAGMA foreign_keys = ON;
      CREATE TABLE apples (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL DEFAULT 'New apple',
        origin TEXT DEFAULT 'Italy',
        quantity INTEGER NOT NULL DEFAULT 0 CHECK(quantity >= 0),
        price REAL DEFAULT 1.25,
        variety TEXT,
        color TEXT DEFAULT 'red',
        size_grade TEXT DEFAULT 'medium',
        organic INTEGER DEFAULT 0,
        grower TEXT,
        orchard TEXT,
        harvested_at TEXT,
        packed_at TEXT,
        best_before TEXT,
        lot_code TEXT,
        supplier TEXT,
        warehouse TEXT,
        storage_temperature_c REAL DEFAULT 3,
        weight_g INTEGER,
        sugar_brix REAL,
        currency TEXT DEFAULT 'EUR',
        sku TEXT,
        barcode TEXT,
        tasting_notes TEXT,
        certification TEXT
      );
      CREATE TABLE bananas (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL DEFAULT 'Cavendish',
        origin TEXT DEFAULT 'Ecuador',
        ripeness TEXT DEFAULT 'green',
        price REAL DEFAULT 0.75
      );
      CREATE TABLE grapes (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL DEFAULT 'New grape',
        origin TEXT DEFAULT 'France',
        quantity INTEGER NOT NULL DEFAULT 1,
        price REAL DEFAULT 2.5,
        notes TEXT,
        payload BLOB,
        harvest_id INTEGER DEFAULT 9007199254740993
      );
      CREATE TABLE fruit_audit (
        id INTEGER PRIMARY KEY,
        table_name TEXT NOT NULL,
        event TEXT NOT NULL,
        row_id INTEGER,
        occurred_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
      );
      CREATE INDEX idx_apples_origin ON apples(origin);
      CREATE INDEX idx_apples_price ON apples(price);
      CREATE INDEX idx_bananas_origin ON bananas(origin);
      CREATE INDEX idx_grapes_origin ON grapes(origin);
      CREATE INDEX idx_grapes_price ON grapes(price);
      CREATE UNIQUE INDEX idx_grapes_name ON grapes(name);
      CREATE VIEW stock_summary AS
        SELECT origin, count(*) AS varieties, sum(quantity) AS quantity, round(avg(price), 2) AS average_price
        FROM (SELECT origin, quantity, price FROM apples UNION ALL SELECT origin, quantity, price FROM grapes)
        GROUP BY origin;
      CREATE VIEW grape_origins AS
        SELECT origin, count(*) AS varieties, min(price) AS lowest_price FROM grapes GROUP BY origin;
    `);
    const origins = ["Italy", "France", "USA", "Japan", "Spain"];
    db.transaction(() => {
      const apple = db.prepare("INSERT INTO apples(name, origin, quantity, price) VALUES (?, ?, ?, ?)");
      for (let i = 0; i < 125; i++) apple.run(`${["Gala", "Fuji", "Honeycrisp", "Granny Smith", "Pink Lady"][i % 5]} ${i + 1}`, origins[i % 5]!, 10 + i, Math.round((1.25 + i % 9 * 0.15) * 100) / 100);
      const banana = db.prepare("INSERT INTO bananas(name, origin, ripeness, price) VALUES (?, ?, ?, ?)");
      for (let i = 0; i < 18; i++) banana.run(`Cavendish ${i + 1}`, ["Ecuador", "Colombia", "Costa Rica"][i % 3]!, ["green", "yellow", "ripe"][i % 3]!, 0.75 + i % 4 * 0.1);
      const grape = db.prepare("INSERT INTO grapes(name, origin, quantity, price, notes, payload) VALUES (?, ?, ?, ?, ?, ?)");
      for (let i = 0; i < 12; i++) grape.run(`${["Concord", "Muscat", "Thompson", "Cotton Candy"][i % 4]} ${i + 1}`, origins[i % 5]!, 3 + i, 2.5 + i % 5 * 0.25,
        i % 3 === 0 ? "" : i % 3 === 1 ? null : "Seedless · fresh harvest", i === 0 ? new Uint8Array([0xde, 0xad, 0xbe, 0xef]) : null);
      db.run("INSERT INTO fruit_audit(table_name, event, row_id) VALUES ('fixture', 'seeded', NULL)");
      seedSupplyChain(db);
      db.exec(`UPDATE apples SET
        variety = substr(name, 1, length(name) - length(CAST(id AS TEXT)) - 1),
        color = CASE id % 3 WHEN 0 THEN 'green' WHEN 1 THEN 'red' ELSE 'yellow' END,
        size_grade = CASE id % 2 WHEN 0 THEN 'large' ELSE 'medium' END,
        organic = id % 2,
        grower = CASE id % 3 WHEN 0 THEN 'Valley Cooperative' WHEN 1 THEN 'Loire Orchards' ELSE 'Pacific Fruit' END,
        orchard = printf('Orchard %02d', 1 + id % 12),
        harvested_at = date('2026-09-01', printf('+%d days', id % 30)),
        packed_at = date('2026-09-03', printf('+%d days', id % 30)),
        best_before = date('2026-10-01', printf('+%d days', id % 30)),
        lot_code = printf('APPLE-2026-%04d', id),
        supplier = CASE id % 2 WHEN 0 THEN 'Fresh Routes' ELSE 'Harvest Direct' END,
        warehouse = CASE id % 2 WHEN 0 THEN 'North cold store' ELSE 'Central distribution' END,
        storage_temperature_c = 2 + id % 4,
        weight_g = 140 + id % 60,
        sugar_brix = 10.5 + id % 9 * 0.4,
        sku = printf('APL-%05d', id),
        barcode = printf('4901234%06d', id),
        tasting_notes = CASE WHEN id % 4 = 0 THEN NULL ELSE 'Crisp, fresh harvest' END,
        certification = CASE WHEN id % 2 = 1 THEN 'EU Organic' ELSE NULL END`);
    })();
    for (const table of ["apples", "grapes"]) {
      for (const operation of ["insert", "update", "delete"]) {
        // Identifiers are fixed fixture constants, never user input.
        db.exec(`CREATE TRIGGER ${table}_after_${operation} AFTER ${operation.toUpperCase()} ON ${table}
          BEGIN INSERT INTO fruit_audit(table_name, event, row_id) VALUES ('${table}', '${operation}', ${operation === "delete" ? "OLD" : "NEW"}.id); END;`);
      }
    }
    assert.equal(db.query<{ integrity_check: string }, []>("PRAGMA integrity_check").get()?.integrity_check, "ok");
  } finally { db.close(); }
  chmodSync(path, 0o600);
  return { workspace, database: path, rows: { apples: 125, bananas: 18, grapes: 12, fruit_audit: 1 } };
}

/** Scratch-only SQL contract check. This is not an App/UI end-to-end test. */
export function checkDatabaseSeed(path: string) {
  const db = new Database(path, { readwrite: true });
  try {
    const objects = db.query<{ name: string }, []>("SELECT name FROM sqlite_schema WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%'").all().map(row => row.name).sort();
    assert.deepEqual(objects, [...databaseFixtureObjects].sort());
    assert.equal(db.query("PRAGMA table_info(apples)").all().length, appleColumns);
    assert.equal(db.query("PRAGMA table_info(product_catalog)").all().length, catalogColumns);
    assert.equal(db.query<{ count: number }, []>("SELECT count(*) AS count FROM product_catalog").get()?.count, 48);
    assert.equal(db.query<{ count: number }, []>("SELECT count(*) AS count FROM order_totals").get()?.count, 12);
    assert.deepEqual(db.query("PRAGMA foreign_key_check").all(), []);
    assert.equal(db.query<{ count: number }, []>("SELECT count(*) AS count FROM apples").get()?.count, 125);
    assert.equal(db.query<{ harvest_id: string }, []>("SELECT CAST(harvest_id AS TEXT) AS harvest_id FROM grapes LIMIT 1").get()?.harvest_id, "9007199254740993");
    assert.equal(db.query<{ bytes: number }, []>("SELECT length(payload) AS bytes FROM grapes WHERE id = 1").get()?.bytes, 4);
    assert.equal(db.query<{ count: number }, []>("SELECT count(*) AS count FROM grapes WHERE notes IS NULL").get()?.count, 4);
    assert.equal(db.query<{ count: number }, []>("SELECT count(*) AS count FROM grapes WHERE notes = ''").get()?.count, 4);
    assert.equal(db.query<{ count: number }, []>("SELECT count(*) AS count FROM apples WHERE origin IN ('Italy', 'France') AND quantity >= 20").get()?.count, 46);
    assert.equal(db.query<{ count: number }, []>("SELECT count(*) AS count FROM grape_origins").get()?.count, 5);
    db.exec("BEGIN");
    try {
      db.run("INSERT INTO apples DEFAULT VALUES");
      const row = db.query<{ id: number; name: string; origin: string; quantity: number; price: number }, []>("SELECT * FROM apples ORDER BY id DESC LIMIT 1").get()!;
      assert.equal(row.name, "New apple"); assert.equal(row.origin, "Italy"); assert.equal(row.quantity, 0); assert.equal(row.price, 1.25);
      db.run("UPDATE apples SET name = '', origin = NULL, price = 3.75 WHERE id = ?", [row.id]);
      assert.deepEqual(db.query("SELECT name, origin, price FROM apples WHERE id = ?").get(row.id), { name: "", origin: null, price: 3.75 });
      db.run("DELETE FROM apples WHERE id = ?", [row.id]);
      const events = db.query<{ event: string }, [number]>("SELECT event FROM fruit_audit WHERE table_name = 'apples' AND row_id = ? ORDER BY id").all(row.id).map(row => row.event);
      assert.deepEqual(events, ["insert", "update", "delete"]);
    } finally { db.exec("ROLLBACK"); }
    assert.equal(db.query<{ count: number }, []>("SELECT count(*) AS count FROM fruit_audit").get()?.count, 1, "Validation must leave preview data unchanged");
  } finally { db.close(); }
}

async function main() {
  const args = process.argv.slice(2);
  if (args.includes("--help")) {
    console.log("Usage: bun scripts/database-debug.ts [--seed-only | --check]\nDefault: seed fruit.db and open a separate real debug Neoism app using launch-block-debug.ts.\n--seed-only: create/validate the fixture without loading or launching native artifacts.\n--check: validate debug display/core/daemon presence and fixture SQL contracts; no app launch and no native UI automation.");
    return;
  }
  if (args.some(arg => !["--seed-only", "--check"].includes(arg)) || args.includes("--seed-only") && args.includes("--check")) throw new Error("Use --help; existing database/workspace paths are deliberately not accepted.");
  const repository = resolve(frontendRoot, "../..");
  const core = resolve(process.env.NEOISM_GPUIX_CORE || join(frontendRoot, "artifacts", process.platform === "darwin" ? "libneoism_gpuix_core.dylib" : process.platform === "win32" ? "neoism_gpuix_core.dll" : "libneoism_gpuix_core.so"));
  const daemon = resolve(process.env.NEOISM_GPUIX_WORKSPACE_DAEMON || join(repository, "target/debug", process.platform === "win32" ? "neoism-workspace-daemon.exe" : "neoism-workspace-daemon"));
  if (!args.includes("--seed-only")) {
    for (const artifact of [core, daemon]) if (!existsSync(artifact)) throw new Error(`Debug artifact missing: ${artifact}. Wait for the authorized debug builds to finish.`);
  }
  const fixture = seedDatabaseWorkspace();
  checkDatabaseSeed(fixture.database);
  if (args.includes("--seed-only")) { console.log(JSON.stringify({ ...fixture, mode: "seed-only", validated: true })); return; }
  if (args.includes("--check")) {
    const display = verifiedDisplayBinding(frontendRoot, true);
    console.log(JSON.stringify({ ...fixture, mode: "artifact-and-fixture-check", core, daemon, display: display.binding, profile: display.stamp.profile, validated: true, nativeUiChecked: false }));
    return;
  }
  // All theme copying, private ports/state, display verification and detached
  // app/service lifecycle stay in the existing launcher. Do not reproduce it.
  const child = Bun.spawn([process.execPath, join(frontendRoot, "scripts/launch-block-debug.ts"), fixture.workspace], {
    cwd: frontendRoot, env: { ...process.env, NEOISM_GPUIX_DISPLAY_DEBUG: "1", NEOISM_GPUIX_CORE: core, NEOISM_GPUIX_WORKSPACE_DAEMON: daemon }, stdout: "pipe", stderr: "inherit",
  });
  const output = new Response(child.stdout).text();
  const code = await child.exited;
  const text = await output;
  if (code !== 0) throw new Error(`Debug launcher exited ${code}. Scratch fixture retained at ${fixture.database}. ${text.trim()}`);
  const launched = JSON.parse(text.trim()) as Record<string, unknown>;
  console.log(JSON.stringify({ ...launched, ...fixture, mode: "preview", isolated: true }));
  console.error(`Separate debug app launched. Profile: ${String(launched.profile)}\nWorkspace: ${fixture.workspace}\nFixture database: ${fixture.database}\nLog: ${String(launched.log)}\nClick fruit.db in Files, then open tables from the shared Databases sidebar: ${databaseFixtureObjects.length} real tables/views use normal Neoism buffer tabs. Open several to test tab-strip scrolling.\nThe default apples table has ${appleColumns} columns for horizontal header scrolling and 125 rows for paging. product_catalog has ${catalogColumns} columns; grapes has NULL/empty text, a read-only BLOB, and an exact 64-bit integer.\nNew rows use DEFAULT until explicitly changed. Apply commits only to this scratch fruit.db; deletes require inline confirmation.\nNo automated UI actions were sent. The app and scratch workspace remain available for manual testing.`);
}

if (import.meta.main) await main();