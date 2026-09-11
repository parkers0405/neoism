import type { Choice } from './components/Composer';
export const OPTION_HEIGHT = 48;
export const HEADER_HEIGHT = 24;
export const PICKER_HEIGHT = 336;
export function indexChoices(choices: Choice[]) {
    return choices.map(choice => ({ choice, text: `${choice.label} ${choice.id} ${choice.description || ''}`.toLowerCase() }));
}
/** Native multiword AND matching, preserving Current/Recent/provider ordering. */
export function filterChoices(index: ReturnType<typeof indexChoices>, query: string): Choice[] {
    const words = query.toLowerCase().trim().split(/\s+/).filter(Boolean);
    return index.filter(row => words.every(word => row.text.includes(word))).map(row => row.choice);
}
export interface PickerRow { top: number; height: number; section?: string; choice?: Choice; index: number }
export function layoutChoices(choices: Choice[]) {
    const rows: PickerRow[] = [], options: PickerRow[] = [];
    let height = 0;
    choices.forEach((choice, index) => {
        if (choice.section && (index === 0 || choice.section !== choices[index - 1].section)) {
            rows.push({ top: height, height: HEADER_HEIGHT, section: choice.section, index: -1 }); height += HEADER_HEIGHT;
        }
        const row = { top: height, height: OPTION_HEIGHT, choice, index };
        rows.push(row); options.push(row); height += OPTION_HEIGHT;
    });
    return { rows, options, height };
}
/** Binary search only the visible interval; no overscan/full-catalog DOM. */
export function visibleChoices(layout: ReturnType<typeof layoutChoices>, scrollTop: number, viewport = PICKER_HEIGHT) {
    const top = Math.max(0, Math.min(scrollTop, Math.max(0, layout.height - viewport)));
    let lo = 0, hi = layout.rows.length;
    while (lo < hi) { const mid = (lo + hi) >>> 1, row = layout.rows[mid]; if (row.top + row.height <= top) lo = mid + 1; else hi = mid; }
    let end = lo;
    while (end < layout.rows.length && layout.rows[end].top < top + viewport) end++;
    return layout.rows.slice(lo, end);
}
export function revealChoice(layout: ReturnType<typeof layoutChoices>, index: number, top: number, viewport = PICKER_HEIGHT) {
    const row = layout.options[index];
    if (!row) return 0;
    const next = row.top < top ? row.top : row.top + row.height > top + viewport ? row.top + row.height - viewport : top;
    return Math.max(0, Math.min(next, Math.max(0, layout.height - viewport)));
}
