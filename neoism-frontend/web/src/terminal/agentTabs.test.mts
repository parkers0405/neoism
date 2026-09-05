import { test } from "node:test";
import assert from "node:assert/strict";

import { allocateAgentTabIdentity } from "./agentTabs.ts";

test("every agent tab receives a distinct route and label", () => {
  const first = allocateAgentTabIdentity([], 1);
  const second = allocateAgentTabIdentity([first.routeId], first.nextRouteId);
  const third = allocateAgentTabIdentity(
    [first.routeId, second.routeId, 99],
    second.routeId,
  );

  assert.deepEqual(first, { routeId: 1, nextRouteId: 2, title: "Neoism 1" });
  assert.deepEqual(second, { routeId: 2, nextRouteId: 3, title: "Neoism 2" });
  assert.equal(third.routeId, 3);
  assert.equal(third.title, "Neoism 3");
});
