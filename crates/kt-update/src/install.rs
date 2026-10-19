//! 安装事务：目标同目录暂存、保留备份，启动确认后才清理。
use crate::{
    archive,
    client::Available,
    download,
    metadata::{BuildInfo, Package, PackageKind},
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Installation {
    pub executable: PathBuf,
    pub target: PathBuf,
    pub bundle: bool,
    pub installed: bool,
}
/// 替换目标文件所需的权限级别。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
    /// 目录可写，可直接替换。
    Writable,
    /// 目录不可写但存在：替换必须经过系统授权（Windows 受保护目录）。
    NeedsElevation,
    /// 提权也无法完成替换的原因，例如只读卷或 App Translocation。
    Unavailable(String),
}
impl Installation {
    pub fn discover() -> Result<Self> {
        Self::from_executable(fs::canonicalize(std::env::current_exe()?)?)
    }
    pub fn from_executable(executable: PathBuf) -> Result<Self> {
        ensure!(
            executable.is_absolute() && fs::symlink_metadata(&executable)?.is_file(),
            "应用路径无效 / Invalid application path"
        );
        let bundle = executable
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|n| n == "MacOS")
            && executable
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .is_some_and(|n| n == "Contents")
            && executable
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .is_some_and(|n| n == "KitonyTerms.app");
        let target = if bundle {
            executable
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf()
        } else {
            executable.clone()
        };
        let installed = cfg!(windows)
            && executable
                .parent()
                .unwrap()
                .join("kitonyterms-installation.txt")
                .is_file()
            && executable.parent().unwrap().join("unins000.exe").is_file()
            && registered_installation(&executable);
        Ok(Self {
            executable,
            target,
            bundle,
            installed,
        })
    }
    /// 探测替换所需的权限；已由提权安装器登记的安装恒为 `Writable`。
    pub fn access(&self) -> Access {
        if self.installed {
            return Access::Writable;
        }
        let Some(parent) = self.target.parent() else {
            return Access::Unavailable("缺少安装父目录 / Missing install parent".into());
        };
        if self.target.starts_with("/Volumes")
            || self.target.to_string_lossy().contains("/AppTranslocation/")
        {
            return Access::Unavailable(
                "请先安装到可写目录 / Install into a writable folder first".into(),
            );
        }
        match tempfile::NamedTempFile::new_in(parent) {
            Ok(_) => Access::Writable,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                Access::NeedsElevation
            }
            Err(error) => Access::Unavailable(format!("安装目录探测失败: {error}")),
        }
    }
    /// 便携副本位于受保护目录时，替换需要管理员授权。
    pub fn needs_elevation(&self) -> bool {
        matches!(self.access(), Access::NeedsElevation)
    }
    pub fn preflight(&self) -> Result<()> {
        match self.access() {
            Access::Writable => Ok(()),
            Access::NeedsElevation => anyhow::bail!(
                "安装目录不可写，暂存更新需要可写目录，请把便携版移动到用户目录或使用安装包 / Install folder is not writable; move the portable app to a user folder or use the installer: {}",
                self.target.display()
            ),
            Access::Unavailable(reason) => anyhow::bail!(
                "安装目录不可写，请使用安装包 / Install folder is not writable; use the installer: {reason}"
            ),
        }
    }
    pub fn backup(&self) -> PathBuf {
        self.target.with_file_name(format!(
            ".{}-update-backup",
            self.target.file_name().unwrap().to_string_lossy()
        ))
    }
    fn relative_executable(&self) -> &Path {
        if self.bundle {
            Path::new("Contents/MacOS/kitonyterms")
        } else {
            Path::new("")
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallTask {
    pub installation: Installation,
    pub current: BuildInfo,
    pub next: BuildInfo,
    pub source_sha256: String,
    pub package: Package,
    pub parent_pid: u32,
    /// 便携副本位于受保护目录：替换交由提权辅助进程完成。
    #[serde(default)]
    pub elevate: bool,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Prepared,
    BackedUp,
    Committed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Journal {
    pub task: InstallTask,
    pub staging: PathBuf,
    /// 暂存载荷的 SHA-256；提权辅助进程据此校验待替换文件。
    #[serde(default)]
    pub staged_sha256: String,
    pub stage: Stage,
}

pub fn private_root(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    ensure!(
        fs::symlink_metadata(path)?.is_dir(),
        "更新目录必须为普通目录 / Update directory must be a directory"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut temp =
        tempfile::NamedTempFile::new_in(path.parent().context("缺少父目录 / Missing parent")?)?;
    temp.write_all(&serde_json::to_vec_pretty(value)?)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    sync_directory(path.parent().unwrap())?;
    Ok(())
}
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_file() && meta.len() <= 2 * 1024 * 1024,
        "更新记录无效 / Invalid update record"
    );
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
pub fn gate(root: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("installation.lock"))?)
}
pub fn prepare(
    root: &Path,
    installation: &Installation,
    current: &BuildInfo,
    available: &Available,
    elevate: bool,
) -> Result<InstallTask> {
    ensure!(
        current.channel != crate::metadata::Channel::Development
            && available.info.channel == current.channel
            && available.info.target == current.target,
        "更新身份不匹配 / Update identity mismatch"
    );
    // 提权辅助程序只负责最终替换；暂存和备份仍由普通权限进程创建。
    // 必须在退出应用之前确认目录可写，提权选项不能跳过此检查。
    installation.preflight()?;
    ensure!(
        !root.join("task.json").exists() && !root.join("journal.json").exists(),
        "存在未完成的安装 / Pending installation exists"
    );
    let task = InstallTask {
        installation: installation.clone(),
        current: current.clone(),
        next: available.info.clone(),
        source_sha256: download::sha256(&installation.executable)?,
        package: available.package.clone(),
        parent_pid: std::process::id(),
        elevate: elevate && !installation.installed,
    };
    let package_path = package_path(root, &task.package);
    download::verify(&package_path, &task.package)?;
    if task.package.kind != PackageKind::Installer {
        let dir = tempfile::tempdir_in(root)?;
        let extracted = dir.path().join("payload");
        archive::extract(&task.package, &package_path, &extracted)?;
        let _ = archive::payload(
            &task.package,
            &extracted,
            &task.current.target,
            installation.bundle,
        )?;
    }
    atomic_json(&root.join("task.json"), &task)?;
    Ok(task)
}
pub fn package_path(root: &Path, package: &Package) -> PathBuf {
    root.join(format!("package-{}", package.sha256))
}
pub fn validate_task(root: &Path, task: &InstallTask, helper: &Path) -> Result<()> {
    task.package.validate()?;
    ensure!(
        task.current == BuildInfo::current()
            && task.next.channel == task.current.channel
            && task.next.target == task.current.target
            && task.parent_pid > 0,
        "任务构建身份不匹配 / Task build identity mismatch"
    );
    ensure!(
        fs::canonicalize(helper)? == helper_path(root)
            && download::sha256(helper)? == task.source_sha256,
        "更新辅助程序身份不匹配 / Helper identity mismatch"
    );
    let discovered =
        Installation::from_executable(fs::canonicalize(&task.installation.executable)?)?;
    ensure!(
        discovered.executable == task.installation.executable
            && discovered.target == task.installation.target
            && discovered.bundle == task.installation.bundle
            && discovered.installed == task.installation.installed,
        "安装目标身份不匹配 / Installation identity mismatch"
    );
    ensure!(
        download::sha256(&discovered.executable)? == task.source_sha256,
        "安装目标已变化 / Installation changed"
    );
    download::verify(&package_path(root, &task.package), &task.package)
}
/// 提权替换入口的校验：只接受便携副本、已备份且载荷摘要一致的安装记录。
///
/// 提权进程运行的是用户可写目录里的辅助程序，因此在做任何文件操作之前必须把
/// 事务、目标路径与暂存载荷三条身份都复核一遍。
pub fn validate_elevated_replace(root: &Path, task: &InstallTask) -> Result<Journal> {
    ensure!(
        task.elevate && !task.installation.installed && !task.installation.bundle,
        "提权替换不适用于该安装 / Elevated replacement does not apply"
    );
    let journal: Journal = read_json(&root.join("journal.json"))?;
    validate_journal(root, &journal)?;
    ensure!(
        journal.task.source_sha256 == task.source_sha256
            && journal.task.installation.executable == task.installation.executable
            && journal.stage == Stage::BackedUp,
        "提权替换状态无效 / Invalid elevated replacement state"
    );
    ensure!(
        !journal.staged_sha256.is_empty()
            && download::sha256(&journal.staging.join("payload"))? == journal.staged_sha256,
        "暂存载荷校验失败 / Staged payload checksum mismatch"
    );
    Ok(journal)
}
pub fn apply(root: &Path, task: &InstallTask) -> Result<()> {
    let result = apply_inner(root, task);
    if let Err(error) = &result {
        let restored = rollback(root);
        let message = match restored {
            Ok(()) => format!(
                "更新失败，已恢复旧版本 / Update failed; previous version restored: {error:#}"
            ),
            Err(recovery) => {
                format!("更新失败，恢复失败 / Update and recovery failed: {error:#}; {recovery:#}")
            }
        };
        atomic_json(&root.join("error.json"), &message)?;
    }
    result
}
fn apply_inner(root: &Path, task: &InstallTask) -> Result<()> {
    let install = &task.installation;
    ensure!(
        download::sha256(&install.executable)? == task.source_sha256,
        "安装目标已变化 / Installation changed"
    );
    let package = package_path(root, &task.package);
    download::verify(&package, &task.package)?;
    let backup = backup_path(root, install);
    ensure!(
        !backup.exists(),
        "备份路径已存在 / Backup path already exists"
    );
    let staging_dir = tempfile::Builder::new()
        .prefix(".kt-update-stage-")
        .tempdir_in(if install.installed {
            root
        } else {
            install.target.parent().unwrap()
        })?;
    let staged = staging_dir.path().join("payload");
    if install.installed {
        fs::copy(&install.executable, &staged)?;
        File::open(&staged)?.sync_all()?;
    } else {
        let extraction = tempfile::tempdir_in(root)?;
        let extracted = extraction.path().join("archive");
        archive::extract(&task.package, &package, &extracted)?;
        let source = archive::payload(
            &task.package,
            &extracted,
            &task.current.target,
            install.bundle,
        )?;
        copy_tree(&source, &staged)?;
        if !install.bundle {
            fs::set_permissions(&staged, fs::metadata(&install.executable)?.permissions())?;
        }
    }
    let staging = staging_dir.keep();
    let mut journal = Journal {
        task: task.clone(),
        staging,
        staged_sha256: if install.bundle {
            // 应用包是目录：提权替换不适用于应用包，此处不计算摘要。
            String::new()
        } else {
            download::sha256(&staged)?
        },
        stage: Stage::Prepared,
    };
    atomic_json(&root.join("journal.json"), &journal)?;
    copy_tree(&install.target, &backup)?;
    sync_directory(install.target.parent().unwrap())?;
    journal.stage = Stage::BackedUp;
    atomic_json(&root.join("journal.json"), &journal)?;
    if install.installed {
        run_installer(root, task)?;
    } else if task.elevate {
        // 受保护目录：替换只能由提权辅助进程完成，校验后由它单独执行文件交换。
        ensure!(
            !journal.staged_sha256.is_empty(),
            "缺少暂存校验值 / Missing staged checksum"
        );
        run_elevated_replace(root, task)?;
        ensure!(
            download::sha256(&install.target)? == journal.staged_sha256,
            "提权替换结果校验失败 / Elevated replacement checksum mismatch"
        );
    } else {
        replace(
            &journal.staging.join("payload"),
            &install.target,
            install.bundle,
        )?;
    }
    sync_directory(install.target.parent().unwrap())?;
    journal.stage = Stage::Committed;
    atomic_json(&root.join("journal.json"), &journal)?;
    Ok(())
}
fn validate_journal(root: &Path, j: &Journal) -> Result<()> {
    let install = &j.task.installation;
    ensure!(
        install.target.is_absolute()
            && install.executable.is_absolute()
            && j.staging.parent()
                == Some(if install.installed {
                    root
                } else {
                    install.target.parent().unwrap()
                })
            && j.staging
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".kt-update-stage-")),
        "安装记录路径无效 / Invalid transaction paths"
    );
    ensure!(
        if install.bundle {
            install.executable == install.target.join(install.relative_executable())
                && install
                    .target
                    .file_name()
                    .is_some_and(|v| v == "KitonyTerms.app")
        } else {
            install.executable == install.target
        },
        "安装记录身份无效 / Invalid transaction identity"
    );
    Ok(())
}
pub fn rollback(root: &Path) -> Result<()> {
    let path = root.join("journal.json");
    if !path.exists() {
        return Ok(());
    }
    let journal: Journal = read_json(&path)?;
    validate_journal(root, &journal)?;
    let install = &journal.task.installation;
    let backup = backup_path(root, install);
    // 备份复制尚未完成，或安装器未改动文件时，原版本本身仍是有效恢复点。
    if (journal.stage == Stage::Prepared || install.installed)
        && download::sha256(&install.executable)
            .is_ok_and(|digest| digest == journal.task.source_sha256)
    {
        remove(&backup)?;
        remove(&journal.staging)?;
        fs::remove_file(path)?;
        return Ok(());
    }
    if backup.exists() {
        let original = if install.bundle {
            backup.join(install.relative_executable())
        } else {
            backup.clone()
        };
        ensure!(
            download::sha256(&original)? == journal.task.source_sha256,
            "旧版本备份校验失败 / Backup checksum mismatch"
        );
        if install.installed {
            fs::copy(&backup, &install.target)?;
            File::open(&install.target)?.sync_all()?;
        } else {
            replace(&backup, &install.target, install.bundle)?;
        }
        remove(&backup)?;
        sync_directory(install.target.parent().unwrap())?;
    } else {
        ensure!(
            download::sha256(&install.executable)? == journal.task.source_sha256,
            "缺少可恢复备份 / Recovery backup missing"
        );
    }
    remove(&journal.staging)?;
    fs::remove_file(path)?;
    Ok(())
}
/// 必须在安装互斥锁内调用。仅新版本实际启动到 UI 后确认成功。
pub fn recover(root: &Path, current: &BuildInfo, executable: &Path, confirm: bool) -> Result<()> {
    let path = root.join("journal.json");
    if !path.exists() {
        return Ok(());
    }
    let journal: Journal = read_json(&path)?;
    validate_journal(root, &journal)?;
    ensure!(
        journal.task.installation.executable == executable,
        "安装记录属于其他应用路径 / Transaction belongs to another installation"
    );
    if journal.stage == Stage::Committed && &journal.task.next == current {
        if confirm {
            remove(&backup_path(root, &journal.task.installation))?;
            remove(&journal.staging)?;
            fs::remove_file(path)?;
        }
    } else {
        rollback(root)?;
        atomic_json(
            &root.join("error.json"),
            &"上次更新被中断，已恢复旧版本 / Interrupted update restored",
        )?;
    }
    Ok(())
}
fn backup_path(root: &Path, install: &Installation) -> PathBuf {
    if install.installed {
        root.join("previous-executable.exe")
    } else {
        install.backup()
    }
}
pub fn remove(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path)?,
        Ok(_) => fs::remove_file(path)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(source)?;
    if meta.file_type().is_symlink() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(fs::read_link(source)?, target)?;
        #[cfg(not(unix))]
        anyhow::bail!("不支持的链接 / Unsupported symlink");
    } else if meta.is_dir() {
        fs::create_dir(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
        fs::set_permissions(target, meta.permissions())?;
        sync_directory(target)?;
    } else {
        ensure!(
            meta.is_file(),
            "不支持的安装文件 / Unsupported installation file"
        );
        fs::copy(source, target)?;
        File::open(target)?.sync_all()?;
    }
    Ok(())
}
fn replace(source: &Path, target: &Path, bundle: bool) -> Result<()> {
    #[cfg(target_os = "macos")]
    if bundle && target.exists() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let source = CString::new(source.as_os_str().as_bytes())?;
        let target = CString::new(target.as_os_str().as_bytes())?;
        let result = unsafe {
            libc::renameatx_np(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                target.as_ptr(),
                libc::RENAME_SWAP,
            )
        };
        ensure!(
            result == 0,
            "原子交换应用包失败 / Atomic bundle swap failed: {}",
            std::io::Error::last_os_error()
        );
        return Ok(());
    }
    #[cfg(not(target_os = "macos"))]
    let _ = bundle;
    replace_file(source, target)
}

