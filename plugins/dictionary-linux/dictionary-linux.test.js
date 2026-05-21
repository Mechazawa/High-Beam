import { beforeEach, describe, expect, test, vi } from 'vitest';

// Force-linux: the plugin gates on isLinux() so tests have to lie about the
// host. The platform stub doesn't ship as a vi.fn(), so we replace the whole
// module up front (same trick app-launcher uses).
vi.mock('highbeam:platform', () => ({
    isMacOS: vi.fn(() => false),
    isLinux: vi.fn(() => true),
    os: 'linux',
    arch: 'x86_64',
    version: 'test',
}));

import { exec } from 'highbeam:system';
import { __resetForTests, query } from './plugin.js';

async function collect(iter) {
    const out = [];
    for await (const item of iter) out.push(item);
    return out;
}

// `wn rust -over` fixture, trimmed to two parts-of-speech with multiple
// senses each. Real `wn` indents wrapped lines further; the regex needs to
// cope with that, so the fixture exercises both tight and wrapped formats.
const WN_RUST = `
Overview of noun rust

The noun rust has 4 senses (first 1 from tagged texts)

1. (5) rust, rusting, rust_yellow -- (a red or brown oxide coating on iron or steel caused by the action of oxygen and moisture)
2. corrosion, corroding, rusting -- (a state of deterioration in metal caused by moist air or chemicals; "the surface was nearly hidden by rust")
3. rust, rust_fungus -- (any of various fungi causing rust disease in plants)

Overview of verb rust

The verb rust has 4 senses (first 1 from tagged texts)

1. (1) corrode, rust, eat_into -- (become destroyed by the action of water, air, or a corrosive such as an acid; "The metal corroded"; "The pipes rusted")
2. rust -- (cause to deteriorate due to the action of water, air, or an acid)
`;

const DICT_SERENDIPITY = `1 definition found

From The Collaborative International Dictionary of English v.0.48 [gcide]:

  Serendipity \\Ser\`en*dip"i*ty\\, n.
     The faculty or phenomenon of finding valuable or agreeable
     things not sought for.
     [1913 Webster]

`;

const DICT_MULTI = `2 definitions found

From The Collaborative International Dictionary of English v.0.48 [gcide]:

  Serendipity \\Ser\`en*dip"i*ty\\, n.
     The faculty or phenomenon of finding valuable or agreeable
     things not sought for.
     [1913 Webster]

From WordNet (r) 3.0 (2006) [wn]:

  serendipity
      n 1: good luck in making unexpected and fortunate discoveries
`;

// Drives a single test: every `exec(cmd, args)` call routes through this
// table. Misses log + return a 1-code so missing fixtures show up loud.
function mockExec(table) {
    vi.mocked(exec).mockReset();
    vi.mocked(exec).mockImplementation(async (cmd, args) => {
        const key = `${cmd} ${(args ?? []).join(' ')}`;
        const handler = table[key];
        if (!handler) {
            return { stdout: '', stderr: `no fixture for: ${key}`, code: 127 };
        }
        return handler();
    });
}

const OK = (stdout = '') => ({ stdout, stderr: '', code: 0 });
const FAIL = (code = 1, stderr = '') => ({ stdout: '', stderr, code });

describe('dictionary-linux trigger parsing', () => {
    beforeEach(() => __resetForTests());

    test('non-trigger query returns 0 results', async () => {
        mockExec({});
        const results = await collect(query('rust', { aborted: false }));
        expect(results).toEqual([]);
        expect(exec).not.toHaveBeenCalled();
    });

    test('"define " (empty word) returns 0 results', async () => {
        mockExec({});
        const results = await collect(query('define ', { aborted: false }));
        expect(results).toEqual([]);
        expect(exec).not.toHaveBeenCalled();
    });

    test('"dict" prefix without trailing space is not a trigger', async () => {
        mockExec({});
        const results = await collect(query('dictate', { aborted: false }));
        expect(results).toEqual([]);
    });
});

describe('dictionary-linux WordNet path', () => {
    beforeEach(() => __resetForTests());

    test('`define rust` with WordNet present yields 3 results from `wn -over`', async () => {
        mockExec({
            'which wn': () => OK('/usr/bin/wn\n'),
            'wn rust -over': () => OK(WN_RUST),
        });

        const results = await collect(query('define rust', { aborted: false }));

        expect(results).toHaveLength(3);
        for (const r of results) {
            expect(r.title).toBe('rust');
            expect(r.weight).toBe(80);
            expect(r.pinned).toBe(true);
            expect(r.action.kind).toBe('copy');
        }
        // First sense ranks first; subtitle is the truncated definition.
        expect(results[0].action.text).toMatch(/^a red or brown oxide coating/);
        expect(results[0].subtitle.length).toBeLessThanOrEqual(81);
        expect(results[1].action.text).toMatch(/^a state of deterioration/);
        expect(results[2].action.text).toMatch(/^any of various fungi/);
    });

    test('subtitle truncation keeps full text in copy action', async () => {
        mockExec({
            'which wn': () => OK(),
            'wn rust -over': () => OK(WN_RUST),
        });

        const results = await collect(query('dict rust', { aborted: false }));
        const long = results.find((r) => r.action.text.length > 80);
        expect(long).toBeDefined();
        expect(long.subtitle.endsWith('…')).toBe(true);
    });
});

