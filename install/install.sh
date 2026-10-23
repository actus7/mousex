#!/usr/bin/env bash
# MouseX installer for macOS and Linux (client machine).
#   curl -fsSL https://github.com/actus7/mousex/releases/latest/download/install.sh | bash
# Optional environment variables:
#   MOUSEX_VERSION  release tag to install, e.g. v1.0.0 (default: latest)
#   MOUSEX_MACHINE  name of this computer in MouseX (default: the computer name)
set -euo pipefail

REPO_URL="https://github.com/actus7/mousex/releases"
OS="$(uname -s)"
ARCHIVE="MouseX-macos-universal.tar.gz"
[ "$OS" = "Linux" ] && ARCHIVE="MouseX-linux-x64.tar.gz"
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
    local tools
    case "$OS" in
        Darwin) tools="curl shasum tar codesign security launchctl" ;;
        Linux)
            [ "$(uname -m)" = "x86_64" ] || die "MouseX for Linux is built for x86_64 only (this is $(uname -m))."
            tools="curl sha256sum tar systemctl"
            ;;
        *) die "this installer is for macOS and Linux (use install.ps1 on Windows)." ;;
    esac
    [ "$(id -u)" -ne 0 ] || die "run this as your normal user, not with sudo: the agent runs in your session."
    local tool
    for tool in $tools; do
        command -v "$tool" >/dev/null 2>&1 || die "required tool not found: $tool"
    done
}

sha256_of() {
    if [ "$OS" = "Linux" ]; then sha256sum "$1"; else shasum -a 256 "$1"; fi | awk '{ print $1 }'
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
    actual="$(sha256_of "$dir/$ARCHIVE")"
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

explain_linux_next_steps() {
    say ""
    if ! command -v zenity >/dev/null 2>&1 && ! command -v kdialog >/dev/null 2>&1; then
        warn "neither zenity nor kdialog is installed: MouseX cannot ask you to confirm the pairing code."
        warn "install one with your package manager (for example: sudo apt install zenity) and run this again."
    fi
    if [ -n "${WAYLAND_DISPLAY:-}" ]; then
        say "MouseX is installed. The system now asks to allow remote control of this desktop:"
        say "allow it and choose to remember the choice, so it is not asked again at the next login."
    else
        say "MouseX is installed and running. Nothing else to do."
    fi
}

explain_next_steps() {
    local settings="x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
    local log="$HOME/Library/Application Support/mousex/mousex.log"
    say ""
    # The agent logs this warning when it starts without Accessibility. A stable signature keeps the
    # permission across updates, so most updates need nothing more.
    sleep 3
    if [ -f "$log" ] && ! tail -n 5 "$log" | grep -q "platform setup incomplete"; then
        say "MouseX is installed and running. Nothing else to do."
        return 0
    fi
    say "MouseX is installed. One manual step remains:"
    say "  1. Allow mousex-agent in System Settings > Privacy & Security > Accessibility."
    say "     The settings page is opening now."
    open "$settings" >/dev/null 2>&1 || true
    say "  2. Restart the agent so it picks up the permission:"
    say "       launchctl kickstart -k \"gui/$(id -u)/com.mousex.agent\""
}

main() {
    require_environment

    # tmp is global on purpose: the EXIT trap runs after main returns, when locals are gone.
    local base="$REPO_URL/latest/download" pkg
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

    if [ "$OS" = "Linux" ]; then
        install_mousex "$pkg"
        explain_linux_next_steps
    else
        sign_binaries "$pkg"
        install_mousex "$pkg"
        explain_next_steps
    fi
}

main "$@"
