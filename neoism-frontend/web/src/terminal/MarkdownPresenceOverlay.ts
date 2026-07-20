// Wave 7F — remote collaborator carets over the web markdown layer.
//
// The web renders markdown buffers as a DOM article
// (`renderMarkdownDocument`), so remote carets are DOM elements
// absolutely positioned inside the scrolling `.web-markdown-layer`.
// Each rendered block carries its source-line range
// (`data-md-line` / `data-md-line-end`); presence cursors arrive in
// source coordinates (zero-based line, UTF-16 column) on the CRDT
// wire, and `caretLayoutForLine` interpolates a caret y within the
// matching block. Column positioning is intentionally approximate
// (carets sit at the block's left edge): the read-only web viewer has
// no per-character layout, and the name tag — the social cue — is
// what matters.

import type { CrdtPeerPresence } from "../workspace/types";
import { colorCss } from "../presence/presenceColor";

export interface MarkdownPresenceAnchor {
  /** 0-based first source line covered by the block. */
  line: number;
  /** 0-based last source line covered by the block (inclusive). */
  endLine: number;
  /** Block top in the markdown layer's content coordinates (px). */
  top: number;
  /** Block height in px. */
  height: number;
  /** Block left edge in px. */
  left: number;
}

export interface MarkdownCaretLayout {
  top: number;
  height: number;
  left: number;
}

/**
 * Pure mapping: source line → caret geometry, interpolating per-line
 * inside the matching block's box. Lines that fall between blocks
 * (blank separators) snap to the nearest following block; lines past
 * the last block return `null` (no caret this frame — mirrors the
 * desktop renderer skipping carets on non-visible lines).
 */
export function caretLayoutForLine(
  anchors: MarkdownPresenceAnchor[],
  line: number,
): MarkdownCaretLayout | null {
  let following: MarkdownPresenceAnchor | null = null;
  for (const anchor of anchors) {
    if (line >= anchor.line && line <= anchor.endLine) {
      const lineCount = Math.max(1, anchor.endLine - anchor.line + 1);
      const lineHeight = anchor.height / lineCount;
      return {
        top: anchor.top + (line - anchor.line) * lineHeight,
        height: lineHeight,
        left: anchor.left,
      };
    }
    if (
      anchor.line > line &&
      (following === null || anchor.line < following.line)
    ) {
      following = anchor;
    }
  }
  if (following) {
    const lineCount = Math.max(1, following.endLine - following.line + 1);
    return {
      top: following.top,
      height: following.height / lineCount,
      left: following.left,
    };
  }
  return null;
}

export class MarkdownPresenceOverlay {
  private readonly host: HTMLDivElement;

  constructor(private readonly layer: HTMLElement) {
    this.host = document.createElement("div");
    this.host.className = "web-markdown-presence-layer";
  }

  /**
   * Rebuild the caret elements for the given remote peers.
   * `renderMarkdownLayer` wipes the layer's children on every content
   * push, so `sync` re-attaches the overlay host as needed.
   */
  sync(peers: CrdtPeerPresence[]): void {
    if (peers.length === 0) {
      this.clear();
      return;
    }
    if (!this.host.isConnected) {
      this.layer.appendChild(this.host);
    }
    const anchors = this.collectAnchors();
    const carets: HTMLElement[] = [];
    for (const peer of peers) {
      const layout = caretLayoutForLine(anchors, peer.cursor.line);
      if (!layout) continue;
      carets.push(this.caretElement(peer, layout));
    }
    this.host.replaceChildren(...carets);
  }

  clear(): void {
    this.host.replaceChildren();
    this.host.remove();
  }

  private collectAnchors(): MarkdownPresenceAnchor[] {
    const anchors: MarkdownPresenceAnchor[] = [];
    const blocks = this.layer.querySelectorAll<HTMLElement>("[data-md-line]");
    for (const el of blocks) {
      const line = Number(el.dataset.mdLine);
      const endLine = Number(el.dataset.mdLineEnd ?? el.dataset.mdLine);
      if (!Number.isFinite(line)) continue;
      anchors.push({
        line,
        endLine: Number.isFinite(endLine) ? endLine : line,
        top: el.offsetTop,
        height: Math.max(1, el.offsetHeight),
        left: el.offsetLeft,
      });
    }
    return anchors;
  }

  private caretElement(
    peer: CrdtPeerPresence,
    layout: MarkdownCaretLayout,
  ): HTMLElement {
    const caret = document.createElement("div");
    caret.className = "web-markdown-presence-caret";
    const color = colorCss(peer.color);
    caret.style.top = `${layout.top}px`;
    caret.style.left = `${Math.max(0, layout.left - 6)}px`;
    caret.style.height = `${Math.max(10, Math.min(layout.height, 28))}px`;
    caret.style.background = color;

    const name = (peer.display_name || peer.peer_id).trim().slice(0, 32);
    if (name.length > 0) {
      const tag = document.createElement("span");
      tag.className = "web-markdown-presence-tag";
      tag.style.background = color;
      tag.textContent = name;
      caret.appendChild(tag);
    }
    return caret;
  }
}
