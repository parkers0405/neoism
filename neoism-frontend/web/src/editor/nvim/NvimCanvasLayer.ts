import type {
  CrdtServerMessage,
  EditorClientMessage,
  EditorServerMessage,
} from "../../workspace/types";
import {
  NvimGridModel,
  type NvimGridCellSnapshot,
  type NvimGridSnapshot,
} from "./NvimGridModel";
import { selectionSegments } from "./NvimPresence";

export interface NvimCanvasLayerOptions {
  mount: HTMLElement;
  sendEditor: (message: EditorClientMessage) => void;
  activeSurfaceId: () => string | null;
  focusHost: () => void;
}

export interface NvimCanvasViewport {
  x: number;
  y: number;
  w: number;
  h: number;
}

const FONT_FAMILY =
  '"Geist Mono", "Symbols Nerd Font Mono", ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", "Apple Color Emoji", "Segoe UI Emoji", "Noto Color Emoji", monospace';
const MIN_CELL_W = 4;
const MIN_CELL_H = 8;

export class NvimCanvasLayer {
  readonly canvas: HTMLCanvasElement;
  private readonly model = new NvimGridModel();
  private readonly ctx: CanvasRenderingContext2D | null;
  private viewport: NvimCanvasViewport | null = null;
  private visible = false;
  private dragging = false;
  private wheelRemainder = 0;

  constructor(private readonly options: NvimCanvasLayerOptions) {
    this.canvas = document.createElement("canvas");
    this.canvas.className = "nvim-grid-canvas";
    this.canvas.tabIndex = -1;
    this.ctx = this.canvas.getContext("2d", { alpha: false });
    this.installInputHandlers();
    options.mount.appendChild(this.canvas);
  }

  ingest(message: EditorServerMessage): void {
    this.model.ingest(message);
    const surfaceId = this.options.activeSurfaceId();
    if (surfaceId) {
      this.model.setActiveSurface(surfaceId);
    }
    this.render();
  }

  ingestPresence(message: CrdtServerMessage, localPeerId: string | null = null): void {
    this.model.ingestPresence(message, localPeerId);
    this.render();
  }

  setVisible(visible: boolean): void {
    this.visible = visible;
    this.canvas.hidden = !visible;
    this.canvas.style.pointerEvents = visible ? "auto" : "none";
    if (visible) this.render();
  }

  setViewport(viewport: NvimCanvasViewport): void {
    this.viewport = viewport;
    this.canvas.style.left = `${viewport.x}px`;
    this.canvas.style.top = `${viewport.y}px`;
    this.canvas.style.width = `${viewport.w}px`;
    this.canvas.style.height = `${viewport.h}px`;
    this.resizeBackingStore(viewport.w, viewport.h);
    this.render();
  }

  setActiveSurfaceId(surfaceId: string | null): void {
    this.model.setActiveSurface(surfaceId);
    this.render();
  }

  hasAnySnapshot(): boolean {
    return this.model.hasAnySnapshot();
  }

  hasSnapshotForSurface(surfaceId: string | null): boolean {
    return this.model.snapshotForSurface(surfaceId) !== null;
  }

  surfaceIds(): string[] {
    return this.model.surfaceIds();
  }

  snapshotForSurface(surfaceId: string | null): NvimGridSnapshot | null {
    return this.model.snapshotForSurface(surfaceId);
  }

  activeSnapshot(): NvimGridSnapshot | null {
    return this.model.activeSnapshot();
  }

  render(): void {
    if (!this.visible || !this.ctx) return;
    const viewport = this.viewport;
    if (!viewport || viewport.w <= 0 || viewport.h <= 0) return;
    const surfaceId = this.options.activeSurfaceId();
    if (surfaceId) this.model.setActiveSurface(surfaceId);
    const snapshot = this.model.activeSnapshot();
    if (!snapshot) return;

    const ctx = this.ctx;
    const dpr = window.devicePixelRatio || 1;
    ctx.save();
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.fillStyle = rgb(snapshot.default_bg);
    ctx.fillRect(0, 0, viewport.w, viewport.h);

    const metrics = this.cellMetrics(snapshot, viewport);
    ctx.textBaseline = "top";
    ctx.font = `${metrics.fontSize}px ${FONT_FAMILY}`;

    for (let row = 0; row < snapshot.height; row += 1) {
      for (let col = 0; col < snapshot.width; col += 1) {
        const cell = snapshot.cells[row * snapshot.width + col];
        if (!cell) continue;
        this.drawCell(ctx, cell, snapshot, row, col, metrics.cellW, metrics.cellH, metrics.fontSize);
      }
    }

    this.drawPeerPresence(ctx, snapshot, metrics.cellW, metrics.cellH, viewport);
    this.drawCursor(ctx, snapshot, metrics.cellW, metrics.cellH);
    this.drawPopupMenu(ctx, snapshot, metrics.cellW, metrics.cellH, viewport);
    this.drawStatus(ctx, snapshot, viewport);
    ctx.restore();
  }

