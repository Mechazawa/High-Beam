import { beforeEach, describe, expect, test, vi } from 'vitest';

const FIXED_NOW = Date.UTC(2026, 4, 21, 12, 0, 0); // 2026-05-21T12:00:00Z

function rateResponse({
    base = 'USD',
    rates = { EUR: 0.9234, GBP: 0.79, JPY: 154.5, SEK: 10.45, USD: 1 },
    lastUpdate = '2026-05-21T00:00:01+00:00',
    nextUpdate = '2026-05-22T00:00:01+00:00',
} = {}) {
    return {
        result: 'success',
        base_code: base,
        rates,
        time_last_update_utc: lastUpdate,
        time_next_update_utc: nextUpdate,
    };
}

function httpResponse({ status = 200, body = '' } = {}) {
    return {
        status,
        statusText: status === 200 ? 'OK' : 'Error',
        headers: {},
        body,
        ok: status >= 200 && status < 300,
        json() { return JSON.parse(body); },
        text() { return body; },
    };
}

function okJson(obj) {
    return httpResponse({ status: 200, body: JSON.stringify(obj) });
}

function cacheEntry({
    base = 'USD',
    rates = { EUR: 0.9234, GBP: 0.79, JPY: 154.5, SEK: 10.45, USD: 1 },
    fetchedAt = FIXED_NOW - 60 * 60 * 1000,
    lastUpdateMs = FIXED_NOW - 60 * 60 * 1000,
    nextUpdateMs = FIXED_NOW + 23 * 60 * 60 * 1000,
} = {}) {
    return {
        base_code: base,
        rates,
        fetched_at: fetchedAt,
        last_update_ms: lastUpdateMs,
        next_update_ms: nextUpdateMs,
    };
}

async function collect(iter) {
    const out = [];
    for await (const item of iter) out.push(item);
    return out;
}

async function loadPlugin() {
    vi.resetModules();
    const http = await import('highbeam:http');
    const fs = await import('highbeam:fs');
    const settings = await import('highbeam:settings');
    vi.mocked(http.get).mockReset();
    vi.mocked(fs.readCache).mockReset();
    vi.mocked(fs.writeCache).mockReset();
    vi.mocked(settings.getString).mockReset();
    vi.mocked(settings.getInt).mockReset();
    vi.mocked(http.get).mockResolvedValue(httpResponse());
    vi.mocked(fs.readCache).mockResolvedValue(null);
    vi.mocked(fs.writeCache).mockResolvedValue(undefined);
    vi.mocked(settings.getString).mockReturnValue(undefined);
    vi.mocked(settings.getInt).mockReturnValue(undefined);
    const plugin = await import('./plugin.js');
    return { plugin, http, fs, settings };
}

beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(FIXED_NOW));
});

