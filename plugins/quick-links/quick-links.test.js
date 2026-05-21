import { beforeEach, describe, expect, test, vi } from 'vitest';
import * as fs from 'highbeam:fs';
import { query } from './plugin.js';

// Fixture mirrors the bundled `links.json` shape but stays small so the
// per-test assertions stay obvious. Reusing the real prefixes means each
// test reads like a user typing into the launcher.
const FIXTURE = [
    {
        prefix: 'gh',
        template: 'https://github.com/{}',
        description: 'GitHub repository or user',
    },
    {
        prefix: 'npm',
        template: 'https://www.npmjs.com/package/{}',
        description: 'npm package',
    },
    {
        prefix: 'rfc',
        template: 'https://www.rfc-editor.org/rfc/rfc{}.html',
        description: 'IETF RFC by number',
    },
];

beforeEach(() => {
    // Reset call counts but not module-level cache — the cache is part of the
    // contract we want to assert on (see the "loaded once" test).
    vi.mocked(fs.readText).mockReset();
    vi.mocked(fs.readText).mockResolvedValue(JSON.stringify(FIXTURE));
});

async function collect(iter) {
    const out = [];
    for await (const item of iter) {
        out.push(item);
    }
    return out;
}

describe('quick-links plugin', () => {
    test('gh microsoft/vscode encodes the slash and opens GitHub', async () => {
        const results = await collect(
            query('gh microsoft/vscode', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        const [r] = results;
        expect(r.title).toBe('gh microsoft/vscode');
        expect(r.subtitle).toBe('GitHub repository or user');
        expect(r.pinned).toBe(true);
        expect(r.weight).toBe(90);
        // Slash is percent-encoded — `encodeURIComponent` is the safe default
        // across all templates. GitHub redirects %2F to / so the user still
        // lands at microsoft/vscode.
        expect(r.action).toEqual({
            kind: 'openUrl',
            url: 'https://github.com/microsoft%2Fvscode',
        });
    });

    test('npm react opens the package page', async () => {
        const results = await collect(query('npm react', { aborted: false }));
        expect(results).toHaveLength(1);
        const [r] = results;
        expect(r.action).toEqual({
            kind: 'openUrl',
            url: 'https://www.npmjs.com/package/react',
        });
    });

    test('rfc 7231 yields the RFC PDF URL', async () => {
        const results = await collect(query('rfc 7231', { aborted: false }));
        expect(results).toHaveLength(1);
        const [r] = results;
        expect(r.action).toEqual({
            kind: 'openUrl',
            url: 'https://www.rfc-editor.org/rfc/rfc7231.html',
        });
    });

    test('unknown prefix yields zero results', async () => {
        const results = await collect(query('unknown abc', { aborted: false }));
        expect(results).toEqual([]);
    });

    test('bare prefix with no argument yields zero results', async () => {
        // No trailing arg → regex never matches, plugin bails before loading
        // the data file.
        const results = await collect(query('gh', { aborted: false }));
        expect(results).toEqual([]);
        expect(vi.mocked(fs.readText)).not.toHaveBeenCalled();
    });

    test('prefix followed by whitespace only yields zero results', async () => {
        // `gh ` matches the split regex but the arg is empty — same outcome
        // as the bare-prefix case from the user's perspective.
        const results = await collect(query('gh   ', { aborted: false }));
        expect(results).toEqual([]);
    });

    test('links.json is loaded once across multiple queries', async () => {
        // Re-import the plugin with a fresh module registry so the in-module
        // promise cache starts empty. `resetModules` also rebuilds the SDK
        // mock, so we re-grab `fs` and re-stub `readText` on the fresh
        // instance. Three back-to-back queries should hit it exactly once.
        vi.resetModules();
        const freshFs = await import('highbeam:fs');
        vi.mocked(freshFs.readText).mockResolvedValue(JSON.stringify(FIXTURE));
        const fresh = await import('./plugin.js');
        await collect(fresh.query('gh foo', { aborted: false }));
        await collect(fresh.query('npm bar', { aborted: false }));
        await collect(fresh.query('rfc 1', { aborted: false }));
        expect(vi.mocked(freshFs.readText)).toHaveBeenCalledTimes(1);
        expect(vi.mocked(freshFs.readText)).toHaveBeenCalledWith('./links.json');
    });
});
