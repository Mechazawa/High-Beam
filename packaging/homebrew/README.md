# Homebrew distribution

High Beam is a GUI `.app` shipped as a `.dmg`, so on Homebrew it is a
**cask**, not a formula (`homebrew/core` does not accept GUI apps).

We distribute through a **personal tap** rather than the official
`homebrew/cask` repo. The official repo auto-rejects new casks that miss
its notability bar (≥ 75 GitHub stars, or ≥ 30 forks/watchers) and that
aren't Apple-notarized — neither of which High Beam clears yet. A tap has
no such gate, so users can install today:

```sh
brew install --cask mechazawa/high-beam/high-beam
```

## Layout

`Casks/high-beam.rb` here is the source of truth, edited in-tree
alongside the code it packages (same convention as `packaging/aur/`). The
live tap is a **separate GitHub repo** Homebrew clones on `brew tap`:

```
Mechazawa/homebrew-high-beam     # the tap repo (note the homebrew- prefix)
└── Casks/
    └── high-beam.rb             # a copy of this file
```

The `homebrew-` prefix is mandatory — `brew tap mechazawa/high-beam`
expands to `github.com/Mechazawa/homebrew-high-beam`.

## First-time setup (once)

1. Create the public repo `Mechazawa/homebrew-high-beam` on GitHub.
2. Add a repo secret on **Mechazawa/High-Beam** named `HOMEBREW_TAP_TOKEN`
   — a PAT (fine-grained, `contents: write` on `homebrew-high-beam`, or a
   classic `repo`-scoped token). This is what lets the release workflow
   push cask bumps into the tap.
3. Seed the tap once so `brew tap` has something to read — either copy
   `Casks/high-beam.rb` in by hand, or just cut one stable release and
   let the workflow create it.
4. Verify end-to-end:

   ```sh
   brew tap mechazawa/high-beam
   brew install --cask high-beam
   brew uninstall --cask high-beam
   ```

## Per-release update — automated

`version` + `sha256` are bumped for you. On every **stable** `vX.Y.Z`
tag, `.github/workflows/release.yml` (step "Update Homebrew tap cask"):

1. Builds `HighBeam_<version>_universal.dmg` via `just bundle-universal`.
2. Computes its `sha256`.
3. Copies this template into the tap repo, pins `version` + `sha256`, and
   commits/pushes `high-beam <version>` to `Mechazawa/homebrew-high-beam`.

Pre-release tags (`-rc`/`-beta`/`-alpha`) and a missing `HOMEBREW_TAP_TOKEN`
both skip the step — no manual sha edit in the normal path.

The in-repo `Casks/high-beam.rb` stays a **template** with placeholder
`version`/`sha256`; edit it only for the hand-maintained fields
(`caveats`, `zap`, `livecheck`, `desc`) — those propagate to the tap on
the next release.

### Bumping by hand (fallback)

If you ever need to bump without a tagged release:

```sh
shasum -a 256 HighBeam_<version>_universal.dmg
# set version + sha256 in the tap repo's Casks/high-beam.rb, commit, push
```

`brew livecheck high-beam` reads GitHub Releases and reports when a newer
tag exists.

## Before validating with `brew audit`

```sh
brew audit --cask --online mechazawa/high-beam/high-beam
brew style mechazawa/high-beam/high-beam
```

## Path to the official homebrew/cask repo

Two upstream gates remain before a `homebrew/cask` PR would be accepted:

- **Notability** — the GitHub repo needs ≥ 75 stars (or ≥ 30 forks /
  watchers). Tracked by traction, not code.
- **Notarization** — replace the self-signed identity with an Apple
  Developer ID cert and add an `xcrun notarytool submit` + `stapler`
  step to `release.yml`, so the cask no longer needs the
  `xattr -dr com.apple.quarantine` caveat. cargo-packager wraps
  notarytool directly when `APPLE_*` credentials are present (see
  `docs/distribution.md` § Signing).
