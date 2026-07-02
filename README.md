# KitonyTerms

**English** | [中文](README.zh-CN.md)

KitonyTerms is a cross-platform SSH desktop client built in **Rust** with
[Dioxus Desktop](https://dioxuslabs.com/). It keeps SSH, terminal emulation,
SFTP, monitoring, configuration, and secret storage in Rust crates, while the
desktop UI is rendered through the native WebView stack.

## Current Shape

- **Primary app:** GUI-only desktop binary, `kitonyterms`, from `kt-app`.
- **Supported platforms:** macOS / Windows / Linux release artifacts for
  `x64` and `aarch64`. No 32-bit artifacts are produced.
- **Core engine:** pure-Rust SSH client, terminal grid, SFTP task, and remote
  monitor in `kt-core`, with no UI dependency.
- **UI:** Dioxus 0.7 desktop, native window, left connection/SFTP sidebar,
  central terminal workbench, monitor strip, status bar, dialogs, and settings.
- **Validation:** 182 tests currently pass across the workspace; clippy is run
  with `-D warnings`.

## What Works

- SSH terminal sessions with password, public key, keyboard-interactive, and
  ssh-agent/Pageant authentication.
- Saved sessions grouped in the sidebar, with reconnect, edit, copy, delete,
  and `~/.ssh/config` merge support.
- Host-key trust flow backed by `known_hosts.toml`, including unknown/changed
  key confirmation and one-time allow.
- Encrypted local secret vault for passwords and private-key passphrases,
  opened automatically by the UI on startup.
- Single-hop `ProxyJump`, TCP proxy modes (`Direct`, `System`, `SOCKS5`,
  `HTTP CONNECT`), and optional agent forwarding.
- Terminal rendering with RGB colors, common text attributes, cursor styles,
  scrollback, split views, trigger highlighting, optional line numbers, and
  optional timestamps.
- SFTP file browser with list, upload, download, mkdir, delete, rename, remote
  path navigation, terminal-directory follow, and remote editing through local
  editors.
- Editor settings for default editor selection and "Open With" entries.
- Remote CPU, memory, disk, network, load, uptime, and latency monitoring.
- Light/dark theme and Chinese/English UI language settings.

## Current Limits

- The main `kt-app` binary intentionally exposes only GUI entry points:
  no args, `--gui`, and `--help`. Historical flags such as `--safe`,
  `--system-ssh`, `--show-log`, and `--list` fail clearly.
- Multi-hop `ProxyJump` chains are not implemented.
- Trigger rules are not editable from the UI yet, and full syntax highlighting
  is not implemented.
- Release packages are currently unsigned / not notarized, so macOS Gatekeeper
  and Windows SmartScreen may require manual confirmation.
- The headless client exists as a `kt-core` example for debugging the core
  pipeline; it is not the main product surface.

## Quick Start

Requires Rust stable 1.85+.

### Linux Dependencies

Ubuntu/Debian:

```bash
sudo apt install libwebkit2gtk-4.1-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libxdo-dev \
  libssl-dev \
  pkg-config
```

macOS and Windows need no extra system packages for local development.

### Run The App

```bash
cargo run -p kt-app
```

Useful entry checks:

```bash
cargo run -p kt-app -- --gui
cargo run -p kt-app -- --help
```

In the UI, create a connection from the sidebar, choose authentication options,
connect, then save the session if you want it persisted. Saved passwords and key
passphrases go into the encrypted vault, not into `config.toml`.

## Developer Map

```text
kt-app
  Dioxus Desktop entry, window/icon/menu setup, minimal CLI argument handling

kt-ui
  Dioxus components, AppState/Store bridge, terminal workbench, SFTP sidebar,
  monitor UI, dialogs, settings, host-key/auth prompts

kt-core
  SessionManager, russh connection/auth, PTY shell, terminal engine,
  SFTP worker, remote monitor, UI <-> core message protocol

kt-config
  Config paths, TOML model, sessions, app settings, known_hosts, ssh_config merge

kt-secrets
  Argon2id + XChaCha20-Poly1305 vault for local secret storage
```

The important boundary is `kt-core`: it owns protocol and terminal behavior and
does not depend on the UI. `kt-ui` talks to it through `ToCore` / `FromCore`
messages and renders selector-style view models.

## Storage Model

- `config.toml`: non-secret session profiles and app settings.
- `known_hosts.toml`: trusted host-key fingerprints and last-seen metadata.
- `secrets.vault`: encrypted passwords and key passphrases.
- Legacy master-password vaults that cannot be opened automatically are backed
  up as `secrets.vault.legacy*`; new secrets continue in a fresh encrypted
  vault.

Secret values should never be written to config files or logs.

## Validation

Maintainer gate:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Current test coverage by package:

| Area | Tests |
| --- | ---: |
| `kt-app` | 8 |
| `kt-config` | 24 |
| `kt-core` | 50 |
| `kt-secrets` | 6 |
| `kt-ui` | 94 |
| **Total** | **182** |

The core integration test at
[`crates/kt-core/tests/roundtrip.rs`](crates/kt-core/tests/roundtrip.rs)
starts a real in-process `russh` server on loopback and verifies the full path:
connect, password auth, PTY, shell data, `TermEngine`, and `GridSnapshot`.

## Release Automation

GitHub Actions has two packaging workflows:

- `.github/workflows/release.yml`: `v*` tags create formal GitHub Releases.
- `.github/workflows/alpha.yml`: branch pushes update the rolling `alpha`
  prerelease.

Both workflows share the same six-platform matrix and artifact naming:
Linux/macOS/Windows x `x64`/`aarch64`. Rust target triples still use standard
names such as `x86_64-pc-windows-msvc`.

## Roadmap Snapshot

- [x] Core SSH + terminal engine
- [x] Dioxus desktop GUI
- [x] Session persistence, encrypted vault, SFTP, monitor
- [x] Host-key trust flow, split views, ssh-agent, ProxyJump, trigger highlight
- [x] UI modularization and selector-driven shell panels
- [x] Release/alpha packaging and maintenance governance
- [ ] Multi-hop ProxyJump, editable trigger rules, richer terminal highlighting

## License

Apache-2.0
