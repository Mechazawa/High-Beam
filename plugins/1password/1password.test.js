import { beforeEach, describe, expect, test, vi } from 'vitest';

// `platform` ships with no vi.fn()s, so we swap it wholesale and re-bind in
// loadPlugin(). Mirrors the pattern from kill-process / app-launcher.
vi.mock('highbeam:platform', () => ({
    isMacOS: vi.fn(() => true),
    isLinux: vi.fn(() => false),
    os: 'macos',
    arch: 'x86_64',
    version: 'test',
}));

// A realistic `op item list --format=json` payload. Vault names exercise
// the subtitle suffix; one item without a vault confirms the fallback path.
const ITEM_LIST_FIXTURE = [
    {
        id: 'aaaaaaaaaaaaaaaaaaaaaaaaaa',
        title: 'GitHub',
        vault: { id: 'v1', name: 'Personal' },
        category: 'LOGIN',
    },
    {
        id: 'bbbbbbbbbbbbbbbbbbbbbbbbbb',
        title: 'Gmail',
        vault: { id: 'v1', name: 'Personal' },
        category: 'LOGIN',
    },
    {
        id: 'cccccccccccccccccccccccccc',
        title: 'AWS Console',
        vault: { id: 'v2', name: 'Work' },
        category: 'LOGIN',
    },
    {
        id: 'dddddddddddddddddddddddddd',
        title: 'Glitch.com',
        vault: { id: 'v1', name: 'Personal' },
        category: 'LOGIN',
    },
];

// `op item get <id> --format=json` returns the full record. We mock just the
// `urls` array we care about for the Open URL row.
const ITEM_GET_FIXTURES = {
    aaaaaaaaaaaaaaaaaaaaaaaaaa: {
        id: 'aaaaaaaaaaaaaaaaaaaaaaaaaa',
        title: 'GitHub',
        urls: [{ primary: true, href: 'https://github.com/login' }],
    },
    bbbbbbbbbbbbbbbbbbbbbbbbbb: {
        id: 'bbbbbbbbbbbbbbbbbbbbbbbbbb',
        title: 'Gmail',
        urls: [{ primary: true, href: 'https://mail.google.com' }],
    },
    cccccccccccccccccccccccccc: {
        id: 'cccccccccccccccccccccccccc',
        title: 'AWS Console',
        urls: [
            { primary: true, href: 'https://console.aws.amazon.com' },
        ],
    },
    dddddddddddddddddddddddddd: {
        id: 'dddddddddddddddddddddddddd',
        title: 'Glitch.com',
        // No URLs — tests the URL-absent code path.
        urls: [],
    },
};

async function collect(iter) {
    const out = [];
    for await (const item of iter) out.push(item);
    return out;
}

// Build a `system.exec` mock that dispatches on the args of each `op` call:
//   `op item list ...` → returns the list fixture
//   `op item get <id> ...` → returns the matching detail fixture
// `overrides` lets a single test tweak one branch (e.g. throw on list).
function buildOpExecMock({ listResult, listError, getResultFor } = {}) {
    return vi.fn(async (cmd, args, _opts) => {
        if (cmd !== 'op') {
            return { stdout: '', stderr: '', code: 0 };
        }
        if (args[0] === 'item' && args[1] === 'list') {
            if (listError) throw listError;
            if (listResult) return listResult;
            return {
                stdout: JSON.stringify(ITEM_LIST_FIXTURE),
                stderr: '',
                code: 0,
            };
        }
        if (args[0] === 'item' && args[1] === 'get') {
            const id = args[2];
            if (getResultFor) {
                const override = getResultFor(id);
                if (override) return override;
            }
            const detail = ITEM_GET_FIXTURES[id];
            if (!detail) {
                return { stdout: '', stderr: 'not found', code: 1 };
            }
            return {
                stdout: JSON.stringify(detail),
                stderr: '',
                code: 0,
            };
        }
        return { stdout: '', stderr: '', code: 0 };
    });
}

