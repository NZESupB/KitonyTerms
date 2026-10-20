//! 更新辅助进程入口与 GUI 握手。
use anyhow::{ensure, Context, Result};
use fs2::FileExt;
use kt_config::Paths;
use kt_update::{
    download,
    install::{self, InstallTask, Installation, Journal, Stage},
    metadata::BuildInfo,
    UpdateService,
};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub fn root(paths: &Paths) -> PathBuf {
    paths.data_dir().join("updates")
}
fn helper_path(root: &Path) -> PathBuf {
    root.join(if cfg!(windows) {
        "update-helper.exe"
    } else {
        "update-helper"
    })
}
fn helper_lock(root: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("helper.lock"))?)
}

pub fn internal_command() -> Option<Result<()>> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args
        .first()
        .is_some_and(|argument| argument == "--elevated-replace" || argument == "--restore-update")
    {
        return Some(install::elevation::parse_arguments(&args).and_then(
            |(operation, root, digest)| install::elevation::perform(&root, &digest, operation),
        ));
    }
    if args.len() != 1 {
        return None;
    }
    if args[0] == "--apply-update" {
        Some(apply_helper())
    } else if args[0] == "--relaunch-update" {
        Some(relaunch_helper())
    } else {
        None
    }
}
pub enum Startup {
    Ready(File),
    Relaunch(File, bool),
}
/// GUI 持有安装锁期间，辅助程序不能替换当前程序。
pub fn startup(paths: &Paths, installation: &Installation) -> Result<Startup> {
    let root = root(paths);
    install::private_root(&root)?;
    let lock = install::gate(&root)?;
    FileExt::try_lock_exclusive(&lock)
        .context("更新或其他应用实例正在运行 / Update or another instance is running")?;
    let helper = helper_lock(&root)?;
    FileExt::try_lock_exclusive(&helper)
        .context("更新正在安装，请稍后启动 / Update is being installed")?;
    #[cfg(not(windows))]
    let before = download::sha256(&installation.executable)?;
    if root.join("task.json").exists() {
        let task: InstallTask = install::read_json(&root.join("task.json"))?;
        ensure!(
            !process_alive(task.parent_pid)?,
            "更新重启仍在等待旧进程 / Update is waiting for the old process"
        );
        ensure!(
            task.installation.executable == installation.executable,
            "更新记录属于其他安装 / Update belongs to another installation"
        );
    }
    if root.join("journal.json").exists() {
        let journal: Journal = install::read_json(&root.join("journal.json"))?;
        ensure!(
            journal.task.installation.executable == installation.executable,
            "安装记录属于其他应用路径 / Transaction belongs to another installation"
        );
        if journal.stage != Stage::Committed || journal.task.next != BuildInfo::current() {
            #[cfg(windows)]
            return Ok(Startup::Relaunch(lock, true));
            #[cfg(not(windows))]
            {
                install::recover(
                    &root,
                    &BuildInfo::current(),
                    &installation.executable,
                    false,
                )?;
                if download::sha256(&installation.executable)? != before {
                    return Ok(Startup::Relaunch(lock, false));
                }
            }
        }
    }
    // 回收上一次替换留下的让位文件；仍被占用时保留到下次启动。
    install::cleanup_replace_leftovers(installation.executable.parent().unwrap_or(Path::new(".")))?;
    cleanup_markers(&root)?;
    Ok(Startup::Ready(lock))
}
pub fn launch(service: &UpdateService, elevate: bool) -> Result<()> {
    let root = &service.root;
    ensure!(
        !root.join("task.json").exists(),
        "辅助程序尚未结束，请稍后重试 / Previous helper still active; retry shortly"
    );
    cleanup_markers(root)?;
    let task = service.prepare(elevate)?;
    let result = (|| {
        install_helper(root, &task)?;
        spawn_ready(root, "--apply-update", &task.source_sha256)
    })();
    if result.is_err() {
        cleanup_markers(root)?;
    }
    result
}
/// 就位辅助程序：先写临时副本，再原子替换目标，避免覆盖被占用或只读的旧文件。
fn install_helper(root: &Path, task: &InstallTask) -> Result<()> {
    let helper = helper_path(root);
    // 残留的辅助程序仍持有 helper.lock：不做替换，给出可操作的提示。
    let lock = helper_lock(root)?;
    FileExt::try_lock_exclusive(&lock).context(
        "上一次更新辅助程序仍在运行，请稍后重试 / Previous update helper is still running; retry shortly",
    )?;
    let staged = root.join(format!(
        "{}.new",
        helper.file_name().unwrap_or_default().to_string_lossy()
    ));
    install::remove(&staged)?;
    install::copy_file_synced(&task.installation.executable, &staged)
        .context("复制更新辅助程序失败 / Cannot copy update helper")?;
    ensure!(
        download::sha256(&staged)? == task.source_sha256,
        "辅助程序复制校验失败 / Helper copy checksum failed"
    );
    if let Err(error) = install::replace_helper_file(&staged, &helper) {
        let _ = install::remove(&staged);
        return Err(error.context("放置更新辅助程序失败 / Cannot place the update helper"));
    }
    Ok(())
}

