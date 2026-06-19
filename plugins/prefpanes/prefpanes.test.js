import { describe, expect, test, vi } from 'vitest';

// The plugin derives the platform from `os.platform()`; mock node:os so the
// return value is reassignable per-test ('darwin' = macOS, anything else = not).
vi.mock('node:os', () => {
    const platform = vi.fn(() => 'darwin');
    const homedir = vi.fn(() => '/Users/me');
    return { default: { platform, homedir }, platform, homedir };
});

const ICON_SENTINEL = 'data:image/png;base64,PREFPANE';

function dirEntry(path, name) {
    return { name, path, isFile: false, isDir: true };
}

const PANE_FIXTURES = {
    '/System/Library/PreferencePanes': [
        dirEntry(
            '/System/Library/PreferencePanes/Displays.prefPane',
            'Displays.prefPane',
        ),
        dirEntry(
            '/System/Library/PreferencePanes/Network.prefPane',
            'Network.prefPane',
        ),
        dirEntry(
            '/System/Library/PreferencePanes/ReadMe.txt',
            'ReadMe.txt',
        ),
    ],
    '/Library/PreferencePanes': [
        dirEntry(
            '/Library/PreferencePanes/Flash Player.prefPane',
            'Flash Player.prefPane',
        ),
    ],
    // ~/Library/PreferencePanes is intentionally absent — readDir throws and
    // the plugin must swallow it.
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

// `vi.resetModules()` reloads the highbeam:* stubs; re-bind mocks against
// the post-reset module instances so the freshly-imported plugin sees them.
async function loadPlugin({ platform = 'macos' } = {}) {
    vi.resetModules();
    const fs = await import('highbeam:fs');
    const icons = await import('highbeam:icons');
    const osMod = await import('node:os');
    vi.mocked(osMod.default.platform).mockReturnValue(
        platform === 'macos' ? 'darwin' : 'linux',
    );
    vi.mocked(fs.readDir).mockImplementation((path) => {
        const entries = PANE_FIXTURES[path];
        if (entries === undefined) {
            throw new Error(`no fixture for ${path}`);
        }
        return asyncIterable(entries);
    });
    vi.mocked(icons.forPath).mockResolvedValue(ICON_SENTINEL);
    const plugin = await import('./plugin.js');
    return { plugin, fs, icons, os: osMod };
}

describe('prefpanes macOS', () => {
    test('query "dis" finds Displays.prefPane with icon + exec("open", ...)', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(plugin.query('dis', { aborted: false }));

        expect(results.length).toBeGreaterThan(0);
        const [first] = results;
        expect(first.title).toBe('Displays');
        expect(first.key).toBe('/System/Library/PreferencePanes/Displays.prefPane');
        expect(first.subtitle).toBe('/System/Library/PreferencePanes/Displays.prefPane');
        expect(first.icon).toBe(ICON_SENTINEL);
        expect(first.action).toEqual({
            kind: 'exec',
            cmd: 'open',
            args: ['/System/Library/PreferencePanes/Displays.prefPane'],
        });
    });

    test('query "net" finds Network.prefPane', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(plugin.query('net', { aborted: false }));

        expect(results.length).toBeGreaterThan(0);
        const network = results.find((r) => r.title === 'Network');
        expect(network).toBeDefined();
        expect(network.key).toBe('/System/Library/PreferencePanes/Network.prefPane');
        expect(network.action).toEqual({
            kind: 'exec',
            cmd: 'open',
            args: ['/System/Library/PreferencePanes/Network.prefPane'],
        });
    });

    test('non-.prefPane entries are filtered out', async () => {
        const { plugin } = await loadPlugin();
        // ReadMe.txt would fuzzy-match "readme" if not filtered by extension.
        const results = await collect(plugin.query('readme', { aborted: false }));
        expect(results).toEqual([]);
    });

    test('empty input returns no results', async () => {
        const { plugin } = await loadPlugin();
        expect(await collect(plugin.query('', { aborted: false }))).toEqual([]);
        expect(await collect(plugin.query('   ', { aborted: false }))).toEqual([]);
    });

    test('non-matching query returns no results', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(
            plugin.query('zzzzzzzzzzzzz', { aborted: false }),
        );
        expect(results).toEqual([]);
    });

    test('returns zero results on non-macOS platforms', async () => {
        const { plugin, fs } = await loadPlugin({ platform: 'linux' });
        const results = await collect(plugin.query('displays', { aborted: false }));
        expect(results).toEqual([]);
        // Platform gate must short-circuit before any fs traversal.
        expect(fs.readDir).not.toHaveBeenCalled();
    });
});
