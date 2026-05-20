import { copy } from "highbeam:actions";

// Pure-compute color converter. Accepts hex / rgb(a) / hsl input and yields a
// row per "other format" so the input format isn't echoed back at the user.

// Format families used to filter out the input's own family from the output.
const FAMILY_HEX = "hex";
const FAMILY_RGB = "rgb";
const FAMILY_HSL = "hsl";

const HEX_SHORT = /^#([0-9a-f])([0-9a-f])([0-9a-f])$/i;
const HEX_LONG = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i;
const HEX_LONG_ALPHA = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i;

// Allow integer or decimal channel values inside rgb()/rgba(); range is
// validated after parsing so out-of-range inputs silently drop.
const RGB_RE = /^rgb\(\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)\s*\)$/i;
const RGBA_RE =
    /^rgba\(\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)\s*\)$/i;

// HSL accepts a unitless hue and percent s/l only — `deg/grad/rad/turn` are
// post-v1; reject them for now so we don't silently mis-parse.
const HSL_RE =
    /^hsl\(\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)%\s*,\s*(-?\d+(?:\.\d+)?)%\s*\)$/i;

function parse(input) {
    const src = input.trim();
    if (!src) return null;

    const short = HEX_SHORT.exec(src);
    if (short) {
        const [, r, g, b] = short;
        return {
            family: FAMILY_HEX,
            r: parseInt(r + r, 16),
            g: parseInt(g + g, 16),
            b: parseInt(b + b, 16),
            a: 1,
        };
    }

    const longAlpha = HEX_LONG_ALPHA.exec(src);
    if (longAlpha) {
        const [, r, g, b, a] = longAlpha;
        return {
            family: FAMILY_HEX,
            r: parseInt(r, 16),
            g: parseInt(g, 16),
            b: parseInt(b, 16),
            a: parseInt(a, 16) / 255,
        };
    }

    const long = HEX_LONG.exec(src);
    if (long) {
        const [, r, g, b] = long;
        return {
            family: FAMILY_HEX,
            r: parseInt(r, 16),
            g: parseInt(g, 16),
            b: parseInt(b, 16),
            a: 1,
        };
    }

    const rgba = RGBA_RE.exec(src);
    if (rgba) {
        const r = Number(rgba[1]);
        const g = Number(rgba[2]);
        const b = Number(rgba[3]);
        const a = Number(rgba[4]);
        if (!inByteRange(r) || !inByteRange(g) || !inByteRange(b)) return null;
        if (!Number.isFinite(a) || a < 0 || a > 1) return null;
        return { family: FAMILY_RGB, r: Math.round(r), g: Math.round(g), b: Math.round(b), a };
    }

    const rgb = RGB_RE.exec(src);
    if (rgb) {
        const r = Number(rgb[1]);
        const g = Number(rgb[2]);
        const b = Number(rgb[3]);
        if (!inByteRange(r) || !inByteRange(g) || !inByteRange(b)) return null;
        return { family: FAMILY_RGB, r: Math.round(r), g: Math.round(g), b: Math.round(b), a: 1 };
    }

    const hsl = HSL_RE.exec(src);
    if (hsl) {
        const h = Number(hsl[1]);
        const s = Number(hsl[2]);
        const l = Number(hsl[3]);
        if (!Number.isFinite(h) || h < 0 || h > 360) return null;
        if (!Number.isFinite(s) || s < 0 || s > 100) return null;
        if (!Number.isFinite(l) || l < 0 || l > 100) return null;
        const { r, g, b } = hslToRgb(h, s / 100, l / 100);
        return { family: FAMILY_HSL, r, g, b, a: 1 };
    }

    return null;
}

function inByteRange(v) {
    return Number.isFinite(v) && v >= 0 && v <= 255;
}

// Standard HSL→RGB; returns 0..255 integers. We round once at the boundary so
// downstream formatters don't see fractional channels.
function hslToRgb(hDeg, s, l) {
    const h = ((hDeg % 360) + 360) % 360 / 360;
    if (s === 0) {
        const v = Math.round(l * 255);
        return { r: v, g: v, b: v };
    }
    const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
    const p = 2 * l - q;
    return {
        r: Math.round(hueToRgb(p, q, h + 1 / 3) * 255),
        g: Math.round(hueToRgb(p, q, h) * 255),
        b: Math.round(hueToRgb(p, q, h - 1 / 3) * 255),
    };
}

