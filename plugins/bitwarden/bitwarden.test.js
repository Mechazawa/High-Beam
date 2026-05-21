import { beforeEach, describe, expect, test, vi } from 'vitest';

// `bw status` JSON the daemon sees when the user is unlocked. We only read
// the `status` field, but the real CLI emits a full object — included here
// to keep the fixture honest.
const STATUS_UNLOCKED = JSON.stringify({
    serverUrl: null,
    lastSync: '2026-05-21T08:00:00.000Z',
    userEmail: 'user@example.com',
    userId: 'user-uuid',
    status: 'unlocked',
});

const STATUS_LOCKED = JSON.stringify({
    serverUrl: null,
    lastSync: null,
    userEmail: 'user@example.com',
    userId: 'user-uuid',
    status: 'locked',
});

const STATUS_UNAUTH = JSON.stringify({
    serverUrl: null,
    lastSync: null,
    userEmail: null,
    userId: null,
    status: 'unauthenticated',
});

const FOLDERS = [
    { id: 'work-folder', name: 'Work' },
    { id: 'personal-folder', name: 'Personal' },
    // `bw` always emits a synthetic "No folder" sentinel with id=null. We
    // include it here to mirror reality even though normaliseFolders drops it.
    { id: null, name: 'No Folder' },
];

const ITEMS = [
    {
        object: 'item',
        id: 'item-github',
        organizationId: null,
        folderId: 'work-folder',
        type: 1,
        name: 'GitHub',
        notes: null,
        favorite: false,
        login: {
            uris: [
                { match: null, uri: 'https://github.com' },
            ],
            username: 'octocat',
            password: 'hunter2',
            totp: null,
            passwordRevisionDate: null,
        },
        collectionIds: [],
        revisionDate: '2026-04-01T10:00:00.000Z',
        creationDate: '2024-01-01T10:00:00.000Z',
        deletedDate: null,
    },
    {
        object: 'item',
        id: 'item-gitlab',
        organizationId: null,
        folderId: 'work-folder',
        type: 1,
        name: 'GitLab',
        notes: null,
        favorite: false,
        login: {
            uris: [
                { match: null, uri: 'https://gitlab.com' },
            ],
            username: 'octocat-gl',
            password: 'hunter3',
            totp: null,
            passwordRevisionDate: null,
        },
        collectionIds: [],
        revisionDate: '2026-04-02T10:00:00.000Z',
        creationDate: '2024-01-02T10:00:00.000Z',
        deletedDate: null,
    },
    {
        object: 'item',
        id: 'item-bank',
        organizationId: null,
        folderId: 'personal-folder',
        type: 1,
        name: 'My Bank',
        notes: null,
        favorite: false,
        login: {
            uris: [
                { match: null, uri: 'https://mybank.example.com' },
            ],
            username: 'me@example.com',
            password: 'hunter4',
            totp: null,
            passwordRevisionDate: null,
        },
        collectionIds: [],
        revisionDate: '2026-04-03T10:00:00.000Z',
        creationDate: '2024-01-03T10:00:00.000Z',
        deletedDate: null,
    },
    {
        object: 'item',
        id: 'item-note',
        organizationId: null,
        folderId: null,
        type: 2, // secure note — must be filtered out
        name: 'Some Secure Note',
        notes: 'super secret',
        favorite: false,
        collectionIds: [],
        revisionDate: '2026-04-04T10:00:00.000Z',
        creationDate: '2024-01-04T10:00:00.000Z',
        deletedDate: null,
    },
    {
        object: 'item',
        id: 'item-no-url',
        organizationId: null,
        folderId: null,
        type: 1,
        name: 'GitHubBackup',
        notes: null,
        favorite: false,
        login: {
            uris: [],
            username: 'octocat-backup',
            password: 'hunter5',
            totp: null,
        },
        collectionIds: [],
        revisionDate: '2026-04-05T10:00:00.000Z',
        creationDate: '2024-01-05T10:00:00.000Z',
        deletedDate: null,
    },
];

async function collect(iter) {
    const out = [];
    for await (const item of iter) out.push(item);
    return out;
}

