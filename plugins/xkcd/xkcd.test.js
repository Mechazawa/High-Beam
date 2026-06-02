import { beforeEach, describe, expect, test, vi } from 'vitest';

// Canned comic fixtures. xkcd's `info.0.json` actually returns a fair bit
// more (`year`, `month`, `day`, `news`, `safe_title`, `transcript`,
// `img`, etc.) — we only model what the plugin reads.
function comic(num, title, alt) {
    return {
        num,
        title,
        alt,
        // Fields the plugin ignores, included to keep the shape realistic.
        year: '2020',
        month: '1',
        day: '1',
        safe_title: title,
        transcript: '',
        img: `https://imgs.xkcd.com/comics/${num}.png`,
        link: '',
        news: '',
    };
}

const LATEST_NUM = 2700;
const LATEST = comic(LATEST_NUM, 'Newest Comic', 'newest alt');
const SPECIFIC = comic(614, 'Woodpecker', 'a comic about woodpeckers');

function httpResponse({ status = 200, body = '' } = {}) {
    return {
        status,
        statusText: status === 200 ? 'OK' : 'Not Found',
        headers: {},
        body,
        ok: status >= 200 && status < 300,
        async json() { return JSON.parse(body); },
        async text() { return body; },
    };
}

function okJson(obj) {
    return httpResponse({ status: 200, body: JSON.stringify(obj) });
}

function notFound() {
    return httpResponse({ status: 404, body: '' });
}

async function collect(iter) {
    const out = [];
    for await (const item of iter) out.push(item);
    return out;
}

// Each test gets its own plugin instance (module-level cache resets too).
async function loadPlugin() {
    vi.resetModules();
    const fs = await import('highbeam:fs');
    vi.mocked(fs.readCache).mockReset();
    vi.mocked(fs.writeCache).mockReset();
    const fetchMock = vi.fn(async () => httpResponse());
    vi.stubGlobal('fetch', fetchMock);
    vi.mocked(fs.readCache).mockResolvedValue(null);
    vi.mocked(fs.writeCache).mockResolvedValue(undefined);
    const plugin = await import('./plugin.js');
    return { plugin, fetchMock, fs };
}

beforeEach(() => {
    // resetModules() takes care of the per-plugin cache; nothing to do here
    // beyond letting each loadPlugin() set up its own canned responses.
});

