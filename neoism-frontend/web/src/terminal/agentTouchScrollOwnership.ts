export type AgentTouchScrollOwner = "nested" | "timeline" | "horizontal";

export function agentMomentumAxis(
  owner: AgentTouchScrollOwner | null,
): "x" | "y" | "dominant" {
  if (owner === "horizontal") return "x";
  if (owner === "timeline" || owner === "nested") return "y";
  return "dominant";
}

export interface AgentTouchScrollRoutes {
  /** 0 resolves by hit test, 1 keeps picker/side-panel ownership, 2 keeps timeline ownership. */
  dragVertical: (x: number, y: number, dy: number, owner: 0 | 1 | 2) => number;
  dragHorizontal: (x: number, y: number, dx: number) => boolean;
}

/**
 * Locks an Agent direct-manipulation gesture to the surface under touch-down.
 *
 * Rendered chunks move underneath the finger while the outer timeline scrolls,
 * so hit-testing every sample makes ownership jump between diff cards and the
 * timeline. Keeping both the original point and the resolved owner also lets
 * release momentum follow the same route after the finger has gone away.
 */
export class AgentTouchScrollOwnership {
  private start: { x: number; y: number } | null = null;
  private owner: AgentTouchScrollOwner | null = null;

  begin(x: number, y: number): void {
    this.start = { x, y };
    this.owner = null;
  }

  reset(): void {
    this.start = null;
    this.owner = null;
  }

  currentOwner(): AgentTouchScrollOwner | null {
    return this.owner;
  }

  route(
    x: number,
    y: number,
    dx: number,
    dy: number,
    surfaceAvailable: boolean,
    routes: AgentTouchScrollRoutes,
  ): boolean {
    const start = this.start;
    if (!start || !surfaceAvailable) {
      this.reset();
      return false;
    }

    if (this.owner === null) {
      const totalX = x - start.x;
      const totalY = y - start.y;
      if (Math.abs(totalX) > Math.abs(totalY)) {
        if (!routes.dragHorizontal(start.x, start.y, -dx)) return false;
        this.owner = "horizontal";
        return true;
      }
    }

    if (this.owner === "horizontal") {
      if (Math.abs(dx) <= Number.EPSILON) return true;
      return routes.dragHorizontal(start.x, start.y, -dx);
    }

    if (
      (this.owner === "timeline" || this.owner === "nested") &&
      Math.abs(dy) <= Number.EPSILON
    ) {
      return true;
    }

    const ownerCode = this.owner === "nested" ? 1 : this.owner === "timeline" ? 2 : 0;
    const consumed = routes.dragVertical(start.x, start.y, dy, ownerCode);
    if (consumed === 0) return false;
    this.owner = consumed === 2 ? "timeline" : "nested";
    return true;
  }
}