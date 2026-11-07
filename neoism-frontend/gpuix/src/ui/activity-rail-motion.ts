import type { PaintCommand } from "./PaintLayer";

export function railIconMotion(hover: number, clickElapsed: number, scale: number) {
  const t = Math.max(0, Math.min(1, clickElapsed / 420));
  const bounce = t < 1 ? Math.sin(t * Math.PI * 3) * (1 - t) ** 2 : 0;
  return { zoom: 1 + hover * .12 - bounce * .10, lift: (-hover * 2 - bounce * 3) * scale };
}

/** Animate only ink contained in a button, never the rail background or hit targets. */
export function animateRailIcon(command: PaintCommand, rect: number[], zoom: number, lift: number): PaintCommand {
  const [x, y, w, h] = rect, cx = x + w / 2, cy = y + h / 2;
  const point = (p: number[]) => [cx + (p[0] - cx) * zoom, cy + (p[1] - cy) * zoom + lift];
  const box = (b: number[]) => [...point(b), b[2] * zoom, b[3] * zoom];
  let bounds: number[];
  if ("rect" in command) bounds = command.rect;
  else if (command.kind === "text") {
    const r = command.raster;
    bounds = r ? [command.x + r.dx, command.y + r.dy, r.width / (r.scale || 1), r.height / (r.scale || 1)]
      : [command.x, command.y, command.width, command.size];
  } else {
    const points = command.kind === "ring" ? command.outer : command.points;
    const xs = points.map(p => p[0]), ys = points.map(p => p[1]);
    bounds = [Math.min(...xs), Math.min(...ys), Math.max(...xs) - Math.min(...xs), Math.max(...ys) - Math.min(...ys)];
  }
  if (bounds[0] < x || bounds[1] < y || bounds[0] + bounds[2] > x + w || bounds[1] + bounds[3] > y + h) return command;
  switch (command.kind) {
    case "rect": return { ...command, rect: box(command.rect), radii: command.radii.map(r => r * zoom) };
    case "image": case "raster-text": return { ...command, rect: box(command.rect) };
    case "text": {
      if (command.raster) return { kind: "raster-text", rect: box(bounds), raster: command.raster, layer: command.layer, clip: command.clip };
      const [px, py] = point([command.x, command.y]);
      return { ...command, x: px, y: py, size: command.size * zoom, width: command.width * zoom };
    }
    case "polygon": return { ...command, points: command.points.map(point) };
    case "ring": return { ...command, outer: command.outer.map(point), inner: command.inner.map(point) };
  }
}