// Fake `system.exec` router. We pattern-match on argv so each test can
// override individual subcommand outputs without rebuilding the whole map.
function makeBwRouter(overrides = {}) {
    const defaults = {
        status: { stdout: STATUS_UNLOCKED, stderr: '', code: 0 },
        listItems: { stdout: JSON.stringify(ITEMS), stderr: '', code: 0 },
        listFolders: { stdout: JSON.stringify(FOLDERS), stderr: '', code: 0 },
        getPassword: (id) => ({
            stdout:
                id === 'item-github'
                    ? 'hunter2'
                    : id === 'item-gitlab'
                      ? 'hunter3'
                      : id === 'item-bank'
                        ? 'hunter4'
                        : 'unknown',
            stderr: '',
            code: 0,
        }),
        getUsername: (id) => ({
            stdout:
                id === 'item-github'
                    ? 'octocat'
                    : id === 'item-gitlab'
                      ? 'octocat-gl'
                      : id === 'item-bank'
                        ? 'me@example.com'
                        : 'unknown',
            stderr: '',
            code: 0,
        }),
    };
    const config = { ...defaults, ...overrides };
    return vi.fn(async (cmd, args) => {
        if (cmd !== 'bw') {
            return { stdout: '', stderr: 'unknown cmd', code: 127 };
        }
        const argv = args ?? [];
        if (argv.length === 1 && argv[0] === 'status') return config.status;
        if (argv.length === 2 && argv[0] === 'list' && argv[1] === 'items') {
            return config.listItems;
        }
        if (argv.length === 2 && argv[0] === 'list' && argv[1] === 'folders') {
            return config.listFolders;
        }
        if (argv.length === 3 && argv[0] === 'get' && argv[1] === 'password') {
            return config.getPassword(argv[2]);
        }
        if (argv.length === 3 && argv[0] === 'get' && argv[1] === 'username') {
            return config.getUsername(argv[2]);
        }
        return { stdout: '', stderr: `unexpected bw call: ${argv.join(' ')}`, code: 1 };
    });
}

async function loadPlugin({
    keyword = undefined,
    cacheSeconds = undefined,
    router = makeBwRouter(),
    cacheBlob = null,
} = {}) {
    vi.resetModules();
    const system = await import('highbeam:system');
    const settings = await import('highbeam:settings');
    const fs = await import('highbeam:fs');
    vi.mocked(system.exec).mockReset();
    vi.mocked(system.exec).mockImplementation(router);
    vi.mocked(settings.getString).mockReset();
    vi.mocked(settings.getInt).mockReset();
    vi.mocked(settings.getString).mockImplementation((key) => {
        if (key === 'keyword') return keyword;
        return undefined;
    });
    vi.mocked(settings.getInt).mockImplementation((key) => {
        if (key === 'cache_seconds') return cacheSeconds;
        return undefined;
    });
    vi.mocked(fs.readCache).mockReset();
    vi.mocked(fs.writeCache).mockReset();
    vi.mocked(fs.readCache).mockResolvedValue(cacheBlob);
    vi.mocked(fs.writeCache).mockResolvedValue(undefined);
    const plugin = await import('./plugin.js');
    plugin.__test__.resetMemoryCache();
    return { plugin, system, settings, fs, router };
}

beforeEach(() => {
    vi.clearAllMocks();
});

describe('bitwarden trigger', () => {
    test('does not trigger without the keyword', async () => {
        const { plugin, router } = await loadPlugin();
        const results = await collect(plugin.query('github', { aborted: false }));
        expect(results).toEqual([]);
        expect(router).not.toHaveBeenCalled();
    });

    test('does not trigger on prefix-but-no-boundary (`bwsomething`)', async () => {
        const { plugin, router } = await loadPlugin();
        const results = await collect(
            plugin.query('bwsomething', { aborted: false }),
        );
        expect(results).toEqual([]);
        expect(router).not.toHaveBeenCalled();
    });

    test('empty query after `bw ` yields nothing', async () => {
        const { plugin, router } = await loadPlugin();
        expect(await collect(plugin.query('bw', { aborted: false }))).toEqual([]);
        expect(await collect(plugin.query('bw ', { aborted: false }))).toEqual([]);
        expect(await collect(plugin.query('bw   ', { aborted: false }))).toEqual([]);
        expect(router).not.toHaveBeenCalled();
    });

    test('honours a configured custom keyword', async () => {
        const { plugin } = await loadPlugin({ keyword: 'vault' });
        const results = await collect(
            plugin.query('vault github', { aborted: false }),
        );
        expect(results.length).toBeGreaterThan(0);
        expect(results.every((r) => r.title.includes('GitHub'))).toBe(true);
    });
});

