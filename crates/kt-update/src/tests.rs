//! 使用本地 HTTP 和隔离安装目录验证更新链路。
use super::*;
use crate::metadata::{Manifest, Package, PackageKind, TargetPackages};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Instant,
};

struct Reply {
    path: &'static str,
    status: &'static str,
    headers: String,
    body: Vec<u8>,
    delay: Duration,
    slow: bool,
    started: Option<std::sync::mpsc::Sender<()>>,
}
impl Reply {
    fn ok(path: &'static str, body: Vec<u8>) -> Self {
        Self {
            path,
            status: "200 OK",
            headers: String::new(),
            body,
            delay: Duration::ZERO,
            slow: false,
            started: None,
        }
    }
}
fn server(replies: Vec<Reply>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        for reply in replies {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                assert!(Instant::now() < deadline, "missing request: {}", reply.path);
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                // macOS 会继承 listener 的非阻塞模式，恢复阻塞后读取超时才能生效。
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_millis(250)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut buffer = [0; 1024];
                    match stream.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => request.extend_from_slice(&buffer[..n]),
                        Err(e)
                            if request.is_empty()
                                && matches!(
                                    e.kind(),
                                    std::io::ErrorKind::WouldBlock
                                        | std::io::ErrorKind::TimedOut
                                        | std::io::ErrorKind::ConnectionReset
                                ) =>
                        {
                            break
                        }
                        Err(e) => panic!("read update request: {e}"),
                    }
                    assert!(request.len() <= 4096, "request headers too large");
                }
                if request.is_empty() {
                    continue;
                }
                assert!(request.ends_with(b"\r\n\r\n"), "incomplete request headers");
                assert_eq!(
                    String::from_utf8_lossy(&request).split_whitespace().nth(1),
                    Some(reply.path),
                    "unexpected update request"
                );
                break stream;
            };
            if let Some(started) = reply.started {
                let _ = started.send(());
            }
            thread::sleep(reply.delay);
            let header = format!(
                "HTTP/1.1 {}\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n",
                reply.status,
                reply.body.len(),
                reply.headers
            );
            if stream.write_all(header.as_bytes()).is_err() {
                continue;
            }
            if reply.slow {
                for byte in reply.body {
                    if stream.write_all(&[byte]).is_err() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(30));
                }
            } else {
                let _ = stream.write_all(&reply.body);
            }
        }
    });
    (base, handle)
}
fn identity(channel: Channel) -> BuildInfo {
    BuildInfo {
        channel,
        version: "0.1.2".into(),
        commit: "a".repeat(40),
        build: 1,
        attempt: 1,
        target: "x86_64-unknown-linux-gnu".into(),
    }
}
fn release_replies(channel: Channel, body: &[u8]) -> (Vec<Reply>, Package) {
    let package = Package {
        name: "kitonyterms-v0.1.3-linux-x64.tar.gz".into(),
        size: body.len() as u64,
        sha256: hex::encode(Sha256::digest(body)),
        kind: PackageKind::TarGz,
    };
    let manifest = Manifest {
        schema: 1,
        channel,
        version: "0.1.3".into(),
        commit: "b".repeat(40),
        build: 2,
        attempt: 1,
        targets: BTreeMap::from([(
            "x86_64-unknown-linux-gnu".into(),
            TargetPackages {
                archive: package.clone(),
                installer: None,
            },
        )]),
    };
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let release = serde_json::json!({ "id": 123, "tag_name": if channel == Channel::Alpha { "alpha" } else { "v0.1.3" }, "draft": false, "prerelease": channel == Channel::Alpha, "assets": [
        { "id":1, "name":"update-manifest.json", "size":bytes.len(), "digest":format!("sha256:{}", hex::encode(Sha256::digest(&bytes))) },
        { "id":2, "name":package.name, "size":package.size, "digest":format!("sha256:{}", package.sha256) }
    ] });
    (
        vec![
            Reply::ok(
                if channel == Channel::Alpha {
                    "/releases/tags/alpha"
                } else {
                    "/releases/latest"
                },
                serde_json::to_vec(&release).unwrap(),
            ),
            Reply::ok("/releases/assets/1", bytes),
        ],
        package,
    )
}
fn service(
    dir: &std::path::Path,
    base: String,
    channel: Channel,
    timeout: Duration,
) -> Arc<UpdateService> {
    let exe = dir.join("kitonyterms");
    std::fs::write(&exe, b"old").unwrap();
    let current = identity(channel);
    let service = UpdateService::open_with(
        dir.join("updates"),
        Installation::from_executable(exe).unwrap(),
        current.clone(),
        ReleaseClient::testing(base, timeout),
        Preferences::defaults(&current),
        None,
    )
    .unwrap();
    service
        .preferences(Preferences {
            auto_check: false,
            auto_download: false,
            proxy: UpdateProxy::System,
        })
        .unwrap();
    service
}
fn wait(service: &UpdateService, expected: Phase) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let snapshot = service.snapshot();
        if snapshot.phase == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected {expected:?}; actual {snapshot:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn checks_downloads_once_and_binds_asset_id() {
    let dir = tempfile::tempdir().unwrap();
    let body = b"verified download";
    let (mut replies, package) = release_replies(Channel::Stable, body);
    replies[0].delay = Duration::from_millis(50);
    replies.push(Reply::ok("/releases/assets/2", body.to_vec()));
    let (base, server) = server(replies);
    let service = service(dir.path(), base, Channel::Stable, Duration::from_secs(2));
    service.check().unwrap();
    assert!(service.check().is_err());
    wait(&service, Phase::Available);
    service.download().unwrap();
    assert!(service.download().is_err());
    wait(&service, Phase::Ready);
    download::verify(&install::package_path(&service.root, &package), &package).unwrap();
    assert_eq!(service.snapshot().available.unwrap().release_id, 123);
    assert!(service.check().is_err());
    server.join().unwrap();
}
#[test]
fn download_rejects_digest_size_and_disappearing_alpha() {
    for case in ["digest", "size", "alpha-disappears"] {
        let dir = tempfile::tempdir().unwrap();
        let channel = if case == "alpha-disappears" {
            Channel::Alpha
        } else {
            Channel::Stable
        };
        let (mut replies, package) = release_replies(channel, b"correct");
        let mut reply = Reply::ok(
            "/releases/assets/2",
            if case == "digest" {
                b"invalid".to_vec()
            } else {
                b"short".to_vec()
            },
        );
        if case == "alpha-disappears" {
            reply.status = "404 Not Found";
        }
        replies.push(reply);
        let (base, server) = server(replies);
        let service = service(dir.path(), base, channel, Duration::from_secs(2));
        service.check().unwrap();
        wait(&service, Phase::Available);
        service.download().unwrap();
        wait(&service, Phase::Error);
        assert!(!install::package_path(&service.root, &package).exists());
        if case == "alpha-disappears" {
            assert!(
                service.snapshot().available.is_none(),
                "{:?}",
                service.snapshot()
            );
        }
        server.join().unwrap();
    }
}
#[test]
fn cancellation_removes_partial_download_and_allows_retry() {
    let dir = tempfile::tempdir().unwrap();
    let body = [42; 64];
    let (mut replies, package) = release_replies(Channel::Stable, &body);
    let mut reply = Reply::ok("/releases/assets/2", body.to_vec());
    reply.slow = true;
    replies.push(reply);
    replies.push(Reply::ok("/releases/assets/2", body.to_vec()));
    let (base, server) = server(replies);
    let service = service(dir.path(), base, Channel::Stable, Duration::from_secs(2));
    service.check().unwrap();
    wait(&service, Phase::Available);
    service.download().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while service.snapshot().received == 0 {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    service.cancel();
    wait(&service, Phase::Available);
    assert!(!install::package_path(&service.root, &package).exists());
    assert!(std::fs::read_dir(&service.root).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".tmp")));
    service.download().unwrap();
    wait(&service, Phase::Ready);
    download::verify(&install::package_path(&service.root, &package), &package).unwrap();
    server.join().unwrap();
}
#[test]
fn timeouts_and_rate_limits_remain_visible() {
    for limited in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut reply = Reply::ok("/releases/latest", Vec::new());
        if limited {
            reply.status = "429 Too Many Requests";
            reply.headers = "Retry-After: 120\r\n".into();
        } else {
            reply.delay = Duration::from_millis(150);
        }
        let (base, server) = server(vec![reply]);
        let timeout = if limited {
            Duration::from_secs(2)
        } else {
            Duration::from_millis(100)
        };
        let service = service(dir.path(), base, Channel::Stable, timeout);
        service.check().unwrap();
        wait(&service, Phase::Error);
        assert!(service.snapshot().message.is_some());
        if limited {
            assert!(service.check().is_err());
        }
        server.join().unwrap();
    }
}
#[test]
fn valid_package_commits_and_invalid_package_preserves_original() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("updates");
    install::private_root(&root).unwrap();
    let executable = dir.path().join("kitonyterms");
    fs::write(&executable, b"old").unwrap();
    let mut binary = [0; 64];
    binary[..6].copy_from_slice(b"\x7fELF\x02\x01");
    binary[18] = 62;
    let archive_path = dir.path().join("fixture.tar.gz");
    let encoder = flate2::write::GzEncoder::new(
        fs::File::create(&archive_path).unwrap(),
        flate2::Compression::default(),
    );
    let mut tar = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_size(binary.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    tar.append_data(
        &mut header,
        "kitonyterms-v0.1.3-linux-x64/kitonyterms",
        &binary[..],
    )
    .unwrap();
    tar.into_inner().unwrap().finish().unwrap();
    let package = Package {
        name: "kitonyterms-v0.1.3-linux-x64.tar.gz".into(),
        size: fs::metadata(&archive_path).unwrap().len(),
        sha256: download::sha256(&archive_path).unwrap(),
        kind: PackageKind::TarGz,
    };
    fs::copy(&archive_path, install::package_path(&root, &package)).unwrap();
    let current = identity(Channel::Stable);
    let mut next = current.clone();
    next.version = "0.1.3".into();
    let task = install::InstallTask {
        installation: Installation::from_executable(executable.clone()).unwrap(),
        current: current.clone(),
        next: next.clone(),
        source_sha256: download::sha256(&executable).unwrap(),
        package,
        parent_pid: 1,
        elevate: false,
    };
    install::apply(&root, &task).unwrap();
    assert_eq!(fs::read(&executable).unwrap(), binary);
    assert_eq!(fs::read(task.installation.backup()).unwrap(), b"old");
    install::recover(&root, &next, &executable, true).unwrap();
    fs::write(&executable, b"old").unwrap();
    fs::write(install::package_path(&root, &task.package), b"corrupt").unwrap();
    assert!(install::apply(&root, &task).is_err());
    assert_eq!(fs::read(&executable).unwrap(), b"old");
    assert!(!root.join("journal.json").exists());
}

