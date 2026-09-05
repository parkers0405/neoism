export type WheelOwner =
  | "chrome"
  | "active-content"
  | "terminal";

export interface WheelRoute {
  owner: WheelOwner;
  route: () => boolean;
}

/** Run wheel candidates in visual z-order, stopping after one owner claims it. */
export function routeWheelToFirstOwner(routes: readonly WheelRoute[]): WheelOwner | null {
  for (const candidate of routes) {
    if (candidate.route()) return candidate.owner;
  }
  return null;
}