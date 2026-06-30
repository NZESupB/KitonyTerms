//! SSH client built on `russh`: connect, authenticate, open a PTY shell.
//!
//! This module is deliberately small and synchronous-looking (async fns) so the
//! session loop can drive it. Higher-level orchestration (per-session tasks,
//! the UI message protocol) lives in [`crate::session`].

use std::sync::Arc;

use russh::client::{self, Handle};
use russh::keys::{load_secret_key, HashAlg, PrivateKey, PrivateKeyWithHashAlg};
use russh::{ChannelMsg, Disconnect};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use kt_config::{AuthMethod, ConnectParams, SshProxy};

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

    #[error("ssh-agent authentication failed: {0}")]
    Agent(String),

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

    #[error("ProxyJump error: {0}")]
    ProxyJump(String),

    #[error("proxy error: {0}")]
    Proxy(String),

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
    /// A password for `user@host:port`. Called for password auth.
    fn password(&mut self, user: &str, host: &str, port: u16) -> Option<String>;

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
    _proxy_handle: Option<Handle<ClientHandler>>,
    channel: russh::Channel<client::Msg>,
}

/// Keeps every SSH handle required by an auxiliary connection alive.
pub struct SshConnectionGuard {
    _handle: Handle<ClientHandler>,
    _proxy_handle: Option<Handle<ClientHandler>>,
}

struct AuthenticatedHandle {
    handle: Handle<ClientHandler>,
    proxy_handle: Option<Handle<ClientHandler>>,
}

impl SshShell {
    /// Connect, authenticate, and open an interactive PTY shell in one call.
    pub async fn open(
        params: &ConnectParams,
        pty: PtySize,
        verifier: Arc<dyn HostKeyVerifier>,
        auth: &mut dyn AuthProvider,
    ) -> Result<Self> {
        let authenticated = connect_authenticated(params, verifier, auth).await?;
        Self::open_shell_on_handle(authenticated, pty, params.forward_agent).await
    }

    async fn open_shell_on_handle(
        authenticated: AuthenticatedHandle,
        pty: PtySize,
        forward_agent: bool,
    ) -> Result<Self> {
        // Open session channel, request a PTY, then a shell.
        let channel = authenticated
            .handle
            .channel_open_session()
            .await
            .map_err(|e| SshError::Channel(e.to_string()))?;

        if forward_agent {
            channel
                .agent_forward(true)
                .await
                .map_err(|e| SshError::Channel(format!("agent_forward: {e}")))?;
        }

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

        Ok(Self {
            handle: authenticated.handle,
            _proxy_handle: authenticated.proxy_handle,
            channel,
        })
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

    /// 新建一条独立 SSH 连接并打开 SFTP 子系统。
    /// 返回的 `Handle` 必须与 `SftpSession` 一起持有,否则连接会被关闭。
    pub async fn open_standalone_sftp(
        params: &ConnectParams,
        verifier: Arc<dyn HostKeyVerifier>,
        auth: &mut dyn AuthProvider,
    ) -> Result<(russh_sftp::client::SftpSession, SshConnectionGuard)> {
        let authenticated = connect_authenticated(params, verifier, auth).await?;
        let channel = authenticated
            .handle
            .channel_open_session()
            .await
            .map_err(|e| SshError::Channel(e.to_string()))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| SshError::Channel(format!("request_subsystem sftp: {e}")))?;
        let session = russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| SshError::Sftp(e.to_string()))?;
        Ok((
            session,
            SshConnectionGuard {
                _handle: authenticated.handle,
                _proxy_handle: authenticated.proxy_handle,
            },
        ))
    }
}

async fn connect_authenticated(
    params: &ConnectParams,
    verifier: Arc<dyn HostKeyVerifier>,
    auth: &mut dyn AuthProvider,
) -> Result<AuthenticatedHandle> {
    let config = Arc::new(client::Config {
        keepalive_interval: Some(std::time::Duration::from_secs(30)),
        ..Default::default()
    });

    let authenticated = if let Some(proxy_jump) = params
        .proxy_jump
        .as_deref()
        .map(str::trim)
        .filter(|proxy_jump| !proxy_jump.is_empty())
    {
        connect_via_proxy_jump(params, proxy_jump, config, verifier, auth).await?
    } else {
        let handle = connect_direct(params, config, verifier).await?;
        AuthenticatedHandle {
            handle,
            proxy_handle: None,
        }
    };

    let mut handle = authenticated.handle;
    authenticate(&mut handle, params, auth).await?;
    Ok(AuthenticatedHandle {
        handle,
        proxy_handle: authenticated.proxy_handle,
    })
}

