import { ArrowUp } from "lucide-react";
import { Fragment, memo, useEffect, useLayoutEffect, useMemo, useRef, type ReactNode } from "react";
import { NativeActivity, type NativeActivityProps } from "./nativeActivity";
import { responseMetadata } from "./responseMetadata";
import { normalizeMessages, RuntimeNotice } from "./runtimeMessages";
import { ResponseFooter } from "./semanticMarkdown";
import "./message-presentation.css";
import { AttachmentPreview } from "./AttachmentPreview";
import { rasterMime } from "./attachmentPreview";
import type { NeoismClient, MessageWithParts, Part } from "@neoism/sdk";
import { ConversationSkeleton } from "./Skeleton";
import { Markdown } from "./Markdown";
import { scrollPlan, type ScrollSnapshot } from "./chatSupport";
import { ToolPart } from "./ToolPart";
import type { TaskChildStatus } from "./toolCardData";
import { ThinkingPart } from "./ThinkingPart";
import { outstandingTaskParts, pendingSnapshotMessage, taskRuntimeStates, visibleDetailParts, type LivePartMap } from "../livePartOrigins";
import { useHistoryPagination } from "../useHistoryPagination";
import { isTodoTool, latestTodoSnapshot } from "../todoHelpers";
import { TodoPanel } from "./TodoPanel";
import { SessionTodoPanel } from "./SessionTodoPanel";
import "./chat-details.css";

/** Server seeds an empty text part before reasoning. Hold that text until the
 * thinking block is painted so answer tokens cannot jump above it. A text part
 * that already finished before later reasoning started stays in chronological order. */
export function presentAssistantParts<T extends { type: string; time?: { start?: number; end?: number } }>(parts: T[]): T[] {
    const held: T[] = [];
    const presented: T[] = [];
    for (let i = 0; i < parts.length; i++) {
        const part = parts[i];
        if (part.type === "text" && parts.slice(i + 1).some(next => next.type === "reasoning" && !textFinishedBeforeReasoning(part, next))) {
            held.push(part);
            continue;
        }
        presented.push(part);
        if (part.type === "reasoning") presented.push(...held.splice(0));
    }
    presented.push(...held);
    return presented;
}
function textFinishedBeforeReasoning(text: { time?: { end?: number } }, reasoning: { time?: { start?: number } }) {
    const end = text.time?.end, start = reasoning.time?.start;
    return typeof end === "number" && typeof start === "number" && end <= start;
}

export function PartView({ part, childStatus, onOpenSession, client }: { client?: NeoismClient; part: Part; childStatus?: TaskChildStatus; onOpenSession?(id: string): void }) {
    switch (part.type) {
        case "text": return <Markdown text={part.text} />;
        case "reasoning": return <ThinkingPart part={part} />;
        case "tool":
        case "subtask": return <ToolPart part={part} childStatus={childStatus} onOpenSession={onOpenSession} />;
        case "agent": return <p className="muted">↳ Agent: {part.name}</p>;
        case "file": return <AttachmentPreview part={part} client={client} />;
        case "compaction": return <p className="divider-label">Context compacted</p>;
        default: return null;
    }
}

