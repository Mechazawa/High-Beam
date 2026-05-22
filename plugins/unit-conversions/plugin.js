import { copy } from "highbeam:actions";

// Hand-rolled units table. Each unit declares the category it belongs to plus
// a linear conversion to that category's base unit: `base = value * scale + offset`.
// Only temperature uses a non-zero offset; everything else is a pure ratio.
//
// Base units per category:
//   length      -> meter
//   mass        -> gram
//   temperature -> kelvin
//   volume      -> liter
//   time        -> second
//   data        -> byte
//   area        -> square meter
//
// Lookup keys are case-sensitive on purpose so we can distinguish `K` (kelvin)
// from `k`-prefixes and `KB` (1000) from `kB`. Aliases below add common
// case-insensitive forms where ambiguity is impossible.

const UNITS = {
    // length (base: meter)
    m: { category: "length", scale: 1 },
    cm: { category: "length", scale: 0.01 },
    mm: { category: "length", scale: 0.001 },
    km: { category: "length", scale: 1000 },
    in: { category: "length", scale: 0.0254 },
    ft: { category: "length", scale: 0.3048 },
    yd: { category: "length", scale: 0.9144 },
    mi: { category: "length", scale: 1609.344 },
    nmi: { category: "length", scale: 1852 },

    // mass (base: gram)
    g: { category: "mass", scale: 1 },
    kg: { category: "mass", scale: 1000 },
    mg: { category: "mass", scale: 0.001 },
    lb: { category: "mass", scale: 453.59237 },
    oz: { category: "mass", scale: 28.349523125 },
    t: { category: "mass", scale: 1_000_000 },          // metric ton
    tn: { category: "mass", scale: 907_184.74 },        // US short ton

    // temperature (base: kelvin) — only category with an offset
    K: { category: "temperature", scale: 1, offset: 0 },
    C: { category: "temperature", scale: 1, offset: 273.15 },
    F: { category: "temperature", scale: 5 / 9, offset: 459.67 * (5 / 9) },

    // volume (base: liter)
    L: { category: "volume", scale: 1 },
    mL: { category: "volume", scale: 0.001 },
    gal: { category: "volume", scale: 3.785411784 },    // US gallon
    qt: { category: "volume", scale: 0.946352946 },     // US quart
    pt: { category: "volume", scale: 0.473176473 },     // US pint
    cup: { category: "volume", scale: 0.2365882365 },   // US legal cup
    fl_oz: { category: "volume", scale: 0.0295735295625 }, // US fluid ounce
    m3: { category: "volume", scale: 1000 },

    // time (base: second)
    s: { category: "time", scale: 1 },
    min: { category: "time", scale: 60 },
    hr: { category: "time", scale: 3600 },
    day: { category: "time", scale: 86_400 },
    week: { category: "time", scale: 604_800 },
    month: { category: "time", scale: 2_629_746 },      // average Gregorian month
    year: { category: "time", scale: 31_556_952 },      // average Gregorian year

    // data (base: byte) — SI (×1000) vs IEC (×1024) tracked separately
    B: { category: "data", scale: 1 },
    KB: { category: "data", scale: 1_000 },
    MB: { category: "data", scale: 1_000_000 },
    GB: { category: "data", scale: 1_000_000_000 },
    TB: { category: "data", scale: 1_000_000_000_000 },
    PB: { category: "data", scale: 1_000_000_000_000_000 },
    KiB: { category: "data", scale: 1024 },
    MiB: { category: "data", scale: 1024 ** 2 },
    GiB: { category: "data", scale: 1024 ** 3 },
    TiB: { category: "data", scale: 1024 ** 4 },

    // area (base: square meter)
    m2: { category: "area", scale: 1 },
    km2: { category: "area", scale: 1_000_000 },
    ft2: { category: "area", scale: 0.09290304 },
    mi2: { category: "area", scale: 2_589_988.110336 },
    acre: { category: "area", scale: 4046.8564224 },
    hectare: { category: "area", scale: 10_000 },
};

