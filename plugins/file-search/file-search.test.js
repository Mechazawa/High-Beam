import { describe, expect, test, vi } from 'vitest';

// `platform` exports plain consts/funcs (no vi.fn()s) — replace the module
// outright so we can flip OS per test without touching real process.platform.
vi.mock('highbeam:platform', () => ({
    isMacOS: vi.fn(() => true),
    isLinux: vi.fn(() => false),
    os: 'macos',
    arch: 'x86_64',
    version: 'test',
}));

const ICON_SENTINEL = 'data:image/png;base64,SENTINEL';

async function collect(iter) {
    const out = [];
    for await (const item of iter) out.push(item);
    return out;
}

// vi.resetModules() ensures each test re-imports plugin.js with the freshly
// mocked highbeam:* modules; otherwise module-level platform checks would
// stick on whatever the first test set.
async function loadPlugin({ platform = 'macos' } = {}) {
    vi.resetModules();
    const system = await import('highbeam:system');
    const icons = await import('highbeam:icons');
    const platformMod = await import('highbeam:platform');
    const isMac = platform === 'macos';
    const isLin = platform === 'linux';
    vi.mocked(platformMod.isMacOS).mockReturnValue(isMac);
    vi.mocked(platformMod.isLinux).mockReturnValue(isLin);
    vi.mocked(icons.forPath).mockResolvedValue(ICON_SENTINEL);
    // Default exec mock: empty stdout, success. Tests override per-call.
    vi.mocked(system.exec).mockResolvedValue({
        stdout: '',
        stderr: '',
        code: 0,
    });
    const plugin = await import('./plugin.js');
    return { plugin, system, icons, platform: platformMod };
}

