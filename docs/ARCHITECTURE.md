# MouseX architecture

This document describes how MouseX is built: the crates, the processes, the wire protocol, how the screen layout drives the cursor, how computers find each other, and where data lives. Read it before a larger change.

## Crates

```mermaid
flowchart TB
    core["mousex-core (src/)<br/>protocol, QUIC, pairing, discovery,<br/>layout, router, keymap"]
    native["mousex-native (native/)<br/>Windows: hooks, injection, clipboard,<br/>tray, power, access checks"]
    macos["mousex-platform-macos (platform-macos/)<br/>macOS: CGEvent, displays,<br/>pasteboard, accessibility"]
    linux["mousex-platform-linux (platform-linux/)<br/>Linux: XTest or portal + libei,<br/>InputCapture, RandR, X11 clipboard, dialogs"]
    daemon["mousex (daemon/)<br/>binaries: mousex (CLI), mousex-agent<br/>client, server, installers"]
    app["mousex-app (app/src-tauri/)<br/>Arrange screens editor<br/>(separate Cargo workspace)"]

    daemon --> core
    daemon -. "Windows only" .-> native
    daemon -. "macOS only" .-> macos
    daemon -. "Linux only" .-> linux
    native --> core
    macos --> core
    app -- "reads and writes layout.json" --> core
```

| Crate | Path | Responsibility |
|---|---|---|
| `mousex-core` | `src/` | Platform-independent logic: the wire protocol (`proto.rs`), QUIC endpoints and certificate handling (`net.rs`), device identity (`identity.rs`), pairing codes and the allowlist (`pairing.rs`), discovery (`discovery.rs`), screen layout and edge routing (`router.rs`), and key maps between Windows, HID, macOS and Linux evdev (`keymap.rs`). |
| `mousex-native` | `native/` | Windows backend. Low-level keyboard and mouse hooks that never block, input injection, clipboard (text, DIB and PNG images, file lists), the tray icon and menu, power and session events, access checks, and helpers for the installer. |
| `mousex-platform-macos` | `platform-macos/` | macOS backend. Event injection with CoreGraphics, display detection, pasteboard access (text, PNG, TIFF), accessibility checks, and the keyboard-layout check. |
| `mousex-platform-linux` | `platform-linux/` | Linux backend. Input injection with XTest on X11, or with the RemoteDesktop portal and libei on Wayland, input capture for the main computer with the InputCapture portal and libei as receiver (`capture.rs`), display detection with RandR, the X11 clipboard (text, PNG, file lists) with XFixes change notifications, and the pairing dialog, lock and keep-awake through `zenity`/`kdialog`, `loginctl` and D-Bus. |
| `mousex` | `daemon/` | The product itself. The `mousex` CLI, the `mousex-agent` background process, the client and server state machines, clipboard and file transfer (`clip.rs`), the Windows installer (`install.rs`), the macOS and Linux installers (`mac_install.rs`, `linux_install.rs`, sharing `client_install.rs`), the status window (`monitor.rs`), and on a Linux main computer the side command and the screen editor (`side.rs`, `arrange.rs`). |
| `mousex-app` | `app/src-tauri/` | The **Arrange screens** editor: a Tauri window with a React front end (`app/src/`). It only runs while open. It is a separate Cargo workspace. |

Platform-specific code is isolated behind traits. The daemon has a `Platform` trait for the client side (keys, buttons, motion, clipboard, lock, keep-awake, monitors), and the server has its own event loop that consumes hook events.

## Processes

### Windows

```mermaid
flowchart LR
    logon(["User logs on"]) --> task["Scheduled task 'MouseX'<br/>runs elevated, at logon"]
    task --> agent["mousex-agent.exe<br/>no window"]
    agent --> net["QUIC endpoint<br/>UDP 41414"]
    agent --> disc["Discovery<br/>UDP 41415 and mDNS"]
    agent --> hook["Low-level hook thread<br/>keyboard and mouse"]
    agent --> tray["Tray thread<br/>icon, menu, session events"]
    tray -. "opens" .-> editor["mousex-editor.exe<br/>only while open"]
```

