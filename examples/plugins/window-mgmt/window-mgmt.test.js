import { describe, expect, test, vi } from "vitest";

// Force macOS for every test. The platform stub exports plain consts/fns;
// replacing the whole module gives us a vi.fn() we can override per case.
vi.mock("highbeam:platform", () => ({
    isMacOS: vi.fn(() => true),
    isLinux: vi.fn(() => false),
    os: "macos",
    arch: "x86_64",
    version: "test",
}));

async function collect(iter) {
    const out = [];
    for await (const item of iter) out.push(item);
    return out;
}

async function loadPlugin({ macOS = true } = {}) {
    vi.resetModules();
    const platformMod = await import("highbeam:platform");
    vi.mocked(platformMod.isMacOS).mockReturnValue(macOS);
    vi.mocked(platformMod.isLinux).mockReturnValue(!macOS);
    return import("./plugin.js");
}

async function run(input, opts) {
    const { query } = await loadPlugin(opts);
    return collect(query(input, { aborted: false }));
}

function findByTitle(results, title) {
    return results.find((r) => r.title === title);
}

describe("window-mgmt verbs", () => {
    test("`left half` produces one osascript exec action", async () => {
        const results = await run("left half");
        const row = findByTitle(results, "left half");
        expect(row).toBeDefined();
        expect(row.weight).toBe(90);
        expect(row.pinned).toBe(false);
        expect(row.subtitle).toMatch(/left half/i);
        expect(row.action.kind).toBe("exec");
        expect(row.action.cmd).toBe("osascript");
        expect(row.action.args[0]).toBe("-e");
        const script = row.action.args[1];
        // Sanity-check the script wires the right pieces together, without
        // pinning the exact whitespace.
        expect(script).toContain("set size of theWindow");
        expect(script).toContain("set position of theWindow");
        expect(script).toContain("screenW / 2");
        expect(script).toContain("first application process whose frontmost is true");
    });

    test("`maximize` yields a single full-screen verb row", async () => {
        const results = await run("maximize");
        const titles = results.map((r) => r.title);
        // Only `maximize` itself prefix-matches `maximize` — no other verb
        // shares that prefix.
        expect(titles).toEqual(["maximize"]);
        const [row] = results;
        const script = row.action.args[1];
        expect(script).toContain("set size of theWindow to {screenW, screenH}");
        expect(script).toContain("set position of theWindow to {screenX, screenY}");
    });

    test("`top-left` quarter sets the right fractions", async () => {
        const results = await run("top-left");
        const row = findByTitle(results, "top-left");
        expect(row).toBeDefined();
        const script = row.action.args[1];
        expect(script).toContain("set position of theWindow to {screenX, screenY}");
        expect(script).toContain("set size of theWindow to {screenW / 2, screenH / 2}");
    });

    test("`full screen` alias prefix-matches the maximize verb", async () => {
        const results = await run("full screen");
        const titles = results.map((r) => r.title);
        expect(titles).toEqual(["maximize"]);
    });

    test("case-insensitive prefix: `LEFT` surfaces `left half`", async () => {
        const results = await run("LEFT");
        const titles = results.map((r) => r.title);
        expect(titles).toContain("left half");
    });

    test("`next display` is a special verb without size set", async () => {
        const results = await run("next display");
        const row = findByTitle(results, "next display");
        expect(row).toBeDefined();
        const script = row.action.args[1];
        // It only nudges position; size stays as the window has it.
        expect(script).toContain("set position of theWindow");
        expect(script).not.toContain("set size of theWindow");
    });
});

describe("window-mgmt no-match cases", () => {
    test("non-matching query returns zero results", async () => {
        const results = await run("hello");
        expect(results).toEqual([]);
    });

    test("empty input returns zero results", async () => {
        expect(await run("")).toEqual([]);
        expect(await run("   ")).toEqual([]);
    });

    test("returns zero results when not on macOS", async () => {
        const results = await run("maximize", { macOS: false });
        expect(results).toEqual([]);
    });
});
