# Changelog

All notable changes to MouseX are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.1.4] - 2026-10-27

### Added

- **Arrange screens…**: a **Connected screens only** button removes ghost screens (computers or monitors that are no longer connected). They could push newly connected screens away from yours, so the cursor stopped crossing between computers and only the hotkeys worked.
- **Arrange screens…**: **Save** no longer closes the editor. The new layout applies at once, so you can test the cursor and keep adjusting.
- Linux as a client computer: keyboard, mouse, media keys, clipboard (text, images, files and folders), screen lock and automatic updates, installed with the same `install.sh` as macOS. X11 sessions use XTest; Wayland sessions use the desktop's remote control permission (RemoteDesktop portal with libei, GNOME 46 or KDE Plasma 6.1 or newer), which is experimental.

## [1.1.3] - 2026-10-22

### Added

- Windows: the agent restarts by itself if it crashes. The scheduled task now runs a small supervisor that keeps the real agent running (Task Scheduler only restarts tasks that fail to start, not ones that exit with an error).
- Windows: if Windows silently removes the keyboard and mouse hooks (it does that to hooks that respond too slowly), MouseX notices within a few seconds and installs them again.

## [1.1.2] - 2026-10-22

### Fixed

- Windows: after an automatic update the agent could exit with "address in use" (error 10048) because the previous one had not released its ports yet, leaving MouseX stopped until the next sign-in. The agent now waits up to 15 s for the ports.

## [1.1.1] - 2026-10-22

### Fixed

- macOS installer: no more `tmp: unbound variable` at the end, and the temporary folder is removed.
- macOS installer: asks for Accessibility only when the agent actually lacks it (updates keep the permission).

## [1.1.0] - 2026-10-22

### Added

- Automatic updates: every computer installs new releases by itself, once a day, only when they carry a valid Ed25519 signature (`SHA256SUMS.sig`). Failed updates roll back. Turn off with `"auto_update": false` in `config.json`; update now with `mousex update`. Installations of 1.0.0 need one manual update to get this.
- `mousex version` prints the version.
- Screens of the main computer can be dragged in **Arrange screens…**; on save, Windows is rearranged to match, so the arrangement no longer has to be repeated in Windows Settings.

### Fixed

- A client screen that disappears (cable swapped, monitor turned off) is no longer a cursor destination; it keeps its place in the layout for when it comes back.
- A client screen that changes resolution is resized in the layout, keeping its position and size on desk.

## [1.0.0] - 2026-10-22

First public release.

### Added

- **Shared desk:** move the cursor across screen edges between a Windows main
  computer and other Windows PCs or Macs, monitor by monitor, without any
  network address to type.
- **Arrange screens editor:** a visual editor to match the screen layout to
  your real desk, with a "size on desk" slider for physically larger or smaller
  screens. The main computer's screens always follow Windows' real arrangement.
- **Clipboard sync:** text, images (PNG, DIB, and TIFF), and files and folders
  (up to 2 GB), in both directions between Windows and Mac. Symbolic links are
  skipped.
- **Pairing:** each device has its own key pair. Pairing shows the same 6-digit
  code on both machines, which must be confirmed on both. Paired devices are
  remembered across reboots.
- **Encrypted transport:** QUIC with TLS 1.3. Mouse motion uses unreliable
  datagrams with sequence numbers, so the cursor never drifts; keys, clicks, and
  clipboard use reliable streams.
- **Automatic discovery:** mDNS, UDP broadcast, and a last-known address cache.
  No IP addresses to configure.
- **Automatic reconnection** with backoff, a 500 ms keepalive, and release of
  all held keys and buttons when a link drops.
- **Install once:** one command per Windows PC (`install.ps1`, or the
  `mousex.exe` download) and one command on macOS (`install.sh`). The Windows
  installer opens the firewall for the local subnet only. The macOS installer
  signs the binaries locally, so the Accessibility permission survives updates.
- **Tray icon** on Windows: machines numbered left to right with connection
  status, Pause sharing, Lock cursor to screen, Reconnect, Arrange screens, and
  Open log.
- **Hotkeys:** `Ctrl+Alt+Shift+1…9` to jump to a machine, `Scroll Lock` to lock
  the cursor to the current machine, and `Ctrl+Alt+Shift+Home` to bring the
  cursor home.
- **Keep the desk awake:** other machines stay awake while you are active on
  any of them. Locking the main PC (Win+L) locks the others too. Idle locks are
  not propagated.
- **Media keys** (play/pause, next, previous, volume, mute) work on the Mac.
- **Keyboards:** ABNT2 and international layouts through scan codes. Ctrl and
  Cmd are swapped when controlling a Mac. A one-time hint appears on a Mac whose
  keyboard layout does not match ABNT2.
- **Live monitor changes:** hot-plugging or rearranging monitors is detected on
  the server and on the clients.
- **Languages:** English and Portuguese, chosen from the operating system's
  language.
- **Security hardening:** pinned per-device certificates, pairing confirmation
  on both sides with memory of rejected devices, restricted-token reads for
  copied files, a private folder for received files, validation of device paths
  and Windows reserved names, and a Windows data folder writable only by
  Administrators.

[Unreleased]: https://github.com/actus7/mousex/compare/v1.1.4...HEAD
[1.1.4]: https://github.com/actus7/mousex/compare/v1.1.3...v1.1.4
[1.1.3]: https://github.com/actus7/mousex/compare/v1.1.2...v1.1.3
[1.1.2]: https://github.com/actus7/mousex/compare/v1.1.1...v1.1.2
[1.1.1]: https://github.com/actus7/mousex/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/actus7/mousex/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/actus7/mousex/releases/tag/v1.0.0
