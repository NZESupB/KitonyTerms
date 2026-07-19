# KitonyTerms

**English** | [中文](README.zh-CN.md)

KitonyTerms is a cross-platform SSH client built in **Rust** with
[Dioxus](https://dioxuslabs.com/). It keeps SSH, terminal emulation, SFTP,
monitoring, configuration, and secret storage in Rust crates, while desktop
and mobile UIs are rendered through native WebView stacks.

## Current Shape

- **Primary app:** GUI-only `kitonyterms` application from `kt-app`.
- **Supported platforms:** macOS / Windows / Linux desktop artifacts for `x64`
  and `aarch64`, plus Android / iOS mobile artifacts for `aarch64`. No 32-bit
  artifacts are produced.
- **Core engine:** pure-Rust SSH client, terminal grid, SFTP task, and remote
  monitor in `kt-core`, with no UI dependency.
- **UI:** Dioxus 0.7 desktop/mobile, native desktop window or mobile WebView,
  responsive connection/SFTP area, terminal workbench, monitor strip, status
  bar, dialogs, and settings.
- **Validation:** unit and integration tests cover every workspace crate; clippy
  is run with `-D warnings`.

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
- macOS and Windows desktop packages are not formally signed/notarized, so
  Gatekeeper and SmartScreen may require manual confirmation.
- Android APKs use a stable keystore and increasing `versionCode` values, so
  newer builds can replace older installations. iOS IPAs are intentionally
  unsigned and cannot be installed until the user signs them.
- The pipeline does not upload to TestFlight or the App Store. iOS update
  continuity depends on the user's re-signing setup, not on this CI pipeline.
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

### Mobile Packaging

Mobile builds are pinned to Dioxus CLI 0.7.9:

```bash
cargo install dioxus-cli --locked --version 0.7.9
dx bundle --release --platform android --target aarch64-linux-android --package-types apk --package kt-app
dx build --release --platform ios --target aarch64-apple-ios --package kt-app
```

Android additionally needs Android SDK 35, Build Tools 35.0.0, and NDK 27.2;
iOS compilation only needs Xcode. Create a protected `mobile-signing` Environment
for the Android job and configure these Environment secrets:

- Android (PKCS#12 keystore): `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`,
  `ANDROID_KEY_ALIAS`, and `ANDROID_CERT_SHA256`; add `ANDROID_KEY_PASSWORD`
  when it differs from the store password.

Both platforms use the fixed identifier `com.kitonyterms.app`. Never replace
the Android keystore after publishing. The iOS artifact contains no signing
identity or provisioning profile and must be re-signed before installation.
For later iOS builds to replace an existing installation, keep the same Apple
Team/application identifier, Bundle ID, and compatible entitlements, and do not
decrease the app version or build number. Some free-account signing tools may
rewrite the Bundle ID or issue short-lived signatures, so seamless updates are
not guaranteed.

Restrict the `mobile-signing` deployment branch/tag policy to `main` and formal
`v*` tags, and protect both `main` and `v*` tag creation so unreviewed workflow
changes cannot access production signing material. Mobile build numbers are
allocated through a fast-forward CAS counter at `refs/ci/mobile-build-number`
and must remain at or below Android's `2,100,000,000` versionCode limit.

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
  Dioxus desktop/mobile entry, desktop window/icon/menu setup, minimal CLI handling

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
- `secrets.vault.key`: per-install local vault key; keep it private together
  with the vault.
- On Android these files live under the app-private `files/config` and
  `files/data` directories.
- Legacy fixed-key vaults are migrated to the current per-install key at startup.
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
bash -n .github/scripts/allocate-mobile-build-number.sh
bash -n .github/scripts/package-android-apk.sh
bash -n .github/scripts/package-ios-ipa.sh
bash -n .github/scripts/publish-alpha.sh
```

The workspace test suite covers the app entry point, configuration and secret
storage, SSH/terminal/SFTP core behavior, UI state transitions, and pure UI
logic. Counts are intentionally not listed here because they change whenever
coverage grows.

The core integration test at
[`crates/kt-core/tests/roundtrip.rs`](crates/kt-core/tests/roundtrip.rs)
starts a real in-process `russh` server on loopback and verifies the full path:
connect, password auth, PTY, shell data, `TermEngine`, and `GridSnapshot`.

## Release Automation

GitHub Actions has two packaging workflows:

- `.github/workflows/release.yml`: `v*` tags create formal GitHub Releases with
  desktop artifacts, signed Android APKs, and unsigned iOS IPAs after the
  blocking RustSec audit passes.
- `.github/workflows/alpha.yml`: pushes to `main` update the rolling `alpha`
  prerelease. RustSec findings are warnings only. New assets are uploaded to an
  invisible draft first, then the fixed `alpha` tag and public Release are
  switched together; failures restore the previous tag and Release.

Both workflows share artifact naming for Linux/macOS/Windows x
`x64`/`aarch64`, plus Android/iOS `aarch64`. Android packaging verifies the
identifier, version, ABI, icon, and signing certificate. iOS packaging verifies
the unique `Payload/*.app`, Info.plist metadata, arm64 architecture, and absence
of provisioning profiles or code-signature residue. Missing or mismatched
Android signing secrets fail closed. GitHub cannot mark a prerelease as Latest,
so Alpha uses the rolling tag/newest prerelease semantics rather than promising
a permanent first position after later releases.

## Roadmap Snapshot

- [x] Core SSH + terminal engine
- [x] Dioxus desktop GUI
- [x] Session persistence, encrypted vault, SFTP, monitor
- [x] Host-key trust flow, split views, ssh-agent, ProxyJump, trigger highlight
- [x] UI modularization and selector-driven shell panels
- [x] Release/alpha packaging and maintenance governance
- [x] Stable-signed Android APK and re-signable unsigned iOS IPA packaging
- [ ] Multi-hop ProxyJump, editable trigger rules, richer terminal highlighting

## License

Apache-2.0
