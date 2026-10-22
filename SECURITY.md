# Security Policy

## Supported versions

| Version | Supported |
|---|---|
| 1.0.x | ✅ |
| Older | ❌ |

Security fixes go into the latest release, which every installation picks up by itself within a day (see [Signed automatic updates](#4-tampered-update)).

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

Report it privately with GitHub's private vulnerability reporting:

1. Go to the repository on GitHub.
2. Open **Security** → **Report a vulnerability**.
3. Describe the problem, the steps to reproduce it, the affected version, and the operating systems involved.

We aim to acknowledge a report within a few days and to keep you informed while we work on a fix. Once a fix is released, we will credit you in the release notes, unless you prefer to stay anonymous.

## Threat model

MouseX lets one computer send keyboard, mouse, clipboard, and files to other computers on the same local network. That makes the following attackers relevant.

### 1. Attacker on the local network

An attacker on the same Wi-Fi or LAN can see the traffic, send packets, and try to pretend to be a paired computer.

**Mitigations**

- All traffic uses QUIC with TLS 1.3. The payload is encrypted.
- Each device has its own key pair and certificate. Peers are identified by the fingerprint of their certificate.
- A peer is authorized only after the pairing step, or if its fingerprint is already in the local allowlist. The allowlist is the only thing that grants access across restarts.
- The pairing code is derived from the TLS session secret (TLS exporter, label `mousex pairing v1`) and the peer fingerprint. A man-in-the-middle who terminates two separate TLS sessions gets two different codes, so the two screens would not match.
- A peer that is rejected is remembered for 10 minutes, so it cannot retry pairing in a loop.
- Only one pairing prompt is open at a time.
- The firewall rule opened by the Windows installer covers only the local subnet (`remoteip=localsubnet`).

**Known limits**

- The TLS handshake accepts any certificate and checks only its signature. Access control happens afterwards, through the allowlist and the pairing code. This is by design, so that pairing works for new devices.
- Denial of service on the local network (flooding the UDP ports) is not prevented.
- On networks with client isolation disabled, every device on the network can reach the MouseX ports. Use the allowlist and the firewall as the boundary.

### 2. Local unprivileged process on the same computer

A program running as a normal user on the same computer may try to read MouseX data, impersonate the agent, or use its files.

**Mitigations**

- On Windows, the elevated agent keeps its data in `C:\ProgramData\MouseX`. The folder is writable only by Administrators. The agent refuses to use a folder that someone else created, and it checks the folder's access list.
- Files copied from the clipboard are read with a restricted (non-administrator) token, so a crafted file cannot use the agent's privileges.
- Files received from another computer are written to a private folder.
- Paths from other computers are validated. Device names and Windows reserved names (`CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`) are rejected. Symbolic links are not copied.
- Transfers are limited in size: files up to 2 GB in total and 10,000 entries per copy, and clipboard images up to 32 MB.

### 3. Rogue or mistaken pairing

A user is tricked into confirming a code that belongs to an attacker, or a device is paired by mistake.

**Mitigations**

- Pairing needs a human confirmation on **both** computers. Nothing is saved without it.
- The dialog shows the name and the code of the other device. Confirm only if the same code appears on the other machine.

**Known limit:** version 1.0 has no screen or command to revoke a paired device yet. Removing a device means editing `allowlist.json` in the data folder (see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)). Adding a revoke command is on the roadmap.

### 4. Tampered update

Someone serves a modified MouseX package, or replays an old release with a known bug as if it were new.

**Mitigations**

- Every release has a `SHA256SUMS.sig`: an Ed25519 signature over the release tag plus `SHA256SUMS`. The public key is built into MouseX (`src/update.rs`); the private key is only in the `release` environment of the repository and with the maintainer. That environment requires the maintainer's approval for every run and accepts only `v*` tags, and a ruleset lets only administrators create, move or delete `v*` tags. A workflow change merged by mistake therefore cannot reach the key without an approved release.
- Automatic updates install nothing unless the signature is valid **and** the package hash matches. Because the tag is part of the signed text, an old release cannot pass as a newer one, and only versions higher than the running one are installed.
- Downloads use HTTPS only (`curl --proto =https --proto-redir =https --tlsv1.2`). On Windows the update runs as SYSTEM and stages files inside `Program Files\MouseX`, which only administrators can write.
- If the new binary does not start and report the expected version, the previous files are restored.

**Known limit:** whoever holds the signing key can ship an update to every installation. Set `"auto_update": false` in `config.json` to take updates manually.

## Data and privacy

- No cloud service, no account, and no telemetry. MouseX does not send data to the internet.
- Keys and the allowlist live on each computer, in the data folders listed in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
- Clipboard content and files pass only between your paired computers.
- Logs stay on the computer that writes them. Check them before you share them in a bug report.

## Out of scope

The following are not covered by this policy:

- An attacker with physical access to an unlocked computer, or with administrator rights on it.
- Malware that already runs with administrator privileges on a computer.
- Compromise of a paired computer's operating system or of its user account.
- Attacks on the Wi-Fi network itself (for example, cracking the Wi-Fi password).
- Keyboard logging or screen capture by other software on the same computer.
- Attacks on third-party software MouseX depends on. Report those to the upstream project. We will update our dependencies when fixes are available.
- Issues that only affect unsupported versions.

## Hardening tips

- Use a home or office network you trust, and keep your router's firmware up to date.
- Pair only computers that you own.
- Review the list of paired devices in `allowlist.json` from time to time, and remove any device you do not recognize.
- Keep your operating systems and MouseX updated.