describe('dictionary-linux DICT fallback', () => {
    beforeEach(() => __resetForTests());

    test('only `dict` present (WordNet returns non-zero) → 1 result from dict', async () => {
        mockExec({
            'which wn': () => FAIL(1),
            'which dict': () => OK('/usr/bin/dict\n'),
            'dict serendipity': () => OK(DICT_SERENDIPITY),
        });

        const results = await collect(
            query('define serendipity', { aborted: false }),
        );

        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('serendipity');
        expect(results[0].action.text).toMatch(/faculty or phenomenon/);
    });

    test('multi-database dict output surfaces the first block only', async () => {
        mockExec({
            'which wn': () => FAIL(1),
            'which dict': () => OK(),
            'dict serendipity': () => OK(DICT_MULTI),
        });

        const results = await collect(
            query('define serendipity', { aborted: false }),
        );

        expect(results).toHaveLength(1);
        // gcide block wins; WordNet block is ignored.
        expect(results[0].action.text).toMatch(/faculty or phenomenon/);
        expect(results[0].action.text).not.toMatch(/good luck in making/);
    });

    test('wn present but returns nothing → falls through to dict', async () => {
        mockExec({
            'which wn': () => OK(),
            'wn nonsense -over': () => OK(''),
            'which dict': () => OK(),
            'dict nonsense': () => OK(DICT_SERENDIPITY),
        });

        const results = await collect(query('define nonsense', { aborted: false }));
        expect(results).toHaveLength(1);
    });
});

describe('dictionary-linux grep fallback', () => {
    beforeEach(() => __resetForTests());

    test('both wn and dict absent → /usr/share/dict/words check fires', async () => {
        mockExec({
            'which wn': () => FAIL(1),
            'which dict': () => FAIL(1),
            'which grep': () => OK(),
            'grep -i ^foo$ /usr/share/dict/words': () => OK('foo\n'),
        });

        const results = await collect(query('define foo', { aborted: false }));
        expect(results).toHaveLength(1);
        expect(results[0].subtitle).toMatch(/no definition available/i);
        expect(results[0].action.text).toMatch(/install wn/);
    });

    test('unknown word with nothing installed → "no definition found"', async () => {
        mockExec({
            'which wn': () => FAIL(1),
            'which dict': () => FAIL(1),
            'which grep': () => OK(),
            'grep -i ^notarealword$ /usr/share/dict/words': () => FAIL(1),
        });

        const results = await collect(
            query('define notarealword', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].subtitle).toMatch(/No definition found/);
    });
});

describe('dictionary-linux tool detection caching', () => {
    beforeEach(() => __resetForTests());

    test('repeat queries reuse the cached `which` verdict', async () => {
        mockExec({
            'which wn': () => OK(),
            'wn rust -over': () => OK(WN_RUST),
        });

        await collect(query('define rust', { aborted: false }));
        await collect(query('define rust', { aborted: false }));

        const whichCalls = vi
            .mocked(exec)
            .mock.calls.filter((c) => c[0] === 'which');
        expect(whichCalls).toHaveLength(1);
    });
});

describe('dictionary-linux fallback chain order', () => {
    beforeEach(() => __resetForTests());

    test('wn missing → tries dict; dict missing → tries grep', async () => {
        const calls = [];
        vi.mocked(exec).mockReset();
        vi.mocked(exec).mockImplementation(async (cmd, args) => {
            calls.push(`${cmd} ${(args ?? []).join(' ')}`);
            if (cmd === 'which' && args[0] === 'wn') return FAIL(1);
            if (cmd === 'which' && args[0] === 'dict') return FAIL(1);
            if (cmd === 'which' && args[0] === 'grep') return OK();
            if (cmd === 'grep') return FAIL(1);
            return FAIL(127, `unexpected exec: ${cmd}`);
        });

        await collect(query('define whatever', { aborted: false }));

        // Ordering matters: wn first, dict second, grep third.
        expect(calls).toEqual([
            'which wn',
            'which dict',
            'which grep',
            'grep -i ^whatever$ /usr/share/dict/words',
        ]);
    });
});
