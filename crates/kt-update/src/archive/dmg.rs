//! DMG 只读挂载与有界复制；固定私有挂载点便于回收中断留下的镜像。
use super::{checked_path, create_links, validate_link, write_entry};
use crate::metadata::MAX_EXPANDED;
use anyhow::{ensure, Context, Result};
use std::{
    collections::HashSet,
    ffi::OsStr,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(super) fn extract(image: &Path, directory: &Path) -> Result<()> {
    let root = image
        .parent()
        .context("镜像缺少父目录 / Missing image parent")?;
    let mount = root.join(".dmg-mount");
    if mount.exists() || fs::symlink_metadata(&mount).is_ok() {
        ensure!(
            fs::symlink_metadata(&mount)?.is_dir(),
            "挂载点无效 / Invalid DMG mount point"
        );
        if mounted(&mount, root)? {
            detach(&mount)?;
        }
        fs::remove_dir(&mount).context("旧挂载目录非空 / Previous mount directory is not empty")?;
    }
    fs::create_dir(&mount)?;
    let result = (|| {
        fs::set_permissions(&mount, fs::Permissions::from_mode(0o700))?;
        hdiutil(
            &[
                OsStr::new("verify"),
                OsStr::new("-quiet"),
                image.as_os_str(),
            ],
            Duration::from_secs(90),
        )?;
        hdiutil(
            &[
                OsStr::new("attach"),
                OsStr::new("-readonly"),
                OsStr::new("-nobrowse"),
                OsStr::new("-noautoopen"),
                OsStr::new("-mountpoint"),
                mount.as_os_str(),
                image.as_os_str(),
            ],
            Duration::from_secs(90),
        )?;
        ensure!(mounted(&mount, root)?, "镜像未挂载 / DMG was not mounted");
        copy_bundle(&mount, directory)
    })();
    // attach 失败也可能已建立设备；始终检查并卸载，失败时保留挂载点供重试。
    if mounted(&mount, root)? {
        if let Err(error) = detach(&mount) {
            anyhow::bail!("无法卸载更新镜像，保留挂载点 {} / Cannot detach update image: {error:#}; extraction: {result:?}", mount.display());
        }
    }
    fs::remove_dir(&mount)?;
    result
}

fn mounted(mount: &Path, root: &Path) -> Result<bool> {
    Ok(fs::metadata(mount)?.dev() != fs::metadata(root)?.dev())
}

fn detach(mount: &Path) -> Result<()> {
    if hdiutil(
        &[OsStr::new("detach"), mount.as_os_str()],
        Duration::from_secs(20),
    )
    .is_err()
    {
        hdiutil(
            &[
                OsStr::new("detach"),
                OsStr::new("-force"),
                mount.as_os_str(),
            ],
            Duration::from_secs(20),
        )?;
    }
    Ok(())
}

fn hdiutil(arguments: &[&OsStr], timeout: Duration) -> Result<()> {
    let mut diagnostic = tempfile::tempfile()?;
    let mut child = Command::new("/usr/bin/hdiutil")
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(diagnostic.try_clone()?)
        .stderr(diagnostic.try_clone()?)
        .spawn()
        .context("无法启动磁盘镜像工具 / Cannot start disk image utility")?;
    let deadline = Instant::now() + timeout;
    let status = (|| -> Result<_> {
        loop {
            ensure!(
                diagnostic.metadata()?.len() <= 64 * 1024,
                "磁盘镜像输出超限 / Disk image output exceeded limit"
            );
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            ensure!(
                Instant::now() < deadline,
                "磁盘镜像操作超时 / Disk image operation timed out"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    })();
    if status.is_err() {
        let kill = child.kill();
        let wait = child.wait();
        ensure!(wait.is_ok(), "无法回收磁盘镜像进程 / Cannot reap disk image process: {status:?}; kill: {kill:?}; wait: {wait:?}");
    }
    let status = status?;
    diagnostic.seek(SeekFrom::Start(0))?;
    let mut output = Vec::new();
    diagnostic.take(8192).read_to_end(&mut output)?;
    ensure!(
        status.success(),
        "磁盘镜像操作失败 / Disk image operation failed: {}",
        String::from_utf8_lossy(&output)
    );
    Ok(())
}

fn copy_bundle(mount: &Path, directory: &Path) -> Result<()> {
    let source = mount.join("KitonyTerms.app");
    ensure!(
        fs::symlink_metadata(&source)?.is_dir(),
        "镜像缺少应用包 / DMG has no application bundle"
    );
    let mut seen = HashSet::new();
    let mut total = 0;
    let mut links = Vec::new();
    copy_entry(
        &source,
        Path::new("KitonyTerms.app"),
        directory,
        &mut seen,
        &mut total,
        &mut links,
    )?;
    create_links(directory, &links)
}

fn copy_entry(
    source: &Path,
    relative: &Path,
    directory: &Path,
    seen: &mut HashSet<String>,
    total: &mut u64,
    links: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<()> {
    let relative = checked_path(relative, seen)?;
    let metadata = fs::symlink_metadata(source)?;
    let target = directory.join(&relative);
    if metadata.file_type().is_symlink() {
        let link = fs::read_link(source)?;
        ensure!(
            link.as_os_str().len() <= 4096,
            "镜像链接过长 / DMG link is too long"
        );
        validate_link(&relative, &link)?;
        links.push((relative, link));
    } else if metadata.is_dir() {
        fs::create_dir(&target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_entry(
                &entry.path(),
                &relative.join(entry.file_name()),
                directory,
                seen,
                total,
                links,
            )?;
        }
    } else {
        ensure!(
            metadata.is_file(),
            "不支持的镜像文件 / Unsupported DMG file"
        );
        ensure!(
            metadata.len() <= MAX_EXPANDED.saturating_sub(*total),
            "镜像内容超过上限 / DMG contents exceed size limit"
        );
        write_entry(
            &mut File::open(source)?,
            &target,
            metadata.permissions().mode(),
            total,
        )?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn create_test_image(source: &Path, image: &Path) -> Result<()> {
    hdiutil(
        &[
            OsStr::new("create"),
            OsStr::new("-volname"),
            OsStr::new("KitonyTerms"),
            OsStr::new("-srcfolder"),
            source.as_os_str(),
            OsStr::new("-format"),
            OsStr::new("UDZO"),
            image.as_os_str(),
        ],
        Duration::from_secs(90),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_only_bundle_and_preserves_internal_links() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let resources = source.join("KitonyTerms.app/Contents/Resources");
        fs::create_dir_all(&resources).unwrap();
        fs::write(resources.join("file"), b"resource").unwrap();
        std::os::unix::fs::symlink("file", resources.join("link")).unwrap();
        std::os::unix::fs::symlink("/Applications", source.join("Applications")).unwrap();
        let output = dir.path().join("output");
        fs::create_dir(&output).unwrap();
        copy_bundle(&source, &output).unwrap();
        assert_eq!(
            fs::read(output.join("KitonyTerms.app/Contents/Resources/link")).unwrap(),
            b"resource"
        );
        assert!(!output.join("Applications").exists());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(output.join("KitonyTerms.app/Contents/Resources/file"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            fs::metadata(resources.join("file"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        );
    }

    #[test]
    fn rejects_escaping_links_and_oversized_files() {
        for oversized in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("source");
            let app = source.join("KitonyTerms.app");
            fs::create_dir_all(&app).unwrap();
            if oversized {
                File::create(app.join("large"))
                    .unwrap()
                    .set_len(MAX_EXPANDED + 1)
                    .unwrap();
            } else {
                std::os::unix::fs::symlink("../outside", app.join("link")).unwrap();
            }
            let output = dir.path().join("output");
            fs::create_dir(&output).unwrap();
            assert!(copy_bundle(&source, &output).is_err());
        }
    }

    #[test]
    fn native_images_clean_up_and_recover_previous_mounts() {
        let _guard = crate::archive::DMG_TEST_LOCK.lock().unwrap();
        for escaping in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("source");
            let app = source.join("KitonyTerms.app");
            fs::create_dir_all(&app).unwrap();
            fs::write(app.join("file"), b"resource").unwrap();
            std::os::unix::fs::symlink("/Applications", source.join("Applications")).unwrap();
            if escaping {
                std::os::unix::fs::symlink("../outside", app.join("link")).unwrap();
            }
            let image = dir.path().join("update image.dmg");
            create_test_image(&source, &image).unwrap();
            let mount = dir.path().join(".dmg-mount");
            if !escaping {
                fs::create_dir(&mount).unwrap();
                hdiutil(
                    &[
                        OsStr::new("attach"),
                        OsStr::new("-readonly"),
                        OsStr::new("-nobrowse"),
                        OsStr::new("-noautoopen"),
                        OsStr::new("-mountpoint"),
                        mount.as_os_str(),
                        image.as_os_str(),
                    ],
                    Duration::from_secs(90),
                )
                .unwrap();
                assert!(mounted(&mount, dir.path()).unwrap());
            }
            let output = dir.path().join("output");
            let package = crate::metadata::Package {
                name: "kitonyterms-test.dmg".into(),
                size: 1,
                sha256: "0".repeat(64),
                kind: crate::metadata::PackageKind::Dmg,
            };
            let result = super::super::extract(&package, &image, &output);
            assert!(
                !mount.exists(),
                "挂载点必须被清理 / Mount point must be removed"
            );
            if escaping {
                assert!(result.is_err());
                assert!(!output.exists());
            } else {
                result.unwrap();
                assert_eq!(
                    fs::read(output.join("KitonyTerms.app/file")).unwrap(),
                    b"resource"
                );
                assert!(!output.join("Applications").exists());
            }
        }
    }

    #[test]
    fn corrupt_image_cleans_mount_and_partial_output() {
        let _guard = crate::archive::DMG_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("corrupt.dmg");
        fs::write(&image, b"invalid disk image").unwrap();
        let output = dir.path().join("output");
        let package = crate::metadata::Package {
            name: "kitonyterms-test.dmg".into(),
            size: 1,
            sha256: "0".repeat(64),
            kind: crate::metadata::PackageKind::Dmg,
        };
        assert!(super::super::extract(&package, &image, &output).is_err());
        assert!(!dir.path().join(".dmg-mount").exists());
        assert!(!output.exists());
    }
}