describe('currency-converter plugin', () => {
    test('non-currency input yields nothing', async () => {
        const { plugin, http } = await loadPlugin();
        const a = await collect(plugin.query('hello', { aborted: false }));
        const b = await collect(plugin.query('', { aborted: false }));
        const c = await collect(plugin.query('100 km to mi', { aborted: false }));
        const d = await collect(plugin.query('USD EUR', { aborted: false }));
        const e = await collect(plugin.query('100 dollars', { aborted: false }));
        expect(a).toEqual([]);
        expect(b).toEqual([]);
        // `km`, `mi` are 2-letter — no 3-letter codes so it's skipped.
        // (`100 km to mi` does contain "to" but no 3-letter token.)
        expect(c).toEqual([]);
        // `USD EUR` has no digit.
        expect(d).toEqual([]);
        // `dollars` is 7 letters but there's no 3-letter token; reject.
        expect(e).toEqual([]);
        expect(http.get).not.toHaveBeenCalled();
    });

    test('parses `100 USD to EUR`', async () => {
        const { plugin, http } = await loadPlugin();
        vi.mocked(http.get).mockResolvedValueOnce(okJson(rateResponse()));
        const results = await collect(
            plugin.query('100 USD to EUR', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        const [r] = results;
        expect(r.title).toBe('100.00 USD = 92.34 EUR');
        expect(r.subtitle).toMatch(/^1 USD = 0\.9234 EUR/);
        expect(r.weight).toBe(100);
        expect(r.pinned).toBe(true);
        expect(r.action).toEqual({ kind: 'copy', text: '92.34' });
        expect(http.get).toHaveBeenCalledWith(
            'https://open.er-api.com/v6/latest/USD',
            expect.any(Object),
        );
    });

    test('parses `5 GBP JPY` (implicit `to`)', async () => {
        const { plugin, http } = await loadPlugin();
        vi.mocked(http.get).mockResolvedValueOnce(okJson(rateResponse({
            base: 'GBP',
            rates: { EUR: 1.17, USD: 1.27, JPY: 195.5 },
        })));
        const results = await collect(
            plugin.query('5 GBP JPY', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        const [r] = results;
        expect(r.title).toBe('5.00 GBP = 977.50 JPY');
        expect(r.action).toEqual({ kind: 'copy', text: '977.50' });
        expect(http.get).toHaveBeenCalledWith(
            'https://open.er-api.com/v6/latest/GBP',
            expect.any(Object),
        );
    });

    test('parses `200 SEK eur` (case-insensitive)', async () => {
        const { plugin } = await loadPlugin();
        const http = await import('highbeam:http');
        vi.mocked(http.get).mockResolvedValueOnce(okJson(rateResponse({
            base: 'SEK',
            rates: { EUR: 0.087, USD: 0.096 },
        })));
        const results = await collect(
            plugin.query('200 SEK eur', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('200.00 SEK = 17.40 EUR');
    });

    test('single-code `100 USD` uses home_currency option', async () => {
        const { plugin, http, settings } = await loadPlugin();
        vi.mocked(settings.getString).mockImplementation((key) =>
            key === 'home_currency' ? 'EUR' : undefined,
        );
        vi.mocked(http.get).mockResolvedValueOnce(okJson(rateResponse()));
        const results = await collect(
            plugin.query('100 USD', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('100.00 USD = 92.34 EUR');
    });

    test('single-code without home_currency yields a hint row', async () => {
        const { plugin, http } = await loadPlugin();
        const results = await collect(
            plugin.query('100 USD', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('Currency Converter');
        expect(results[0].subtitle).toMatch(/home currency/i);
        expect(results[0].action).toEqual({ kind: 'noop' });
        expect(http.get).not.toHaveBeenCalled();
    });

    test('cache hit avoids HTTP and writeCache', async () => {
        const { plugin, http, fs } = await loadPlugin();
        vi.mocked(fs.readCache).mockResolvedValueOnce(
            new TextEncoder().encode(JSON.stringify(cacheEntry())),
        );
        const results = await collect(
            plugin.query('100 USD to EUR', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('100.00 USD = 92.34 EUR');
        expect(http.get).not.toHaveBeenCalled();
        expect(fs.writeCache).not.toHaveBeenCalled();
    });

    test('cache miss triggers HTTP and writes cache', async () => {
        const { plugin, http, fs } = await loadPlugin();
        vi.mocked(fs.readCache).mockResolvedValueOnce(null);
        vi.mocked(http.get).mockResolvedValueOnce(okJson(rateResponse()));
        const results = await collect(
            plugin.query('100 USD to EUR', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(http.get).toHaveBeenCalledTimes(1);
        expect(fs.writeCache).toHaveBeenCalledTimes(1);
        const [name, data] = vi.mocked(fs.writeCache).mock.calls[0];
        expect(name).toBe('rates-USD.json');
        const persisted = JSON.parse(data);
        expect(persisted.base_code).toBe('USD');
        expect(persisted.rates.EUR).toBe(0.9234);
        expect(typeof persisted.fetched_at).toBe('number');
        expect(typeof persisted.next_update_ms).toBe('number');
    });

    test('expired cache re-fetches', async () => {
        const { plugin, http, fs } = await loadPlugin();
        // next_update_ms is in the past — must re-fetch.
        const stale = cacheEntry({
            fetchedAt: FIXED_NOW - 48 * 60 * 60 * 1000,
            lastUpdateMs: FIXED_NOW - 48 * 60 * 60 * 1000,
            nextUpdateMs: FIXED_NOW - 24 * 60 * 60 * 1000,
        });
        vi.mocked(fs.readCache).mockResolvedValueOnce(
            new TextEncoder().encode(JSON.stringify(stale)),
        );
        vi.mocked(http.get).mockResolvedValueOnce(okJson(rateResponse()));
        const results = await collect(
            plugin.query('100 USD to EUR', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(http.get).toHaveBeenCalledTimes(1);
        expect(fs.writeCache).toHaveBeenCalledTimes(1);
    });

    test('cache_seconds option overrides API TTL', async () => {
        const { plugin, http, fs, settings } = await loadPlugin();
        vi.mocked(settings.getInt).mockImplementation((key) =>
            key === 'cache_seconds' ? 60 : undefined,
        );
        // Cache fetched 10 minutes ago — fresh per API (24h next-update)
        // but stale per the user's 60-second override. Should refetch.
        const cache = cacheEntry({
            fetchedAt: FIXED_NOW - 10 * 60 * 1000,
            lastUpdateMs: FIXED_NOW - 10 * 60 * 1000,
            nextUpdateMs: FIXED_NOW + 24 * 60 * 60 * 1000,
        });
        vi.mocked(fs.readCache).mockResolvedValueOnce(
            new TextEncoder().encode(JSON.stringify(cache)),
        );
        vi.mocked(http.get).mockResolvedValueOnce(okJson(rateResponse()));
        const results = await collect(
            plugin.query('100 USD to EUR', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(http.get).toHaveBeenCalledTimes(1);
    });

    test('API failure falls back to cached rates with stale subtitle', async () => {
        const { plugin, http, fs } = await loadPlugin();
        // Cache is technically expired; HTTP also fails.
        const stale = cacheEntry({
            fetchedAt: FIXED_NOW - 48 * 60 * 60 * 1000,
            lastUpdateMs: FIXED_NOW - 48 * 60 * 60 * 1000,
            nextUpdateMs: FIXED_NOW - 24 * 60 * 60 * 1000,
        });
        vi.mocked(fs.readCache).mockResolvedValueOnce(
            new TextEncoder().encode(JSON.stringify(stale)),
        );
        vi.mocked(http.get).mockRejectedValueOnce(new Error('network down'));
        const results = await collect(
            plugin.query('100 USD to EUR', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('100.00 USD = 92.34 EUR');
        expect(results[0].subtitle).toMatch(/rates may be stale/);
        expect(results[0].action).toEqual({ kind: 'copy', text: '92.34' });
    });

    test('API failure with no cache yields a clear failure row + noop', async () => {
        const { plugin, http, fs } = await loadPlugin();
        vi.mocked(fs.readCache).mockResolvedValueOnce(null);
        vi.mocked(http.get).mockRejectedValueOnce(new Error('network down'));
        const results = await collect(
            plugin.query('100 USD to EUR', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].title).toMatch(/couldn't fetch/i);
        expect(results[0].action).toEqual({ kind: 'noop' });
    });

    test('unknown target currency yields a failure row', async () => {
        const { plugin, http } = await loadPlugin();
        vi.mocked(http.get).mockResolvedValueOnce(okJson(rateResponse({
            rates: { EUR: 0.9234, JPY: 154.5 },
        })));
        const results = await collect(
            plugin.query('100 USD to XYZ', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].title).toMatch(/couldn't fetch/i);
        expect(results[0].subtitle).toMatch(/unknown currency code/i);
    });

    test('precision option controls decimal places', async () => {
        const { plugin, http, settings } = await loadPlugin();
        vi.mocked(settings.getInt).mockImplementation((key) =>
            key === 'precision' ? 4 : undefined,
        );
        vi.mocked(http.get).mockResolvedValueOnce(okJson(rateResponse()));
        const results = await collect(
            plugin.query('1 USD to EUR', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('1.0000 USD = 0.9234 EUR');
        expect(results[0].action).toEqual({ kind: 'copy', text: '0.9234' });
    });

    test('same-currency query returns 1:1 without HTTP', async () => {
        const { plugin, http } = await loadPlugin();
        const results = await collect(
            plugin.query('100 USD to USD', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('100.00 USD = 100.00 USD');
        expect(http.get).not.toHaveBeenCalled();
    });
});
