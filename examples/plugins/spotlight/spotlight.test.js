import { describe, expect, test, vi } from 'vitest';

// `platform` is a near-real stub (no vi.fn() exports), and ESM namespaces are
// frozen — so spyOn won't bite. Replace the whole module with vi.fn()s.
vi.mock('highbeam:platform', () => ({
    isMacOS: vi.fn(() => true),
    isLinux: vi.fn(() => false),
    os: 'macos',
    arch: 'x86_64',
    version: 'test',
}));

const ICON_SENTINEL = 'data:image/png;base64,SENTINEL';

function dirEntry(path, name) {
    return { name, path, isFile: false, isDir: true };
}

const APPS_FIXTURES = {
    '/Applications': [
        dirEntry('/Applications/Safari.app', 'Safari.app'),
        dirEntry('/Applications/Calculator.app', 'Calculator.app'),
        dirEntry('/Applications/README.txt', 'README.txt'),
    ],
    '/System/Applications': [
        dirEntry('/System/Applications/Terminal.app', 'Terminal.app'),
    ],
    '/System/Library/CoreServices': [
        dirEntry('/System/Library/CoreServices/Finder.app', 'Finder.app'),
        dirEntry(
            '/System/Library/CoreServices/SystemUIServer',
            'SystemUIServer',
        ),
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

// vi.resetModules() also reloads `highbeam:*`, so each test's fresh import
// gets a fresh stub with no implementation. We re-bind mocks against the
// post-reset module instances and return them alongside the plugin.
async function loadPlugin({ isMac = true } = {}) {
    vi.resetModules();
    const fs = await import('highbeam:fs');
    const icons = await import('highbeam:icons');
    const platform = await import('highbeam:platform');
    vi.mocked(platform.isMacOS).mockReturnValue(isMac);
    vi.mocked(fs.readDir).mockImplementation((path) =>
        asyncIterable(APPS_FIXTURES[path] ?? []),
    );
    vi.mocked(icons.forPath).mockResolvedValue(ICON_SENTINEL);
    const plugin = await import('./plugin.js');
    return { plugin, fs, icons, platform };
}

describe('spotlight plugin', () => {
    test('query "saf" puts Safari first with icon + openUrl action', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(plugin.query('saf', { aborted: false }));

        expect(results.length).toBeGreaterThan(0);
        const [first] = results;
        expect(first.key).toBe('/Applications/Safari.app');
        expect(first.subtitle).toBe('/Applications/Safari.app');
        // Title carries HTML <b>...</b> around the fuzzy-matched chars so the
        // UI can highlight them; strip tags before comparing.
        expect(first.title.replace(/<[^>]+>/g, '')).toBe('Safari');
        expect(first.title).toMatch(/<b>Saf<\/b>/i);
        expect(first.icon).toBe(ICON_SENTINEL);
        expect(first.action).toEqual({
            kind: 'openUrl',
            url: '/Applications/Safari.app',
        });
    });

    test('query "cal" ranks Calculator first', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(plugin.query('cal', { aborted: false }));

        expect(results.length).toBeGreaterThan(0);
        expect(results[0].key).toBe('/Applications/Calculator.app');
    });

    test('non-matching query returns no results', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(
            plugin.query('zzzzzzzzzzzzz', { aborted: false }),
        );
        expect(results).toEqual([]);
    });

    test('empty input returns no results', async () => {
        const { plugin } = await loadPlugin();
        expect(await collect(plugin.query('', { aborted: false }))).toEqual(
            [],
        );
        expect(await collect(plugin.query('   ', { aborted: false }))).toEqual(
            [],
        );
    });

    test('non-.app entries are filtered out', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(
            plugin.query('readme', { aborted: false }),
        );
        // README.txt would match "readme" if not filtered.
        expect(results).toEqual([]);
    });

    test('returns zero results on non-macOS regardless of input', async () => {
        const { plugin, fs } = await loadPlugin({ isMac: false });
        const results = await collect(
            plugin.query('safari', { aborted: false }),
        );
        expect(results).toEqual([]);
        // The platform gate fires before fs.readDir is touched.
        expect(fs.readDir).not.toHaveBeenCalled();
    });
});
