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

fn selection_request(
    manager: &SessionManager,
    id: SessionId,
    exec_id: Option<kt_core::ExecId>,
    owner: &str,
    action: kt_core::term::selection::SelectionAction,
) -> Result<kt_core::term::selection::SelectionReply, String> {
    use kt_core::term::selection::SelectionCommand;
    let (reply, mut receiver) = tokio::sync::oneshot::channel();
    assert!(manager.send(ToCore::TerminalSelection {
        id,
        exec_id,
        command: SelectionCommand {
            owner: owner.into(),
            action,
            reply
        },
    }));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match receiver.try_recv() {
            Ok(result) => return result,
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => panic!("选区请求缺少终态"),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                assert!(std::time::Instant::now() < deadline, "选区请求超时");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

fn verify_buffer_copy(
    manager: &SessionManager,
    id: SessionId,
    exec_id: Option<kt_core::ExecId>,
    snapshot: &kt_core::GridSnapshot,
    needle: &str,
) {
    use kt_core::term::selection::{SelectionAction, SelectionMode, SelectionPoint};
    let (row, column) = (0..snapshot.rows)
        .find_map(|row| snapshot.row_text(row).find(needle).map(|col| (row, col)))
        .expect("测试文本应已渲染");
    let anchor = SelectionPoint {
        row,
        column,
        right: false,
        revision: snapshot.revision,
        display_offset: snapshot.display_offset,
    };
    let focus = SelectionPoint {
        column: column + needle.len() - 1,
        right: true,
        ..anchor
    };
    assert!(
        selection_request(
            manager,
            id,
            exec_id,
            "test-pane",
            SelectionAction::Begin {
                point: anchor,
                mode: SelectionMode::Simple
            }
        )
        .unwrap()
        .accepted
    );
    assert!(
        selection_request(
            manager,
            id,
            exec_id,
            "test-pane",
            SelectionAction::Extend {
                point: focus,
                scroll: 0
            }
        )
        .unwrap()
        .accepted
    );
    assert_eq!(
        selection_request(manager, id, exec_id, "test-pane", SelectionAction::Copy)
            .unwrap()
            .text
            .as_deref(),
        Some(needle)
    );
    assert!(
        selection_request(manager, id, exec_id, "other-pane", SelectionAction::Copy)
            .unwrap()
            .text
            .is_none()
    );
}

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
    operation_mode: Option<OperationMode>,
    monitor: Option<MonitorScenario>,
    connections: usize,
    exec_replies: std::collections::HashMap<Vec<u8>, Vec<u8>>,
}

#[derive(Default)]
struct MonitorScenario {
    channel: Option<ChannelId>,
    input: String,
    delays: std::collections::VecDeque<Option<Duration>>,
    sample_delay: Duration,
    stale_marker: Option<String>,
}

#[derive(Clone, Copy)]
enum OperationMode {
    Opening,
    Reading,
    Sudo,
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
        if data == b"sh" {
            if let Some(monitor) = self.state.lock().unwrap().monitor.as_mut() {
                monitor.channel = Some(channel);
                session.channel_success(channel)?;
                return Ok(());
            }
        }
        let hold_container_start = self
            .state
            .lock()
            .map(|state| state.hold_container_start)
            .unwrap_or(false);

        let reply = self.state.lock().unwrap().exec_replies.get(data).cloned();
        if let Some(reply) = reply {
            session.channel_success(channel)?;
            for chunk in reply.chunks(16384) {
                session.data(channel, bytes::Bytes::copy_from_slice(chunk))?;
            }
            session.extended_data(
                channel,
                1,
                bytes::Bytes::from_static(b"docker-stderr-canary"),
            )?;
            session.eof(channel)?;
            session.exit_status_request(channel, 0)?;
            session.close(channel)?;
            return Ok(());
        }

        if let Some(mode) = self.state.lock().unwrap().operation_mode {
            if !matches!(mode, OperationMode::Opening) {
                session.channel_success(channel)?;
            }
            if matches!(mode, OperationMode::Sudo) {
                session.extended_data(
                    channel,
                    1,
                    bytes::Bytes::from_static(b"__KT_SUDO_PROMPT__"),
                )?;
            }
            return Ok(());
        }

        if !(data == b"docker exec -it safe-container /bin/sh" && hold_container_start) {
            session.channel_success(channel)?;
        }

        if data == kt_core::remote_ops::PROBE_COMMAND.as_bytes() {
            session.data(channel, bytes::Bytes::from_static(br#"{"linux":true,"systemd":false,"ps":true,"proc":true,"ss":false,"proc_net":true,"docker_daemon":false,"compose":true}"#))?;
            session.eof(channel)?;
            session.exit_status_request(channel, 0)?;
            session.close(channel)?;
        } else if data == read_command(OperationsDomain::Processes).as_bytes() {
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
        if terminal_render::scripted_output(channel, data, session)? {
            return Ok(());
        }
        {
            let mut state = self.state.lock().unwrap();
            if let Some(monitor) = state
                .monitor
                .as_mut()
                .filter(|m| m.channel == Some(channel))
            {
                monitor.input.push_str(std::str::from_utf8(data).unwrap());
                while let Some(end) = monitor.input.find('\n') {
                    let command: String = monitor.input.drain(..=end).collect();
                    let (delay, response) = if let Some(body) = command.strip_prefix("printf '") {
                        let marker = body.strip_suffix("\\n'\n").expect("轻量心跳命令");
                        assert!(marker.starts_with("__KTM_HEARTBEAT__"));
                        let response = format!("{marker}\n");
                        // 旧响应特意留到下一次请求，检验序号匹配。
                        if let Some(stale) = monitor.stale_marker.take() {
                            session.data(channel, bytes::Bytes::from(stale))?;
                        }
                        match monitor.delays.pop_front().expect("意外的额外心跳") {
                            Some(delay) => (delay, response),
                            None => {
                                monitor.stale_marker = Some(response);
                                continue;
                            }
                        }
                    } else {
                        assert!(command.starts_with("echo __KTM_BEGIN__;"));
                        (monitor.sample_delay, "__KTM_BEGIN__\ncpu 1 0 1 8\n__KTM_SEC__\nMemTotal: 100 kB\nMemAvailable: 50 kB\n__KTM_SEC__\n__KTM_SEC__\n__KTM_SEC__\n0.1\n__KTM_SEC__\n100\n__KTM_SEC__\nhostname=test\n__KTM_END__\n".to_string())
                    };
                    let handle = session.handle();
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        let _ = handle.data(channel, bytes::Bytes::from(response)).await;
                    });
                }
                return Ok(());
            }
        }
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
            server_state.lock().unwrap().connections += 1;
            let handler = EchoHandler {
                defer_banner_until_bootstrap,
                state: server_state.clone(),
            };
            let _ = server::run_stream(config, stream, handler).await;
            // 接受并计数额外探测连接，确保测试能发现 TCP 探测回归。
            while let Ok(Ok((stream, _))) =
                tokio::time::timeout(Duration::from_secs(30), listener.accept()).await
            {
                server_state.lock().unwrap().connections += 1;
                drop(stream);
            }
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
    let mut echo_snapshot = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline && !saw_echo {
        match recv_timeout(&mut mgr, Duration::from_secs(5)) {
            Some(FromCore::Render { snapshot, .. }) => {
                if snapshot.to_plain_text().contains("DSR_OK HI") {
                    saw_echo = true;
                    echo_snapshot = Some(*snapshot);
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
    verify_buffer_copy(&mgr, id, None, echo_snapshot.as_ref().unwrap(), "DSR_OK HI");
    assert!(selection_request(
        &mgr,
        SessionId(9999),
        None,
        "test-pane",
        kt_core::term::selection::SelectionAction::Copy
    )
    .is_err());

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
fn docker_usage_queries_return_typed_results_over_independent_exec() {
    use kt_core::{DockerResource, OperationsResult};
    let requests = [
        (
            OperationsRequest::DockerStats { container: None },
            r#"{"ID":"abc","CPUPerc":"250.5%","MemUsage":"1MiB / 1GiB","MemPerc":"0.1%"}"#,
        ),
        (
            OperationsRequest::DockerStorage {
                containers: vec!["abc".into()],
            },
            r#"{"id":"abc","image_id":"sha256:def","size_rw":10,"size_rootfs":100}"#,
        ),
        (
            OperationsRequest::DockerImageSizes {
                images: vec!["sha256:def".into()],
            },
            r#"{"id":"sha256:def","size":90}"#,
        ),
        (
            OperationsRequest::DockerContainerNetwork {
                container: "abc".into(),
            },
            r#"{"bridge":{"IPAddress":"172.17.0.2"}}"#,
        ),
        (
            OperationsRequest::DockerList(DockerResource::Containers),
            r#"{"ID":"abc","Names":"web","State":"running","Status":"Up","Image":"example"}"#,
        ),
    ];
    let mut echo = EchoState::default();
    let commands: Vec<_> = requests
        .iter()
        .map(|(request, response)| {
            let command = kt_core::remote_ops::command_for_request(request).unwrap();
            echo.exec_replies
                .insert(command.as_bytes().to_vec(), response.as_bytes().to_vec());
            command.into_bytes()
        })
        .collect();
    assert_eq!(
        commands[0],
        b"LC_ALL=C docker stats --no-stream --no-trunc --format '{{json .}}' --all"
    );
    assert!(String::from_utf8_lossy(&commands[1]).contains("--size --format"));
    assert!(String::from_utf8_lossy(&commands[1]).contains(".SizeRw"));
    assert!(!String::from_utf8_lossy(&commands[1]).contains(".Config"));
    assert_eq!(
        commands[3],
        b"docker container inspect --format '{{json .NetworkSettings.Networks}}' 'abc'"
    );
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (port, state) =
        runtime.block_on(start_server_with_state(false, Arc::new(Mutex::new(echo))));
    let (mut manager, id) = operations_test_manager(port);
    for (index, (request, _)) in requests.into_iter().enumerate() {
        let operation_id = OperationId(400 + index as u64);
        assert!(manager.send(ToCore::Operations {
            id,
            operation_id,
            request
        }));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut received = false;
        while std::time::Instant::now() < deadline && !received {
            match recv_timeout(&mut manager, Duration::from_millis(100)) {
                Some(FromCore::OperationResult {
                    operation_id: actual,
                    domain,
                    result,
                    ..
                }) => {
                    assert_eq!(actual, operation_id);
                    assert_eq!(domain, OperationsDomain::Docker);
                    match (index, result) {
                        (0, OperationsResult::DockerRuntime(rows)) => {
                            assert_eq!(rows[0].cpu_percent, Some(250.5));
                            assert_eq!(rows[0].memory_used_bytes, Some(1048576));
                        }
                        (1, OperationsResult::DockerStorage(rows)) => {
                            assert_eq!(rows[0].size_rw, Some(10))
                        }
                        (2, OperationsResult::DockerImageSizes(rows)) => {
                            assert_eq!(rows[0].size, Some(90))
                        }
                        (3, OperationsResult::Details { content, .. }) => {
                            assert!(content.contains("172.17.0.2"))
                        }
                        (4, OperationsResult::DockerContainers(rows)) => {
                            assert_eq!(rows[0].state.as_deref(), Some("running"))
                        }
                        (_, other) => panic!("Docker 返回类型错误: {other:?}"),
                    }
                    received = true;
                }
                Some(FromCore::Render { snapshot, .. }) => {
                    assert!(!snapshot.to_plain_text().contains("docker-stderr-canary"))
                }
                Some(FromCore::OperationFailed { error, .. }) => {
                    panic!("Docker 查询失败: {error:?}")
                }
                _ => {}
            }
        }
        assert!(received, "未收到 Docker 查询结果");
    }
    assert_eq!(state.lock().unwrap().exec_commands, commands);
    manager.send(ToCore::Disconnect { id });
}

#[test]
fn docker_stats_output_limit_rejects_payload_without_terminal_leak() {
    let command = kt_core::remote_ops::command_for_request(&OperationsRequest::DockerStats {
        container: None,
    })
    .unwrap();
    let mut echo = EchoState::default();
    echo.exec_replies
        .insert(command.into_bytes(), vec![b'x'; 9 * 1024 * 1024]);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (port, _) = runtime.block_on(start_server_with_state(false, Arc::new(Mutex::new(echo))));
    let (mut manager, id) = operations_test_manager(port);
    let operation_id = OperationId(499);
    manager.send(ToCore::Operations {
        id,
        operation_id,
        request: OperationsRequest::DockerStats { container: None },
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut failed = false;
    while std::time::Instant::now() < deadline && !failed {
        match recv_timeout(&mut manager, Duration::from_millis(100)) {
            Some(FromCore::OperationFailed {
                operation_id: actual,
                error,
                ..
            }) => {
                assert_eq!(actual, operation_id);
                assert_eq!(
                    error.kind,
                    kt_core::OperationsErrorKind::OutputLimitExceeded
                );
                failed = true;
            }
            Some(FromCore::OperationResult { .. }) => panic!("超限输出不能成功"),
            Some(FromCore::Render { snapshot, .. }) => {
                assert!(!snapshot.to_plain_text().contains("xxxxxxxx"))
            }
            _ => {}
        }
    }
    assert!(failed, "超限结果未收敛");
    manager.send(ToCore::Disconnect { id });
}

fn operations_test_manager(port: u16) -> (SessionManager, SessionId) {
    let mut manager =
        SessionManager::spawn(Arc::new(AcceptAllVerifier), Arc::new(FixedPasswordFactory)).unwrap();
    let id = SessionId(30);
    let mut params = ConnectParams::new("127.0.0.1", "tester");
    params.port = port;
    params.auth = vec![AuthMethod::Password];
    manager.send(ToCore::Connect {
        id,
        params: Box::new(params),
        pty: PtySize::default(),
    });
    wait_for_connected(&mut manager, id);
    (manager, id)
}

#[test]
fn operations_probe_uses_independent_exec_and_preserves_partial_capabilities() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (port, state) = runtime.block_on(start_server_with_state(
        false,
        Arc::new(Mutex::new(EchoState::default())),
    ));
    let (mut manager, id) = operations_test_manager(port);
    assert!(
        state.lock().unwrap().exec_commands.is_empty(),
        "连接本身不得主动探测"
    );
    manager.send(ToCore::Operations {
        id,
        operation_id: OperationId(100),
        request: OperationsRequest::ProbeCapabilities,
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut received = false;
    while std::time::Instant::now() < deadline && !received {
        match recv_timeout(&mut manager, Duration::from_millis(100)) {
            Some(FromCore::OperationResult {
                operation_id,
                result: kt_core::OperationsResult::Capabilities(caps),
                ..
            }) => {
                assert_eq!(operation_id, OperationId(100));
                assert!(caps.linux && caps.ps && caps.proc_net && caps.compose);
                assert!(!caps.systemd && !caps.ss && !caps.docker_daemon);
                received = true;
            }
            Some(FromCore::OperationFailed { error, .. }) => panic!("探测失败: {error:?}"),
            Some(FromCore::Render { snapshot, .. }) => {
                assert!(!snapshot.to_plain_text().contains("docker_daemon"))
            }
            _ => {}
        }
    }
    assert!(received);
    assert_eq!(
        state.lock().unwrap().exec_commands,
        vec![kt_core::remote_ops::PROBE_COMMAND.as_bytes()]
    );
    manager.send(ToCore::Disconnect { id });
}

#[test]
fn operations_cancel_open_read_and_sudo_emit_one_terminal_and_close_channel() {
    for mode in [
        OperationMode::Opening,
        OperationMode::Reading,
        OperationMode::Sudo,
    ] {
        exercise_operation_cancellation(mode, false);
    }
}

#[test]
fn operations_disconnect_converges_every_pending_request_before_closed() {
    exercise_operation_cancellation(OperationMode::Reading, true);
}

#[test]
fn operations_reconnect_cancels_old_generation_without_overwriting_new_result() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (old_port, old_state) = runtime.block_on(start_server_with_state(
        false,
        Arc::new(Mutex::new(EchoState {
            operation_mode: Some(OperationMode::Reading),
            ..Default::default()
        })),
    ));
    let (new_port, _) = runtime.block_on(start_server_with_state(
        false,
        Arc::new(Mutex::new(EchoState::default())),
    ));
    let (mut manager, id) = operations_test_manager(old_port);
    manager.send(ToCore::Operations {
        id,
        operation_id: OperationId(300),
        request: OperationsRequest::Refresh(OperationsDomain::Processes),
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while old_state.lock().unwrap().exec_commands.is_empty() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut params = ConnectParams::new("127.0.0.1", "tester");
    params.port = new_port;
    params.auth = vec![AuthMethod::Password];
    manager.send(ToCore::Connect {
        id,
        params: Box::new(params),
        pty: PtySize::default(),
    });
    let mut cancelled = 0;
    let mut connected = false;
    while std::time::Instant::now() < deadline && !connected {
        match recv_timeout(&mut manager, Duration::from_millis(50)) {
            Some(FromCore::OperationFailed {
                operation_id: OperationId(300),
                error,
                ..
            }) => {
                assert_eq!(error.kind, kt_core::OperationsErrorKind::Cancelled);
                cancelled += 1;
            }
            Some(FromCore::Connected { .. }) => connected = true,
            Some(FromCore::Closed { .. } | FromCore::OperationResult { .. }) => {
                panic!("旧代次事件不得污染新会话")
            }
            _ => {}
        }
    }
    assert!(connected);
    assert_eq!(cancelled, 1);
    manager.send(ToCore::Operations {
        id,
        operation_id: OperationId(301),
        request: OperationsRequest::Refresh(OperationsDomain::Processes),
    });
    // 迟到的旧取消命令不得取消新连接中的不同请求。
    manager.send(ToCore::CancelOperation {
        id,
        operation_id: OperationId(300),
    });
    let mut completed = false;
    while std::time::Instant::now() < deadline && !completed {
        match recv_timeout(&mut manager, Duration::from_millis(50)) {
            Some(FromCore::OperationResult {
                operation_id: OperationId(301),
                ..
            }) => completed = true,
            Some(FromCore::OperationFailed { .. } | FromCore::Closed { .. }) => {
                panic!("新请求被旧会话终态覆盖")
            }
            _ => {}
        }
    }
    assert!(completed);
    manager.send(ToCore::CancelOperation {
        id,
        operation_id: OperationId(301),
    });
    let deadline = std::time::Instant::now() + Duration::from_millis(200);
    while std::time::Instant::now() < deadline {
        assert!(
            !matches!(
                recv_timeout(&mut manager, Duration::from_millis(20)),
                Some(FromCore::OperationResult { .. } | FromCore::OperationFailed { .. })
            ),
            "完成后取消不得产生第二个终态"
        );
    }
    manager.send(ToCore::Disconnect { id });
}

fn exercise_operation_cancellation(mode: OperationMode, disconnect: bool) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (port, state) = runtime.block_on(start_server_with_state(
        false,
        Arc::new(Mutex::new(EchoState {
            operation_mode: Some(mode),
            ..Default::default()
        })),
    ));
    let (mut manager, id) = operations_test_manager(port);
    let count = if disconnect { 3 } else { 1 };
    for index in 0..count {
        let request = if matches!(mode, OperationMode::Sudo) {
            OperationsRequest::ServiceAction {
                name: "test.service".into(),
                action: kt_core::ServiceAction::Restart,
            }
        } else {
            OperationsRequest::Refresh(OperationsDomain::Processes)
        };
        manager.send(ToCore::Operations {
            id,
            operation_id: OperationId(200 + index),
            request,
        });
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while state.lock().unwrap().exec_commands.len() < count as usize {
        assert!(std::time::Instant::now() < deadline, "请求未到达远端");
        std::thread::sleep(Duration::from_millis(10));
    }
    if matches!(mode, OperationMode::Sudo) {
        let mut challenged = false;
        while std::time::Instant::now() < deadline && !challenged {
            challenged = matches!(
                recv_timeout(&mut manager, Duration::from_millis(50)),
                Some(FromCore::AuthChallenge {
                    challenge: kt_core::AuthChallenge::Sudo { .. },
                    ..
                })
            );
        }
        assert!(challenged, "未进入 sudo 等待");
    }
    if disconnect {
        manager.send(ToCore::Disconnect { id });
    } else {
        manager.send(ToCore::CancelOperation {
            id,
            operation_id: OperationId(200),
        });
        manager.send(ToCore::CancelOperation {
            id,
            operation_id: OperationId(200),
        });
    }
    let mut terminal = std::collections::HashSet::new();
    let mut closed = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        match recv_timeout(&mut manager, Duration::from_millis(50)) {
            Some(FromCore::OperationFailed {
                operation_id,
                error,
                ..
            }) => {
                assert_eq!(error.kind, kt_core::OperationsErrorKind::Cancelled);
                assert!(terminal.insert(operation_id), "重复终态");
            }
            Some(FromCore::OperationResult { .. }) => panic!("取消后不应有成功结果"),
            Some(FromCore::Closed { .. }) => {
                assert_eq!(terminal.len(), count as usize, "Closed 前必须收敛所有请求");
                closed = true;
            }
            _ => {}
        }
    }
    assert_eq!(terminal.len(), count as usize);
    if disconnect {
        assert!(closed);
    } else {
        assert!(
            !state.lock().unwrap().channel_closes.is_empty(),
            "取消必须关闭远端通道"
        );
        manager.send(ToCore::Disconnect { id });
    }
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
    let mut ready_snapshot = None;
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
                if saw_ready {
                    ready_snapshot = Some(*snapshot);
                }
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
    verify_buffer_copy(
        &mgr,
        id,
        Some(exec_id),
        ready_snapshot.as_ref().unwrap(),
        "container-ready",
    );
    assert!(selection_request(
        &mgr,
        id,
        Some(kt_core::ExecId(9999)),
        "test-pane",
        kt_core::term::selection::SelectionAction::Copy
    )
    .is_err());
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

#[test]
fn monitor_latency_tracks_remote_response_and_recovers_after_timeout() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (port, state) = runtime.block_on(start_server_with_state(
        false,
        Arc::new(Mutex::new(EchoState {
            monitor: Some(MonitorScenario {
                delays: [
                    Some(Duration::from_millis(80)),
                    None,
                    Some(Duration::from_millis(250)),
                    Some(Duration::ZERO),
                ]
                .into(),
                sample_delay: Duration::from_millis(700),
                ..Default::default()
            }),
            ..Default::default()
        })),
    ));
    let (mut manager, id) = operations_test_manager(port);
    manager.send(ToCore::StartMonitor { id });
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    let mut latencies = Vec::new();
    while latencies.len() < 4 && std::time::Instant::now() < deadline {
        match recv_timeout(&mut manager, Duration::from_millis(100)) {
            Some(FromCore::Monitor { stats, .. }) => latencies.push(stats.latency_ms),
            Some(FromCore::MonitorError { message, .. }) => panic!("监控失败: {message}"),
            _ => {}
        }
    }
    assert_eq!(latencies.len(), 4, "未收到全部采样: {latencies:?}");
    assert!((80..700).contains(&latencies[0]), "{latencies:?}");
    assert_eq!(latencies[1], 0, "超时应为未知");
    assert!(
        (250..700).contains(&latencies[2]),
        "旧响应不能满足新心跳: {latencies:?}"
    );
    assert!((1..700).contains(&latencies[3]), "{latencies:?}");
    assert_eq!(
        state.lock().unwrap().connections,
        1,
        "监控不得新建 TCP 连接"
    );
    manager.send(ToCore::Disconnect { id });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(FromCore::Closed {
            id: closed_id,
            error,
        }) = recv_timeout(&mut manager, Duration::from_millis(100))
        {
            assert_eq!(closed_id, id);
            assert!(error.is_none(), "{error:?}");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "会话未关闭");
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        assert!(
            !matches!(
                recv_timeout(&mut manager, Duration::from_millis(100)),
                Some(FromCore::Monitor { .. })
            ),
            "关闭后不应继续采样"
        );
    }
}

#[path = "roundtrip/terminal_render.rs"]
mod terminal_render;
