import { beforeEach, describe, expect, test, vi } from 'vitest';

const VAULT = '/Users/me/vault';

// Mirror the shape `fs.readDir` yields: { name, path, isFile, isDir }. Keys
// are absolute paths so the BFS walker can recurse using `entry.path`.
function dirEntry(path, name, { isDir = false, isFile = !isDir } = {}) {
    return { name, path, isFile, isDir };
}

const VAULT_TREE = {
    [VAULT]: [
        dirEntry(`${VAULT}/Welcome.md`, 'Welcome.md'),
        dirEntry(`${VAULT}/Projects`, 'Projects', { isDir: true }),
        dirEntry(`${VAULT}/Daily`, 'Daily', { isDir: true }),
        dirEntry(`${VAULT}/.obsidian`, '.obsidian', { isDir: true }),
        dirEntry(`${VAULT}/.trash`, '.trash', { isDir: true }),
        dirEntry(`${VAULT}/_archive`, '_archive', { isDir: true }),
        dirEntry(`${VAULT}/README.txt`, 'README.txt'),
    ],
    [`${VAULT}/Projects`]: [
        dirEntry(`${VAULT}/Projects/2026`, '2026', { isDir: true }),
        dirEntry(`${VAULT}/Projects/Roadmap.md`, 'Roadmap.md'),
    ],
    [`${VAULT}/Projects/2026`]: [
        dirEntry(`${VAULT}/Projects/2026/Fireball Spec.md`, 'Fireball Spec.md'),
        dirEntry(`${VAULT}/Projects/2026/Notes.md`, 'Notes.md'),
    ],
    [`${VAULT}/Daily`]: [
        dirEntry(`${VAULT}/Daily/2026-05-21.md`, '2026-05-21.md'),
    ],
    // Walker MUST skip these — fixtures intentionally contain markdown that
    // should never surface in results.
    [`${VAULT}/.obsidian`]: [
        dirEntry(`${VAULT}/.obsidian/workspace.md`, 'workspace.md'),
    ],
    [`${VAULT}/.trash`]: [
        dirEntry(`${VAULT}/.trash/oldnote.md`, 'oldnote.md'),
    ],
    [`${VAULT}/_archive`]: [
        dirEntry(`${VAULT}/_archive/Fireball Old.md`, 'Fireball Old.md'),
    ],
};

function asyncIterable(items) {
    return {
        async *[Symbol.asyncIterator]() {
            for (const item of items) yield item;
        },
    };
}

async function collect(iter) {
    const out = [];
    for await (const item of iter) out.push(item);
    return out;
}

async function loadPlugin({
    vaultPath = VAULT,
    vaultName,
    keyword,
    alwaysOn,
    cacheSeconds,
} = {}) {
    vi.resetModules();
    const fs = await import('highbeam:fs');
    const settings = await import('highbeam:settings');
    vi.mocked(fs.readDir).mockImplementation((path) =>
        asyncIterable(VAULT_TREE[path] ?? []),
    );
    // Cold cache — every test starts from a fresh walk.
    vi.mocked(fs.readCache).mockResolvedValue(null);
    vi.mocked(fs.writeCache).mockResolvedValue(undefined);
    vi.mocked(settings.getString).mockImplementation((key) => {
        if (key === 'vault_path') return vaultPath;
        if (key === 'vault_name') return vaultName ?? '';
        if (key === 'keyword') return keyword ?? '';
        return undefined;
    });
    vi.mocked(settings.getBool).mockImplementation((key) => {
        if (key === 'always_on') return alwaysOn === true;
        return undefined;
    });
    vi.mocked(settings.getInt).mockImplementation((key) => {
        if (key === 'cache_seconds') return cacheSeconds;
        return undefined;
    });
    const plugin = await import('./plugin.js');
    return { plugin, fs, settings };
}

