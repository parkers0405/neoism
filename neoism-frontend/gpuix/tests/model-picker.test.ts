import { expect, test } from "bun:test";
import { modelCatalogStatus, modelPickerOptions } from "../src/agent/model-picker";

test("account catalog filters current and recent model choices and refreshes their metadata", () => {
  const catalog = [
    { title: "OpenAI", value: "header:openai", header: true },
    { title: "Available Codex model", value: "openai/account-model", section: "OpenAI" },
    { title: "Other provider model", value: "anthropic/other-model" },
  ];
  const recent = [
    { title: "API only", value: "openai/api-model" },
    { title: "Stale name", value: "openai/account-model" },
  ];
  const rows = modelPickerOptions(catalog, recent, "openai/api-model");
  expect(rows.some(row => row.value === "openai/api-model")).toBe(false);
  expect(rows.filter(row => row.section === "Recent" && !row.header)).toEqual([
    { title: "Available Codex model", value: "openai/account-model", description: "OpenAI", section: "Recent", header: false },
  ]);
  expect(rows.some(row => row.value === "anthropic/other-model")).toBe(true);
  expect(modelPickerOptions([], recent, "openai/api-model")).toEqual([]);
  expect(modelPickerOptions(catalog, [], "openai/account-model")[0]).toMatchObject({ current: true, value: "openai/account-model" });
});

test("model discovery exposes loading and bounded retries without inventing choices", () => {
  expect(modelCatalogStatus(undefined)).toEqual({ loading: false });
  expect(modelCatalogStatus({ catalogStatus: { state: "ready" } })).toEqual({ loading: false });
  expect(modelCatalogStatus({ catalogStatus: { state: "loading", retryAfterMs: 0 } })).toMatchObject({ loading: true, retryAfterMs: 500 });
  expect(modelCatalogStatus({ catalogStatus: { state: "retryable-error", retryAfterMs: 60_000 } })).toMatchObject({ loading: false, retryAfterMs: 60_000, error: expect.any(String) });
});
