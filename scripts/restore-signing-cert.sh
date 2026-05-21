#!/usr/bin/env bash
# Restore the High Beam signing cert from 1Password onto a fresh machine.
# Pairs with `scripts/create-signing-cert.sh` (which creates a NEW cert);
# this one re-installs the existing cert so the Authority + key fingerprint
# stay consistent across machines.
#
# Prereqs:
#   - `op` CLI installed and signed in (`op signin`)
#   - The cert was previously uploaded as a Document titled
#     "High Beam signing cert" with the passphrase in the notes field.
#
# Usage:
#   ./scripts/restore-signing-cert.sh                  # default vault: Private
#   ./scripts/restore-signing-cert.sh "Vault Name"     # explicit vault

set -euo pipefail

VAULT="${1:-Private}"
ITEM="High Beam signing cert"
CERT_NAME="High Beam Self-Signed"
KEYCHAIN="${HOME}/Library/Keychains/login.keychain-db"

if ! command -v op >/dev/null; then
    echo "1Password CLI (\`op\`) not on PATH. Install with 'brew install --cask 1password-cli'." >&2
    exit 1
fi

# Verify auth. `op vault list` is more reliable than `op whoami` for
# session-token-only flows.
if ! op vault list >/dev/null 2>&1; then
    echo "Not signed in to 1Password CLI. Run 'op signin' first." >&2
    exit 1
fi

p12=$(mktemp -t highbeam-cert.XXXXXX)
mv "$p12" "$p12.p12"
p12="$p12.p12"
trap 'rm -f "$p12"' EXIT

# 1. Download the .p12 attachment from the Document item.
op document get "$ITEM" --vault "$VAULT" --output "$p12" >/dev/null

# 2. Pull the passphrase from the notes field. Notes have the form
#    "PKCS12 passphrase: <pass>\n\n..." — grep + sed extracts it.
notes=$(op item get "$ITEM" --vault "$VAULT" --fields label=notesPlain 2>/dev/null | tr -d '"')
pbe_pass=$(printf '%s\n' "$notes" | sed -n 's/^PKCS12 passphrase: \(.*\)/\1/p' | head -1)

if [ -z "$pbe_pass" ]; then
    echo "Couldn't extract the PKCS12 passphrase from the 1Password item's notes." >&2
    echo "Expected a line of the form: PKCS12 passphrase: <value>" >&2
    exit 1
fi

# 3. Clean any prior install (cert + key) so the import doesn't end up
#    with duplicates. delete-identity refuses on duplicates, so we loop
#    by SHA-1 hash like the create script does.
while true; do
    hash=$(security find-certificate -c "$CERT_NAME" -Z 2>/dev/null \
        | awk '/SHA-1 hash/ {print $NF; exit}') || true
    [ -z "$hash" ] && break
    security delete-certificate -Z "$hash" >/dev/null 2>&1 || break
done

# 4. Import.
security import "$p12" \
    -k "$KEYCHAIN" \
    -P "$pbe_pass" \
    -T /usr/bin/codesign \
    -T /usr/bin/security >/dev/null

# 5. Confirm the identity landed. Same caveat as the create script —
#    self-signed never passes `-p codesigning -v` (untrusted), but
#    codesign accepts it just fine.
if ! security find-identity "$KEYCHAIN" 2>/dev/null | grep -qF "$CERT_NAME"; then
    echo "Import succeeded but no '$CERT_NAME' identity found in the keychain." >&2
    exit 1
fi

cat <<MSG
✓ Restored code-signing identity: $CERT_NAME (from 1Password vault '$VAULT')

Verify: security find-identity -p codesigning -v | grep "$CERT_NAME"
        (will show CSSMERR_TP_NOT_TRUSTED — that's expected for self-signed
         and doesn't stop codesign from using it)

Sign:   just bundle
MSG
