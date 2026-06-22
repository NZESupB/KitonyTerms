//! SSH client built on `russh`: connect, authenticate, open a PTY shell.
//!
//! This module is deliberately small and synchronous-looking (async fns) so the
//! session loop can drive it. Higher-level orchestration (per-session tasks,
//! the UI message protocol) lives in [`crate::session`].

use std::sync::Arc;

use russh::client::{self, Handle};
use russh::keys::{load_secret_key, HashAlg, PrivateKey, PrivateKeyWithHashAlg};
use russh::{ChannelMsg, Disconnect};

use kt_config::{AuthMethod, ConnectParams};

mod handler;
pub use handler::{AcceptAllVerifier, ClientHandler, HostKeyDecision, HostKeyVerifier};

/// SSH-layer errors.
#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("connection failed: {0}")]
    Connect(String),

    #[error("host key rejected by user")]
    HostKeyRejected,

    #[error("authentication failed (tried: {0})")]
    AuthFailed(String),

    #[error("no authentication method available")]
    NoAuthMethod,

    #[error("failed to load private key {path}: {source}")]
    KeyLoad {
        path: String,
        source: russh::keys::Error,
    },

    #[error("user cancelled authentication")]
    AuthCancelled,

    #[error("channel error: {0}")]
    Channel(String),

    #[error("sftp error: {0}")]
    Sftp(String),

    #[error(transparent)]
    Russh(#[from] russh::Error),
}

type Result<T> = std::result::Result<T, SshError>;

/// Supplies secrets/answers on demand during authentication.
///
/// Implementations may read the vault, prompt the user on a terminal, or pop a
/// GUI dialog. Returning `None` means "cancel / give up".
pub trait AuthProvider: Send {
    /// A password for `user@host`. Called for password auth.
    fn password(&mut self, user: &str, host: &str) -> Option<String>;

    /// A passphrase to decrypt the private key at `key_path`.
    fn key_passphrase(&mut self, key_path: &str) -> Option<String>;

    /// Answers for a keyboard-interactive round. `prompts` are
    /// `(prompt_text, echo)` pairs; return one answer per prompt, in order.
    fn keyboard_interactive(
        &mut self,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<String>>;
}

/// Initial PTY geometry.
#[derive(Debug, Clone, Copy)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
}

impl Default for PtySize {
    fn default() -> Self {
        Self { cols: 80, rows: 24 }
    }
}

/// A connected, authenticated SSH session with an open PTY shell channel.
pub struct SshShell {
    handle: Handle<ClientHandler>,
    channel: russh::Channel<client::Msg>,
}

impl SshShell {
    /// Connect, authenticate, and open an interactive PTY shell in one call.
    pub async fn open(
        params: &ConnectParams,
        pty: PtySize,
        verifier: Arc<dyn HostKeyVerifier>,
        auth: &mut dyn AuthProvider,
    ) -> Result<Self> {
        let config = Arc::new(client::Config {
            keepalive_interval: Some(std::time::Duration::from_secs(30)),
            ..Default::default()
        });

        let handler = ClientHandler {
            host: params.host.clone(),
            port: params.port,
            verifier,
        };

        let addr = (params.host.as_str(), params.port);
        // 给 TCP 连接 + 握手设上限,避免不可达主机长时间卡在 "Connecting"。
        // Bound connect + handshake so an unreachable host fails fast instead of
        // hanging in the "Connecting" state.
        let connect_fut = client::connect(config, addr, handler);
        let mut handle = match tokio::time::timeout(
            std::time::Duration::from_secs(15),
            connect_fut,
        )
        .await
        {
            Ok(res) => res.map_err(|e| match e {
                russh::Error::IO(io) => SshError::Connect(io.to_string()),
                other => SshError::Connect(other.to_string()),
            })?,
            Err(_) => {
                return Err(SshError::Connect(
                    "connection timed out after 15s / 连接超时(15 秒)".to_string(),
                ))
            }
        };

        authenticate(&mut handle, params, auth).await?;

        // Open session channel, request a PTY, then a shell.
        let channel = handle
            .channel_open_session()
            .await
            .map_err(|e| SshError::Channel(e.to_string()))?;

        let modes = &[];
        channel
            .request_pty(
                true,
                "xterm-256color",
                pty.cols as u32,
                pty.rows as u32,
                0,
                0,
                modes,
            )
            .await
            .map_err(|e| SshError::Channel(format!("request_pty: {e}")))?;

        channel
            .request_shell(true)
            .await
            .map_err(|e| SshError::Channel(format!("request_shell: {e}")))?;

        Ok(Self { handle, channel })
    }

    /// Write bytes (keyboard input) to the remote shell.
    pub async fn write(&self, data: &[u8]) -> Result<()> {
        self.channel
            .data(data)
            .await
            .map_err(|e| SshError::Channel(e.to_string()))
    }

