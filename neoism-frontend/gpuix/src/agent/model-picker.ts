import type { PickerOption } from "../ui/InlinePicker";
import { isRecord } from "../daemon/protocol";

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

export function modelCatalogStatus(catalog: unknown): { loading: boolean; error?: string; retryAfterMs?: number } {
  const status = isRecord(catalog) && isRecord(catalog.catalogStatus) ? catalog.catalogStatus : undefined;
  if (!status || !["loading", "retryable-error"].includes(String(status.state))) return { loading: false };
  return {
    loading: status.state === "loading",
    error: status.state === "retryable-error" ? "Model availability could not be refreshed. Retrying…" : undefined,
    retryAfterMs: Math.min(60_000, Math.max(500, typeof status.retryAfterMs === "number" ? status.retryAfterMs : 1000)),
  };
}
