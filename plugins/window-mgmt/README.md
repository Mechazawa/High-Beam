# window-mgmt

Snap the frontmost window to common positions on the **main display** by
typing a verb into High Beam: `left half`, `right half`, `top half`,
`bottom half`, `maximize` (alias `full screen`), `center`, `top-left`,
`top-right`, `bottom-left`, `bottom-right`, `next display`.

macOS only. Each result runs `osascript -e <script>` via
`actions.exec(...)`, so the only capability required is `actions`.

## How it works

The plugin streams one row per verb whose canonical name (or alias) prefix-
matches the (case-insensitive) input. Pressing Enter fires AppleScript that:

1. Reads the main display's bounds from `Finder` (`bounds of window of
   desktop`), which conveniently excludes the menu bar.
2. Finds the frontmost application process via `System Events`.
3. Sets `position` and `size` of its first window to a fraction of the
   display bounds (left half → `{screenW / 2, screenH}`, etc.).

The first time the action runs on a fresh user account, macOS prompts for
**Accessibility** permission for the host binary — this is required for
`System Events` to move other apps' windows. AppleScript fails silently
without it.

## Known limitations (v1)

- **Main display only.** All sizing math runs against `Finder`'s desktop
  bounds, which always reports the primary display. A window already on a
  second monitor will be yanked back to the main one when you snap it.
- **`next display` is a fixed nudge.** It shifts the window by one
  primary-display width along X. That works for the common case of two
  monitors arranged horizontally, but it's not display-aware — enumerating
  the actual display layout from AppleScript requires shelling out to
  `system_profiler SPDisplaysDataType` or calling private APIs, which is
  out of scope for v1.
- **No fullscreen toggle.** `maximize` resizes to the full menu-bar-aware
  bounds; it does not enter macOS Spaces fullscreen mode.
- **AppleScript window control is fragile.** Apps that draw their own
  decorations (Chrome's PWAs, some Electron variants, Spotify) can refuse
  to be resized, or report a position offset by their shadow inset. There's
  no portable workaround — that's a constraint of the Accessibility APIs
  System Events sits on top of.