/// 替换失败的原因分类；上层据此判断是否提示管理员授权重试。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplaceKind {
    /// 目标文件被其他进程占用。
    InUse,
    /// 访问被拒绝：缺少权限或目标只读。
    AccessDenied,
    /// 其他错误，重试没有意义。
    Other,
}
#[cfg(any(windows, test))]
impl ReplaceKind {
    fn from_os_error(code: Option<i32>) -> Self {
        // 5=拒绝访问、32=共享冲突、33=锁冲突、1231=网络资源不可用。
        match code {
            Some(5) => Self::AccessDenied,
            Some(32) | Some(33) => Self::InUse,
            _ => Self::Other,
        }
    }
}
/// 替换失败的分类信息，携带出错路径以便给出可定位的错误文案。
#[derive(Debug)]
pub struct ReplaceFailure {
    pub target: PathBuf,
    pub kind: ReplaceKind,
}
impl std::fmt::Display for ReplaceFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self.kind {
            ReplaceKind::InUse => "目标文件被占用 / Target file is in use",
            ReplaceKind::AccessDenied => {
                "拒绝访问，可能缺少管理员权限或文件只读 / Access denied; administrator rights or a read-only file"
            }
            ReplaceKind::Other => "替换失败 / Replacement failed",
        };
        write!(f, "{reason}: {}", self.target.display())
    }
}
impl std::error::Error for ReplaceFailure {}