#[test]
fn release_notes_metadata_is_optional_and_utf8_bounded() {
    for metadata in [
        serde_json::json!({}),
        serde_json::json!({ "body": null, "published_at": null }),
        serde_json::json!({ "body": "", "published_at": "invalid" }),
        serde_json::json!({ "body": "## 改进\n\n- 修复终端", "published_at": "2026-10-16T01:00:00Z" }),
        serde_json::json!({ "body": "更新".repeat(20_000), "published_at": null }),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (mut replies, _) = release_replies(Channel::Stable, b"test");
        let mut release: serde_json::Value = serde_json::from_slice(&replies[0].body).unwrap();
        for (key, value) in metadata.as_object().unwrap() {
            release[key] = value.clone();
        }
        replies[0].body = serde_json::to_vec(&release).unwrap();
        let (base, server) = server(replies);
        let service = service(dir.path(), base, Channel::Stable, Duration::from_secs(2));
        service.check().unwrap();
        wait(&service, Phase::Available);
        let available = service.snapshot().available.unwrap();
        let original = metadata["body"].as_str().unwrap_or_default();
        assert_eq!(
            available.notes_truncated,
            original.len() > client::MAX_RELEASE_NOTES
        );
        assert!(available.notes.len() <= client::MAX_RELEASE_NOTES);
        assert!(original.starts_with(&available.notes));
        assert_eq!(available.tag, "v0.1.3");
        assert_eq!(
            available.published_at.as_deref(),
            metadata["published_at"].as_str()
        );
        assert!(service.snapshot().last_check.is_some());
        server.join().unwrap();
    }
}

