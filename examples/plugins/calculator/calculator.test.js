import { describe, expect, test } from 'vitest';
import { query } from './plugin.js';

async function collect(iter) {
    const out = [];
    for await (const item of iter) {
        out.push(item);
    }
    return out;
}

async function run(input) {
    return collect(query(input, { aborted: false }));
}

describe('calculator plugin', () => {
    test('evaluates simple addition', async () => {
        const [r, ...rest] = await run('1+1');
        expect(rest).toEqual([]);
        expect(r.title).toBe('2');
        expect(r.pinned).toBe(true);
        expect(r.weight).toBe(100);
        expect(r.action.kind).toBe('copy');
        expect(r.action.text).toBe('2');
    });

    test('honors precedence and parens', async () => {
        const [r] = await run('(2+3)*4');
        expect(r.title).toBe('20');
        expect(r.action.text).toBe('20');
    });

    test('calls sqrt', async () => {
        const [r] = await run('sqrt(16)');
        expect(r.title).toBe('4');
    });

    test('right-associative exponentiation', async () => {
        const [r] = await run('2**10');
        expect(r.title).toBe('1024');
    });

    test('divide-by-zero yields no result', async () => {
        expect(await run('10/0')).toEqual([]);
    });

    test('empty input yields no result', async () => {
        expect(await run('')).toEqual([]);
    });

    test('whitespace-only input yields no result', async () => {
        expect(await run('   \t\n')).toEqual([]);
    });

    test('pi constant', async () => {
        const [r] = await run('pi*2');
        // 2π ≈ 6.283185307180 after the formatter's 12-digit rounding
        expect(r.title.startsWith('6.28318530718')).toBe(true);
        expect(r.action.text).toBe(r.title);
    });

    test('overflow yields no result', async () => {
        expect(await run('10**500')).toEqual([]);
    });

    test('invalid syntax yields no result', async () => {
        expect(await run('1 +* 2')).toEqual([]);
        expect(await run('((1+2)')).toEqual([]);
        expect(await run('sqrt()')).toEqual([]);
        expect(await run('foo(1)')).toEqual([]);
    });

    test('unary minus and modulo', async () => {
        const [neg] = await run('-3+5');
        expect(neg.title).toBe('2');
        const [mod] = await run('10%3');
        expect(mod.title).toBe('1');
    });

    test('multi-arg functions', async () => {
        const [mx] = await run('max(1, 2, 3)');
        expect(mx.title).toBe('3');
        const [mn] = await run('min(4, 2)');
        expect(mn.title).toBe('2');
        const [pw] = await run('pow(2, 8)');
        expect(pw.title).toBe('256');
    });

    test('e constant', async () => {
        const [r] = await run('e');
        expect(r.title.startsWith('2.71828')).toBe(true);
    });

    test('rounding helpers', async () => {
        expect((await run('floor(2.9)'))[0].title).toBe('2');
        expect((await run('ceil(2.1)'))[0].title).toBe('3');
        expect((await run('round(2.5)'))[0].title).toBe('3');
        expect((await run('abs(-7)'))[0].title).toBe('7');
    });

    test('stable key per input', async () => {
        const [a] = await run('1+1');
        const [b] = await run(' 1+1 ');
        expect(a.key).toBe(b.key);
    });

    test('= prefix forces a result on clean evaluation', async () => {
        const [r, ...rest] = await run('=1+1');
        expect(rest).toEqual([]);
        expect(r.title).toBe('2');
        expect(r.subtitle).toBe('1+1');
        expect(r.pinned).toBe(true);
        expect(r.action.kind).toBe('copy');
        expect(r.action.text).toBe('2');
    });

    test('= alone yields a syntax-error row labelled empty expression', async () => {
        const [r, ...rest] = await run('=');
        expect(rest).toEqual([]);
        expect(r.title).toBe('Syntax error');
        expect(r.subtitle).toBe('empty expression');
        expect(r.pinned).toBe(true);
        expect(r.action.kind).toBe('copy');
        // Enter copies what they typed so the keystroke isn't lost.
        expect(r.action.text).toBe('=');
    });

    test('=10/0 yields a syntax-error row labelled divide by zero or overflow', async () => {
        const [r, ...rest] = await run('=10/0');
        expect(rest).toEqual([]);
        expect(r.title).toBe('Syntax error');
        expect(r.subtitle).toBe('divide by zero or overflow');
        expect(r.pinned).toBe(true);
        expect(r.action.text).toBe('=10/0');
    });

    test('= prefix on garbled input yields a syntax-error row', async () => {
        const [r, ...rest] = await run('=oops syntax');
        expect(rest).toEqual([]);
        expect(r.title).toBe('Syntax error');
        expect(typeof r.subtitle).toBe('string');
        expect(r.subtitle.length).toBeGreaterThan(0);
        expect(r.pinned).toBe(true);
        expect(r.action.text).toBe('=oops syntax');
    });

    test('no prefix keeps clean evaluation unchanged', async () => {
        const [r, ...rest] = await run('1+1');
        expect(rest).toEqual([]);
        expect(r.title).toBe('2');
        // The bare form must not gain a subtitle — that would shift the layout
        // of every successful calc row across the launcher.
        expect(r.subtitle).toBeUndefined();
    });

    test('no prefix stays silent on garbled input', async () => {
        expect(await run('oops syntax')).toEqual([]);
    });
});
