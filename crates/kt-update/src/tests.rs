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
    let service = UpdateService::open_with(
        dir.join("updates"),
        Installation::from_executable(exe).unwrap(),
        identity(channel),
        ReleaseClient::testing(base, timeout),
    )
    .unwrap();
    service
        .preferences(Preferences {
            auto_check: false,
            auto_download: false,
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
