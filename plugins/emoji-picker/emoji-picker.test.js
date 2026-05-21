import { beforeEach, describe, expect, test, vi } from 'vitest';
import * as fs from 'highbeam:fs';
import * as settings from 'highbeam:settings';
import emojiData from './emoji-data.json';

beforeEach(() => {
    vi.mocked(fs.readText).mockReset();
    vi.mocked(fs.readText).mockResolvedValue(JSON.stringify(emojiData));
    vi.mocked(settings.getString).mockReset();
    vi.mocked(settings.getBool).mockReset();
});

async function collect(iter) {
    const out = [];
    for await (const item of iter) {
        out.push(item);
    }
    return out;
}

// `import` is cached across tests, so the in-module `emojiPromise` survives
// between cases — first test triggers the load, the rest hit cache.
const { query } = await import('./plugin.js');

describe('emoji-picker plugin', () => {
    test('default `emoji` trigger surfaces matching emoji for `smile`', async () => {
        const results = await collect(query('emoji smile'));
        expect(results.length).toBeGreaterThan(0);
        expect(results.length).toBeLessThanOrEqual(9);
        // The top hit for `smile` is the alias "smile" (😄), or its near
        // relatives. Either way the title char must be a smiling face.
        const top = results[0];
        expect(top.action.kind).toBe('copy');
        expect(typeof top.action.text).toBe('string');
        // Title format: "<char>  <name>".
        expect(top.title).toMatch(/  /);
        // The matched emoji should have either name, alias, or tag containing
        // "smile".
        const titles = results.map((r) => r.title.toLowerCase());
        expect(titles.some((t) => t.includes('smil') || t.includes('grin') || t.includes('happy'))).toBe(true);
    });

    test('missing trigger keyword yields nothing', async () => {
        const results = await collect(query('smile'));
        expect(results).toEqual([]);
        // Bail-before-load is the whole point of the keyword gate.
        expect(vi.mocked(fs.readText)).not.toHaveBeenCalled();
    });

    test('bare trigger with no query yields nothing', async () => {
        const results = await collect(query('emoji'));
        expect(results).toEqual([]);
    });

    test('trigger plus whitespace yields nothing', async () => {
        const results = await collect(query('emoji   '));
        expect(results).toEqual([]);
    });

    test('exact-name query for `fire` ranks the fire emoji at or near the top', async () => {
        const results = await collect(query('emoji fire'));
        expect(results.length).toBeGreaterThan(0);
        const chars = results.map((r) => r.action.text);
        expect(chars).toContain('🔥');
    });

    test('alias match: `rofl` finds the rolling-on-floor emoji', async () => {
        const results = await collect(query('emoji rofl'));
        expect(results.length).toBeGreaterThan(0);
        const chars = results.map((r) => r.action.text);
        expect(chars).toContain('🤣');
    });

    test('Enter action copies just the emoji character', async () => {
        const results = await collect(query('emoji fire'));
        const top = results.find((r) => r.action.text === '🔥');
        expect(top).toBeDefined();
        expect(top.action).toEqual({ kind: 'copy', text: '🔥' });
    });

    test('every result has a numeric weight in 0..100 and a stable key prefix', async () => {
        const results = await collect(query('emoji smile'));
        for (const r of results) {
            expect(typeof r.weight).toBe('number');
            expect(r.weight).toBeGreaterThanOrEqual(0);
            expect(r.weight).toBeLessThanOrEqual(100);
            expect(r.key.startsWith('emoji:')).toBe(true);
        }
    });

    test('nonsense query falls below threshold and yields nothing', async () => {
        const results = await collect(query('emoji xyznonexistentqqq'));
        expect(results).toEqual([]);
    });

    test('caps results at 9 even for very generic queries', async () => {
        const results = await collect(query('emoji e'));
        expect(results.length).toBeLessThanOrEqual(9);
    });

    test('custom trigger from settings replaces the default', async () => {
        vi.mocked(settings.getString).mockImplementation((key) => key === 'trigger' ? 'e:' : undefined);
        const results = await collect(query('e: fire'));
        expect(results.length).toBeGreaterThan(0);
        const chars = results.map((r) => r.action.text);
        expect(chars).toContain('🔥');
        // The default `emoji` keyword must NOT trigger when the override is set.
        const defaultResults = await collect(query('emoji fire'));
        expect(defaultResults).toEqual([]);
    });

    test('blank custom trigger falls back to the default', async () => {
        vi.mocked(settings.getString).mockImplementation((key) => key === 'trigger' ? '   ' : undefined);
        const results = await collect(query('emoji fire'));
        expect(results.length).toBeGreaterThan(0);
    });

    test('skin-tones toggle off: no skin variants in results', async () => {
        vi.mocked(settings.getBool).mockReturnValue(false);
        const results = await collect(query('emoji wave'));
        expect(results.length).toBeGreaterThan(0);
        // Modifier code points (U+1F3FB..U+1F3FF) must not appear in any
        // emitted char when the toggle is off.
        for (const r of results) {
            expect(/[\u{1F3FB}-\u{1F3FF}]/u.test(r.action.text)).toBe(false);
        }
    });

    test('skin-tones toggle on: variants are emitted for emoji that support them', async () => {
        vi.mocked(settings.getBool).mockImplementation((key) => key === 'skin_tones');
        const results = await collect(query('emoji wave'));
        // Among the top results we should see at least one variant carrying a
        // skin-tone modifier code point.
        const hasVariant = results.some((r) => /[\u{1F3FB}-\u{1F3FF}]/u.test(r.action.text));
        expect(hasVariant).toBe(true);
        // And the variant titles should indicate which tone.
        const variantTitle = results.find((r) => /skin\)$/.test(r.title));
        expect(variantTitle).toBeDefined();
    });
});