/// 目标被占用/拒绝访问时的重试预算与间隔。
#[cfg(windows)]
const REPLACE_ATTEMPTS: u32 = 30;
#[cfg(windows)]
const REPLACE_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);
/// 让位文件标记：`.<文件名>.kt-replaced-<时间戳>`，启动时统一回收。
const REPLACE_ASIDE_MARKER: &str = ".kt-replaced-";

/// 统一替换原语：Windows 上目标被占用或拒绝访问时重试，必要时先让位再放入。
pub fn replace_file(source: &Path, target: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        match move_file(source, target) {
            Ok(()) => return Ok(()),
            Err(kind @ ReplaceKind::Other) => {
                return Err(anyhow::Error::new(ReplaceFailure {
                    target: target.to_path_buf(),
                    kind,
                }))
            }
            Err(kind) => {
                // 旧文件先改名让位：运行中的程序可以改名，但不能被覆盖。
                let aside = replace_aside_path(target)?;
                if let Err(aside_kind) = move_file(target, &aside) {
                    return Err(anyhow::Error::new(ReplaceFailure {
                        target: target.to_path_buf(),
                        // 让位失败时以原有分类上报，除非让位给出了更具体的占用信息。
                        kind: if aside_kind == ReplaceKind::Other {
                            kind
                        } else {
                            aside_kind
                        },
                    }));
                }
                if let Err(kind) = move_file(source, target) {
                    // 放不回新文件时尽力恢复旧文件，避免留下空路径。
                    let _ = move_file(&aside, target);
                    return Err(anyhow::Error::new(ReplaceFailure {
                        target: target.to_path_buf(),
                        kind,
                    }));
                }
                return Ok(());
            }
        }
    }
    #[cfg(not(windows))]
    {
        fs::rename(source, target)?;
        Ok(())
    }
}

