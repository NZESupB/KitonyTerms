# KitonyTerms

**English** | [中文](README.zh-CN.md)

A lightweight, cross-platform SSH terminal client built in **Rust** with [Dioxus](https://dioxuslabs.com/) — a modern desktop framework leveraging native WebView for the UI layer while keeping SSH/terminal logic in pure Rust.

> **Goals:** Fast startup, low memory footprint, native system integration, and a single compact binary for macOS / Windows / Linux (x86_64 + aarch64).

## Status

**Functional** — core SSH engine, terminal emulation, system monitoring, and SFTP file transfer are working. The UI is built with Dioxus 0.7 desktop.

| Crate | Responsibility | Tests |
|-------|---------------|-------|
| `kt-secrets` | Master-password vault: Argon2id + XChaCha20-Poly1305 for encrypted-at-rest secrets (SSH passwords, key passphrases) | 6 ✅ |
| `kt-config` | TOML session profiles & app settings + `~/.ssh/config` parsing/merging | 9 ✅ |
| `kt-core` | SSH client (`russh`) + terminal engine (`alacritty_terminal`) + session orchestration + SFTP support + system monitoring. **No UI dependencies.** | 13 ✅ |
| `kt-ui` | Dioxus components: terminal view, connection dialog, SFTP panel, system monitor | — |
| `kt-app` | Main binary: integrates `kt-ui` with `kt-core` via async message channels | — |

**28 tests pass; `clippy` is clean.** The core validation is an **in-process round-trip integration test** ([`kt-core/tests/roundtrip.rs`](crates/kt-core/tests/roundtrip.rs)) that spins up a real `russh` SSH server on loopback and drives the full `SessionManager` path: `connect → password auth → PTY → shell → channel data → TermEngine → GridSnapshot`, asserting that server output and echoed keystrokes actually land in the rendered grid.

### What works today

- **SSH Terminal**: Connect via password / public-key / keyboard-interactive auth
- **Multiple Sessions**: Tabbed interface with per-session scrollback and resize
- **Terminal Features**: True color, bold/italic/underline/strikethrough, block/bar/underline cursor
- **Session Persistence**: Save connections to `config.toml`; passwords encrypted in master-password vault
- **Master Password**: Set on first run, prompted on later launches (skippable)
- **SFTP Panel**: Browse remote filesystem, upload/download files, create directories, delete/rename
- **System Monitoring**: Real-time CPU, memory, network, and disk usage (local system)
- **SSH Config Integration**: Reads `~/.ssh/config` for host aliases and default settings

### Not yet implemented

- Real `known_hosts` trust store (currently trust-on-first-use)
- Mid-handshake auth prompts (password collected upfront in connect dialog)
- SSH agent forwarding, ProxyJump, split panes, triggers/syntax highlighting

## Architecture

```
kt-app (Dioxus desktop binary)
   ├─ kt-ui (Dioxus components)
   │    └─ Terminal / Dialog / SFTP / Monitor views
   └─ kt-core (tokio runtime, background)
       ├─ ssh/      russh: connect, auth, PTY shell, SFTP subsystem
       ├─ term/     alacritty_terminal wrapper → GridSnapshot (resolved RGB)
       ├─ monitor/  System resource monitoring (CPU, memory, disk, network)
       ├─ sftp/     SFTP task: list/upload/download/mkdir/remove/rename
       └─ session/  SessionManager: one task per session, UI⇄core message protocol
            │                         │
       kt-config             kt-secrets
       (TOML + ssh_config)   (Argon2id + XChaCha20 vault)
```

The terminal **engine** (VT parsing, grid, scrollback) is fully decoupled from **rendering**: the core produces an immutable `GridSnapshot` with colors resolved to 24-bit RGB, so the renderer needs no dependency on `alacritty_terminal`. The `alacritty_terminal` API (which is explicitly *not* stability-guaranteed) is contained entirely within `kt-core/src/term/`.

### Session & secrets storage

- **Sessions** (`SessionProfile`: host/port/user/auth/…) are **non-secret** and stored plaintext in `config.toml`.
- **Secrets** (passwords, key passphrases) are indexed by vault id (`user@host:port`) and encrypted in the vault, never plaintext on disk.
- On startup the vault is **locked** until the master password is entered in the unlock dialog; skipping unlock allows connections but disables saved-password reading/writing.

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
cargo test

# Launch the GUI
cargo run -p kt-app
#   First run: set a master password (skippable)
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
- [x] **Phase 3** — Session persistence (TOML + vault), master password, SFTP panel, system monitor
- [ ] **Phase 4** — `known_hosts` trust store, split panes, ssh-agent forwarding, ProxyJump, triggers/highlighting

## License

Apache-2.0
