//! Windows 授权入口、参数引用与结果传递；协调进程始终保持原权限。
use super::*;
use std::ffi::OsString;
#[cfg(windows)]
use std::time::Duration;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Replace,
    Restore,
}
impl Operation {
    pub fn argument(self) -> &'static str {
        match self {
            Self::Replace => "--elevated-replace",
            Self::Restore => "--restore-update",
        }
    }
}
#[cfg(windows)]
#[derive(Debug, Serialize, Deserialize)]
struct Outcome {
    operation: Operation,
    journal_sha256: String,
    error: Option<String>,
}

/// 管理员入口参数显式携带原用户事务位置及快照身份。
pub fn parse_arguments(args: &[OsString]) -> Result<(Operation, PathBuf, String)> {
    ensure!(
        args.len() == 3,
        "管理员入口参数不完整 / Incomplete elevated arguments"
    );
    let operation = if args[0] == Operation::Replace.argument() {
        Operation::Replace
    } else if args[0] == Operation::Restore.argument() {
        Operation::Restore
    } else {
        anyhow::bail!("无效管理员操作 / Invalid elevated operation");
    };
    let root = PathBuf::from(&args[1]);
    let digest = args[2]
        .to_str()
        .context("无效事务摘要 / Invalid transaction digest")?;
    ensure!(
        root.is_absolute()
            && digest.len() == 64
            && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "无效事务位置或摘要 / Invalid transaction location or digest"
    );
    Ok((operation, root, digest.to_string()))
}

pub fn request(root: &Path, operation: Operation) -> Result<()> {
    #[cfg(windows)]
    {
        let root = fs::canonicalize(root)?;
        let digest = download::sha256(&root.join("journal.json"))?;
        remove(&root.join("elevated-result.json"))?;
        let arguments = [
            OsString::from(operation.argument()),
            root.as_os_str().to_owned(),
            OsString::from(&digest),
        ];
        let result = run(&helper_path(&root), &arguments, Duration::from_secs(300));
        if root.join("elevated-result.json").exists() {
            let outcome: Outcome = read_json(&root.join("elevated-result.json"))?;
            ensure!(
                outcome.operation == operation && outcome.journal_sha256 == digest,
                "管理员结果身份不匹配 / Elevated result identity mismatch"
            );
            if let Some(error) = outcome.error {
                return Err(anyhow::anyhow!(
                    "管理员操作失败 / Elevated operation failed: {error}"
                ));
            }
        } else if result.is_ok() {
            anyhow::bail!("管理员程序未返回结果 / Elevated helper returned no result");
        }
        result
    }
    #[cfg(not(windows))]
    {
        let _ = (root, operation);
        anyhow::bail!("此平台不支持提权替换 / Elevated replacement unsupported");
    }
}

/// 先把入口绑定到正在运行的辅助副本，再允许向原用户目录写结果。
pub fn perform(root: &Path, digest: &str, operation: Operation) -> Result<()> {
    #[cfg(windows)]
    {
        ensure!(
            root.is_absolute()
                && fs::canonicalize(root)? == root
                && fs::canonicalize(std::env::current_exe()?)? == helper_path(root),
            "管理员入口位置不匹配 / Elevated entry location mismatch"
        );
        let _directories = pin_directories(root, &[])?;
        let result = (|| {
            // 禁止事务在摘要检查和文件交换之间被改写/删除。
            let _record = pin_file(&root.join("journal.json"))?;
            ensure!(
                download::sha256(&root.join("journal.json"))? == digest,
                "事务记录已变化 / Transaction record changed"
            );
            let journal: Journal = read_json(&root.join("journal.json"))?;
            validate_request(root, &journal, &std::env::current_exe()?, operation)?;
            let _targets = pin_directories(
                root,
                &[
                    &journal.task.installation.target,
                    &journal.staging.join("payload"),
                ],
            )?;
            let source = match operation {
                Operation::Replace => journal.staging.join("payload"),
                Operation::Restore => {
                    backup_path(root, &journal.task.installation, journal.storage)
                }
            };
            let _source = pin_file(&source)?;
            let expected = match operation {
                Operation::Replace => &journal.staged_sha256,
                Operation::Restore => &journal.task.source_sha256,
            };
            ensure!(
                download::sha256(&source)? == *expected,
                "管理员载荷校验失败 / Elevated payload checksum mismatch"
            );
            replace_payload(&source, &journal.task.installation)?;
            ensure!(
                download::sha256(&journal.task.installation.target)? == *expected,
                "管理员替换结果校验失败 / Elevated replacement checksum mismatch"
            );
            Ok(())
        })();
        atomic_json(
            &root.join("elevated-result.json"),
            &Outcome {
                operation,
                journal_sha256: digest.to_string(),
                error: result
                    .as_ref()
                    .err()
                    .map(|error: &anyhow::Error| format!("{error:#}")),
            },
        )?;
        result
    }
    #[cfg(not(windows))]
    {
        let _ = (root, digest, operation);
        anyhow::bail!("此平台不支持管理员入口 / Elevated entry unsupported");
    }
}

