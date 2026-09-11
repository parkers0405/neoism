import { useRef, type KeyboardEvent, type TouchEvent, type UIEvent, type WheelEvent } from "react";

function nestedScrollCanConsume(target: EventTarget | null, viewport: HTMLElement, delta: number): boolean {
    for (let node = target instanceof Element ? target : null; node && node !== viewport; node = node.parentElement) {
        if (!(node instanceof HTMLElement) || node.scrollHeight <= node.clientHeight + 1) continue;
        if (!/auto|scroll/.test(getComputedStyle(node).overflowY)) continue;
        if (delta < 0 ? node.scrollTop > 0 : node.scrollTop + node.clientHeight < node.scrollHeight - 1) return true;
    }
    return false;
}

/** Only user-directed upward scrolling starts a page; layout/anchor scrolls never cascade. */
export function useHistoryPagination({ sessionId, cursor, loading, loadOlder }: {
    sessionId: string;
    cursor?: string;
    loading: boolean;
    loadOlder(): Promise<void> | void;
}) {
    const state = useRef({ sessionId, cursor, intent: false, requested: "", pending: "", touchY: undefined as number | undefined });
    if (state.current.sessionId !== sessionId) state.current = {sessionId, cursor, intent:false, requested:"", pending:"", touchY:undefined};
    if (state.current.cursor !== cursor) {
        state.current.cursor = cursor;
        state.current.intent = false;
    }
    const attempt = (element: HTMLElement) => {
        const current = state.current;
        if (!cursor || loading || current.pending || !current.intent || element.scrollTop > 80) return;
        const key = `${sessionId}:${cursor}`;
        if (current.requested === key) return;
        current.intent = false;
        current.requested = key;
        current.pending = key;
        void Promise.resolve().then(loadOlder).catch(() => {
            // The history owner reports errors; the visible Load earlier button remains a retry.
        }).finally(() => { if (state.current.pending === key) state.current.pending = ""; });
    };
    return {
        onScroll: (event: UIEvent<HTMLDivElement>) => attempt(event.currentTarget),
        onWheel: (event: WheelEvent<HTMLDivElement>) => {
            if (event.ctrlKey || !event.deltaY || nestedScrollCanConsume(event.target, event.currentTarget, event.deltaY)) {
                state.current.intent = false;
                return;
            }
            state.current.intent = event.deltaY < 0;
            if (event.deltaY < 0) attempt(event.currentTarget);
        },
        onTouchStart: (event: TouchEvent<HTMLDivElement>) => {
            state.current.touchY = event.touches.length === 1 ? event.touches[0].clientY : undefined;
            state.current.intent = false;
        },
        onTouchMove: (event: TouchEvent<HTMLDivElement>) => {
            const y = event.touches.length === 1 ? event.touches[0].clientY : undefined;
            if (y !== undefined && state.current.touchY !== undefined) {
                const delta = state.current.touchY - y;
                state.current.intent = delta < 0 && !nestedScrollCanConsume(event.target, event.currentTarget, delta);
            }
            state.current.touchY = y;
            attempt(event.currentTarget);
        },
        onKeyDown: (event: KeyboardEvent<HTMLDivElement>) => {
            const target = event.target;
            if (target instanceof Element && target.closest("input,textarea,select,[contenteditable=true]")) return;
            if (["PageUp", "Home", "ArrowUp"].includes(event.key)) {
                if (nestedScrollCanConsume(event.target, event.currentTarget, -1)) { state.current.intent = false; return; }
                state.current.intent = true;
                attempt(event.currentTarget);
            }
        },
    };
}