async fn connect_direct(
    params: &ConnectParams,
    config: Arc<client::Config>,
    verifier: Arc<dyn HostKeyVerifier>,
) -> Result<Handle<ClientHandler>> {
    let handler = ClientHandler {
        host: params.host.clone(),
        port: params.port,
        verifier,
    };

    match effective_network_proxy(params)? {
        SshProxy::None => {
            let addr = (params.host.as_str(), params.port);
            // 给 TCP 连接 + 握手设上限,避免不可达主机长时间卡在 "Connecting"。
            // Bound connect + handshake so an unreachable host fails fast instead of
            // hanging in the "Connecting" state.
            let connect_fut = client::connect(config, addr, handler);
            timeout_connect(connect_fut).await
        }
        proxy => {
            let stream = timeout_proxy_connect(connect_proxy_stream(params, &proxy)).await?;
            let connect_fut = client::connect_stream(config, stream, handler);
            timeout_connect(connect_fut).await
        }
    }
}

async fn timeout_proxy_connect<F>(connect_fut: F) -> Result<TcpStream>
where
    F: std::future::Future<Output = Result<TcpStream>>,
{
    match tokio::time::timeout(std::time::Duration::from_secs(15), connect_fut).await {
        Ok(result) => result,
        Err(_) => Err(SshError::Proxy(
            "proxy connection timed out after 15s / 代理连接超时(15 秒)".to_string(),
        )),
    }
}

fn effective_network_proxy(params: &ConnectParams) -> Result<SshProxy> {
    match &params.proxy {
        SshProxy::System => system_proxy_for_host(&params.host)
            .map_err(SshError::Proxy)?
            .map(Ok)
            .unwrap_or(Ok(SshProxy::None)),
        proxy => Ok(proxy.clone()),
    }
}

async fn connect_proxy_stream(params: &ConnectParams, proxy: &SshProxy) -> Result<TcpStream> {
    match proxy {
        SshProxy::Socks {
            host,
            port,
            version,
        } if *version == 5 => connect_socks5_proxy(host, *port, &params.host, params.port).await,
        SshProxy::Socks { version, .. } => Err(SshError::Proxy(format!(
            "unsupported SOCKS version: {version}"
        ))),
        SshProxy::Http { host, port } => {
            connect_http_proxy(host, *port, &params.host, params.port).await
        }
        SshProxy::None | SshProxy::System => Err(SshError::Proxy(
            "internal error: proxy stream requested without a concrete proxy".to_string(),
        )),
    }
}