describe('obsidian: keyword gating', () => {
    test('input without the keyword yields nothing', async () => {
        const { plugin, fs } = await loadPlugin();
        const results = await collect(plugin.query('fireball', { aborted: false }));
        expect(results).toEqual([]);
        // Bail-before-walk is the whole point of the keyword gate.
        expect(fs.readDir).not.toHaveBeenCalled();
    });

    test('default `obs` keyword unlocks the search', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(plugin.query('obs fireball', { aborted: false }));
        expect(results.length).toBeGreaterThan(0);
        expect(results[0].title).toBe('Fireball Spec');
    });

    test('custom keyword from settings replaces the default', async () => {
        const { plugin } = await loadPlugin({ keyword: 'note' });
        const customResults = await collect(plugin.query('note fireball', { aborted: false }));
        expect(customResults.length).toBeGreaterThan(0);
        const defaultResults = await collect(plugin.query('obs fireball', { aborted: false }));
        expect(defaultResults).toEqual([]);
    });

    test('keyword without trailing whitespace is not treated as a trigger', async () => {
        const { plugin } = await loadPlugin();
        // `obsfoo` matches the prefix but lacks a word boundary — caller
        // shouldn't get note results for unrelated text starting with "obs".
        const results = await collect(plugin.query('obsfoo', { aborted: false }));
        expect(results).toEqual([]);
    });

    test('bare keyword with no query yields nothing', async () => {
        const { plugin } = await loadPlugin();
        expect(await collect(plugin.query('obs', { aborted: false }))).toEqual([]);
        expect(await collect(plugin.query('obs   ', { aborted: false }))).toEqual([]);
    });

    test('always_on bypasses the keyword and searches every query', async () => {
        const { plugin } = await loadPlugin({ alwaysOn: true });
        const results = await collect(plugin.query('fireball', { aborted: false }));
        expect(results.length).toBeGreaterThan(0);
        expect(results[0].title).toBe('Fireball Spec');
    });
});

describe('obsidian: fuzzy matching + walk', () => {
    test('matches notes by filename and ranks the best hit first', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(plugin.query('obs fireball', { aborted: false }));
        const titles = results.map((r) => r.title);
        expect(titles[0]).toBe('Fireball Spec');
    });

    test('top-level note carries `/` as its subtitle', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(plugin.query('obs welcome', { aborted: false }));
        const welcome = results.find((r) => r.title === 'Welcome');
        expect(welcome).toBeDefined();
        expect(welcome.subtitle).toBe('/');
    });

    test('nested note carries its folder path as subtitle', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(plugin.query('obs fireball', { aborted: false }));
        const note = results.find((r) => r.title === 'Fireball Spec');
        expect(note.subtitle).toBe('Projects/2026/');
    });

    test('walker skips `.obsidian`, dotted dirs, and `_archive`', async () => {
        const { plugin } = await loadPlugin();
        // Each of those fixture dirs contains a `.md` file whose name would
        // otherwise match — none of them should appear.
        const all = await collect(plugin.query('obs ', { aborted: false }));
        expect(all).toEqual([]); // empty query after trim
        const fireResults = await collect(plugin.query('obs fireball', { aborted: false }));
        const titles = fireResults.map((r) => r.title);
        expect(titles).not.toContain('Fireball Old');
        const workspaceResults = await collect(plugin.query('obs workspace', { aborted: false }));
        expect(workspaceResults).toEqual([]);
        const trashResults = await collect(plugin.query('obs oldnote', { aborted: false }));
        expect(trashResults).toEqual([]);
    });

    test('non-md files (README.txt) are not surfaced', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(plugin.query('obs readme', { aborted: false }));
        expect(results).toEqual([]);
    });

    test('caps results at 9', async () => {
        const { plugin } = await loadPlugin();
        // Single-char query matches almost everything — count must clamp.
        const results = await collect(plugin.query('obs e', { aborted: false }));
        expect(results.length).toBeLessThanOrEqual(9);
    });

    test('every result has a numeric weight in 0..100 and a stable key', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(plugin.query('obs note', { aborted: false }));
        for (const r of results) {
            expect(typeof r.weight).toBe('number');
            expect(r.weight).toBeGreaterThanOrEqual(0);
            expect(r.weight).toBeLessThanOrEqual(100);
            expect(r.key.startsWith('obsidian:')).toBe(true);
        }
    });
});

describe('obsidian: open URL', () => {
    test('action is an obsidian:// URL with vault name + file (no .md)', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(plugin.query('obs fireball', { aborted: false }));
        const note = results.find((r) => r.title === 'Fireball Spec');
        expect(note.action.kind).toBe('openUrl');
        const url = new URL(note.action.url);
        expect(url.protocol).toBe('obsidian:');
        expect(url.hostname || url.pathname.replace(/^\/\//, '')).toMatch(/open/);
        // The query string format ordering isn't guaranteed; check via the
        // parsed params instead of substring comparisons.
        const params = url.searchParams;
        expect(params.get('vault')).toBe('vault'); // basename of /Users/me/vault
        expect(params.get('file')).toBe('Projects/2026/Fireball Spec');
        expect(params.has('path')).toBe(false);
    });

    test('custom vault_name from settings wins over the basename', async () => {
        const { plugin } = await loadPlugin({ vaultName: 'My Brain' });
        const results = await collect(plugin.query('obs welcome', { aborted: false }));
        const note = results.find((r) => r.title === 'Welcome');
        const url = new URL(note.action.url);
        expect(url.searchParams.get('vault')).toBe('My Brain');
        expect(url.searchParams.get('file')).toBe('Welcome');
    });

    test('top-level note URL has no folder prefix', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(plugin.query('obs welcome', { aborted: false }));
        const note = results.find((r) => r.title === 'Welcome');
        const url = new URL(note.action.url);
        expect(url.searchParams.get('file')).toBe('Welcome');
    });
});

