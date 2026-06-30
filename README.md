# KitonyTerms

**English** | [中文](README.zh-CN.md)

A lightweight, cross-platform SSH terminal client built in **Rust** with [Dioxus](https://dioxuslabs.com/) — a modern desktop framework leveraging native WebView for the UI layer while keeping SSH/terminal logic in pure Rust.

> **Goals:** Fast startup, low memory footprint, native system integration, and a single compact binary for macOS / Windows / Linux (x86_64 + aarch64).

## Status

**Functional** — core SSH engine, terminal emulation, remote system monitoring, SFTP file management, host-key confirmation, and interactive auth prompts are working. The UI is built with Dioxus 0.7 desktop.

| Crate | Responsibility | Tests |
|-------|---------------|-------|
| `kt-secrets` | Encrypted-at-rest secret vault: Argon2id + XChaCha20-Poly1305 for SSH passwords and key passphrases | 6 ✅ |
| `kt-config` | TOML session profiles & app settings + `~/.ssh/config` parsing/merging | 21 ✅ |
| `kt-core` | SSH client (`russh`) + terminal engine (`alacritty_terminal`) + session orchestration + SFTP support + remote system monitoring. **No UI dependencies.** | 33 ✅ |
| `kt-ui` | Dioxus components and UI state: terminal workbench, dialogs, SFTP tree/actions, system monitor, selector-driven main shell | 70 ✅ |
| `kt-app` | Main binary: GUI-only Dioxus desktop entry, CLI argument validation, app icon integration | 8 ✅ |

**138 tests pass; `clippy` is clean.** The core validation is an **in-process round-trip integration test** ([`kt-core/tests/roundtrip.rs`](crates/kt-core/tests/roundtrip.rs)) that spins up a real `russh` SSH server on loopback and drives the full `SessionManager` path: `connect → password auth → PTY → shell → channel data → TermEngine → GridSnapshot`, asserting that server output and echoed keystrokes actually land in the rendered grid.

### What works today

- **SSH Terminal**: Connect via password / public-key / keyboard-interactive auth
- **Multiple Sessions & Split Panes**: Tabbed interface with per-session scrollback and resize, plus horizontal/vertical terminal split views
- **Terminal Features**: True color, bold/italic/underline/strikethrough, block/bar/underline cursor
- **Interactive Auth Prompts**: Password, private-key passphrase, and keyboard-interactive prompts can be collected mid-handshake when saved secrets are missing
- **Session Persistence**: Save connections to `config.toml`; passwords and key passphrases are encrypted in the local vault
- **Automatic Secret Vault**: The local vault is created/opened automatically, so saved passwords can be reused on reconnect without an extra vault password prompt
- **SFTP Panel**: Browse remote filesystem, upload/download files, create directories, delete/rename, and edit remote files via a local editor upload flow
- **System Monitoring**: Real-time remote CPU, memory, network, disk, load, uptime, and latency summary
- **SSH Config Integration**: Reads `~/.ssh/config` for host aliases, defaults, and single-hop `ProxyJump`
- **Host-Key Trust Store**: Persists `known_hosts.toml`; unknown or changed fingerprints require user confirmation with "allow once" or "trust host" choices
- **ssh-agent**: Supports local ssh-agent/Pageant public-key auth and can request agent forwarding for shell sessions
- **Trigger Highlighting**: Highlights terminal rows matching built-in trigger rules

### Not yet implemented

- Multi-hop ProxyJump chains
- Editable trigger rules and full syntax highlighting
- Non-GUI fallback entries such as `--safe`, `--system-ssh`, `--show-log`, and `--list` are intentionally not provided by the current binary

## Architecture

```
kt-app (Dioxus desktop binary)
   ├─ kt-ui (Dioxus components)
   │    └─ Terminal / Dialog / SFTP / Monitor views
   └─ kt-core (tokio runtime, background)
       ├─ ssh/      russh: connect, auth, PTY shell, SFTP subsystem
       ├─ term/     alacritty_terminal wrapper → GridSnapshot (resolved RGB)
       ├─ monitor/  Remote system resource monitoring (CPU, memory, disk, network)
       ├─ sftp/     SFTP task: list/upload/download/mkdir/remove/rename
       └─ session/  SessionManager: one task per session, UI⇄core message protocol
            │                         │
       kt-config             kt-secrets
       (TOML + ssh_config)   (Argon2id + XChaCha20 vault)
```

The terminal **engine** (VT parsing, grid, scrollback) is fully decoupled from **rendering**: the core produces an immutable `GridSnapshot` with colors resolved to 24-bit RGB, so the renderer needs no dependency on `alacritty_terminal`. The `alacritty_terminal` API (which is explicitly *not* stability-guaranteed) is contained entirely within `kt-core/src/term/`.

### Session & secrets storage

- **Sessions** (`SessionProfile`: host/port/user/auth/…) are **non-secret** and stored plaintext in `config.toml`.
- **Secrets** (passwords, key passphrases) are indexed by vault id (`user@host:port`) and encrypted in the local vault, never plaintext on disk.
- **Host keys** (host/port/fingerprint) are stored in `known_hosts.toml` to detect changed remote host keys.
- On startup the vault is opened automatically. Legacy master-password vaults that cannot be opened are backed up as `secrets.vault.legacy*`; new saved passwords continue into a fresh encrypted vault.

## Tech stack

- **SSH:** [`russh`](https://crates.io/crates/russh) 0.61 (pure Rust, async)
- **Terminal backend:** [`alacritty_terminal`](https://crates.io/crates/alacritty_terminal) 0.26 (pinned)
- **UI framework:** [Dioxus](https://dioxuslabs.com/) 0.7 desktop (wry + tao + native WebView)
- **Async runtime:** `tokio`
- **Crypto:** `argon2`, `chacha20poly1305`, `zeroize`
- **Config:** `serde` + `toml`, `directories`

## Building & running

Requires Rust toolchain (stable, 1.85+) and platform-specific dependencies:

### Linux (Ubuntu/Debian)
```bash
sudo apt install libwebkit2gtk-4.1-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libxdo-dev \
  libssl-dev \
  pkg-config
```

### macOS
No extra dependencies — the system WebKit is used.

### Windows
No extra dependencies.

### Build & run
```bash
# Run all tests
cargo test --workspace

# Quality gate used by maintainers
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# Launch the GUI
cargo run -p kt-app
# Or explicitly select the GUI entry / print current entry usage
cargo run -p kt-app -- --gui
cargo run -p kt-app -- --help
# Removed entries fail clearly: --safe, --system-ssh, --show-log, --list
#   Click ➕ New → enter host / user / auth → Connect
#   Tick "Save session" to persist it; password encrypted in vault
#   Click sidebar session to reconnect (password auto-filled)
#   Terminal view: click to focus, type; mouse wheel scrolls; Cmd/Ctrl +/− zoom

# Try the headless SSH client (exercises full core pipeline in terminal)
cargo run -p kt-core --example headless -- user@host
#   Auth: tries ~/.ssh/config + default keys, then keyboard-interactive, then password
#   Quit: Ctrl-]
```

## Roadmap

- [x] **Phase 1** — Core engine (SSH + terminal + sessions), verified end-to-end
- [x] **Phase 2** — Dioxus desktop UI: terminal rendering, input, connect dialog, multi-tab
- [x] **Phase 3** — Session persistence (TOML + encrypted vault), SFTP panel, system monitor
- [x] **Phase 4** — `known_hosts` trust store, split panes, ssh-agent forwarding, ProxyJump, triggers/highlighting
- [x] **Phase 5** — UI modularization: state controller, main shell split, selector-driven SFTP/monitor/status views
- [x] **Phase 6** — Documentation and engineering convergence: README/architecture/QA paths/release notes
- [x] **Phase 7** — Maintenance governance: impact checklist, regression suite, quarterly architecture review

## License

Apache-2.0
