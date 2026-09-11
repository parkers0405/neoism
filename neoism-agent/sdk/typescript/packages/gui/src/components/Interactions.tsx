import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { NeoismClient } from "@neoism/sdk";
import { errorMessage } from "../types";
import { interactionShortcut } from "./chatSupport";
import { createInteractionController, emptyInteractions, questionAnswers,
    type AnswerDraft, type InteractionController, type InteractionSnapshot, type SafeQuestion } from "./interactionController";
import "./chat-details.css";

function shortcut(e: KeyboardEvent, kind: "permission" | "question") {
    const target = e.target as HTMLElement;
    return interactionShortcut({ key: e.key, kind, composing: e.nativeEvent.isComposing, repeat: e.repeat,
        modified: e.altKey || e.ctrlKey || e.metaKey || e.shiftKey,
        editable: !!target.closest('textarea, select, input:not([type="radio"]):not([type="checkbox"]), [contenteditable="true"], [role="textbox"]'),
        button: !!target.closest('button, a, input[type="submit"]'),
    });
}
export function QuestionCard({ question: q, busy, submit, reject }: {
    question: SafeQuestion; busy: boolean; submit(drafts: AnswerDraft[]): void; reject(): void;
}) {
    const [drafts, setDrafts] = useState<AnswerDraft[]>(() => q.questions.map(() => ({ selected: [], custom: "" })));
    const valid = !!questionAnswers(q, drafts);
    const change = (index: number, update: (draft: AnswerDraft) => AnswerDraft) =>
        setDrafts(old => old.map((draft, i) => i === index ? update(draft) : draft));
    const malformed = !q.questions.length || q.questions.some(item => !item.valid);
    return <form className="interaction chat-question" tabIndex={0} aria-label="Agent question"
        onKeyDown={e => {
            if (e.defaultPrevented || busy) return;
            const action = shortcut(e, "question");
            if (action === "reject") { e.preventDefault(); e.stopPropagation(); reject(); }
            else if (action === "submit") { e.preventDefault(); e.stopPropagation(); if (valid) submit(drafts); }
        }}
        onSubmit={e => { e.preventDefault(); if (valid && !busy) submit(drafts); }}>
        <strong>The agent has a question</strong>
        {malformed && <p role="alert">This request contains an unsupported question. It cannot be answered safely; reject it so the agent can ask again.</p>}
        {q.questions.map((item, i) => <fieldset key={i} disabled={busy || !item.valid}>
            <legend>{item.question}</legend>
            {item.multiple && <small>Select one or more answers.</small>}
            {item.options.map(option => <label className="answer-option" key={option.label}>
                <input type={item.multiple ? "checkbox" : "radio"} name={`${q.id}:${i}`}
                    checked={drafts[i].selected.includes(option.label)}
                    onChange={e => {
                        const checked = e.target.checked;
                        change(i, draft => ({ custom: item.multiple ? draft.custom : "", selected: item.multiple
                            ? checked ? [...draft.selected, option.label] : draft.selected.filter(v => v !== option.label)
                            : [option.label] }));
                    }} />
                <span>{option.label}{option.description && <small>{option.description}</small>}</span>
            </label>)}
            {item.custom && <label className="chat-custom-answer">{item.options.length ? "Custom answer" : "Your answer"}
                <input aria-label={`Answer: ${item.question}`} placeholder={item.options.length ? "Or type an answer…" : "Type an answer…"}
                    value={drafts[i].custom} onChange={e => {
                        const custom = e.target.value;
                        change(i, draft => ({ selected: item.multiple ? draft.selected : [], custom }));
                    }} />
            </label>}
        </fieldset>)}
        <div className="actions">
            <button type="submit" className="primary" disabled={busy || !valid}>{busy ? "Responding…" : "Submit answer"}</button>
            <button type="button" disabled={busy} onClick={reject}>Reject</button>
        </div>
        <small className="muted">Focus this card: Enter submits a complete answer · Escape rejects. Typing fields keep their normal keys.</small>
    </form>;
}

export function Interactions({ client, id, notify, skipPermissions = false }: {
    client: NeoismClient; id: string; notify(error: string): void; skipPermissions?: boolean;
}) {
    const latest = useRef({ client, id, skipPermissions });
    latest.current = { client, id, skipPermissions };
    const notifyRef = useRef(notify); notifyRef.current = notify;
    const controller = useRef<InteractionController | undefined>(undefined);
    const [state, setState] = useState<{ client: NeoismClient; id: string; skip: boolean; data: InteractionSnapshot }>();
    useEffect(() => {
        const current = () => latest.current.client === client && latest.current.id === id && latest.current.skipPermissions === skipPermissions;
        const store = createInteractionController(client, id, skipPermissions, current,
            data => setState({ client, id, skip: skipPermissions, data }), e => notifyRef.current(errorMessage(e)));
        controller.current = store;
        setState({ client, id, skip: skipPermissions, data: emptyInteractions() });
        void store.events(); void store.refresh();
        // SSE is immediate; polling/focus recover missed events and stream reconnects.
        const timer = setInterval(() => { if (!document.hidden) void store.refresh(); }, 15000);
        const focus = () => void store.refresh();
        window.addEventListener("focus", focus);
        return () => { store.dispose(); clearInterval(timer); window.removeEventListener("focus", focus); if (controller.current === store) controller.current = undefined; };
    }, [client, id, skipPermissions]);
    // Hide stale cards during the render BEFORE effect cleanup/reset, not one frame later.
    const data = state?.client === client && state.id === id && state.skip === skipPermissions ? state.data : emptyInteractions();
    const permission = (requestId: string, reply: "once" | "always" | "reject") => {
        if (reply === "always" && !window.confirm("Always allow this permission pattern for this session?")) return;
        void controller.current?.permission(requestId, reply);
    };
    return <div className="interactions chat-interactions" aria-label="Pending agent requests">
        {data.questions.map(q => <QuestionCard key={`${id}:${q.id}:${JSON.stringify(q.questions)}`} question={q} busy={!!data.busy}
            submit={drafts => { void controller.current?.question(q.id, drafts); }} reject={() => { void controller.current?.reject(q.id); }} />)}
        {data.permissions.map(p => <section className="interaction chat-permission" tabIndex={0} key={p.id}
            aria-label={`Permission requested: ${p.permission}`} onKeyDown={e => {
                if (e.defaultPrevented || data.busy) return;
                const action = shortcut(e, "permission");
                if (action && action !== "submit") { e.preventDefault(); e.stopPropagation(); permission(p.id, action); }
            }}>
            <strong>Permission requested · {p.permission}</strong>
            {p.source && <small>From {p.source}</small>}
            {p.title && <p>{p.title}</p>}
            {!!p.patterns.length && <pre>{p.patterns.join("\n")}</pre>}
            <div className="actions">
                <button disabled={!!data.busy} onClick={() => permission(p.id, "once")}>Allow once <kbd>Y</kbd></button>
                <button disabled={!!data.busy} onClick={() => permission(p.id, "always")}>Always allow <kbd>A</kbd></button>
                <button disabled={!!data.busy} onClick={() => permission(p.id, "reject")}>Reject <kbd>N</kbd></button>
            </div>
            <small className="muted">Focus this card: Y / Enter allows once · A asks to always allow · N / Escape rejects.</small>
        </section>)}
    </div>;
}