#[test]
fn cancelled_check_is_neutral_and_can_be_retried() {
    let dir = tempfile::tempdir().unwrap();
    let (started, received) = std::sync::mpsc::channel();
    let mut first = Reply::ok("/releases/latest", b"{}".to_vec());
    first.started = Some(started);
    first.delay = Duration::from_millis(100);
    let (next, _) = release_replies(Channel::Stable, b"test");
    let mut replies = vec![first];
    replies.extend(next);
    let (base, server) = server(replies);
    let service = service(dir.path(), base, Channel::Stable, Duration::from_secs(2));
    {
        let mut state = service.state.lock().unwrap();
        state.snapshot.last_check = Some(123);
        state.failures = 2;
    }
    service.check().unwrap();
    received.recv_timeout(Duration::from_secs(2)).unwrap();
    service.cancel();
    wait(&service, Phase::Cancelled);
    {
        let state = service.state.lock().unwrap();
        assert_eq!(state.failures, 2);
        assert_eq!(state.snapshot.last_check, Some(123));
        assert!(state.snapshot.message.is_none());
        assert!(state.snapshot.available.is_none());
        assert!(state.next_check >= client::now() + 5 * 3600);
    }
    service.check().unwrap();
    wait(&service, Phase::Available);
    server.join().unwrap();
}

