# KitonyTerms

English | [中文](README.zh-CN.md)

A Rust + Dioxus SSH client with saved sessions and groups, password/public-key/keyboard-interactive/agent authentication, single-hop ProxyJump, TCP proxies, terminal, SFTP, monitoring, and configuration sync.

## Features and limits

- SFTP supports batch uploads, downloads, directory operations, an inline text editor, and desktop external editors.
- Linux/WSL operations include services, processes, network, Docker/Compose queries and management, capability detection, cancellation, and independent container terminals. Management uses typed commands; some actions require sudo authentication.
- Compact desktop layout and a separate touch UI for phones; system/light/dark themes, English/Chinese, and terminal font settings.
- WebDAV and LAN sync only non-secret configuration, excluding vault, vault key, and known_hosts. WebDAV takes a full resource URL; LAN v2 uses a 26-character pairing secret or QR code.
- The main binary is a GUI with no-argument, `--gui`, and `--help` entry points. Multi-hop ProxyJump, UI editing of trigger rules, and full syntax highlighting are not provided.

Sources: [app](crates/kt-app/src/main.rs), [UI](crates/kt-ui/src/components/app.rs), [operations](crates/kt-core/src/remote_ops.rs), [sync](crates/kt-sync/src/lib.rs).

## Run locally

Use the [Rust stable toolchain](rust-toolchain.toml) and dependencies recorded in [Cargo.lock](Cargo.lock).

Ubuntu/Debian dependencies match [CI](.github/workflows/alpha.yml):

```bash
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev pkg-config
cargo run -p kt-app
```

On macOS/Windows, install Rust and the platform build tools, then run `cargo run -p kt-app`. For a desktop phone preview:

```bash
cargo run -p kt-app --features phone-preview
```

Resize the window below a 600 CSS-pixel short side. Create and save connections from the sidebar; passwords and key passphrases go into the encrypted vault.

## Storage

[Config paths](crates/kt-config/src/lib.rs) and [Store](crates/kt-ui/src/store.rs) manage:

- `config.toml`: sessions and non-secret settings; `known_hosts.toml`: trusted host keys.
- `secrets.vault`: encrypted passwords and passphrases; `secrets.vault.key`: the per-install local key. Keep both private.
- Android uses app-private `files/config` and `files/data`.
- Legacy fixed-key vaults migrate automatically; legacy master-password vaults that cannot be opened are backed up as `secrets.vault.legacy*` before replacement.

## Build and release

[Alpha](.github/workflows/alpha.yml) updates the rolling prerelease on main pushes. [Release](.github/workflows/release.yml) publishes v* tags whose version matches [Cargo.toml](Cargo.toml). RustSec findings block formal releases and produce warnings for Alpha.

Artifacts cover macOS/Windows/Linux x64 and aarch64, plus Android/iOS aarch64. Android APKs use a fixed signing certificate. iOS IPAs are unsigned and require user re-signing before installation.

The workflows define mobile environments and commands: Dioxus CLI, `llvm-tools-preview`, Android SDK/NDK or Xcode. Packaging entry points:

- [Android](.github/scripts/package-android-apk.sh): the `mobile-signing` Environment supplies `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, and `ANDROID_CERT_SHA256`; set `ANDROID_KEY_PASSWORD` when different. Missing or mismatched credentials fail the build.
- [iOS](.github/scripts/package-ios-ipa.sh): produces unsigned IPAs without Android signing secrets.
- [Build numbers](.github/scripts/allocate-mobile-build-number.sh): UTC-second allocation under shared concurrency; absolute uniqueness and monotonicity are not guaranteed across clock rollback.
- [objcopy preflight](.github/scripts/prepare-rust-objcopy.sh): prepares the LLVM loader path before dx.

Both platforms use [com.kitonyterms.app](Dioxus.toml). Android updates retain the signing certificate; iOS replacement depends on re-signing identity and configuration.

## Validation and development

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

[SSH loopback tests](crates/kt-core/tests/roundtrip.rs) cover exec, PTY, cancellation, and generation isolation. [Mobile packaging contracts](crates/kt-app/tests/mobile_packaging_contract.rs) cover scripts and artifact rules. Test definitions are not evidence of a passing run in the current environment.

See [architecture](.agentdocs/architecture.md), [targeted checks](.agentdocs/maintenance.md), and [pending phone input validation](.agentdocs/workflow/260820-mobile-phone-ui.md).

License: Apache-2.0 ([workspace declaration](Cargo.toml)).
