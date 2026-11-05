import type { DatabaseValue, DatabaseObject } from "./protocol";

export type { DatabaseValue, DatabaseObject } from "./protocol";
export interface DatabaseConnection {
  id: string;
  title: string;
  path: string;
  rootDir: string;
  workspaceId: string;
}
export interface GridRow {
  key: string;
  values: DatabaseValue[];
  identity: Record<string, DatabaseValue> | null;
  original: Record<string, DatabaseValue>;
  kind: "existing" | "insert" | "update" | "delete";
}
export interface DatabaseState {
  objects: DatabaseObject[];
  tabs: string[];
  table?: string;
  columns: string[];
  rows: GridRow[];
  total: number;
  offset: number;
  limit: number;
  editable: boolean;
  loading: boolean;
  applying: boolean;
  error?: string;
  notice?: string;
  sort?: { column: string; descending: boolean };
  filters: Record<string, DatabaseValue[]>;
  selected?: { row: string; column: number };
  pending: number;
}

export function valueText(value: DatabaseValue): string {
  switch (value.kind) {
    case "null": return "NULL";
    case "blob": return `[BLOB ${Math.floor(value.value.length * 3 / 4)} bytes]`;
    default: return String(value.value);
  }
}

export function sameValue(a: DatabaseValue, b: DatabaseValue): boolean {
  return a.kind === b.kind && (a.kind === "null" || b.kind !== "null" && a.value === b.value);
}

export function parseCell(text: string, kind: DatabaseValue["kind"]): DatabaseValue {
  switch (kind) {
    case "null": return { kind: "null" };
    case "integer": {
      if (!/^[+-]?\d+$/.test(text.trim())) throw new Error("Enter a whole number");
      const value = BigInt(text.trim());
      if (value < -9223372036854775808n || value > 9223372036854775807n) throw new Error("Integer is outside SQLite's 64-bit range");
      return { kind: "integer", value: value.toString() };
    }
    case "real": {
      const value = Number(text);
      if (!text.trim() || !Number.isFinite(value)) throw new Error("Enter a finite number");
      return { kind: "real", value };
    }
    case "text": return { kind: "text", value: text };
    case "blob": throw new Error("Binary cells are read-only in the grid");
  }
}