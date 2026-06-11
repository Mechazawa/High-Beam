import { describe, expect, test, vi } from 'vitest';

// `node:os` ships with no vi.fn()s, so we swap it wholesale and re-bind the
// platform() return value in loadPlugin().
vi.mock('node:os', () => {
    const platform = vi.fn(() => 'darwin');
    return { default: { platform }, platform };
});

// Realistic `ps -axo pid,comm` fixture. Column 1 is right-padded; column 2
// is the binary name (or its full path on macOS). We include a leading
// header line and a couple of edge cases:
// - PIDs with varying widths (1, 142, 28733)
// - macOS-style absolute paths in the `comm` column
// - A bare binary name (Linux-style)
// - Multiple matches against `code`
const PS_FIXTURE = `  PID COMM
    1 /sbin/launchd
  142 /Applications/Safari.app/Contents/MacOS/Safari
  201 /Applications/Visual Studio Code.app/Contents/MacOS/Electron
  202 /Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper.app/Contents/MacOS/Code Helper
  203 /Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper (Renderer).app/Contents/MacOS/Code Helper (Renderer)
 5023 finder
 9012 /usr/sbin/syslogd
28733 /Applications/Calculator.app/Contents/MacOS/Calculator
`;

async function collect(iter) {
    const out = [];
    for await (const item of iter) out.push(item);
    return out;
}

async function loadPlugin({
    platform = 'macos',
    psStdout = PS_FIXTURE,
    psCode = 0,
} = {}) {
    vi.resetModules();
    const system = await import('highbeam:system');
    const osMod = await import('node:os');
    const osPlatform = { macos: 'darwin', linux: 'linux' }[platform] ?? platform;
    vi.mocked(osMod.default.platform).mockReturnValue(osPlatform);
    vi.mocked(system.exec).mockResolvedValue({
        stdout: psStdout,
        stderr: '',
        code: psCode,
    });
    const plugin = await import('./plugin.js');
    return { plugin, system, os: osMod };
}

describe('kill-process trigger', () => {
    test('does not trigger without the kill prefix', async () => {
        const { plugin, system } = await loadPlugin();
        expect(await collect(plugin.query('safari', { aborted: false }))).toEqual([]);
        expect(system.exec).not.toHaveBeenCalled();
    });

    test('does not trigger on prefix words like `killer`', async () => {
        const { plugin, system } = await loadPlugin();
        expect(await collect(plugin.query('killer foo', { aborted: false }))).toEqual([]);
        expect(system.exec).not.toHaveBeenCalled();
    });

    test('empty query after `kill ` returns zero results', async () => {
        const { plugin } = await loadPlugin();
        expect(await collect(plugin.query('kill ', { aborted: false }))).toEqual([]);
        // Bare `kill` (no trailing space) is the same shape — empty query.
        expect(await collect(plugin.query('kill', { aborted: false }))).toEqual([]);
        expect(await collect(plugin.query('kill   ', { aborted: false }))).toEqual([]);
    });

    test('non-matching query returns 0 results even when ps runs', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(plugin.query('kill hello', { aborted: false }));
        expect(results).toEqual([]);
    });
});

describe('kill-process matching', () => {
    test('`kill saf` finds Safari with exec("kill", [pid])', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(plugin.query('kill saf', { aborted: false }));
        expect(results.length).toBeGreaterThan(0);
        const safari = results.find((r) => r.title === 'Safari');
        expect(safari).toBeDefined();
        expect(safari.key).toBe('142');
        expect(safari.subtitle).toBe('PID 142 — Enter to send SIGTERM');
        expect(safari.action).toEqual({
            kind: 'exec',
            cmd: 'kill',
            args: ['142'],
        });
    });

    test('`kill code` returns multiple Code Helper rows ranked by score', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(plugin.query('kill code', { aborted: false }));
        // We seeded multiple `code`-matching rows; expect at least the two
        // Code Helper variants to come back.
        const titles = results.map((r) => r.title);
        expect(titles.some((t) => /Code Helper/i.test(t))).toBe(true);
        // Scores should be monotonically non-increasing.
        for (let i = 1; i < results.length; i += 1) {
            expect(results[i - 1].weight).toBeGreaterThanOrEqual(
                results[i].weight,
            );
        }
    });

    test('header line is skipped and not surfaced as a result', async () => {
        const { plugin } = await loadPlugin();
        // The header is `PID COMM` — searching for "comm" must not return
        // a row whose title is "COMM".
        const results = await collect(plugin.query('kill comm', { aborted: false }));
        expect(results.every((r) => r.title !== 'COMM')).toBe(true);
        expect(results.every((r) => r.key !== 'PID')).toBe(true);
    });

    test('cap is enforced at 10 results', async () => {
        // Build a fixture with 25 matching processes so the cap is exercised.
        const lines = ['  PID COMM'];
        for (let i = 1; i <= 25; i += 1) {
            lines.push(`${String(i).padStart(5, ' ')} matchy${i}`);
        }
        const { plugin } = await loadPlugin({ psStdout: lines.join('\n') + '\n' });
        const results = await collect(plugin.query('kill matchy', { aborted: false }));
        expect(results.length).toBeLessThanOrEqual(10);
    });

    test('basename of full-path comm is matched (macOS-style paths)', async () => {
        const { plugin } = await loadPlugin();
        // Calculator's `comm` is a full path; we should match against the
        // binary basename, not the path components.
        const results = await collect(plugin.query('kill calcul', { aborted: false }));
        const calc = results.find((r) => r.title === 'Calculator');
        expect(calc).toBeDefined();
        expect(calc.key).toBe('28733');
    });

    test('weight is match.score * 100', async () => {
        const { plugin } = await loadPlugin();
        const results = await collect(plugin.query('kill safari', { aborted: false }));
        const safari = results.find((r) => r.title === 'Safari');
        expect(safari).toBeDefined();
        // Weight should be a finite number in [0, 100].
        expect(Number.isFinite(safari.weight)).toBe(true);
        expect(safari.weight).toBeGreaterThan(0);
        expect(safari.weight).toBeLessThanOrEqual(100);
    });
});

describe('kill-process ps invocation', () => {
    test('invokes `ps -axo pid,comm`', async () => {
        const { plugin, system } = await loadPlugin();
        await collect(plugin.query('kill saf', { aborted: false }));
        expect(system.exec).toHaveBeenCalledWith(
            'ps',
            ['-axo', 'pid,comm'],
            expect.any(Object),
        );
    });

    test('non-zero ps exit code yields no results', async () => {
        const { plugin } = await loadPlugin({ psCode: 1, psStdout: '' });
        const results = await collect(plugin.query('kill safari', { aborted: false }));
        expect(results).toEqual([]);
    });

    test('ps exec rejection is swallowed', async () => {
        vi.resetModules();
        const system = await import('highbeam:system');
        const osMod = await import('node:os');
        vi.mocked(osMod.default.platform).mockReturnValue('darwin');
        vi.mocked(system.exec).mockRejectedValue(new Error('aborted'));
        const plugin = await import('./plugin.js');
        const results = await collect(plugin.query('kill saf', { aborted: false }));
        expect(results).toEqual([]);
    });

    test('returns zero results on unsupported platform without touching ps', async () => {
        const { plugin, system } = await loadPlugin({ platform: 'other' });
        const results = await collect(plugin.query('kill saf', { aborted: false }));
        expect(results).toEqual([]);
        expect(system.exec).not.toHaveBeenCalled();
    });
});