  private drawCell(
    ctx: CanvasRenderingContext2D,
    cell: NvimGridCellSnapshot,
    snapshot: NvimGridSnapshot,
    row: number,
    col: number,
    cellW: number,
    cellH: number,
    fontSize: number,
  ): void {
    const x = col * cellW;
    const y = row * cellH;
    const bg = (cell.attrs & 32) !== 0 ? cell.fg : cell.bg;
    const fg = (cell.attrs & 32) !== 0 ? cell.bg || snapshot.default_bg : cell.fg;
    if (bg !== snapshot.default_bg) {
      ctx.fillStyle = rgb(bg);
      ctx.fillRect(x, y, cellW + 1, cellH + 1);
    }
    const ch = cell.ch.length > 0 ? cell.ch : " ";
    if (ch !== " ") {
      ctx.fillStyle = rgb(fg || snapshot.default_fg);
      ctx.font = `${(cell.attrs & 1) !== 0 ? "700 " : ""}${(cell.attrs & 2) !== 0 ? "italic " : ""}${fontSize}px ${FONT_FAMILY}`;
      ctx.fillText(ch, x, y + Math.max(0, (cellH - fontSize) / 2));
    }
    if ((cell.attrs & 4) !== 0) {
      ctx.fillStyle = rgb(fg || snapshot.default_fg);
      ctx.fillRect(x, y + cellH - 2, cellW, 1);
    }
    if ((cell.attrs & 16) !== 0) {
      ctx.fillStyle = rgb(fg || snapshot.default_fg);
      ctx.fillRect(x, y + Math.floor(cellH / 2), cellW, 1);
    }
  }

  private drawCursor(
    ctx: CanvasRenderingContext2D,
    snapshot: NvimGridSnapshot,
    cellW: number,
    cellH: number,
  ): void {
    if (!snapshot.cursorState?.visible) return;
    const x = snapshot.cursorState.col * cellW;
    const y = snapshot.cursorState.row * cellH;
    const insert = snapshot.mode === "insert" || snapshot.mode === "replace";
    ctx.fillStyle = insert ? "rgba(232, 232, 232, 0.95)" : "rgba(232, 232, 232, 0.28)";
    ctx.fillRect(x, y, insert ? Math.max(2, Math.floor(cellW * 0.16)) : cellW, cellH);
  }

  private drawPeerPresence(
    ctx: CanvasRenderingContext2D,
    snapshot: NvimGridSnapshot,
    cellW: number,
    cellH: number,
    viewport: NvimCanvasViewport,
  ): void {
    if (snapshot.presence.length === 0) return;
    ctx.save();
    for (const peer of snapshot.presence) {
      if (peer.selection) {
        ctx.fillStyle = rgba(peer.color, 0.18);
        for (const segment of selectionSegments(peer.selection, snapshot.width)) {
          const x = segment.startCol * cellW;
          const y = segment.row * cellH;
          const w = (segment.endCol - segment.startCol + 1) * cellW;
          ctx.fillRect(x, y, w, cellH);
        }
      }
    }

    for (const peer of snapshot.presence) {
      const x = peer.cursor.col * cellW;
      const y = peer.cursor.row * cellH;
      ctx.fillStyle = peer.colorCss;
      ctx.fillRect(x, y, Math.max(2, Math.floor(cellW * 0.16)), cellH);
      this.drawPeerLabel(ctx, peer.label, peer.colorCss, x, y, viewport);
    }
    ctx.restore();
  }

