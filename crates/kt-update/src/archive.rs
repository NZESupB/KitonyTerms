//! 隔离解包和平台校验；不直接使用归档库的任意路径解包接口。
use crate::metadata::{Package, PackageKind, MAX_EXPANDED};
use anyhow::{ensure, Result};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
#[cfg(target_os = "macos")]
mod dmg;

#[cfg(all(test, target_os = "macos"))]
pub(crate) use dmg::create_test_image;
// 原生镜像工具共用系统服务，测试按完整镜像生命周期串行执行。
#[cfg(all(test, target_os = "macos"))]
pub(crate) static DMG_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn extract(package: &Package, archive: &Path, directory: &Path) -> Result<()> {
    ensure!(
        !directory.exists(),
        "解包目录已存在 / Extraction directory exists"
    );
    fs::create_dir(directory)?;
    let result = extract_inner(package, archive, directory);
    if result.is_err() {
        fs::remove_dir_all(directory)?;
    }
    result
}
fn extract_inner(package: &Package, archive: &Path, directory: &Path) -> Result<()> {
    let mut seen = HashSet::new();
    let mut total = 0u64;
    let mut links: Vec<(PathBuf, PathBuf)> = Vec::new();
    match package.kind {
        PackageKind::Zip => {
            let mut zip = zip::ZipArchive::new(File::open(archive)?)?;
            ensure!(
                zip.len() <= 20000,
                "归档文件过多 / Too many archive entries"
            );
            for i in 0..zip.len() {
                let mut entry = zip.by_index(i)?;
                let name = entry.name()?;
                let relative = checked_path(Path::new(name.as_ref()), &mut seen)?;
                let path = directory.join(&relative);
                let mode = entry.unix_mode().unwrap_or(0o100644);
                if mode & 0o170000 == 0o120000 {
                    ensure!(
                        relative.starts_with("KitonyTerms.app") && entry.size() <= 4096,
                        "归档链接无效 / Invalid archive link"
                    );
                    let mut target = String::new();
                    entry.take(4097).read_to_string(&mut target)?;
                    let target = PathBuf::from(target);
                    validate_link(&relative, &target)?;
                    links.push((relative, target));
                } else if entry.is_dir() {
                    fs::create_dir_all(&path)?;
                } else {
                    ensure!(
                        mode & 0o170000 == 0 || mode & 0o170000 == 0o100000,
                        "不支持的归档文件 / Unsupported archive entry"
                    );
                    // ditto 的 AppleDouble 载体不能作为普通资源加入已签名应用包。
                    if (relative.starts_with("KitonyTerms.app") || relative.starts_with("__MACOSX"))
                        && relative
                            .file_name()
                            .is_some_and(|name| name.to_string_lossy().starts_with("._"))
                    {
                        discard_apple_double(&mut entry, &mut total)?;
                    } else {
                        write_entry(&mut entry, &path, mode, &mut total)?;
                    }
                }
            }
        }
        PackageKind::TarGz => {
            let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(File::open(archive)?));
            for entry in tar.entries()? {
                let mut entry = entry?;
                let relative = checked_path(&entry.path()?, &mut seen)?;
                let path = directory.join(relative);
                let kind = entry.header().entry_type();
                if kind.is_dir() {
                    fs::create_dir_all(&path)?;
                } else {
                    ensure!(kind.is_file(), "不支持的 TAR 文件 / Unsupported TAR entry");
                    let mode = entry.header().mode()?;
                    write_entry(&mut entry, &path, mode, &mut total)?;
                }
            }
        }
        PackageKind::Dmg => {
            #[cfg(target_os = "macos")]
            return dmg::extract(archive, directory);
            #[cfg(not(target_os = "macos"))]
            anyhow::bail!("此平台不支持 DMG 更新包 / DMG updates require macOS");
        }
        PackageKind::Installer => anyhow::bail!("安装器无需解包 / Installer cannot be extracted"),
    }
    create_links(directory, &links)
}

