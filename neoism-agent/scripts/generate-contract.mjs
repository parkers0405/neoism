#!/usr/bin/env node

const document = JSON.parse(await readStdin());
const schemas = document.components?.schemas ?? {};

const lines = [];

for (const name of Object.keys(schemas).sort()) {
  lines.push(`export type ${safeName(name)} = ${schemaType(schemas[name])};`);
}

lines.push("", "export interface ApiOperations {");
const operations = [];
for (const path of Object.keys(document.paths ?? {}).sort()) {
  const item = document.paths[path];
  for (const method of ["get", "post", "put", "patch", "delete"]) {
    const operation = item?.[method];
    if (!operation?.operationId) continue;
    operations.push([operation.operationId, method.toUpperCase(), path, operation]);
  }
}
for (const [id, method, path, operation] of operations.sort(([left], [right]) => left.localeCompare(right))) {
  const request = operation.requestBody?.content?.["application/json"]?.schema;
  const responses = operation.responses ?? {};
  const success = ["200", "201", "202", "204", "default"]
    .map((status) => responses[status])
    .find(Boolean);
  const response = success?.content?.["application/json"]?.schema;
  lines.push(
    `  ${quote(id)}: { method: ${quote(method)}; path: ${quote(path)}; request: ${request ? schemaType(request) : "void"}; response: ${response ? schemaType(response) : "void"}; };`,
  );
}
lines.push("}", "");

const body = lines.map((line) => line ? `  ${line}` : "").join("\n");
process.stdout.write(
  "// Generated from the canonical Neoism Agent OpenAPI document.\n" +
  "// Run neoism-agent/scripts/openapi.sh update; do not edit by hand.\n\n" +
  `export namespace Contract {\n${body}\n}`,
);

function schemaType(schema) {
  if (!schema || Object.keys(schema).length === 0) return "unknown";
  if (schema.$ref) return safeName(schema.$ref.split("/").at(-1));
  if (schema.const !== undefined) return literal(schema.const);
  if (schema.enum) return schema.enum.map(literal).join(" | ") || "never";
  if (schema.oneOf) return join(schema.oneOf, " | ");
  if (schema.anyOf) return join(schema.anyOf, " | ");
  if (schema.allOf) return join(schema.allOf, " & ");

  let type;
  if (schema.type === "array" || schema.items) {
    type = `Array<${schemaType(schema.items ?? {})}>`;
  } else if (schema.type === "object" || schema.properties || schema.additionalProperties) {
    const required = new Set(schema.required ?? []);
    const fields = Object.entries(schema.properties ?? {}).map(([name, value]) =>
      `${propertyName(name)}${required.has(name) ? "" : "?"}: ${schemaType(value)};`
    );
    if (schema.additionalProperties) {
      fields.push(`[key: string]: ${schema.additionalProperties === true ? "unknown" : schemaType(schema.additionalProperties)};`);
    }
    type = fields.length ? `{ ${fields.join(" ")} }` : "Record<string, unknown>";
  } else {
    type = ({ string: "string", integer: "number", number: "number", boolean: "boolean", null: "null" })[schema.type] ?? "unknown";
  }
  return schema.nullable ? `${type} | null` : type;
}

function join(items, separator) {
  return items.map((item) => `(${schemaType(item)})`).join(separator) || "never";
}

function literal(value) {
  return value === null ? "null" : JSON.stringify(value);
}

function safeName(value) {
  const name = String(value).replace(/[^A-Za-z0-9_$]/g, "_");
  return /^[A-Za-z_$]/.test(name) ? name : `_${name}`;
}

function propertyName(value) {
  return /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(value) ? value : quote(value);
}

function quote(value) {
  return JSON.stringify(value);
}

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}