async function loadPlugin({
    platform = 'macos',
    execMock,
    settings = {},
} = {}) {
    vi.resetModules();
    const system = await import('highbeam:system');
    const platformMod = await import('highbeam:platform');
    const fs = await import('highbeam:fs');
    const settingsMod = await import('highbeam:settings');

    const isMac = platform === 'macos';
    const isLin = platform === 'linux';
    vi.mocked(platformMod.isMacOS).mockReturnValue(isMac);
    vi.mocked(platformMod.isLinux).mockReturnValue(isLin);

    // Default to cache disabled so each test sees a fresh `op item list`
    // invocation. The cache itself is exercised in its own describe block.
    vi.mocked(settingsMod.getString).mockImplementation((key) => {
        if (Object.prototype.hasOwnProperty.call(settings, key)) {
            return settings[key];
        }
        return undefined;
    });
    vi.mocked(settingsMod.getInt).mockImplementation((key) => {
        if (Object.prototype.hasOwnProperty.call(settings, key)) {
            return settings[key];
        }
        if (key === 'cache_seconds') return 0;
        return undefined;
    });

    // `readCache` returns null by default (cache miss). `writeCache` is a noop.
    vi.mocked(fs.readCache).mockResolvedValue(null);
    vi.mocked(fs.writeCache).mockResolvedValue(undefined);

    // Install the `op` exec mock.
    const mock = execMock ?? buildOpExecMock();
    vi.mocked(system.exec).mockImplementation(mock);

    const plugin = await import('./plugin.js');
    return { plugin, system, platform: platformMod, fs, settings: settingsMod };
}

beforeEach(() => {
    vi.clearAllMocks();
});

describe('1password trigger parsing', () => {
    test('does not trigger without the op keyword', async () => {
        const { plugin, system } = await loadPlugin();
        const results = await collect(
            plugin.query('github', { aborted: false }),
        );
        expect(results).toEqual([]);
        expect(system.exec).not.toHaveBeenCalled();
    });

    test('does not match prefix words like `optimist`', async () => {
        const { plugin, system } = await loadPlugin();
        const results = await collect(
            plugin.query('optimist', { aborted: false }),
        );
        expect(results).toEqual([]);
        expect(system.exec).not.toHaveBeenCalled();
    });

    test('bare `op` (no query) returns zero results', async () => {
        const { plugin, system } = await loadPlugin();
        expect(await collect(plugin.query('op', { aborted: false }))).toEqual([]);
        expect(await collect(plugin.query('op   ', { aborted: false }))).toEqual([]);
        expect(system.exec).not.toHaveBeenCalled();
    });

    test('`1p` alias also triggers the plugin', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(
            plugin.query('1p github', { aborted: false }),
        );
        expect(results.length).toBeGreaterThan(0);
        expect(results.some((r) => r.title.includes('GitHub'))).toBe(true);
    });

    test('custom keyword from settings is honoured', async () => {
        const { plugin, system } = await loadPlugin({
            settings: { keyword: 'pw' },
        });
        // Old keyword no longer triggers.
        expect(
            await collect(plugin.query('op github', { aborted: false })),
        ).toEqual([]);
        expect(system.exec).not.toHaveBeenCalled();
        // New keyword does.
        const results = await collect(
            plugin.query('pw github', { aborted: false }),
        );
        expect(results.length).toBeGreaterThan(0);
    });
});

