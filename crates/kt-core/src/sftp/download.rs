//! 目录下载：先校验清单，再逐文件提交，失败保留已完成文件。
use super::*;
use std::collections::HashSet;

const MAX_ENTRIES: usize = 100_000;
const MAX_DEPTH: usize = 128;

pub(super) struct LocalTempGuard(pub PathBuf);
impl Drop for LocalTempGuard {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.0) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("清理下载临时文件 {} 失败：{error}", self.0.display());
            }
        }
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || "/\\:<>\"|?*".contains(c))
        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        return Err(format!("无法安全保存远端名称：{name}"));
    }
    Ok(())
}

struct Entry {
    remote: String,
    local: PathBuf,
    display: String,
    is_dir: bool,
}

/// 检查目标及相对根的所有现存父目录，不跟随本地链接。
fn check_target(path: &Path, root: &Path, is_dir: bool) -> Result<bool, String> {
    if !path.starts_with(root) {
        return Err("下载路径超出目标目录".into());
    }
    let mut current = Some(path);
    let mut exists = false;
    while let Some(p) = current {
        match std::fs::symlink_metadata(p) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(format!("下载目标包含符号链接：{}", p.display()));
                }
                let expected_dir = p != path || is_dir;
                if (expected_dir && !meta.is_dir()) || (!expected_dir && !meta.is_file()) {
                    return Err(format!("下载目标类型冲突：{}", p.display()));
                }
                if p == path {
                    exists = true;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("检查下载目标 {} 失败：{e}", p.display())),
        }
        if p == root {
            break;
        }
        current = p.parent();
    }
    Ok(exists)
}

pub(super) async fn download_tree(
    session: &SftpSession,
    id: SessionId,
    request_id: SftpRequestId,
    remote: &str,
    local: &Path,
    overwrite: bool,
    out: &SessionEventSender,
) -> Result<(), String> {
    let parent = local
        .parent()
        .ok_or("下载目标缺少父目录")?
        .canonicalize()
        .map_err(|e| format!("读取下载父目录失败：{e}"))?;
    let name = local.file_name().ok_or("下载目标缺少文件名")?;
    let root = parent.join(name);
    let mut pending = vec![(remote.to_owned(), root.clone(), basename(remote), 0usize)];
    let mut entries = Vec::new();
    let mut destinations = HashSet::new();
    let mut confirmed = HashSet::new();
    while let Some((remote, local, display, depth)) = pending.pop() {
        if depth > MAX_DEPTH || entries.len() + pending.len() >= MAX_ENTRIES {
            return Err("下载目录超过深度或条目上限".into());
        }
        let meta = session
            .symlink_metadata(remote.clone())
            .await
            .map_err(|e| format!("读取远端属性 {remote} 失败：{e}"))?;
        if meta.is_symlink() || (!meta.is_dir() && !meta.file_type().is_file()) {
            return Err(format!("不支持下载符号链接或特殊文件：{remote}"));
        }
        // 同时拒绝大小写折叠冲突，避免跨平台静默覆盖。
        if !destinations.insert(local.to_string_lossy().to_lowercase()) {
            return Err(format!("下载名称冲突：{}", local.display()));
        }
        if check_target(&local, &root, meta.is_dir())? {
            if !overwrite {
                return Err(format!("目标已存在，未确认覆盖：{}", local.display()));
            }
            confirmed.insert(local.clone());
        }
        if meta.is_dir() {
            let mut children = session
                .read_dir(remote.clone())
                .await
                .map_err(|e| format!("读取远端目录 {remote} 失败：{e}"))?
                .collect::<Vec<_>>();
            children.sort_by_key(|e| e.file_name());
            for child in children.into_iter().rev() {
                let name = child.file_name();
                if name == "." || name == ".." {
                    continue;
                }
                validate_name(&name)?;
                pending.push((
                    format!("{}/{name}", remote.trim_end_matches('/')),
                    local.join(&name),
                    format!("{display}/{name}"),
                    depth + 1,
                ));
                if entries.len() + pending.len() >= MAX_ENTRIES {
                    return Err("下载目录超过条目上限".into());
                }
            }
        }
        entries.push(Entry {
            remote,
            local,
            display,
            is_dir: meta.is_dir(),
        });
    }
    for entry in entries {
        let exists = check_target(&entry.local, &root, entry.is_dir)?;
        if exists && !confirmed.contains(&entry.local) {
            return Err(format!(
                "下载期间出现新的目标冲突：{}",
                entry.local.display()
            ));
        }
        // 传输前再次拒绝被替换成链接的远端条目。
        let meta = session
            .symlink_metadata(entry.remote.clone())
            .await
            .map_err(|e| e.to_string())?;
        if meta.is_symlink()
            || meta.is_dir() != entry.is_dir
            || (!entry.is_dir && !meta.file_type().is_file())
        {
            return Err(format!("远端条目类型已改变：{}", entry.remote));
        }
        if entry.is_dir {
            if !exists {
                tokio::fs::create_dir(&entry.local)
                    .await
                    .map_err(|e| format!("创建目录 {} 失败：{e}", entry.local.display()))?;
            }
            continue;
        }
        let mut src = session
            .open(entry.remote.clone())
            .await
            .map_err(|e| format!("打开 {} 失败：{e}", entry.remote))?;
        let (temp, dst) = create_private_download_temp(&entry.local)
            .await
            .map_err(|e| e.to_string())?;
        let guard = LocalTempGuard(temp.clone());
        let mut dst = dst;
        copy_with_progress(
            &mut src,
            &mut dst,
            id,
            request_id,
            &entry.display,
            meta.size.unwrap_or(0),
            out,
        )
        .await?;
        dst.flush().await.map_err(|e| e.to_string())?;
        dst.sync_all().await.map_err(|e| e.to_string())?;
        drop(dst);
        check_target(&entry.local, &root, false)?;
        commit_local(&temp, &entry.local, confirmed.contains(&entry.local))?;
        drop(guard);
    }
    Ok(())
}

