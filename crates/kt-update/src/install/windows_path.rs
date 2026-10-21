//! 只在 Windows Shell/安装器边界转换路径；事务与文件校验保留规范路径。
use anyhow::{ensure, Context, Result};
use std::{
    ffi::OsString,
    os::windows::ffi::OsStrExt,
    path::{Component, Path, PathBuf, Prefix},
};

pub(super) fn external(path: &Path) -> Result<PathBuf> {
    ensure!(
        path.is_absolute(),
        "外部程序路径必须绝对 / External path must be absolute"
    );
    let mut components = path.components();
    let compatible = match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::VerbatimUNC(server, share) => {
                // dunce 只简化盘符路径；用等价的盘符分量校验 UNC 名称，再重建 UNC 前缀。
                let mut probe = PathBuf::from(r"\\?\C:\");
                probe.push(server);
                probe.push(share);
                probe.extend(components.filter(|part| *part != Component::RootDir));
                let simple = dunce::simplified(&probe);
                ensure!(
                    simple != probe,
                    "网络路径无法安全转换 / Cannot safely convert UNC path"
                );
                let mut unc = OsString::from(r"\\");
                unc.push(
                    simple
                        .strip_prefix(r"C:\")
                        .context("无效网络路径 / Invalid UNC path")?,
                );
                PathBuf::from(unc)
            }
            _ => dunce::simplified(path).to_path_buf(),
        },
        _ => anyhow::bail!("缺少 Windows 路径前缀 / Missing Windows path prefix"),
    };
    let units = compatible.as_os_str().encode_wide().collect::<Vec<_>>();
    ensure!(
        matches!(compatible.components().next(), Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::UNC(_, _)))
            && units.len() < 260
            && !units.contains(&0),
        "路径无法安全传给系统安装器，请将应用和更新目录移到较短的普通路径 / Path cannot be safely passed to the system installer; use shorter conventional application and update paths: {}",
        path.display()
    );
    Ok(compatible)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_disk_unc_spaces_and_unicode_at_external_boundary() {
        for (source, expected) in [
            (
                r"\\?\C:\Program Files\中文\kitonyterms.exe",
                r"C:\Program Files\中文\kitonyterms.exe",
            ),
            (
                r"C:\Users\中文 用户\updates\setup.exe",
                r"C:\Users\中文 用户\updates\setup.exe",
            ),
            (
                r"\\?\UNC\server\share\中文 用户\setup.exe",
                r"\\server\share\中文 用户\setup.exe",
            ),
            (r"\\server\share\setup.exe", r"\\server\share\setup.exe"),
        ] {
            assert_eq!(external(Path::new(source)).unwrap(), Path::new(expected));
        }
    }

    #[test]
    fn rejects_paths_whose_meaning_or_length_requires_extended_syntax() {
        for source in [
            r"\\?\C:\folder.\setup.exe",
            r"\\?\C:\NUL\setup.exe",
            r"\\?\UNC\server\share\folder.\setup.exe",
            r"\\.\C:\setup.exe",
            r"relative\setup.exe",
        ] {
            assert!(external(Path::new(source)).is_err(), "{source}");
        }
        for prefix in [r"C:\", r"\\?\C:\", r"\\?\UNC\server\share\"] {
            let long = format!("{prefix}{}\\setup.exe", "目录".repeat(140));
            assert!(external(Path::new(&long)).is_err());
        }
    }
}
