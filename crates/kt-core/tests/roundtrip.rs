//! End-to-end round-trip test against an **in-process SSH server**.
//!
//! This is the deterministic equivalent of "connect to a real host": it spins
//! up a minimal russh echo server on a loopback port, then drives the real
//! [`SessionManager`] to connect, authenticate (password), open a PTY shell,
//! receive bytes, and render them through the `TermEngine` into a
//! [`GridSnapshot`]. It exercises the entire core pipeline with no external
//! dependencies.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::server::{self, Auth, Msg, Session as ServerSession};
use russh::{Channel, ChannelId};

use kt_config::{AuthMethod, ConnectParams};
use kt_core::remote_ops::read_command;
use kt_core::session::{AuthProviderFactory, SessionId};
use kt_core::shell_integration::{BOOTSTRAP_COMMAND, BOOTSTRAP_DONE_MARKER};
use kt_core::ssh::{AcceptAllVerifier, AuthProvider, HostKeyVerifier};
use kt_core::{
    FromCore, OperationId, OperationsDomain, OperationsRequest, PtySize, SessionManager, ToCore,
};

/// Minimal server handler: accepts password "test", echoes a banner on shell
/// request, and echoes back any input the client types (upper-cased so we can
/// distinguish server output from a naive loopback).
///
/// `EchoServer` implements [`server::Server`] for completeness/documentation;
/// the test drives a single connection via `run_stream` with a handler directly.
#[allow(dead_code)]
struct EchoServer {
    state: SharedEchoState,
}

impl server::Server for EchoServer {
    type Handler = EchoHandler;
    fn new_client(&mut self, _peer: Option<std::net::SocketAddr>) -> EchoHandler {
        EchoHandler {
            defer_banner_until_bootstrap: false,
            state: self.state.clone(),
        }
    }
}

#[derive(Default)]
struct EchoState {
    exec_commands: Vec<Vec<u8>>,
    pty_requests: Vec<(ChannelId, u32, u32)>,
    channel_closes: Vec<ChannelId>,
    hold_container_start: bool,
}

type SharedEchoState = Arc<Mutex<EchoState>>;

struct EchoHandler {
    defer_banner_until_bootstrap: bool,
    state: SharedEchoState,
}

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

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        if let Ok(mut state) = self.state.lock() {
            state.channel_closes.push(channel);
        }
        Ok(())
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
        if let Ok(mut state) = self.state.lock() {
            state.pty_requests.push((channel, _cols, _rows));
        }
        session.channel_success(channel)?;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        if let Ok(mut state) = self.state.lock() {
            state.exec_commands.push(data.to_vec());
        }
        let hold_container_start = self
            .state
            .lock()
            .map(|state| state.hold_container_start)
            .unwrap_or(false);

        if !(data == b"docker exec -it safe-container /bin/sh" && hold_container_start) {
            session.channel_success(channel)?;
        }

        if data == read_command(OperationsDomain::Processes).as_bytes() {
            session.data(
                channel,
                bytes::Bytes::from_static(b"7 1 1000 S 1.2 3.4 42 256 00:01 worker process\n"),
            )?;
            session.extended_data(
                channel,
                1,
                bytes::Bytes::from_static(b"stderr-canary-not-an-event"),
            )?;
            // OpenSSH commonly sends EOF before the exit status; the runner must
            // keep draining until it observes both messages.
            session.eof(channel)?;
            session.exit_status_request(channel, 0)?;
            session.close(channel)?;
        } else if data == b"docker exec -it safe-container /bin/sh" {
            if hold_container_start {
                return Ok(());
            }
            session.data(channel, bytes::Bytes::from_static(b"container-ready\r\n"))?;
        } else {
            session.channel_failure(channel)?;
        }
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        if !self.defer_banner_until_bootstrap {
            // 横幅后发 DSR 光标位置查询，验证终端生成的 PtyWrite 会写回远端 shell。
            session.data(channel, bytes::Bytes::from_static(b"READY> \x1b[6n"))?;
        }
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        if self.defer_banner_until_bootstrap && data == BOOTSTRAP_COMMAND.as_bytes() {
            // 模拟最容易触发旧竞态的时序：登录横幅尚在 channel 排队时，客户端已
            // 写入 bootstrap；服务器把 MOTD、prompt、命令回显和完成标记放进同一包。
            let mut output = b"Welcome to Test Linux\r\nLast login: today\r\nrace$ ".to_vec();
            output.extend_from_slice(data);
            output.extend_from_slice(b"\x1b]7;file:///home/tester\x07");
            output.extend_from_slice(BOOTSTRAP_DONE_MARKER);
            output.extend_from_slice(b"race$ ");
            session.data(channel, bytes::Bytes::from(output))?;
        } else if data.starts_with(b"\x1b[") && data.ends_with(b"R") {
            session.data(channel, bytes::Bytes::from_static(b"DSR_OK "))?;
        } else {
            // Echo back upper-cased.
            let upper: Vec<u8> = data.iter().map(|b| b.to_ascii_uppercase()).collect();
            session.data(channel, bytes::Bytes::from(upper))?;
        }
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
async fn start_server(defer_banner_until_bootstrap: bool) -> u16 {
    start_server_with_state(
        defer_banner_until_bootstrap,
        Arc::new(Mutex::new(EchoState::default())),
    )
    .await
    .0
}

