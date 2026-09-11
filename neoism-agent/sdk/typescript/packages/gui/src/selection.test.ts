import { describe, expect, it } from 'vitest';
import type { ProviderListResult, Session } from '@neoism/sdk';
import { configuredModel, resolveSelection } from './selection';
const catalog = (ids: string[], connected: string[], defaults: Record<string, string>) => ({ all: ids.map(id => ({ id })), connected, default: defaults }) as ProviderListResult;
describe('canonical api_mapping.rs provider fallback', () => {
    it('prefers connected non-opencode providers preserving source order', () => {
        expect(configuredModel(catalog(['opencode', 'z', 'a'], ['opencode', 'z', 'a'], { opencode: 'free', z: 'z-model', a: 'a-model' }))).toBe('z/z-model');
    });
    it('filters disconnected providers only when the connected list is nonempty', () => {
        expect(configuredModel(catalog(['a', 'opencode'], ['opencode'], { a: 'model', opencode: 'free' }))).toBe('opencode/free');
        expect(configuredModel(catalog(['a', 'opencode'], [], { a: 'model', opencode: 'free' }))).toBe('a/model');
    });
    it('does not invent a model from a provider first model or sort providers', () => {
        expect(configuredModel(catalog(['a', 'opencode'], ['a'], { a: '  ', opencode: 'free' }))).toBe('');
        expect(configuredModel()).toBe('');
    });
});
describe('selection layers', () => {
    it('preserves explicit empty effort and account alongside session model/agent', () => {
        const metadata = { model: { providerId: 'p', id: 'm', variant: 'high', connectionId: 'c' }, agent: 'plan' } as Session;
        expect(resolveSelection({ thinking: '', connectionId: '' }, metadata, { defaultAgent: 'build', model: 'd/m', variant: 'medium' })).toEqual({ model: 'p/m', agent: 'plan', thinking: '', connectionId: '' });
    });
    it('does not inherit configured effort into session metadata with no variant', () => {
        expect(resolveSelection({}, { model: { providerId: 'p', id: 'm' } } as Session, { defaultAgent: null, model: 'd/m', variant: 'high' }).thinking).toBe('');
    });
});