describe('bitwarden vault rendering', () => {
    test('top item produces password + username + URL rows', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(
            plugin.query('bw github', { aborted: false }),
        );
        const titles = results.map((r) => r.title);
        // GitHub should be the top-ranked match for "github" given our fixture.
        expect(titles).toContain('Copy password for GitHub');
        expect(titles).toContain('Copy username for GitHub');
        expect(titles).toContain('Open URL for GitHub');

        const password = results.find((r) => r.title === 'Copy password for GitHub');
        expect(password.subtitle).toBe('Work — octocat');
        expect(password.action).toEqual({ kind: 'copy', text: 'hunter2' });

        const username = results.find((r) => r.title === 'Copy username for GitHub');
        expect(username.action).toEqual({ kind: 'copy', text: 'octocat' });

        const url = results.find((r) => r.title === 'Open URL for GitHub');
        expect(url.action).toEqual({
            kind: 'openUrl',
            url: 'https://github.com',
        });
        expect(url.subtitle).toBe('Work — https://github.com');
    });

    test('non-login items (secure notes, cards) are filtered out', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(
            plugin.query('bw secure', { aborted: false }),
        );
        // The secure-note fixture is named "Some Secure Note" — must not
        // surface even though "secure" matches its name.
        const titles = results.map((r) => r.title);
        expect(titles.every((t) => !t.includes('Some Secure Note'))).toBe(true);
    });

    test('items without URLs omit the Open URL row', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(
            plugin.query('bw GitHubBackup', { aborted: false }),
        );
        const backupRows = results.filter((r) => /GitHubBackup/.test(r.title));
        expect(backupRows.length).toBeGreaterThan(0);
        expect(backupRows.every((r) => !r.title.startsWith('Open URL'))).toBe(true);
        // Folder fallback when folderId is null.
        const subtitle = backupRows[0].subtitle;
        expect(subtitle.startsWith('No folder')).toBe(true);
    });

    test('caps at three items × three actions = nine rows', async () => {
        // Build a fixture with many matching items.
        const lots = [];
        for (let i = 1; i <= 12; i += 1) {
            lots.push({
                object: 'item',
                id: `id-${i}`,
                folderId: 'work-folder',
                type: 1,
                name: `matchy${i}`,
                login: {
                    uris: [{ uri: `https://matchy${i}.example.com` }],
                    username: `user${i}`,
                    password: `pw${i}`,
                },
            });
        }
        const router = makeBwRouter({
            listItems: { stdout: JSON.stringify(lots), stderr: '', code: 0 },
        });
        const { plugin } = await loadPlugin({ router });
        const results = await collect(
            plugin.query('bw matchy', { aborted: false }),
        );
        expect(results.length).toBeLessThanOrEqual(9);
    });
});