function hueToRgb(p, q, t) {
    let x = t;
    if (x < 0) x += 1;
    if (x > 1) x -= 1;
    if (x < 1 / 6) return p + (q - p) * 6 * x;
    if (x < 1 / 2) return q;
    if (x < 2 / 3) return p + (q - p) * (2 / 3 - x) * 6;
    return p;
}

// RGB→HSL on 0..255 channels; returns hue in degrees and s/l as 0..1. This
// trip is lossy (e.g. #808080 → hsl(0, 0%, 50%) instead of the input's
// hue), so the round-trip rgb → hsl → rgb won't always match.
function rgbToHsl(r, g, b) {
    const rn = r / 255;
    const gn = g / 255;
    const bn = b / 255;
    const max = Math.max(rn, gn, bn);
    const min = Math.min(rn, gn, bn);
    const l = (max + min) / 2;
    if (max === min) {
        return { h: 0, s: 0, l };
    }
    const d = max - min;
    const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    let h;
    if (max === rn) h = (gn - bn) / d + (gn < bn ? 6 : 0);
    else if (max === gn) h = (bn - rn) / d + 2;
    else h = (rn - gn) / d + 4;
    return { h: h * 60, s, l };
}

function toHex2(n) {
    return n.toString(16).padStart(2, "0");
}

function formatHex(color) {
    return `#${toHex2(color.r)}${toHex2(color.g)}${toHex2(color.b)}`;
}

function formatHex8(color) {
    return `#${toHex2(color.r)}${toHex2(color.g)}${toHex2(color.b)}${toHex2(Math.round(color.a * 255))}`;
}

function formatRgb(color) {
    return `rgb(${color.r}, ${color.g}, ${color.b})`;
}

function formatRgba(color) {
    return `rgba(${color.r}, ${color.g}, ${color.b}, ${formatAlpha(color.a)})`;
}

// HSL is rounded to whole degrees and whole percents; we lose precision here
// versus the float math but match the conventional CSS notation and avoid
// noisy output like `hsl(0.142, 99.6%, 50.001%)`.
function formatHsl(color) {
    const { h, s, l } = rgbToHsl(color.r, color.g, color.b);
    return `hsl(${Math.round(h)}, ${Math.round(s * 100)}%, ${Math.round(l * 100)}%)`;
}

// Trim trailing zeroes so `0.5` doesn't render as `0.500`. Alpha is bounded
// to three decimals; that's enough resolution given hex8 alpha is 1/255 ≈ 0.4%.
function formatAlpha(a) {
    const rounded = Math.round(a * 1000) / 1000;
    return Number.isInteger(rounded) ? rounded.toFixed(1) : String(rounded);
}

// Per-family weights give hex a slight lead, then rgb(a), then hsl. The
// host's pinned-first sort dominates so this is purely a tiebreaker.
function variantsFor(color) {
    const opaque = color.a === 1;
    const out = [];
    if (color.family !== FAMILY_HEX) {
        out.push({
            family: FAMILY_HEX,
            label: opaque ? "HEX" : "HEX (with alpha)",
            text: opaque ? formatHex(color) : formatHex8(color),
            weight: 100,
        });
    }
    if (color.family !== FAMILY_RGB) {
        out.push({
            family: FAMILY_RGB,
            label: opaque ? "RGB" : "RGBA",
            text: opaque ? formatRgb(color) : formatRgba(color),
            weight: 90,
        });
    }
    // Skip HSL when alpha is non-1: we don't emit `hsla(...)` in v1, and
    // dropping the alpha would silently misrepresent the color.
    if (color.family !== FAMILY_HSL && opaque) {
        out.push({
            family: FAMILY_HSL,
            label: "HSL",
            text: formatHsl(color),
            weight: 80,
        });
    }
    return out;
}

export async function* query(input, _signal) {
    if (!input || !input.trim()) return;
    const color = parse(input);
    if (!color) return;
    for (const variant of variantsFor(color)) {
        yield {
            key: `color:${variant.family}:${variant.text}`,
            title: variant.text,
            subtitle: variant.label,
            weight: variant.weight,
            pinned: true,
            action: copy(variant.text),
        };
    }
}
