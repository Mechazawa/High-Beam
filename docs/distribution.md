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

## Signing & distribution

The bundle config uses `signing-identity = "High Beam Self-Signed"` — a
self-signed identity that lives in your keychain. It identifies the build
as "yours" (same dev across versions) without Apple's $99/yr program.
Gatekeeper still doesn't trust self-signed certs by default, so end users
need a one-time `xattr` step (below) — but that's an artifact of Apple's
trust model, not of the cert flow.

### Creating the signing cert (one-time)

```sh
./scripts/create-signing-cert.sh
```

This generates an RSA key + self-signed cert with the `codeSigning`
extended key usage via `openssl`, bundles into pkcs12, and imports into
your login keychain with `codesign` access granted. The CN is
`High Beam Self-Signed` to match `Cargo.toml`'s `signing-identity`. Pass
a name as `$1` to override.

Verify with `security find-identity -p codesigning -v` — you should see
the new identity listed. From there `just bundle` picks it up
automatically.

To re-issue (e.g. cert expired after the 10-year window, or you want a
different CN), delete the old one first:

```sh
security delete-identity -c "High Beam Self-Signed"
./scripts/create-signing-cert.sh
```

### What end users still need

After dragging the .app to `/Applications`, the user runs:

```bash
xattr -dr com.apple.quarantine /Applications/HighBeam.app
```

This strips the download quarantine bit so macOS launches the app
without prompting. `spctl --assess --verbose /Applications/HighBeam.app`
will still report `rejected` for self-signed bundles — that's expected;
Gatekeeper trust is strictly a signed-by-Apple-CA-and-notarized story.
The `xattr` workaround sits beside that reality: it tells macOS the user
vouched for the launch.

### The trade-off

| Path | Cost | Gatekeeper trust | User friction |
|------|------|------------------|---------------|
| Self-signed (current)   | $0      | No                                | One `xattr -dr` command after install |
| Developer ID + notarize | $99/yr  | Yes — launches cleanly everywhere | None                                  |

For real distribution to other Macs without the `xattr` step, you need:

### 1. Enroll in the Apple Developer Program

$99/yr at <https://developer.apple.com/programs/>. Required to issue any
Developer ID certificate; this is the cost of admission, not optional.

### 2. Create a Developer ID Application certificate

Inside Xcode → Settings → Accounts → Manage Certificates, click `+` → "Developer ID
Application". The certificate name is what goes in `signing-identity`, of
the form `Developer ID Application: Your Name (TEAMID12)`.

### 3. Point cargo-packager at the cert

Update `[package.metadata.packager.macos]` in `Cargo.toml`:

```toml
signing-identity = "Developer ID Application: Bas Bieling (XXXXXXXXXX)"
```

The keychain holding the private key must be unlocked when `cargo packager`
runs.

### 4. Notarize the .dmg

Apple's notarytool inspects the bundle for malware/policy violations and
issues a ticket. The recommended flow uses a keychain-stored
app-specific password:

```sh
# One-time setup: store an app-specific password (created at appleid.apple.com)
# under a keychain profile name of your choice.
xcrun notarytool store-credentials high-beam-notary \
    --apple-id "you@example.com" \
    --team-id "XXXXXXXXXX"

# Per release:
xcrun notarytool submit target/release/HighBeam_*.dmg \
    --keychain-profile high-beam-notary \
    --wait
```

### 5. Staple the ticket

Notarization succeeds asynchronously; stapling embeds the ticket into the
.dmg so Gatekeeper can verify offline.

```sh
xcrun stapler staple target/release/HighBeam_*.dmg
```

`cargo packager` can drive notarization automatically when the
`APPLE_KEYCHAIN_PROFILE` (or `APPLE_ID` / `APPLE_PASSWORD` / `APPLE_TEAM_ID`)
environment variables are set; it currently logs `Skipping app notarization`
when none are present.

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
`[package.metadata.packager].resources` list — the 16 "real" plugins,
with the dev fixtures (`echo`, `echo-ts`, `slow-echo`, `frecency-demo`)
excluded by name in the tarball recipe.

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

Flatpak is a poor fit for High Beam. Every launcher feature we care
about wants host-OS access that the sandbox blocks:

- The `app-launcher` plugin reads `/usr/share/applications/*.desktop`
  to enumerate installed apps. Flatpak hides the host filesystem;
  exposing it via `--filesystem=host` defeats the sandbox.
- The `kill-process` plugin walks `/proc` and sends signals. Flatpak's
  PID namespace makes this either impossible (default) or pointless
  (`--share=process` removes the isolation).
- The `window-mgmt` plugin shells out to `wmctrl`, `xdotool`, or the
  KWin/Hyprland IPC sockets. None of these are reliably available
  inside the sandbox.
- Global hotkeys rely on the desktop-portal `GlobalShortcuts`
  interface, which is uneven across compositors and adds permission
  prompts that defeat the "press one key, launcher appears" UX
  Spotlight-class tools sell.

Launchers are inherently host-OS integration tools; sandboxing fights
the value proposition. The four formats above land in the
`/usr/{bin,share,lib}` namespace that those features expect.

## Windows

Not wired. `cargo-packager` supports `wix` (`.msi`) and `nsis`
(`.exe`); the Linux precedent above is the template once a Windows
port lands.