async fn start_server_with_state(
    defer_banner_until_bootstrap: bool,
    state: SharedEchoState,
) -> (u16, SharedEchoState) {
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

    let server_state = state.clone();
    tokio::spawn(async move {
        // Accept exactly one connection and run it.
        if let Ok((stream, _addr)) = listener.accept().await {
            let handler = EchoHandler {
                defer_banner_until_bootstrap,
                state: server_state,
            };
            let _ = server::run_stream(config, stream, handler).await;
            // Keep the task alive while the session runs.
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });

    (port, state)
}

#[test]
fn full_roundtrip_through_term_engine() {
    // The SessionManager owns its own runtime; we need a separate one to host
    // the test server. Use a dedicated current-thread runtime on a thread.
    let server_rt = tokio::runtime::Runtime::new().unwrap();
    let port = server_rt.block_on(start_server(false));
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
        proxy: kt_config::ProxyConfig::Direct,
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
    let mut saw_pty_writeback = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);

    while std::time::Instant::now() < deadline && !(connected && saw_banner && saw_pty_writeback) {
        match recv_timeout(&mut mgr, Duration::from_secs(5)) {
            Some(FromCore::Connected { .. }) => connected = true,
            Some(FromCore::Render { snapshot, .. }) => {
                if snapshot.to_plain_text().contains("READY>") {
                    saw_banner = true;
                }
                if snapshot.to_plain_text().contains("DSR_OK") {
                    saw_pty_writeback = true;
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
    assert!(
        saw_pty_writeback,
        "terminal DSR response was not written back to the remote shell"
    );

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
                if snapshot.to_plain_text().contains("DSR_OK HI") {
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

#[test]
fn shell_integration_keeps_login_banner_when_setup_wins_the_race() {
    let server_rt = tokio::runtime::Runtime::new().unwrap();
    let port = server_rt.block_on(start_server(true));
    let _server_guard = server_rt;

    let verifier: Arc<dyn HostKeyVerifier> = Arc::new(AcceptAllVerifier);
    let factory: Arc<dyn AuthProviderFactory> = Arc::new(FixedPasswordFactory);
    let mut mgr = SessionManager::spawn(verifier, factory).expect("spawn core");
    let id = SessionId(2);
    mgr.send(ToCore::Connect {
        id,
        params: Box::new(ConnectParams {
            host: "127.0.0.1".into(),
            port,
            user: "tester".into(),
            auth: vec![AuthMethod::Password],
            vault_id: None,
            proxy_jump: None,
            proxy: kt_config::ProxyConfig::Direct,
            forward_agent: false,
        }),
        pty: PtySize {
            cols: 100,
            rows: 24,
        },
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut setup_sent = false;
    let mut saw_banner = false;
    let mut saw_cwd = false;
    while std::time::Instant::now() < deadline && !(saw_banner && saw_cwd) {
        match recv_timeout(&mut mgr, Duration::from_secs(3)) {
            Some(FromCore::Connected { .. }) => {
                mgr.send(ToCore::SetupShellIntegration { id });
                setup_sent = true;
            }
            Some(FromCore::Render { snapshot, .. }) => {
                let text = snapshot.to_plain_text();
                if text.contains("Welcome to Test Linux") && text.contains("Last login: today") {
                    assert!(text.contains("race$"), "最终 prompt 应可见: {text:?}");
                    assert!(!text.contains("__kt_cwd"), "bootstrap 不得可见: {text:?}");
                    saw_banner = true;
                }
            }
            Some(FromCore::Cwd { path, .. }) if path == "/home/tester" => saw_cwd = true,
            Some(FromCore::Closed { error, .. }) => {
                panic!("session closed early: {error:?}");
            }
            Some(_) => {}
            None => break,
        }
    }

    assert!(setup_sent, "never received Connected");
    assert!(saw_banner, "login banner was lost during bootstrap setup");
    assert!(saw_cwd, "bootstrap OSC 7 cwd was not reported");

    mgr.send(ToCore::Disconnect { id });
}

#[test]
fn readonly_operations_use_allowlisted_exec_and_keep_stderr_out_of_events() {
    let server_rt = tokio::runtime::Runtime::new().unwrap();
    let (port, state) = server_rt.block_on(start_server_with_state(
        false,
        Arc::new(Mutex::new(EchoState::default())),
    ));
    let _server_guard = server_rt;

    let verifier: Arc<dyn HostKeyVerifier> = Arc::new(AcceptAllVerifier);
    let factory: Arc<dyn AuthProviderFactory> = Arc::new(FixedPasswordFactory);
    let mut mgr = SessionManager::spawn(verifier, factory).expect("spawn core");
    let id = SessionId(3);
    mgr.send(ToCore::Connect {
        id,
        params: Box::new(ConnectParams {
            host: "127.0.0.1".into(),
            port,
            user: "tester".into(),
            auth: vec![AuthMethod::Password],
            vault_id: None,
            proxy_jump: None,
            proxy: kt_config::ProxyConfig::Direct,
            forward_agent: false,
        }),
        pty: PtySize::default(),
    });

    wait_for_connected(&mut mgr, id);
    let operation_id = OperationId(41);
    mgr.send(ToCore::Operations {
        id,
        operation_id,
        request: OperationsRequest::Refresh(OperationsDomain::Processes),
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut result = None;
    let mut leaked_stderr = false;
    while std::time::Instant::now() < deadline && result.is_none() {
        match recv_timeout(&mut mgr, Duration::from_secs(2)) {
            Some(FromCore::OperationResult {
                id: result_id,
                operation_id: result_operation_id,
                domain,
                result: operation_result,
            }) => {
                assert_eq!(result_id, id);
                assert_eq!(result_operation_id, operation_id);
                assert_eq!(domain, OperationsDomain::Processes);
                result = Some(operation_result);
            }
            Some(FromCore::Render { snapshot, .. }) => {
                leaked_stderr |= snapshot
                    .to_plain_text()
                    .contains("stderr-canary-not-an-event");
                assert!(!snapshot.to_plain_text().contains("worker process"));
            }
            Some(FromCore::OperationFailed { error, .. }) => {
                panic!("operations request failed: {error:?}");
            }
            Some(_) => {}
            None => break,
        }
    }
    let Some(kt_core::OperationsResult::Processes(rows)) = result else {
        panic!("never received process result");
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pid, 7);
    assert_eq!(rows[0].command, "worker process");
    assert!(!leaked_stderr, "stderr must not enter terminal events");
    let commands = state.lock().unwrap().exec_commands.clone();
    assert_eq!(
        commands,
        vec![read_command(OperationsDomain::Processes).as_bytes()]
    );

    mgr.send(ToCore::Disconnect { id });
}

#[test]
fn container_pty_is_isolated_validated_and_rendered() {
    let server_rt = tokio::runtime::Runtime::new().unwrap();
    let (port, state) = server_rt.block_on(start_server_with_state(
        false,
        Arc::new(Mutex::new(EchoState::default())),
    ));
    let _server_guard = server_rt;

    let verifier: Arc<dyn HostKeyVerifier> = Arc::new(AcceptAllVerifier);
    let factory: Arc<dyn AuthProviderFactory> = Arc::new(FixedPasswordFactory);
    let mut mgr = SessionManager::spawn(verifier, factory).expect("spawn core");
    let id = SessionId(4);
    mgr.send(ToCore::Connect {
        id,
        params: Box::new(ConnectParams {
            host: "127.0.0.1".into(),
            port,
            user: "tester".into(),
            auth: vec![AuthMethod::Password],
            vault_id: None,
            proxy_jump: None,
            proxy: kt_config::ProxyConfig::Direct,
            forward_agent: false,
        }),
        pty: PtySize::default(),
    });
    wait_for_connected(&mut mgr, id);

    let exec_id = kt_core::ExecId(9);
    mgr.send(ToCore::OpenContainerTerminal {
        id,
        exec_id,
        container_id: "safe-container".into(),
        pty: PtySize { cols: 90, rows: 25 },
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut started = false;
    let mut saw_ready = false;
    while std::time::Instant::now() < deadline && !(started && saw_ready) {
        match recv_timeout(&mut mgr, Duration::from_secs(2)) {
            Some(FromCore::ExecStarted {
                id: event_id,
                exec_id: event_exec_id,
                container_id,
            }) => {
                assert_eq!(event_id, id);
                assert_eq!(event_exec_id, exec_id);
                assert_eq!(container_id, "safe-container");
                started = true;
            }
            Some(FromCore::ExecRender {
                id: event_id,
                exec_id: event_exec_id,
                snapshot,
            }) => {
                assert_eq!(event_id, id);
                assert_eq!(event_exec_id, exec_id);
                saw_ready |= snapshot.to_plain_text().contains("container-ready");
            }
            Some(FromCore::ExecClosed {
                error: Some(error), ..
            }) => panic!("container closed: {error}"),
            Some(_) => {}
            None => break,
        }
    }
    assert!(started, "container exec was not acknowledged");
    assert!(saw_ready, "container output was not rendered");
    let state_after_open = state.lock().unwrap();
    assert_eq!(
        state_after_open.exec_commands,
        vec![b"docker exec -it safe-container /bin/sh".to_vec()]
    );
    assert!(state_after_open
        .pty_requests
        .iter()
        .any(|(_, cols, rows)| (*cols, *rows) == (90, 25)));
    drop(state_after_open);

    mgr.send(ToCore::ContainerInput {
        id,
        exec_id,
        data: b"ls\n".to_vec(),
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut saw_input_echo = false;
    while std::time::Instant::now() < deadline && !saw_input_echo {
        if let Some(FromCore::ExecRender { snapshot, .. }) =
            recv_timeout(&mut mgr, Duration::from_secs(1))
        {
            saw_input_echo = snapshot.to_plain_text().contains("LS");
        }
    }
    assert!(
        saw_input_echo,
        "container input did not stay on the container PTY"
    );

    // Invalid identifiers are rejected before channel_open/exec; no second
    // command should reach the server.
    let invalid_exec_id = kt_core::ExecId(10);
    mgr.send(ToCore::OpenContainerTerminal {
        id,
        exec_id: invalid_exec_id,
        container_id: "bad;id".into(),
        pty: PtySize::default(),
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut rejected = false;
    while std::time::Instant::now() < deadline && !rejected {
        if let Some(FromCore::ExecClosed {
            exec_id,
            error: Some(_),
            ..
        }) = recv_timeout(&mut mgr, Duration::from_secs(1))
        {
            rejected = exec_id == invalid_exec_id;
        }
    }
    assert!(rejected, "invalid container id was not rejected");
    let commands = state.lock().unwrap().exec_commands.clone();
    assert_eq!(commands.len(), 1, "invalid id must not reach remote exec");
    mgr.send(ToCore::Disconnect { id });
}

#[test]
fn container_pty_close_during_startup_converges_without_waiting_for_confirmation() {
    let server_rt = tokio::runtime::Runtime::new().unwrap();
    let state = Arc::new(Mutex::new(EchoState {
        hold_container_start: true,
        ..EchoState::default()
    }));
    let (port, state) = server_rt.block_on(start_server_with_state(false, state));
    let _server_guard = server_rt;

    let verifier: Arc<dyn HostKeyVerifier> = Arc::new(AcceptAllVerifier);
    let factory: Arc<dyn AuthProviderFactory> = Arc::new(FixedPasswordFactory);
    let mut mgr = SessionManager::spawn(verifier, factory).expect("spawn core");
    let id = SessionId(5);
    mgr.send(ToCore::Connect {
        id,
        params: Box::new(ConnectParams {
            host: "127.0.0.1".into(),
            port,
            user: "tester".into(),
            auth: vec![AuthMethod::Password],
            vault_id: None,
            proxy_jump: None,
            proxy: kt_config::ProxyConfig::Direct,
            forward_agent: false,
        }),
        pty: PtySize::default(),
    });
    wait_for_connected(&mut mgr, id);

    let exec_id = kt_core::ExecId(11);
    mgr.send(ToCore::OpenContainerTerminal {
        id,
        exec_id,
        container_id: "safe-container".into(),
        pty: PtySize::default(),
    });
    // Let the open request reach the server and enter the startup wait. The
    // server intentionally never confirms the exec request.
    std::thread::sleep(Duration::from_millis(100));
    mgr.send(ToCore::CloseContainerTerminal { id, exec_id });

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut closed_events = 0;
    while std::time::Instant::now() < deadline {
        match recv_timeout(&mut mgr, Duration::from_millis(100)) {
            Some(FromCore::ExecClosed {
                exec_id: event_exec_id,
                error,
                ..
            }) if event_exec_id == exec_id => {
                assert!(error.is_none(), "explicit close should be clean: {error:?}");
                closed_events += 1;
                if closed_events > 1 {
                    break;
                }
            }
            Some(_) | None => {}
        }
    }
    assert_eq!(
        closed_events, 1,
        "startup close must emit one terminal event"
    );

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let saw_channel_close = loop {
        if !state.lock().unwrap().channel_closes.is_empty() {
            break true;
        }
        if std::time::Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        saw_channel_close,
        "startup close must send SSH channel close"
    );

    mgr.send(ToCore::Disconnect { id });
}

fn wait_for_connected(mgr: &mut SessionManager, id: SessionId) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        match recv_timeout(mgr, Duration::from_secs(2)) {
            Some(FromCore::Connected { id: connected_id }) if connected_id == id => return,
            Some(FromCore::Closed { error, .. }) => {
                panic!("session closed before connect: {error:?}")
            }
            Some(_) => {}
            None => break,
        }
    }
    panic!("never received Connected");
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