/// 读取 HTTP 请求头（读到空行为止），供代理与隧道服务共用。
fn read_http_head(stream: &mut std::net::TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    while !request.ends_with(b"\r\n\r\n") {
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0, "请求在头部结束前被关闭");
        request.extend_from_slice(&buffer[..read]);
        assert!(request.len() <= 8192, "请求头过大");
    }
    request
}

fn write_http_response(stream: &mut std::net::TcpStream, status: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
}

fn address_parts(address: &str) -> (String, u16) {
    let (host, port) = address.rsplit_once(':').unwrap();
    (host.to_string(), port.parse().unwrap())
}

/// 预设的 Release 响应（latest + 清单），供代理服务按序返回。
fn proxied_replies(channel: Channel, body: &[u8]) -> Vec<(&'static str, Vec<u8>)> {
    let (releases, _) = release_replies(channel, body);
    releases
        .into_iter()
        .map(|reply| (reply.path, reply.body))
        .collect()
}

/// 本地 HTTP 代理：断言收到绝对形式的请求行（`GET http://host/path`）——这正是
/// 「请求经代理转发」的判据——再按序返回预设响应。
fn http_proxy_server(replies: Vec<(&'static str, Vec<u8>)>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let handle = thread::spawn(move || {
        for (path, body) in replies {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let request = read_http_head(&mut stream);
            let target = String::from_utf8_lossy(&request)
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            assert!(
                target.starts_with("http://") && target.ends_with(path),
                "请求未经代理转发 / request did not go through the proxy: {target}"
            );
            write_http_response(&mut stream, "200 OK", &body);
        }
    });
    (address, handle)
}

/// 只回一个状态码的代理：用于 407 之类的异常状态。
fn failing_proxy_server(status: &'static str) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let _ = read_http_head(&mut stream);
        write_http_response(&mut stream, status, b"");
    });
    (address, handle)
}

