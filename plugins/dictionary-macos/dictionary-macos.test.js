import { describe, expect, test } from 'vitest';
import { query } from './plugin.js';

async function collect(iter) {
    const out = [];
    for await (const item of iter) {
        out.push(item);
    }
    return out;
}

describe('dictionary-macos plugin', () => {
    test('`define rust` yields a pinned result that opens dict://rust', async () => {
        const results = await collect(query('define rust', { aborted: false }));
        expect(results).toHaveLength(1);
        const [r] = results;
        expect(r.title).toBe('Define "rust"');
        expect(r.subtitle).toBe('Open in Dictionary.app');
        expect(r.weight).toBe(80);
        expect(r.pinned).toBe(true);
        expect(r.action).toEqual({ kind: 'openUrl', url: 'dict://rust' });
    });

    test('`dict serendipity` yields the same shape with the right URL', async () => {
        const results = await collect(
            query('dict serendipity', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].action).toEqual({
            kind: 'openUrl',
            url: 'dict://serendipity',
        });
        expect(results[0].pinned).toBe(true);
    });

    test('non-trigger query yields zero results', async () => {
        const results = await collect(query('hello', { aborted: false }));
        expect(results).toEqual([]);
    });

    test('empty word after `define ` yields zero results', async () => {
        const results = await collect(query('define ', { aborted: false }));
        expect(results).toEqual([]);
    });

    test('multi-word phrase is percent-encoded in the dict:// URL', async () => {
        // Multi-word handling decision: encodeURIComponent. `dict://` URL
        // handlers vary in how leniently they accept raw whitespace; emitting
        // a strictly-valid URL keeps every plausible consumer (LaunchServices,
        // an explicit `open` call, future logging) happy. Dictionary.app
        // decodes the percent-escaped form before the lookup.
        const results = await collect(
            query('define multi word', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('Define "multi word"');
        expect(results[0].action).toEqual({
            kind: 'openUrl',
            url: 'dict://multi%20word',
        });
    });
});
