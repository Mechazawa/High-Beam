import { describe, expect, test } from 'vitest';
import { query } from './plugin.js';

async function collect(iter) {
    const out = [];
    for await (const item of iter) {
        out.push(item);
    }
    return out;
}

describe('web-search plugin — explicit engine prefixes', () => {
    test('google <query> yields a pinned, high-weight Google result', async () => {
        const results = await collect(query('google rust', { aborted: false }));
        expect(results).toHaveLength(1);
        const [r] = results;
        expect(r.title).toBe('Search Google for "rust"');
        expect(r.subtitle).toBe('https://www.google.com/search?q=rust');
        expect(r.weight).toBe(80);
        expect(r.pinned).toBe(true);
        expect(r.action).toEqual({
            kind: 'openUrl',
            url: 'https://www.google.com/search?q=rust',
        });
    });

    test('ddg <query with spaces> URL-encodes the query', async () => {
        const results = await collect(query('ddg query with spaces', { aborted: false }));
        expect(results).toHaveLength(1);
        const [r] = results;
        expect(r.title).toBe('Search DuckDuckGo for "query with spaces"');
        expect(r.subtitle).toBe('https://duckduckgo.com/?q=query%20with%20spaces');
        expect(r.action).toEqual({
            kind: 'openUrl',
            url: 'https://duckduckgo.com/?q=query%20with%20spaces',
        });
    });

    test('bing <query> uses the Bing search URL', async () => {
        const results = await collect(query('bing weather', { aborted: false }));
        expect(results).toHaveLength(1);
        expect(results[0].subtitle).toBe('https://www.bing.com/search?q=weather');
    });

    test('wiki Albert Einstein opens the Wikipedia search', async () => {
        const results = await collect(query('wiki Albert Einstein', { aborted: false }));
        expect(results).toHaveLength(1);
        const [r] = results;
        expect(r.title).toBe('Search Wikipedia for "Albert Einstein"');
        expect(r.subtitle).toBe(
            'https://en.wikipedia.org/wiki/Special:Search?search=Albert%20Einstein',
        );
        expect(r.pinned).toBe(true);
        expect(r.weight).toBe(80);
    });

    test('wikipedia alias works the same as wiki', async () => {
        const results = await collect(query('wikipedia Albert Einstein', { aborted: false }));
        expect(results).toHaveLength(1);
        expect(results[0].subtitle).toBe(
            'https://en.wikipedia.org/wiki/Special:Search?search=Albert%20Einstein',
        );
    });

    test('yt <query> opens YouTube (yt alias)', async () => {
        const results = await collect(query('yt lofi beats', { aborted: false }));
        expect(results).toHaveLength(1);
        expect(results[0].subtitle).toBe(
            'https://www.youtube.com/results?search_query=lofi%20beats',
        );
    });

    test('youtube <query> opens YouTube', async () => {
        const results = await collect(query('youtube lofi beats', { aborted: false }));
        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('Search YouTube for "lofi beats"');
    });

    test('gh user/repo yields a GitHub search URL', async () => {
        const results = await collect(query('gh user/repo', { aborted: false }));
        expect(results).toHaveLength(1);
        const [r] = results;
        expect(r.title).toBe('Search GitHub for "user/repo"');
        expect(r.subtitle).toBe('https://github.com/search?q=user%2Frepo');
        expect(r.pinned).toBe(true);
        expect(r.action).toEqual({
            kind: 'openUrl',
            url: 'https://github.com/search?q=user%2Frepo',
        });
    });

    test('github alias works the same as gh', async () => {
        const results = await collect(query('github user/repo', { aborted: false }));
        expect(results).toHaveLength(1);
        expect(results[0].subtitle).toBe('https://github.com/search?q=user%2Frepo');
    });

    test('so <query> opens Stack Overflow', async () => {
        const results = await collect(query('so segfault rust', { aborted: false }));
        expect(results).toHaveLength(1);
        expect(results[0].subtitle).toBe(
            'https://stackoverflow.com/search?q=segfault%20rust',
        );
    });

    test('stackoverflow alias works the same as so', async () => {
        const results = await collect(query('stackoverflow segfault rust', { aborted: false }));
        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('Search Stack Overflow for "segfault rust"');
    });

    test('prefix matching is case-insensitive', async () => {
        const results = await collect(query('Google rust', { aborted: false }));
        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('Search Google for "rust"');
    });

    test('URL-encodes characters that need escaping (& = +)', async () => {
        const results = await collect(query('google a & b = c', { aborted: false }));
        expect(results).toHaveLength(1);
        expect(results[0].subtitle).toBe(
            'https://www.google.com/search?q=a%20%26%20b%20%3D%20c',
        );
    });
});

describe('web-search plugin — Google fallback', () => {
    test('hello world (no prefix) yields one low-weight, unpinned Google fallback', async () => {
        const results = await collect(query('hello world', { aborted: false }));
        expect(results).toHaveLength(1);
        const [r] = results;
        expect(r.title).toBe('Search Google for "hello world"');
        expect(r.subtitle).toBe('https://www.google.com/search?q=hello%20world');
        expect(r.weight).toBe(5);
        expect(r.pinned).toBe(false);
        expect(r.action).toEqual({
            kind: 'openUrl',
            url: 'https://www.google.com/search?q=hello%20world',
        });
    });

    test('unknown prefix falls through to Google fallback (whole input)', async () => {
        const results = await collect(query('xyz hello', { aborted: false }));
        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('Search Google for "xyz hello"');
        expect(results[0].pinned).toBe(false);
        expect(results[0].weight).toBe(5);
    });

    test('single-word input yields the Google fallback', async () => {
        const results = await collect(query('rust', { aborted: false }));
        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('Search Google for "rust"');
        expect(results[0].weight).toBe(5);
        expect(results[0].pinned).toBe(false);
    });
});

describe('web-search plugin — empty / degenerate inputs', () => {
    test('empty input yields zero results', async () => {
        const results = await collect(query('', { aborted: false }));
        expect(results).toEqual([]);
    });

    test('whitespace-only input yields zero results', async () => {
        const results = await collect(query('   ', { aborted: false }));
        expect(results).toEqual([]);
    });

    test('plain `google` (no query) yields zero results', async () => {
        const results = await collect(query('google', { aborted: false }));
        expect(results).toEqual([]);
    });

    test('engine prefix followed by only whitespace yields zero results', async () => {
        const results = await collect(query('google    ', { aborted: false }));
        expect(results).toEqual([]);
    });
});
