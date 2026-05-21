#!/usr/bin/env bash
# Creates a self-signed code-signing certificate in the user's login keychain
# AND drops a PKCS12 backup file in the current dir for 1Password storage.
#
# The cert name must match `[package.metadata.packager.macos]::signing-identity`
# in Cargo.toml so `cargo packager` finds it automatically.
#
# Usage:
#   ./scripts/create-signing-cert.sh                       # default name
#   ./scripts/create-signing-cert.sh "Some Other Name"     # custom CN
#
# Idempotent — every prior cert with the same CN is removed first.

set -euo pipefail

CERT_NAME="${1:-High Beam Self-Signed}"
KEYCHAIN="${HOME}/Library/Keychains/login.keychain-db"
P12_OUT="${PWD}/highbeam-signing-cert.p12"
PBE_PASS="highbeam-signing"

if ! command -v openssl >/dev/null; then
    echo "openssl not on PATH — install via 'brew install openssl' or use Apple's bundled cli" >&2
    exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# 1. Generate RSA key + self-signed cert with `codeSigning` in the EKU.
#    The named section in the extfile is required — without it, the
#    extensions are silently dropped from the issued cert.
cat >"$tmp/v3.ext" <<'EXT'
[ v3_codesign ]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
EXT

openssl genrsa -out "$tmp/key.pem" 2048 >/dev/null 2>&1
openssl req -new -key "$tmp/key.pem" -out "$tmp/csr.pem" \
    -subj "/CN=${CERT_NAME}" >/dev/null 2>&1
openssl x509 -req \
    -in "$tmp/csr.pem" \
    -signkey "$tmp/key.pem" \
    -out "$tmp/cert.pem" \
    -days 3650 \
    -extfile "$tmp/v3.ext" \
    -extensions v3_codesign >/dev/null 2>&1

# 2. Remove every prior cert with the same CN. `delete-certificate -c`
#    refuses on duplicates, so we loop by SHA-1 hash until find returns
#    a non-zero (none left).
while true; do
    hash=$(security find-certificate -c "$CERT_NAME" -Z 2>/dev/null \
        | awk '/SHA-1 hash/ {print $NF; exit}') || true
    [ -z "$hash" ] && break
    security delete-certificate -Z "$hash" >/dev/null 2>&1 || break
done

# 3. Bundle key + cert into a PKCS12 with options macOS's `security
#    import` actually accepts:
#      * `-legacy` switches OpenSSL 3.x to the legacy PBKDF/PRF set
#      * `-keypbe`/`-certpbe` PBE-SHA1-3DES is the format macOS reads
#      * `-macalg sha1` (OpenSSL 3 defaults to SHA-256, which fails MAC)
#      * non-empty passphrase — empty passphrases on OpenSSL 3 emit a
#        MAC iteration count macOS treats as malformed
#    The PKCS12 path is the one that produces a proper identity (cert
#    + key linked) when imported; separate PEM imports leave them
#    unlinked and `find-identity -p codesigning` won't see them.
openssl pkcs12 -export -legacy \
    -keypbe PBE-SHA1-3DES \
    -certpbe PBE-SHA1-3DES \
    -macalg sha1 \
    -inkey "$tmp/key.pem" \
    -in "$tmp/cert.pem" \
    -out "$P12_OUT" \
    -name "$CERT_NAME" \
    -passout "pass:$PBE_PASS" >/dev/null 2>&1

# 4. Import. `-T /usr/bin/codesign` grants codesign access without
#    prompting; same for `security` so re-runs of this script can
#    inspect.
security import "$P12_OUT" \
    -k "$KEYCHAIN" \
    -P "$PBE_PASS" \
    -T /usr/bin/codesign \
    -T /usr/bin/security >/dev/null

# 5. Verify the identity is present in the keychain. We deliberately
#    DO NOT use `-p codesigning -v` here — that filter rejects
#    untrusted self-signed certs ("CSSMERR_TP_NOT_TRUSTED"), but
#    `codesign` itself accepts them just fine. The script's job is to
#    confirm the cert+key landed bound; trust is a separate (and not
#    actually-needed) layer.
if ! security find-identity "$KEYCHAIN" 2>/dev/null | grep -qF "$CERT_NAME"; then
    echo "PKCS12 imported but no identity named '$CERT_NAME' found in the keychain." >&2
    echo "Open Keychain Access to inspect; the login keychain should have a" >&2
    echo "'$CERT_NAME' entry with a disclosure triangle (▸) revealing a" >&2
    echo "private key. Without that pairing, codesign won't sign with it." >&2
    exit 1
fi

cat <<MSG
✓ Installed code-signing identity: $CERT_NAME

PKCS12 backup (private key + cert): $P12_OUT
PKCS12 passphrase: $PBE_PASS

Recommended next steps:
  1. In 1Password, create a Document titled "High Beam signing cert" and
     attach the .p12 file. Note the passphrase above (1Password's
     "passphrase" field is the right spot).
  2. Delete the local copy once it's safely in 1Password:
       rm "$P12_OUT"
  3. (Now or anytime) verify the signing identity is usable:
       security find-identity -p codesigning -v | grep "$CERT_NAME"
  4. Build the bundle — the cert is picked up automatically:
       just bundle
MSG