fn apply_helper() -> Result<()> {
    let paths = Paths::discover()?;
    let root = fs::canonicalize(root(&paths))?;
    let helper_lock = helper_lock(&root)?;
    FileExt::try_lock_exclusive(&helper_lock)?;
    let task: InstallTask = install::read_json(&root.join("task.json"))?;
    install::validate_task(&root, &task, &std::env::current_exe()?)?;
    install::atomic_json(
        &root.join("helper-ready.json"),
        &(std::process::id(), task.source_sha256.clone()),
    )?;
    let result = (|| {
        let deadline = Instant::now() + Duration::from_secs(60);
        while process_alive(task.parent_pid)? {
            if root.join("abort.json").exists() {
                cleanup_markers(&root)?;
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "旧进程退出超时 / Old process exit timed out"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        ensure!(
            root.join("commit.json").exists() && !root.join("abort.json").exists(),
            "重启未确认，取消安装 / Restart not confirmed; installation cancelled"
        );
        let gate = install::gate(&root)?;
        wait_lock(&gate, Duration::from_secs(10))?;
        let instance =
            crate::single_instance::SingleInstanceLock::try_acquire(&paths.instance_lock_file())?
                .context("应用实例尚未退出 / Application instance still running")?;
        let applied = install::apply(&root, &task).and_then(|()| verify_installed(&root, &task));
        if let Err(error) = &applied {
            let rollback = install::rollback(&root);
            if replace_needs_elevation(error, &task.installation.target) {
                install::atomic_json(&root.join("elevation.json"), &true)?;
            }
            let message = match &rollback {
                Ok(()) => format!("更新失败，已保留或恢复旧版本 / Update failed; previous version retained or restored: {error:#}"),
                Err(recovery) => format!("更新失败，恢复未完成 / Update failed; recovery incomplete: {error:#}; {recovery:#}"),
            };
            install::atomic_json(&root.join("error.json"), &message)?;
            rollback.with_context(|| message)?;
        }
        cleanup_markers(&root)?;
        drop(instance);
        drop(gate);
        drop(helper_lock);
        // 辅助进程保持普通权限；仅系统安装器/恢复程序提权，新 GUI 不继承管理员权限。
        Command::new(&task.installation.executable)
            .arg("--gui")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("无法重新启动应用 / Cannot restart application")?;
        Ok(())
    })();
    if let Err(error) = &result {
        install::atomic_json(&root.join("error.json"), &format!("{error:#}"))?;
        cleanup_markers(&root)?;
    }
    result
}
/// 替换失败是否指向「缺少管理员授权」：界面据此提供提权重试入口。
///
/// 仅「拒绝访问」会给出该提示；纯粹的占用冲突由普通重试（带退避）解决，
/// 把提权当成占用问题的解法会让界面给出错误的指引。
fn replace_needs_elevation(error: &anyhow::Error, target: &Path) -> bool {
    error
        .downcast_ref::<install::ReplaceFailure>()
        .is_some_and(|failure| {
            failure.target == target
                && failure.kind == install::ReplaceKind::AccessDenied
                && !fs::metadata(target).is_ok_and(|metadata| metadata.permissions().readonly())
        })
}
fn cleanup_markers(root: &Path) -> Result<()> {
    for name in [
        "task.json",
        "helper-ready.json",
        "commit.json",
        "abort.json",
        "relaunch.json",
    ] {
        install::remove(&root.join(name))?;
    }
    // 上一次就位辅助程序时中途退出留下的临时副本。
    let staged = helper_path(root);
    let staged = staged.with_file_name(format!(
        "{}.new",
        staged.file_name().unwrap_or_default().to_string_lossy()
    ));
    install::remove(&staged)?;
    Ok(())
}
fn wait_lock(lock: &File, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match FileExt::try_lock_exclusive(lock) {
            Ok(()) => return Ok(()),
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
            {
                ensure!(
                    Instant::now() < deadline,
                    "安装锁等待超时 / Installation lock timeout"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.into()),
        }
    }
}
#[cfg(unix)]
fn process_alive(pid: u32) -> Result<bool> {
    ensure!(
        pid > 0 && pid <= i32::MAX as u32,
        "无效进程 ID / Invalid process ID"
    );
    let result = unsafe { libc::kill(pid as i32, 0) };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(false)
    } else {
        Err(error.into())
    }
}
#[cfg(windows)]
fn process_alive(pid: u32) -> Result<bool> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_INVALID_PARAMETER},
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        let e = std::io::Error::last_os_error();
        return if e.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
            Ok(false)
        } else {
            Err(e.into())
        };
    }
    let mut code = 0;
    let result = unsafe { GetExitCodeProcess(handle, &mut code) };
    unsafe {
        CloseHandle(handle);
    }
    ensure!(result != 0, "查询进程失败 / Cannot query process");
    Ok(code == 259)
}