fn commit_local(temp: &Path, target: &Path, overwrite: bool) -> Result<(), String> {
    if !overwrite {
        // hard_link 的创建语义保证并发出现的目标不会被覆盖。
        std::fs::hard_link(temp, target)
            .map_err(|e| format!("提交 {} 失败：{e}", target.display()))?;
        return Ok(());
    }
    std::fs::rename(temp, target)
        .map_err(|e| format!("提交 {} 失败，原文件保持不变：{e}", target.display()))
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn local_symlink_cannot_escape_download_root() {
        let base = std::env::temp_dir().join(format!("kt-download-links-{}", unique_temp_suffix()));
        let root = base.join("root");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        assert!(check_target(&root.join("link/file"), &root, false).is_err());
        assert!(check_target(&outside, &root, true).is_err());
        std::fs::remove_dir_all(base).unwrap();
    }
    use super::*;
    #[test]
    fn unsafe_names_are_rejected() {
        for name in ["..", "../a", "/a", "C:a", "a\\b", "a\0b", "CON", "a."] {
            assert!(validate_name(name).is_err(), "{name}");
        }
        assert!(validate_name("中文 文件.txt").is_ok());
    }
    #[test]
    fn new_target_cannot_overwrite_and_guard_removes_partial() {
        let dir = std::env::temp_dir().join(format!("kt-download-{}", unique_temp_suffix()));
        std::fs::create_dir(&dir).unwrap();
        let temp = dir.join("partial");
        let target = dir.join("target");
        std::fs::write(&temp, b"new").unwrap();
        std::fs::write(&target, b"old").unwrap();
        assert!(commit_local(&temp, &target, false).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        drop(LocalTempGuard(temp.clone()));
        assert!(!temp.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(all(test, unix))]
mod protocol_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "需要本机 OpenSSH sftp-server；通过 --ignored 显式运行"]
    async fn recursive_download_against_openssh() {
        let server = ["/usr/libexec/sftp-server", "/usr/lib/openssh/sftp-server"]
            .into_iter()
            .find(|p| Path::new(p).is_file())
            .expect("缺少 sftp-server");
        let mut child = tokio::process::Command::new(server)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = child.stdout.take().unwrap();
        let (client, bridge) = tokio::io::duplex(65536);
        let (mut reader, mut writer) = tokio::io::split(bridge);
        let a = tokio::spawn(async move { tokio::io::copy(&mut reader, &mut input).await });
        let b = tokio::spawn(async move { tokio::io::copy(&mut output, &mut writer).await });
        let session = SftpSession::new(client).await.unwrap();
        let base =
            std::env::temp_dir().join(format!("kt-sftp-integration-{}", unique_temp_suffix()));
        std::fs::create_dir_all(base.join("remote/nested/empty")).unwrap();
        std::fs::write(base.join("remote/nested/中文.txt"), "中文内容").unwrap();
        std::fs::write(base.join("remote/zero"), "").unwrap();
        let base = base.canonicalize().unwrap();
        let remote = base.join("remote").to_str().unwrap().to_string();
        let target = base.join("download");
        let (tx, mut rx) = mpsc::channel(128);
        let out = SessionEventSender::new(SessionId(1), 0, tx);
        download_tree(
            &session,
            SessionId(1),
            SftpRequestId(1),
            &remote,
            &target,
            false,
            &out,
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("nested/中文.txt")).unwrap(),
            "中文内容"
        );
        assert!(target.join("nested/empty").is_dir());
        assert_eq!(std::fs::metadata(target.join("zero")).unwrap().len(), 0);
        assert!(rx.try_recv().is_ok());
        std::fs::write(target.join("nested/中文.txt"), "old").unwrap();
        assert!(download_tree(
            &session,
            SessionId(1),
            SftpRequestId(2),
            &remote,
            &target,
            false,
            &out
        )
        .await
        .is_err());
        assert_eq!(
            std::fs::read_to_string(target.join("nested/中文.txt")).unwrap(),
            "old"
        );
        download_tree(
            &session,
            SessionId(1),
            SftpRequestId(3),
            &remote,
            &target,
            true,
            &out,
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("nested/中文.txt")).unwrap(),
            "中文内容"
        );
        let single = base.join("single");
        download_tree(
            &session,
            SessionId(1),
            SftpRequestId(4),
            &format!("{remote}/nested/中文.txt"),
            &single,
            false,
            &out,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(single).unwrap(), "中文内容");
        std::os::unix::fs::symlink(base.join("remote"), base.join("remote/link")).unwrap();
        assert!(download_tree(
            &session,
            SessionId(1),
            SftpRequestId(5),
            &remote,
            &base.join("rejected"),
            false,
            &out
        )
        .await
        .is_err());
        assert!(!base.join("rejected").exists());
        session.close().await.unwrap();
        child.kill().await.unwrap();
        a.abort();
        b.abort();
        std::fs::remove_dir_all(base).unwrap();
    }
}