- **Scheduled task `MouseX`.** Created by the installer. It runs `mousex-agent.exe` at every logon with elevated rights. Those rights let the agent inject input into elevated windows and write to `ProgramData`.
- **Hook thread.** Captures keyboard and mouse events. It forwards them through a channel and returns at once, so input is never delayed.
- **Tray thread.** Shows the machine list with connection status, the menu, and the grey icon when nothing is connected. It also listens for session lock events (Win+L).
- **Editor.** Started from the tray menu. It is not a background service.

The `mousex.exe` CLI is the same program. It runs `install`, `uninstall`, and `status`, and it has a dialog for choosing the role.

### macOS

```mermaid
flowchart LR
    login(["User logs on"]) --> launchd["LaunchAgent<br/>com.mousex.agent"]
    launchd --> agent["mousex-agent<br/>user process"]
    agent --> net["QUIC endpoint<br/>UDP 41414"]
    agent --> disc["Discovery<br/>UDP 41415 and mDNS"]
    agent --> events["CGEvent injection<br/>and event taps"]
```

- **LaunchAgent `com.mousex.agent`** is installed in `~/Library/LaunchAgents`. It needs no administrator rights.
- **Accessibility.** The user grants Accessibility to `mousex-agent`. Without it, the agent can run but cannot inject or capture input.
- **Signing.** The installer signs the binaries with a local self-signed identity, so macOS keeps the Accessibility permission across updates.
- The macOS build is **client only**. It can receive the cursor, keys, clipboard and files, and it can lock its own screen.

### Linux

```mermaid
flowchart LR
    login(["User logs on"]) --> autostart["Autostart entry<br/>~/.config/autostart/mousex.desktop"]
    autostart --> service["systemd user service<br/>mousex.service"]
    service --> agent["mousex-agent<br/>user process"]
    agent --> net["QUIC endpoint<br/>UDP 41414"]
    agent --> disc["Discovery<br/>UDP 41415 and mDNS"]
    agent --> inject["XTest (X11) or<br/>RemoteDesktop portal + libei (Wayland)"]
```

- **Autostart entry.** Desktops run `~/.config/autostart/mousex.desktop` at login (GNOME, KDE, XFCE, Cinnamon and others). It passes `DISPLAY` and `WAYLAND_DISPLAY` to the systemd user manager and starts `mousex.service`, which restarts the agent if it exits. No administrator rights.
- **X11.** Keys, motion, buttons and the wheel go through XTest; screens come from RandR.
- **Wayland.** XTest would reach only XWayland windows, so the agent opens a RemoteDesktop portal session and talks libei (`ConnectToEIS`, portal version 2: GNOME 46, KDE Plasma 6.1 or newer). The desktop asks the user once; the restore token is kept in `portal-token` so later sessions start without asking. If the session ends (permission revoked, compositor restarted), the agent exits and systemd starts a fresh one. Without the portal, the agent falls back to XTest and logs why. Screens and the clipboard still come from XWayland, which mirrors the compositor's.
- **Main computer (Wayland).** The agent opens an InputCapture portal session and places pointer barriers only on the screen edges that touch another computer's screen in the saved layout. The other edges stay the desktop's, so a dock that hides at the bottom, hot corners and the top bar keep working. A barrier on the right or bottom edge sits one pixel past the last column or row, as the portal specification places them. GNOME 46 takes no new barriers while the session is enabled: when the layout changes, the agent disables the session, sets the new barriers and enables it again (if the cursor is on another computer, once it comes back). It is the same session, so the desktop does not ask again. When the cursor enters another computer, the agent keeps the capture and reads keys, motion, buttons and the wheel from libei (receiver context); keys arrive as evdev codes and become Windows scan codes. On the way back, the pointer is released 20 px inside the edge, so a small hand movement does not send it straight back. GNOME 46 has no restore token for InputCapture: it asks to allow the capture at every agent start. If libei or the session ends (locking the screen can do it), the agent exits and systemd starts a fresh one; started behind the lock screen, it waits for the unlock before asking. X11 sessions cannot be the main computer yet.
- **Status window.** The agent serves `127.0.0.1:41416` (`mousex monitor`). It reads the config, the service state, `activity.json` and the log. On a Linux main computer it also changes the layout, because the Tauri editor does not run there: `POST /api/side` (also used by `mousex side`) and the screen editor at `/arrange` with `GET` and `POST /api/layout`. Those requests are accepted only with `Host` 127.0.0.1 or localhost, and only from these pages or without an `Origin` (not from another site open in the browser). The agent rereads the layout in place, without a restart, since a restart means allowing the capture again.
- **Tray.** The default Linux build has no tray icon (the build machine needs the libdbus headers). `--features tray` builds one with the same menu as Windows.