/// 回收 `replace_file` 留下的让位文件；仍被占用时保留到下次启动。
pub fn cleanup_replace_leftovers(directory: &Path) -> Result<()> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.')
            && name.contains(REPLACE_ASIDE_MARKER)
            && entry.file_type().is_ok_and(|file_type| file_type.is_file())
        {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn replace_aside_path(target: &Path) -> Result<PathBuf> {
    let name = target
        .file_name()
        .context("替换目标缺少文件名 / Replacement target has no name")?
        .to_string_lossy()
        .into_owned();
    let directory = target.parent().unwrap_or(Path::new("."));
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    for index in 0.. {
        let suffix = if index == 0 {
            String::new()
        } else {
            format!("-{index}")
        };
        let candidate = directory.join(format!(".{name}{REPLACE_ASIDE_MARKER}{stamp}{suffix}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    unreachable!("递增后缀总能找到可用的让位路径")
}

#[cfg(windows)]
fn move_file(source: &Path, target: &Path) -> std::result::Result<(), ReplaceKind> {
    use std::{os::windows::ffi::OsStrExt, time::Instant};
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING};
    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<u16>>()
    };
    let source = wide(source);
    let target = wide(target);
    let deadline = Instant::now() + REPLACE_RETRY_DELAY * REPLACE_ATTEMPTS;
    loop {
        let moved =
            unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_REPLACE_EXISTING) };
        if moved != 0 {
            return Ok(());
        }
        let kind = ReplaceKind::from_os_error(std::io::Error::last_os_error().raw_os_error());
        if kind == ReplaceKind::Other || Instant::now() >= deadline {
            return Err(kind);
        }
        std::thread::sleep(REPLACE_RETRY_DELAY);
    }
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
/// 更新辅助程序的固定路径；`validate_task` 与提权入口共用。
pub fn helper_path(root: &Path) -> PathBuf {
    root.join(if cfg!(windows) {
        "update-helper.exe"
    } else {
        "update-helper"
    })
}
/// 提权替换：辅助程序在此入口只做文件交换，不启动 GUI，也不做其他工作。
#[cfg(windows)]
fn run_elevated_replace(root: &Path, _task: &InstallTask) -> Result<()> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{GetExitCodeProcess, WaitForSingleObject},
        UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW},
    };
    const ELEVATED_TIMEOUT_MS: u32 = 5 * 60 * 1000;
    let helper = helper_path(root);
    ensure!(
        helper.is_file(),
        "缺少更新辅助程序 / Update helper is missing"
    );
    let wide = |s: &OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<_>>();
    let file = wide(helper.as_os_str());
    let verb = wide(OsStr::new("runas"));
    let params = wide(OsStr::new("--elevated-replace"));
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = 0;
    ensure!(
        unsafe { ShellExecuteExW(&mut info) } != 0,
        "无法请求管理员授权替换应用 / Cannot request administrator approval"
    );
    let wait = unsafe { WaitForSingleObject(info.hProcess, ELEVATED_TIMEOUT_MS) };
    let mut code = 1;
    let read = unsafe { GetExitCodeProcess(info.hProcess, &mut code) };
    unsafe {
        CloseHandle(info.hProcess);
    }
    ensure!(
        wait == 0 && read != 0 && code == 0,
        "提权替换失败或被取消 / Elevated replacement failed or was cancelled ({code})"
    );
    Ok(())
}
#[cfg(not(windows))]
fn run_elevated_replace(_: &Path, _: &InstallTask) -> Result<()> {
    anyhow::bail!("此平台不支持提权替换 / Elevated replacement unsupported")
}
#[cfg(windows)]
fn run_installer(root: &Path, task: &InstallTask) -> Result<()> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{GetExitCodeProcess, WaitForSingleObject},
        UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW},
    };
    let setup = root.join("update-setup.exe");
    fs::copy(package_path(root, &task.package), &setup).with_context(|| {
        format!(
            "写入安装器失败 / Cannot stage installer: {}",
            setup.display()
        )
    })?;
    let wide = |s: &OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<_>>();
    let file = wide(setup.as_os_str());
    let verb = wide(OsStr::new("runas"));
    let folder = task
        .installation
        .executable
        .parent()
        .unwrap()
        .to_str()
        .context("安装目录编码无效 / Invalid install path")?;
    ensure!(!folder.contains('"'), "安装路径无效 / Invalid install path");
    let params = wide(OsStr::new(&format!(
        "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /DIR=\"{folder}\""
    )));
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = 1;
    ensure!(
        unsafe { ShellExecuteExW(&mut info) } != 0,
        "无法启动安装器或 UAC 被取消 / Installer launch or UAC cancelled"
    );
    let wait = unsafe { WaitForSingleObject(info.hProcess, 10 * 60 * 1000) };
    let mut code = 1;
    let read = unsafe { GetExitCodeProcess(info.hProcess, &mut code) };
    unsafe {
        CloseHandle(info.hProcess);
    }
    ensure!(
        wait == 0 && read != 0 && code == 0,
        "安装器失败 / Installer failed ({code})"
    );
    Ok(())
}
#[cfg(not(windows))]
fn run_installer(_: &Path, _: &InstallTask) -> Result<()> {
    anyhow::bail!("此平台不支持 Windows 安装器 / Windows installer unsupported")
}