fn verify_installed(root: &Path, task: &InstallTask) -> Result<()> {
    let output_path = root.join("installed-info.json");
    let output = File::create(&output_path)?;
    let mut child = Command::new(&task.installation.executable)
        .arg("--update-info")
        .stdin(Stdio::null())
        .stdout(output)
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if fs::metadata(&output_path)?.len() > 8192 || Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            anyhow::bail!(
                "安装身份验证超时或输出过大 / Installed identity probe timed out or exceeded limit"
            );
        }
        if let Some(status) = child.try_wait()? {
            ensure!(
                status.success(),
                "新程序无法运行 / Updated binary cannot run"
            );
            let info: BuildInfo = install::read_json(&output_path)?;
            ensure!(
                info == task.next,
                "安装后的构建身份不一致 / Installed build identity mismatch"
            );
            install::remove(&output_path)?;
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn is_installing(paths: &Paths) -> Result<bool> {
    let root = root(paths);
    if !root.exists() {
        return Ok(false);
    }
    let lock = helper_lock(&root)?;
    match FileExt::try_lock_exclusive(&lock) {
        Ok(()) => Ok(false),
        Err(e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
        {
            Ok(true)
        }
        Err(e) => Err(e.into()),
    }
}
pub fn relaunch_after_recovery(
    paths: &Paths,
    installation: &Installation,
    restore: bool,
) -> Result<()> {
    let root = root(paths);
    let digest = download::sha256(&helper_path(&root))?;
    if restore {
        let journal: Journal = install::read_json(&root.join("journal.json"))?;
        ensure!(
            journal.task.source_sha256 == digest
                && journal.task.installation.executable == installation.executable,
            "恢复程序身份无效 / Invalid recovery helper identity"
        );
    } else {
        ensure!(
            download::sha256(&installation.executable)? == digest,
            "恢复程序与已恢复的应用不一致 / Recovery helper does not match restored application"
        );
    }
    install::atomic_json(
        &root.join("relaunch.json"),
        &(
            std::process::id(),
            installation.executable.clone(),
            digest.clone(),
            restore,
        ),
    )?;
    if let Err(error) = spawn_ready(&root, "--relaunch-update", &digest) {
        cleanup_markers(&root)?;
        return Err(error);
    }
    Ok(())
}
fn relaunch_helper() -> Result<()> {
    let paths = Paths::discover()?;
    let root = fs::canonicalize(root(&paths))?;
    let helper = helper_lock(&root)?;
    FileExt::try_lock_exclusive(&helper)?;
    let (parent, executable, digest, restore): (u32, PathBuf, String, bool) =
        install::read_json(&root.join("relaunch.json"))?;
    ensure!(
        fs::canonicalize(std::env::current_exe()?)? == helper_path(&root)
            && download::sha256(&helper_path(&root))? == digest,
        "重启程序身份无效 / Invalid relaunch helper identity"
    );
    if restore {
        let journal: Journal = install::read_json(&root.join("journal.json"))?;
        ensure!(
            journal.task.installation.executable == executable
                && journal.task.source_sha256 == digest
                && journal.task.current == BuildInfo::current(),
            "恢复任务身份不一致 / Recovery task identity mismatch"
        );
    } else {
        ensure!(
            download::sha256(&executable)? == digest,
            "已恢复的应用身份不一致 / Restored application checksum mismatch"
        );
    }
    install::atomic_json(
        &root.join("helper-ready.json"),
        &(std::process::id(), digest.clone()),
    )?;
    let deadline = Instant::now() + Duration::from_secs(60);
    while process_alive(parent)? {
        ensure!(
            Instant::now() < deadline,
            "恢复重启超时 / Recovery restart timed out"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let instance =
        crate::single_instance::SingleInstanceLock::try_acquire(&paths.instance_lock_file())?
            .context("应用实例尚未退出 / Application instance still running")?;
    if restore {
        let journal: Journal = install::read_json(&root.join("journal.json"))?;
        ensure!(
            journal.task.installation.executable == executable
                && journal.task.source_sha256 == digest
                && journal.task.current == BuildInfo::current(),
            "恢复任务身份不一致 / Recovery task identity mismatch"
        );
        #[cfg(windows)]
        {
            let gate = install::gate(&root)?;
            wait_lock(&gate, Duration::from_secs(10))?;
            install::rollback(&root)?;
            install::atomic_json(
                &root.join("error.json"),
                &"上次更新被中断，已恢复旧版本 / Interrupted update restored",
            )?;
        }
        #[cfg(not(windows))]
        anyhow::bail!("此平台不使用提权恢复 / Elevated recovery unsupported on this platform");
    }
    ensure!(
        download::sha256(&executable)? == digest,
        "已恢复的应用身份不一致 / Restored application checksum mismatch"
    );
    cleanup_markers(&root)?;
    install::remove(&root.join("relaunch.json"))?;
    drop(instance);
    drop(helper);
    Command::new(executable)
        .arg("--gui")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

fn spawn_ready(root: &Path, argument: &str, digest: &str) -> Result<()> {
    install::remove(&root.join("helper-ready.json"))?;
    let mut child = Command::new(helper_path(root))
        .current_dir(root)
        .arg(argument)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(15);
    let handshake = (|| {
        while Instant::now() < deadline {
            if root.join("helper-ready.json").exists() {
                let identity: (u32, String) = install::read_json(&root.join("helper-ready.json"))?;
                ensure!(
                    identity.0 == child.id() && identity.1 == digest,
                    "辅助程序握手不匹配 / Helper handshake mismatch"
                );
                return Ok(());
            }
            if let Some(status) = child.try_wait()? {
                anyhow::bail!("辅助程序提前退出 / Helper exited: {status}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        anyhow::bail!("辅助程序握手超时 / Helper handshake timed out")
    })();
    if handshake.is_err() && child.try_wait()?.is_none() {
        child.kill()?;
        child.wait()?;
    }
    handshake
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helper_copy_flushes_verifies_and_respects_the_running_helper_lock() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("updates");
        install::private_root(&root).unwrap();
        let executable = directory.path().join("application");
        fs::write(&executable, b"current binary").unwrap();
        let current = BuildInfo::current();
        let task = InstallTask {
            installation: Installation::from_executable(executable.clone()).unwrap(),
            current: current.clone(),
            next: current,
            source_sha256: download::sha256(&executable).unwrap(),
            package: kt_update::metadata::Package {
                name: "kitonyterms-test.zip".into(),
                size: 1,
                sha256: "0".repeat(64),
                kind: kt_update::metadata::PackageKind::Zip,
            },
            parent_pid: std::process::id(),
            elevate: false,
        };
        install_helper(&root, &task).unwrap();
        assert_eq!(fs::read(helper_path(&root)).unwrap(), b"current binary");
        install_helper(&root, &task).unwrap();
        let locked = helper_lock(&root).unwrap();
        FileExt::try_lock_exclusive(&locked).unwrap();
        assert!(install_helper(&root, &task)
            .unwrap_err()
            .to_string()
            .contains("still running"));
        assert_eq!(fs::read(helper_path(&root)).unwrap(), b"current binary");
    }

    #[test]
    fn helper_copy_failures_do_not_become_target_elevation_requests() {
        let error = anyhow::Error::new(std::io::Error::from_raw_os_error(5))
            .context("复制更新辅助程序失败");
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("application.exe");
        fs::write(&target, b"current binary").unwrap();
        assert!(!replace_needs_elevation(&error, &target));
        for (kind, expected) in [
            (install::ReplaceKind::AccessDenied, true),
            (install::ReplaceKind::InUse, false),
        ] {
            let error = anyhow::Error::new(install::ReplaceFailure {
                target: target.clone(),
                kind,
                source: std::io::Error::from_raw_os_error(if expected { 5 } else { 32 }),
            });
            assert_eq!(replace_needs_elevation(&error, &target), expected);
            assert!(!replace_needs_elevation(
                &error,
                &directory.path().join("another.exe")
            ));
        }
        let mut permissions = fs::metadata(&target).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&target, permissions).unwrap();
        let error = anyhow::Error::new(install::ReplaceFailure {
            target: target.clone(),
            kind: install::ReplaceKind::AccessDenied,
            source: std::io::Error::from_raw_os_error(5),
        });
        assert!(!replace_needs_elevation(&error, &target));
        install::remove(&target).unwrap();
    }

    #[test]
    fn process_exit_and_lock_timeout() {
        assert!(process_alive(std::process::id()).unwrap());
        let d = tempfile::tempdir().unwrap();
        let a = install::gate(d.path()).unwrap();
        let b = install::gate(d.path()).unwrap();
        FileExt::try_lock_exclusive(&a).unwrap();
        assert!(wait_lock(&b, Duration::from_millis(10)).is_err());
        drop(a);
        wait_lock(&b, Duration::from_millis(10)).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn observes_mock_child_exit() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let id = child.id();
        child.wait().unwrap();
        assert!(!process_alive(id).unwrap());
    }
    #[test]
    fn marker_cleanup_preserves_journal_and_preferences() {
        let d = tempfile::tempdir().unwrap();
        for name in [
            "task.json",
            "helper-ready.json",
            "commit.json",
            "abort.json",
            "journal.json",
            "preferences.json",
        ] {
            fs::write(d.path().join(name), b"{}").unwrap();
        }
        cleanup_markers(d.path()).unwrap();
        assert!(!d.path().join("task.json").exists());
        assert!(d.path().join("journal.json").exists());
        assert!(d.path().join("preferences.json").exists());
    }
    #[cfg(unix)]
    fn script(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn handshake_mismatch_terminates_helper() {
        let dir = tempfile::tempdir().unwrap();
        script(
            &helper_path(dir.path()),
            "printf '[%s,\"wrong\"]' \"$$\" > helper-ready.json\nexec /bin/sleep 10",
        );
        assert!(spawn_ready(dir.path(), "--apply-update", "expected").is_err());
        let (pid, _): (u32, String) =
            install::read_json(&dir.path().join("helper-ready.json")).unwrap();
        assert!(!process_alive(pid).unwrap());
    }
    #[cfg(unix)]
    #[test]
    fn installed_probe_checks_exact_identity_and_exit_status() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("kitonyterms");
        let info = BuildInfo::current();
        script(
            &executable,
            &format!("printf '%s' '{}'", serde_json::to_string(&info).unwrap()),
        );
        let mut task = InstallTask {
            installation: Installation::from_executable(executable.clone()).unwrap(),
            current: info.clone(),
            next: info,
            source_sha256: download::sha256(&executable).unwrap(),
            package: kt_update::metadata::Package {
                name: "kitonyterms-test.zip".into(),
                size: 1,
                sha256: "0".repeat(64),
                kind: kt_update::metadata::PackageKind::Zip,
            },
            parent_pid: 1,
            elevate: false,
        };
        verify_installed(dir.path(), &task).unwrap();
        task.next.version = "99.0.0".into();
        assert!(verify_installed(dir.path(), &task).is_err());
        script(&executable, "exit 1");
        assert!(verify_installed(dir.path(), &task).is_err());
    }
}