// Reducer deltas preserve all other message identities. A footer may change with
// turn context, so compare it too; never ignore a callback/runtime prop globally.
const MessageRow = memo(function MessageRow({ message, footer, todo, liveParts, livePending, outstandingIds, taskStates, onOpenSession, client }: { todo?: ReactNode; client?: NeoismClient; message: MessageWithParts; footer?: string; liveParts?: ReadonlySet<string>; livePending?: boolean; outstandingIds?: string; taskStates?: string; onOpenSession?(id: string): void }) {
    const { runtime: notice, remainingParts } = useMemo(() => normalizeMessages([message])[0], [message]);
    const outstanding = useMemo(() => outstandingIds ? new Set<string>(JSON.parse(outstandingIds)) : undefined, [outstandingIds]);
    const childStates = useMemo(() => new Map<string, TaskChildStatus>(taskStates ? JSON.parse(taskStates) : []), [taskStates]);
    const parts = visibleDetailParts(remainingParts as Part[], liveParts, livePending, outstanding)
        .filter(part => !(part.type === "tool" && part.state?.status === "completed" && isTodoTool(part)));
    if (!notice && !todo && !parts.some(p => ['text', 'reasoning', 'tool', 'subtask', 'agent', 'file', 'compaction'].includes(p.type))) return null;
    // Presentation only: never mutate wire parts. User images lead; assistant
    // text that was seeded before reasoning is painted after the thinking block.
    const indexed = parts.map((part, partIndex) => ({ part, partIndex }));
    const isImage = ({ part }: typeof indexed[number]) => part.type === "file" && rasterMime(part.mime);
    const presented = message.info.role === "user" && !notice
        ? [...indexed.filter(isImage), ...indexed.filter(entry => !isImage(entry))]
        : presentAssistantParts(parts).map(part => ({ part, partIndex: parts.indexOf(part) }));
    const lastText = presented.map(({ part }) => part.type).lastIndexOf("text");
    return <article data-message-id={message.info.id} className={`message ${message.info.role}${notice ? " runtime-message" : ""}`}>
        {notice && <RuntimeNotice notice={notice} />}
        {presented.map(({ part }, index) => <Fragment key={part.id}>
            <PartView client={client} part={part} childStatus={childStates.get(part.id)} onOpenSession={onOpenSession} />
            {!notice && index === lastText && footer && <ResponseFooter value={footer} />}
        </Fragment>)}
        {todo}
    </article>;
});

