# KitonyTerms

English | [中文](README.zh-CN.md)

A Rust + Dioxus SSH client with saved sessions and groups, password/public-key/keyboard-interactive/agent authentication, single-hop ProxyJump, TCP proxies, terminal, SFTP, monitoring, and configuration sync.

## Features and limits

- SFTP supports batch uploads, downloads, directory operations, an inline text editor, and desktop external editors.
- Linux/WSL operations include services, processes, network, Docker/Compose queries and management, capability detection, cancellation, and independent container terminals. Management uses typed commands; some actions require sudo authentication.
- Compact desktop layout and a separate touch UI for phones; system/light/dark themes, English/Chinese, and terminal font settings.
- WebDAV and LAN sync only non-secret configuration, excluding vault, vault key, and known_hosts. WebDAV takes a full resource URL; LAN v2 uses a 26-character pairing secret or QR code.
- Windows AMD64, macOS ARM64, and Linux x64/aarch64 release builds check and download updates in the background, then install after a confirmed restart.
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
- Desktop `updates/` under the application data directory stores local update preferences, cached packages, and installation transactions; WebDAV/LAN sync excludes it.

## Desktop in-app updates

Open **Settings → Software update** to see the current build/channel, target version, last check, progress, and errors. Release builds enable automatic checks and downloads by default. Checks start about five seconds after launch and repeat every six hours, with backoff after failures. You can turn either setting off, check immediately, or download manually.

After a download finishes, select **Restart and install** to start closing connections and preparing installation immediately. You can keep using the app until you select it. Save and close inline editors, finish external editing, and wait for SFTP work and configuration sync first. If helper preparation fails, the application stays open. Failed checks, downloads, and validation can be retried from the update page.

Windows portable builds replace the current EXE; installed builds use the system installer. Protected target folders request UAC approval once; cancelling preserves the current version, and the app reopens with its original privileges. macOS mounts the DMG read-only, verifies the signed bundle, detaches the image, and replaces the entire `.app`; Linux replaces the executable actually launched. When running from a macOS DMG or App Translocation, or if the installation cannot be updated, use the release download link to install manually. Configuration, vault, and known_hosts are preserved. Failed or interrupted transactions attempt to restore the previous version; backups are removed after the new UI confirms startup. If an older Windows updater fails while flushing files, manually install v0.1.9 or later first.

Stable builds follow higher stable versions; Alpha uses workflow build/attempt numbers. Channels do not switch automatically. Development builds disable updates, and Android/iOS have no updater entry. Existing older versions require one manual installation of a version containing this feature; later releases must include a valid update manifest.

## Build and release

[Alpha](.github/workflows/alpha.yml) updates the rolling prerelease on main pushes. [Release](.github/workflows/release.yml) publishes v* tags whose version matches [Cargo.toml](Cargo.toml). RustSec findings block formal releases and produce warnings for Alpha.

Artifacts include macOS ARM64 DMG, Windows AMD64 portable ZIP and Inno Setup installer, Linux x64/aarch64 TAR.GZ, and Android/iOS aarch64 packages. Android APKs use a fixed signing certificate. iOS IPAs are unsigned and require user re-signing before installation. JavaScript Actions run on Node.js 24.

The workflows define mobile environments and commands: Dioxus CLI, `llvm-tools-preview`, Android SDK/NDK or Xcode. Packaging entry points:

- [Android](.github/scripts/package-android-apk.sh): the `mobile-signing` Environment supplies `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, and `ANDROID_CERT_SHA256`; set `ANDROID_KEY_PASSWORD` when different. Missing or mismatched credentials fail the build.
- [iOS](.github/scripts/package-ios-ipa.sh): produces unsigned IPAs without Android signing secrets.
- [Build numbers](.github/scripts/allocate-mobile-build-number.sh): UTC-second allocation under shared concurrency; absolute uniqueness and monotonicity are not guaranteed across clock rollback.
- [objcopy preflight](.github/scripts/prepare-rust-objcopy.sh): prepares the LLVM loader path before dx.

Both platforms use [com.kitonyterms.app](Dioxus.toml). Android updates retain the signing certificate; iOS replacement depends on re-signing identity and configuration.

Desktop builds embed the channel, commit, build/attempt numbers, and target. The [manifest generator](.github/scripts/generate-update-manifest.py) checks all four desktop targets and records package sizes and SHA-256 hashes, using DMG for macOS and ZIP/installer for Windows. Missing packages or inconsistent identities block manifest publishing. Alpha validates the staged manifest before switching the public release; manual builds only produce artifacts.

## Validation and development

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo audit
python3 -B .github/scripts/test-update-manifest.py
bash -n .github/scripts/publish-alpha.sh
```

[SSH loopback tests](crates/kt-core/tests/roundtrip.rs) cover exec, PTY, cancellation, and generation isolation. [Mobile packaging contracts](crates/kt-app/tests/mobile_packaging_contract.rs) cover scripts and artifact rules. Test definitions are not evidence of a passing run in the current environment.

[Update tests](crates/kt-update/src/tests.rs) cover local HTTP, cancellation, rate limits, and download validation. The same crate tests installation transactions and archive safety, including native DMG mounting, cleanup, and signed temporary macOS bundle replacement/rollback. Real installation and GUI restart acceptance still requires the corresponding Windows, macOS, and Linux systems.

See [architecture](.agentdocs/architecture.md), [targeted checks](.agentdocs/maintenance.md), and [pending phone input validation](.agentdocs/workflow/260820-mobile-phone-ui.md).

License: Apache-2.0 ([workspace declaration](Cargo.toml)).
