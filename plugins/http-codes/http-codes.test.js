import { beforeEach, describe, expect, test, vi } from 'vitest';
import * as fs from 'highbeam:fs';
import { query } from './plugin.js';

// Small fixture spanning 2xx/4xx/5xx so prefix-filter assertions exercise
// more than one code-class without bundling the full 70-entry table.
const FIXTURE = [
    { key: 200, title: 'OK', description: 'Standard success.' },
    { key: 201, title: 'Created', description: 'Resource created.' },
    { key: 404, title: 'Not Found', description: 'Resource missing.' },
    { key: 500, title: 'Internal Server Error', description: 'Server fault.' },
];

beforeEach(() => {
    // `mockReset` wipes call history per test — the plugin's in-module
    // cache survives between tests (real-world behaviour), so the first
    // test triggers the single `readText` call and the rest hit cache.
    vi.mocked(fs.readText).mockReset();
    vi.mocked(fs.readText).mockResolvedValue(JSON.stringify(FIXTURE));
});

async function collect(iter) {
    const out = [];
    for await (const item of iter) {
        out.push(item);
    }
    return out;
}

describe('http-codes plugin', () => {
    test('bare "http" prefix matches all codes in the fixture', async () => {
        const results = await collect(query('http', { aborted: false }));
        expect(results).toHaveLength(FIXTURE.length);
        expect(results.map((r) => r.key)).toEqual(['200', '201', '404', '500']);
    });

    test('"http 4" filters to codes starting with 4', async () => {
        const results = await collect(query('http 4', { aborted: false }));
        expect(results).toHaveLength(1);
        expect(results[0].key).toBe('404');
    });

    test('"http 404" yields the exact match with MDN URL action', async () => {
        const results = await collect(query('http 404', { aborted: false }));
        expect(results).toHaveLength(1);
        const [r] = results;
        expect(r.title).toBe('404 - Not Found');
        expect(r.subtitle).toBe('Resource missing.');
        expect(r.action).toEqual({
            kind: 'openUrl',
            url: 'https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/404',
        });
        // Three-digit prefix saturates the weight at the 100 ceiling.
        expect(r.weight).toBe(100);
    });

    test('non-matching input yields zero results and skips the data load', async () => {
        const results = await collect(
            query('something else', { aborted: false }),
        );
        expect(results).toEqual([]);
        // The regex bails before `readText` is touched — important for
        // keeping the per-keystroke cost at zero when the plugin isn't
        // the one the user is reaching for.
        expect(vi.mocked(fs.readText)).not.toHaveBeenCalled();
    });

    test('every result is pinned and opens the matching MDN status page', async () => {
        const results = await collect(query('http', { aborted: false }));
        expect(results.length).toBeGreaterThan(0);
        for (const r of results) {
            expect(r.pinned).toBe(true);
            expect(r.action.kind).toBe('openUrl');
            expect(r.action.url).toBe(
                `https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/${r.key}`,
            );
        }
    });
});
