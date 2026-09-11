import { useEffect, useRef, useState } from "react";
import type { NeoismClient } from "@neoism/sdk";
import type { useAppController } from "../useAppController";
import { TodoPanel } from "./TodoPanel";
import { useSessionTodos } from "../useSessionTodos";
import { activeSidebarTasks, contextCaption, contextFraction, latestSidebarUsage, rawContextLimit, sidebarTaskTitle, type SidebarUsageSource } from "./sidebarUsage";
import { createSubagentController, emptySubagents, type SubagentSnapshot } from "./subagentController";
import "./chat-details.css";
import "./sidebar-native.css";

function useSubagents(client: NeoismClient, id: string | undefined) {
    const latest = useRef({ client, id }); latest.current = { client, id };
    const [state, setState] = useState<{ client: NeoismClient; id: string; data: SubagentSnapshot }>();
    useEffect(() => {
        if (!id) return;
        const current = () => latest.current.client === client && latest.current.id === id;
        const store = createSubagentController(client, id, current, data => setState({ client, id, data }));
        setState({ client, id, data: emptySubagents() });
        void store.events(); void store.refresh();
        const timer = setInterval(() => { if (!document.hidden) void store.refresh(); }, 15000);
        const focus = () => void store.refresh();
        window.addEventListener("focus", focus);
        return () => { store.dispose(); clearInterval(timer); window.removeEventListener("focus", focus); };
    }, [client, id]);
    return { data: state?.client === client && state.id === id ? state.data : emptySubagents() };
}
type SidebarApp = Pick<ReturnType<typeof useAppController>, "client" | "id" | "usage" | "active" | "prefs" | "model" | "agent" | "thinking" | "openSession"> & SidebarUsageSource & Partial<Pick<ReturnType<typeof useAppController>, "chat">>;

export function ChatDetails({ app: a }: { app: SidebarApp }) {
    const tasks = useSubagents(a.client, a.id);
    const todos = useSessionTodos(a.client, a.id, {messages:a.chat?.state.messages,serverKey:a.prefs.server});
    const usage = latestSidebarUsage(a.usage.filter(part => part.sessionId === a.id));
    const active = a.active?.id === a.id ? a.active : undefined;
    const model = a.model || (active?.model ? `${active.model.providerId}/${active.model.id}` : "");
    const limit = rawContextLimit(a.providerCatalog, model);
    const fraction = contextFraction(usage.context, limit);
    const caption = contextCaption(usage.context, limit);
    return <aside className="right-sidebar chat-details sidebar-native">
        <section><h3>Directory</h3><p className="directory">{active?.directory || a.prefs.directory || "Server workspace"}</p></section>
        <section><h3>Usage</h3>
            <div className="sidebar-context-meter" role={fraction === undefined ? "img" : "meter"} aria-label={fraction === undefined ? `Context usage: ${caption}` : "Context usage"}
                aria-valuemin={fraction === undefined ? undefined : 0} aria-valuemax={fraction === undefined ? undefined : 100}
                aria-valuenow={fraction === undefined ? undefined : fraction * 100} aria-valuetext={fraction === undefined ? undefined : caption}>
                <div className="sidebar-context-well">{fraction !== undefined && <div className="sidebar-context-fill" style={{ width: `${fraction * 100}%` }} />}</div>
            </div>
            <p className="sidebar-context-caption">{caption}</p>
        </section>
        <SidebarSubagents data={tasks.data} parentId={a.id} open={a.openSession} />
        <TodoPanel key={`${a.prefs.server}:${a.id}`} todos={todos.todos} />
    </aside>;
}

export function SidebarSubagents({ data, parentId, open }: {
    data: SubagentSnapshot; parentId?: string; open: (id: string) => void;
}) {
    const rows = activeSidebarTasks(data.rows, parentId);
    if (!rows.length) return null;
    return <section className="sidebar-subagents">
        <h3>Subagents</h3>
        {rows.map(task => <button className="chat-subagent-open" key={task.sessionId} onClick={() => open(task.sessionId)} title="Open subagent">{sidebarTaskTitle(task)}</button>)}
    </section>;
}