describe('xkcd plugin', () => {
    test('non-trigger input yields nothing and skips network', async () => {
        const { plugin, fetchMock } = await loadPlugin();
        const results = await collect(
            plugin.query('hello', { aborted: false }),
        );
        expect(results).toEqual([]);
        expect(fetchMock).not.toHaveBeenCalled();
    });

    test('empty `xkcd ` yields no results', async () => {
        const { plugin, fetchMock } = await loadPlugin();
        const a = await collect(plugin.query('xkcd', { aborted: false }));
        const b = await collect(plugin.query('xkcd ', { aborted: false }));
        const c = await collect(plugin.query('xkcd   ', { aborted: false }));
        expect(a).toEqual([]);
        expect(b).toEqual([]);
        expect(c).toEqual([]);
        expect(fetchMock).not.toHaveBeenCalled();
    });

    test('`xkcd latest` hits info.0.json and yields one result', async () => {
        const { plugin, fetchMock } = await loadPlugin();
        fetchMock.mockResolvedValueOnce(okJson(LATEST));
        const results = await collect(
            plugin.query('xkcd latest', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        const [r] = results;
        expect(r.title).toBe(`${LATEST_NUM}: Newest Comic`);
        expect(r.subtitle).toBe('newest alt');
        expect(r.weight).toBe(80);
        expect(r.pinned).toBe(false);
        expect(r.action).toEqual({
            kind: 'openUrl',
            url: `https://xkcd.com/${LATEST_NUM}/`,
        });
        expect(fetchMock).toHaveBeenCalledWith(
            'https://xkcd.com/info.0.json',
            expect.any(Object),
        );
    });

    test('`xkcd 614` fetches that specific comic', async () => {
        const { plugin, fetchMock } = await loadPlugin();
        fetchMock.mockResolvedValueOnce(okJson(SPECIFIC));
        const results = await collect(
            plugin.query('xkcd 614', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        expect(results[0].title).toBe('614: Woodpecker');
        expect(results[0].subtitle).toBe('a comic about woodpeckers');
        expect(results[0].action).toEqual({
            kind: 'openUrl',
            url: 'https://xkcd.com/614/',
        });
        expect(fetchMock).toHaveBeenCalledWith(
            'https://xkcd.com/614/info.0.json',
            expect.any(Object),
        );
    });

    test('`xkcd 9999999` returns 0 results when the comic is missing', async () => {
        const { plugin, fetchMock } = await loadPlugin();
        fetchMock.mockResolvedValueOnce(notFound());
        const results = await collect(
            plugin.query('xkcd 9999999', { aborted: false }),
        );
        expect(results).toEqual([]);
        expect(fetchMock).toHaveBeenCalledWith(
            'https://xkcd.com/9999999/info.0.json',
            expect.any(Object),
        );
    });

    test('`xkcd random` resolves to some comic (latest = 10 fixture)', async () => {
        const { plugin, fetchMock } = await loadPlugin();
        // First call: latest metadata to discover the highest number.
        // Subsequent call: the specific N picked at random. We return the
        // latest comic to both so the result is well-defined regardless of
        // which N Math.random picked.
        const latestTen = comic(10, 'Ten', 'alt-ten');
        fetchMock.mockImplementation(async (url) => {
            if (url === 'https://xkcd.com/info.0.json') return okJson(latestTen);
            return okJson(latestTen);
        });
        const results = await collect(
            plugin.query('xkcd random', { aborted: false }),
        );
        expect(results).toHaveLength(1);
        const r = results[0];
        expect(r.title).toMatch(/^\d+: /);
        expect(r.action.kind).toBe('openUrl');
        expect(r.action.url).toMatch(/^https:\/\/xkcd\.com\/\d+\/$/);
    });

    test('text search uses cached index without hitting the network', async () => {
        const { plugin, fetchMock, fs } = await loadPlugin();
        const cached = {
            last_updated: Date.now(),
            comics: [
                { num: 1, title: 'Barrel', alt: 'a barrel' },
                { num: 2, title: 'Petit Trees', alt: 'tiny tree' },
                { num: 100, title: 'Family Circus', alt: 'circus alt' },
                { num: 535, title: 'Tape Measure', alt: 'tape' },
                { num: 660, title: 'Sympathy', alt: 'sympathy alt' },
                {
                    num: 1110,
                    title: 'Click and Drag',
                    alt: 'huge xkcd map',
                },
                {
                    num: 1252,
                    title: 'Increased Risk',
                    alt: 'risk alt',
                },
                // The needle:
                {
                    num: 1747,
                    title: 'Spider Paleontology',
                    alt: 'raptor in here actually no',
                },
                {
                    num: 1422,
                    title: 'My Hobby: Raptor Identification',
                    alt: 'velociraptor mouseover',
                },
            ],
        };
        vi.mocked(fs.readCache).mockResolvedValue(
            new TextEncoder().encode(JSON.stringify(cached)),
        );
        const results = await collect(
            plugin.query('xkcd raptor', { aborted: false }),
        );
        expect(results.length).toBeGreaterThan(0);
        expect(results[0].title).toBe('1422: My Hobby: Raptor Identification');
        expect(results[0].subtitle).toBe('velociraptor mouseover');
        expect(results[0].action).toEqual({
            kind: 'openUrl',
            url: 'https://xkcd.com/1422/',
        });
        // No network round-trips when the cache is fresh.
        expect(fetchMock).not.toHaveBeenCalled();
    });

    test('text search with no cache fetches a small index and matches', async () => {
        const { plugin, fetchMock, fs } = await loadPlugin();
        // No cache on disk — readCache returns null (set in loadPlugin).
        // Five-comic fixture; one of them (#4) has "Raptor" in its title.
        const latest = comic(5, 'Newest', 'newest alt');
        const comics = {
            1: comic(1, 'Barrel - Part 1', 'barrel alt'),
            2: comic(2, 'Petit Trees', 'trees alt'),
            3: comic(3, 'Island', 'island alt'),
            4: comic(4, 'Raptor Identification', 'velociraptors!'),
            5: latest,
        };
        fetchMock.mockImplementation(async (url) => {
            if (url === 'https://xkcd.com/info.0.json') return okJson(latest);
            const m = /xkcd\.com\/(\d+)\/info\.0\.json$/.exec(url);
            if (m) {
                const n = Number(m[1]);
                const c = comics[n];
                if (c) return okJson(c);
                return notFound();
            }
            return notFound();
        });

        const results = await collect(
            plugin.query('xkcd raptor', { aborted: false }),
        );
        expect(results.length).toBeGreaterThan(0);
        expect(results[0].title).toBe('4: Raptor Identification');
        expect(results[0].subtitle).toBe('velociraptors!');
        // Cache write fired so subsequent queries can skip the network.
        expect(fs.writeCache).toHaveBeenCalledTimes(1);
        const [name, data] = vi.mocked(fs.writeCache).mock.calls[0];
        expect(name).toBe('xkcd-index.json');
        const persisted = JSON.parse(data);
        expect(persisted.comics.length).toBeGreaterThan(0);
        expect(typeof persisted.last_updated).toBe('number');
    });
});