/// 最小 SOCKS5 服务：断言 user/pass 认证（RFC 1929）与「域名交给代理解析」的
/// `socks5h` 语义，然后在同一连接上返回预设 HTTP 响应。
fn socks5_server(
    username: Option<&'static str>,
    domain: &'static str,
    replies: Vec<(&'static str, Vec<u8>)>,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let handle = thread::spawn(move || {
        for (path, body) in replies {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();

            let mut greeting = [0u8; 2];
            stream.read_exact(&mut greeting).unwrap();
            assert_eq!(greeting[0], 5, "SOCKS 版本必须为 5");
            let mut methods = vec![0u8; greeting[1] as usize];
            stream.read_exact(&mut methods).unwrap();
            match username {
                Some(expected) => {
                    assert!(
                        methods.contains(&2),
                        "客户端未提供 user/pass 认证: {methods:?}"
                    );
                    stream.write_all(&[5, 2]).unwrap();
                    let mut head = [0u8; 2];
                    stream.read_exact(&mut head).unwrap();
                    assert_eq!(head[0], 1, "认证子协商版本必须为 1");
                    let mut user = vec![0u8; head[1] as usize];
                    stream.read_exact(&mut user).unwrap();
                    let mut password_len = [0u8; 1];
                    stream.read_exact(&mut password_len).unwrap();
                    let mut password = vec![0u8; password_len[0] as usize];
                    stream.read_exact(&mut password).unwrap();
                    assert_eq!(String::from_utf8_lossy(&user), expected);
                    assert!(password.is_empty(), "显式代理按约定不保存密码");
                    stream.write_all(&[1, 0]).unwrap();
                }
                None => stream.write_all(&[5, 0]).unwrap(),
            }

            let mut request = [0u8; 4];
            stream.read_exact(&mut request).unwrap();
            assert_eq!(
                (request[0], request[1], request[3]),
                (5, 1, 3),
                "必须是以域名为地址类型的 CONNECT（socks5h）"
            );
            let mut length = [0u8; 1];
            stream.read_exact(&mut length).unwrap();
            let mut host = vec![0u8; length[0] as usize];
            stream.read_exact(&mut host).unwrap();
            let mut port = [0u8; 2];
            stream.read_exact(&mut port).unwrap();
            assert_eq!(String::from_utf8_lossy(&host), domain);
            assert_eq!(u16::from_be_bytes(port), 80);
            stream.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();

            // 隧道已指向目标，隧道内的请求是 origin-form。
            let request = read_http_head(&mut stream);
            let target = String::from_utf8_lossy(&request)
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            assert_eq!(target, path, "隧道内必须是 origin-form 请求");
            write_http_response(&mut stream, "200 OK", &body);
        }
    });
    (address, handle)
}

#[tokio::test]
async fn explicit_http_proxy_routes_the_release_check_through_the_proxy() {
    let (address, server) = http_proxy_server(proxied_replies(Channel::Stable, b"proxied"));
    let (host, port) = address_parts(&address);
    // 目标域名不可解析：只有真的经代理转发，检查才可能成功。
    let client = ReleaseClient::testing_with_proxy(
        "http://update.invalid".into(),
        &UpdateProxy::Http {
            host,
            port,
            username: None,
        },
        Duration::from_secs(3),
    )
    .unwrap();

    let available = client
        .check(&identity(Channel::Stable), false)
        .await
        .unwrap();

    assert!(available.is_some());
    server.join().unwrap();
}

#[tokio::test]
async fn socks5_proxy_tunnels_the_check_and_sends_the_configured_username() {
    let (address, server) = socks5_server(
        Some("me"),
        "update.invalid",
        proxied_replies(Channel::Stable, b"proxied"),
    );
    let (host, port) = address_parts(&address);
    let client = ReleaseClient::testing_with_proxy(
        "http://update.invalid".into(),
        &UpdateProxy::Socks5 {
            host,
            port,
            username: Some("me".into()),
        },
        Duration::from_secs(3),
    )
    .unwrap();

    let available = client
        .check(&identity(Channel::Stable), false)
        .await
        .unwrap();

    assert!(available.is_some());
    server.join().unwrap();
}

#[tokio::test]
async fn proxy_authentication_required_is_reported_as_a_proxy_error() {
    let (address, server) = failing_proxy_server("407 Proxy Authentication Required");
    let (host, port) = address_parts(&address);
    let proxy = UpdateProxy::Http {
        host,
        port,
        username: Some("me".into()),
    };
    let client = ReleaseClient::testing_with_proxy(
        "http://update.invalid".into(),
        &proxy,
        Duration::from_secs(3),
    )
    .unwrap();

    let error = client
        .check(&identity(Channel::Stable), false)
        .await
        .unwrap_err();

    let message = describe_failure(&error, &proxy);
    assert!(message.contains("代理认证失败"), "{message}");
    server.join().unwrap();
}

