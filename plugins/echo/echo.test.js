import { describe, expect, test } from 'vitest';
import { query } from './plugin.js';

async function collect(iter) {
    const out = [];
    for await (const item of iter) {
        out.push(item);
    }
    return out;
}

describe('echo plugin', () => {
    test('yields one result for non-empty input', async () => {
        const results = await collect(query('hello', { aborted: false }));
        expect(results).toHaveLength(1);
        const [r] = results;
        expect(r.key).toBe('echo');
        expect(r.title).toBe('echo: hello');
        expect(r.subtitle).toBe('press Enter to copy to clipboard');
        expect(r.action).toEqual({ kind: 'copy', text: 'hello' });
    });

    test('yields nothing for empty input', async () => {
        const results = await collect(query('', { aborted: false }));
        expect(results).toEqual([]);
    });
});
