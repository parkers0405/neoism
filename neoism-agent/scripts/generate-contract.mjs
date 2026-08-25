#!/usr/bin/env node

const document = JSON.parse(await readStdin());
const schemas = document.components?.schemas ?? {};
const methods = ["get", "post", "put", "patch", "delete"];
const operations = [];

for (const path of Object.keys(document.paths ?? {}).sort()) {
  const item = document.paths[path];
  for (const method of methods) {
    const operation = item?.[method];
    if (!operation?.operationId) continue;
    const parameters = [...(item.parameters ?? []), ...(operation.parameters ?? [])]
      .map(resolveParameter);
    operations.push({
      id: operation.operationId,
      method: method.toUpperCase(),
      path,
      operation,
      parameters,
    });
  }
}
operations.sort((left, right) => left.id.localeCompare(right.id));

const lines = [
  'import type { NeoismTransport, RequestDescriptor } from "../transport.js";',
  "",
];
for (const name of Object.keys(schemas).sort()) {
  lines.push(`export type ${safeName(name)} = ${schemaType(schemas[name])};`);
}

lines.push("", "export interface ApiOperations {");
for (const entry of operations) {
  const input = operationInputType(entry);
  const responses = successResponses(entry.operation.responses ?? {});
  const responseMap = responses.map(({ status, type }) => `${quote(status)}: ${type};`).join(" ");
  const response = unique(responses.map(({ type }) => type)).join(" | ") || "void";
  lines.push(
    `  ${quote(entry.id)}: { method: ${quote(entry.method)}; path: ${quote(entry.path)}; input: ${input}; responses: { ${responseMap} }; response: ${response}; };`,
  );
}
lines.push("}", "");
lines.push(
  "export type OperationId = keyof ApiOperations;",
  "export type OperationInput<Id extends OperationId> = ApiOperations[Id][\"input\"];",
  "export type OperationResponse<Id extends OperationId> = ApiOperations[Id][\"response\"];",
  "export type OperationResponses<Id extends OperationId> = ApiOperations[Id][\"responses\"];",
  "",
  "export interface OperationDescriptor {",
  '  readonly method: "GET" | "POST" | "PUT" | "PATCH" | "DELETE";',
  "  readonly path: string;",
  '  readonly transport: "http" | "sse" | "websocket";',
  "  readonly requestMediaType?: string;",
  '  readonly response?: "json" | "bytes" | "text";',
  "  readonly responses: Readonly<Record<string, readonly string[]>>;",
  "}",
  "",
  "export const operationDescriptors = {",
);
for (const entry of operations) {
  lines.push(`  ${quote(entry.id)}: ${JSON.stringify(operationDescriptor(entry))},`);
}
lines.push("} as const satisfies Record<OperationId, OperationDescriptor>;", "");
lines.push(
  "export function buildOperationRequest<Id extends OperationId>(",
  "  id: Id,",
  "  input: OperationInput<Id>,",
  "): RequestDescriptor {",
  "  const descriptor = operationDescriptors[id] as OperationDescriptor;",
  "  const value = (input ?? {}) as { path?: Record<string, unknown>; query?: Record<string, unknown>; headers?: Record<string, unknown>; body?: unknown; signal?: AbortSignal };",
  "  let path = descriptor.path;",
  "  for (const [name, part] of Object.entries(value.path ?? {})) {",
  "    path = path.replace(`{${name}}`, encodeURIComponent(String(part)));",
  "  }",
  "  if (/\\{[^}]+\\}/.test(path)) throw new TypeError(`missing path parameter for ${id}`);",
  "  const headers = Object.fromEntries(Object.entries(value.headers ?? {}).filter(([, item]) => item !== undefined).map(([name, item]) => [name, String(item)]));",
  "  if (descriptor.requestMediaType && value.body !== undefined) headers[\"content-type\"] ??= descriptor.requestMediaType;",
  "  return {",
  "    method: descriptor.method,",
  "    path,",
  "    ...(value.query ? { query: value.query as NonNullable<RequestDescriptor[\"query\"]> } : {}),",
  "    ...(Object.keys(headers).length ? { headers } : {}),",
  "    ...(value.body !== undefined ? { body: value.body } : {}),",
  "    ...(descriptor.response ? { response: descriptor.response } : {}),",
  "    ...(value.signal ? { signal: value.signal } : {}),",
  "  };",
  "}",
  "",
  "export interface ContractClient {",
  "  request<Id extends OperationId>(id: Id, input: OperationInput<Id>): Promise<OperationResponse<Id>>;",
  "  descriptor<Id extends OperationId>(id: Id): (typeof operationDescriptors)[Id];",
  "}",
  "",
  "export function createContractClient(transport: NeoismTransport): ContractClient {",
  "  return {",
  "    request: (id, input) => transport.request(buildOperationRequest(id, input)),",
  "    descriptor: (id) => operationDescriptors[id],",
  "  };",
  "}",
);

process.stdout.write(
  "// Generated from the authoritative canonical Neoism Agent OpenAPI document.\n" +
  "// Run neoism-agent/scripts/openapi.sh update. Do not edit by hand.\n\n" +
  `${lines.join("\n")}\n`,
);

function resolveParameter(parameter) {
  if (!parameter?.$ref) return parameter;
  const name = parameter.$ref.split("/").at(-1);
  return document.components?.parameters?.[name] ?? parameter;
}

