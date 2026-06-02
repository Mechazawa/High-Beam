# Code-smell cleanup pass — summary

Branch: `claude/cleanup-code-smell-pass-VMQ6l`

## Headline

This codebase is already high quality. It is carefully structured and its
comments overwhelmingly explain genuine "why" — platform quirks (Wayland
`xdg_activation_v1`, macOS focus), `SAFETY` justifications, schema/decay-formula
constraints, and deliberate behavior choices. The cleanup brief is to remove
LLM smell *without* destroying that value, so the honest result is a small,
surgical change set rather than a large sweep.

Every top-level source directory was reviewed (see `CLEANUP_PROGRESS.md`).
Baseline and post-change: `cargo test` green (266 lib + all integration
suites), `cargo clippy --all-targets` clean, emoji-picker vitest 14/14.

## Changes by category

### Over-eager splitting — inlined single-use wrappers (3)
- `src/bundle_install.rs`: removed `bundled_plugins_dir()`, a one-caller
  pass-through to `bundled_resource_dir("plugins")`; inlined at the call site
  (now symmetric with how the themes path resolves its dir).
- `src/sdk/icons.rs`: removed `throw_io()`, a one-caller forwarder to
  `throw_named(.., "IconError", ..)`; inlined. (The analogous per-module
  `throw_*` helpers in clipboard/http/system/fs each have 3+ uses and stay.)
- `plugins/emoji-picker/plugin.js`: removed `bestAliasHay`/`bestTagHay`,
  one-caller helpers whose `length === 0` guards were redundant with
  `[].join(" ")`; inlined as lambdas at the `scoreField` call sites.

### Comments
No comment deletions: every comment inspected was a real "why" / gotcha /
constraint, which the brief says to keep. No commented-out code or
changelog/metadata narration was found.

### Dead code / unused imports
None found. Two survey-reported "unused imports" were verified false:
- `src/app/callbacks.rs` `Model` — required as a trait for `row_count` /
  `set_row_data`; confirmed by a failed compile when removed, then reverted.
- `src/query_history/db.rs` `OptionalExtension` — no such import exists
  (hallucinated).

### Premature abstraction / awkward interfaces
None actioned. Functions stay within sensible param counts; bool params seen
are idiomatic single-flag setters, not behavior selectors. `reveal()` and
`extract_tar()` in `src/plugins/` were left alone: both carry real
platform/security logic parallel to sibling functions, and inlining would
duplicate security-critical comments.

## Rough size

~3 helper functions removed; net ~20 lines of indirection deleted across Rust
and JS. (Diff stat: 5 insertions / ~27 deletions over 3 files.)

## Skipped — needs human review
- **Duplicate `basename` / `basenameWithoutExt` across plugins** (app-launcher,
  prefpanes, file-search, kill-process, obsidian). De-duplicating needs a
  cross-plugin shared-util mechanism; each `plugin.js` is independently
  sandboxed/loaded with no shared import path today. That's an architecture
  change, not a behavior-preserving cleanup.
- **`plugins/view-demo/plugin.js:70`** — `onClick: () => closeView` may pass the
  action object instead of invoking it. Possibly a real behavior bug; fixing it
  would change runtime behavior, so left for a human to confirm intent.

## Directories where tests were missing
- `src/window.rs`, `src/window_wayland.rs`, `src/os_appearance.rs`: no unit
  tests (platform IPC / real threads / windowing) — but these files were
  reviewed and not modified.
- Slint `ui/` files and most JS plugins other than those with a `*.test.js`
  have no automated tests. None of the untested files were modified except
  emoji-picker, which does have a vitest suite (passed).

## Plugin/tooling install status
The `/plugin marketplace` + `/plugin install` steps (karpathy-skills,
code-simplifier) are Claude Code CLI commands and could not be invoked from
this autonomous run. Proceeded by applying the cleanup rules directly.
