import type { CrdtPresenceColor } from "../workspace/types";

// Deterministic per-peer presence colors, shared by the presence
// publisher (self color) and the markdown presence overlay (peer
// carets). Formerly lived in the deleted embedded-nvim editor path.

const PRESENCE_PALETTE: CrdtPresenceColor[] = [
  { r: 0x2f, g: 0x80, b: 0xed },
  { r: 0x27, g: 0xae, b: 0x60 },
  { r: 0xeb, g: 0x57, b: 0x57 },
  { r: 0xf2, g: 0xc9, b: 0x4c },
  { r: 0xbb, g: 0x6b, b: 0xd9 },
  { r: 0x56, g: 0xcc, b: 0xf2 },
  { r: 0xf2, g: 0x99, b: 0x4a },
  { r: 0x21, g: 0x92, b: 0x6b },
  { r: 0x9b, g: 0x51, b: 0xe0 },
  { r: 0x00, g: 0xac, b: 0xd7 },
  { r: 0xd6, g: 0x5d, b: 0x0e },
  { r: 0x6f, g: 0x7d, b: 0xff },
];

export function stablePresenceColor(peerId: string): CrdtPresenceColor {
  let hash = 0xcbf29ce484222325n;
  for (const byte of new TextEncoder().encode(peerId)) {
    hash ^= BigInt(byte);
    hash = BigInt.asUintN(64, hash * 0x1000000001b3n);
  }
  return PRESENCE_PALETTE[Number(hash % BigInt(PRESENCE_PALETTE.length))];
}

export function colorCss(color: CrdtPresenceColor): string {
  return `rgb(${clampByte(color.r)}, ${clampByte(color.g)}, ${clampByte(color.b)})`;
}

function clampByte(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(255, Math.trunc(value)));
}
