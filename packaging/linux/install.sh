#!/usr/bin/env bash
# Tarball installer. The .deb / .rpm / pacman paths each have their own
# package manager doing this work; this script is the "I unpacked the
# tarball, now what?" answer.
#
# Defaults to /usr/local (the FHS-blessed location for locally-installed
# software, separate from distro packages under /usr). Override with
# PREFIX=/some/path ./install.sh — useful for ~/.local user installs.

set -euo pipefail

PREFIX="${PREFIX:-/usr/local}"
SYSTEMD_USER_DIR="${SYSTEMD_USER_DIR:-$PREFIX/lib/systemd/user}"

here="$(cd "$(dirname "$0")" && pwd)"

# Two install modes:
#   * `PREFIX=/usr/local` (default) — system-wide; needs sudo, writes
#     /usr/local/bin, /usr/local/share, /usr/local/lib/systemd/user.
#   * `PREFIX=$HOME/.local` — per-user; no sudo, writes
#     ~/.local/{bin,share,lib/systemd/user}. systemctl --user picks the
#     unit up from ~/.local/lib/systemd/user too.
need_sudo=
if [ ! -w "$PREFIX" ] && [ "$(id -u)" -ne 0 ]; then
    if command -v sudo >/dev/null 2>&1; then
        need_sudo=sudo
    else
        echo "error: $PREFIX is not writable and sudo is unavailable" >&2
        exit 1
    fi
fi

run() {
    if [ -n "$need_sudo" ]; then
        $need_sudo "$@"
    else
        "$@"
    fi
}

echo "Installing High Beam to $PREFIX..."

run install -Dm755 "$here/bin/highbeam" "$PREFIX/bin/highbeam"

# `cp -r` instead of `install -D` for tree copies — install(1) operates
# on single files, and the bundled plugins / themes are directory trees.
run mkdir -p "$PREFIX/share/highbeam"
run cp -r "$here/share/highbeam/plugins" "$PREFIX/share/highbeam/"
run cp -r "$here/share/highbeam/themes" "$PREFIX/share/highbeam/"

run install -Dm644 "$here/share/applications/highbeam.desktop" \
    "$PREFIX/share/applications/highbeam.desktop"

run install -Dm644 "$here/lib/systemd/user/highbeam.service" \
    "$SYSTEMD_USER_DIR/highbeam.service"

cat <<EOF

High Beam installed. Next steps:

  1. Enable + start the user daemon:

       systemctl --user daemon-reload
       systemctl --user enable --now highbeam.service

  2. Bind a global hotkey in your WM / DE to invoke:

       $PREFIX/bin/highbeam --open

     See /usr/share/doc or docs/platform.md for per-WM examples
     (GNOME / KDE / sway / Hyprland).

EOF