describe('obsidian: missing vault_path', () => {
    test('empty vault_path yields a single hint row regardless of keyword', async () => {
        const { plugin, fs } = await loadPlugin({ vaultPath: '' });
        const results = await collect(plugin.query('obs anything', { aborted: false }));
        expect(results).toHaveLength(1);
        const [row] = results;
        expect(row.title).toMatch(/vault path/i);
        expect(row.pinned).toBe(true);
        expect(row.action).toEqual({ kind: 'noop' });
        // We never tried to walk the filesystem.
        expect(fs.readDir).not.toHaveBeenCalled();
    });

    test('whitespace-only vault_path is treated as empty', async () => {
        const { plugin } = await loadPlugin({ vaultPath: '   ' });
        const results = await collect(plugin.query('obs foo', { aborted: false }));
        expect(results).toHaveLength(1);
        expect(results[0].key).toBe('obsidian:missing-vault-path');
    });
});

describe('obsidian: caching', () => {
    test('repeat queries within the TTL window only walk the vault once', async () => {
        const { plugin, fs } = await loadPlugin({ cacheSeconds: 60 });
        await collect(plugin.query('obs fireball', { aborted: false }));
        const callsAfterFirst = fs.readDir.mock.calls.length;
        expect(callsAfterFirst).toBeGreaterThan(0);
        await collect(plugin.query('obs notes', { aborted: false }));
        await collect(plugin.query('obs welcome', { aborted: false }));
        // Memory cache served the second + third queries — no extra walks.
        expect(fs.readDir.mock.calls.length).toBe(callsAfterFirst);
    });

    test('cache_seconds=0 forces a fresh walk on every query', async () => {
        const { plugin, fs } = await loadPlugin({ cacheSeconds: 0 });
        await collect(plugin.query('obs fireball', { aborted: false }));
        const callsAfterFirst = fs.readDir.mock.calls.length;
        await collect(plugin.query('obs notes', { aborted: false }));
        expect(fs.readDir.mock.calls.length).toBeGreaterThan(callsAfterFirst);
    });
});

describe('obsidian: helpers', () => {
    test('parseTrigger respects the keyword + word boundary', async () => {
        const { plugin } = await loadPlugin();
        const { parseTrigger } = plugin.__test__;
        expect(parseTrigger('obs foo', 'obs')).toBe('foo');
        expect(parseTrigger('OBS foo', 'obs')).toBe('foo');
        expect(parseTrigger('obsfoo', 'obs')).toBeNull();
        expect(parseTrigger('  obs   foo   ', 'obs')).toBe('foo');
        expect(parseTrigger('obs', 'obs')).toBe('');
    });

    test('folderOf returns `/` for top-level and `folder/` otherwise', async () => {
        const { plugin } = await loadPlugin();
        const { folderOf } = plugin.__test__;
        expect(folderOf('Welcome.md')).toBe('/');
        expect(folderOf('Projects/2026/Notes.md')).toBe('Projects/2026/');
    });

    test('buildObsidianUrl uses vault name when present, path when not', async () => {
        const { plugin } = await loadPlugin();
        const { buildObsidianUrl } = plugin.__test__;
        const namedUrl = new URL(
            buildObsidianUrl('Brain', '/Users/me/Brain', 'Notes/A.md'),
        );
        expect(namedUrl.searchParams.get('vault')).toBe('Brain');
        expect(namedUrl.searchParams.get('file')).toBe('Notes/A');

        const pathUrl = new URL(
            buildObsidianUrl('', '/Users/me/Brain', 'Notes/A.md'),
        );
        expect(pathUrl.searchParams.get('path')).toBe('/Users/me/Brain');
        expect(pathUrl.searchParams.has('vault')).toBe(false);
    });

    test('shouldSkipDir skips `.obsidian`, dotted dirs, and `_archive`', async () => {
        const { plugin } = await loadPlugin();
        const { shouldSkipDir } = plugin.__test__;
        expect(shouldSkipDir('.obsidian')).toBe(true);
        expect(shouldSkipDir('.git')).toBe(true);
        expect(shouldSkipDir('_archive')).toBe(true);
        expect(shouldSkipDir('Projects')).toBe(false);
        expect(shouldSkipDir('Daily Notes')).toBe(false);
    });
});