// Aliases let users type natural variants without polluting the canonical
// table. Each maps to a key in UNITS. Case-sensitive: we add lowercase forms
// only where they don't collide with the canonical key.
const ALIASES = {
    // length
    meter: "m",
    meters: "m",
    metre: "m",
    metres: "m",
    centimeter: "cm",
    centimeters: "cm",
    millimeter: "mm",
    millimeters: "mm",
    kilometer: "km",
    kilometers: "km",
    inch: "in",
    inches: "in",
    foot: "ft",
    feet: "ft",
    yard: "yd",
    yards: "yd",
    mile: "mi",
    miles: "mi",

    // mass
    gram: "g",
    grams: "g",
    kilogram: "kg",
    kilograms: "kg",
    milligram: "mg",
    milligrams: "mg",
    pound: "lb",
    pounds: "lb",
    lbs: "lb",
    ounce: "oz",
    ounces: "oz",
    ton: "t",                  // metric ton, matching `t`
    tonne: "t",
    tonnes: "t",
    tons: "tn",                // plural reads as US shorthand
    "short_ton": "tn",

    // temperature
    c: "C",
    f: "F",
    k: "K",
    celsius: "C",
    centigrade: "C",
    fahrenheit: "F",
    kelvin: "K",

    // volume
    l: "L",
    ml: "mL",
    liter: "L",
    liters: "L",
    litre: "L",
    litres: "L",
    milliliter: "mL",
    milliliters: "mL",
    millilitre: "mL",
    millilitres: "mL",
    gallon: "gal",
    gallons: "gal",
    gals: "gal",
    quart: "qt",
    quarts: "qt",
    pint: "pt",
    pints: "pt",
    cups: "cup",
    floz: "fl_oz",
    "fl.oz": "fl_oz",
    "fl-oz": "fl_oz",

    // time
    sec: "s",
    secs: "s",
    second: "s",
    seconds: "s",
    minute: "min",
    minutes: "min",
    mins: "min",
    h: "hr",
    hour: "hr",
    hours: "hr",
    hrs: "hr",
    days: "day",
    d: "day",
    weeks: "week",
    w: "week",
    months: "month",
    mo: "month",
    years: "year",
    y: "year",
    yr: "year",
    yrs: "year",

    // data
    bytes: "B",
    byte: "B",
    kb: "KB",
    mb: "MB",
    gb: "GB",
    tb: "TB",
    pb: "PB",
    kib: "KiB",
    mib: "MiB",
    gib: "GiB",
    tib: "TiB",

    // area
    "m^2": "m2",
    "km^2": "km2",
    "ft^2": "ft2",
    "mi^2": "mi2",
    sqm: "m2",
    sqkm: "km2",
    sqft: "ft2",
    sqmi: "mi2",
    ha: "hectare",
    acres: "acre",
    hectares: "hectare",
};

function resolveUnit(token) {
    if (UNITS[token]) return token;
    const lower = token.toLowerCase();
    if (ALIASES[lower]) return ALIASES[lower];
    if (ALIASES[token]) return ALIASES[token];
    return null;
}

// `<number> <unit> [to] <unit>`. Whitespace between the number and the
// source unit is optional (`5kg to g`), and the literal `to` is optional
// (`5 kg g` parses the same as `5 kg to g`) — units always sit on either
// side of the source-to-target whitespace so dropping `to` is unambiguous.
const QUERY_RE = /^\s*(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)\s*(\S+?)\s+(?:to\s+)?(\S+)\s*$/;

function parseQuery(input) {
    const match = QUERY_RE.exec(input);
    if (!match) return null;
    const value = Number(match[1]);
    if (!Number.isFinite(value)) return null;
    const fromKey = resolveUnit(match[2]);
    const toKey = resolveUnit(match[3]);
    if (!fromKey || !toKey) return null;
    const from = UNITS[fromKey];
    const to = UNITS[toKey];
    if (from.category !== to.category) return null;
    return { value, fromKey, toKey, from, to };
}

function convert({ value, from, to }) {
    // For temperature: base = value * from.scale + from.offset (kelvin),
    // then result = (base - to.offset) / to.scale.
    // For everything else the offsets are 0 so this collapses to a ratio.
    const fromOffset = from.offset ?? 0;
    const toOffset = to.offset ?? 0;
    const base = value * from.scale + fromOffset;
    return (base - toOffset) / to.scale;
}

// Up to 6 significant digits, trailing zeros trimmed. Integers render bare.
// Very large or very small magnitudes fall back to exponential form so we
// don't surface walls of zeros for things like `1 PB to B`.
function formatNumber(value) {
    if (!Number.isFinite(value)) return null;
    if (value === 0) return "0";
    const abs = Math.abs(value);
    if (abs >= 1e15 || abs < 1e-4) {
        return Number(value.toPrecision(6))
            .toExponential()
            .replace(/e\+?(-?)0*(\d)/, "e$1$2");
    }
    // toPrecision then Number() trims insignificant trailing zeros without
    // dragging us into exponential form for ordinary magnitudes.
    const rounded = Number(value.toPrecision(6));
    if (Number.isInteger(rounded)) return String(rounded);
    return String(rounded);
}

// Render display unit for the result row — `m3` is rare to type but ugly to
// read; same for `m2`/`km2`/`ft2`/`mi2`. The map below replaces them with the
// superscript-2/3 forms. Everything else passes through.
const DISPLAY_UNIT = {
    m2: "m²",
    km2: "km²",
    ft2: "ft²",
    mi2: "mi²",
    m3: "m³",
    fl_oz: "fl oz",
};

function displayUnit(key) {
    return DISPLAY_UNIT[key] ?? key;
}

export async function* query(input, _signal) {
    if (!input || !input.trim()) return;
    const parsed = parseQuery(input);
    if (!parsed) return;
    const result = convert(parsed);
    const text = formatNumber(result);
    if (text === null) return;
    const unit = displayUnit(parsed.toKey);
    const title = `${text} ${unit}`;
    yield {
        key: `unit:${parsed.value}:${parsed.fromKey}:${parsed.toKey}`,
        title,
        // Subtitle echoes the normalised input so the row is unambiguous when
        // multiple plugins surface results for the same query.
        subtitle: input.trim(),
        weight: 100,
        pinned: true,
        // Copies just the numeric portion — that's what users overwhelmingly
        // want to paste into spreadsheets, code, and other text fields.
        action: copy(text),
    };
}
