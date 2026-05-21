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

function titles(results) {
    return results.map((r) => r.title);
}

describe('color-converter plugin', () => {
    test('#ff0000 yields rgb + hsl, no hex echo', async () => {
        const results = await run('#ff0000');
        expect(titles(results)).toEqual(['rgb(255, 0, 0)', 'hsl(0, 100%, 50%)']);
        results.forEach((r) => {
            expect(r.pinned).toBe(true);
            expect(r.action.kind).toBe('copy');
            expect(r.action.text).toBe(r.title);
        });
    });

    test('#f00 expands to rgb + hsl with no hex echo', async () => {
        const results = await run('#f00');
        expect(titles(results)).toEqual(['rgb(255, 0, 0)', 'hsl(0, 100%, 50%)']);
    });

    test('rgb(0, 128, 255) yields hex + hsl', async () => {
        const results = await run('rgb(0, 128, 255)');
        const labels = results.map((r) => r.title);
        expect(labels).toContain('#0080ff');
        expect(labels.some((l) => l.startsWith('hsl('))).toBe(true);
        // No echo of the rgb input family.
        expect(labels.some((l) => l.startsWith('rgb('))).toBe(false);
    });

    test('rgba(255, 0, 0, 0.5) yields hex8 with 80 alpha, no rgba echo, no hsl', async () => {
        const results = await run('rgba(255, 0, 0, 0.5)');
        expect(titles(results)).toEqual(['#ff000080']);
        expect(results[0].subtitle).toBe('HEX (with alpha)');
        expect(results[0].action.text).toBe('#ff000080');
    });

    test('hsl(120, 100%, 50%) yields #00ff00 + rgb, no hsl echo', async () => {
        const results = await run('hsl(120, 100%, 50%)');
        const labels = titles(results);
        expect(labels).toContain('#00ff00');
        expect(labels).toContain('rgb(0, 255, 0)');
        expect(labels.some((l) => l.startsWith('hsl('))).toBe(false);
    });

    test('#xyz is rejected', async () => {
        expect(await run('#xyz')).toEqual([]);
    });

    test('rgb(300, 0, 0) is rejected (out of range)', async () => {
        expect(await run('rgb(300, 0, 0)')).toEqual([]);
    });

    test('hello is rejected', async () => {
        expect(await run('hello')).toEqual([]);
    });

    test('empty input yields nothing', async () => {
        expect(await run('')).toEqual([]);
        expect(await run('   ')).toEqual([]);
    });

    test('every non-empty match is pinned', async () => {
        const inputs = [
            '#fff',
            '#000000',
            '#ff000080',
            'rgb(10, 20, 30)',
            'rgba(10, 20, 30, 0.25)',
            'hsl(200, 50%, 50%)',
        ];
        for (const input of inputs) {
            const results = await run(input);
            expect(results.length).toBeGreaterThan(0);
            results.forEach((r) => expect(r.pinned).toBe(true));
        }
    });

    test('hex8 input with alpha yields rgba only (no hex echo, no hsl)', async () => {
        const results = await run('#ff000080');
        expect(results).toHaveLength(1);
        // 0x80 / 255 ≈ 0.502 — the format rounds alpha to 3 decimals.
        expect(results[0].title).toBe('rgba(255, 0, 0, 0.502)');
        expect(results[0].subtitle).toBe('RGBA');
    });

    test('opaque rgba (alpha = 1) is treated as opaque', async () => {
        const results = await run('rgba(255, 0, 0, 1)');
        const labels = titles(results);
        expect(labels).toContain('#ff0000');
        expect(labels.some((l) => l.startsWith('hsl('))).toBe(true);
        expect(labels.some((l) => l.startsWith('rgb'))).toBe(false);
    });

    test('hex case-insensitive', async () => {
        const results = await run('#AABBCC');
        const labels = titles(results);
        expect(labels.some((l) => l.startsWith('rgb('))).toBe(true);
    });

    test('keys are stable per (family, formatted text)', async () => {
        const a = await run('#ff0000');
        const b = await run(' #ff0000 ');
        expect(a.map((r) => r.key)).toEqual(b.map((r) => r.key));
    });

    test('weight ordering: hex 100, rgb 90, hsl 80', async () => {
        const results = await run('#ff0000');
        const rgb = results.find((r) => r.subtitle === 'RGB');
        const hsl = results.find((r) => r.subtitle === 'HSL');
        expect(rgb.weight).toBe(90);
        expect(hsl.weight).toBe(80);
    });
});