  private drawPeerLabel(
    ctx: CanvasRenderingContext2D,
    label: string,
    color: string,
    cursorX: number,
    cursorY: number,
    viewport: NvimCanvasViewport,
  ): void {
    const fontSize = 11;
    ctx.font = `700 ${fontSize}px ${FONT_FAMILY}`;
    const w = Math.max(1, Math.min(Math.max(1, viewport.w - 4), Math.ceil(ctx.measureText(label).width) + 10));
    const h = 16;
    const x = Math.max(2, Math.min(viewport.w - w - 2, cursorX));
    const y = Math.max(2, Math.min(viewport.h - h - 2, cursorY - h));
    ctx.fillStyle = color;
    ctx.fillRect(x, y, w, h);
    ctx.fillStyle = "#0b0d10";
    ctx.fillText(label, x + 5, y + 2);
  }

  private drawPopupMenu(
    ctx: CanvasRenderingContext2D,
    snapshot: NvimGridSnapshot,
    cellW: number,
    cellH: number,
    viewport: NvimCanvasViewport,
  ): void {
    const menu = snapshot.popupMenu;
    if (!menu || menu.items.length === 0) return;
    const rows = Math.min(8, menu.items.length);
    const widthChars = Math.min(
      60,
      Math.max(16, ...menu.items.slice(0, rows).map((item) => item.word.length + item.kind.length + item.menu.length + 4)),
    );
    const w = Math.min(viewport.w - 8, widthChars * cellW);
    const h = rows * cellH;
    const x = Math.min(viewport.w - w - 4, menu.anchor.col * cellW);
    const y = Math.min(viewport.h - h - 4, (menu.anchor.row + 1) * cellH);
    ctx.fillStyle = "rgba(18, 18, 18, 0.96)";
    ctx.fillRect(x, y, w, h);
    ctx.strokeStyle = "rgba(232, 232, 232, 0.35)";
    ctx.strokeRect(x + 0.5, y + 0.5, w - 1, h - 1);
    ctx.font = `${Math.max(10, Math.floor(cellH * 0.72))}px ${FONT_FAMILY}`;
    for (let i = 0; i < rows; i += 1) {
      const item = menu.items[i];
      const rowY = y + i * cellH;
      if (menu.selected === i) {
        ctx.fillStyle = "rgba(232, 232, 232, 0.18)";
        ctx.fillRect(x + 1, rowY, w - 2, cellH);
      }
      ctx.fillStyle = "#e8e8e8";
      ctx.fillText(`${item.word} ${item.kind} ${item.menu}`.trim(), x + 6, rowY + 1);
    }
  }

  private drawStatus(
    ctx: CanvasRenderingContext2D,
    snapshot: NvimGridSnapshot,
    viewport: NvimCanvasViewport,
  ): void {
    if (!snapshot.error && !snapshot.mode) return;
    const label = snapshot.error ?? snapshot.mode ?? "";
    ctx.font = `11px ${FONT_FAMILY}`;
    const w = Math.min(viewport.w - 8, Math.ceil(ctx.measureText(label).width) + 12);
    ctx.fillStyle = snapshot.error ? "rgba(120, 28, 38, 0.92)" : "rgba(18, 18, 18, 0.72)";
    ctx.fillRect(4, viewport.h - 22, w, 18);
    ctx.fillStyle = "#e8e8e8";
    ctx.fillText(label, 10, viewport.h - 19);
  }

  private installInputHandlers(): void {
    this.canvas.addEventListener("pointerdown", (event) => {
      if (!this.visible) return;
      event.preventDefault();
      this.options.focusHost();
      this.dragging = true;
      this.canvas.setPointerCapture(event.pointerId);
      this.sendMouse(event, "left", "press", 1);
    });
    this.canvas.addEventListener("pointermove", (event) => {
      if (!this.visible || !this.dragging) return;
      event.preventDefault();
      this.sendMouse(event, "left", "drag", 1);
    });
    this.canvas.addEventListener("pointerup", (event) => {
      if (!this.visible) return;
      event.preventDefault();
      this.dragging = false;
      this.sendMouse(event, "left", "release", 1);
      try {
        this.canvas.releasePointerCapture(event.pointerId);
      } catch {
        // Pointer capture may already be gone after browser cancellation.
      }
    });
    this.canvas.addEventListener("pointerleave", () => {
      this.dragging = false;
    });
    this.canvas.addEventListener(
      "wheel",
      (event) => {
        if (!this.visible) return;
        const snapshot = this.model.activeSnapshot();
        const viewport = this.viewport;
        if (!snapshot || !viewport) return;
        event.preventDefault();
        const { row, col, cellH } = this.pointerCell(event, snapshot, viewport);
        this.wheelRemainder += wheelDeltaPixels(event, cellH, viewport.h);
        const wholeRows = Math.trunc(this.wheelRemainder / cellH);
        if (wholeRows === 0) return;
        this.wheelRemainder -= wholeRows * cellH;
        this.options.sendEditor({
          MouseInput: {
            button: "wheel",
            action: wholeRows > 0 ? "down" : "up",
            modifier: nvimMouseModifier(event),
            grid: snapshot.gridId,
            row,
            col,
            count: Math.abs(wholeRows),
            ...(snapshot.surfaceId ? { surface_id: snapshot.surfaceId } : {}),
          },
        });
      },
      { passive: false },
    );
  }

