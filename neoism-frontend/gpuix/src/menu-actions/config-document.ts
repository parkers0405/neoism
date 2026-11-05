import { applyEdits, modify, parse } from "jsonc-parser";
import type { ConfigDocument } from "../../../../neoism-protocol/typescript/types";
import * as Effect from "effect/Effect";
import type { DaemonConnection } from "../daemon/client";
import {expectVariant,isRecord} from "../daemon/protocol";

/** Apply related settings in one revision-checked write, retaining comments. */
export function editConfigDocument(document: ConfigDocument, updates: readonly [string, unknown][]) {
  let content = document.content;
  for (const [path, value] of updates) content = applyEdits(content, modify(content, path.split("."), value, { formattingOptions: { insertSpaces: true, tabSize: 2, eol: "\n" } }));
  return { content, expected_revision: document.revision };
}
export function themeUpdates(document: ConfigDocument, name: string): [string, unknown][] {
  const appearance = parse(document.content)?.appearance || {}, updates: [string, unknown][] = [["appearance.theme", name]];
  if (typeof appearance["mashup-pack"] === "string" && appearance["mashup-pack"].trim()) updates.push(["appearance.mashup-baseline", {
    ...(appearance["mashup-baseline"] || { "font-family": appearance.fonts?.family || null }), theme: name,
  }]);
  return updates;
}
export async function saveTheme(client:DaemonConnection,name:string){
  const reply=expectVariant(await Effect.runPromise(client.request("Config","EnsureConfigDocument")),"ConfigDocument");
  if(!isRecord(reply)||!isRecord(reply.document)||typeof reply.document.content!=="string"||typeof reply.document.revision!=="string")throw new Error("Invalid local config document");
  const document=reply.document as unknown as ConfigDocument;
  if(!document.writable)throw new Error("The local config document is read-only");
  expectVariant(await Effect.runPromise(client.request("Config",{SaveConfigDocument:editConfigDocument(document,themeUpdates(document,name))})),"ConfigDocumentSaved");
}