describe('1password result rows', () => {
    test('`op github` yields 3 rows for the matched item', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(
            plugin.query('op github', { aborted: false }),
        );
        // Three rows per matched item: password, username, URL.
        const githubRows = results.filter((r) => r.title.includes('GitHub'));
        expect(githubRows).toHaveLength(3);
    });

    test('password row uses a shell pipe to pbcopy on macOS', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(
            plugin.query('op github', { aborted: false }),
        );
        const pwRow = results.find((r) =>
            r.title === 'Copy password for GitHub',
        );
        expect(pwRow).toBeDefined();
        expect(pwRow.action.kind).toBe('exec');
        expect(pwRow.action.cmd).toBe('sh');
        expect(pwRow.action.args[0]).toBe('-c');
        const script = pwRow.action.args[1];
        expect(script).toContain('op item get');
        expect(script).toContain('aaaaaaaaaaaaaaaaaaaaaaaaaa');
        expect(script).toContain('--field');
        expect(script).toContain('password');
        expect(script).toContain('--reveal');
        expect(script).toContain('pbcopy');
        // Trailing newline must be stripped — pasting into a login form with
        // an LF can submit prematurely.
        expect(script).toContain("tr -d '\\n'");
    });

    test('username row uses the username field', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(
            plugin.query('op github', { aborted: false }),
        );
        const userRow = results.find((r) =>
            r.title === 'Copy username for GitHub',
        );
        expect(userRow).toBeDefined();
        const script = userRow.action.args[1];
        expect(script).toContain('--field');
        expect(script).toContain('username');
    });

    test('Open URL row uses openUrl with the first url.href', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(
            plugin.query('op github', { aborted: false }),
        );
        const urlRow = results.find((r) => r.title === 'Open URL for GitHub');
        expect(urlRow).toBeDefined();
        expect(urlRow.action).toEqual({
            kind: 'openUrl',
            url: 'https://github.com/login',
        });
        expect(urlRow.subtitle).toBe('https://github.com/login');
    });

    test('items without URLs skip the Open URL row', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(
            plugin.query('op glitch', { aborted: false }),
        );
        const glitchRows = results.filter((r) => r.title.includes('Glitch'));
        // Two rows (password + username), not three.
        expect(glitchRows).toHaveLength(2);
        expect(glitchRows.some((r) => r.title.startsWith('Open URL'))).toBe(false);
    });

    test('subtitle includes vault name when present', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(
            plugin.query('op github', { aborted: false }),
        );
        const pwRow = results.find((r) =>
            r.title === 'Copy password for GitHub',
        );
        expect(pwRow.subtitle).toBe('1Password — Personal');
    });

    test('items are capped at 3 (so at most 9 rows total)', async () => {
        // Build a 10-item fixture that all loosely match the query.
        const big = [];
        for (let i = 0; i < 10; i += 1) {
            const id = String(i).padStart(26, 'x');
            big.push({
                id,
                title: `Test Item ${i}`,
                vault: { id: 'v1', name: 'Personal' },
            });
        }
        const execMock = buildOpExecMock({
            listResult: {
                stdout: JSON.stringify(big),
                stderr: '',
                code: 0,
            },
            // No URL data for any of these — keeps row count to 2 per item.
            getResultFor: () => ({
                stdout: JSON.stringify({ urls: [] }),
                stderr: '',
                code: 0,
            }),
        });
        const { plugin } = await loadPlugin({ execMock });
        const results = await collect(
            plugin.query('op Test Item', { aborted: false }),
        );
        // At most 3 items × 3 rows = 9 rows.
        expect(results.length).toBeLessThanOrEqual(9);
        // And the distinct item count is at most 3.
        const distinctIds = new Set(results.map((r) => r.key.split(':')[1]));
        expect(distinctIds.size).toBeLessThanOrEqual(3);
    });
});