async fn connect_http_proxy(
    proxy_host: &str,
    proxy_port: u16,
    target_host: &str,
    target_port: u16,
) -> Result<TcpStream> {
    let mut stream = TcpStream::connect((proxy_host, proxy_port))
        .await
        .map_err(|err| {
            SshError::Proxy(format!(
                "connect HTTP proxy {proxy_host}:{proxy_port}: {err}"
            ))
        })?;
    let target = format_host_port(target_host, target_port);
    let request = format!(
        "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Connection: Keep-Alive\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|err| SshError::Proxy(format!("write HTTP CONNECT request: {err}")))?;

    let response = read_http_connect_response(&mut stream).await?;
    if http_connect_status(&response) == Some(200) {
        Ok(stream)
    } else {
        Err(SshError::Proxy(format!(
            "HTTP proxy rejected CONNECT: {}",
            response.lines().next().unwrap_or("invalid response")
        )))
    }
}

async fn read_http_connect_response(stream: &mut TcpStream) -> Result<String> {
    let mut response = Vec::new();
    let mut buf = [0u8; 256];
    while response.len() < 8192 {
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|err| SshError::Proxy(format!("read HTTP CONNECT response: {err}")))?;
        if n == 0 {
            break;
        }
        response.extend_from_slice(&buf[..n]);
        if response.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(response)
        .map_err(|err| SshError::Proxy(format!("HTTP proxy response is not UTF-8: {err}")))
}

fn http_connect_status(response: &str) -> Option<u16> {
    response
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

async fn connect_socks5_proxy(
    proxy_host: &str,
    proxy_port: u16,
    target_host: &str,
    target_port: u16,
) -> Result<TcpStream> {
    let mut stream = TcpStream::connect((proxy_host, proxy_port))
        .await
        .map_err(|err| {
            SshError::Proxy(format!(
                "connect SOCKS proxy {proxy_host}:{proxy_port}: {err}"
            ))
        })?;
    stream
        .write_all(&[0x05, 0x01, 0x00])
        .await
        .map_err(|err| SshError::Proxy(format!("write SOCKS greeting: {err}")))?;
    let mut greeting = [0u8; 2];
    stream
        .read_exact(&mut greeting)
        .await
        .map_err(|err| SshError::Proxy(format!("read SOCKS greeting: {err}")))?;
    if greeting != [0x05, 0x00] {
        return Err(SshError::Proxy(
            "SOCKS proxy requires unsupported authentication".to_string(),
        ));
    }

    let request = socks5_connect_request(target_host, target_port)?;
    stream
        .write_all(&request)
        .await
        .map_err(|err| SshError::Proxy(format!("write SOCKS connect request: {err}")))?;
    read_socks5_connect_response(&mut stream).await?;
    Ok(stream)
}

fn socks5_connect_request(host: &str, port: u16) -> Result<Vec<u8>> {
    let host_bytes = host.as_bytes();
    if host_bytes.len() > u8::MAX as usize {
        return Err(SshError::Proxy("SOCKS target host is too long".to_string()));
    }
    let mut request = Vec::with_capacity(7 + host_bytes.len());
    request.extend_from_slice(&[0x05, 0x01, 0x00, 0x03, host_bytes.len() as u8]);
    request.extend_from_slice(host_bytes);
    request.extend_from_slice(&port.to_be_bytes());
    Ok(request)
}

async fn read_socks5_connect_response(stream: &mut TcpStream) -> Result<()> {
    let mut head = [0u8; 4];
    stream
        .read_exact(&mut head)
        .await
        .map_err(|err| SshError::Proxy(format!("read SOCKS response: {err}")))?;
    if head[0] != 0x05 || head[1] != 0x00 {
        return Err(SshError::Proxy(format!(
            "SOCKS connect failed with status 0x{:02x}",
            head[1]
        )));
    }
    let addr_len = match head[3] {
        0x01 => 4,
        0x03 => {
            let mut len = [0u8; 1];
            stream
                .read_exact(&mut len)
                .await
                .map_err(|err| SshError::Proxy(format!("read SOCKS domain length: {err}")))?;
            len[0] as usize
        }
        0x04 => 16,
        other => {
            return Err(SshError::Proxy(format!(
                "SOCKS response used unknown address type 0x{other:02x}"
            )))
        }
    };
    let mut discard = vec![0u8; addr_len + 2];
    stream
        .read_exact(&mut discard)
        .await
        .map_err(|err| SshError::Proxy(format!("read SOCKS bound address: {err}")))?;
    Ok(())
}

fn system_proxy_for_host(host: &str) -> std::result::Result<Option<SshProxy>, String> {
    let no_proxy = std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .ok();
    if no_proxy
        .as_deref()
        .map(|patterns| no_proxy_matches(patterns, host))
        .unwrap_or(false)
    {
        return Ok(None);
    }

    for key in [
        "ALL_PROXY",
        "all_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ] {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                return parse_proxy_url(&value);
            }
        }
    }
    Ok(None)
}

fn parse_proxy_url(value: &str) -> std::result::Result<Option<SshProxy>, String> {
    let value = value.trim();
    let (scheme, rest) = value
        .split_once("://")
        .map(|(scheme, rest)| (scheme.to_ascii_lowercase(), rest))
        .unwrap_or_else(|| ("http".to_string(), value));
    let rest = rest
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(rest)
        .trim_end_matches('/');
    let (host, port) = parse_proxy_host_port(rest, default_proxy_port(&scheme))?;
    match scheme.as_str() {
        "socks" | "socks5" | "socks5h" => Ok(Some(SshProxy::Socks {
            host,
            port,
            version: 5,
        })),
        "http" | "https" => Ok(Some(SshProxy::Http { host, port })),
        _ => Err(format!("unsupported proxy scheme: {scheme}")),
    }
}

fn default_proxy_port(scheme: &str) -> u16 {
    match scheme {
        "socks" | "socks5" | "socks5h" => 1080,
        _ => 8080,
    }
}

