// Window-management plugin — types a verb like `left half` or `maximize` and
// gets back one row whose action runs an AppleScript snippet that moves and
// resizes the frontmost window of the frontmost application.
//
// macOS only. The script always targets the main display (whatever `Finder`
// reports as `bounds of window of desktop`); multi-display layouts beyond
// "next display" are out of scope for v1 — see README.md.

import { exec } from "highbeam:actions";
import os from "node:os";
const isMacOS = () => os.platform() === "darwin";

const WEIGHT = 90;

// Each layout returns the AppleScript fragment that computes the target
// position and size in terms of `screenX`, `screenY`, `screenW`, `screenH` —
// the bounds variables the wrapper script defines before calling in.
//
// `position` is the top-left of the window; `size` is `{width, height}`. We
// keep all arithmetic in AppleScript so the fractions stay exact at the
// runtime display's actual resolution rather than whatever Node thinks it is.
const HALF_LAYOUTS = {
    "left half": {
        subtitle: "Move active window to the left half",
        position: "{screenX, screenY}",
        size: "{screenW / 2, screenH}",
    },
    "right half": {
        subtitle: "Move active window to the right half",
        position: "{screenX + screenW / 2, screenY}",
        size: "{screenW / 2, screenH}",
    },
    "top half": {
        subtitle: "Move active window to the top half",
        position: "{screenX, screenY}",
        size: "{screenW, screenH / 2}",
    },
    "bottom half": {
        subtitle: "Move active window to the bottom half",
        position: "{screenX, screenY + screenH / 2}",
        size: "{screenW, screenH / 2}",
    },
};

const QUARTER_LAYOUTS = {
    "top-left": {
        subtitle: "Move active window to the top-left quarter",
        position: "{screenX, screenY}",
        size: "{screenW / 2, screenH / 2}",
    },
    "top-right": {
        subtitle: "Move active window to the top-right quarter",
        position: "{screenX + screenW / 2, screenY}",
        size: "{screenW / 2, screenH / 2}",
    },
    "bottom-left": {
        subtitle: "Move active window to the bottom-left quarter",
        position: "{screenX, screenY + screenH / 2}",
        size: "{screenW / 2, screenH / 2}",
    },
    "bottom-right": {
        subtitle: "Move active window to the bottom-right quarter",
        position: "{screenX + screenW / 2, screenY + screenH / 2}",
        size: "{screenW / 2, screenH / 2}",
    },
};

const FULL_LAYOUT = {
    subtitle: "Maximize active window on the current display",
    position: "{screenX, screenY}",
    size: "{screenW, screenH}",
};

const CENTER_LAYOUT = {
    subtitle: "Center active window at half the display's dimensions",
    // Half-size, centered on the display.
    position: "{screenX + screenW / 4, screenY + screenH / 4}",
    size: "{screenW / 2, screenH / 2}",
};

// Verb table — order is the display order when nothing's been typed. Each
// entry has the canonical title (what we render + what prefix-match runs
// against), the layout descriptor, and any extra aliases that also prefix-
// match the same verb.
const VERBS = [
    { title: "left half", layout: HALF_LAYOUTS["left half"] },
    { title: "right half", layout: HALF_LAYOUTS["right half"] },
    { title: "top half", layout: HALF_LAYOUTS["top half"] },
    { title: "bottom half", layout: HALF_LAYOUTS["bottom half"] },
    {
        title: "maximize",
        layout: FULL_LAYOUT,
        aliases: ["full screen", "full-screen"],
    },
    { title: "center", layout: CENTER_LAYOUT },
    { title: "top-left", layout: QUARTER_LAYOUTS["top-left"] },
    { title: "top-right", layout: QUARTER_LAYOUTS["top-right"] },
    { title: "bottom-left", layout: QUARTER_LAYOUTS["bottom-left"] },
    { title: "bottom-right", layout: QUARTER_LAYOUTS["bottom-right"] },
    { title: "next display" },
];

// AppleScript wrapper that resolves the main display's bounds from Finder,
// then sets the frontmost window's position + size from `<POS>` / `<SIZE>`.
// Finder's desktop bounds reliably exclude the menu bar; System Events
// doesn't expose screen geometry without poking accessibility APIs that
// require entitlement nudges we'd rather avoid in v1.
function layoutScript(layout) {
    return [
        'tell application "Finder"',
        "    set screenBounds to bounds of window of desktop",
        "end tell",
        "set screenX to item 1 of screenBounds",
        "set screenY to item 2 of screenBounds",
        "set screenW to (item 3 of screenBounds) - screenX",
        "set screenH to (item 4 of screenBounds) - screenY",
        'tell application "System Events"',
        "    set frontApp to first application process whose frontmost is true",
        "    set theWindow to first window of frontApp",
        `    set position of theWindow to ${layout.position}`,
        `    set size of theWindow to ${layout.size}`,
        "end tell",
    ].join("\n");
}

// "Next display" can't query the OS display list from AppleScript without
// shelling out, so we approximate: shift the front window by one screen
// width along X, keep Y, and let the OS clamp into the next display's
// usable area. Works for the common horizontal dual-monitor case; a future
// improvement is to enumerate displays via `system_profiler` and pick the
// nearest centre.
function nextDisplayScript() {
    return [
        'tell application "Finder"',
        "    set screenBounds to bounds of window of desktop",
        "end tell",
        "set screenW to (item 3 of screenBounds) - (item 1 of screenBounds)",
        'tell application "System Events"',
        "    set frontApp to first application process whose frontmost is true",
        "    set theWindow to first window of frontApp",
        "    set {curX, curY} to position of theWindow",
        "    set position of theWindow to {curX + screenW, curY}",
        "end tell",
    ].join("\n");
}

function scriptFor(verb) {
    if (verb.title === "next display") return nextDisplayScript();
    return layoutScript(verb.layout);
}

// A verb matches if `input` is a case-insensitive prefix of either its
// canonical title or any alias.
function matchesPrefix(verb, normalizedInput) {
    if (verb.title.startsWith(normalizedInput)) return true;

    const aliases = verb.aliases ?? [];
    return aliases.some((alias) => alias.startsWith(normalizedInput));
}

function subtitleFor(verb) {
    if (verb.title === "next display") {
        return "Shift active window one screen-width right (next display)";
    }
    return verb.layout.subtitle;
}

export async function* query(input, _signal) {
    if (!isMacOS()) return;

    const trimmed = input?.trim();
    if (!trimmed) return;

    const normalized = trimmed.toLowerCase();
    const matches = VERBS.filter((verb) => matchesPrefix(verb, normalized));

    for (const verb of matches) {
        yield {
            key: `window-mgmt:${verb.title}`,
            title: verb.title,
            subtitle: subtitleFor(verb),
            weight: WEIGHT,
            pinned: false,
            action: exec("osascript", ["-e", scriptFor(verb)]),
        };
    }
}
