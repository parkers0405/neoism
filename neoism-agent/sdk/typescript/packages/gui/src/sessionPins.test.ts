import { afterEach, expect, it, vi } from "vitest";
import type { NeoismClient, Session } from "@neoism/sdk";
import { hydrateSessionPins, SessionPinIndex } from "./sessionPins";
const session = (id: string, pinned = true): Session => ({id,pinned,title:id,directory:'/work',projectId:'p',slug:id,version:'1',time:{created:1,updated:1}});
function storage() {
    const values = new Map<string,string>();
    vi.stubGlobal('localStorage',{getItem:(key:string) => values.get(key) ?? null, setItem:(key:string,value:string) => values.set(key,value)});
}
afterEach(() => vi.unstubAllGlobals());
it('bounds hydration to four scoped gets and stops scheduling after cancellation', async () => {
    storage(); const index = new SessionPinIndex('http://server');
    for (let i=0;i<12;i++) index.observe(session(String(i)));
    let alive = true;
    const resolves: ((s:Session)=>void)[] = [];
    const get = vi.fn(() => new Promise<Session>(resolve => resolves.push(resolve)));
    const receive = vi.fn();
    const task = hydrateSessionPins({sessions:{get}} as unknown as NeoismClient,index,() => alive,receive);
    expect(get).toHaveBeenCalledTimes(4);
    alive = false; resolves.forEach((resolve,i) => resolve(session(String(i)))); await task;
    expect(get).toHaveBeenCalledTimes(4); expect(receive).not.toHaveBeenCalled();
});
it('prunes deleted IDs, ignores empty responses and retains auth/network failures for retry', async () => {
    storage(); const index = new SessionPinIndex('http://server');
    for (const id of ['deleted','empty','private','offline','unpinned']) index.observe(session(id));
    const get = vi.fn(async (id:string) => {
        if(id === 'empty') return undefined;
        if(id === 'unpinned') return session(id,false);
        throw {status:id === 'deleted' ? 404 : id === 'private' ? 403 : 500};
    });
    await hydrateSessionPins({sessions:{get}} as unknown as NeoismClient,index,() => true,s => index.observe(s));
    expect([...new SessionPinIndex('http://server').ids]).toEqual(['empty','private','offline']);
    expect(new SessionPinIndex('http://other').ids.size).toBe(0);
});
it('ignores malformed storage and never stores server credentials', () => {
    vi.stubGlobal('localStorage',{getItem:() => '{bad',setItem:vi.fn()});
    const index = new SessionPinIndex('https://user:password@server/path?token=secret');
    index.observe(session('pin'));
    expect(localStorage.setItem).toHaveBeenCalledWith('neoism.gui.pin-index:https://server/path','["pin"]');
});
