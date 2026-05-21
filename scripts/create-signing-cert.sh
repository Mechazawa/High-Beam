#!/usr/bin/env bash
# Creates a self-signed code-signing certificate in the user's login keychain.
# The cert name must match `[package.metadata.packager.macos]::signing-identity`
# in Cargo.toml so `cargo packager` finds it automatically.
#
# Usage:
#   ./scripts/create-signing-cert.sh                          # uses default name
#   ./scripts/create-signing-cert.sh "Some Other Name"        # custom CN
#
# Idempotent — re-running will install another cert with the same CN. Use
# `security delete-identity -c "<name>"` first if you want a clean redo.

set -euo pipefail

CERT_NAME="${1:-High Beam Self-Signed}"
KEYCHAIN="${HOME}/Library/Keychains/login.keychain-db"

if ! command -v openssl >/dev/null; then
    echo "openssl not on PATH — install via 'brew install openssl' or use Apple's bundled cli" >&2
    exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Inline extension block: codesign accepts certs whose EKU includes
# `codeSigning` (1.3.6.1.5.5.7.3.3). basicConstraints + keyUsage are added
# for completeness; Apple's tooling tolerates their absence but a few
# third-party signers don't.
cat >"$tmp/v3.ext" <<'EXT'
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
EXT

# 1. Generate RSA private key + CSR + self-signed cert in one shot.
openssl genrsa -out "$tmp/key.pem" 2048 >/dev/null 2>&1
openssl req -new -key "$tmp/key.pem" -out "$tmp/csr.pem" \
    -subj "/CN=${CERT_NAME}" >/dev/null 2>&1
openssl x509 -req \
    -in "$tmp/csr.pem" \
    -signkey "$tmp/key.pem" \
    -out "$tmp/cert.pem" \
    -days 3650 \
    -extfile "$tmp/v3.ext" >/dev/null 2>&1

# 2. Bundle key + cert into pkcs12 for keychain import. Empty passphrase
# is fine: the keychain itself is the security boundary.
openssl pkcs12 -export \
    -inkey "$tmp/key.pem" \
    -in "$tmp/cert.pem" \
    -out "$tmp/cert.p12" \
    -name "$CERT_NAME" \
    -passout pass: >/dev/null 2>&1

# 3. Import. `-T /usr/bin/codesign` grants codesign access without
# prompting. `-A` (any app) would also work but is broader than needed.
security import "$tmp/cert.p12" \
    -k "$KEYCHAIN" \
    -P "" \
    -T /usr/bin/codesign \
    -T /usr/bin/security >/dev/null

# 4. Verify codesign can see it.
if security find-identity -p codesigning -v "$KEYCHAIN" 2>/dev/null | grep -qF "$CERT_NAME"; then
    echo "✓ Installed code-signing identity: $CERT_NAME"
    echo
    echo "Verify:  security find-identity -p codesigning -v"
    echo "Bundle:  just bundle"
    echo "Inspect: codesign -dvv target/release/HighBeam.app"
else
    echo "Cert imported but codesign can't find it. Try Keychain Access:" >&2
    echo "  - Find the '$CERT_NAME' cert" >&2
    echo "  - Right-click → Get Info → expand 'Trust' → set 'Code Signing' to 'Always Trust'" >&2
    exit 1
fi
