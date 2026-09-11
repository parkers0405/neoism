import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { decodeRuntimeMessage, normalizeMessages, RuntimeNotice, stripTerminalControls } from "./runtimeMessages";
const shell = "Background shell task finished.\njob_id: job_1\ndescription: Check sources\nstatus: completed\nexit_code: 0\ncwd: /repo\ncommand: cargo check\n\nThe captured process output is included below as runtime system context.\nCall background_task_result with this job_id to reread retained output during this server lifetime.\n\n<background_task_result>\n\x1b[32mOK\x1b[0m <script>alert(1)</script>\n</background_task_result>";
const row = (content: string, info = {}) => ({ info: { id: "human", role: "user", ...info }, parts: [{ type: "text", id: "p", text: content }] });
describe("runtime notification projection", () => {
    it("accepts current Agent and legacy marker kinds across runtime roles without relying on the ID", () => {
        for (const prefix of ["Agent ", "Neoism ", ""]) for (const role of ["user", "system", "assistant"]) {
            const notice = decodeRuntimeMessage(row("Group worktree changes (@explore subagent)", {
                role, system: `${prefix}runtime notification: background subagent completion.`,
            }))!;
            expect(notice.kind).toBe("subagent"); expect(notice.title).toBe("Group worktree changes");
            expect(notice.envelope).toContain("(@explore subagent)");
        }
        expect(decodeRuntimeMessage(row("", { system: "Agent runtime notification: background shell task completion." }))?.kind).toBe("shell");
    });
    it("normalizes a separate title part and completion envelope without leaking the title into Markdown", () => {
        const title = "Group worktree changes (@explore subagent)";
        const message = { info: { id: "imported-notice", role: "user", system: "Agent runtime notification: background subagent completion." }, parts: [
            { type: "text", id: "title", text: title },
            { type: "text", id: "body", text: `Subagent finished.\ntask_id: ses_1\nagent: @explore\ntitle: ${title}\nstatus: completed\n\n<task_result>\nDone\n</task_result>` },
        ] };
        const projected = normalizeMessages([message])[0];
        expect(projected.runtime?.title).toBe("Group worktree changes");
        expect(projected.runtime?.fields.task_id).toBe("ses_1");
        expect(projected.runtime?.output).toBe("Done"); expect(projected.remainingParts).toEqual([]);
        expect(projected.runtime?.envelope).toContain(title); expect(message.parts[0].text).toBe(title);
    });
    it("preserves unmarked title-only human/assistant messages and mentions verbatim", () => {
        for (const role of ["user", "assistant"]) for (const content of ["Group worktree changes (@explore subagent)", "The task title is Group worktree changes (@explore subagent)", "Agent runtime notification: background subagent completion."]) {
            const message = row(content, { role });
            const projected = normalizeMessages([message])[0];
            expect(projected.runtime).toBeUndefined(); expect(projected.message).toBe(message);
            expect(projected.remainingParts).toBe(message.parts);
        }
    });
    it("requires an anchored full envelope after a literal runtime heading", () => {
        const content = `Agent runtime notification: background shell task completion.\n${shell}`;
        expect(decodeRuntimeMessage(row(content))?.kind).toBe("shell");
        expect(decodeRuntimeMessage(row(`Example:\n${content}`))).toBeUndefined();
        expect(decodeRuntimeMessage(row(`> ${content.replaceAll("\n", "\n> ")}`))).toBeUndefined();
    });
    it("maps history and earliest live reserved-ID parts to the same stable card", () => {
        const history = decodeRuntimeMessage(row(shell, { id: "msg_background_completion_job_1", system: "Neoism runtime notification: background shell task completion." }))!;
        const live = decodeRuntimeMessage({ type: "text", messageID: "msg_background_completion_job_1", text: "" })!;
        expect(live.id).toBe(history.id); expect(history.id).toBe("background-task-job_1");
        expect(history.output).toBe("OK <script>alert(1)</script>");
        expect(history.fields.exit_code).toBe("0"); expect(history.title).toBe("Check sources");
        expect(decodeRuntimeMessage(row(shell))?.id).toBe(history.id);
    });
    it("preserves ordinary and quoted mentions, including assistant explanations", () => {
        for (const content of ["Subagent finished.", "Background shell task finished. Why?", `Example:\n${shell}`, `\`\`\`text\n${shell}\n\`\`\``, `> ${shell.replaceAll("\n", "\n> ")}`, 'job_id: fake\n<background_task_result>\nhello\n</background_task_result>']) expect(decodeRuntimeMessage(row(content))).toBeUndefined();
        expect(decodeRuntimeMessage(row(shell, { role: "assistant" }))).toBeUndefined();
    });
    it("preserves all original rows and unrelated parts without mutation", () => {
        const ordinary = row("hello"), runtime = { ...row(shell), parts: [...row(shell).parts, { type: "file", id: "file", url: "x" }] };
        const result = normalizeMessages([ordinary, runtime]);
        expect(result[0].message).toBe(ordinary); expect(result[0].remainingParts).toBe(ordinary.parts);
        expect(result[1].remainingParts).toEqual([runtime.parts[1]]); expect(runtime.parts).toHaveLength(2);
    });
    it("renders a collapsed escaped preview without raw notification submenus", () => {
        const html = renderToStaticMarkup(<RuntimeNotice notice={decodeRuntimeMessage(row(shell))!} />);
        expect(html).not.toContain("<script>"); expect(html).not.toContain("\x1b");
        expect(html).toContain("&lt;script&gt;"); expect(html).toContain('class="tc-preview"');
        expect(html).not.toContain("Full runtime notification"); expect(html).not.toContain("<details");
        expect(html).not.toContain('class="markdown"');
    });
    it("handles subagent completions and errors on history/live", () => {
        const content = "Subagent finished.\ntask_id: ses_1\nagent: @build\ntitle: Implement\nstatus: error\n\nThe subagent result is included below as runtime system context.\n\n<task_error>\nfailed\n</task_error>";
        expect(decodeRuntimeMessage(row(content))?.output).toBe("failed");
        expect(decodeRuntimeMessage({ type: "text", messageID: "msg_subtask_completion_ses_1", text: "" })?.kind).toBe("subagent");
        expect(decodeRuntimeMessage(row("", { system: "Neoism runtime notification: background subagent completion." }))?.kind).toBe("subagent");
    });
    it("does not let output lines overwrite task metadata", () => {
        const notice = decodeRuntimeMessage(row(shell.replace("OK", "OK\nstatus: error\njob_id: injected")))!;
        expect(notice.status).toBe("completed"); expect(notice.id).toBe("background-task-job_1");
        expect(notice.output).toContain("job_id: injected");
    });
    it("accepts older raw header/tag envelopes and safely handles partial marked output", () => {
        const legacy = shell.replace(/The captured process output[^\n]*\nCall background_task_result[^\n]*\n/, "");
        expect(decodeRuntimeMessage(row(legacy))?.kind).toBe("shell");
        const failed = shell.replace("status: completed", "status: timed_out").replaceAll("background_task_result>", "background_task_error>");
        expect(decodeRuntimeMessage(row(failed))?.status).toBe("timed_out");
        const partial = shell.slice(0, shell.indexOf("</background_task_result>"));
        expect(decodeRuntimeMessage(row(partial, { id: "msg_background_completion_job_1" }))?.output).toContain("OK");
    });
    it("reassembles live text parts and retains every batched child result", () => {
        const message = row(shell);
        message.parts = [{ type: "text", id: "p1", text: shell.slice(0, 70) }, { type: "text", id: "p2", text: shell.slice(70) }];
        expect(decodeRuntimeMessage(message)?.title).toBe("Check sources");
        const batch = "Subagents finished.\ncount: 2\n\nThe subagent results are included below as runtime system context.\n\n---\ntask_id: ses_1\nstatus: completed\n\n<task_result>\nFIRST\n</task_result>\n---\ntask_id: ses_2\nstatus: error\n\n<task_error>\nSECOND\n</task_error>";
        const notice = decodeRuntimeMessage(row(batch))!;
        expect(notice.output).toContain("FIRST"); expect(notice.output).toContain("SECOND");
    });
    it("strips OSC hyperlinks, CSI and control strings without executing them", () => {
        expect(stripTerminalControls("\x1b]8;;https://evil\x1b\\label\x1b]8;;\x07\x1b[31mred\x1b[0m\n\tX\x00")).toBe("labelred\n\tX");
    });
});
