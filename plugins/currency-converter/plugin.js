import { copy } from "highbeam:actions";
import { get } from "highbeam:http";
import { readCache, writeCache } from "highbeam:fs";
import { getInt, getString } from "highbeam:settings";

const API_URL = (base) => `https://open.er-api.com/v6/latest/${base}`;
const CACHE_NAME = (base) => `rates-${base}.json`;

const DEFAULT_PRECISION = 2;
const PRECISION_MIN = 0;
const PRECISION_MAX = 6;
const RATE_PRECISION = 4;
const CACHE_SECONDS_MAX = 86_400;

// Cheap pre-filter: don't even consider input without a digit and a
// 3-letter run. Keeps us out of the parser on every casual keystroke.
const CURRENCY_TOKEN_RE = /\b[A-Za-z]{3}\b/;
const DIGIT_RE = /\d/;

// `<amount> <from> to <to>` and `<amount> <from> <to>` and `<amount> <from>`.
// Amount accepts leading sign, optional decimal, optional scientific. Commas
// in the integer portion are normalised before parsing.
const QUERY_RE_TO = /^\s*(-?[\d.,]+(?:[eE][+-]?\d+)?)\s+([A-Za-z]{3})\s+to\s+([A-Za-z]{3})\s*$/;
const QUERY_RE_PAIR = /^\s*(-?[\d.,]+(?:[eE][+-]?\d+)?)\s+([A-Za-z]{3})\s+([A-Za-z]{3})\s*$/;
const QUERY_RE_SOLO = /^\s*(-?[\d.,]+(?:[eE][+-]?\d+)?)\s+([A-Za-z]{3})\s*$/;

function configuredHomeCurrency() {
    const raw = getString("home_currency");
    if (typeof raw !== "string") return "";
    const trimmed = raw.trim().toUpperCase();
    if (!/^[A-Z]{3}$/.test(trimmed)) return "";
    return trimmed;
}

function configuredPrecision() {
    const raw = getInt("precision");
    if (typeof raw !== "number" || !Number.isFinite(raw)) return DEFAULT_PRECISION;
    const clamped = Math.min(PRECISION_MAX, Math.max(PRECISION_MIN, Math.floor(raw)));
    return clamped;
}

function configuredCacheSeconds() {
    const raw = getInt("cache_seconds");
    if (typeof raw !== "number" || !Number.isFinite(raw) || raw < 0) return 0;
    return Math.min(CACHE_SECONDS_MAX, Math.floor(raw));
}

function parseAmount(text) {
    // Strip thousands-style commas but leave a decimal comma alone if it's the
    // only one. Easiest portable rule: drop commas; users typing "1,234.56"
    // and "1234.56" both work, "1,5" decimal-comma is not supported (would
    // collide with thousand-grouping). Keep it simple.
    const normalised = text.replace(/,/g, "");
    const value = Number(normalised);
    if (!Number.isFinite(value)) return null;
    return value;
}

function parseQuery(input) {
    const trimmed = input.trim();
    if (!trimmed) return null;
    if (!DIGIT_RE.test(trimmed)) return null;
    if (!CURRENCY_TOKEN_RE.test(trimmed)) return null;

    const toMatch = QUERY_RE_TO.exec(trimmed);
    if (toMatch) {
        const amount = parseAmount(toMatch[1]);
        if (amount === null) return null;
        return {
            amount,
            from: toMatch[2].toUpperCase(),
            to: toMatch[3].toUpperCase(),
            implicitTarget: false,
        };
    }
    const pairMatch = QUERY_RE_PAIR.exec(trimmed);
    if (pairMatch) {
        const amount = parseAmount(pairMatch[1]);
        if (amount === null) return null;
        return {
            amount,
            from: pairMatch[2].toUpperCase(),
            to: pairMatch[3].toUpperCase(),
            implicitTarget: false,
        };
    }
    const soloMatch = QUERY_RE_SOLO.exec(trimmed);
    if (soloMatch) {
        const amount = parseAmount(soloMatch[1]);
        if (amount === null) return null;
        return {
            amount,
            from: soloMatch[2].toUpperCase(),
            to: null,
            implicitTarget: true,
        };
    }
    return null;
}

async function readCachedRates(base) {
    let raw;
    try {
        raw = await readCache(CACHE_NAME(base));
    } catch {
        return null;
    }
    if (!raw) return null;
    try {
        const text = typeof raw === "string"
            ? raw
            : new TextDecoder().decode(raw);
        const parsed = JSON.parse(text);
        if (!parsed || typeof parsed !== "object") return null;
        if (typeof parsed.base_code !== "string") return null;
        if (!parsed.rates || typeof parsed.rates !== "object") return null;
        if (typeof parsed.fetched_at !== "number") return null;
        return parsed;
    } catch {
        return null;
    }
}

function cacheIsFresh(cache, cacheSecondsOverride) {
    if (!cache) return false;
    if (cacheSecondsOverride > 0) {
        return Date.now() - cache.fetched_at < cacheSecondsOverride * 1000;
    }
    // Trust the API's next-update timestamp. If it's missing or unparseable
    // fall back to a conservative 1h window so we don't pin a stale snapshot
    // forever.
    if (typeof cache.next_update_ms === "number" && Number.isFinite(cache.next_update_ms)) {
        return Date.now() < cache.next_update_ms;
    }
    return Date.now() - cache.fetched_at < 60 * 60 * 1000;
}

