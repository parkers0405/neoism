import { subscribeGuiEvents } from "../sharedEvents";
import type { NeoismClient } from "@neoism/sdk";
import { eventForSession, record, refreshQueue, strings, text } from "./chatSupport";

export type QuestionItem = { question: string; multiple: boolean; custom: boolean; options: { label: string; description: string }[]; valid: boolean };
export type SafeQuestion = { id: string; sessionId: string; questions: QuestionItem[] };
export type SafePermission = { id: string; sessionId: string; permission: string; title: string; patterns: string[]; source: string };
export type InteractionSnapshot = { permissions: SafePermission[]; questions: SafeQuestion[]; busy: string | undefined };
export const emptyInteractions = (): InteractionSnapshot => ({ permissions: [], questions: [], busy: undefined });
function belongs(row: Record<string, unknown>, sessionId: string): boolean {
    return row.sessionId === sessionId || row.parentSessionID === sessionId;
}
export function decodePermissions(value: unknown, id: string): SafePermission[] {
    if (!Array.isArray(value)) return [];
    const rows = new Map<string, SafePermission>();
    for (const raw of value) {
        const p = record(raw);
        if (!text(p.id) || !text(p.sessionId) || !belongs(p, id)) continue;
        rows.set(text(p.id), { id: text(p.id), sessionId: text(p.sessionId), permission: text(p.permission) || "Tool access",
            title: text(p.title), patterns: strings(p.patterns), source: text(p.sourceTitle) || text(p.sourceAgent) });
    }
    return [...rows.values()];
}
export function decodeQuestionItem(value: unknown): QuestionItem {
    const q = record(value), options = new Map<string, { label: string; description: string }>();
    for (const raw of Array.isArray(q.options) ? q.options : []) {
        const option = record(raw), label = text(option.label).trim();
        if (label) options.set(label, { label, description: text(option.description) });
    }
    const question = text(q.question).trim() || text(q.header).trim() || text(q.text).trim();
    const custom = q.custom !== false;
    return { question: question || "Unrecognized question", multiple: q.multiple === true, custom,
        options: [...options.values()], valid: !!question && (custom || options.size > 0) };
}
export function decodeQuestions(value: unknown, id: string): SafeQuestion[] {
    if (!Array.isArray(value)) return [];
    const rows = new Map<string, SafeQuestion>();
    for (const raw of value) {
        const q = record(raw);
        if (!text(q.id) || !text(q.sessionId) || !belongs(q, id)) continue;
        const questions = Array.isArray(q.questions) ? q.questions.map(decodeQuestionItem) : [];
        rows.set(text(q.id), { id: text(q.id), sessionId: text(q.sessionId), questions });
    }
    return [...rows.values()];
}
export type AnswerDraft = { selected: string[]; custom: string };
export function questionAnswers(question: SafeQuestion, drafts: readonly AnswerDraft[]): string[][] | undefined {
    if (!question.questions.length) return;
    const answers: string[][] = [];
    for (let i = 0; i < question.questions.length; i++) {
        const item = question.questions[i], draft = drafts[i];
        if (!item.valid || !draft) return;
        const selected = [...new Set(draft.selected.filter(s => item.options.some(o => o.label === s)))];
        const custom = item.custom ? draft.custom.trim() : "";
        const values = custom ? item.multiple ? [...selected, custom] : [custom] : selected;
        if (!values.length || (!item.multiple && values.length !== 1)) return;
        answers.push([...new Set(values)]);
    }
    return answers;
}

/** All fetches/actions belong to one mount + client + session + skip-policy generation. */
export function createInteractionController(client: NeoismClient, id: string, skip: boolean,
    isCurrent: () => boolean, publish: (snapshot: InteractionSnapshot) => void, report: (error: unknown) => void) {
    const abort = new AbortController(), closed = new Set<string>(), attempted = new Set<string>();
    let snapshot = emptyInteractions();
    const current = () => !abort.signal.aborted && isCurrent();
    const emit = () => { if (current()) publish({ ...snapshot }); };
    const signal = abort.signal;
    const refresh = refreshQueue(async () => {
        const [p, q] = await Promise.all([
            client.operations.request("v2.interactions.permissions.list", { query: { sessionId: id }, signal }),
            client.operations.request("v2.interactions.questions.list", { query: { sessionId: id }, signal }),
        ]);
        return { permissions: decodePermissions(p, id), questions: decodeQuestions(q, id) };
    }, value => {
        snapshot = { ...value, busy: snapshot.busy,
            permissions: value.permissions.filter(p => !closed.has(p.id)), questions: value.questions.filter(q => !closed.has(q.id)) };
        emit();
        if (skip) void autoApprove();
    }, current, report);
    const remove = (requestId: string) => {
        closed.add(requestId);
        snapshot = { ...snapshot, permissions: snapshot.permissions.filter(p => p.id !== requestId), questions: snapshot.questions.filter(q => q.id !== requestId) };
        emit();
    };
    async function action(requestId: string, fn: () => Promise<boolean>) {
        if (!current() || snapshot.busy || closed.has(requestId)) return false;
        snapshot = { ...snapshot, busy: requestId }; emit();
        try {
            // Synchronous gate immediately before any mutation leaves this process.
            if (!current()) return false;
            const accepted = await fn();
            if (!current()) return false;
            if (!accepted) throw new Error("The server did not accept this response. Refresh the request and try again.");
            remove(requestId);
            return true;
        } catch (e) { if (current()) report(e); return false; }
        finally {
            if (current()) { snapshot = { ...snapshot, busy: undefined }; emit(); void refresh(); }
        }
    }
    function permission(requestId: string, reply: "once" | "always" | "reject") {
        if (!snapshot.permissions.some(p => p.id === requestId)) return Promise.resolve(false);
        return action(requestId, () => client.operations.request("v2.interactions.permissions.reply", {
            path: { request_id: requestId }, body: { reply }, signal,
        }));
    }
    function question(requestId: string, drafts: readonly AnswerDraft[]) {
        const q = snapshot.questions.find(q => q.id === requestId);
        const answers = q && questionAnswers(q, drafts);
        if (!answers) return Promise.resolve(false);
        return action(requestId, () => client.operations.request("v2.interactions.questions.reply", {
            path: { request_id: requestId }, body: { answers }, signal,
        }));
    }
    function reject(requestId: string) {
        if (!snapshot.questions.some(q => q.id === requestId)) return Promise.resolve(false);
        return action(requestId, () => client.operations.request("v2.interactions.questions.reject", { path: { request_id: requestId }, signal }));
    }
    async function autoApprove() {
        for (const p of snapshot.permissions) {
            // Do not automatically approve forwarded child requests: the skip toggle is scoped to this session.
            if (!current() || snapshot.busy) return;
            if (p.sessionId !== id || attempted.has(p.id)) continue;
            attempted.add(p.id);
            await permission(p.id, "once");
        }
    }
    async function events() {
        try {
            for await (const event of subscribeGuiEvents(client, { sessionId: id, tail: true, signal })) {
                if (!current()) break;
                const known = new Set([...snapshot.permissions, ...snapshot.questions].map(p => p.id));
                if (!eventForSession(event, id, known)) continue;
                if (event.type === "permission.replied" || event.type === "question.replied" || event.type === "question.rejected") remove(event.data.requestID);
                if (event.type.startsWith("permission.") || event.type.startsWith("question.")) void refresh();
            }
        } catch (e) { if (current()) report(e); }
    }
    return { refresh, permission, question, reject, events, dispose: () => abort.abort() };
}
export type InteractionController = ReturnType<typeof createInteractionController>;