## Roles and state machines

- **Server** (Windows, or Linux on Wayland): owns the physical keyboard and mouse. It accepts paired clients, tracks which machine the cursor is on, and sends `Enter`, `Leave`, keys, clicks, wheel events, and clipboard requests.
- **Client** (Windows, macOS or Linux): connects to the server, announces its name and monitors with `Hello`, and applies what the server sends. It reconnects with backoff starting at 250 ms and capped at 2 s. A connection that stayed up for at least 5 s resets the backoff. When the link drops, it releases every held key and button.

## Wire protocol

Three channels carry traffic over one QUIC connection.

| Channel | QUIC mechanism | Used for | Delivery |
|---|---|---|---|
| Control | One bidirectional stream, preceded by a single byte `0x53` | `Hello`, `Enter`, `Leave`, keys, clicks, wheel, clipboard pull, activity, lock | Reliable, in order |
| Motion | Unreliable datagrams | Absolute cursor position on a monitor | Unreliable; latest wins |
| Clipboard and files | One unidirectional stream per transfer, low priority (`-1`) for files | Text, PNG images, file lists | Reliable, one transfer per stream |

Other QUIC settings: ALPN `mousex/1`, keepalive every 500 ms, and idle timeout of 1.5 s. A connection that stops answering is dropped, and the client starts reconnecting.

### Control frames

Each control message is a frame:

```text
+----------------------+------------------------------+
| length: u32 (LE)     | body: JSON (serde, tagged)   |
+----------------------+------------------------------+
```

- The length must be greater than zero and at most **64 KiB** (`MAX_CONTROL_BYTES`).
- A message this version does not know, sent by a newer peer during an upgrade, is consumed and skipped. The reader moves on to the next frame. This keeps a mixed-version pair working during an update.

Control messages:

| Message | Direction | Meaning |
|---|---|---|
| `Hello { machine, monitors }` | client → server | On connect, and whenever the monitors change |
| `Enter { monitor_id, x, y }` | server → client | The cursor entered this client, at local coordinates |
| `Leave` | server → client | Release everything |
| `Key { hid, down }` | server → client | Keyboard key, as a USB HID usage |
| `Button { button, down, monitor_id, x, y }` | server → client | Mouse button, with the cursor position |
| `Wheel { dx, dy }` | server → client | Scroll |
| `ClipboardPull` | server → client | The cursor left; send the clipboard if it changed |
| `Activity` | server → client | A user is active somewhere; do not lock for idleness |
| `Lock` | server → client | The main PC was locked with Win+L; lock this screen too |

### Motion datagram

Motion uses a fixed 15-byte packet, all integers little-endian:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | Tag, always `0x4D` (`M`) |
| 1 | 4 | `seq`: `u32` sequence number |
| 5 | 2 | `monitor`: `u16`, index into the client's `Hello.monitors` |
| 7 | 4 | `x`: `i32`, local pixels |
| 11 | 4 | `y`: `i32`, local pixels |

A receiver ignores a packet that is not newer than the last one it applied. Comparison wraps around, so the sequence number can roll over. Lost packets are simply skipped, and the cursor never drifts.

### Clipboard and file streams

```text
+--------------+----------------------+------------------------------+
| kind: u8     | length: u32 (LE)     | payload                      |
+--------------+----------------------+------------------------------+
```

| Kind | Name | Payload | Limit |
|---|---|---|---|
| `1` | Text | UTF-8 text | 10 MiB |
| `2` | PNG | PNG image | 32 MiB |
| `3` | Files | JSON manifest, then the bytes of each file in manifest order | Manifest 4 MiB; 2 GiB total; 10,000 entries |

A file transfer sends the manifest (paths and sizes) first, then the contents. If a file shrinks during the copy, the sender aborts, and the receiver discards the partial copy. Transfers have a timeout that grows with the size of the copy.

## Screen layout and routing

### Layout

The layout is a list of monitors in **global desk coordinates**:

```json
{"monitores":[
  {"id":"casa:1","machine":"Este computador","x":0,"y":0,"width":3440,"height":1440,"scale_fator":1.0},
  {"id":"mac:1","machine":"mac","x":-1470,"y":242,"width":1470,"height":956,"scale_fator":1.0}
]}
```

