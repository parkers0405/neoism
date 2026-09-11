import { memo, useId, useState } from "react";
import type { Part } from "@neoism/sdk";
import { Markdown } from "./Markdown";
import { stripTerminalControls } from "./runtimeMessages";
import "./thinking-part.css";
export type ThinkingPartProps = { part: Extract<Part, { type: "reasoning" }> };
/** Native references: api_mapping.rs::agent_message_reasoning (always "Thinking"),
 * assistant.rs::render_reasoning_message_with (12px amber italic, 18px inset),
 * message_card.rs::measure_message (empty = zero, otherwise expanded), and
 * markdown.rs::render_markdown_blocks (13.5px italic, dim prose).
 * Native does not display reasoning duration or auto-collapse on completion.
 * Disclosure is a web accessibility affordance, never a stream-driven effect. */
function ThinkingBlock({ part }: ThinkingPartProps) {
    const [expanded, setExpanded] = useState(true), bodyId = useId();
    if (!part.text.trim()) return null;
    return <section className="neo-thinking-part" aria-label="Thinking">
        <button type="button" className="neo-thinking-header" aria-expanded={expanded} aria-controls={bodyId} onClick={() => setExpanded(v => !v)}>
            <span>Thinking</span>
        </button>
        {expanded && <div id={bodyId} className="neo-thinking-body"><Markdown text={stripTerminalControls(part.text)} /></div>}
    </section>;
}
/** Key state to the durable block, not text/revision/time. No fabricated clock. */
export const ThinkingPart = memo(function ThinkingPart({ part }: ThinkingPartProps) {
    return <ThinkingBlock key={`${part.sessionId}:${part.messageId}:${part.id}`} part={part} />;
});
