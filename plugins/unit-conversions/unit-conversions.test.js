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

// Convenience for comparing the numeric portion of a title without baking in
// floating-point quirks of the formatter at every call site.
function numericPart(title) {
    const m = /^(-?\d+(?:\.\d+)?(?:e-?\d+)?)/.exec(title);
    if (!m) throw new Error(`title has no number: ${title}`);
    return Number(m[1]);
}

describe('unit-conversions plugin', () => {
    test('100 km to mi -> ~62.137 mi', async () => {
        const [r, ...rest] = await run('100 km to mi');
        expect(rest).toEqual([]);
        expect(r.title.endsWith(' mi')).toBe(true);
        expect(numericPart(r.title)).toBeCloseTo(62.1371, 3);
        expect(r.pinned).toBe(true);
        expect(r.weight).toBe(100);
        expect(r.subtitle).toBe('100 km to mi');
        expect(r.action.kind).toBe('copy');
        expect(r.action.text).toBe(r.title.split(' ')[0]);
    });

    test('1 mi to km -> ~1.609 km', async () => {
        const [r] = await run('1 mi to km');
        expect(numericPart(r.title)).toBeCloseTo(1.60934, 3);
        expect(r.title.endsWith(' km')).toBe(true);
    });

    test('72 F to C -> ~22.222 C', async () => {
        const [r] = await run('72 F to C');
        expect(numericPart(r.title)).toBeCloseTo(22.2222, 3);
        expect(r.title.endsWith(' C')).toBe(true);
    });

    test('0 C to K -> 273.15 K', async () => {
        const [r] = await run('0 C to K');
        expect(numericPart(r.title)).toBeCloseTo(273.15, 5);
        expect(r.title.endsWith(' K')).toBe(true);
    });

    test('2 GB to MB -> 2000 MB (SI)', async () => {
        const [r] = await run('2 GB to MB');
        expect(r.title).toBe('2000 MB');
    });

    test('1 GiB to MiB -> 1024 MiB (binary)', async () => {
        const [r] = await run('1 GiB to MiB');
        expect(r.title).toBe('1024 MiB');
    });

    test('30 min to s -> 1800 s', async () => {
        const [r] = await run('30 min to s');
        expect(r.title).toBe('1800 s');
    });

    test('1 day to hr -> 24 hr', async () => {
        const [r] = await run('1 day to hr');
        expect(r.title).toBe('24 hr');
    });

    test('5 cup to ml -> ~1182.94 mL (US cup)', async () => {
        const [r] = await run('5 cup to ml');
        expect(numericPart(r.title)).toBeCloseTo(1182.94, 1);
        expect(r.title.endsWith(' mL')).toBe(true);
    });

    test('mismatched categories yields no result', async () => {
        expect(await run('100 km to s')).toEqual([]);
        expect(await run('5 kg to C')).toEqual([]);
        expect(await run('1 GB to m')).toEqual([]);
    });

    test('non-numeric value yields no result', async () => {
        expect(await run('abc km to mi')).toEqual([]);
    });

    test('missing "to <unit>" yields no result', async () => {
        expect(await run('100 km')).toEqual([]);
    });

    test('empty input yields no result', async () => {
        expect(await run('')).toEqual([]);
        expect(await run('   ')).toEqual([]);
    });

    test('unknown unit yields no result', async () => {
        expect(await run('100 km to blorp')).toEqual([]);
        expect(await run('100 zzz to mi')).toEqual([]);
    });

    test('temperature scaling is offset-aware, not just scale', async () => {
        // -40 is the famous fixed point of C↔F.
        const [c2f] = await run('-40 C to F');
        expect(numericPart(c2f.title)).toBeCloseTo(-40, 6);
        const [f2c] = await run('-40 F to C');
        expect(numericPart(f2c.title)).toBeCloseTo(-40, 6);
        // 100 C boils -> 212 F.
        const [boil] = await run('100 C to F');
        expect(numericPart(boil.title)).toBeCloseTo(212, 6);
        // 32 F freezes -> 0 C, exactly.
        const [freeze] = await run('32 F to C');
        expect(numericPart(freeze.title)).toBeCloseTo(0, 6);
    });

    test('handles negative values', async () => {
        const [r] = await run('-5 km to m');
        expect(r.title).toBe('-5000 m');
    });

    test('handles decimal values', async () => {
        const [r] = await run('1.5 hr to min');
        expect(r.title).toBe('90 min');
    });

    test('handles scientific notation in input', async () => {
        const [r] = await run('1e3 m to km');
        expect(r.title).toBe('1 km');
    });

    test('aliases: meters/feet/pounds/celsius', async () => {
        const [meters] = await run('1000 meters to km');
        expect(meters.title).toBe('1 km');
        const [feet] = await run('1 mile to feet');
        expect(numericPart(feet.title)).toBeCloseTo(5280, 1);
        const [lb] = await run('1 kg to pounds');
        expect(numericPart(lb.title)).toBeCloseTo(2.20462, 3);
        const [tempr] = await run('100 celsius to fahrenheit');
        expect(numericPart(tempr.title)).toBeCloseTo(212, 6);
    });

    test('mass: pound vs ounce', async () => {
        const [r] = await run('1 lb to oz');
        expect(numericPart(r.title)).toBeCloseTo(16, 3);
    });

    test('mass: metric vs short ton', async () => {
        const [metric] = await run('1 t to kg');
        expect(metric.title).toBe('1000 kg');
        const [short] = await run('1 tn to kg');
        expect(numericPart(short.title)).toBeCloseTo(907.185, 2);
    });

    test('volume: cubic meter to liter', async () => {
        const [r] = await run('1 m3 to L');
        expect(r.title).toBe('1000 L');
    });

    test('volume: gallon to liter (US)', async () => {
        const [r] = await run('1 gal to L');
        expect(numericPart(r.title)).toBeCloseTo(3.78541, 3);
    });

    test('area: hectare to m²', async () => {
        const [r] = await run('1 hectare to m2');
        expect(numericPart(r.title)).toBeCloseTo(10000, 1);
        expect(r.title.endsWith(' m²')).toBe(true);
    });

    test('area: acre to m²', async () => {
        const [r] = await run('1 acre to m2');
        expect(numericPart(r.title)).toBeCloseTo(4046.86, 1);
    });

    test('data: SI and IEC remain distinct', async () => {
        const [si] = await run('1 KB to B');
        expect(si.title).toBe('1000 B');
        const [iec] = await run('1 KiB to B');
        expect(iec.title).toBe('1024 B');
    });

    test('large data conversion stays readable', async () => {
        const [r] = await run('1 PB to GB');
        // 1 PB = 1_000_000 GB. The result should print as a normal integer,
        // not exponential.
        expect(r.title).toBe('1000000 GB');
    });

    test('nautical mile to km', async () => {
        const [r] = await run('1 nmi to km');
        expect(numericPart(r.title)).toBeCloseTo(1.852, 6);
    });

    test('action copies just the numeric portion', async () => {
        const [r] = await run('100 km to mi');
        expect(r.action).toEqual({ kind: 'copy', text: r.title.split(' ')[0] });
    });

    test('key is stable for the same query', async () => {
        const [a] = await run('100 km to mi');
        const [b] = await run('  100 km to mi  ');
        expect(a.key).toBe(b.key);
    });

    test('does not echo the input expression as a result title', async () => {
        const [r] = await run('100 km to mi');
        expect(r.title).not.toBe('100 km to mi');
    });

    test('case-insensitive unit aliases (mb/gb)', async () => {
        const [r] = await run('2 gb to mb');
        expect(r.title).toBe('2000 MB');
    });

    test('rejects bare numbers / pure text', async () => {
        expect(await run('100')).toEqual([]);
        expect(await run('hello world')).toEqual([]);
        expect(await run('to mi')).toEqual([]);
    });
});