#[cfg(any(windows, test))]
fn validate_request(
    root: &Path,
    journal: &Journal,
    helper: &Path,
    operation: Operation,
) -> Result<()> {
    validate_journal(root, journal)?;
    let task = &journal.task;
    task.package.validate()?;
    ensure!(
        !task.installation.bundle
            && task.current == BuildInfo::current()
            && task.next.target == task.current.target
            && task.next.channel == task.current.channel
            && fs::canonicalize(helper)? == helper_path(root)
            && download::sha256(helper)? == task.source_sha256,
        "管理员辅助程序身份不匹配 / Elevated helper identity mismatch"
    );
    ensure!(
        !task.installation.installed || registered_installation(&task.installation.executable),
        "管理员恢复目标未登记 / Elevated recovery target is not registered"
    );
    match operation {
        Operation::Replace => {
            ensure!(
                task.elevate
                    && !task.installation.installed
                    && journal.stage == Stage::BackedUp
                    && journal.staged_sha256.len() == 64,
                "提权替换状态无效 / Invalid elevated replacement state"
            );
            ensure!(
                download::sha256(&task.installation.executable)? == task.source_sha256,
                "安装目标已变化 / Installation changed"
            );
            download::verify(&package_path(root, &task.package), &task.package)?;
        }
        Operation::Restore => {
            ensure!(
                matches!(journal.stage, Stage::BackedUp | Stage::Committed),
                "提权恢复状态无效 / Invalid elevated recovery state"
            );
        }
    }
    Ok(())
}

/// 参数引用遵循 Windows CommandLineToArgvW 的反斜线/双引号规则，保留 UTF-16。
#[cfg(any(windows, test))]
fn quote_units(value: &[u16]) -> Result<Vec<u16>> {
    ensure!(!value.contains(&0), "参数包含 NUL / Argument contains NUL");
    let mut result = vec![b'"' as u16];
    let mut slashes = 0;
    for &unit in value {
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        result.extend(std::iter::repeat_n(
            b'\\' as u16,
            if unit == b'"' as u16 {
                slashes * 2 + 1
            } else {
                slashes
            },
        ));
        result.push(unit);
        slashes = 0;
    }
    result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    result.push(b'"' as u16);
    Ok(result)
}

/// 一个进程句柄只有在已确认子进程退出后才释放给调用方；超时不并发回滚。
#[cfg(windows)]
pub(super) fn run(file: &Path, arguments: &[OsString], budget: Duration) -> Result<()> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt, time::Instant};
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, ERROR_CANCELLED, HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        System::Com::{
            CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
        },
        System::Threading::{GetExitCodeProcess, WaitForSingleObject},
        UI::Shell::{
            ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
        },
    };
    struct Com;
    impl Drop for Com {
        fn drop(&mut self) {
            unsafe {
                CoUninitialize();
            }
        }
    }
    let initialized = unsafe {
        CoInitializeEx(
            std::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    ensure!(
        initialized >= 0,
        "初始化管理员启动 COM 失败 / Cannot initialize elevation COM (HRESULT {initialized:#x})"
    );
    let _com = Com;
    struct Process(HANDLE);
    impl Drop for Process {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    let wide = |text: &OsStr| -> Result<Vec<u16>> {
        let units = text.encode_wide().collect::<Vec<_>>();
        ensure!(!units.contains(&0), "路径包含 NUL / Path contains NUL");
        Ok(units.into_iter().chain(Some(0)).collect())
    };
    let file = wide(file.as_os_str())?;
    let verb = wide(OsStr::new("runas"))?;
    let mut params = Vec::new();
    for argument in arguments {
        if !params.is_empty() {
            params.push(b' ' as u16);
        }
        params.extend(quote_units(&argument.encode_wide().collect::<Vec<_>>())?);
    }
    params.push(0);
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = 1;
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let error = std::io::Error::last_os_error();
        let message = if error.raw_os_error() == Some(ERROR_CANCELLED as i32) {
            "已取消管理员授权，保留当前版本 / Administrator authorization cancelled; current version retained"
        } else {
            "无法启动管理员程序 / Cannot launch elevated process"
        };
        return Err(anyhow::Error::new(error).context(message));
    }
    ensure!(
        !info.hProcess.is_null(),
        "缺少管理员进程句柄 / Missing elevated process handle"
    );
    let process = Process(info.hProcess);
    let started = Instant::now();
    let mut waiting_error = None;
    let mut timed_out = false;
    loop {
        let wait = unsafe { WaitForSingleObject(process.0, 1000) };
        if wait == WAIT_FAILED && waiting_error.is_none() {
            waiting_error = Some(std::io::Error::last_os_error());
        }
        if started.elapsed() > budget && !timed_out {
            timed_out = true;
            tracing::warn!("管理员程序超时，持有更新锁并等待退出 / Elevated process timed out; waiting for exit before recovery");
        }
        let mut code = 259;
        let read = unsafe { GetExitCodeProcess(process.0, &mut code) };
        if wait == WAIT_OBJECT_0 || (wait != WAIT_TIMEOUT && read != 0 && code != 259) {
            ensure!(
                read != 0,
                "读取管理员退出码失败 / Cannot read elevated exit code: {}",
                std::io::Error::last_os_error()
            );
            if let Some(error) = waiting_error {
                return Err(error).context("管理员进程等待失败 / Elevated process wait failed");
            }
            ensure!(
                !timed_out,
                "管理员程序超时后已退出 / Elevated process exited after timeout ({code})"
            );
            ensure!(
                code == 0,
                "管理员程序失败 / Elevated process failed (exit code {code})"
            );
            return Ok(());
        }
        // 查询异常时也不释放锁给恢复流程；有效句柄正常情况下只会返回活动或退出。
        if wait == WAIT_FAILED {
            std::thread::sleep(Duration::from_millis(1000));
        }
    }
}

#[cfg(windows)]
fn pin_file(path: &Path) -> Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
    reject_reparse(path)?;
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .with_context(|| {
            format!(
                "锁定管理员输入失败 / Cannot pin elevated input: {}",
                path.display()
            )
        })
}

