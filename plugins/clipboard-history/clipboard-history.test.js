import { beforeEach, describe, expect, test, vi } from 'vitest';

async function collect(iter) {
    const out = [];
    for await (const item of iter) out.push(item);
    return out;
}

// Fresh plugin instance + reset SDK stubs per test. The plugin keeps an
// in-module history cache so we always re-import via vi.resetModules().
async function loadPlugin({
    clipboard = '',
    cache = null,
    keyword,
    maxHistory,
    maxEntryBytes,
} = {}) {
    vi.resetModules();
    const clip = await import('highbeam:clipboard');
    const fs = await import('highbeam:fs');
    const settings = await import('highbeam:settings');

    vi.mocked(clip.read).mockReset();
    vi.mocked(fs.readCache).mockReset();
    vi.mocked(fs.writeCache).mockReset();
    vi.mocked(settings.getString).mockReset();
    vi.mocked(settings.getInt).mockReset();

    vi.mocked(clip.read).mockResolvedValue(clipboard);
    vi.mocked(fs.readCache).mockResolvedValue(
        cache === null ? null : new TextEncoder().encode(JSON.stringify(cache)),
    );
    vi.mocked(fs.writeCache).mockResolvedValue(undefined);
    vi.mocked(settings.getString).mockImplementation((key) => {
        if (key === 'keyword') return keyword;
        return undefined;
    });
    vi.mocked(settings.getInt).mockImplementation((key) => {
        if (key === 'max_history') return maxHistory;
        if (key === 'max_entry_bytes') return maxEntryBytes;
        return undefined;
    });

    const plugin = await import('./plugin.js');
    return { plugin, clip, fs, settings };
}

function lastWrittenHistory(fs) {
    const calls = vi.mocked(fs.writeCache).mock.calls;
    expect(calls.length).toBeGreaterThan(0);
    const [name, data] = calls[calls.length - 1];
    expect(name).toBe('history.json');
    return JSON.parse(data);
}

beforeEach(() => {
    // Per-test reset is handled by loadPlugin() — nothing global to clean up.
});

