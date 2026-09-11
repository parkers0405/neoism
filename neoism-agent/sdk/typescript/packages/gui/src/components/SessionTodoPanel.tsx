import type { MessageWithParts, NeoismClient } from "@neoism/sdk";
import { useSessionTodos } from "../useSessionTodos";
import { TodoPanel } from "./TodoPanel";

/** A stable session-level checklist, not one new list per todowrite call. */
export function SessionTodoPanel({ client, sessionId, messages }: {
    client: NeoismClient;
    sessionId: string;
    messages: readonly MessageWithParts[];
}) {
    const tasks = useSessionTodos(client, sessionId, { messages });
    return <TodoPanel todos={tasks.todos} placement="inline" />;
}
