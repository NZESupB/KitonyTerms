//! 有界下载，成功校验后原子提交缓存。
use crate::{
    client::{Available, ReleaseClient},
    metadata::Package,
};
use anyhow::{ensure, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

pub async fn download(
    client: &ReleaseClient,
    available: &Available,
    root: &Path,
    cancel: &CancellationToken,
    progress: impl Fn(u64) + Send,
) -> Result<PathBuf> {
    available.package.validate()?;
    let package = &available.package;
    let cached = root.join(format!("package-{}", package.sha256));
    if cached.exists() {
        let path = cached.clone();
        let metadata = package.clone();
        if tokio::task::spawn_blocking(move || verify(&path, &metadata))
            .await?
            .is_ok()
        {
            return Ok(cached);
        }
        tokio::fs::remove_file(&cached).await?;
    }
    let temp = tempfile::NamedTempFile::new_in(root)?;
    let mut file = tokio::fs::File::from_std(temp.reopen()?);
    let mut response = tokio::select! {
        _ = cancel.cancelled() => anyhow::bail!("下载已取消 / Download cancelled"),
        response = client.asset(available.asset_id) => response?,
    };
    ensure!(
        response
            .content_length()
            .is_none_or(|size| size == package.size),
        "下载大小不一致 / Download length mismatch"
    );
    let mut received = 0u64;
    let mut hash = Sha256::new();
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => anyhow::bail!("下载已取消 / Download cancelled"),
            chunk = response.chunk() => chunk?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        received = received
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| anyhow::anyhow!("下载过大 / Download too large"))?;
        ensure!(
            received <= package.size,
            "下载超过声明大小 / Download exceeds declared size"
        );
        hash.update(&chunk);
        file.write_all(&chunk).await?;
        progress(received);
    }
    ensure!(!cancel.is_cancelled(), "下载已取消 / Download cancelled");
    ensure!(
        received == package.size
            && hex::encode(hash.finalize()).eq_ignore_ascii_case(&package.sha256),
        "更新包校验失败 / Package checksum mismatch"
    );
    file.sync_all().await?;
    drop(file);
    temp.persist(&cached)?;
    prune_cache(root, &cached)?;
    Ok(cached)
}
pub fn verify(path: &Path, package: &Package) -> Result<()> {
    package.validate()?;
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.len() == package.size,
        "更新包大小无效 / Package size mismatch"
    );
    ensure!(
        sha256(path)?.eq_ignore_ascii_case(&package.sha256),
        "更新包校验失败 / Package checksum mismatch"
    );
    Ok(())
}
pub fn sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hex::encode(hash.finalize()))
}
fn prune_cache(root: &Path, keep: &Path) -> Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry.path() != keep
            && name
                .strip_prefix("package-")
                .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            && entry.file_type()?.is_file()
        {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::PackageKind;
    #[test]
    fn checksum_size_and_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package");
        std::fs::write(&path, b"valid").unwrap();
        let mut p = Package {
            name: "kitonyterms-v1-linux-x64.tar.gz".into(),
            size: 5,
            sha256: sha256(&path).unwrap(),
            kind: PackageKind::TarGz,
        };
        verify(&path, &p).unwrap();
        p.size = 6;
        assert!(verify(&path, &p).is_err());
        p.size = 5;
        p.sha256 = "0".repeat(64);
        assert!(verify(&path, &p).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&path, dir.path().join("link")).unwrap();
            assert!(verify(&dir.path().join("link"), &p).is_err());
        }
    }
}
