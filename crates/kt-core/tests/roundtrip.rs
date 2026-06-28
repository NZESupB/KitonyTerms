//! End-to-end round-trip test against an **in-process SSH server**.
//!
//! This is the deterministic equivalent of "connect to a real host": it spins
//! up a minimal russh echo server on a loopback port, then drives the real
//! [`SessionManager`] to connect, authenticate (password), open a PTY shell,
//! receive bytes, and render them through the `TermEngine` into a
//! [`GridSnapshot`]. It exercises the entire core pipeline with no external
//! dependencies.

use std::sync::Arc;
use std::time::Duration;

use russh::server::{self, Auth, Msg, Session as ServerSession};
use russh::{Channel, ChannelId};

use kt_config::{AuthMethod, ConnectParams};
use kt_core::session::{AuthProviderFactory, SessionId};
use kt_core::ssh::{AcceptAllVerifier, AuthProvider, HostKeyVerifier};
use kt_core::{FromCore, PtySize, SessionManager, ToCore};

/// Minimal server handler: accepts password "test", echoes a banner on shell
/// request, and echoes back any input the client types (upper-cased so we can
/// distinguish server output from a naive loopback).
///
/// `EchoServer` implements [`server::Server`] for completeness/documentation;
/// the test drives a single connection via `run_stream` with a handler directly.
#[allow(dead_code)]
struct EchoServer;

impl server::Server for EchoServer {
    type Handler = EchoHandler;
    fn new_client(&mut self, _peer: Option<std::net::SocketAddr>) -> EchoHandler {
        EchoHandler
    }
}

struct EchoHandler;

impl server::Handler for EchoHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if user == "tester" && password == "test" {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        _session: &mut ServerSession,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _cols: u32,
        _rows: u32,
        _pw: u32,
        _ph: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        // Send a recognizable banner once the shell starts.
        session.data(channel, bytes::Bytes::from_static(b"READY> "))?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        // Echo back upper-cased.
        let upper: Vec<u8> = data.iter().map(|b| b.to_ascii_uppercase()).collect();
        session.data(channel, bytes::Bytes::from(upper))?;
        Ok(())
    }
}

/// Auth provider that always supplies the password "test".
struct FixedPassword;
impl AuthProvider for FixedPassword {
    fn password(&mut self, _user: &str, _host: &str, _port: u16) -> Option<String> {
        Some("test".to_string())
    }
    fn key_passphrase(&mut self, _key_path: &str) -> Option<String> {
        None
    }
    fn keyboard_interactive(
        &mut self,
        _n: &str,
        _i: &str,
        _p: &[(String, bool)],
    ) -> Option<Vec<String>> {
        None
    }
}

struct FixedPasswordFactory;
impl AuthProviderFactory for FixedPasswordFactory {
    fn create(&self, _id: SessionId, _p: &ConnectParams) -> Box<dyn AuthProvider> {
        Box::new(FixedPassword)
    }
}

/// Start the echo server on an ephemeral loopback port; returns the bound port.
async fn start_server() -> u16 {
    let key = russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
        .expect("generate host key");
    let config = Arc::new(server::Config {
        keys: vec![key],
        ..Default::default()
    });

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind");
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        // Accept exactly one connection and run it.
        if let Ok((stream, _addr)) = listener.accept().await {
            let handler = EchoHandler;
            let _ = server::run_stream(config, stream, handler).await;
            // Keep the task alive while the session runs.
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });

    port
}

#[test]
fn full_roundtrip_through_term_engine() {
    // The SessionManager owns its own runtime; we need a separate one to host
    // the test server. Use a dedicated current-thread runtime on a thread.
    let server_rt = tokio::runtime::Runtime::new().unwrap();
    let port = server_rt.block_on(start_server());
    // Keep the server runtime alive for the duration of the test.
    let _server_guard = server_rt;

    let verifier: Arc<dyn HostKeyVerifier> = Arc::new(AcceptAllVerifier);
    let factory: Arc<dyn AuthProviderFactory> = Arc::new(FixedPasswordFactory);
    let mut mgr = SessionManager::spawn(verifier, factory).expect("spawn core");

    let id = SessionId(1);
    let params = ConnectParams {
        host: "127.0.0.1".into(),
        port,
        user: "tester".into(),
        auth: vec![AuthMethod::Password],
        vault_id: None,
        proxy_jump: None,
        forward_agent: false,
    };
    mgr.send(ToCore::Connect {
        id,
        params: Box::new(params),
        pty: PtySize { cols: 80, rows: 24 },
    });

    // 1) Expect Connected, then a Render containing the server banner.
    let mut connected = false;
    let mut saw_banner = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);

    while std::time::Instant::now() < deadline && !(connected && saw_banner) {
        match recv_timeout(&mut mgr, Duration::from_secs(5)) {
            Some(FromCore::Connected { .. }) => connected = true,
            Some(FromCore::Render { snapshot, .. }) => {
                if snapshot.to_plain_text().contains("READY>") {
                    saw_banner = true;
                }
            }
            Some(FromCore::Closed { error, .. }) => {
                panic!("session closed early: {error:?}");
            }
            Some(_) => {}
            None => panic!("timed out waiting for connect/banner"),
        }
    }
    assert!(connected, "never received Connected");
    assert!(saw_banner, "never rendered the server banner");

    // 2) Type "hi" — server echoes "HI"; verify it lands in the grid.
    mgr.send(ToCore::Input {
        id,
        data: b"hi".to_vec(),
    });

    let mut saw_echo = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline && !saw_echo {
        match recv_timeout(&mut mgr, Duration::from_secs(5)) {
            Some(FromCore::Render { snapshot, .. }) => {
                if snapshot.to_plain_text().contains("READY> HI") {
                    saw_echo = true;
                }
            }
            Some(FromCore::Closed { error, .. }) => panic!("closed during echo: {error:?}"),
            Some(_) => {}
            None => break,
        }
    }
    assert!(
        saw_echo,
        "server echo 'HI' never appeared in the rendered grid"
    );

    // 3) Clean disconnect.
    mgr.send(ToCore::Disconnect { id });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut closed = false;
    while std::time::Instant::now() < deadline && !closed {
        match recv_timeout(&mut mgr, Duration::from_secs(2)) {
            Some(FromCore::Closed { .. }) => closed = true,
            Some(_) => {}
            None => break,
        }
    }
    assert!(closed, "never received Closed after Disconnect");
}

/// Block for the next event with an overall timeout, polling `try_recv`.
fn recv_timeout(mgr: &mut SessionManager, timeout: Duration) -> Option<FromCore> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(ev) = mgr.try_recv() {
            return Some(ev);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