- `x`, `y`, `width`, and `height` are integer global pixels. Bounds are half-open.
- `scale_fator` converts global pixels to a machine's logical coordinates: `local = (global - origin) / scale`.
- Monitor IDs must be unique and not empty. The `machine` name must not be empty. An empty layout is valid.
- **Home screens** (the screens of the computer with the physical keyboard) always use the real Windows coordinates. When the editor moves them, the server applies the new arrangement to Windows (`ChangeDisplaySettingsEx`, primary kept at 0,0) and shifts the whole saved layout to match, so the two never disagree. Home screens must touch each other, because Windows does not allow detached screens. On a Linux main computer the desktop owns the home screens: the editor keeps them fixed, and saving changes only the other machines' screens (screens that are not in the saved layout are ignored).
- Other machines' screens can be placed anywhere. The **size on desk** slider changes the area a screen occupies without changing its resolution.

### Router

The router turns cursor movement into machine changes:

1. **Home movement** comes from the hook. The router clamps the cursor to the desk bounds and checks whether it crossed the edge of a monitor.
2. **Edge detection** looks for a neighbor monitor on the other side of the edge. A transition only happens when the monitors are physically adjacent. There is no wrap-around. When the edge touches a neighbor along only part of its length (screens of different heights), leaving through the rest of the edge goes to the nearest point of the shared part, so it is never a wall; corners that touch at a single point still do not connect.
3. **Remote movement** comes from relative deltas. The router moves a virtual point on the desk, clamps it to the current monitor, and when it leaves the monitor through an edge, it finds the monitor on that side.
4. The router sends `Enter` to the new machine (or `Leave` back to home), at the matching local position.
5. **Jumps** (`Ctrl+Alt+Shift+N`, or the tray) move the cursor to the first monitor of machine *N*, counting left to right. **Force home** (the emergency hotkey) brings the cursor back to the home screen.
6. **Scroll Lock** toggles a lock that keeps the cursor on the current machine.

Movement is handled in two modes. While the cursor is at home, the hook's absolute positions are used. While it is remote, only relative deltas are used, and deltas that look implausible (too large in one event) are dropped.

## Discovery

The client wants a specific server fingerprint, or any server before pairing (`*`).

1. A **UDP broadcast** on port `41415` sends `MOUSEX?` (padded to at least 128 bytes). A server answers `MOUSEX!` with its fingerprint and QUIC port.
2. In parallel, an **mDNS** browse looks for `_mousex._udp.local.`.
3. The first match within the timeout (2 s) wins. A match for a known fingerprint is saved in `last_server.json`.
4. If nothing answers, the client uses the cached address for that fingerprint.

There is no IP address to configure.

## Pairing

```mermaid
sequenceDiagram
    participant C as Client
    participant S as Server
    participant U as Users

    C->>S: QUIC handshake (TLS 1.3, client certificate)
    S->>S: Is the fingerprint in allowlist.json?
    alt Known device
        S-->>C: Accepted, control stream opens
    else Unknown device
        S->>U: Dialog with 6-digit code (from TLS session secret)
        C->>U: Dialog with the same 6-digit code
        U-->>S: Confirm
        S->>S: Save fingerprint in allowlist.json
        S-->>C: Accepted
    end
```

- The **code** is an HMAC-SHA256 of the peer fingerprint, keyed with the TLS exporter secret (label `mousex pairing v1`), taken modulo 1,000,000. Both sides compute it from the same session, so they show the same code. A different TLS session gives a different code.
- The handshake accepts any certificate and checks its signature. Authorization is separate and happens after the handshake, through the allowlist and the human confirmation.
- A rejected device is remembered for 10 minutes. Only one pairing dialog is open at a time.
- The client remembers the server's fingerprint in `config.json` and reconnects only to it.

## Data locations

