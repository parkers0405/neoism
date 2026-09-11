import type { Session } from '@neoism/sdk';
import type { ChatSelection } from './selection';
export interface ChatTab { key: string; sessionId?: string; directory?: string; draft: string; explicit: Partial<ChatSelection>; metadata?: Session; files?: File[] }
export interface TabState { tabs: ChatTab[]; active: string }
export function localTab(): ChatTab { return { key: `local-${Date.now()}-${Math.random().toString(36).slice(2)}`, draft: '', explicit: {} }; }
export function loadTabs(server: string): TabState {
    try {
        const value = JSON.parse(localStorage.getItem('neoism.gui.tabs:' + server) || 'null');
        if (value?.tabs?.length && value.tabs.every((t: ChatTab) => typeof t.key === 'string' && typeof t.draft === 'string' && t.explicit && typeof t.explicit === 'object'))
            return { tabs: value.tabs, active: value.tabs.some((t: ChatTab) => t.key === value.active) ? value.active : value.tabs[0].key };
    } catch { /* use a visit-local tab */ }
    const tab = localTab(); return { tabs: [tab], active: tab.key };
}
export function saveTabs(server: string, state: TabState) { try { localStorage.setItem('neoism.gui.tabs:' + server, JSON.stringify({ ...state, tabs: state.tabs.map(({ files: _files, ...tab }) => tab) })); } catch { /* visit-local */ } }
export function closeTab(state: TabState, key: string): TabState {
    const index = state.tabs.findIndex(t => t.key === key);
    if (index < 0) return state;
    const tabs = state.tabs.filter(t => t.key !== key);
    if (!tabs.length) tabs.push(localTab());
    return { tabs, active: state.active === key ? tabs[Math.min(index, tabs.length - 1)].key : state.active };
}
