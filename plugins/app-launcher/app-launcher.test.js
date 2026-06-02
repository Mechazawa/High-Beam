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

function fileEntry(path, name) {
    return { name, path, isFile: true, isDir: false };
}

const MAC_FIXTURES = {
    '/Applications': [
        dirEntry('/Applications/Safari.app', 'Safari.app'),
        dirEntry('/Applications/Calculator.app', 'Calculator.app'),
        fileEntry('/Applications/README.txt', 'README.txt'),
    ],
    '/System/Applications': [
        dirEntry('/System/Applications/Terminal.app', 'Terminal.app'),
        dirEntry('/System/Applications/Utilities', 'Utilities'),
    ],
    '/System/Applications/Utilities': [
        dirEntry(
            '/System/Applications/Utilities/Activity Monitor.app',
            'Activity Monitor.app',
        ),
    ],
    '/System/Library/CoreServices': [
        dirEntry('/System/Library/CoreServices/Finder.app', 'Finder.app'),
        fileEntry(
            '/System/Library/CoreServices/SystemUIServer',
            'SystemUIServer',
        ),
    ],
};

const LINUX_DIR = '/usr/share/applications';
const LINUX_FIXTURES = {
    [LINUX_DIR]: [
        dirEntry(`${LINUX_DIR}/firefox.desktop`, 'firefox.desktop'),
        dirEntry(`${LINUX_DIR}/hidden.desktop`, 'hidden.desktop'),
        dirEntry(`${LINUX_DIR}/service.desktop`, 'service.desktop'),
        dirEntry(`${LINUX_DIR}/gimp.desktop`, 'gimp.desktop'),
        dirEntry(`${LINUX_DIR}/README`, 'README'),
    ],
};

const LINUX_DESKTOP_FILES = {
    [`${LINUX_DIR}/firefox.desktop`]: `[Desktop Entry]
Type=Application
Name=Firefox
Name[de]=Feuerfuchs
Exec=firefox %u
Icon=firefox
Comment=Browse the web
`,
    [`${LINUX_DIR}/hidden.desktop`]: `[Desktop Entry]
Type=Application
Name=Hidden Helper
Exec=hidden-helper
NoDisplay=true
`,
    [`${LINUX_DIR}/service.desktop`]: `[Desktop Entry]
Type=Service
Name=Background Service
Exec=svc
`,
    [`${LINUX_DIR}/gimp.desktop`]: `[Desktop Entry]
Type=Application
Name=GIMP
Exec=gimp %F
Icon=/usr/share/pixmaps/gimp.png
`,
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
async function loadPlugin({ platform = 'macos' } = {}) {
    vi.resetModules();
    const fs = await import('highbeam:fs');
    const icons = await import('highbeam:icons');
    const platformMod = await import('highbeam:platform');
    const isMac = platform === 'macos';
    const isLin = platform === 'linux';
    vi.mocked(platformMod.isMacOS).mockReturnValue(isMac);
    vi.mocked(platformMod.isLinux).mockReturnValue(isLin);
    const fixtures = isLin ? LINUX_FIXTURES : MAC_FIXTURES;
    vi.mocked(fs.readDir).mockImplementation((path) =>
        asyncIterable(fixtures[path] ?? []),
    );
    vi.mocked(fs.readText).mockImplementation(async (path) => {
        const text = LINUX_DESKTOP_FILES[path];
        if (text === undefined) throw new Error(`no fixture for ${path}`);
        return text;
    });
    vi.mocked(icons.forPath).mockResolvedValue(ICON_SENTINEL);
    const plugin = await import('./plugin.js');
    return { plugin, fs, icons, platform: platformMod };
}

describe('app-launcher macOS', () => {
    test('query "saf" puts Safari first with icon + openUrl action', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(plugin.query('saf', { aborted: false }));

        expect(results.length).toBeGreaterThan(0);
        const [first] = results;
        expect(first.key).toBe('/Applications/Safari.app');
        expect(first.subtitle).toBe('/Applications/Safari.app');
        expect(first.title).toBe('Safari');
        expect(first.icon).toBe(ICON_SENTINEL);
        expect(first.action).toEqual({
            kind: 'openUrl',
            url: '/Applications/Safari.app',
        });
    });

    test('query "calc" ranks Calculator first', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(
            plugin.query('calc', { aborted: false }),
        );

        expect(results.length).toBeGreaterThan(0);
        expect(results[0].key).toBe('/Applications/Calculator.app');
    });

    test('non-.app entries are filtered out', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(
            plugin.query('readme', { aborted: false }),
        );
        // README.txt would match "readme" if not filtered.
        expect(results).toEqual([]);
    });

    test('descends into subdirs like /System/Applications/Utilities', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(
            plugin.query('activity', { aborted: false }),
        );
        expect(results.length).toBeGreaterThan(0);
        expect(results[0].key).toBe(
            '/System/Applications/Utilities/Activity Monitor.app',
        );
    });
});

