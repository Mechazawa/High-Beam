# High Beam — Homebrew cask (personal tap).
#
# Lives in the tap repo `Mechazawa/homebrew-high-beam` as
# `Casks/high-beam.rb`; this copy under packaging/ is the source of
# truth we edit in-tree and mirror out on release (see README.md here).
#
# Install once the tap is published:
#   brew install --cask mechazawa/high-beam/high-beam
#
# The `sha256` below MUST be refreshed every release — it pins the
# universal .dmg `just bundle-universal` uploads to GitHub Releases.
# `just bundle-universal` prints the digest at the end of its run, or:
#   shasum -a 256 HighBeam_<version>_universal.dmg
cask "high-beam" do
  version "0.3.1"
  # PLACEHOLDER — replace with the real digest of the universal .dmg for
  # this `version` before publishing. No universal artifact has been cut
  # yet (current releases ship an arm64-only .dmg), so this stays fake
  # until the first release built by the updated workflow lands.
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"

  url "https://github.com/Mechazawa/high-beam/releases/download/v#{version}/HighBeam_#{version}_universal.dmg",
      verified: "github.com/Mechazawa/high-beam/"
  name "High Beam"
  desc "Keyboard launcher in the Spotlight / Alfred / Raycast / Ulauncher class"
  homepage "https://github.com/Mechazawa/high-beam"

  # GitHub Releases tags are `vX.Y.Z`; strip the leading `v` for the
  # cask version. Pre-release tags (-rc/-beta/-alpha) are flagged as
  # prereleases upstream and skipped by :github_latest automatically.
  livecheck do
    url :url
    strategy :github_latest
  end

  # minimum-system-version = "14.0" in Cargo.toml's packager.macos table.
  depends_on macos: ">= :sonoma"

  app "HighBeam.app"

  # `directories::ProjectDirs` with empty qualifier/org resolves to
  # ~/Library/Application Support/high-beam on macOS (src/paths.rs) — it
  # holds settings, the single-instance socket, and the seeded plugins/
  # + themes/ trees.
  zap trash: [
    "~/Library/Application Support/high-beam",
  ]

  caveats <<~EOS
    HighBeam is currently self-signed (not Apple-notarized), so the
    first launch is blocked by Gatekeeper. Until a notarized build
    ships, strip the download quarantine once after install:

      xattr -dr com.apple.quarantine "#{appdir}/HighBeam.app"

    Open with Shift+Space (configurable in Settings -> Global).
  EOS
end
