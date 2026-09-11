import type { Choice } from './components/Composer';
import { serverScope } from './types';
const key = (server: string) => 'neoism.gui.recent-models:' + serverScope(server);
export function loadRecentModels(server: string): string[] {
    try {
        const value: unknown = JSON.parse(localStorage.getItem(key(server)) || '[]');
        return Array.isArray(value) ? [...new Set(value.filter((id): id is string => typeof id === 'string' && id.indexOf('/') > 0))].slice(0, 8) : [];
    } catch { return []; }
}
export function rememberModel(recent: string[], model: string): string[] {
    return model.indexOf('/') > 0 ? [model, ...recent.filter(id => id !== model)].slice(0, 8) : recent;
}
export function saveRecentModels(server: string, recent: string[]) {
    // Only provider/model references: no credentials or stale account IDs.
    try { localStorage.setItem(key(server), JSON.stringify(recent)); } catch { /* visit-local */ }
}
export function groupedModelChoices(models: Choice[], recent: string[], current: string): Choice[] {
    const byId = new Map(models.map(m => [m.id, m]));
    const choice = (id: string): Choice => byId.get(id) || { id, label: id.split('/').slice(1).join('/') || id, description: id.split('/')[0], badge: 'Unavailable' };
    return [current ? { ...choice(current), section: 'Current', badge: byId.has(current) ? 'Selected' : 'Selected · unavailable' }
        : { id: '', label: 'Server default', description: 'Use Neoism default', badge: 'Selected', section: 'Current' },
        ...recent.map(id => ({ ...choice(id), section: 'Recent' })), ...models];
}