fn create_links(directory: &Path, links: &[(PathBuf, PathBuf)]) -> Result<()> {
    // 所有普通文件落盘后再建立链接，后续写入不会沿链接走出目录。
    for (relative, target) in links {
        let path = directory.join(relative);
        ensure!(
            fs::canonicalize(path.parent().unwrap())?
                .starts_with(fs::canonicalize(directory.join("KitonyTerms.app"))?),
            "链接父目录越界 / Link parent escapes app bundle"
        );
        ensure!(
            path.parent().is_some_and(Path::is_dir),
            "链接父目录缺失 / Missing link parent"
        );
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, &path)?;
        #[cfg(not(unix))]
        {
            let _ = (path, target);
            anyhow::bail!("当前平台不支持归档链接 / Archive links unsupported");
        }
    }
    for (relative, _) in links {
        ensure!(
            fs::canonicalize(directory.join(relative))?
                .starts_with(fs::canonicalize(directory.join("KitonyTerms.app"))?),
            "链接解析后越界 / Resolved link escapes app bundle"
        );
    }
    Ok(())
}
fn discard_apple_double(reader: &mut impl Read, total: &mut u64) -> Result<()> {
    let mut magic = [0; 4];
    reader.read_exact(&mut magic)?;
    ensure!(
        magic == [0, 5, 22, 7],
        "无效 AppleDouble 元数据 / Invalid AppleDouble metadata"
    );
    let copied = std::io::copy(
        &mut reader.take(MAX_EXPANDED.saturating_sub(*total) + 1),
        &mut std::io::sink(),
    )?;
    *total = total
        .checked_add(copied)
        .and_then(|n| n.checked_add(4))
        .ok_or_else(|| anyhow::anyhow!("解包大小溢出 / Archive size overflow"))?;
    ensure!(
        *total <= MAX_EXPANDED,
        "解包超过上限 / Expanded archive exceeds limit"
    );
    Ok(())
}

