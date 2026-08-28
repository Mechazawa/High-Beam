# Distribution

This page covers distributing the High Beam app binary. For shipping a
**plugin** to other people's running launchers — `install <manifestUrl>`
+ `update` — see
[plugin-authoring.md § Publishing + distribution](./plugin-authoring.md#publishing--distribution).
Plugin distribution is HTTP-only and independent of the app bundle: any
running High Beam, however installed, can `install` a hosted plugin.

`just bundle` produces two artifacts under `target/release/`:

- `HighBeam.app` — the macOS app bundle, self-signed
- `HighBeam_<version>_<arch>.dmg` — drag-to-Applications disk image
  containing the .app

The pipeline is driven by [`cargo-packager`](https://github.com/crabnebula-dev/cargo-packager).
The full config lives in `[package.metadata.packager]` in `Cargo.toml`.

```sh
cargo install cargo-packager --locked     # one-time
just bundle                               # every release
```

## Bundle contents

```
HighBeam.app/
  Contents/
    Info.plist                    # generated; merged with bundle/Info.plist
    _CodeSignature/
    MacOS/
      high-beam                   # the release binary, self-signed
    Resources/
      HighBeam.icns               # converted from bundle/icon.png
      plugins/                    # bundled defaults (see below)
        calculator/{manifest.json,plugin.js}
        http-codes/{manifest.json,plugin.js,http.json}
        app-launcher/{manifest.json,plugin.js}
        paper-size/{manifest.json,plugin.js}
        dnd/{manifest.json,plugin.js,5eSpells.json}
      themes/
        yosemite-spotlight.toml
```

### First-launch plugin install

`src/bundle_install.rs::install_default_plugins_if_needed` runs once at
daemon startup (before the plugin loader scans). If the user's plugin
directory is missing or empty, the bundled defaults are recursively copied
in. Subsequent launches notice the populated dir and leave it alone, so the
user can edit / remove / add plugins freely without `.app` updates clobbering
their copy.

The `--plugins-dir <path>` CLI override skips this entirely; a developer
pointing at a checkout doesn't want a workspace seeded.

### Updating the shipped plugin list

The set of bundled plugins is the explicit list in
`Cargo.toml::[package.metadata.packager]::resources`. Dev-only fixtures
(`echo`, `echo-ts`, `slow-echo`, `frecency-demo`) deliberately stay out.
Adding a new plugin to the ship-list: append three entries (manifest,
plugin.js, any data files) to the `resources` array.

## Icon

The current `bundle/icon.png` is a generated 512×512 placeholder ("HB"
letters on amber background). To swap in a real icon:

1. Drop a 1024×1024 PNG at `bundle/icon.png` (or commit multiple sizes —
   cargo-packager picks the largest available).
2. Run `just bundle`; cargo-packager generates the multi-resolution
   `.icns` automatically.

Pre-generated `.icns` files are also accepted — list them in the `icons`
array in `Cargo.toml`.

## Signing

Default config uses `signing-identity = "High Beam Self-Signed"` — a
self-signed identity in your keychain. Generate it once:

```sh
./scripts/create-signing-cert.sh
```

End users still need one post-install command to strip the download
quarantine, because Gatekeeper doesn't trust self-signed certs:

```bash
xattr -dr com.apple.quarantine /Applications/HighBeam.app
```

For Gatekeeper-trusted distribution (no `xattr` step), enroll in the
Apple Developer Program ($99/yr), issue a Developer ID certificate,
update `signing-identity` in `Cargo.toml`, and pass notarization
credentials via `APPLE_KEYCHAIN_PROFILE` (or `APPLE_ID` /
`APPLE_PASSWORD` / `APPLE_TEAM_ID`) when running `cargo packager` —
the packager wraps `xcrun notarytool` + `stapler` directly. Apple's
[Notarizing macOS software](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)
docs cover the cert + notarytool setup.

## Auto-update (macOS)

The packaged `.app` self-updates in place via
[`cargo-packager-updater`](https://docs.rs/cargo-packager-updater) — the
sibling crate to the `cargo-packager` bundler above. The flow lives in
`src/updater.rs`:

1. A background thread (and the Core `check for updates` verb) fetches
   `latest.json` from
   `https://github.com/Mechazawa/high-beam/releases/latest/download/latest.json`.
   The `latest/download` alias always resolves to the newest non-prerelease,
   so there are no GitHub API calls or rate limits.
2. If `latest.json`'s `version` is newer, the user sees an
   `Update available → vX.Y.Z` row in the launcher. Enter downloads the
   signed `.app.tar.gz`, minisign-verifies it, swaps the bundle, and
   relaunches.

**Gating.** The whole feature is disabled unless the running binary lives
inside a `.app` (`current_exe()` has a `.app/Contents/MacOS` ancestor). That
one check excludes Homebrew, `cargo install`, and dev runs — none of which
should self-update. Linux compiles the updater to no-ops.

### Enabling it (one-time)

The updater is inert until a signing key exists. To turn it on:

1. Generate a minisign keypair:

   ```sh
   cargo packager signer generate
   ```

2. Paste the printed **public** key into `UPDATER_PUBKEY` in
   `src/updater.rs` (it is not secret; it ships in the binary).
3. Add the **private** key + its password as repo secrets
   `UPDATER_PRIVATE_KEY` and `UPDATER_KEY_PASSWORD`. The release workflow
   (`.github/workflows/release.yml`) then tars + signs the `.app`, generates
   `latest.json`, and attaches both to the release. Confirm the
   `cargo packager signer sign` invocation in the workflow against the help
   output of your installed `cargo-packager` version.

This is separate from Apple codesigning (above). minisign protects the update
channel; Gatekeeper trust is a different axis.

> **Single-arch.** GitHub's `macos-latest` runner is arm64, so the published
> `.app.tar.gz` is arm64-only — the same limitation the `.dmg` already has.
> Until the release builds both arches and keys `latest.json` by
> `darwin-<arch>`, Intel Macs should keep updating via the `.dmg`.

### Post-update bundled resources

After an app update lands a new bundle, the next launch reconciles the
shipped plugins + themes into the user's dirs
(`src/bundle_install.rs::reconcile_bundled_resources`): plugins and themes
newly added to the bundle are installed, and existing ones the bundle ships a
newer copy of are updated — but only when the user hasn't edited their copy
since we last shipped it. Plugins are gated by manifest `version`
(`is_newer_version`), themes by content hash (no version field). The
"what did we last ship" record lives in `shipped-resources.json` in the
config dir; the reconcile runs once per app-version change and never clobbers
a user-edited resource. This also covers manual `.dmg` re-installs, not just
self-update.

## Linux

Four shipped package formats, in the user-stated priority order. The
portable tarball + Arch (`.pkg.tar.zst` / AUR) are the must-haves;
`.deb` covers Ubuntu / Debian; `.rpm` covers Fedora / RHEL / SUSE.

| Format               | Tool                  | Recipe                |
|----------------------|-----------------------|-----------------------|
| Portable tarball     | `tar` (built-in)      | `just bundle-tarball` |
| Arch `.pkg.tar.zst`  | `cargo-packager`      | `just bundle-arch`    |
| `.deb`               | `cargo-packager`      | `just bundle-deb`     |
| `.rpm`               | `cargo-generate-rpm`  | `just bundle-rpm`     |

`just bundle-linux` runs all four in sequence. Each artifact ships
the same payload:

```
/usr/bin/highbeam                            # release binary
/usr/share/highbeam/plugins/<name>/...       # bundled defaults
/usr/share/highbeam/themes/yosemite-spotlight.toml
/usr/share/applications/highbeam.desktop     # app-menu entry
/usr/lib/systemd/user/highbeam.service       # user daemon
```

The bundled plugin set matches the macOS `.app`'s
`[package.metadata.packager].resources` list — the real plugins, with the
dev fixtures (`echo`, `echo-ts`, `slow-echo`, `frecency-demo`) excluded by
name in the tarball recipe.

### First-launch install on Linux

`src/bundle_install.rs::install_default_plugins_if_needed` copies the
bundled defaults into `$XDG_DATA_HOME/high-beam/plugins/` on first
launch. The current resolver computes the bundled source as
`current_exe().parent().parent().join("Resources/plugins")` — that's a
macOS-`.app`-shaped path (`HighBeam.app/Contents/MacOS/high-beam` →
`Contents/Resources/plugins`). The Linux equivalent lives at
`/usr/share/highbeam/plugins/`, which `current_exe()` from
`/usr/bin/highbeam` does NOT reach with the same two-parent walk.

Until the resolver learns about the Linux layout, first-launch
seeding on Linux is a no-op and users have to copy plugins out of
`/usr/share/highbeam/plugins/` manually (or symlink). Tracked as a
follow-up — the fix is a small `#[cfg(target_os = "linux")]` branch
in `bundle_install.rs::bundled_plugins_dir`.

### Tarball

```sh
just bundle-tarball
# -> target/release/dist/highbeam-<ver>-linux-x86_64.tar.gz
```

The user untars somewhere, then either runs the embedded `install.sh`
(defaults to `/usr/local`, override with `PREFIX=$HOME/.local`) or
copies the tree by hand. `install.sh` registers the systemd unit and
prints the `systemctl --user enable --now` + WM-keybind next steps.

### Arch — pacman + AUR

`cargo-packager`'s `pacman` format produces a `.tar.gz` + `PKGBUILD`
pair under `target/release/`. `makepkg` against that pair produces the
actual `.pkg.tar.zst` you install with `pacman -U`. (cargo-packager
deliberately doesn't shell out to `makepkg` itself, so we can build
the source materials on any host — no Arch machine required.)

For AUR distribution we ship a separate, hand-maintained `PKGBUILD`
under `packaging/aur/` — the `-bin` variant that pulls a prebuilt
release tarball from GitHub Releases rather than rebuilding from
source. The full submission workflow lives at
[`packaging/aur/README.md`](../packaging/aur/README.md): per-release
sha256 bump, `makepkg --printsrcinfo > .SRCINFO`, push to
`ssh://aur@aur.archlinux.org/high-beam-bin.git`.

### .deb (Debian / Ubuntu)

```sh
just bundle-deb
# -> target/release/high-beam_<ver>_amd64.deb
```

Runtime deps declared in `[package.metadata.packager.deb].depends`:
`libxkbcommon0` (winit's Wayland/X11 keymap backend) and `libssl3`
(rustls' system-roots fallback). These names resolve on Debian 12 /
Ubuntu 22.04 and newer.

Inspect the produced `.deb` with `dpkg --info <file>.deb` and
`dpkg --contents <file>.deb`. Install with `sudo apt install
./<file>.deb` (apt's local-file mode pulls runtime deps from the
distro repo).

### .rpm (Fedora / RHEL / SUSE)

`cargo-packager` 0.11.x has no rpm backend — the fallback is
[`cargo-generate-rpm`](https://github.com/cat-in-136/cargo-generate-rpm).
The `bundle-rpm` recipe defers to it; install via:

```sh
cargo install cargo-generate-rpm
```

Then add a `[package.metadata.generate-rpm]` block to `Cargo.toml`
mirroring the `.deb`'s payload shape and deps (`libxkbcommon`,
`openssl-libs` are the Fedora names). The recipe is wired but the
metadata block is the open work; runs `bundle-rpm` print an install
hint and exit cleanly when `cargo-generate-rpm` isn't present so
`bundle-linux` stays green.

### Why not Flatpak

The sandbox blocks the host-OS access launcher plugins need —
`/usr/share/applications/*.desktop` reads, `/proc` walks, IPC to
window managers, global hotkeys. Working around it via
`--filesystem=host` / `--share=process` defeats the sandbox anyway.

## Windows

Not wired. `cargo-packager` supports `wix` (`.msi`) and `nsis`
(`.exe`); the Linux precedent above is the template once a Windows
port lands.

## Release workflow (GitHub Actions)

`.github/workflows/release.yml` builds macOS + Linux artifacts and
publishes a GitHub Release on `v*` tag pushes. Tags containing `-rc`,
`-beta`, or `-alpha` flag as pre-releases.

Optional secrets:

| Secret | Purpose |
|--------|---------|
| `MACOS_CERT_P12_BASE64` | Base64 of the codesigning `.p12` (cert + key). Missing → ad-hoc-signed bundle, `xattr` step still required. |
| `MACOS_CERT_PASSWORD` | The PKCS12 passphrase. |
| `UPDATER_PRIVATE_KEY` | minisign private key for the self-updater (`cargo packager signer generate`). Missing → no `latest.json`, updater stays inert. See [Auto-update](#auto-update-macos). |
| `UPDATER_KEY_PASSWORD` | Password for `UPDATER_PRIVATE_KEY`. |

Release notes are AI-summarised via
[GitHub Models](https://github.com/marketplace/models) using the
auto-provisioned `GITHUB_TOKEN`; the workflow falls back to the raw
commit log on any failure, so missing both secrets still publishes.
