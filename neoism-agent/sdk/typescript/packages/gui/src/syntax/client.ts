import { languageName, MAX_SOURCE, type Span } from "./tokens";
let worker: Worker | undefined;
let nextId = 0;
const pending = new Map<number, (spans: Span[]) => void>();
function fail() {
    worker?.terminate(); worker = undefined;
    for (const finish of [...pending.values()]) finish([]);
}
function getWorker() {
    if (!worker) {
        worker = new Worker(new URL("./highlight.worker.ts", import.meta.url), { type: "module" });
        worker.onmessage = ({ data }: MessageEvent<{ id: number; spans: Span[] }>) => pending.get(data.id)?.(data.spans);
        worker.onerror = fail;
        worker.onmessageerror = fail;
    }
    return worker;
}
/** Cancellation detaches the subscriber; stale replies never update a remounted/streamed block.
 * A worker watchdog also bounds parse/query time without blocking the UI thread. */
export function highlight(source: string, fence: string, signal: AbortSignal): Promise<Span[]> {
    const language = languageName(fence);
    if (!language || source.length > MAX_SOURCE || signal.aborted || !source) return Promise.resolve([]);
    return new Promise(resolve => {
        const id = ++nextId;
        const timer = setTimeout(fail, 15_000);
        const finish = (spans: Span[]) => { clearTimeout(timer); pending.delete(id); signal.removeEventListener("abort", abort); resolve(spans); };
        const abort = () => finish([]);
        pending.set(id, finish); signal.addEventListener("abort", abort, { once: true });
        try { getWorker().postMessage({ id, source, language }); } catch { finish([]); }
    });
}