  private sendMouse(event: PointerEvent, button: string, action: string, count: number): void {
    const snapshot = this.model.activeSnapshot();
    const viewport = this.viewport;
    if (!snapshot || !viewport) return;
    const { row, col } = this.pointerCell(event, snapshot, viewport);
    this.options.sendEditor({
      MouseInput: {
        button,
        action,
        modifier: nvimMouseModifier(event),
        grid: snapshot.gridId,
        row,
        col,
        count,
        ...(snapshot.surfaceId ? { surface_id: snapshot.surfaceId } : {}),
      },
    });
  }

  private pointerCell(
    event: MouseEvent,
    snapshot: NvimGridSnapshot,
    viewport: NvimCanvasViewport,
  ): { row: number; col: number; cellW: number; cellH: number } {
    const rect = this.canvas.getBoundingClientRect();
    const cellW = Math.max(MIN_CELL_W, viewport.w / Math.max(1, snapshot.width));
    const cellH = Math.max(MIN_CELL_H, viewport.h / Math.max(1, snapshot.height));
    const x = event.clientX - rect.left;
    const y = event.clientY - rect.top;
    return {
      row: clampInt(Math.floor(y / cellH), 0, Math.max(0, snapshot.height - 1)),
      col: clampInt(Math.floor(x / cellW), 0, Math.max(0, snapshot.width - 1)),
      cellW,
      cellH,
    };
  }

  private cellMetrics(
    snapshot: NvimGridSnapshot,
    viewport: NvimCanvasViewport,
  ): { cellW: number; cellH: number; fontSize: number } {
    const cellW = Math.max(MIN_CELL_W, viewport.w / Math.max(1, snapshot.width));
    const cellH = Math.max(MIN_CELL_H, viewport.h / Math.max(1, snapshot.height));
    return {
      cellW,
      cellH,
      fontSize: Math.max(8, Math.floor(cellH * 0.78)),
    };
  }

  private resizeBackingStore(width: number, height: number): void {
    const dpr = window.devicePixelRatio || 1;
    const nextW = Math.max(1, Math.floor(width * dpr));
    const nextH = Math.max(1, Math.floor(height * dpr));
    if (this.canvas.width !== nextW) this.canvas.width = nextW;
    if (this.canvas.height !== nextH) this.canvas.height = nextH;
  }
}

function rgb(value: number): string {
  const n = Math.max(0, Math.min(0xffffff, Math.trunc(value)));
  return `#${n.toString(16).padStart(6, "0")}`;
}

function rgba(color: { r: number; g: number; b: number }, alpha: number): string {
  return `rgba(${clampInt(color.r, 0, 255)}, ${clampInt(color.g, 0, 255)}, ${clampInt(color.b, 0, 255)}, ${Math.max(0, Math.min(1, alpha))})`;
}

function clampInt(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) return min;
  return Math.max(min, Math.min(max, Math.trunc(value)));
}

function nvimMouseModifier(event: MouseEvent): string {
  const parts: string[] = [];
  if (event.shiftKey) parts.push("S");
  if (event.altKey) parts.push("A");
  if (event.ctrlKey) parts.push("C");
  if (event.metaKey) parts.push("M");
  return parts.join("");
}

function wheelDeltaPixels(event: WheelEvent, lineHeight: number, pageHeight: number): number {
  if (event.deltaMode === WheelEvent.DOM_DELTA_LINE) {
    return event.deltaY * lineHeight;
  }
  if (event.deltaMode === WheelEvent.DOM_DELTA_PAGE) {
    return event.deltaY * pageHeight;
  }
  return event.deltaY;
}
