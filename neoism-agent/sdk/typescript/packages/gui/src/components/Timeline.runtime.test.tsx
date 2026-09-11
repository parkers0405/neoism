import { expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { MessageWithParts } from "@neoism/sdk";
import { Timeline } from "./Timeline";

const user = { info: { id: "u", sessionId: "s", role: "user", author: { name: "Parker" }, time: { created: 1000 } }, parts: [{ id: "up", type: "text", text: "Question" }] };
const notification = {
    info: { id: "msg_background_completion_job_1", sessionId: "s", role: "user", time: { created: 2000 } },
    parts: [
        { id: "runtime-text", type: "text", text: "Background shell task finished.\njob_id: job_1\ndescription: Check sources\nstatus: completed\nexit_code: 0\ncwd: /repo\ncommand: cargo check\n\n<background_task_result>\n\x1b[32mOK\x1b[0m\n</background_task_result>" },
        { id: "tool", type: "tool", tool: "bash", state: { status: "completed", output: "Retained tool output" } },
        { id: "file", type: "file", filename: "report.txt", mime: "text/plain", url: "file:///report.txt" },
    ],
};
const assistant = { info: { id: "a", sessionId: "s", role: "assistant", agent: "build", modelId: "gpt-5.6", providerId: "openai", parentId: "u", time: { created: 3000, completed: 8000 }, finish: "stop" }, parts: [{ id: "answer", type: "text", text: "Final answer" }] };
const render = (rows: unknown[]) => renderToStaticMarkup(<Timeline messages={rows as MessageWithParts[]} busy={false} loading={false} loadOlder={() => {}} />);
it("keeps every original anchor and renders runtime envelopes only in collapsed task cards", () => {
    const html = render([user, notification, assistant]);
    expect([...html.matchAll(/data-message-id="([^"]+)"/g)].map(match => match[1])).toEqual(["u", "msg_background_completion_job_1", "a"]);
    const runtime = html.split('data-message-id="msg_background_completion_job_1"')[1].split("</article>")[0];
    expect(runtime).toContain('class="message user runtime-message"');
    expect(runtime).toContain('class="neo-runtime-notice tc-card completed"');
    expect(runtime).not.toContain("message-label");
    expect(runtime).not.toContain('class="markdown"');
    expect(runtime).not.toContain("\x1b"); expect(runtime).not.toContain(" open");
    expect(runtime).not.toContain("Retained tool output"); expect(runtime).toContain("report.txt");
    expect(notification.parts).toHaveLength(3);
    expect(html).not.toContain('class="message-label"');
});
it("renders current Agent notices with clean task titles and no ordinary user bubble", () => {
    const title = "Group worktree changes (@explore subagent)";
    const runtime = { info: { id: "imported", sessionId: "s", role: "user", system: "Agent runtime notification: background subagent completion." }, parts: [
        { id: "title", type: "text", text: title },
        { id: "notice", type: "text", text: "Subagent finished.\ntask_id: ses_child\nstatus: completed\n\n<task_result>\nDone\n</task_result>" },
    ] };
    const html = render([runtime]);
    expect(html).toContain('class="message user runtime-message"');
    expect(html).toContain('class="neo-runtime-title">Group worktree changes</span>');
    expect(html).not.toContain('class="message-label"'); expect(html).not.toContain('class="markdown"');
    expect(html).not.toContain(" open");
    // A human writing that same title is not a notification.
    const ordinary = render([{ ...user, parts: [{ id: "u", type: "text", text: title }] }]);
    expect(ordinary).not.toContain("runtime-message"); expect(ordinary).toContain('class="markdown"');
});
it("keeps final-response footer indexing across runtime rows and colors only the agent span", () => {
    const html = render([user, notification, assistant]);
    const response = html.split('data-message-id="a"')[1];
    expect(html.match(/<footer/g)).toHaveLength(1);
    expect(response).toContain('Final answer</p></div><footer class="neo-response-footer"');
    expect(response).toContain('<span class="neo-response-agent">Build</span> · GPT-5.6 · 7.0s');
    expect(html).not.toContain('class="message-label"');
});