describe('bitwarden auth/install fallbacks', () => {
    test('locked vault yields a single hint row', async () => {
        const router = makeBwRouter({
            status: { stdout: STATUS_LOCKED, stderr: '', code: 0 },
        });
        const { plugin } = await loadPlugin({ router });
        const results = await collect(
            plugin.query('bw github', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        const [row] = results;
        expect(row.title).toBe('Unlock Bitwarden');
        expect(row.subtitle).toMatch(/bw unlock/);
        expect(row.action.kind).toBe('copy');
        expect(row.pinned).toBe(true);
    });

    test('unauthenticated status yields the unlock hint too', async () => {
        const router = makeBwRouter({
            status: { stdout: STATUS_UNAUTH, stderr: '', code: 0 },
        });
        const { plugin } = await loadPlugin({ router });
        const results = await collect(
            plugin.query('bw github', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('Unlock Bitwarden');
    });

    test('bw missing from PATH yields the install hint', async () => {
        const router = vi.fn(async () => {
            const err = new Error('spawn bw ENOENT');
            throw err;
        });
        const { plugin } = await loadPlugin({ router });
        const results = await collect(
            plugin.query('bw github', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        const [row] = results;
        expect(row.title).toBe('Install the Bitwarden CLI');
        expect(row.action).toEqual({
            kind: 'openUrl',
            url: 'https://bitwarden.com/help/cli/',
        });
    });

    test('bw exits non-zero with "not found" stderr → install hint', async () => {
        const router = vi.fn(async () => ({
            stdout: '',
            stderr: 'sh: bw: command not found',
            code: 127,
        }));
        const { plugin } = await loadPlugin({ router });
        const results = await collect(
            plugin.query('bw github', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('Install the Bitwarden CLI');
    });

    test('bw status returns malformed JSON → locked hint', async () => {
        const router = makeBwRouter({
            status: { stdout: 'not-json', stderr: '', code: 0 },
        });
        const { plugin } = await loadPlugin({ router });
        const results = await collect(
            plugin.query('bw github', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('Unlock Bitwarden');
    });
});

describe('bitwarden caching', () => {
    test('uses fs.cache when fresh and skips `bw list items`', async () => {
        const cached = {
            fetchedAt: Date.now(),
            items: [
                {
                    id: 'item-github',
                    name: 'GitHub',
                    folderId: 'work-folder',
                    username: 'octocat',
                    url: 'https://github.com',
                    hasPassword: true,
                },
            ],
            folders: { 'work-folder': 'Work' },
        };
        const blob = new TextEncoder().encode(JSON.stringify(cached));
        const router = makeBwRouter();
        const { plugin } = await loadPlugin({ router, cacheBlob: blob });
        await collect(plugin.query('bw github', { aborted: false }));
        // status + get(password) + get(username) — NO list items / folders calls.
        const argvCalls = router.mock.calls.map((c) => c[1]?.join(' '));
        expect(argvCalls).toContain('status');
        expect(argvCalls).not.toContain('list items');
        expect(argvCalls).not.toContain('list folders');
        expect(argvCalls).toContain('get password item-github');
    });

    test('stale fs.cache is ignored and refreshed', async () => {
        const cached = {
            // 10 minutes ago — older than the default 30s TTL.
            fetchedAt: Date.now() - 10 * 60 * 1000,
            items: [
                {
                    id: 'stale',
                    name: 'StaleItem',
                    folderId: null,
                    username: 'old',
                    url: null,
                    hasPassword: true,
                },
            ],
            folders: {},
        };
        const blob = new TextEncoder().encode(JSON.stringify(cached));
        const router = makeBwRouter();
        const { plugin, fs } = await loadPlugin({ router, cacheBlob: blob });
        await collect(plugin.query('bw github', { aborted: false }));
        const argvCalls = router.mock.calls.map((c) => c[1]?.join(' '));
        expect(argvCalls).toContain('list items');
        expect(argvCalls).toContain('list folders');
        expect(fs.writeCache).toHaveBeenCalled();
        const [name, data] = vi.mocked(fs.writeCache).mock.calls[0];
        expect(name).toBe('bitwarden-items.json');
        const persisted = JSON.parse(data);
        expect(typeof persisted.fetchedAt).toBe('number');
        expect(Array.isArray(persisted.items)).toBe(true);
    });

    test('cache_seconds = 0 disables the cache entirely', async () => {
        const fresh = {
            fetchedAt: Date.now(),
            items: [],
            folders: {},
        };
        const blob = new TextEncoder().encode(JSON.stringify(fresh));
        const router = makeBwRouter();
        const { plugin } = await loadPlugin({
            cacheSeconds: 0,
            router,
            cacheBlob: blob,
        });
        await collect(plugin.query('bw github', { aborted: false }));
        const argvCalls = router.mock.calls.map((c) => c[1]?.join(' '));
        // With TTL=0 we always refetch.
        expect(argvCalls).toContain('list items');
    });
});

describe('bitwarden helpers', () => {
    test('parseTrigger respects the configured keyword and word boundary', async () => {
        const { plugin } = await loadPlugin();
        const { parseTrigger } = plugin.__test__;
        expect(parseTrigger('bw github', 'bw')).toBe('github');
        expect(parseTrigger('  bw   github  ', 'bw')).toBe('github');
        expect(parseTrigger('bw', 'bw')).toBe('');
        expect(parseTrigger('bwgithub', 'bw')).toBe(null);
        expect(parseTrigger('vault foo', 'bw')).toBe(null);
        expect(parseTrigger('vault foo', 'vault')).toBe('foo');
        // Case-insensitive prefix match (matches kill-process semantics).
        expect(parseTrigger('BW github', 'bw')).toBe('github');
    });

    test('normaliseItem drops non-login items and items without ids/names', async () => {
        const { plugin } = await loadPlugin();
        const { normaliseItem } = plugin.__test__;
        expect(normaliseItem({ type: 2, id: 'x', name: 'note' })).toBe(null);
        expect(normaliseItem({ type: 1, id: '', name: 'x', login: {} })).toBe(null);
        expect(normaliseItem({ type: 1, id: 'x', name: '', login: {} })).toBe(null);
        const ok = normaliseItem({
            type: 1,
            id: 'x',
            name: 'X',
            folderId: 'f',
            login: {
                username: 'u',
                password: 'p',
                uris: [{ uri: 'https://x' }],
            },
        });
        expect(ok).toEqual({
            id: 'x',
            name: 'X',
            folderId: 'f',
            username: 'u',
            url: 'https://x',
            hasPassword: true,
        });
    });

    test('looksLikeNotInstalled / looksLikeLocked recognise common messages', async () => {
        const { plugin } = await loadPlugin();
        const { looksLikeNotInstalled, looksLikeLocked } = plugin.__test__;
        expect(looksLikeNotInstalled('spawn bw ENOENT')).toBe(true);
        expect(looksLikeNotInstalled('command not found')).toBe(true);
        expect(looksLikeNotInstalled('no such file or directory')).toBe(true);
        expect(looksLikeNotInstalled('plain failure')).toBe(false);

        expect(looksLikeLocked('Vault is locked.')).toBe(true);
        expect(looksLikeLocked('You are not logged in.')).toBe(true);
        expect(looksLikeLocked('Session is invalid')).toBe(true);
        expect(looksLikeLocked('something else')).toBe(false);
    });
});
