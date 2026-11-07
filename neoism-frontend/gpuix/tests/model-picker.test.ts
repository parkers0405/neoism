import { expect, test } from "bun:test";
import { ADD_PROVIDER_VALUE, modelCatalogStatus, modelPickerOptions, modelPickerRootOptions, modelProviderOptions, providerModelOptions } from "../src/agent/model-picker";

test("model root offers a few unique recent choices before providers and Add Provider last", () => {
  const catalog = ["a", "b", "c", "d"].map(id => ({ title: id, value: `openai/${id}`, section: "OpenAI" }));
  const options = modelPickerOptions(catalog, [catalog[0], catalog[1], catalog[1], catalog[2], catalog[3], { title: "Gone", value: "missing/model" }], "openai/a");
  const root = modelPickerRootOptions(options);
  expect(root.slice(0, 4).map(row => row.value)).toEqual(["header:recent-models", "openai/a", "openai/b", "openai/c"]);
  expect(root.slice(4).map(row => row.value)).toEqual(["header:providers", "openai", ADD_PROVIDER_VALUE]);
  expect(root[1].current).toBe(true);
  expect(modelPickerRootOptions([]).at(-1)?.value).toBe(ADD_PROVIDER_VALUE);
});

test("provider navigation counts unique available models and scopes the detail screen", () => {
  const catalog = [
    { title: "OpenAI", value: "", header: true },
    { title: "Model A", value: "openai/a", section: "OpenAI" },
    { title: "Model B", value: "openai/b", section: "OpenAI" },
    { title: "Other", value: "anthropic/c" },
  ];
  const rows = modelPickerOptions(catalog, [{ title: "Stale", value: "openai/missing" }], "openai/a");
  expect(modelProviderOptions(rows)).toEqual([
    { title: "OpenAI", value: "openai", footer: "2 models", current: true },
    { title: "anthropic", value: "anthropic", footer: "1 model", current: false },
  ]);
  expect(providerModelOptions(rows, "openai").map(row => row.value)).toEqual(["openai/a", "openai/b"]);
  expect(providerModelOptions(rows, "missing")).toEqual([]);
  expect(modelProviderOptions(modelPickerOptions([], [], null))[0].value).toBe("");
});

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

test("transport failures retry without server status and respect an existing server backoff", () => {
  expect(modelCatalogStatus(undefined,2000)).toMatchObject({loading:false,retryAfterMs:2000});
  expect(modelCatalogStatus({catalogStatus:{state:"ready"}},4000)).toMatchObject({loading:false,retryAfterMs:4000});
  expect(modelCatalogStatus({catalogStatus:{state:"retryable-error",retryAfterMs:15000}},2000)).toMatchObject({retryAfterMs:15000});
  expect(modelCatalogStatus({catalogStatus:{state:"ready"}})).toEqual({loading:false});
});