| Item | Windows (installed) | macOS | Linux |
|---|---|---|---|
| Data folder | `C:\ProgramData\MouseX` | `~/Library/Application Support/mousex` | `~/.local/share/mousex` |
| Who can write | Administrators only | The user | The user |
| Identity (certificate and key) | `identity.der`, `identity.pk8` | same | same |
| Paired devices | `allowlist.json` | same | same |
| Screen layout | `layout.json` | same | same |
| Last server address | `last_server.json` | same | same |
| Settings (role, machine name, pinned server) | `config.json` | same | same |
| Wayland permission (restore token) | – | – | `portal-token` |
| Log | `mousex.log`; when it passes about 1 MB, it is renamed to `mousex.log.old` and a new one starts | same | same |
| Installed binaries | `C:\Program Files\MouseX` | `~/Library/Application Support/mousex/bin` | `~/.local/share/mousex/bin` |
| Received files and folders | `recebidos` inside the data folder, one new subfolder per transfer; the two most recent are kept | same | same |

Before an install, a Windows user's data folder is `%APPDATA%\mousex`. The installer moves `identity.der`, `identity.pk8`, `allowlist.json`, `layout.json` and `last_server.json` to `ProgramData` and keeps the pairings. Data from older builds that used the name Sculpin is migrated the same way, but only from a folder that is protected like the new one.

The log file is `mousex.log`. Use the tray's **Open log** item, or `mousex status`, to see it.

## Installers

| Step | Windows | macOS | Linux |
|---|---|---|---|
| Admin rights | Yes (UAC) | No | No |
| Binaries | Copied to `C:\Program Files\MouseX` | Copied to `~/Library/Application Support/mousex/bin` | Copied to `~/.local/share/mousex/bin` |
| Autostart | Scheduled task `MouseX`, at logon, elevated | LaunchAgent `com.mousex.agent` | Autostart entry `mousex.desktop` starting the systemd user service `mousex.service` |
| Firewall | Rule `MouseX`, UDP 41414 and 41415, remote `localsubnet` | Not applicable | Not changed (most desktop distributions allow outgoing UDP) |
| Permission | None extra | Accessibility for `mousex-agent` | Wayland only: remote control, asked once by the desktop |
| Update | Task `MouseX Update` as SYSTEM, every 30 min and 10 min after boot; the agent re-creates it at start | The agent runs `mousex update` 10 min after start, then every 30 min | Same as macOS; the agent restarts through systemd |
| Uninstall | Settings → Apps, or `mousex uninstall` | `mousex uninstall` | `mousex uninstall` |

Updates keep pairings, the layout, and the identity, so already-paired machines reconnect without a new code. `mousex update` finds the newest tag through the `/releases/latest` redirect, downloads the package, `SHA256SUMS` and `SHA256SUMS.sig` with the system `curl`, checks the Ed25519 signature (tag + sums) and the hash, unpacks with the system `tar`, swaps each binary keeping a `.old` copy, and runs `mousex version` on the new one. If that fails the `.old` files come back. On macOS the new binaries are re-signed with the local identity before the agent restarts.

### Self-recovery

- **Agent crash (Windows).** The scheduled task starts `mousex-agent run`, which only supervises: it runs the real agent as a child (`mousex-agent worker`) inside a kill-on-close job object and starts it again if it exits with an error (1 s after a stable run, doubling up to 30 s on repeated crashes). Ending the task kills both, so no orphan keeps the ports. On macOS, `launchd` (`KeepAlive`) does the same; on Linux, systemd (`Restart=always`).
- **Silently removed hooks (Windows).** Windows removes a low-level hook that takes too long, without telling the program. A timer on the hook thread checks every 2 s: if the cursor moves for two checks in a row and the mouse hook saw nothing, both hooks are installed again. If that fails, the agent exits and the supervisor starts a fresh one.
- **Ports still held by the previous agent** (update, reinstall): the agent waits up to 15 s for them instead of exiting.
- **Network and peers.** Clients reconnect with backoff and find the server again by mDNS, broadcast or the last known address. QUIC keep-alives detect a vanished peer within seconds; the cursor comes home and that machine becomes a wall.

## Design decisions

- **QUIC with TLS 1.3** gives encryption, multiplexing, and datagrams in one protocol, without managing separate sockets.
- **Datagrams for motion** keep the cursor responsive. A late packet is worth nothing, so it is dropped instead of retransmitted.
- **JSON for control, binary for motion.** Control messages are infrequent, so readability wins. Motion is the hot path, so it has a fixed binary layout.
- **Hooks never block.** The OS disables a slow low-level hook, so all work is handed to other threads.
- **Allowlist as the only durable grant.** Nothing is trusted because it is on the local network. Trust is a file the user can read.
- **No central server.** Discovery is local, so the desk works without any service.