describe('clipboard-history plugin', () => {
    test('empty clipboard is not captured', async () => {
        const { plugin, fs } = await loadPlugin({ clipboard: '' });
        await collect(plugin.query('something else', { aborted: false }));
        expect(vi.mocked(fs.writeCache)).not.toHaveBeenCalled();
    });

    test('whitespace-only clipboard is not captured', async () => {
        const { plugin, fs } = await loadPlugin({ clipboard: '   \n\t  ' });
        await collect(plugin.query('clip', { aborted: false }));
        expect(vi.mocked(fs.writeCache)).not.toHaveBeenCalled();
    });

    test('first capture saves the entry and surfaces it on `clip`', async () => {
        const { plugin, fs } = await loadPlugin({ clipboard: 'hello world' });
        const results = await collect(plugin.query('clip', { aborted: false }));
        const written = lastWrittenHistory(fs);
        expect(written).toHaveLength(1);
        expect(written[0].text).toBe('hello world');
        expect(typeof written[0].copiedAt).toBe('number');

        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('hello world');
        expect(results[0].action).toEqual({ kind: 'copy', text: 'hello world' });
        expect(results[0].subtitle).toMatch(/ago|just now|yesterday/);
    });

    test('duplicate of last entry does not grow history', async () => {
        const existing = [
            { text: 'already here', copiedAt: Date.now() - 1000 },
        ];
        const { plugin, fs } = await loadPlugin({
            clipboard: 'already here',
            cache: existing,
        });
        await collect(plugin.query('clip', { aborted: false }));
        expect(vi.mocked(fs.writeCache)).not.toHaveBeenCalled();
    });

    test('new entry is prepended to existing history', async () => {
        const existing = [
            { text: 'old one', copiedAt: Date.now() - 5000 },
        ];
        const { plugin, fs } = await loadPlugin({
            clipboard: 'fresh paste',
            cache: existing,
        });
        await collect(plugin.query('clip', { aborted: false }));
        const written = lastWrittenHistory(fs);
        expect(written).toHaveLength(2);
        expect(written[0].text).toBe('fresh paste');
        expect(written[1].text).toBe('old one');
    });

    test('cap at max history drops oldest', async () => {
        const now = Date.now();
        // 3 existing entries, max set to 3 — the new capture should push the
        // oldest out.
        const existing = [
            { text: 'newest existing', copiedAt: now - 1000 },
            { text: 'middle existing', copiedAt: now - 2000 },
            { text: 'oldest existing', copiedAt: now - 3000 },
        ];
        const { plugin, fs } = await loadPlugin({
            clipboard: 'brand new',
            cache: existing,
            maxHistory: 3,
        });
        await collect(plugin.query('clip', { aborted: false }));
        const written = lastWrittenHistory(fs);
        expect(written).toHaveLength(3);
        expect(written.map((e) => e.text)).toEqual([
            'brand new',
            'newest existing',
            'middle existing',
        ]);
    });

    test('fuzzy match returns relevant hits', async () => {
        const now = Date.now();
        const existing = [
            { text: 'meeting notes for the Q2 plan', copiedAt: now - 1000 },
            { text: 'random unrelated text', copiedAt: now - 2000 },
            { text: 'another meeting summary', copiedAt: now - 3000 },
            { text: 'totally off-topic', copiedAt: now - 4000 },
        ];
        const { plugin } = await loadPlugin({
            clipboard: '', // skip capture; we only care about query matching
            cache: existing,
        });
        const results = await collect(
            plugin.query('clip meeting', { aborted: false }),
        );
        expect(results.length).toBeGreaterThan(0);
        // Both meeting rows should appear; off-topic rows should not.
        const titles = results.map((r) => r.title);
        expect(titles.some((t) => t.includes('meeting notes'))).toBe(true);
        expect(titles.some((t) => t.includes('meeting summary'))).toBe(true);
        expect(titles.some((t) => t.includes('totally off-topic'))).toBe(false);
    });

    test('size cap excludes huge clipboard text', async () => {
        const huge = 'x'.repeat(50_000); // 50KB ASCII => 50KB UTF-8.
        const { plugin, fs } = await loadPlugin({
            clipboard: huge,
            maxEntryBytes: 10_000,
        });
        await collect(plugin.query('clip', { aborted: false }));
        expect(vi.mocked(fs.writeCache)).not.toHaveBeenCalled();
    });

    test('non-trigger input still captures but yields nothing', async () => {
        const { plugin, fs } = await loadPlugin({ clipboard: 'snapshot' });
        const results = await collect(
            plugin.query('hello there', { aborted: false }),
        );
        expect(results).toEqual([]);
        const written = lastWrittenHistory(fs);
        expect(written).toHaveLength(1);
        expect(written[0].text).toBe('snapshot');
    });

    test('configured keyword overrides the default', async () => {
        const existing = [
            { text: 'entry one', copiedAt: Date.now() - 1000 },
        ];
        const { plugin } = await loadPlugin({
            clipboard: '',
            cache: existing,
            keyword: 'paste',
        });
        const fromOldKeyword = await collect(
            plugin.query('clip', { aborted: false }),
        );
        const fromNewKeyword = await collect(
            plugin.query('paste', { aborted: false }),
        );
        expect(fromOldKeyword).toEqual([]);
        expect(fromNewKeyword).toHaveLength(1);
    });

    test('clipboard and history aliases trigger regardless of keyword', async () => {
        const existing = [
            { text: 'entry one', copiedAt: Date.now() - 1000 },
        ];
        const { plugin } = await loadPlugin({
            clipboard: '',
            cache: existing,
            keyword: 'paste',
        });
        const a = await collect(plugin.query('clipboard', { aborted: false }));
        const b = await collect(plugin.query('history', { aborted: false }));
        expect(a).toHaveLength(1);
        expect(b).toHaveLength(1);
    });

    test('corrupt cache falls back to empty history', async () => {
        vi.resetModules();
        const clip = await import('highbeam:clipboard');
        const fs = await import('highbeam:fs');
        const settings = await import('highbeam:settings');
        vi.mocked(clip.read).mockReset();
        vi.mocked(fs.readCache).mockReset();
        vi.mocked(fs.writeCache).mockReset();
        vi.mocked(settings.getString).mockReset();
        vi.mocked(settings.getInt).mockReset();
        vi.mocked(clip.read).mockResolvedValue('');
        // Corrupt JSON shouldn't crash the plugin — it should treat the file
        // as empty and move on.
        vi.mocked(fs.readCache).mockResolvedValue(
            new TextEncoder().encode('{not valid json'),
        );
        vi.mocked(fs.writeCache).mockResolvedValue(undefined);
        vi.mocked(settings.getString).mockReturnValue(undefined);
        vi.mocked(settings.getInt).mockReturnValue(undefined);

        const plugin = await import('./plugin.js');
        const results = await collect(plugin.query('clip', { aborted: false }));
        expect(results).toEqual([]);
    });

    test('subsequent queries reuse the in-memory history without re-reading cache', async () => {
        const existing = [
            { text: 'cached entry', copiedAt: Date.now() - 1000 },
        ];
        const { plugin, fs } = await loadPlugin({
            clipboard: '',
            cache: existing,
        });
        await collect(plugin.query('clip', { aborted: false }));
        await collect(plugin.query('clip', { aborted: false }));
        await collect(plugin.query('clip', { aborted: false }));
        expect(vi.mocked(fs.readCache)).toHaveBeenCalledTimes(1);
    });
});