describe('app-launcher Linux', () => {
    test('query "fire" finds Firefox with placeholder-stripped exec', async () => {
        const { plugin } = await loadPlugin({ platform: 'linux' });
        const results = await collect(
            plugin.query('fire', { aborted: false }),
        );

        expect(results.length).toBeGreaterThan(0);
        const firefox = results.find((r) =>
            /firefox/i.test(r.title),
        );
        expect(firefox).toBeDefined();
        expect(firefox.key).toBe(`${LINUX_DIR}/firefox.desktop`);
        expect(firefox.action).toEqual({
            kind: 'exec',
            cmd: 'sh',
            args: ['-c', 'firefox'],
        });
    });

    test('forwards both absolute Icon paths and bare XDG names to forPath', async () => {
        const { plugin, icons } = await loadPlugin({ platform: 'linux' });
        const results = await collect(
            plugin.query('gimp', { aborted: false }),
        );
        const gimp = results.find((r) =>
            /gimp/i.test(r.title),
        );
        expect(gimp).toBeDefined();
        expect(gimp.icon).toBe(ICON_SENTINEL);
        expect(icons.forPath).toHaveBeenCalledWith('/usr/share/pixmaps/gimp.png');

        const fireResults = await collect(
            plugin.query('firefox', { aborted: false }),
        );
        const firefox = fireResults.find((r) =>
            /firefox/i.test(r.title),
        );
        // Bare XDG names now resolve via the host's freedesktop-icons lookup;
        // the plugin hands the raw spec to `forPath` and lets the host walk
        // the active GTK theme.
        expect(firefox.icon).toBe(ICON_SENTINEL);
        expect(icons.forPath).toHaveBeenCalledWith('firefox');
    });

    test('NoDisplay=true apps are skipped', async () => {
        const { plugin } = await loadPlugin({ platform: 'linux' });
        const results = await collect(
            plugin.query('hidden', { aborted: false }),
        );
        expect(results).toEqual([]);
    });

    test('Type=Service apps are skipped', async () => {
        const { plugin } = await loadPlugin({ platform: 'linux' });
        const results = await collect(
            plugin.query('background', { aborted: false }),
        );
        expect(results).toEqual([]);
    });
});

describe('app-launcher general', () => {
    test('empty input returns no results', async () => {
        const { plugin } = await loadPlugin();
        expect(await collect(plugin.query('', { aborted: false }))).toEqual(
            [],
        );
        expect(await collect(plugin.query('   ', { aborted: false }))).toEqual(
            [],
        );
    });

    test('non-matching query returns no results', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(
            plugin.query('zzzzzzzzzzzzz', { aborted: false }),
        );
        expect(results).toEqual([]);
    });

    test('returns zero results on unsupported platform', async () => {
        const { plugin, fs } = await loadPlugin({ platform: 'other' });
        const results = await collect(
            plugin.query('safari', { aborted: false }),
        );
        expect(results).toEqual([]);
        // The platform gate fires before fs.readDir is touched.
        expect(fs.readDir).not.toHaveBeenCalled();
    });
});
