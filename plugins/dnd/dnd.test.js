import { describe, expect, test, vi } from 'vitest';
import { readText } from 'highbeam:fs';
import spells from './5eSpells.json';

vi.mocked(readText).mockResolvedValue(JSON.stringify(spells));

const { query } = await import('./plugin.js');

async function collect(iter) {
    const out = [];
    for await (const item of iter) {
        out.push(item);
    }
    return out;
}

describe('dnd plugin', () => {
    test('exact-name query returns the matching spell on top', async () => {
        const results = await collect(query('spell fireball'));
        expect(results.length).toBeGreaterThan(0);
        const top = results[0];
        expect(top.title).toBe('Fireball');
        expect(top.action).toEqual({
            kind: 'openUrl',
            url: 'https://www.dndbeyond.com/spells/fireball',
        });
    });

    test('5e prefix triggers the same matcher', async () => {
        const results = await collect(query('5e spell fire'));
        expect(results.length).toBeGreaterThan(1);
        const titles = results.map((r) => r.title);
        expect(titles.some((t) => /fire/i.test(t))).toBe(true);
    });

    test('missing keyword yields nothing', async () => {
        const results = await collect(query('fireball'));
        expect(results).toEqual([]);
    });

    test('keyword with no query yields nothing', async () => {
        const results = await collect(query('spell '));
        expect(results).toEqual([]);
    });

    test('nonsense query falls below threshold', async () => {
        const results = await collect(query('spell xyznonexistentqqq'));
        expect(results).toEqual([]);
    });

    test('caps results at 10', async () => {
        const results = await collect(query('spell e'));
        expect(results.length).toBeLessThanOrEqual(10);
    });

    test('each result carries a numeric weight and a subtitle', async () => {
        const results = await collect(query('spell fire'));
        for (const r of results) {
            expect(typeof r.weight).toBe('number');
            expect(typeof r.subtitle).toBe('string');
            expect(r.subtitle.length).toBeGreaterThan(0);
        }
    });
});
