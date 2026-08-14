//! GUI 单实例文件锁。

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

use fs2::FileExt;

/// 持有此值期间，操作系统会保持排他文件锁。
pub struct SingleInstanceLock {
    _file: File,
}

impl SingleInstanceLock {
    /// 获取排他锁；`Ok(None)` 表示已有实例持有同一路径的锁。
    pub fn try_acquire(path: &Path) -> io::Result<Option<Self>> {
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(err) if is_lock_contended(&err) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

fn is_lock_contended(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    matches!(
        (
            error.raw_os_error(),
            fs2::lock_contended_error().raw_os_error()
        ),
        (Some(actual), Some(contended)) if actual == contended
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lock_is_rejected_until_first_lock_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kitonyterms.lock");

        let first = SingleInstanceLock::try_acquire(&path).unwrap().unwrap();
        assert!(SingleInstanceLock::try_acquire(&path).unwrap().is_none());

        drop(first);
        assert!(SingleInstanceLock::try_acquire(&path).unwrap().is_some());
    }
}