fn checked_path(path: &Path, seen: &mut HashSet<String>) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        let Component::Normal(name) = component else {
            anyhow::bail!("归档路径不安全 / Unsafe archive path");
        };
        let name = name
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("归档路径编码无效 / Invalid path encoding"))?;
        ensure!(
            !name.contains(['\\', ':']) && !name.ends_with(['.', ' ']),
            "归档路径不安全 / Unsafe archive path"
        );
        normalized.push(name);
    }
    ensure!(
        !normalized.as_os_str().is_empty() && normalized.components().count() <= 32,
        "归档路径无效 / Invalid archive path"
    );
    ensure!(
        seen.len() < 20000 && seen.insert(normalized.to_string_lossy().to_lowercase()),
        "重复路径或过多文件 / Duplicate path or too many files"
    );
    Ok(normalized)
}
fn validate_link(path: &Path, target: &Path) -> Result<()> {
    ensure!(
        target.is_relative(),
        "绝对链接被拒绝 / Absolute link rejected"
    );
    let mut resolved = path.parent().unwrap().to_path_buf();
    for component in target.components() {
        match component {
            Component::Normal(name) => {
                ensure!(
                    !name.to_string_lossy().contains(['\\', ':']),
                    "链接路径无效 / Invalid link path"
                );
                resolved.push(name);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                ensure!(
                    resolved.components().count() > 1,
                    "越界链接被拒绝 / Escaping link rejected"
                );
                resolved.pop();
            }
            _ => anyhow::bail!("链接路径无效 / Invalid link path"),
        }
    }
    ensure!(
        resolved.starts_with("KitonyTerms.app"),
        "越界链接被拒绝 / Escaping link rejected"
    );
    Ok(())
}
fn write_entry(reader: &mut impl Read, path: &Path, mode: u32, total: &mut u64) -> Result<()> {
    fs::create_dir_all(path.parent().unwrap())?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let copied = std::io::copy(
        &mut reader.take(MAX_EXPANDED.saturating_sub(*total) + 1),
        &mut file,
    )?;
    *total = total
        .checked_add(copied)
        .ok_or_else(|| anyhow::anyhow!("解包大小溢出 / Archive size overflow"))?;
    ensure!(
        *total <= MAX_EXPANDED,
        "解包超过上限 / Expanded archive exceeds limit"
    );
    file.flush()?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    Ok(())
}
pub fn payload(package: &Package, directory: &Path, target: &str, bundle: bool) -> Result<PathBuf> {
    let binary = if target.contains("windows") {
        directory.join("kitonyterms.exe")
    } else if target.contains("apple") {
        directory.join("KitonyTerms.app/Contents/MacOS/kitonyterms")
    } else {
        directory
            .join(
                package
                    .name
                    .strip_suffix(".tar.gz")
                    .ok_or_else(|| anyhow::anyhow!("TAR 名称无效 / Invalid TAR name"))?,
            )
            .join("kitonyterms")
    };
    ensure!(
        fs::symlink_metadata(&binary)?.is_file(),
        "更新程序无效 / Invalid update binary"
    );
    validate_binary(&binary, target)?;
    if target.contains("apple") {
        let app = directory.join("KitonyTerms.app");
        #[cfg(target_os = "macos")]
        {
            let signature = std::process::Command::new("/usr/bin/codesign")
                .args(["--verify", "--deep", "--strict", "--verbose=4"])
                .arg(&app)
                .output()?;
            ensure!(
                signature.status.success(),
                "应用签名校验失败 / App signature verification failed: {}",
                String::from_utf8_lossy(&signature.stderr)
            );
            let identity = std::process::Command::new("/usr/libexec/PlistBuddy")
                .args(["-c", "Print :CFBundleIdentifier"])
                .arg(app.join("Contents/Info.plist"))
                .output()?;
            ensure!(
                identity.status.success()
                    && String::from_utf8_lossy(&identity.stdout).trim() == "com.kitonyterms.app",
                "应用身份不匹配 / App identity mismatch"
            );
        }
        if bundle {
            return Ok(app);
        }
    }
    Ok(binary)
}
pub fn validate_binary(path: &Path, target: &str) -> Result<()> {
    let mut file = File::open(path)?;
    let mut header = [0; 64];
    file.read_exact(&mut header)?;
    let arm = target.starts_with("aarch64-");
    let valid = if target.contains("linux") {
        header[..4] == *b"\x7fELF"
            && header[4] == 2
            && header[5] == 1
            && u16::from_le_bytes([header[18], header[19]]) == if arm { 183 } else { 62 }
    } else if target.contains("apple") {
        header[..4] == [0xcf, 0xfa, 0xed, 0xfe]
            && u32::from_le_bytes(header[4..8].try_into().unwrap())
                == if arm { 0x0100000c } else { 0x01000007 }
    } else if target.contains("windows") {
        use std::io::{Seek, SeekFrom};
        let offset = u32::from_le_bytes(header[60..64].try_into().unwrap());
        ensure!(offset < 1024 * 1024, "PE 头无效 / Invalid PE header");
        file.seek(SeekFrom::Start(offset as u64))?;
        let mut pe = [0; 6];
        file.read_exact(&mut pe)?;
        header[..2] == *b"MZ"
            && pe[..4] == *b"PE\0\0"
            && u16::from_le_bytes([pe[4], pe[5]]) == if arm { 0xaa64 } else { 0x8664 }
    } else {
        false
    };
    ensure!(
        valid,
        "更新程序的平台或架构不匹配 / Binary platform or architecture mismatch"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn traversal_duplicates_and_links() {
        for value in ["../bad", "/bad", "a/../bad", "a\\b", "C:bad", "a."] {
            assert!(checked_path(Path::new(value), &mut HashSet::new()).is_err());
        }
        let mut seen = HashSet::new();
        checked_path(Path::new("A/b"), &mut seen).unwrap();
        assert!(checked_path(Path::new("a/b"), &mut seen).is_err());
        validate_link(
            Path::new("KitonyTerms.app/Contents/A"),
            Path::new("../Resources/B"),
        )
        .unwrap();
        assert!(validate_link(Path::new("KitonyTerms.app/A"), Path::new("../outside")).is_err());
    }
    #[test]
    fn zip_failure_cleans_partial_output() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("bad.zip");
        let mut writer = zip::ZipWriter::new(File::create(&zip_path).unwrap());
        writer
            .start_file("../escape", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"bad").unwrap();
        writer.finish().unwrap();
        let p = Package {
            name: "kitonyterms-test.zip".into(),
            size: 1,
            sha256: "0".repeat(64),
            kind: PackageKind::Zip,
        };
        assert!(extract(&p, &zip_path, &dir.path().join("output")).is_err());
        assert!(!dir.path().join("output").exists());
        assert!(!dir.path().join("escape").exists());
    }
    #[test]
    fn expanded_size_and_special_modes_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let mut total = MAX_EXPANDED - 2;
        assert!(write_entry(
            &mut &b"abc"[..],
            &dir.path().join("large"),
            0o644,
            &mut total
        )
        .is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.path().join("permissions");
            write_entry(&mut &b"ok"[..], &path, 0o106755, &mut 0).unwrap();
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o7777,
                0o755
            );
        }
    }
    #[test]
    fn tar_links_and_duplicate_entries_are_rejected() {
        for duplicate in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("bad.tar.gz");
            let encoder = flate2::write::GzEncoder::new(
                File::create(&path).unwrap(),
                flate2::Compression::default(),
            );
            let mut tar = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            if duplicate {
                header.set_size(1);
                header.set_cksum();
                for _ in 0..2 {
                    tar.append_data(&mut header, "same", &b"x"[..]).unwrap();
                }
            } else {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                header.set_link_name("../escape").unwrap();
                header.set_cksum();
                tar.append_data(&mut header, "link", std::io::empty())
                    .unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap();
            let package = Package {
                name: "kitonyterms-test.tar.gz".into(),
                size: 1,
                sha256: "0".repeat(64),
                kind: PackageKind::TarGz,
            };
            assert!(extract(&package, &path, &dir.path().join("output")).is_err());
            assert!(!dir.path().join("output").exists());
        }
    }
    #[test]
    fn binary_architecture() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("binary");
        let mut bytes = [0; 64];
        bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
        bytes[18] = 62;
        fs::write(&path, bytes).unwrap();
        validate_binary(&path, "x86_64-unknown-linux-gnu").unwrap();
        assert!(validate_binary(&path, "aarch64-unknown-linux-gnu").is_err());
    }
}