/// 提权操作只接受由管理员登记的 Inno Setup 安装目录。
#[cfg(windows)]
pub fn registered_installation(executable: &Path) -> bool {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6432KEY,
        RRF_SUBKEY_WOW6464KEY,
    };
    if executable
        .file_name()
        .is_none_or(|n| n != "kitonyterms.exe")
    {
        return false;
    }
    let wide = |s: &OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<_>>();
    let key = wide(OsStr::new(
        "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\KitonyTerms_is1",
    ));
    let value = wide(OsStr::new("InstallLocation"));
    for view in [RRF_SUBKEY_WOW6432KEY, RRF_SUBKEY_WOW6464KEY] {
        let mut buffer = vec![0u16; 4096];
        let mut size = (buffer.len() * 2) as u32;
        let result = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ | view,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if result == 0 && size >= 2 && size as usize <= buffer.len() * 2 {
            let text = String::from_utf16_lossy(&buffer[..(size as usize / 2)]);
            if fs::canonicalize(Path::new(text.trim_end_matches('\0')))
                .ok()
                .as_deref()
                == executable.parent()
            {
                return true;
            }
        }
    }
    false
}
#[cfg(not(windows))]
pub fn registered_installation(_: &Path) -> bool {
    false
}
#[cfg(test)]
mod tests {
    use super::*;
    fn task(dir: &Path) -> InstallTask {
        let exe = dir.join("kitonyterms");
        fs::write(&exe, b"old").unwrap();
        let current = BuildInfo::current();
        InstallTask {
            installation: Installation::from_executable(exe).unwrap(),
            current: current.clone(),
            next: current,
            source_sha256: hex::encode(sha2::Sha256::digest(b"old")),
            package: Package {
                name: "kitonyterms-test.zip".into(),
                size: 1,
                sha256: "0".repeat(64),
                kind: PackageKind::Zip,
            },
            parent_pid: 1,
            elevate: false,
        }
    }
    use sha2::Digest;
    #[test]
    fn interrupted_replace_restores_and_confirms() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("updates");
        private_root(&root).unwrap();
        let task = task(dir.path());
        let staging = tempfile::Builder::new()
            .prefix(".kt-update-stage-")
            .tempdir_in(dir.path())
            .unwrap()
            .keep();
        let mut j = Journal {
            task: task.clone(),
            staging,
            staged_sha256: String::new(),
            stage: Stage::Prepared,
        };
        atomic_json(&root.join("journal.json"), &j).unwrap();
        // 覆盖备份后、阶段记录前被中断。
        fs::rename(&task.installation.target, task.installation.backup()).unwrap();
        rollback(&root).unwrap();
        assert_eq!(fs::read(&task.installation.target).unwrap(), b"old");
        let staging = tempfile::Builder::new()
            .prefix(".kt-update-stage-")
            .tempdir_in(dir.path())
            .unwrap()
            .keep();
        j.staging = staging;
        j.stage = Stage::Committed;
        fs::copy(&task.installation.target, task.installation.backup()).unwrap();
        atomic_json(&root.join("journal.json"), &j).unwrap();
        recover(&root, &task.next, &task.installation.executable, false).unwrap();
        assert!(task.installation.backup().exists());
        recover(&root, &task.next, &task.installation.executable, true).unwrap();
        assert!(!task.installation.backup().exists());
        assert!(!root.join("journal.json").exists());
    }
    #[test]
    fn bad_backup_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("updates");
        private_root(&root).unwrap();
        let task = task(dir.path());
        fs::write(task.installation.backup(), b"unrelated").unwrap();
        let j = Journal {
            task: task.clone(),
            staging: dir.path().join(".kt-update-stage-test"),
            staged_sha256: String::new(),
            stage: Stage::BackedUp,
        };
        atomic_json(&root.join("journal.json"), &j).unwrap();
        assert!(rollback(&root).is_err());
        assert_eq!(fs::read(&task.installation.target).unwrap(), b"old");
    }
    #[test]
    fn interrupted_after_replace_restores_original() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("updates");
        private_root(&root).unwrap();
        let task = task(dir.path());
        fs::copy(&task.installation.target, task.installation.backup()).unwrap();
        fs::write(&task.installation.target, b"new but uncommitted").unwrap();
        let staging = tempfile::Builder::new()
            .prefix(".kt-update-stage-")
            .tempdir_in(dir.path())
            .unwrap()
            .keep();
        let j = Journal {
            task: task.clone(),
            staging,
            staged_sha256: String::new(),
            stage: Stage::BackedUp,
        };
        atomic_json(&root.join("journal.json"), &j).unwrap();
        recover(&root, &task.current, &task.installation.executable, false).unwrap();
        assert_eq!(fs::read(&task.installation.target).unwrap(), b"old");
        assert!(!root.join("journal.json").exists());
        assert!(root.join("error.json").exists());
    }
    #[test]
    fn partial_backup_and_invalid_journal_never_replace_current() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("updates");
        private_root(&root).unwrap();
        let task = task(dir.path());
        fs::write(task.installation.backup(), b"partial").unwrap();
        let staging = tempfile::Builder::new()
            .prefix(".kt-update-stage-")
            .tempdir_in(dir.path())
            .unwrap()
            .keep();
        let mut j = Journal {
            task: task.clone(),
            staging,
            staged_sha256: String::new(),
            stage: Stage::Prepared,
        };
        atomic_json(&root.join("journal.json"), &j).unwrap();
        rollback(&root).unwrap();
        assert_eq!(fs::read(&task.installation.target).unwrap(), b"old");
        assert!(!task.installation.backup().exists());
        j.staging = dir.path().join("unrelated");
        atomic_json(&root.join("journal.json"), &j).unwrap();
        assert!(rollback(&root).is_err());
        assert_eq!(fs::read(&task.installation.target).unwrap(), b"old");
    }
    #[test]
    fn atomic_save_failure_is_visible() {
        let dir = tempfile::tempdir().unwrap();
        assert!(atomic_json(&dir.path().join("missing/settings.json"), &true).is_err());
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn signed_dmg_bundle_swap_rollback_and_startup_confirmation() {
        use std::process::Command;
        let _guard = archive::DMG_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("updates");
        private_root(&root).unwrap();
        let app = dir.path().join("KitonyTerms.app");
        let source = dir.path().join("source/KitonyTerms.app");
        for (bundle, resource) in [(&app, b"old"), (&source, b"new")] {
            fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
            fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
            fs::copy(
                std::env::current_exe().unwrap(),
                bundle.join("Contents/MacOS/kitonyterms"),
            )
            .unwrap();
            fs::write(bundle.join("Contents/Resources/version"), resource).unwrap();
            std::os::unix::fs::symlink("version", bundle.join("Contents/Resources/current"))
                .unwrap();
            fs::write(bundle.join("Contents/Info.plist"), r#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>com.kitonyterms.app</string><key>CFBundleExecutable</key><string>kitonyterms</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>"#).unwrap();
            let binary_signature = Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-"])
                .arg(bundle.join("Contents/MacOS/kitonyterms"))
                .output()
                .unwrap();
            assert!(
                binary_signature.status.success(),
                "{}",
                String::from_utf8_lossy(&binary_signature.stderr)
            );
            let output = Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-"])
                .arg(bundle)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let verified = Command::new("/usr/bin/codesign")
                .args(["--verify", "--deep", "--strict", "--verbose=4"])
                .arg(bundle)
                .output()
                .unwrap();
            assert!(
                verified.status.success(),
                "source bundle: {}",
                String::from_utf8_lossy(&verified.stderr)
            );
        }
        let image = dir.path().join("bundle.dmg");
        std::os::unix::fs::symlink(
            "/Applications",
            source.parent().unwrap().join("Applications"),
        )
        .unwrap();
        archive::create_test_image(source.parent().unwrap(), &image).unwrap();
        let package = Package {
            name: "kitonyterms-test-macos.dmg".into(),
            size: fs::metadata(&image).unwrap().len(),
            sha256: download::sha256(&image).unwrap(),
            kind: PackageKind::Dmg,
        };
        fs::copy(&image, package_path(&root, &package)).unwrap();
        let installation =
            Installation::from_executable(app.join("Contents/MacOS/kitonyterms")).unwrap();
        assert!(installation.bundle);
        let current = BuildInfo::current();
        let task = InstallTask {
            source_sha256: download::sha256(&installation.executable).unwrap(),
            installation,
            current: current.clone(),
            next: current,
            package,
            parent_pid: 1,
            elevate: false,
        };
        apply(&root, &task).unwrap();
        assert!(!root.join(".dmg-mount").exists());
        assert_eq!(
            fs::read_link(app.join("Contents/Resources/current")).unwrap(),
            Path::new("version")
        );
        assert_eq!(
            fs::read(app.join("Contents/Resources/version")).unwrap(),
            b"new"
        );
        rollback(&root).unwrap();
        assert_eq!(
            fs::read(app.join("Contents/Resources/version")).unwrap(),
            b"old"
        );
        apply(&root, &task).unwrap();
        assert!(!root.join(".dmg-mount").exists());
        recover(&root, &task.next, &task.installation.executable, false).unwrap();
        assert!(task.installation.backup().exists());
        recover(&root, &task.next, &task.installation.executable, true).unwrap();
        assert!(!task.installation.backup().exists());
        assert_eq!(
            fs::read(app.join("Contents/Resources/version")).unwrap(),
            b"new"
        );
    }

    #[test]
    fn replace_kind_classifies_windows_errors() {
        assert_eq!(
            ReplaceKind::from_os_error(Some(5)),
            ReplaceKind::AccessDenied
        );
        assert_eq!(ReplaceKind::from_os_error(Some(32)), ReplaceKind::InUse);
        assert_eq!(ReplaceKind::from_os_error(Some(33)), ReplaceKind::InUse);
        assert_eq!(ReplaceKind::from_os_error(Some(2)), ReplaceKind::Other);
        assert_eq!(ReplaceKind::from_os_error(None), ReplaceKind::Other);
    }

    #[test]
    fn replace_aside_path_is_hidden_and_unique() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("kitonyterms");
        std::fs::write(&target, b"current").unwrap();
        let first = replace_aside_path(&target).unwrap();
        assert_eq!(first.parent(), Some(dir.path()));
        let name = first.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with(".kitonyterms.kt-replaced-"), "{name}");
        std::fs::write(&first, b"moved").unwrap();
        assert_ne!(replace_aside_path(&target).unwrap(), first);
        // 没有文件名的路径无法构造让位名称。
        assert!(replace_aside_path(Path::new("/")).is_err());
    }

    #[test]
    fn cleanup_removes_only_replacement_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        let leftover = dir.path().join(".kitonyterms.kt-replaced-123");
        std::fs::write(&leftover, b"moved").unwrap();
        std::fs::write(dir.path().join("kitonyterms"), b"app").unwrap();
        std::fs::write(dir.path().join(".env"), b"secret").unwrap();
        let nested = dir.path().join(".kt-replaced-dir");
        std::fs::create_dir(&nested).unwrap();
        cleanup_replace_leftovers(dir.path()).unwrap();
        assert!(!leftover.exists());
        assert!(dir.path().join("kitonyterms").exists());
        assert!(dir.path().join(".env").exists());
        // 只回收文件：同名目录保留，避免误删用户数据。
        assert!(nested.exists());
    }

    #[test]
    fn replace_file_swaps_contents_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("payload");
        std::fs::write(&target, b"old").unwrap();
        let source = dir.path().join("next");
        std::fs::write(&source, b"new").unwrap();
        replace_file(&source, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(!source.exists());
    }

    #[test]
    fn writable_directory_is_classified_without_elevation() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("kitonyterms");
        fs::write(&exe, b"app").unwrap();
        let installation = Installation::from_executable(exe).unwrap();
        assert_eq!(installation.access(), Access::Writable);
        assert!(!installation.needs_elevation());
        assert!(installation.preflight().is_ok());
    }

    #[test]
    fn elevated_preparation_checks_directory_before_creating_task() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("updates");
        private_root(&root).unwrap();
        let application = dir.path().join("app");
        fs::create_dir(&application).unwrap();
        let mut task = task(&application);
        task.current.channel = crate::metadata::Channel::Stable;
        task.next = task.current.clone();
        let available = Available {
            info: task.next.clone(),
            package: task.package.clone(),
            asset_id: 1,
            release_id: 1,
            tag: format!("v{}", task.next.version),
            notes: String::new(),
            notes_truncated: false,
            published_at: None,
        };
        // 安装目录在下载后失效：提权选项也必须先完成目录检查。
        fs::remove_dir_all(&application).unwrap();
        for elevate in [false, true] {
            let error = prepare(
                &root,
                &task.installation,
                &task.current,
                &available,
                elevate,
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains("Install folder is not writable"), "{error}");
            assert!(!root.join("task.json").exists());
            assert!(!root.join("journal.json").exists());
        }
    }
}
