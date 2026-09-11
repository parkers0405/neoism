import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import type { Event, MessageWithParts, Part } from '@neoism/sdk';
import { livePartLedger, markLiveEvent, markPromptSnapshot, liveOriginRegistry, visibleDetailParts } from './livePartOrigins';
import { Timeline } from './components/Timeline';
const part = (id: string, type: string, text = id) => ({ id, type, sessionId: 's', messageId: 'm', text, time: { start: 1, end: 2 }, tool: 'bash', callId: id, state: { status: 'completed', title: text, output: text } }) as Part;
const message = (id: string, parentId: string, parts: Part[]) => ({ info: { id, sessionId: 's', role: 'assistant', parentId, time: { created: 1 } }, parts }) as MessageWithParts;
const event = (type: string, data: object) => ({ type, data }) as Event;

describe('view-local detail origins', () => {
    it('hides historical reasoning/tools, not their raw metadata or final text', () => {
        const parts = [part('r', 'reasoning', 'OLD REASONING'), part('t', 'tool', 'OLD TOOL'), part('a', 'text', 'Final text')];
        const m = message('m', 'old-prompt', parts), ledger = livePartLedger();
        markPromptSnapshot(ledger, [m]);
        expect(visibleDetailParts(parts, ledger.parts.get('m')).map(p => p.id)).toEqual(['a']);
        const html = renderToStaticMarkup(<Timeline messages={[m]} liveParts={ledger.parts} busy={false} loading={false} loadOlder={() => {}} />);
        expect(html).toContain('Final text'); expect(html).not.toContain('OLD TOOL'); expect(html).not.toContain('OLD REASONING');
        expect(m.parts).toBe(parts); expect(parts).toHaveLength(3);
    });
    it('shows newly updated/delta parts without revealing other historical parts in that message', () => {
        const ledger = livePartLedger(), parts = [part('old', 'tool'), part('new-tool', 'tool'), part('new-thinking', 'reasoning')];
        markLiveEvent(ledger, event('message.part.updated', { part: parts[1] }));
        markLiveEvent(ledger, event('message.part.delta', { messageID: 'm', partID: 'new-thinking' }));
        expect(visibleDetailParts(parts, ledger.parts.get('m')).map(p => p.id)).toEqual(['new-tool', 'new-thinking']);
        const identity = ledger.parts.get('m');
        markPromptSnapshot(ledger, [message('m', 'old', parts)]);
        markLiveEvent(ledger, event('message.part.delta', { messageID: 'm', partID: 'new-thinking' }));
        expect(ledger.parts.get('m')).toBe(identity);
        const html = renderToStaticMarkup(<Timeline messages={[message('m', 'old', parts)]} liveParts={ledger.parts} busy={false} loading={false} loadOlder={() => {}} />);
        expect(html).toContain('neo-thinking-part'); expect(html).toContain('new-thinking'); expect(html).toContain('new-tool');
        expect(html).not.toContain('chat-thinking');
    });
    it('recovers a direct prompt response before SSE via explicit causal IDs, not ordering or clocks', () => {
        const registry = liveOriginRegistry(), client = {}, ledger = registry.view(client, 's');
        const prompt = registry.prompt(client, 's');
        const old = message('zzz-older', 'unrelated', [part('old', 'tool')]);
        const response = message('aaa-new', prompt, [part('new', 'reasoning')]);
        markPromptSnapshot(ledger, [old, response]);
        expect(ledger.parts.get(old.info.id)).toBeUndefined(); expect(ledger.parts.get(response.info.id)?.has('new')).toBe(true);
        // Parent is no longer in the newest HTTP page, but its established origin survives.
        markPromptSnapshot(ledger, [message('opaque-continuation', 'aaa-new', [part('continued', 'tool')])]);
        expect(ledger.parts.get('opaque-continuation')?.has('continued')).toBe(true);
    });
    it('handles new-session creation/prompt before the first selected-view render and resets on return', () => {
        const registry = liveOriginRegistry(), client = {};
        registry.view(client, undefined); registry.created(client, 'new'); registry.prompt(client, 'new');
        const ledger = registry.view(client, 'new');
        markPromptSnapshot(ledger, [message('m', 'server-prompt', [part('tool', 'tool')])]);
        expect(ledger.parts.get('m')?.has('tool')).toBe(true);
        registry.view(client, 'other'); const reopened = registry.view(client, 'new');
        markPromptSnapshot(reopened, [message('m', 'server-prompt', [part('tool', 'tool')])]);
        expect(reopened.parts.size).toBe(0); expect(reopened.createdHere).toBe(false);
        expect(registry.view({}, 'new').parts.size).toBe(0);
    });
    it('keeps native durable runtime notices, but hides their old tool-detail parts', () => {
        const m = { info: { id: 'msg_background_completion_job', sessionId: 's', role: 'user', time: {} }, parts: [part('notice', 'text', 'Background shell task finished.\njob_id: job\nstatus: completed\n\n<background_task_result>\nDone\n</background_task_result>'), part('old-tool', 'tool', 'HIDDEN TOOL')] } as MessageWithParts;
        const html = renderToStaticMarkup(<Timeline messages={[m]} liveParts={new Map()} busy={false} loading={false} loadOlder={() => {}} />);
        expect(html).toContain('neo-runtime-notice'); expect(html).not.toContain('HIDDEN TOOL'); expect(html).not.toContain('message-label');
    });
});
