# Changelog

All notable changes to MouseX are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.4.0] - 2026-10-28

### Added

- Linux: MouseX is in the applications menu. Opening it starts MouseX if it is stopped and shows the status window; right-click it for **Restart** and, on the main computer, **Arrange screens**. The status window has a **Restart** button (**Start** while stopped).
- Windows clients: a LocalSystem service starts an input-only helper at boot, so an already-paired computer can receive keyboard and mouse input at the Windows login and lock screens. The normal user agent takes over after sign-in and forwards secure-desktop input through a local pipe restricted to SYSTEM and Administrators. The helper does not handle clipboard, files or UI. Ctrl+Alt+Del is not forwarded.

### Fixed

- Linux main computer: after the screen was locked the cursor could stop crossing until MouseX was restarted by hand. When libei or the capture ends, the agent now restarts by itself; behind the lock screen it waits for the unlock before asking again to allow the capture.
- A screen that reconnects with a new ID (flaky USB-C/HDMI cable, reboot) takes over its old place in the layout instead of being added again as a duplicate.

## [1.3.0] - 2026-10-27

### Added

- macOS as the main computer: `mousex install --role server` captures the keyboard, mouse and wheel with an event tap and sends them to the other computers, with the same hotkeys as Windows (Cmd+Option+Shift+1…9 jumps to a computer, F14 locks the cursor to the screen, Cmd+Option+Shift+Home brings the cursor back). The menu bar icon gets the main computer menu: computers, pause, lock the cursor, reconnect, log and About. Needs the **Accessibility** and **Input Monitoring** permissions. Media keys are not captured yet, and the screen editor is not bundled for macOS yet.
- Linux as the main computer, on Wayland: the agent places pointer barriers with the InputCapture portal on the screen edges that touch another computer, and while the cursor is on another computer it receives keys, motion, buttons and the wheel through libei. The other edges stay free, so a dock that hides at the bottom and the hot corners keep working; when the layout changes, the barriers move without asking for the permission again. Switch with `mousex install --role server`. Tested on GNOME 46 (Ubuntu 24.04), which asks to allow the capture every time MouseX starts. X11 sessions cannot be the main computer yet. Experimental.
- `mousex monitor` opens a live status window in the browser (`127.0.0.1:41416`): state, role, service, pointer, last input and the recent log. It is available on every platform.
- Linux main computer: `mousex side left|right [MACHINE]` puts the other computer on that side of your screens (new computers still start on the right), and `mousex arrange` opens a screen editor in the browser, with the same rules as **Arrange screens…** on Windows. The Linux screens keep the system's arrangement; you move the other computers' screens around them. The status window has the same side buttons and a link to the editor. Changes apply right away, without restarting the agent.
- Linux: an optional tray icon with the same menu as Windows, built with `--features tray` (needs the libdbus headers).
- Copying files: the computer that receives them shows "N files ready — paste with Ctrl+V" (Cmd+V on a Mac). A copy that takes longer than a second shows a progress window with the name, percentage and speed, and a **Cancel** button on Windows that stops the copy and deletes what had arrived. macOS shows the progress without Cancel; Linux shows only the notice.
- Windows: Print Screen opens the system screen capture, as on a Mac, when that option was never set on the computer.

### Fixed

- A screen taller than the one beside it no longer has a wall along the rest of its edge. Leaving it anywhere on that edge goes to the nearest point the two screens share, so the cursor cannot get stuck on the other computer.
- Failures while receiving files or writing the clipboard are written to the log; before, the copy could vanish without a trace.

### Security

- The status window (`127.0.0.1:41416`) only answers requests addressed to `127.0.0.1` or `localhost`, so a web page cannot read it through DNS rebinding.

## [1.2.0] - 2026-10-27

### Added

- Windows: the last item of the tray menu shows the installed version; clicking it opens **About MouseX** (version, author, license and project page).
- Arrange screens: the edges where two screens touch are highlighted on the canvas, which shows where the cursor crosses between them.
- Arrange screens: **Undo** and **Redo** with Ctrl+Z, Ctrl+Shift+Z and Ctrl+Y. A whole drag, a slider movement or a held arrow key counts as one step.
- Arrange screens: light and dark themes follow the system setting, and the fonts are bundled with the app (no requests to Google Fonts).
- Arrange screens: **Identify screens** shows a large number on every connected screen for 3 seconds, numbered left to right (ties broken top to bottom), on every computer (Windows, macOS and Linux X11; Wayland cannot position windows).
- Mac and Linux client: menu bar icon (macOS) or tray icon (Linux, via StatusNotifierItem) showing the connection status, **Open log** and the version with **About MouseX**.

### Fixed

- Print Screen on the main computer works on a Mac receiving the cursor: it opens the area capture (Cmd+Shift+4), and Shift+Print Screen opens the full-screen capture (Cmd+Shift+3). On Windows and Linux it is passed through as the normal Print Screen key.

## [1.1.6] - 2026-10-27

### Changed

- Automatic updates are checked every 30 minutes instead of once a day. A check is a single request to GitHub; the package is downloaded only when there is a new version.
- Windows: the MouseX agent re-creates the update task every time it starts, so the task follows the schedule of the installed version and comes back if it was deleted.

### Security

- Windows: the scheduled task definition is written to the MouseX folder in Program Files, which only administrators can change, instead of the user's temporary folder, where another program running as that user could swap it before it was registered.

## [1.1.5] - 2026-10-27

### Fixed

- After changing a screen's resolution the cursor could only reach part of it. A client screen that grows or shrinks now keeps touching its neighbors (it grows toward the free side). With no free side it grows to the left and pushes the screens on its left along, so the desk arrangement stays the same. If even that would move this computer's screens, that computer's screens are placed again instead of keeping the old size. Changes to this computer's own screens are noticed within 2 s.

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

[Unreleased]: https://github.com/actus7/mousex/compare/v1.4.0...HEAD
[1.4.0]: https://github.com/actus7/mousex/compare/v1.3.0...v1.4.0
[1.3.0]: https://github.com/actus7/mousex/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/actus7/mousex/compare/v1.1.6...v1.2.0
[1.1.6]: https://github.com/actus7/mousex/compare/v1.1.5...v1.1.6
[1.1.5]: https://github.com/actus7/mousex/compare/v1.1.4...v1.1.5
[1.1.4]: https://github.com/actus7/mousex/compare/v1.1.3...v1.1.4
[1.1.3]: https://github.com/actus7/mousex/compare/v1.1.2...v1.1.3
[1.1.2]: https://github.com/actus7/mousex/compare/v1.1.1...v1.1.2
[1.1.1]: https://github.com/actus7/mousex/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/actus7/mousex/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/actus7/mousex/releases/tag/v1.0.0
