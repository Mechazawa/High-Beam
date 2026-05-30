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
2. Copy `Casks/high-beam.rb` into its `Casks/` directory and push.
3. Verify end-to-end:

   ```sh
   brew tap mechazawa/high-beam
   brew install --cask high-beam
   brew uninstall --cask high-beam
   ```

## Per-release update

The cask pins a universal `.dmg` and its SHA-256, so both move every
release:

1. Cut the release as usual (push a `vX.Y.Z` tag). The macOS job runs
   `just bundle-universal`, which lipo-joins the arm64 + x86_64 binaries
   into one universal2 `.app`, packages `HighBeam_<version>_universal.dmg`,
   and uploads it to the GitHub Release.
2. Grab the digest — `just bundle-universal` prints it, or:

   ```sh
   shasum -a 256 HighBeam_<version>_universal.dmg
   ```

3. Bump `version` and `sha256` in `Casks/high-beam.rb` here, then mirror
   the file into the tap repo and push.

`brew livecheck high-beam` reads GitHub Releases and reports when a newer
tag exists, so the bump can be scripted later if desired.

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