#[tokio::test]
async fn silent_proxy_times_out_and_points_at_the_proxy_settings() {
    // 代理接受连接但从不响应：读超时必须归类为超时并提示检查代理。
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let handle = thread::spawn(move || {
        let (_stream, _) = listener.accept().unwrap();
        thread::sleep(Duration::from_millis(1500));
    });
    let (host, port) = address_parts(&address);
    let proxy = UpdateProxy::Http {
        host,
        port,
        username: None,
    };
    let client = ReleaseClient::testing_with_proxy(
        "http://update.invalid".into(),
        &proxy,
        Duration::from_millis(300),
    )
    .unwrap();

    let error = client
        .check(&identity(Channel::Stable), false)
        .await
        .unwrap_err();

    assert!(error
        .downcast_ref::<reqwest::Error>()
        .is_some_and(|error| error.is_timeout()));
    let message = describe_failure(&error, &proxy);
    assert!(message.contains("请求超时"), "{message}");
    assert!(message.contains("代理"), "{message}");
    handle.join().unwrap();
}

#[test]
fn invalid_proxy_is_rejected_without_persisting() {
    let dir = tempfile::tempdir().unwrap();
    let (base, server) = server(vec![]);
    let service = service(dir.path(), base, Channel::Stable, Duration::from_secs(2));
    let before = service.snapshot().preferences;
    let invalid = UpdateProxy::Http {
        host: String::new(),
        port: 0,
        username: None,
    };

    assert!(service
        .preferences(Preferences {
            proxy: invalid,
            ..before.clone()
        })
        .is_err());

    assert_eq!(service.snapshot().preferences, before);
    let stored: Preferences = install::read_json(&service.root.join("preferences.json")).unwrap();
    assert_eq!(stored.proxy, UpdateProxy::System);
    server.join().unwrap();
}

#[test]
fn changing_the_proxy_cancels_the_running_check_and_drops_its_result() {
    let dir = tempfile::tempdir().unwrap();
    let (started, received) = std::sync::mpsc::channel();
    let mut first = Reply::ok("/releases/latest", b"{}".to_vec());
    first.started = Some(started);
    first.delay = Duration::from_millis(300);
    let (base, server) = server(vec![first]);
    let service = service(dir.path(), base, Channel::Stable, Duration::from_secs(2));

    service.check().unwrap();
    received.recv_timeout(Duration::from_secs(2)).unwrap();
    let proxy = UpdateProxy::Direct;
    service
        .preferences(Preferences {
            auto_check: false,
            auto_download: false,
            proxy: proxy.clone(),
        })
        .unwrap();

    // 改代理立即取消在途检查：状态回到 Idle，而不是停在 Checking。
    assert_eq!(service.snapshot().phase, Phase::Idle);
    assert!(service.snapshot().available.is_none());
    assert_eq!(service.snapshot().preferences.proxy, proxy);

    // 过期检查的结果不得写回状态。
    thread::sleep(Duration::from_millis(500));
    assert_eq!(service.snapshot().phase, Phase::Idle);
    assert!(service.snapshot().available.is_none());

    // 新代理已落盘，重启后仍然生效。
    let stored: Preferences = install::read_json(&service.root.join("preferences.json")).unwrap();
    assert_eq!(stored.proxy, proxy);
    server.join().unwrap();
}

#[test]
fn stored_invalid_proxy_falls_back_to_system_with_a_visible_notice() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("kitonyterms");
    std::fs::write(&exe, b"old").unwrap();
    let root = dir.path().join("updates");
    std::fs::create_dir_all(&root).unwrap();
    install::atomic_json(
        &root.join("preferences.json"),
        &Preferences {
            auto_check: true,
            auto_download: false,
            proxy: UpdateProxy::Http {
                host: "127.0.0.1/24".into(),
                port: 8080,
                username: None,
            },
        },
    )
    .unwrap();

    let service = UpdateService::open(root, Installation::from_executable(exe).unwrap()).unwrap();

    let snapshot = service.snapshot();
    assert_eq!(snapshot.preferences.proxy, UpdateProxy::System);
    assert!(snapshot.preferences.auto_check);
    let message = snapshot.message.expect("回退原因必须可见");
    assert!(message.contains("代理设置无效"), "{message}");
}
