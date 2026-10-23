# Contributing to MouseX

Thanks for helping. This guide covers how to set up a development environment, run the checks, test on two machines, and send a change.

Contributions are welcome in **English or Portuguese**. Please follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Before you start

- For a bug, search [existing issues](https://github.com/actus7/mousex/issues) first.
- For a larger change, open an issue or a draft pull request first, so we can agree on the approach.
- For security problems, do **not** open a public issue. See [SECURITY.md](SECURITY.md).

## Development setup

### Windows

1. Install the GNU Rust toolchain. MouseX builds with the GNU target on Windows.

   ```powershell
   rustup toolchain install stable-x86_64-pc-windows-gnu
   ```

2. Install MinGW-w64, which provides `windres` and `gcc`. Use [MSYS2](https://www.msys2.org/) or [WinLibs](https://winlibs.com/). Put its `bin` folder on your `PATH`.

   Check it:

   ```powershell
   windres --version
   gcc --version
   ```

3. Install Node.js 20 or newer. It is only needed for the editor in `app/`.

4. Clone the repository and build:

   ```powershell
   git clone https://github.com/actus7/mousex.git
   cd mousex
   cargo +stable-x86_64-pc-windows-gnu build
   ```

### macOS

1. Install Rust stable with [rustup](https://rustup.rs/).
2. Install Node.js 20 or newer.
3. Clone the repository and build:

   ```bash
   git clone https://github.com/actus7/mousex.git
   cd mousex
   cargo build
   ```

### Linux

1. Install Rust stable with [rustup](https://rustup.rs/). No system libraries are needed: the X11, D-Bus and libei clients are pure Rust.
2. For the X11 tests, install Xvfb (`sudo apt install xvfb` on Debian and Ubuntu) and run the tests under `xvfb-run -a`. Without a `DISPLAY` those tests return early.

The Windows native backend (`mousex-native`) does not build on macOS or Linux, so the workspace commands below use `--exclude mousex-native` there.

## Checks to run before a pull request

Run all of these. CI runs the same checks.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

On macOS and Linux, add `--exclude mousex-native` to the clippy and test commands (on Linux, run the tests under `xvfb-run -a`):

```bash
cargo clippy --workspace --all-targets --exclude mousex-native -- -D warnings
cargo test --workspace --exclude mousex-native
```

### Editor (`app/`)

```bash
cd app
npm ci
npm test
npm run build
```

To build the editor binary:

```bash
cd app/src-tauri
cargo build --release --features tauri/custom-protocol
```

The editor is a separate Cargo workspace, so the root commands above do not include it.

## Repository layout

| Path | What it is |
|---|---|
| `src/` | `mousex-core`: protocol, QUIC networking, pairing, discovery, screen layout, edge routing, keyboard maps |
| `native/` | `mousex-native`: Windows backend (input hooks, injection, clipboard, tray, power, access checks, installer helpers) |
| `platform-macos/` | `mousex-platform-macos`: macOS backend (CGEvent injection, displays, pasteboard, accessibility) |
| `platform-linux/` | `mousex-platform-linux`: Linux backend (XTest or RemoteDesktop portal with libei, RandR, X11 clipboard, dialogs) |
| `daemon/` | `mousex` crate: the `mousex` CLI, the `mousex-agent` background process, client and server logic, installers for Windows, macOS and Linux |
| `app/` | The **Arrange screens** editor (Tauri app with a React front end) |
| `assets/` | Logo and icons |
| `install/` | Install scripts that are delivered with the releases |
| `docs/` | Architecture notes and images for the docs |

For how the pieces fit together, read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Test-driven development

Write the test first when you can:

1. Write a test that fails for the behavior you want (**red**).
2. Write the smallest code that makes it pass (**green**).
3. Clean up the code while the tests stay green (**refactor**).

The project aims for at least 80% coverage on new logic. Pure logic, such as the protocol, the router, the keymap, and the layout code, is easy to test directly. Keep Windows and macOS API calls thin, so most logic can be tested without them.

## Code style and language

- Format with `cargo fmt`. The settings are in `rustfmt.toml`.
- No warnings. Clippy runs with `-D warnings`.
- Avoid `unwrap()` and `expect()` in production code. Use them in tests.
- Comments and tests are currently written in **Portuguese**. New comments may be in English or Portuguese. Pick the language of the surrounding code.
- Keep hooks and input callbacks fast. They must never block.

## Commit messages

We use [Conventional Commits](https://www.conventionalcommits.org/):

```text
<type>: <short description>

<optional body explaining why>
```

Types we use: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `perf`, `ci`.

Examples:

```text
feat: copy files and folders between Windows and Mac
fix(client): release held keys when the link drops
docs: explain the pairing code
```

## Testing on two machines

Unit tests cannot cover real input, the tray, or the network between computers. Test those on real hardware:

1. Build the agent and the CLI on each machine, or install a release build.
2. Install the main Windows PC first, then the second machine:

   ```powershell
   mousex.exe install --role server
   ```

   On the other machine:

   ```powershell
   mousex.exe install --role client --machine NAME
   ```

   Run these from an elevated terminal. See [README.md](README.md) for the regular install flow.

3. Confirm the pairing code on both screens.
4. Check the cursor crossing an edge, typing, copy and paste of text, an image, and a folder, and the tray menu.
5. Read the logs when something goes wrong:
   - Windows: `C:\ProgramData\MouseX\mousex.log`
   - macOS: `~/Library/Application Support/mousex/mousex.log`
- Linux: `~/.local/share/mousex/mousex.log`

When you report a bug, include the logs and the operating system versions. Remove anything private first.

## Pull requests

1. Create a branch from `master`.
2. Make your change with tests and, if needed, documentation.
3. Run the checks listed above.
4. Open a pull request. Fill in the template. Say which operating systems you tested on, and how.

Keep pull requests focused. A small, reviewed change merges faster than a large one.

Changes to `.github/workflows/` get extra scrutiny: the release workflow signs the packages that every installation updates to by itself.

## Releasing (maintainers)

1. Bump the version in `Cargo.toml`, `app/package.json`, `app/src-tauri/Cargo.toml` and `app/src-tauri/tauri.conf.json`, run `cargo check` in the root and in `app/src-tauri` to refresh the lockfiles, and date the section in `CHANGELOG.md`.
2. Wait for CI to pass on `master`, then push a `vX.Y.Z` tag (only administrators can create `v*` tags).
3. The release workflow builds both packages and then waits for approval of the `release` environment, which holds the signing key. Check that the run belongs to your tag and approve it.
4. Installed copies pick up the signed release by themselves within a day.

## License

By contributing, you agree that your contribution is licensed under the MIT License and the Apache License 2.0, the same as the project. See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
