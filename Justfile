default: check

run:
    cargo run

test:
    cargo test

# Run vitest in every default plugin that ships its own test suite.
# Plugins live in an npm workspace at `plugins/` — one install, one
# lockfile, one node_modules tree. Run `cd plugins && npm install`
# once after a fresh clone.
test-plugins:
    cd plugins && npm test --workspaces --if-present

lint:
    cargo clippy --all-targets --all-features -- -D warnings -A clippy::pedantic

lint-pedantic:
    cargo clippy --all-targets --all-features

fmt:
    cargo fmt -- --check

fmt-fix:
    cargo fmt

check: fmt lint test

# Build the release binary, then run cargo-packager to produce a .app
# bundle and drag-to-Applications .dmg in target/release.
# Requires `cargo install cargo-packager --locked` once per machine.
bundle:
    cargo build --release
    cargo packager --release

# Build every Linux artifact. Run this on a Linux host — cargo-packager
# silently skips deb/pacman when the host is macOS.
bundle-linux: bundle-tarball bundle-deb bundle-arch bundle-rpm

# Portable tarball: a single highbeam-<ver>-linux-x86_64.tar.gz that
# unpacks into a tree mirroring an installed package, with install.sh
# ready to copy everything to /usr/local. The dev-only fixtures
# (echo / echo-ts / slow-echo / frecency-demo) are excluded so the
# tarball matches the .deb / pacman / AUR payloads.
bundle-tarball:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --release
    stage="$(mktemp -d)"
    # `grep '^version'` picks the [package] version, not the various
    # transitive `version = "..."` lines further down Cargo.toml.
    version="$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)"
    name="highbeam-${version}-linux-x86_64"
    root="$stage/$name"
    mkdir -p \
        "$root/bin" \
        "$root/share/highbeam/plugins" \
        "$root/share/highbeam/themes" \
        "$root/share/applications" \
        "$root/lib/systemd/user"
    cp target/release/high-beam "$root/bin/highbeam"
    # Mirror the bundled plugin list in Cargo.toml's `resources` block.
    # Dev fixtures live alongside the real plugins in `plugins/`, so we
    # exclude by name rather than enumerating the keepers — it's the
    # smaller list to maintain.
    for plugin in plugins/*/; do
        name_only="$(basename "$plugin")"
        case "$name_only" in
            echo|echo-ts|slow-echo|frecency-demo) continue ;;
        esac
        dest="$root/share/highbeam/plugins/$name_only"
        mkdir -p "$dest"
        # Copy manifest + plugin.js + any sibling .json data files; skip
        # node_modules, tsconfig, vitest config, *.test.*, lockfiles.
        for f in "$plugin"manifest.json "$plugin"plugin.js "$plugin"*.json; do
            [ -f "$f" ] || continue
            case "$(basename "$f")" in
                package.json|package-lock.json|tsconfig.json) continue ;;
                vitest.config.json) continue ;;
            esac
            cp "$f" "$dest/"
        done
    done
    cp themes/*.toml "$root/share/highbeam/themes/"
    cp packaging/linux/highbeam.desktop "$root/share/applications/"
    cp packaging/linux/highbeam.service "$root/lib/systemd/user/"
    cp packaging/linux/install.sh "$root/install.sh"
    cp README.md "$root/" 2>/dev/null || true
    chmod +x "$root/install.sh" "$root/bin/highbeam"
    mkdir -p target/release/dist
    tar -C "$stage" -czf "target/release/dist/${name}.tar.gz" "$name"
    rm -rf "$stage"
    echo "-> target/release/dist/${name}.tar.gz"

bundle-arch:
    cargo packager --release --formats pacman

bundle-deb:
    cargo packager --release --formats deb

# .rpm via cargo-generate-rpm — cargo-packager 0.11 has no rpm backend,
# so we drive it ourselves. Stages files into `target/rpm-stage/`
# mirroring the .deb / tarball install layout; `cargo generate-rpm`
# then reads `[package.metadata.generate-rpm]` in Cargo.toml.
#
# The plugin filter (echo / echo-ts / slow-echo / frecency-demo) and
# the per-file skiplist (package.json, tsconfig, vitest config, …)
# mirror `bundle-tarball` exactly so the three Linux artifacts ship
# the same set of plugins. If `bundle-tarball`'s logic changes, keep
# the two in sync.
#
# Exits 0 with an install hint when cargo-generate-rpm isn't on PATH
# so `bundle-linux` keeps working on a stock dev box; CI installs the
# tool unconditionally via taiki-e/install-action.
bundle-rpm:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cargo-generate-rpm >/dev/null 2>&1; then
        echo "bundle-rpm: cargo-generate-rpm not installed."
        echo "  cargo install cargo-generate-rpm"
        exit 0
    fi
    cargo build --release
    stage="target/rpm-stage"
    rm -rf "$stage"
    mkdir -p \
        "$stage/usr/bin" \
        "$stage/usr/share/highbeam/plugins" \
        "$stage/usr/share/highbeam/themes" \
        "$stage/usr/share/applications" \
        "$stage/usr/lib/systemd/user"
    cp target/release/high-beam "$stage/usr/bin/highbeam"
    chmod +x "$stage/usr/bin/highbeam"
    for plugin in plugins/*/; do
        name_only="$(basename "$plugin")"
        case "$name_only" in
            echo|echo-ts|slow-echo|frecency-demo) continue ;;
        esac
        dest="$stage/usr/share/highbeam/plugins/$name_only"
        mkdir -p "$dest"
        for f in "$plugin"manifest.json "$plugin"plugin.js "$plugin"*.json; do
            [ -f "$f" ] || continue
            case "$(basename "$f")" in
                package.json|package-lock.json|tsconfig.json) continue ;;
                vitest.config.json) continue ;;
            esac
            cp "$f" "$dest/"
        done
    done
    cp themes/*.toml "$stage/usr/share/highbeam/themes/"
    cp packaging/linux/highbeam.desktop "$stage/usr/share/applications/"
    cp packaging/linux/highbeam.service "$stage/usr/lib/systemd/user/"
    # `-o target/release/` lands the .rpm next to the .deb / pacman
    # tarball so the CI artifact-upload glob (`target/release/*.rpm`)
    # picks it up without a separate path.
    cargo generate-rpm --output target/release/

# Re-render bundle/icon.svg into bundle/icon.png (1024x1024) and
# bundle/icon.icns (multi-resolution). cargo-packager 0.11 needs the
# .icns explicitly — a bare PNG triggers "No matching IconType". Uses
# macOS-built-in qlmanage / sips / iconutil so no extra tooling.
icon:
    #!/usr/bin/env bash
    set -euo pipefail
    qlmanage -t -s 1024 -o /tmp bundle/icon.svg >/dev/null
    mv /tmp/icon.svg.png bundle/icon.png
    iconset="$(mktemp -d)/icon.iconset"
    mkdir -p "$iconset"
    for sz in 16 32 128 256 512; do
        sips -z $sz $sz bundle/icon.png --out "$iconset/icon_${sz}x${sz}.png" >/dev/null
        d=$((sz * 2))
        sips -z $d $d bundle/icon.png --out "$iconset/icon_${sz}x${sz}@2x.png" >/dev/null
    done
    cp bundle/icon.png "$iconset/icon_512x512@2x.png"
    iconutil -c icns "$iconset" -o bundle/icon.icns
    rm -rf "$(dirname "$iconset")"
    echo "bundle/icon.{png,icns} regenerated from bundle/icon.svg"