describe('1password error fallbacks', () => {
    test('shows install prompt when `op` is not on PATH', async () => {
        const execMock = buildOpExecMock({
            listError: new Error('spawn op ENOENT'),
        });
        const { plugin } = await loadPlugin({ execMock });
        const results = await collect(
            plugin.query('op anything', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        const [row] = results;
        expect(row.title).toMatch(/CLI not found/i);
        expect(row.subtitle).toMatch(/install/i);
        expect(row.action.kind).toBe('openUrl');
        expect(row.action.url).toMatch(/developer\.1password\.com/);
    });

    test('shows install prompt when error message looks like not-found', async () => {
        const execMock = buildOpExecMock({
            listError: new Error('no such file or directory'),
        });
        const { plugin } = await loadPlugin({ execMock });
        const results = await collect(
            plugin.query('op anything', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].title).toMatch(/CLI not found/i);
    });

    test('shows signin prompt when `op item list` exits non-zero', async () => {
        const execMock = buildOpExecMock({
            listResult: {
                stdout: '',
                stderr: '[ERROR] you are not currently signed in',
                code: 1,
            },
        });
        const { plugin } = await loadPlugin({ execMock });
        const results = await collect(
            plugin.query('op github', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        const [row] = results;
        expect(row.title).toMatch(/signed out/i);
        expect(row.subtitle).toMatch(/op signin/);
        expect(row.action.kind).toBe('openUrl');
    });

    test('invalidates the cache when `op item list` errors', async () => {
        const execMock = buildOpExecMock({
            listError: new Error('spawn op ENOENT'),
        });
        const { plugin, fs } = await loadPlugin({ execMock });
        await collect(plugin.query('op github', { aborted: false }));
        // `invalidateItemListCache` writes a stale stamp via writeCache.
        expect(vi.mocked(fs.writeCache)).toHaveBeenCalled();
        const [name, payload] = vi.mocked(fs.writeCache).mock.calls[0];
        expect(name).toBe('items.json');
        const parsed = JSON.parse(payload);
        expect(parsed.stamp).toBe(0);
        expect(parsed.items).toEqual([]);
    });

    test('non-matching query returns zero rows without spawning extra `op get`s', async () => {
        const execMock = buildOpExecMock();
        const { plugin } = await loadPlugin({ execMock });
        const results = await collect(
            plugin.query('op zzznosuchthingexists', { aborted: false }),
        );
        expect(results).toEqual([]);
        // `op item list` ran (once) but no `op item get` should follow when
        // there are no fuzzy matches.
        const getCalls = execMock.mock.calls.filter(
            (call) => call[1][0] === 'item' && call[1][1] === 'get',
        );
        expect(getCalls).toHaveLength(0);
    });
});

describe('1password caching', () => {
    test('cache hit avoids invoking `op item list`', async () => {
        const fresh = {
            stamp: Date.now(),
            items: ITEM_LIST_FIXTURE,
        };
        vi.resetModules();
        const system = await import('highbeam:system');
        const platformMod = await import('highbeam:platform');
        const fs = await import('highbeam:fs');
        const settingsMod = await import('highbeam:settings');
        vi.mocked(platformMod.isMacOS).mockReturnValue(true);
        vi.mocked(platformMod.isLinux).mockReturnValue(false);
        vi.mocked(settingsMod.getString).mockReturnValue(undefined);
        vi.mocked(settingsMod.getInt).mockImplementation((key) =>
            key === 'cache_seconds' ? 30 : undefined,
        );
        vi.mocked(fs.readCache).mockResolvedValue(JSON.stringify(fresh));
        vi.mocked(fs.writeCache).mockResolvedValue(undefined);
        const execMock = buildOpExecMock();
        vi.mocked(system.exec).mockImplementation(execMock);
        const plugin = await import('./plugin.js');

        const results = await collect(
            plugin.query('op github', { aborted: false }),
        );
        expect(results.length).toBeGreaterThan(0);

        const listCalls = execMock.mock.calls.filter(
            (call) => call[1][0] === 'item' && call[1][1] === 'list',
        );
        expect(listCalls).toHaveLength(0);
    });

    test('stale cache (older than cache_seconds) triggers a re-fetch', async () => {
        const stale = {
            stamp: Date.now() - 60_000, // 60s ago
            items: ITEM_LIST_FIXTURE,
        };
        vi.resetModules();
        const system = await import('highbeam:system');
        const platformMod = await import('highbeam:platform');
        const fs = await import('highbeam:fs');
        const settingsMod = await import('highbeam:settings');
        vi.mocked(platformMod.isMacOS).mockReturnValue(true);
        vi.mocked(platformMod.isLinux).mockReturnValue(false);
        vi.mocked(settingsMod.getString).mockReturnValue(undefined);
        // Cap freshness at 30s.
        vi.mocked(settingsMod.getInt).mockImplementation((key) =>
            key === 'cache_seconds' ? 30 : undefined,
        );
        vi.mocked(fs.readCache).mockResolvedValue(JSON.stringify(stale));
        vi.mocked(fs.writeCache).mockResolvedValue(undefined);
        const execMock = buildOpExecMock();
        vi.mocked(system.exec).mockImplementation(execMock);
        const plugin = await import('./plugin.js');

        await collect(plugin.query('op github', { aborted: false }));

        const listCalls = execMock.mock.calls.filter(
            (call) => call[1][0] === 'item' && call[1][1] === 'list',
        );
        expect(listCalls).toHaveLength(1);
    });
});

describe('1password account / vault options', () => {
    test('passes `--account` to every `op` call when configured', async () => {
        const execMock = buildOpExecMock();
        const { plugin } = await loadPlugin({
            execMock,
            settings: { account: 'me@example.com' },
        });
        await collect(plugin.query('op github', { aborted: false }));
        // Every call to `op` should include --account.
        const opCalls = execMock.mock.calls.filter((c) => c[0] === 'op');
        expect(opCalls.length).toBeGreaterThan(0);
        for (const [, args] of opCalls) {
            const idx = args.indexOf('--account');
            expect(idx).toBeGreaterThan(-1);
            expect(args[idx + 1]).toBe('me@example.com');
        }
    });

    test('passes `--vault` to `op item list` when configured', async () => {
        const execMock = buildOpExecMock();
        const { plugin } = await loadPlugin({
            execMock,
            settings: { vault: 'Personal' },
        });
        await collect(plugin.query('op github', { aborted: false }));
        const listCall = execMock.mock.calls.find(
            (c) => c[0] === 'op' && c[1][0] === 'item' && c[1][1] === 'list',
        );
        expect(listCall).toBeDefined();
        const args = listCall[1];
        const idx = args.indexOf('--vault');
        expect(idx).toBeGreaterThan(-1);
        expect(args[idx + 1]).toBe('Personal');
    });
});

describe('1password platform gating', () => {
    test('returns zero results on unsupported platforms without spawning `op`', async () => {
        const execMock = buildOpExecMock();
        const { plugin, system } = await loadPlugin({
            platform: 'other',
            execMock,
        });
        const results = await collect(
            plugin.query('op github', { aborted: false }),
        );
        expect(results).toEqual([]);
        expect(system.exec).not.toHaveBeenCalled();
    });
});