describe('file-search trigger', () => {
    test('non-trigger query yields nothing', async () => {
        const { plugin, system } = await loadPlugin();
        const results = await collect(
            plugin.query('hello', { aborted: false }),
        );
        expect(results).toEqual([]);
        // Trigger gate fires before we shell out.
        expect(system.exec).not.toHaveBeenCalled();
    });

    test('bare `find` with no body yields nothing', async () => {
        const { plugin, system } = await loadPlugin();
        // Both with and without trailing space — both are "no body".
        expect(
            await collect(plugin.query('find', { aborted: false })),
        ).toEqual([]);
        expect(
            await collect(plugin.query('find ', { aborted: false })),
        ).toEqual([]);
        expect(
            await collect(plugin.query('find   ', { aborted: false })),
        ).toEqual([]);
        expect(system.exec).not.toHaveBeenCalled();
    });

    test('trigger is case-insensitive', async () => {
        const { plugin, system } = await loadPlugin();
        vi.mocked(system.exec).mockResolvedValue({
            stdout: '/Users/me/foo.txt\n',
            stderr: '',
            code: 0,
        });
        const results = await collect(
            plugin.query('FIND foo', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('foo.txt');
    });
});

describe('file-search macOS', () => {
    test('`find report` parses 3 mdfind paths into 3 results', async () => {
        const { plugin, system, icons } = await loadPlugin();
        vi.mocked(system.exec).mockResolvedValue({
            stdout:
                '/Users/me/Documents/report.pdf\n' +
                '/Users/me/reports/q1.txt\n' +
                '/Users/me/Desktop/report-final.docx\n',
            stderr: '',
            code: 0,
        });

        const results = await collect(
            plugin.query('find report', { aborted: false }),
        );

        expect(system.exec).toHaveBeenCalledWith(
            'mdfind',
            ['-onlyin', '~', 'report'],
            expect.objectContaining({}),
        );
        expect(results).toHaveLength(3);
        expect(results[0]).toMatchObject({
            key: '/Users/me/Documents/report.pdf',
            title: 'report.pdf',
            subtitle: '/Users/me/Documents/report.pdf',
            weight: 50,
            icon: ICON_SENTINEL,
            action: {
                kind: 'openUrl',
                url: '/Users/me/Documents/report.pdf',
            },
        });
        expect(results[1].title).toBe('q1.txt');
        expect(results[2].title).toBe('report-final.docx');
        // Icons resolved once per result.
        expect(icons.forPath).toHaveBeenCalledTimes(3);
    });

    test('mdfind with empty stdout yields zero results', async () => {
        const { plugin, system } = await loadPlugin();
        vi.mocked(system.exec).mockResolvedValue({
            stdout: '',
            stderr: '',
            code: 0,
        });
        const results = await collect(
            plugin.query('find nothing-here', { aborted: false }),
        );
        expect(results).toEqual([]);
        expect(system.exec).toHaveBeenCalledOnce();
    });

    test('icon extraction failure still yields the row without icon', async () => {
        const { plugin, system, icons } = await loadPlugin();
        vi.mocked(system.exec).mockResolvedValue({
            stdout: '/Users/me/x.bin\n',
            stderr: '',
            code: 0,
        });
        vi.mocked(icons.forPath).mockRejectedValue(new Error('boom'));

        const results = await collect(
            plugin.query('find x', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].icon).toBeUndefined();
        expect(results[0].key).toBe('/Users/me/x.bin');
    });

    test('result count caps at 20 even if mdfind returns more', async () => {
        const { plugin, system } = await loadPlugin();
        const many = Array.from({ length: 50 }, (_, i) => `/tmp/f${i}.txt`);
        vi.mocked(system.exec).mockResolvedValue({
            stdout: many.join('\n') + '\n',
            stderr: '',
            code: 0,
        });
        const results = await collect(
            plugin.query('find f', { aborted: false }),
        );
        expect(results).toHaveLength(20);
    });
});

describe('file-search Linux', () => {
    test('`find todo` parses locate paths into results with openUrl', async () => {
        const { plugin, system } = await loadPlugin({ platform: 'linux' });
        vi.mocked(system.exec).mockResolvedValue({
            stdout:
                '/home/me/todo.md\n' +
                '/home/me/Documents/todos.txt\n' +
                '/home/me/projects/todo-list/notes.org\n',
            stderr: '',
            code: 0,
        });

        const results = await collect(
            plugin.query('find todo', { aborted: false }),
        );

        expect(system.exec).toHaveBeenCalledWith(
            'locate',
            ['-i', '-n', '20', 'todo'],
            expect.objectContaining({}),
        );
        expect(results).toHaveLength(3);
        expect(results[0]).toMatchObject({
            key: '/home/me/todo.md',
            title: 'todo.md',
            subtitle: '/home/me/todo.md',
            weight: 50,
            icon: ICON_SENTINEL,
            action: { kind: 'openUrl', url: '/home/me/todo.md' },
        });
        expect(results[2].title).toBe('notes.org');
    });

    // When `locate` isn't installed the host's exec adapter surfaces ENOENT
    // as `code: null` with empty stdout. We surface a single informational
    // row so the user knows to install mlocate/plocate, rather than failing
    // silently — documented choice, see plugin.js.
    test('missing locate yields informational hint row', async () => {
        const { plugin, system } = await loadPlugin({ platform: 'linux' });
        vi.mocked(system.exec).mockResolvedValue({
            stdout: '',
            stderr: 'locate: command not found',
            code: null,
        });

        const results = await collect(
            plugin.query('find anything', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].title).toContain('locate');
        expect(results[0].subtitle).toMatch(/mlocate|plocate/);
        expect(results[0].action).toEqual({ kind: 'noop' });
    });

    test('locate empty stdout + zero exit (no matches) yields nothing', async () => {
        const { plugin, system } = await loadPlugin({ platform: 'linux' });
        vi.mocked(system.exec).mockResolvedValue({
            stdout: '',
            stderr: '',
            code: 0,
        });
        const results = await collect(
            plugin.query('find nothing', { aborted: false }),
        );
        expect(results).toEqual([]);
    });
});
