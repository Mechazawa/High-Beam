# system

Type a power verb into High Beam: `shutdown`, `sleep`, `restart` (or
`reboot`), `lock`, `log out`, `screensaver`, `display sleep`, `empty trash`,
`eject` (macOS only). Each runs one command via `actions.exec(...)`, so the
only capability needed is `actions`.

## Toggling verbs

Each verb is a `bool` option (all default on). Flip one off in
**Settings → Plugins → System** to drop it from results; disabling the whole
plugin hides every verb.

## Platform commands

| Verb | macOS | Linux |
| --- | --- | --- |
| shutdown | `osascript` (Finder) | `systemctl poweroff` |
| sleep | `osascript` (Finder) | `systemctl suspend` |
| restart / reboot | `osascript` (Finder) | `systemctl reboot` |
| lock | `osascript` (System Events) | `loginctl lock-session` |
| log out | `osascript` (System Events) | `loginctl terminate-session` / `kill-session self` |
| screensaver | `open -a ScreenSaverEngine` | `xdg-screensaver activate` |
| display sleep | `pmset displaysleepnow` | `xset dpms force off` |
| empty trash | `osascript` (Finder) | `gio trash --empty` |
| eject | `osascript` (Finder) | n/a |

`eject` has no Linux row: `gio mount --eject` needs a target device, so the
bare verb has no equivalent.
