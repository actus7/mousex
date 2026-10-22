#!/usr/bin/env bash
# MouseX installer for macOS (client machine).
#   curl -fsSL https://github.com/actus7/mousex/releases/latest/download/install.sh | bash
# Optional environment variables:
#   MOUSEX_VERSION  release tag to install, e.g. v1.0.0 (default: latest)
#   MOUSEX_MACHINE  name of this Mac in MouseX (default: the ComputerName)
set -euo pipefail

REPO_URL="https://github.com/actus7/mousex/releases"
ARCHIVE="MouseX-macos-universal.tar.gz"
SUMS="SHA256SUMS"
# The legacy name is reused when present, so a Mac set up before the rename keeps its identity.
LEGACY_IDENTITY="Sculpin Local Signing"
IDENTITY="MouseX Local Signing"

say() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

require_environment() {
    [ "$(uname -s)" = "Darwin" ] || die "this installer is for macOS only (use install.ps1 on Windows)."
    [ "$(id -u)" -ne 0 ] || die "run this as your normal user, not with sudo: the agent is a LaunchAgent."
    local tool
    for tool in curl shasum tar codesign security launchctl; do
        command -v "$tool" >/dev/null 2>&1 || die "required tool not found: $tool"
    done
}

# Prints the 64-hex SHA-256 of the file, or nothing when there is no matching entry.
expected_sha256() {
    local sums="$1" name="$2"
    awk -v name="$name" '{ file = $2; sub(/^\*/, "", file); if (file == name) { print $1; exit } }' "$sums"
}

download_package() {
    local base="$1" dir="$2" expected actual
    say "Downloading MouseX from $base ..."
    curl -fsSL --proto '=https' --tlsv1.2 -o "$dir/$ARCHIVE" "$base/$ARCHIVE"
    curl -fsSL --proto '=https' --tlsv1.2 -o "$dir/$SUMS" "$base/$SUMS"

    expected="$(expected_sha256 "$dir/$SUMS" "$ARCHIVE")"
    [ -n "$expected" ] || die "$SUMS has no entry for $ARCHIVE."
    actual="$(shasum -a 256 "$dir/$ARCHIVE" | awk '{ print $1 }')"
    if [ "$actual" != "$expected" ]; then
        die "SHA-256 mismatch for $ARCHIVE (expected $expected, got $actual). Nothing was installed."
    fi
    say "Checksum OK."
}

# Prints the hash of the first code-signing identity with this name, or nothing.
find_identity() {
    security find-identity -p codesigning 2>/dev/null |
        awk -v id="\"$1\"" 'index($0, id) { print $2; exit }'
}

# Creates a self-signed code-signing identity in the login keychain. It is created once, so
# macOS sees every later build as the same program and keeps the Accessibility grant.
create_identity() {
    local tmp
    tmp="$(mktemp -d)"
    printf '%s\n' '[req]' 'distinguished_name=dn' '[dn]' '[ext]' \
        'basicConstraints=critical,CA:false' \
        'keyUsage=critical,digitalSignature' \
        'extendedKeyUsage=critical,codeSigning' > "$tmp/ext.cnf"
    # /usr/bin/openssl (LibreSSL) writes PKCS#12 that `security` accepts; Homebrew's OpenSSL 3 does not.
    if /usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -subj "/CN=$IDENTITY" \
            -keyout "$tmp/key.pem" -out "$tmp/cert.pem" -config "$tmp/ext.cnf" -extensions ext \
            >/dev/null 2>&1 \
        && /usr/bin/openssl pkcs12 -export -inkey "$tmp/key.pem" -in "$tmp/cert.pem" \
            -name "$IDENTITY" -out "$tmp/id.p12" -passout pass:mousex >/dev/null 2>&1 \
        && security import "$tmp/id.p12" -k "$HOME/Library/Keychains/login.keychain-db" \
            -P mousex -T /usr/bin/codesign >/dev/null; then
        say "Created the local signing identity \"$IDENTITY\"."
    else
        warn "could not create the local signing identity."
    fi
    rm -rf "$tmp"
}

# Signs the binaries with a stable local identity. Failure only costs a re-grant after updates.
sign_binaries() {
    local dir="$1" hash="" bin
    hash="$(find_identity "$LEGACY_IDENTITY" || true)"
    [ -n "$hash" ] || hash="$(find_identity "$IDENTITY" || true)"
    if [ -z "$hash" ]; then
        create_identity
        hash="$(find_identity "$IDENTITY" || true)"
    fi
    if [ -z "$hash" ]; then
        warn "no code-signing identity; Accessibility will have to be granted again after each update."
        return 0
    fi
    for bin in mousex mousex-agent; do
        if ! codesign --force --sign "$hash" --identifier "com.mousex.$bin" "$dir/$bin" >/dev/null 2>&1; then
            warn "signing $bin failed; Accessibility will have to be granted again after each update."
        fi
    done
}

install_mousex() {
    local pkg="$1" args=(install --role client)
    if [ -n "${MOUSEX_MACHINE:-}" ]; then
        args+=(--machine "$MOUSEX_MACHINE")
    fi
    say "Installing MouseX as a client ..."
    "$pkg/mousex" "${args[@]}"
    "$pkg/mousex" status || warn "'mousex status' reported a problem; see the output above."
}

explain_next_steps() {
    local settings="x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
    say ""
    say "MouseX is installed. One manual step remains:"
    say "  1. Allow mousex-agent in System Settings > Privacy & Security > Accessibility."
    say "     The settings page is opening now."
    open "$settings" >/dev/null 2>&1 || true
    say "  2. Restart the agent so it picks up the permission:"
    say "       launchctl kickstart -k \"gui/$(id -u)/com.mousex.agent\""
}

main() {
    require_environment

    local base="$REPO_URL/latest/download" tmp pkg
    if [ -n "${MOUSEX_VERSION:-}" ]; then
        [[ "$MOUSEX_VERSION" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.]+)?$ ]] \
            || die "MOUSEX_VERSION '$MOUSEX_VERSION' is not a tag such as v1.0.0."
        base="$REPO_URL/download/$MOUSEX_VERSION"
    fi

    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    download_package "$base" "$tmp"

    pkg="$tmp/package"
    mkdir -p "$pkg"
    tar -xzf "$tmp/$ARCHIVE" -C "$pkg"
    [ -x "$pkg/mousex" ] || die "mousex is missing from the package."

    sign_binaries "$pkg"
    install_mousex "$pkg"
    explain_next_steps
}

main "$@"