type Anchor = ScrollSnapshot & { anchorId?: string; offset?: number };
export const Timeline = memo(function Timeline({ messages, busy, activityBusy, older, loading, loadOlder, sessionId, runtime, showActivity = true, activity, sessionActivity, activityPalette, liveParts, onOpenSession, client }: {
    activityBusy?: boolean;
    showActivity?: boolean;
    client?: NeoismClient;
    liveParts?: LivePartMap;
    onOpenSession?(id: string): void;
    messages: MessageWithParts[]; busy: boolean; older?: string; loading: boolean; loadOlder(): void;
    /** Optional explicit identity for empty/loading sessions; existing callers safely derive it from messages. */
    sessionId?: string;
    runtime?: NativeActivityProps["runtime"];
    sessionActivity?: NativeActivityProps["sessionActivity"];
    activity?: NativeActivityProps["activity"];
    activityPalette?: NativeActivityProps["palette"];
}) {
    const footers = useMemo(() => responseMetadata(messages), [messages]);
    const viewport = useRef<HTMLDivElement>(null), transcript = useRef<HTMLDivElement>(null);
    const follow = useRef(true), previous = useRef<Anchor | undefined>(undefined);
    const session = sessionId ?? messages[0]?.info.sessionId ?? "";
    const latestTodo = useMemo(() => latestTodoSnapshot(messages, session), [messages, session]);
    const showTodo = !!latestTodo && latestTodo.todos.length > 0 && !!liveParts?.get(latestTodo.messageId)?.has(latestTodo.partId);
    const todoAnchor = useRef<{session: string; client?: NeoismClient; id?: string}>({session, client});
    if (todoAnchor.current.session !== session || todoAnchor.current.client !== client || !showTodo) {
        todoAnchor.current = {session, client};
    }
    if (showTodo && (!todoAnchor.current.id || !messages.some(message => message.info.id === todoAnchor.current.id))) {
        todoAnchor.current.id = latestTodo!.messageId;
    }
    const todoMessage = latestTodo ? messages.find(message => message.info.id === latestTodo.messageId) : undefined;
    const todoMessages = useMemo(() => todoMessage ? [todoMessage] : [], [todoMessage]);
    const todoContent = useMemo(() => {
        if (!showTodo) return undefined;
        return client
            ? <SessionTodoPanel key={`todos:${session}`} client={client} sessionId={session} messages={todoMessages} />
            : <TodoPanel key={`todos:${session}`} todos={latestTodoSnapshot(todoMessages, session)?.todos ?? []} placement="inline" />;
    }, [showTodo, client, session, todoMessages]);
    const pendingMessage = pendingSnapshotMessage(messages, busy);
    const outstanding = useMemo(() => outstandingTaskParts(messages, runtime, session), [messages, runtime, session]);
    const childStates = useMemo(() => taskRuntimeStates(messages, runtime, session), [messages, runtime, session]);
    const pagination = useHistoryPagination({ sessionId: session, cursor: older, loading, loadOlder });
    const ids = messages.map(m => m.info.id);
    const live = useRef({ session, ids }); live.current = { session, ids };
    function capture() {
        const el = viewport.current;
        if (!el) return;
        const top = el.getBoundingClientRect().top;
        const visible = [...el.querySelectorAll<HTMLElement>("[data-message-id]")].find(node => node.getBoundingClientRect().bottom > top);
        previous.current = { session: live.current.session, ids: live.current.ids, height: el.scrollHeight, top: el.scrollTop,
            anchorId: visible?.dataset.messageId, offset: visible ? visible.getBoundingClientRect().top - top : undefined };
    }
    useLayoutEffect(() => {
        const el = viewport.current;
        if (!el) return;
        const before = previous.current;
        const plan = scrollPlan(before, session, ids, el.scrollHeight, follow.current);
        if (plan.mode === "reset") { follow.current = true; el.scrollTop = el.scrollHeight; }
        else if (plan.mode === "anchor") {
            follow.current = false;
            const anchor = [...el.querySelectorAll<HTMLElement>("[data-message-id]")].find(node => node.dataset.messageId === before?.anchorId);
            el.scrollTop = anchor && before?.offset !== undefined
                ? el.scrollTop + anchor.getBoundingClientRect().top - el.getBoundingClientRect().top - before.offset
                : plan.top ?? el.scrollTop;
        } else if (plan.mode === "follow") el.scrollTop = el.scrollHeight;
        capture();
    }, [messages, busy, loading, older, session]);
    useEffect(() => {
        if (typeof ResizeObserver === "undefined" || !transcript.current) return;
        const observer = new ResizeObserver(() => {
            if (follow.current && viewport.current) viewport.current.scrollTop = viewport.current.scrollHeight;
            capture();
        });
        observer.observe(transcript.current);
        return () => observer.disconnect();
    }, []);
    return <div className="timeline chat-timeline" ref={viewport} onWheel={pagination.onWheel} onTouchStart={pagination.onTouchStart} onTouchMove={pagination.onTouchMove} onKeyDown={pagination.onKeyDown} onScroll={e => {
        pagination.onScroll(e);
        const el = e.currentTarget;
        follow.current = el.scrollHeight - el.scrollTop - el.clientHeight < 120;
        capture();
    }}>
        <div className="transcript" ref={transcript}>
            {loading && !messages.length && <ConversationSkeleton />}
            {older && loading && !!messages.length && <ConversationSkeleton code={false} />}
            {older && !loading && <button type="button" className="history-page-control" aria-label="Load earlier messages" onClick={() => {
                follow.current = false; capture(); loadOlder();
            }}><ArrowUp size={15} aria-hidden="true" /></button>}
            {messages.map((message, index) => <MessageRow client={client} key={message.info.id} message={message} footer={footers[index]} todo={message.info.id === todoAnchor.current.id ? todoContent : undefined} liveParts={liveParts?.get(message.info.id)} livePending={message.info.id === pendingMessage} outstandingIds={outstanding.get(message.info.id)} taskStates={childStates.get(message.info.id)} onOpenSession={onOpenSession} />)}
            {showActivity && <NativeActivity sessionActivity={sessionActivity} messages={messages} busy={activityBusy ?? busy} runtime={runtime} activity={activity} palette={activityPalette} sessionId={session} />}
        </div>
    </div>;
});
