import assert from "node:assert/strict";
import test from "node:test";

import { shouldRefreshFileTree } from "./fileTreeInvalidation";

test("refreshes an invalidation for the active workspace root", () => {
  assert.equal(shouldRefreshFileTree("/srv/project", "/srv/project"), true);
});

test("ignores delayed invalidations from another workspace", () => {
  assert.equal(shouldRefreshFileTree("/srv/old", "/srv/current"), false);
});

test("ignores invalidations before a workspace root is confirmed", () => {
  assert.equal(shouldRefreshFileTree("/srv/project", null), false);
});