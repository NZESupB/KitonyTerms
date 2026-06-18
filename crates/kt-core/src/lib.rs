//! KitonyTerms core — SSH connection management and terminal engine.
//!
//! This crate has no UI dependencies so it can be exercised headlessly
//! (see `examples/headless.rs`) and unit-tested in isolation.
//!
//! Modules:
//! * [`term`] — terminal engine (alacritty_terminal wrapper) + grid snapshots
//! * [`ssh`] — SSH client (russh): connect, auth, PTY shell, read/write loop
//! * [`session`] — session lifecycle + the UI⇄core message protocol

pub mod session;
pub mod ssh;
pub mod term;

pub use session::{
    AuthProviderFactory, FromCore, SessionId, SessionManager, ToCore,
};
pub use ssh::{AuthProvider, HostKeyVerifier, PtySize, SshError, SshShell};
pub use term::{GridSnapshot, TermEngine, TermEvent};