fn parse_proxy_host_port(
    value: &str,
    default_port: u16,
) -> std::result::Result<(String, u16), String> {
    if let Some(rest) = value.strip_prefix('[') {
        let (host, after) = rest
            .split_once(']')
            .ok_or_else(|| "invalid bracketed proxy host".to_string())?;
        let port = after
            .strip_prefix(':')
            .and_then(|port| port.parse::<u16>().ok())
            .unwrap_or(default_port);
        return Ok((host.to_string(), port));
    }
    match value.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => Ok((
            host.to_string(),
            port.parse::<u16>()
                .map_err(|_| "invalid proxy port".to_string())?,
        )),
        _ => Ok((value.to_string(), default_port)),
    }
}

fn no_proxy_matches(patterns: &str, host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    patterns.split(',').any(|pattern| {
        let pattern = pattern.trim().to_ascii_lowercase();
        pattern == "*"
            || pattern == host
            || pattern
                .strip_prefix('.')
                .map(|suffix| host == suffix || host.ends_with(&format!(".{suffix}")))
                .unwrap_or(false)
    })
}

fn format_host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

async fn connect_via_proxy_jump(
    params: &ConnectParams,
    proxy_jump: &str,
    config: Arc<client::Config>,
    verifier: Arc<dyn HostKeyVerifier>,
    auth: &mut dyn AuthProvider,
) -> Result<AuthenticatedHandle> {
    let jump = ProxyJumpTarget::parse(proxy_jump)
        .ok_or_else(|| SshError::ProxyJump(format!("invalid ProxyJump target: {proxy_jump}")))?;
    let mut jump_params = params.clone();
    jump_params.host = jump.host;
    jump_params.port = jump.port;
    jump_params.user = jump.user.unwrap_or_else(|| params.user.clone());
    jump_params.proxy_jump = None;
    jump_params.forward_agent = false;
    jump_params.vault_id = Some(jump_params.effective_vault_id());

    let mut proxy_handle = connect_direct(&jump_params, config.clone(), verifier.clone()).await?;
    authenticate(&mut proxy_handle, &jump_params, auth).await?;

    let channel = proxy_handle
        .channel_open_direct_tcpip(params.host.clone(), params.port.into(), "127.0.0.1", 0)
        .await
        .map_err(|e| SshError::ProxyJump(format!("open direct-tcpip: {e}")))?;

    let handler = ClientHandler {
        host: params.host.clone(),
        port: params.port,
        verifier,
    };
    let connect_fut = client::connect_stream(config, channel.into_stream(), handler);
    let handle = timeout_connect(connect_fut).await?;
    Ok(AuthenticatedHandle {
        handle,
        proxy_handle: Some(proxy_handle),
    })
}

async fn timeout_connect<F>(connect_fut: F) -> Result<Handle<ClientHandler>>
where
    F: std::future::Future<Output = std::result::Result<Handle<ClientHandler>, russh::Error>>,
{
    match tokio::time::timeout(std::time::Duration::from_secs(15), connect_fut).await {
        Ok(res) => res.map_err(|e| match e {
            russh::Error::IO(io) => SshError::Connect(io.to_string()),
            other => SshError::Connect(other.to_string()),
        }),
        Err(_) => Err(SshError::Connect(
            "connection timed out after 15s / 连接超时(15 秒)".to_string(),
        )),
    }
}

struct ProxyJumpTarget {
    user: Option<String>,
    host: String,
    port: u16,
}

impl ProxyJumpTarget {
    fn parse(spec: &str) -> Option<Self> {
        let spec = spec.split(',').next()?.trim();
        if spec.is_empty() {
            return None;
        }
        let (user, host_port) = spec
            .rsplit_once('@')
            .map(|(user, host)| (Some(user.to_string()), host))
            .unwrap_or((None, spec));
        let (host, port) = parse_host_port(host_port)?;
        Some(Self { user, host, port })
    }
}

fn parse_host_port(host_port: &str) -> Option<(String, u16)> {
    if let Some(rest) = host_port.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = after
            .strip_prefix(':')
            .and_then(|port| port.parse::<u16>().ok())
            .unwrap_or(22);
        return Some((host.to_string(), port));
    }
    match host_port.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            Some((host.to_string(), port.parse::<u16>().ok()?))
        }
        _ => Some((host_port.to_string(), 22)),
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
                let Some(pw) = auth.password(&params.user, &params.host, params.port) else {
                    return Err(SshError::AuthCancelled);
                };
                let res = handle.authenticate_password(&params.user, pw).await?;
                if res.success() {
                    return Ok(());
                }
            }
            AuthMethod::PublicKey { key_path } => {
                tried.push("publickey");
                let key = match load_private_key(key_path, auth) {
                    Ok(key) => key,
                    Err(e) => {
                        tracing::warn!("public-key authentication unavailable: {e}");
                        continue;
                    }
                };
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
                tried.push("agent");
                match authenticate_agent(handle, &params.user).await {
                    Ok(true) => return Ok(()),
                    Ok(false) => {}
                    Err(e) => {
                        tracing::warn!("ssh-agent authentication unavailable: {e}");
                    }
                }
            }
        }
    }

    Err(SshError::AuthFailed(tried.join(", ")))
}

