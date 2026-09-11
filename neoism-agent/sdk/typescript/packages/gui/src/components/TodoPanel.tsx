import { memo, useId, type InputHTMLAttributes } from "react";
import type { Part } from "@neoism/sdk";
import { todoStatusLabel, todosFromPart, type SessionTodo, type TodoStatus } from "../todoHelpers";
import "./todo-panel.css";

/** Decorative glyph: the containing row owns the accessible checkbox semantics. */
export function TodoCheck({ status }: { status: TodoStatus }) {
    return <span className="todo-check" data-status={status} aria-hidden="true">
        <svg viewBox="0 0 20 20" fill="none"><rect className="todo-check-box" x="3" y="3" width="14" height="14" rx="4" />
            <path className="todo-check-stroke" pathLength="1" d="m6 10 3 3 5-6" />
            <circle className="todo-check-spinner" cx="10" cy="10" r="6" /></svg>
    </span>;
}
export interface TodoPanelProps {
    todos: readonly SessionTodo[];
    /** Default native side-panel section; inline has the native thin left rule. */
    placement?: "side-panel" | "inline";
    title?: string;
    className?: string;
}
export const TodoPanel = memo(function TodoPanel({ todos, placement = "side-panel", title = "Tasks", className = "" }: TodoPanelProps) {
    const heading = useId();
    if (!todos.length) return null;
    const done = todos.filter(todo => todo.status === "completed").length;
    return <section className={`todo-panel ${className}`} data-placement={placement} aria-labelledby={heading}>
        <header><h3 id={heading}>{title}</h3><span className="todo-count" aria-label={`${done} of ${todos.length} tasks done`}>{done}/{todos.length}</span></header>
        <ul role="list" className="todo-list">{todos.map(todo => <li key={todo.key} data-todo-key={todo.key} data-status={todo.status}>
            <div role="checkbox" aria-checked={todo.status === "completed" ? true : todo.status === "in_progress" ? "mixed" : false}
                aria-readonly="true" aria-label={`${todo.content}: ${todoStatusLabel[todo.status]}`} className="todo-row">
                <TodoCheck status={todo.status} /><span className="todo-content">{todo.content}</span>
                <span className="todo-status" aria-hidden="true">{todoStatusLabel[todo.status]}</span>
            </div>
        </li>)}</ul>
    </section>;
});
/** Optional transcript fallback. Render only the latest successful part, not every plan. */
export function TodoToolPart({ part }: { part: Part }) {
    const todos = todosFromPart(part);
    if (todos === undefined) return null;
    return todos.length ? <TodoPanel todos={todos} placement="inline" /> : <p className="todo-empty">Tasks updated</p>;
}
/** Optional ReactMarkdown components.input; preserves user lists instead of hiding them. */
export function MarkdownTodoInput({ type, checked, node: _node, ...props }: InputHTMLAttributes<HTMLInputElement> & { node?: unknown }) {
    if (type !== "checkbox") return <input {...props} type={type} checked={checked} />;
    return <span className="todo-markdown-check" role="checkbox" aria-checked={!!checked} aria-readonly="true"
        aria-label={checked ? "Done" : "Pending"}><TodoCheck status={checked ? "completed" : "pending"} /></span>;
}