#[cfg(windows)]
fn reject_reparse(path: &Path) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => ensure!(
                metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
                "管理员路径不允许重解析点 / Elevated path contains a reparse point: {}",
                ancestor.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && ancestor == path => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "检查管理员路径失败 / Cannot inspect elevated path: {}",
                        ancestor.display()
                    )
                })
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn pin_directories(root: &Path, paths: &[&Path]) -> Result<Vec<File>> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let mut pinned = Vec::new();
    for path in std::iter::once(root).chain(paths.iter().filter_map(|path| path.parent())) {
        let ancestors = path.ancestors().collect::<Vec<_>>();
        for ancestor in ancestors.into_iter().rev() {
            let directory = OpenOptions::new()
                .access_mode(0)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(ancestor)?;
            ensure!(
                directory.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
                "管理员目录不允许重解析点 / Elevated directory contains a reparse point: {}",
                ancestor.display()
            );
            reject_reparse(ancestor)?;
            pinned.push(directory);
        }
    }
    for path in paths {
        reject_reparse(path)?;
    }
    Ok(pinned)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn elevated_validation_rejects_changed_target_helper_and_wrong_stage() {
        let (_directory, root, mut journal) = super::super::regression::fixture();
        let helper = helper_path(&root);
        fs::write(&helper, b"old").unwrap();
        validate_request(&root, &journal, &helper, Operation::Restore).unwrap();
        journal.task.elevate = true;
        fs::write(package_path(&root, &journal.task.package), b"pkg").unwrap();
        journal.task.package.sha256 =
            download::sha256(&package_path(&root, &journal.task.package)).unwrap();
        fs::write(package_path(&root, &journal.task.package), b"pkg").unwrap();
        validate_request(&root, &journal, &helper, Operation::Replace).unwrap();
        fs::write(&journal.task.installation.target, b"changed").unwrap();
        assert!(validate_request(&root, &journal, &helper, Operation::Replace).is_err());
        validate_request(&root, &journal, &helper, Operation::Restore).unwrap();
        fs::write(&helper, b"wrong").unwrap();
        assert!(validate_request(&root, &journal, &helper, Operation::Restore).is_err());
        fs::write(&helper, b"old").unwrap();
        journal.stage = Stage::Prepared;
        assert!(validate_request(&root, &journal, &helper, Operation::Restore).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn pinned_input_can_be_read_but_cannot_be_modified_or_deleted() {
        let (_directory, root, journal) = super::super::regression::fixture();
        let source = journal.staging.join("payload");
        let _directories = pin_directories(&root, &[&source]).unwrap();
        let pinned = pin_file(&source).unwrap();
        assert_eq!(fs::read(&source).unwrap(), b"new");
        assert!(fs::write(&source, b"changed").is_err());
        assert!(fs::remove_file(&source).is_err());
        drop(pinned);
        fs::write(source, b"changed").unwrap();
    }

    #[test]
    fn quotes_unicode_spaces_quotes_and_trailing_slashes() {
        for (input, expected) in [
            ("", "\"\""),
            ("C:\\用户 目录\\", "\"C:\\用户 目录\\\\\""),
            ("a\"b", "\"a\\\"b\""),
        ] {
            let result = quote_units(&input.encode_utf16().collect::<Vec<_>>()).unwrap();
            assert_eq!(String::from_utf16(&result).unwrap(), expected);
        }
        assert!(quote_units(&[0]).is_err());
    }
    #[test]
    fn elevated_arguments_preserve_the_original_user_root() {
        let root = std::env::temp_dir().join("原用户 更新");
        let args = [
            OsString::from("--restore-update"),
            root.clone().into_os_string(),
            OsString::from("a".repeat(64)),
        ];
        let (operation, actual, digest) = parse_arguments(&args).unwrap();
        assert_eq!(operation, Operation::Restore);
        assert_eq!(actual, root);
        assert_eq!(digest, "a".repeat(64));
        assert!(parse_arguments(&args[..1]).is_err());
        let mut bad = args.clone();
        bad[2] = "wrong".into();
        assert!(parse_arguments(&bad).is_err());
    }
}