#[cfg(unix)]
async fn authenticate_agent(handle: &mut Handle<ClientHandler>, user: &str) -> Result<bool> {
    let mut agent = russh::keys::agent::client::AgentClient::connect_env()
        .await
        .map_err(|e| SshError::Agent(e.to_string()))?;
    authenticate_with_agent(handle, user, &mut agent).await
}

#[cfg(windows)]
async fn authenticate_agent(handle: &mut Handle<ClientHandler>, user: &str) -> Result<bool> {
    let mut agent = russh::keys::agent::client::AgentClient::connect_pageant()
        .await
        .map_err(|e| SshError::Agent(e.to_string()))?;
    authenticate_with_agent(handle, user, &mut agent).await
}

#[cfg(not(any(unix, windows)))]
async fn authenticate_agent(_handle: &mut Handle<ClientHandler>, _user: &str) -> Result<bool> {
    Err(SshError::Agent(
        "ssh-agent is not supported on this platform".to_string(),
    ))
}

async fn authenticate_with_agent<R>(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    agent: &mut russh::keys::agent::client::AgentClient<R>,
) -> Result<bool>
where
    R: AsyncRead + AsyncWrite + Unpin + Send,
{
    let identities = agent
        .request_identities()
        .await
        .map_err(|e| SshError::Agent(e.to_string()))?;
    for identity in identities {
        let key = identity.public_key().into_owned();
        let res = handle
            .authenticate_publickey_with(user, key, Some(HashAlg::Sha256), agent)
            .await
            .map_err(|e| SshError::Agent(e.to_string()))?;
        if res.success() {
            return Ok(true);
        }
    }
    Ok(false)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_jump_target_parses_user_host_and_port() {
        let target = ProxyJumpTarget::parse("ops@example.com:2200").unwrap();
        assert_eq!(target.user.as_deref(), Some("ops"));
        assert_eq!(target.host, "example.com");
        assert_eq!(target.port, 2200);
    }

    #[test]
    fn proxy_jump_target_defaults_port_and_accepts_ipv6_brackets() {
        let target = ProxyJumpTarget::parse("[2001:db8::1]").unwrap();
        assert_eq!(target.user, None);
        assert_eq!(target.host, "2001:db8::1");
        assert_eq!(target.port, 22);
    }

    #[test]
    fn proxy_url_parser_supports_http_and_socks_defaults() {
        assert_eq!(
            parse_proxy_url("socks5://127.0.0.1").unwrap(),
            Some(SshProxy::Socks {
                host: "127.0.0.1".to_string(),
                port: 1080,
                version: 5,
            })
        );
        assert_eq!(
            parse_proxy_url("http://proxy.local:3128").unwrap(),
            Some(SshProxy::Http {
                host: "proxy.local".to_string(),
                port: 3128,
            })
        );
    }

    #[test]
    fn proxy_helpers_build_connect_requests() {
        assert_eq!(
            http_connect_status("HTTP/1.1 200 Connection Established\r\n\r\n"),
            Some(200)
        );
        assert_eq!(
            socks5_connect_request("example.com", 22).unwrap(),
            [
                0x05, 0x01, 0x00, 0x03, 11, b'e', b'x', b'a', b'm', b'p', b'l', b'e', b'.', b'c',
                b'o', b'm', 0, 22,
            ]
        );
        assert_eq!(format_host_port("2001:db8::1", 22), "[2001:db8::1]:22");
    }

    #[test]
    fn no_proxy_matches_exact_or_suffix_patterns() {
        assert!(no_proxy_matches(
            "localhost,.example.com",
            "api.example.com"
        ));
        assert!(no_proxy_matches("localhost,.example.com", "localhost"));
        assert!(!no_proxy_matches("localhost,.example.com", "example.net"));
    }
}
