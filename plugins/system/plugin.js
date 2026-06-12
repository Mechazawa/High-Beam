// System power verbs (shutdown, sleep, restart, lock, …), one row each.
// Each verb is individually toggleable in Settings: `getBool` gates it,
// defaulting to on.

import { exec } from "highbeam:actions";
import { getBool } from "highbeam:settings";
import os from "node:os";

// The host only runs on macOS and Linux; anything that isn't Darwin takes the
// Linux command path.
const currentPlatform = () => (os.platform() === "darwin" ? "macos" : "linux");

// macOS AppleScript one-liner: `osascript -e <script>`.
const osa = (script) => ["osascript", ["-e", script]];

// Each verb: canonical title (rendered + prefix-matched), the Settings option
// that gates it, a subtitle, and the command per platform (`null` where the
// verb has no equivalent).
const VERBS = [
    {
        title: "shutdown",
        option: "enableShutdown",
        subtitle: "shut down this computer",
        cmd: { macos: osa('tell application "Finder" to shut down'), linux: ["systemctl", ["poweroff"]] },
    },
    {
        title: "sleep",
        option: "enableSleep",
        subtitle: "put this computer to sleep",
        cmd: { macos: osa('tell application "Finder" to sleep'), linux: ["systemctl", ["suspend"]] },
    },
    {
        title: "restart",
        option: "enableRestart",
        subtitle: "restart this computer",
        cmd: { macos: osa('tell application "Finder" to restart'), linux: ["systemctl", ["reboot"]] },
    },
    // Same action and toggle as `restart`; a separate row so typing `reboot`
    // still hits it.
    {
        title: "reboot",
        option: "enableRestart",
        subtitle: "restart this computer",
        cmd: { macos: osa('tell application "Finder" to restart'), linux: ["systemctl", ["reboot"]] },
    },
    {
        title: "lock",
        option: "enableLock",
        subtitle: "lock the screen",
        cmd: {
            macos: osa('tell application "System Events" to keystroke "q" using {control down, command down}'),
            linux: ["loginctl", ["lock-session"]],
        },
    },
    {
        title: "log out",
        option: "enableLogout",
        subtitle: "end this user session",
        cmd: {
            macos: osa('tell application "System Events" to log out'),
            // `terminate-session` needs a session id; fall back to
            // `kill-session self` when $XDG_SESSION_ID isn't set.
            linux: [
                "sh",
                ['-c', 'if [ -n "$XDG_SESSION_ID" ]; then loginctl terminate-session "$XDG_SESSION_ID"; else loginctl kill-session self; fi'],
            ],
        },
    },
    {
        title: "screensaver",
        option: "enableScreensaver",
        subtitle: "start the screensaver",
        cmd: { macos: ["open", ["-a", "ScreenSaverEngine"]], linux: ["xdg-screensaver", ["activate"]] },
    },
    {
        title: "display sleep",
        option: "enableDisplaySleep",
        subtitle: "turn the display off without sleeping the machine",
        cmd: { macos: ["pmset", ["displaysleepnow"]], linux: ["xset", ["dpms", "force", "off"]] },
    },
    {
        title: "empty trash",
        option: "enableEmptyTrash",
        subtitle: "permanently delete trashed files",
        cmd: { macos: osa('tell application "Finder" to empty trash'), linux: ["gio", ["trash", "--empty"]] },
    },
    {
        title: "eject",
        option: "enableEject",
        subtitle: "eject all ejectable disks",
        cmd: { macos: osa('tell application "Finder" to eject (every disk whose ejectable is true)'), linux: null },
    },
];

// A verb is on unless explicitly toggled off: an unset option (`undefined`)
// means default, which the manifest declares as `true`.
const isEnabled = (verb) => getBool(verb.option) !== false;

// Case-insensitive prefix match, scored by how much of the title was typed
// (`queryLen / titleLen`, capped at 1).
function matchWeight(title, normalizedInput) {
    if (!title.startsWith(normalizedInput)) return null;
    return Math.min(normalizedInput.length / title.length, 1) * 100;
}

export async function* query(input, _signal) {
    const trimmed = input?.trim();
    if (!trimmed) return;

    const normalized = trimmed.toLowerCase();
    const platform = currentPlatform();

    const rows = VERBS
        .map((verb) => ({ verb, cmd: verb.cmd[platform], weight: matchWeight(verb.title, normalized) }))
        .filter(({ verb, cmd, weight }) => cmd && weight !== null && isEnabled(verb));

    for (const { verb, cmd, weight } of rows) {
        const [command, args] = cmd;
        yield {
            key: `system:${verb.title}`,
            title: verb.title,
            subtitle: verb.subtitle,
            weight,
            pinned: false,
            action: exec(command, args),
        };
    }
}
