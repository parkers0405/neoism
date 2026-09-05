export interface AgentTabIdentity {
  routeId: number;
  nextRouteId: number;
  title: string;
}

/** Allocate a route that cannot alias an existing web agent tab. */
export function allocateAgentTabIdentity(
  existingRouteIds: readonly number[],
  candidate: number,
): AgentTabIdentity {
  const used = new Set(existingRouteIds);
  let routeId = Math.max(1, Math.trunc(candidate));
  while (used.has(routeId)) routeId += 1;
  return {
    routeId,
    nextRouteId: routeId + 1,
    title: `Neoism ${routeId}`,
  };
}
