# AUR submission workflow

This directory holds the `PKGBUILD` we publish to the
[Arch User Repository](https://aur.archlinux.org/) under the package
name `high-beam-bin`. The `-bin` suffix is the AUR convention for
"prebuilt binary, not built from source" — Arch users expect either
`high-beam` (source) or `high-beam-bin` (prebuilt) and can opt in to
either.

## Per-release checklist

1. Tag and build a release tarball locally:

   ```sh
   git tag vX.Y.Z
   just bundle-tarball
   ```

   This produces `target/release/dist/highbeam-X.Y.Z-linux-x86_64.tar.gz`.

2. Upload the tarball to a GitHub release at
   `https://github.com/Mechazawa/high-beam/releases/tag/vX.Y.Z`. The
   `PKGBUILD`'s `source=()` URL points at the release asset, so the
   filename matters — keep the `highbeam-<ver>-linux-x86_64.tar.gz`
   shape.

3. Update `PKGBUILD`:
   - bump `pkgver` to `X.Y.Z`
   - reset `pkgrel=1`
   - replace `sha256sums=('SKIP')` with the real hash:

     ```sh
     sha256sum target/release/dist/highbeam-X.Y.Z-linux-x86_64.tar.gz
     ```

4. Regenerate the `.SRCINFO` file. The AUR git repo requires both
   `PKGBUILD` and `.SRCINFO` to stay in sync; this is what their web UI
   reads:

   ```sh
   cd packaging/aur
   makepkg --printsrcinfo > .SRCINFO
   ```

5. Smoke-test the build before pushing:

   ```sh
   makepkg -si      # builds, installs into your test machine, runs deps
   ```

   `makepkg` will refuse to run as root; do this on a workstation Arch
   install or in a clean container.

6. Push to the AUR repo. The AUR uses git-over-SSH with a per-user key
   registered at <https://aur.archlinux.org/account/>. The remote is
   the package name, not this repository:

   ```sh
   # First time only — clone the (initially empty) AUR repo and copy
   # PKGBUILD + .SRCINFO in.
   git clone ssh://aur@aur.archlinux.org/high-beam-bin.git ../aur-high-beam-bin
   cp packaging/aur/{PKGBUILD,.SRCINFO} ../aur-high-beam-bin/

   cd ../aur-high-beam-bin
   git add PKGBUILD .SRCINFO
   git commit -m "high-beam-bin X.Y.Z-1"
   git push
   ```

## Notes

- The AUR git repo carries ONLY `PKGBUILD` + `.SRCINFO` (plus any
  patch files you need). Don't push the rest of the source tree.
- Bump `pkgrel` (leaving `pkgver` unchanged) for packaging-only fixes
  — a `depends` correction, a missing `install` line, etc.
- The `provides=('high-beam')` + `conflicts=('high-beam')` pair means
  an eventual source-build `high-beam` AUR package can coexist with
  this one as alternatives without both being installable
  simultaneously.
