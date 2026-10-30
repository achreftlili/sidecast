#!/bin/bash
# Creates "Sidecast Local Signing", a self-signed code-signing certificate, in
# your login keychain. build-mac.sh signs with it, so macOS keeps the Screen
# Recording / Accessibility permissions across rebuilds (an ad-hoc signed app
# looks like a new app after every build). The private key never leaves this Mac.
#
#   make mac-signing                         (once per Mac)
#   SIDECAST_KEYCHAIN=<path> ...             use another keychain (tests)
set -euo pipefail

NAME="Sidecast Local Signing"
KEYCHAIN="${SIDECAST_KEYCHAIN:-$HOME/Library/Keychains/login.keychain-db}"

# (No `| grep -q` with pipefail: an early grep exit can fail the pipeline with SIGPIPE.)
has_identity() { [[ "$(security find-identity -p codesigning "$KEYCHAIN" 2>/dev/null)" == *"\"$NAME\""* ]]; }

if has_identity; then
    echo "\"$NAME\" is already in $KEYCHAIN; nothing to do."
    exit 0
fi

umask 077
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

cat > "$WORK/cert.cnf" <<EOF
[ req ]
distinguished_name = dn
x509_extensions = ext
prompt = no
[ dn ]
CN = $NAME
[ ext ]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
subjectKeyIdentifier = hash
EOF

# Throwaway password for the temporary .p12 only.
PASS="$(/usr/bin/openssl rand -hex 16)"
# /usr/bin/openssl (LibreSSL) writes a PKCS#12 that `security import` accepts;
# Homebrew's OpenSSL 3 defaults to algorithms it rejects.
/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$WORK/cert.cnf" \
    -keyout "$WORK/key.pem" -out "$WORK/cert.pem" 2>/dev/null
/usr/bin/openssl pkcs12 -export -inkey "$WORK/key.pem" -in "$WORK/cert.pem" -name "$NAME" \
    -passout "pass:$PASS" -out "$WORK/id.p12"
# -T: codesign may use the key (it can still ask once; choose "Always Allow").
security import "$WORK/id.p12" -k "$KEYCHAIN" -P "$PASS" -T /usr/bin/codesign >/dev/null

if ! has_identity; then
    echo "error: \"$NAME\" was imported but codesign cannot see it" >&2
    exit 1
fi
echo "Created \"$NAME\" in $KEYCHAIN."
echo "The next build signs with it. If macOS asks to let codesign use the key, enter your login password and choose Always Allow."
