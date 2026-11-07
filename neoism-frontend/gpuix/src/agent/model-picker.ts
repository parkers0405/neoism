import type { PickerOption } from "../ui/InlinePicker";
import { isRecord } from "../daemon/protocol";

export const ADD_PROVIDER_VALUE = "action:add-provider";

export function modelPickerRootOptions(options: readonly PickerOption[]): PickerOption[] {
  const seen = new Set<string>();
  const recent = options.filter(row => {
    if (row.header || !row.value.includes("/") || !(row.current || row.section === "Recent") || seen.has(row.value)) return false;
    seen.add(row.value);
    return true;
  }).slice(0, 3);
  return [
    ...(recent.length ? [{ title: "Recent models", value: "header:recent-models", header: true }, ...recent] : []),
    { title: "Providers", value: "header:providers", header: true },
    ...modelProviderOptions(options),
    { title: "Add Provider", value: ADD_PROVIDER_VALUE },
  ];
}

/** Derive providers only from models the current account can actually use. */
export function modelProviderOptions(options: readonly PickerOption[]): PickerOption[] {
  const names = new Map<string, string>();
  for (const row of options) {
    if (!row.header && row.value.includes("/") && row.section && !["Current", "Recent"].includes(row.section)) names.set(row.value.slice(0, row.value.indexOf("/")), row.section);
  }
  const providers = new Map<string, { title: string; models: Set<string>; current: boolean }>();
  for (const row of options) {
    if (row.header || !row.value.includes("/")) continue;
    const id = row.value.slice(0, row.value.indexOf("/"));
    if (!id) continue;
    const provider = providers.get(id) || { title: names.get(id) || id, models: new Set<string>(), current: false };
    provider.models.add(row.value);
    provider.current ||= !!row.current;
    providers.set(id, provider);
  }
  return [...options.filter(row => !row.header && row.value === ""), ...[...providers].map(([id, provider]) => ({
    title: provider.title, value: id, footer: `${provider.models.size} ${provider.models.size === 1 ? "model" : "models"}`, current: provider.current,
  }))];
}

export function providerModelOptions(options: readonly PickerOption[], provider: string): PickerOption[] {
  const seen = new Set<string>();
  return options.filter(row => {
    if (row.header || !row.value.startsWith(provider + "/") || seen.has(row.value)) return false;
    seen.add(row.value);
    return true;
  });
}

/** Saved choices are history, not evidence of access for the current account. */
export function modelPickerOptions(catalog: readonly PickerOption[], recent: readonly PickerOption[], selected?: string | null): PickerOption[] {
  const available = new Map(catalog.filter(row => !row.header).map(row => [row.value, row]));
  const current = selected ? available.get(selected) : undefined;
  const history = recent.flatMap(row => {
    const model = available.get(row.value);
    return model ? [{ ...model, header: false, description: model.description || model.section, section: "Recent" }] : [];
  });
  return [
    ...(!selected ? [{ title: "server default", description: "Use Neoism default", footer: "selected", value: "", section: "Current", current: true }]
      : current ? [{ ...current, header: false, description: current.description || current.section, footer: "selected", section: "Current", current: true }] : []),
    ...(history.length ? [{ title: "Recent", value: "header:recent", header: true, section: "Recent" }, ...history] : []),
    ...catalog,
  ];
}

export function modelCatalogStatus(catalog: unknown, transportRetryAfterMs?: number): { loading: boolean; error?: string; retryAfterMs?: number } {
  const status = isRecord(catalog) && isRecord(catalog.catalogStatus) ? catalog.catalogStatus : undefined;
  if(transportRetryAfterMs!==undefined)return {
    loading:false,
    error:"Model availability could not be refreshed. Retrying...",
    retryAfterMs:Math.max(transportRetryAfterMs,typeof status?.retryAfterMs==="number"?status.retryAfterMs:0),
  };
  if (!status || !["loading", "retryable-error"].includes(String(status.state))) return { loading: false };
  return {
    loading: status.state === "loading",
    error: status.state === "retryable-error" ? "Model availability could not be refreshed. Retrying…" : undefined,
    retryAfterMs: Math.min(60_000, Math.max(500, typeof status.retryAfterMs === "number" ? status.retryAfterMs : 1000)),
  };
}