function operationInputType(entry) {
  const groups = { path: [], query: [], header: [] };
  for (const parameter of entry.parameters) {
    if (!groups[parameter.in]) continue;
    groups[parameter.in].push(parameter);
  }
  const fields = [];
  for (const [location, parameters] of Object.entries(groups)) {
    if (!parameters.length) continue;
    const properties = parameters.map((parameter) =>
      `${propertyName(parameter.name)}${parameter.required ? "" : "?"}: ${schemaType(parameter.schema ?? {})};`
    ).join(" ");
    const required = parameters.some((parameter) => parameter.required);
    fields.push(`${location === "header" ? "headers" : location}${required ? "" : "?"}: { ${properties} };`);
  }
  const bodyEntries = Object.entries(entry.operation.requestBody?.content ?? {});
  if (bodyEntries.length) {
    const bodyTypes = unique(bodyEntries.map(([, media]) => requestBodyType(media.schema ?? {}, media)));
    fields.push(`body${entry.operation.requestBody.required ? "" : "?"}: ${bodyTypes.join(" | ")};`);
  }
  fields.push("signal?: AbortSignal;");
  return `{ ${fields.join(" ")} }`;
}

function requestBodyType(schema, media) {
  if (schema?.format === "binary") return "Uint8Array | Blob";
  return schemaType(schema ?? media?.schema ?? {});
}

function successResponses(responses) {
  const result = [];
  for (const [status, responseRef] of Object.entries(responses)) {
    if (!/^2\d\d$/.test(status)) continue;
    const response = resolveResponse(responseRef);
    const content = Object.entries(response?.content ?? {});
    const types = content.length
      ? content.map(([mediaType, media]) => responseBodyType(mediaType, media?.schema ?? {}))
      : ["void"];
    result.push({ status, type: unique(types).join(" | ") });
  }
  return result;
}

function resolveResponse(response) {
  if (!response?.$ref) return response;
  const name = response.$ref.split("/").at(-1);
  return document.components?.responses?.[name] ?? response;
}

function responseBodyType(mediaType, schema) {
  if (schema?.format === "binary" || mediaType === "application/octet-stream") return "Uint8Array";
  if (mediaType.startsWith("text/") || mediaType === "application/x-ndjson") return "string";
  return schemaType(schema);
}

function operationDescriptor(entry) {
  const requestMediaType = Object.keys(entry.operation.requestBody?.content ?? {})[0];
  const responses = {};
  let response;
  let transport = "http";
  for (const [status, responseRef] of Object.entries(entry.operation.responses ?? {})) {
    if (!/^2\d\d$/.test(status)) continue;
    const mediaTypes = Object.keys(resolveResponse(responseRef)?.content ?? {});
    responses[status] = mediaTypes;
    for (const mediaType of mediaTypes) {
      if (mediaType === "text/event-stream") transport = "sse";
      if (mediaType === "application/octet-stream") response ??= "bytes";
      else if (mediaType.startsWith("text/") || mediaType === "application/x-ndjson") response ??= "text";
      else response ??= "json";
    }
  }
  if (entry.operation["x-neoism-transport"] === "websocket") transport = "websocket";
  return {
    method: entry.method,
    path: entry.path,
    transport,
    ...(requestMediaType ? { requestMediaType } : {}),
    ...(transport === "http" && response ? { response } : {}),
    responses,
  };
}

function schemaType(schema) {
  if (!schema || Object.keys(schema).length === 0) return "unknown";
  if (schema.$ref) return safeName(schema.$ref.split("/").at(-1));
  if (schema.const !== undefined) return literal(schema.const);
  if (schema.enum) return schema.enum.map(literal).join(" | ") || "never";

  const composition = [];
  if (schema.oneOf) composition.push(join(schema.oneOf, " | "));
  if (schema.anyOf) composition.push(join(schema.anyOf, " | "));
  if (schema.allOf) composition.push(join(schema.allOf, " & "));
  const sibling = { ...schema };
  delete sibling.oneOf;
  delete sibling.anyOf;
  delete sibling.allOf;
  const hasSibling = Object.keys(sibling).some((key) => !["nullable", "description", "title", "default", "example", "examples", "deprecated", "readOnly", "writeOnly"].includes(key));
  let type = hasSibling ? simpleSchemaType(sibling) : undefined;
  if (composition.length) type = type ? `${type} & (${composition.join(") & (")})` : composition.join(" & ");
  type ??= "unknown";
  return schema.nullable && type !== "null" ? `${type} | null` : type;
}

function simpleSchemaType(schema) {
  if (Array.isArray(schema.type)) {
    return unique(schema.type.map((type) => simpleSchemaType({ ...schema, type }))).join(" | ");
  }
  if (schema.type === "array" || schema.items) return `Array<${schemaType(schema.items ?? {})}>`;
  if (schema.type === "object" || schema.properties || schema.additionalProperties !== undefined) {
    const required = new Set(schema.required ?? []);
    const fields = Object.entries(schema.properties ?? {}).map(([name, value]) =>
      `${propertyName(name)}${required.has(name) ? "" : "?"}: ${schemaType(value)};`
    );
    if (schema.additionalProperties) fields.push(`[key: string]: ${schema.additionalProperties === true ? "unknown" : schemaType(schema.additionalProperties)};`);
    return fields.length ? `{ ${fields.join(" ")} }` : "Record<string, unknown>";
  }
  return ({ string: "string", integer: "number", number: "number", boolean: "boolean", null: "null" })[schema.type] ?? "unknown";
}

function join(items, separator) {
  return items.map((item) => `(${schemaType(item)})`).join(separator) || "never";
}

function unique(items) { return [...new Set(items)]; }
function literal(value) { return value === null ? "null" : JSON.stringify(value); }
function safeName(value) {
  const name = String(value).replace(/[^A-Za-z0-9_$]/g, "_");
  return /^[A-Za-z_$]/.test(name) ? name : `_${name}`;
}
function propertyName(value) { return /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(value) ? value : quote(value); }
function quote(value) { return JSON.stringify(value); }
async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}