async function fetchRates(base, signal) {
    const res = await get(API_URL(base), { signal });
    if (!res.ok) {
        throw new Error(`open.er-api.com returned HTTP ${res.status}`);
    }
    const data = res.json();
    if (!data || data.result !== "success") {
        throw new Error(`open.er-api.com result=${data && data.result}`);
    }
    if (!data.rates || typeof data.rates !== "object") {
        throw new Error("open.er-api.com response missing rates");
    }
    const fetchedAt = Date.now();
    const nextUpdateMs = data.time_next_update_utc
        ? Date.parse(data.time_next_update_utc)
        : null;
    const lastUpdateMs = data.time_last_update_utc
        ? Date.parse(data.time_last_update_utc)
        : fetchedAt;
    const payload = {
        base_code: data.base_code || base,
        rates: data.rates,
        fetched_at: fetchedAt,
        last_update_ms: Number.isFinite(lastUpdateMs) ? lastUpdateMs : fetchedAt,
        next_update_ms: Number.isFinite(nextUpdateMs) ? nextUpdateMs : null,
    };
    try {
        await writeCache(CACHE_NAME(base), JSON.stringify(payload));
    } catch {
        // Persisting the cache is best-effort; serving the in-flight result
        // is more important than guaranteeing the on-disk snapshot.
    }
    return payload;
}

async function getRates(base, cacheSecondsOverride, signal) {
    const cached = await readCachedRates(base);
    if (cached && cacheIsFresh(cached, cacheSecondsOverride)) {
        return { rates: cached, stale: false };
    }
    try {
        const fresh = await fetchRates(base, signal);
        return { rates: fresh, stale: false };
    } catch (err) {
        if (cached) {
            return { rates: cached, stale: true, error: err };
        }
        return { rates: null, stale: false, error: err };
    }
}

function formatAmount(value, precision) {
    if (!Number.isFinite(value)) return null;
    const fixed = value.toFixed(precision);
    if (precision === 0) return fixed;
    // Trim *only* trailing zeros that don't reduce precision below the cap.
    // We deliberately keep all `precision` digits — users asked for 2 dp,
    // they get 2 dp even if the result is round.
    return fixed;
}

function formatRate(value) {
    if (!Number.isFinite(value)) return null;
    const rounded = Number(value.toPrecision(RATE_PRECISION + 1));
    if (Number.isInteger(rounded)) return String(rounded);
    return String(rounded);
}

function formatRelative(ms) {
    if (!Number.isFinite(ms)) return "";
    const diff = Date.now() - ms;
    if (diff < 0) return "just now";
    const minutes = Math.floor(diff / (60 * 1000));
    if (minutes < 1) return "just now";
    if (minutes < 60) return `${minutes}m ago`;
    const hours = Math.floor(minutes / 60);
    if (hours < 48) return `${hours}h ago`;
    const days = Math.floor(hours / 24);
    return `${days}d ago`;
}

function hintRow(message) {
    return {
        key: `currency:hint`,
        title: "Currency Converter",
        subtitle: message,
        weight: 100,
        pinned: true,
        action: { kind: "noop" },
    };
}

function failureRow(parsed, message) {
    const from = parsed?.from ?? "";
    const to = parsed?.to ?? "";
    return {
        key: `currency:fail:${from}:${to}`,
        title: "Couldn't fetch exchange rates",
        subtitle: message,
        weight: 100,
        pinned: true,
        action: { kind: "noop" },
    };
}

export async function* query(input, signal) {
    if (!input) return;
    const parsed = parseQuery(input);
    if (!parsed) return;

    let target = parsed.to;
    if (parsed.implicitTarget) {
        const home = configuredHomeCurrency();
        if (!home) {
            yield hintRow(
                "Set a home currency in plugin options to enable single-code queries like `100 USD`.",
            );
            return;
        }
        target = home;
    }

    if (parsed.from === target) {
        // No conversion needed. Don't pretend the API was consulted.
        const precision = configuredPrecision();
        const amountText = formatAmount(parsed.amount, precision);
        if (amountText === null) return;
        yield {
            key: `currency:${parsed.from}:${target}`,
            title: `${amountText} ${parsed.from} = ${amountText} ${target}`,
            subtitle: `1 ${parsed.from} = 1 ${target}`,
            weight: 100,
            pinned: true,
            action: copy(amountText),
        };
        return;
    }

    const cacheSecondsOverride = configuredCacheSeconds();
    const { rates, stale, error } = await getRates(parsed.from, cacheSecondsOverride, signal);

    if (signal?.aborted) return;

    if (!rates) {
        yield failureRow(
            parsed,
            error ? String(error.message ?? error) : "network unavailable",
        );
        return;
    }

    const rate = rates.rates[target];
    if (typeof rate !== "number" || !Number.isFinite(rate)) {
        yield failureRow(parsed, `unknown currency code: ${target}`);
        return;
    }

    const converted = parsed.amount * rate;
    const precision = configuredPrecision();
    const convertedText = formatAmount(converted, precision);
    if (convertedText === null) {
        yield failureRow(parsed, "result is not finite");
        return;
    }
    const amountText = formatAmount(parsed.amount, precision);
    const rateText = formatRate(rate);
    const updated = formatRelative(rates.last_update_ms);
    const staleSuffix = stale ? " · rates may be stale" : "";
    const updatedSegment = updated ? ` · updated ${updated}` : "";

    yield {
        key: `currency:${parsed.from}:${target}`,
        title: `${amountText} ${parsed.from} = ${convertedText} ${target}`,
        subtitle: `1 ${parsed.from} = ${rateText} ${target}${updatedSegment}${staleSuffix}`,
        weight: 100,
        pinned: true,
        action: copy(convertedText),
    };
}
