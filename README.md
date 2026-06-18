# KitonyTerms

**English** | [中文](README.zh-CN.md)

A lightweight, cross-platform SSH client — a spiritual successor to
[WindTerm](https://github.com/kingToolbox/WindTerm), built in **Rust** with
**native GPU rendering** (no Electron, no WebView).

> **Goals:** fast startup, low memory, native feel, one small binary on
> macOS / Windows / Linux.

## Status

Phase 1 (core engine) and Phase 2 (a working GUI) are **functional**. Polish and
advanced features (Phase 3) are ongoing.

| Crate | What it does | Tests |
|-------|--------------|-------|
| `kt-secrets` | Master-password vault: Argon2id + XChaCha20-Poly1305, encrypted-at-rest secrets (SSH passwords, key passphrases) | 6 ✅ |
| `kt-config` | TOML session profiles & app settings + `~/.ssh/config` parsing/merging | 9 ✅ |
| `kt-core` | SSH client (`russh`) + terminal engine (`alacritty_terminal`) + session orchestration. **No UI deps.** | 13 ✅ |
| `kt-app` | egui/eframe + wgpu (Metal/Vulkan/DX12) GUI: tabs, terminal rendering, input, connect dialog | — |

**28 tests pass; `clippy` is clean.** The headline core result is an **in-process
round-trip integration test** (`kt-core/tests/roundtrip.rs`) that stands up a
real `russh` SSH server on loopback and drives the real `SessionManager` through
the entire path: `connect → password auth → PTY request → shell → channel data
→ TermEngine → GridSnapshot`, asserting that server output and echoed keystrokes
actually land in the rendered grid. The GUI launches on macOS via the wgpu Metal
backend and renders grids through egui's painter.

### What works today

- Connect to an SSH host (password / public-key / agent-selection) via a dialog
- Interactive PTY shell rendered in a GPU-accelerated terminal view
- Multiple concurrent sessions in tabs; per-tab resize, scrollback, font zoom
- True color, bold/underline/strikeout, block/bar/underline cursor
- `~/.ssh/config` lookup + default identity files (headless example)
- **Session persistence**: save connections to `config.toml`; passwords are
  encrypted into the master-password vault and auto-filled on reconnect
- **Master-password unlock**: set on first run, prompted on later launches (skippable)
- **Sidebar**: saved-session list — click to reconnect, delete sessions

### Not yet wired

- Real `known_hosts` trust store (GUI currently trusts on first use).
- Mid-handshake auth prompts in the GUI (password is collected up front).
- SFTP, split panes, ssh-agent forwarding, ProxyJump, triggers/highlighting.

## Architecture

```
kt-app (egui/eframe + wgpu)         ← UI thread, immediate-mode
   │  ToCore (input/resize)   ▲ FromCore (render snapshot/events)
   ▼  via channels            │
kt-core (tokio runtime, background)
   ├─ ssh/      russh: connect, auth (password/pubkey/keyboard-int), PTY shell
   ├─ term/     alacritty_terminal wrapper → Send-able GridSnapshot (resolved RGB)
   └─ session   SessionManager: one task per session, UI⇄core message protocol
        │                         │
   kt-config             kt-secrets
   (TOML + ssh_config)       (Argon2id + XChaCha20-Poly1305 vault)
```

The terminal **engine** (VT parsing, grid, scrollback) is fully decoupled from
**rendering**: the core produces an immutable `GridSnapshot` with colors already
resolved to 24-bit RGB, so the renderer needs no dependency on
`alacritty_terminal`. The alacritty public API (which is explicitly *not*
stability-guaranteed) is contained entirely within `kt-core/src/term/`.

## Tech stack

- **SSH:** [`russh`](https://crates.io/crates/russh) 0.61 (pure-Rust, async)
- **Terminal backend:** [`alacritty_terminal`](https://crates.io/crates/alacritty_terminal) 0.26 (pinned)
- **GUI/GPU:** `eframe`/`egui` + `wgpu` (Phase 2)
- **Async:** `tokio`
- **Crypto:** `argon2`, `chacha20poly1305`, `zeroize`
- **Config:** `serde` + `toml`, `directories`

## Building & running

Requires a Rust toolchain (stable, 1.85+).

```bash
# Run all tests
cargo test

# Launch the GUI
cargo run -p kt-app
#   first run: set a master password (skippable)
#   click ➕ New → enter host / user / auth → Connect
#   tick "Save session" to add it to the sidebar; its password is encrypted in the vault
#   click a sidebar session to reconnect (password auto-filled)
#   click the terminal to focus it, then type; mouse-wheel scrolls; A+/A− zoom

# Try the headless SSH client (exercises the full core pipeline in your terminal)
cargo run -p kt-core --example headless -- user@host
#   auth: tries ~/.ssh/config + default keys, then keyboard-interactive, then password
#   quit: Ctrl-]
```

## Roadmap

- [x] **Phase 1** — core engine (SSH + terminal + sessions), verified end-to-end
- [x] **Phase 2** — GUI: wgpu terminal rendering, input, connect dialog, multi-tab
- [x] **Phase 3 (partial)** — session persistence (TOML + vault) + master-password unlock + sidebar
- [ ] **Remaining** — `known_hosts` trust store, split panes, SFTP panel, ssh-agent,
      ProxyJump, triggers/highlighting

## License

Apache-2.0
