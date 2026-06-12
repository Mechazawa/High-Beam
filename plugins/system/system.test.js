import { describe, expect, test, vi } from "vitest";

// Mock node:os so each test can pick the platform. The `highbeam:settings`
// alias resolves to the SDK stub whose getBool returns undefined by default
// (treated as "enabled"); tests that want a verb off mock it to false.
vi.mock("node:os", () => {
    const platform = vi.fn(() => "darwin");
    return { default: { platform }, platform };
});

async function collect(iter) {
    const out = [];
    for await (const item of iter) out.push(item);
    return out;
}

async function load({ platform = "darwin", disabled = [] } = {}) {
    vi.resetModules();
    vi.mocked((await import("node:os")).default.platform).mockReturnValue(platform);
    const settings = await import("highbeam:settings");
    vi.mocked(settings.getBool).mockImplementation((key) => (disabled.includes(key) ? false : undefined));
    const { query } = await import("./plugin.js");
    return query;
}

async function run(input, opts) {
    const query = await load(opts);
    return collect(query(input, { aborted: false }));
}

const findByTitle = (rows, title) => rows.find((r) => r.title === title);

describe("system verbs", () => {
    test("macOS `shutdown` runs the Finder shut-down AppleScript", async () => {
        const row = findByTitle(await run("shutdown"), "shutdown");
        expect(row).toBeDefined();
        expect(row.key).toBe("system:shutdown");
        expect(row.weight).toBe(100);
        expect(row.pinned).toBe(false);
        expect(row.action.kind).toBe("exec");
        expect(row.action.cmd).toBe("osascript");
        expect(row.action.args[0]).toBe("-e");
        expect(row.action.args[1]).toContain("shut down");
    });

    test("Linux `shutdown` runs systemctl poweroff", async () => {
        const row = findByTitle(await run("shutdown", { platform: "linux" }), "shutdown");
        expect(row.action).toEqual({ kind: "exec", cmd: "systemctl", args: ["poweroff"] });
    });

    test("partial prefix scores proportionally", async () => {
        // "sh" only prefix-matches "shutdown" → 2/8 * 100.
        const row = findByTitle(await run("sh"), "shutdown");
        expect(row).toBeDefined();
        expect(row.weight).toBeCloseTo(25);
    });

    test("case-insensitive: `SHUT` surfaces shutdown", async () => {
        const titles = (await run("SHUT")).map((r) => r.title);
        expect(titles).toContain("shutdown");
    });

    test("`re` surfaces both restart and reboot", async () => {
        const titles = (await run("re")).map((r) => r.title).sort();
        expect(titles).toEqual(["reboot", "restart"]);
    });

    test("`log out` matches the full phrase", async () => {
        const row = findByTitle(await run("log out"), "log out");
        expect(row).toBeDefined();
        expect(row.action.kind).toBe("exec");
    });

    test("eject shows on macOS", async () => {
        const row = findByTitle(await run("eject"), "eject");
        expect(row).toBeDefined();
        expect(row.action.cmd).toBe("osascript");
    });

    test("eject is absent on Linux (no bare-verb command)", async () => {
        const titles = (await run("eject", { platform: "linux" })).map((r) => r.title);
        expect(titles).not.toContain("eject");
    });
});

describe("system option gating", () => {
    test("a verb disabled in settings is omitted", async () => {
        expect(await run("shutdown", { disabled: ["enableShutdown"] })).toEqual([]);
    });

    test("disabling enableRestart hides both restart and reboot", async () => {
        expect(await run("re", { disabled: ["enableRestart"] })).toEqual([]);
    });
});

describe("system no-match cases", () => {
    test("empty / whitespace input yields nothing", async () => {
        expect(await run("")).toEqual([]);
        expect(await run("   ")).toEqual([]);
    });

    test("unrelated query yields nothing", async () => {
        expect(await run("xyzzy")).toEqual([]);
    });
});