    /// Notify the remote of a new terminal size.
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.channel
            .window_change(cols as u32, rows as u32, 0, 0)
            .await
            .map_err(|e| SshError::Channel(e.to_string()))
    }

    /// Await the next channel message (output data, EOF, exit status, close).
    ///
    /// Returns `None` when the channel is fully closed.
    pub async fn next_message(&mut self) -> Option<ChannelMsg> {
        self.channel.wait().await
    }

    /// Cleanly disconnect the session.
    pub async fn disconnect(&self) -> Result<()> {
        self.handle
            .disconnect(Disconnect::ByApplication, "bye", "en")
            .await
            .map_err(SshError::from)
    }

    /// 打开一条用于资源监控的持久通道(非交互 `sh`,从通道 stdin 读命令)。
    /// 返回的 [`russh::Channel`] 可 move 进监控任务,周期写入命令并读取输出。
    ///
    /// Open a persistent channel for resource monitoring: a non-interactive `sh`
    /// reading commands from the channel stdin. The returned channel can be moved
    /// into a monitor task that periodically writes commands and reads output.
    pub async fn open_monitor_channel(&self) -> Result<russh::Channel<client::Msg>> {
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| SshError::Channel(e.to_string()))?;
        channel
            .exec(true, "sh")
            .await
            .map_err(|e| SshError::Channel(format!("exec sh: {e}")))?;
        Ok(channel)
    }

    /// 在当前已认证会话上打开 SFTP 子系统,返回独立拥有通道流的 [`SftpSession`]。
    ///
    /// Open the SFTP subsystem on the already-authenticated session and return an
    /// owned [`SftpSession`]. The returned session drives its own channel stream
    /// independently, so it can be moved into a dedicated task without blocking
    /// the interactive shell loop. The TCP session stays alive as long as this
    /// [`SshShell`] (which owns `handle`) lives.
    pub async fn open_sftp(&self) -> Result<russh_sftp::client::SftpSession> {
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| SshError::Channel(e.to_string()))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| SshError::Channel(format!("request_subsystem sftp: {e}")))?;
        russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| SshError::Sftp(e.to_string()))
    }
}

/// Run the authentication sequence, trying each configured method in order.
async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    params: &ConnectParams,
    auth: &mut dyn AuthProvider,
) -> Result<()> {
    if params.auth.is_empty() {
        return Err(SshError::NoAuthMethod);
    }

    let mut tried = Vec::new();
    for method in &params.auth {
        match method {
            AuthMethod::Password => {
                tried.push("password");
                let Some(pw) = auth.password(&params.user, &params.host) else {
                    return Err(SshError::AuthCancelled);
                };
                let res = handle.authenticate_password(&params.user, pw).await?;
                if res.success() {
                    return Ok(());
                }
            }
            AuthMethod::PublicKey { key_path } => {
                tried.push("publickey");
                let key = load_private_key(key_path, auth)?;
                let key_with_hash =
                    PrivateKeyWithHashAlg::new(Arc::new(key), Some(HashAlg::Sha256));
                let res = handle
                    .authenticate_publickey(&params.user, key_with_hash)
                    .await?;
                if res.success() {
                    return Ok(());
                }
            }
            AuthMethod::KeyboardInteractive => {
                tried.push("keyboard-interactive");
                if keyboard_interactive_auth(handle, &params.user, auth).await? {
                    return Ok(());
                }
            }
            AuthMethod::Agent => {
                // ssh-agent auth is a later phase; skip gracefully.
                tried.push("agent(skipped)");
            }
        }
    }

    Err(SshError::AuthFailed(tried.join(", ")))
}

/// Load a private key, prompting for a passphrase if it is encrypted.
fn load_private_key(key_path: &std::path::Path, auth: &mut dyn AuthProvider) -> Result<PrivateKey> {
    let path_str = key_path.display().to_string();
    // First try without a passphrase.
    match load_secret_key(key_path, None) {
        Ok(key) => Ok(key),
        Err(russh::keys::Error::KeyIsEncrypted) => {
            let Some(pass) = auth.key_passphrase(&path_str) else {
                return Err(SshError::AuthCancelled);
            };
            load_secret_key(key_path, Some(&pass)).map_err(|source| SshError::KeyLoad {
                path: path_str,
                source,
            })
        }
        Err(source) => Err(SshError::KeyLoad {
            path: path_str,
            source,
        }),
    }
}

/// Drive a keyboard-interactive exchange to completion.
async fn keyboard_interactive_auth(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    auth: &mut dyn AuthProvider,
) -> Result<bool> {
    use client::KeyboardInteractiveAuthResponse as Resp;

    let mut response = handle
        .authenticate_keyboard_interactive_start(user, None)
        .await?;

    loop {
        match response {
            Resp::Success => return Ok(true),
            Resp::Failure { .. } => return Ok(false),
            Resp::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                let prompt_pairs: Vec<(String, bool)> =
                    prompts.iter().map(|p| (p.prompt.clone(), p.echo)).collect();
                let Some(answers) = auth.keyboard_interactive(&name, &instructions, &prompt_pairs)
                else {
                    return Err(SshError::AuthCancelled);
                };
                response = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await?;
            }
        }
    }
}
