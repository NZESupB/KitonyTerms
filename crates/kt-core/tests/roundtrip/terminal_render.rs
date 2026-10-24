//! 通过实际 SSH 数据流验证同步更新、输出合帧、尾帧和选区复制。
use super::*;

pub(super) fn scripted_output(
    channel: ChannelId,
    data: &[u8],
    session: &mut ServerSession,
) -> Result<bool, russh::Error> {
    match data {
        b"test-sync-complete" => {
            session.data(
                channel,
                bytes::Bytes::from_static(b"\x1b[?2026h\x1b[H\x1b[2JFRAME-"),
            )?;
            let handle = session.handle();
            tokio::spawn(async move {
                // 间隔超过普通合帧预算，只有同步协议能隐藏中间状态。
                tokio::time::sleep(Duration::from_millis(40)).await;
                let _ = handle
                    .data(channel, bytes::Bytes::from_static(b"DONE\x1b[?2026l"))
                    .await;
            });
        }
        b"test-sync-timeout" => {
            // 不再发送任何普通数据；超时必须自行唤醒并回写 DSR。
            session.data(
                channel,
                bytes::Bytes::from_static(b"\x1b[?2026h\x1b[H\x1b[2JTIMEOUT\x1b[6n"),
            )?;
        }
        b"test-tail-close" => {
            session.data(
                channel,
                bytes::Bytes::from_static(b"\x1b[?2026h\x1b[H\x1b[2JTAIL"),
            )?;
            session.eof(channel)?;
            session.close(channel)?;
        }
        b"test-render-burst" => {
            let handle = session.handle();
            tokio::spawn(async move {
                for n in 0..80 {
                    if handle
                        .data(channel, bytes::Bytes::from(format!("\rVALUE{n:03}")))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            });
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn send_input(manager: &SessionManager, id: SessionId, exec: Option<kt_core::ExecId>, data: &[u8]) {
    let command = match exec {
        Some(exec_id) => ToCore::ContainerInput {
            id,
            exec_id,
            data: data.to_vec(),
        },
        None => ToCore::Input {
            id,
            data: data.to_vec(),
        },
    };
    assert!(manager.send(command));
}

fn frame(event: FromCore, exec: Option<kt_core::ExecId>) -> Option<kt_core::GridSnapshot> {
    match event {
        FromCore::Render { snapshot, .. } if exec.is_none() => Some(*snapshot),
        FromCore::ExecRender {
            exec_id, snapshot, ..
        } if exec == Some(exec_id) => Some(*snapshot),
        _ => None,
    }
}

fn wait_frame(
    manager: &mut SessionManager,
    exec: Option<kt_core::ExecId>,
    mut accept: impl FnMut(&kt_core::GridSnapshot) -> bool,
) -> kt_core::GridSnapshot {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if let Some(snapshot) =
            recv_timeout(manager, Duration::from_millis(100)).and_then(|event| frame(event, exec))
        {
            if accept(&snapshot) {
                return snapshot;
            }
        }
    }
    panic!("未收到目标终端帧（exec={exec:?}）");
}

fn verify_render_pipeline(container: bool) {
    let server_rt = tokio::runtime::Runtime::new().unwrap();
    let port = server_rt.block_on(start_server(false));
    let mut manager =
        SessionManager::spawn(Arc::new(AcceptAllVerifier), Arc::new(FixedPasswordFactory)).unwrap();
    let id = SessionId(81);
    assert!(manager.send(ToCore::Connect {
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
        pty: PtySize { cols: 40, rows: 4 },
    }));
    wait_frame(&mut manager, None, |snapshot| {
        snapshot.to_plain_text().contains("DSR_OK")
    });
    let exec = container.then_some(kt_core::ExecId(82));
    if let Some(exec_id) = exec {
        assert!(manager.send(ToCore::OpenContainerTerminal {
            id,
            exec_id,
            container_id: "safe-container".into(),
            pty: PtySize { cols: 40, rows: 4 },
        }));
        wait_frame(&mut manager, exec, |snapshot| {
            snapshot.to_plain_text().contains("container-ready")
        });
    }

    send_input(&manager, id, exec, b"test-sync-complete");
    let complete = wait_frame(&mut manager, exec, |snapshot| {
        let text = snapshot.to_plain_text();
        assert!(!text.trim().is_empty(), "同步输出期间出现空白中间帧");
        assert!(
            !text.contains("FRAME-") || text.contains("FRAME-DONE"),
            "同步输出期间出现半帧"
        );
        text.contains("FRAME-DONE")
    });
    verify_buffer_copy(&manager, id, exec, &complete, "FRAME-DONE");

    send_input(&manager, id, exec, b"test-sync-timeout");
    let released = wait_frame(&mut manager, exec, |snapshot| {
        snapshot.to_plain_text().contains("TIMEOUTDSR_OK")
    });
    verify_buffer_copy(&manager, id, exec, &released, "TIMEOUT");

    send_input(&manager, id, exec, b"test-render-burst");
    let mut frames = 0;
    let last = wait_frame(&mut manager, exec, |snapshot| {
        if snapshot.to_plain_text().contains("VALUE") {
            frames += 1;
        }
        snapshot.to_plain_text().contains("VALUE079")
    });
    assert!(frames > 0 && frames < 80, "连续输出没有合帧：{frames}");
    verify_buffer_copy(&manager, id, exec, &last, "VALUE079");

    send_input(&manager, id, exec, b"test-tail-close");
    wait_frame(&mut manager, exec, |snapshot| {
        snapshot.to_plain_text().contains("TAIL")
    });
    assert!(manager.send(ToCore::Disconnect { id }));
}

#[test]
fn host_render_batches_sync_timeout_and_close() {
    verify_render_pipeline(false);
}

#[test]
fn container_render_batches_sync_timeout_and_close() {
    verify_render_pipeline(true);
}
