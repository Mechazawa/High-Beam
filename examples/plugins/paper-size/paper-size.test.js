import { describe, expect, test } from 'vitest';
import { query } from './plugin.js';

async function collect(iter) {
    const out = [];
    for await (const item of iter) {
        out.push(item);
    }
    return out;
}

describe('paper-size plugin', () => {
    test('matches A4 exactly', async () => {
        const results = await collect(query('A4', { aborted: false }));
        expect(results).toHaveLength(1);
        const [r] = results;
        expect(r.title).toBe('A4');
        expect(r.subtitle).toBe('210 x 297 mm');
        expect(r.action).toEqual({ kind: 'copy', text: '210 x 297' });
    });

    test('is case-insensitive', async () => {
        const lower = await collect(query('a4', { aborted: false }));
        const upper = await collect(query('A4', { aborted: false }));
        expect(lower).toEqual(upper);
        expect(lower[0].title).toBe('A4');
    });

    test('matches Letter', async () => {
        const results = await collect(query('letter', { aborted: false }));
        // `letter` is a substring of both "Letter" and "Government-Letter".
        const letter = results.find((r) => r.title === 'Letter');
        expect(letter).toBeDefined();
        expect(letter.subtitle).toBe('215.9 x 279.4 mm');
        expect(letter.action).toEqual({ kind: 'copy', text: '215.9 x 279.4' });
    });

    test('matches multiple results for "a"', async () => {
        const results = await collect(query('a', { aborted: false }));
        const titles = results.map((r) => r.title);
        expect(titles).toEqual(expect.arrayContaining(['A0', 'A1', 'A2', 'A4']));
        for (const r of results) {
            expect(r.title.toLowerCase()).toContain('a');
        }
        expect(results.length).toBeGreaterThan(5);
    });

    test('empty input yields nothing', async () => {
        const results = await collect(query('', { aborted: false }));
        expect(results).toEqual([]);
    });

    test('nonsense input yields nothing', async () => {
        const results = await collect(query('nonsense', { aborted: false }));
        expect(results).toEqual([]);
    });

    test('`paper <name>` prefix behaves like bare name', async () => {
        const bare = await collect(query('A4', { aborted: false }));
        const prefixed = await collect(query('paper A4', { aborted: false }));
        expect(prefixed).toEqual(bare);
    });
});
