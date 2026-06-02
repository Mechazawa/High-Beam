# Cleanup: code-smell pass

Branch: `claude/cleanup-code-smell-pass-VMQ6l`

Removing LLM-generated code smell (redundant comments, over-splitting,
premature abstraction, awkward interfaces, dead code). No runtime behavior
changes — only how behavior is expressed.

Baseline: `cargo test` all green (266 lib + integration). `cargo clippy
--all-targets` clean.

Tooling note: the `/plugin` marketplace installs (karpathy-skills,
code-simplifier) are Claude Code CLI commands that can't be invoked from this
autonomous run; proceeded with the rules directly.

Overall finding: this is a high-quality, carefully-commented codebase. The vast
majority of comments explain a genuine "why" (Wayland/macOS quirks, SAFETY
justifications, schema constraints) and were preserved. Genuine smell was
sparse; changes are correspondingly small and surgical.

## Directories

- [x] src (root-level .rs files) — inlined single-use `bundled_plugins_dir`
      wrapper in bundle_install.rs. Other root files (lib/cli/logging/main/
      os_appearance/paths/window_wayland) reviewed, already clean.
- [x] src/app — reviewed, clean. (Reported "unused `Model` import" was a false
      positive: the trait is needed for `row_count`/`set_row_data`. Verified by
      compile.)
- [x] src/frecency — reviewed, clean.
- [x] src/query_history — reviewed, clean. (Reported "unused OptionalExtension
      import" was a hallucination — no such import exists.)
- [x] src/sdk — inlined single-use `throw_io` wrapper in icons.rs. Per-module
      `throw_*` wrappers in clipboard/http/system/fs kept (3+ uses each).
- [x] src/plugins — reviewed, clean. `reveal()` and `extract_tar()` left as-is:
      both carry real platform/security logic parallel to siblings; inlining
      would duplicate security-critical comments.
- [x] src/ui — reviewed, clean.
- [x] src/views — reviewed, clean.
- [x] ui (slint) — reviewed, clean.
- [x] sdk/highbeam (TS/JS SDK) — reviewed. platform.js detect* helpers left as
      named functions (the try/catch in detectVersion reads clearer named; tiny
      public SDK shim). http.js emptyResponse kept (6 uses).
- [x] tests — reviewed, clean. Test helpers all have 3+ callers.
- [x] plugins (JS plugins) — inlined single-use `bestAliasHay`/`bestTagHay`
      wrappers in emoji-picker (their length guards were redundant with
      `[].join`).

## Skipped — needs human review

- `plugins/*/plugin.js` duplicate `basename` / `basenameWithoutExt` helpers
  (app-launcher, prefpanes, file-search, kill-process, obsidian). De-duplicating
  would require a cross-plugin shared-utility mechanism; each plugin.js is an
  independently sandboxed/loaded file with no shared import path today. That is
  an architecture change, not a behavior-preserving cleanup.
- `plugins/view-demo/plugin.js:70` — a survey flagged `onClick: () => closeView`
  as possibly passing the action object instead of invoking it. This may be a
  real behavior bug; changing it would alter runtime behavior, so left for a
  human to confirm intent.
