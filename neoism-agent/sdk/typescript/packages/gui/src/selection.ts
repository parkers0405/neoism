import type { ConfigDefaults, ProviderListResult, Session } from '@neoism/sdk';
export interface ChatSelection { model: string; agent: string; thinking: string; connectionId: string }
export function configuredModel(catalog?: ProviderListResult): string {
    if (!catalog?.default) return '';
    for (const fallback of [false, true]) for (const p of catalog.all) {
        if ((p.id === 'opencode') !== fallback || (catalog.connected.length > 0 && !catalog.connected.includes(p.id))) continue;
        const model = catalog.default[p.id];
        if (model?.trim()) return `${p.id}/${model}`;
    }
    return '';
}
export function resolveSelection(explicit: Partial<ChatSelection>, session?: Session, defaults?: ConfigDefaults, catalog?: ProviderListResult): ChatSelection {
    return {
        model: explicit.model ?? (session?.model ? `${session.model.providerId}/${session.model.id}` : defaults?.model || configuredModel(catalog)),
        agent: explicit.agent ?? (session?.agent || defaults?.defaultAgent || 'build'),
        thinking: explicit.thinking ?? (session?.model ? session.model.variant ?? '' : defaults?.variant ?? ''),
        connectionId: explicit.connectionId ?? session?.model?.connectionId ?? '',
    };
}
