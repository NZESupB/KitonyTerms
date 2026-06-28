//! KitonyTerms core — SSH connection management and terminal engine.
//!
//! This crate has no UI dependencies so it can be exercised headlessly
//! (see `examples/headless.rs`) and unit-tested in isolation.
//!
//! Modules:
//! * [`term`] — terminal engine (alacritty_terminal wrapper) + grid snapshots
//! * [`ssh`] — SSH client (russh): connect, auth, PTY shell, read/write loop
//! * [`sftp`] — SFTP subtask driving russh-sftp over the same session
//! * [`monitor`] — server resource monitor subtask (CPU/mem/net/disk/procs)
//! * [`session`] — session lifecycle + the UI⇄core message protocol

pub mod monitor;
pub mod session;
pub mod sftp;
pub mod ssh;
pub mod term;

pub use monitor::{DiskUsage, MonitorStats, ProcInfo};
pub use session::{
    AuthChallenge, AuthPrompt, AuthProviderFactory, AuthResponse, FromCore, SessionId,
    SessionManager, SftpEntry, SftpOp, SftpRequest, ToCore,
};
pub use ssh::{AuthProvider, HostKeyVerifier, PtySize, SshError, SshShell};
pub use term::{GridSnapshot, TermEngine, TermEvent};
