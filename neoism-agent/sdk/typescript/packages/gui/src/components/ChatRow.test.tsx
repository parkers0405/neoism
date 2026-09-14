// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Session } from "@neoism/sdk";
import { readFileSync } from 'node:fs';
import { ChatRow } from "./ChatRow";
(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
let host: HTMLDivElement, root: Root;
const open = vi.fn(), pin = vi.fn(async () => {}), rename = vi.fn(async () => {}), remove = vi.fn(async () => {});
const s: Session = {id:'a',title:'Chat A',pinned:false,directory:'/work',projectId:'p',slug:'a',version:'1',time:{created:1,updated:1}};
const button = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
async function click(node: HTMLElement) { await act(async () => node.click()); }
async function key(node: HTMLElement, key: string) { await act(async () => node.dispatchEvent(new KeyboardEvent('keydown',{key,bubbles:true}))); }
async function menu() { await click(button('Actions for Chat A')); return document.querySelector<HTMLElement>('[role="menu"]')!; }
beforeEach(async () => {
    vi.clearAllMocks(); host = document.createElement('div'); document.body.append(host); root = createRoot(host);
    await act(async () => root.render(<ChatRow session={s} selected={false} open={open} pin={pin} rename={rename} remove={remove} />));
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); });
it('gives the idle title full width, reserves desktop actions only on interaction, and hides touch controls', async () => {
    const css = readFileSync('src/components/chat-menu.css', 'utf8');
    expect(css).toMatch(/\.recent \.recent-action\s*\{[^}]*position: absolute/);
    expect(css).toMatch(/\.recent:hover \.recent-action,[\s\S]*?\.recent:focus-within \.recent-action,[\s\S]*?\.recent.menu-open \.recent-action\s*\{[^}]*position: static/);
    expect(css).toContain('@media (hover: none), (pointer: coarse)');
    expect(css).toContain('.recent .recent-action { display: none; }');
    expect(host.querySelector('.recent-title .hover-title')).not.toBeNull();
    await menu(); expect(host.querySelector('.recent.menu-open')).not.toBeNull();
    await key(document.activeElement as HTMLElement, 'Escape'); expect(host.querySelector('.menu-open')).toBeNull();
});
it('long press opens touch actions without opening chat; tap and vertical scrolling stay independent', async () => {
    vi.useFakeTimers();
    const title = host.querySelector<HTMLButtonElement>('.recent-title')!;
    const pointer = (type: string, y = 10) => act(() => title.dispatchEvent(new PointerEvent(type, { bubbles: true, pointerType: 'touch', isPrimary: true, clientX: 10, clientY: y })));
    try {
        pointer('pointerdown'); pointer('pointerup'); act(() => title.click()); expect(open).toHaveBeenCalledOnce(); open.mockClear();
        pointer('pointerdown'); pointer('pointermove', 30); act(() => vi.advanceTimersByTime(600)); expect(document.querySelector('[role=menu]')).toBeNull();
        pointer('pointerdown'); pointer('pointercancel'); act(() => vi.advanceTimersByTime(600)); expect(document.querySelector('[role=menu]')).toBeNull();
        pointer('pointerdown'); act(() => vi.advanceTimersByTime(500)); pointer('pointerup'); act(() => title.click());
        expect(open).not.toHaveBeenCalled();
        const popover = document.querySelector('[role=menu]')!;
        expect([...popover.querySelectorAll('button')].map(b => b.textContent)).toEqual(['Pin', 'Rename', 'Delete']);
        await key(document.activeElement as HTMLElement, 'Escape'); expect(document.activeElement).toBe(title);
        pointer('pointerdown'); act(() => root.render(null)); act(() => vi.advanceTimersByTime(600)); expect(document.querySelector('[role=menu]')).toBeNull();
    } finally { vi.useRealTimers(); }
});
it('offers all touch actions from the keyboard context-menu key too', async () => {
    const title = host.querySelector<HTMLButtonElement>('.recent-title')!;
    await key(title, 'ContextMenu');
    expect([...document.querySelectorAll('[role=menuitem]')].map(b => b.textContent)).toEqual(['Pin', 'Rename', 'Delete']);
    await click(document.querySelector<HTMLButtonElement>('[role=menuitem]')!); expect(pin).toHaveBeenCalledWith(true);
});
it('shows only pin and ellipsis actions; clicking pin never opens a chat', async () => {
    expect(host.querySelectorAll('.recent-action')).toHaveLength(2);
    expect(button('Actions for Chat A').getAttribute('aria-expanded')).toBe('false');
    await click(button('Pin Chat A')); expect(pin).toHaveBeenCalledWith(true); expect(open).not.toHaveBeenCalled();
});
it('anchors an accessible menu with arrow navigation and Escape focus restoration', async () => {
    const popover = await menu();
    expect(button('Actions for Chat A').getAttribute('aria-expanded')).toBe('true');
    expect([...popover.querySelectorAll('button')].map(b => b.textContent)).toEqual(['Rename','Delete']);
    expect(document.activeElement?.textContent).toBe('Rename');
    await key(document.activeElement as HTMLElement,'ArrowDown'); expect(document.activeElement?.textContent).toBe('Delete');
    await key(document.activeElement as HTMLElement,'Escape');
    expect(document.querySelector('[role="menu"]')).toBeNull(); expect(document.activeElement).toBe(button('Actions for Chat A'));
    expect(open).not.toHaveBeenCalled();
});
it('closes on outside pointer, scroll and unmount without leaving a floating menu', async () => {
    await menu(); await act(async () => document.body.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true})));
    expect(document.querySelector('[role="menu"]')).toBeNull();
    await menu(); await act(async () => host.dispatchEvent(new Event('scroll')));
    expect(document.querySelector('[role="menu"]')).toBeNull();
    await menu(); await act(async () => root.render(null)); expect(document.querySelector('[role="menu"]')).toBeNull();
});
it('renames inline with Enter and cancels with Escape, without browser prompt', async () => {
    const prompt = vi.fn(); vi.stubGlobal('prompt',prompt);
    await click((await menu()).querySelectorAll('button')[0]);
    const input = document.querySelector<HTMLInputElement>('input[aria-label="Chat name"]')!;
    expect(document.activeElement).toBe(input);
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value')!.set!;
    await act(async () => { setter.call(input,'Renamed'); input.dispatchEvent(new Event('input',{bubbles:true})); });
    await act(async () => input.closest('form')!.dispatchEvent(new Event('submit',{bubbles:true,cancelable:true})));
    expect(rename).toHaveBeenCalledWith('Renamed'); expect(prompt).not.toHaveBeenCalled();
    await click((await menu()).querySelectorAll('button')[0]);
    await key(document.querySelector('input')!,'Escape'); expect(document.querySelector('input')).toBeNull();
    expect(rename).toHaveBeenCalledTimes(1); expect(open).not.toHaveBeenCalled();
});
it('confirms deletion from the menu and unpins only from the row control', async () => {
    const confirm = vi.fn(() => false); vi.stubGlobal('confirm',confirm);
    await click((await menu()).querySelectorAll('button')[1]); expect(remove).not.toHaveBeenCalled();
    confirm.mockReturnValue(true); await click((await menu()).querySelectorAll('button')[1]); expect(remove).toHaveBeenCalledOnce();
    await act(async () => root.render(<ChatRow session={{...s,pinned:true}} selected={false} open={open} pin={pin} rename={rename} remove={remove} />));
    expect([...(await menu()).querySelectorAll('button')].map(b => b.textContent)).toEqual(['Rename','Delete']);
    await click(button('Unpin Chat A')); expect(pin).toHaveBeenCalledWith(false); expect(open).not.toHaveBeenCalled();
});
