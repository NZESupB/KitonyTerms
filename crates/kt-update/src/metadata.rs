//! 发布身份及更新清单协议。
use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_PACKAGE: u64 = 512 * 1024 * 1024;
pub const MAX_EXPANDED: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Alpha,
    Development,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildInfo {
    pub channel: Channel,
    pub version: String,
    pub commit: String,
    pub build: u64,
    pub attempt: u64,
    pub target: String,
}
impl BuildInfo {
    pub fn current() -> Self {
        let channel = match env!("KT_RELEASE_CHANNEL") {
            "stable" => Channel::Stable,
            "alpha" => Channel::Alpha,
            _ => Channel::Development,
        };
        let mut info = Self {
            channel,
            version: env!("CARGO_PKG_VERSION").into(),
            commit: env!("KT_RELEASE_COMMIT").into(),
            build: env!("KT_RELEASE_BUILD").parse().unwrap_or(0),
            attempt: env!("KT_RELEASE_ATTEMPT").parse().unwrap_or(0),
            target: env!("KT_UPDATE_TARGET").into(),
        };
        if info.build == 0 || info.attempt == 0 || !valid_commit(&info.commit) {
            info.channel = Channel::Development;
        }
        info
    }
    pub fn label(&self) -> String {
        match self.channel {
            Channel::Stable => format!("v{}", self.version),
            Channel::Alpha => format!("v{} Alpha {}.{}", self.version, self.build, self.attempt),
            Channel::Development => format!("v{} (development)", self.version),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageKind {
    Zip,
    TarGz,
    Dmg,
    Installer,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub kind: PackageKind,
}
impl Package {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.size > 0 && self.size <= MAX_PACKAGE,
            "更新包大小无效 / Invalid package size"
        );
        ensure!(
            self.name.starts_with("kitonyterms-")
                && !self.name.contains(['/', '\\', ':'])
                && !self.name.contains(".."),
            "更新包名称无效 / Invalid asset name"
        );
        ensure!(
            self.sha256.len() == 64 && self.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "SHA-256 无效 / Invalid digest"
        );
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetPackages {
    pub archive: Package,
    pub installer: Option<Package>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub channel: Channel,
    pub version: String,
    pub commit: String,
    pub build: u64,
    pub attempt: u64,
    pub targets: BTreeMap<String, TargetPackages>,
}
impl Manifest {
    pub fn newer_than(&self, current: &BuildInfo, tag: &str) -> Result<bool> {
        ensure!(
            self.schema == 1,
            "不支持的更新协议 / Unsupported update schema"
        );
        ensure!(
            current.channel != Channel::Development,
            "开发构建不支持更新 / Development builds cannot update"
        );
        ensure!(
            self.channel == current.channel,
            "更新渠道不匹配 / Channel mismatch"
        );
        ensure!(
            valid_commit(&self.commit) && self.build > 0 && self.attempt > 0,
            "缺少发布身份 / Invalid release identity"
        );
        let version = semver::Version::parse(&self.version)?;
        match current.channel {
            Channel::Stable => {
                ensure!(
                    tag == format!("v{}", self.version) && version.pre.is_empty(),
                    "正式发布版本不一致 / Stable release mismatch"
                );
                Ok(version > semver::Version::parse(&current.version)?)
            }
            Channel::Alpha => {
                ensure!(tag == "alpha", "Alpha 标签不匹配 / Alpha tag mismatch");
                Ok((self.build, self.attempt) > (current.build, current.attempt))
            }
            Channel::Development => bail!("开发构建不支持更新 / Development build"),
        }
    }
    pub fn package(&self, target: &str, installed: bool) -> Result<Package> {
        let packages = self
            .targets
            .get(target)
            .ok_or_else(|| anyhow::anyhow!("缺少当前平台更新包 / No package for {target}"))?;
        let package = if installed {
            packages
                .installer
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("缺少安装包 / No installer"))?
        } else {
            &packages.archive
        };
        let expected = if installed {
            PackageKind::Installer
        } else if target.contains("linux") {
            PackageKind::TarGz
        } else if target.contains("apple") {
            PackageKind::Dmg
        } else {
            PackageKind::Zip
        };
        ensure!(
            package.kind == expected,
            "更新包类型不匹配 / Package kind mismatch"
        );
        package.validate()?;
        Ok(package.clone())
    }
}
fn valid_commit(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn current(channel: Channel) -> BuildInfo {
        BuildInfo {
            channel,
            version: "0.1.2".into(),
            commit: "a".repeat(40),
            build: 9,
            attempt: 1,
            target: "x86_64-unknown-linux-gnu".into(),
        }
    }
    fn manifest(channel: Channel) -> Manifest {
        Manifest {
            schema: 1,
            channel,
            version: "0.1.3".into(),
            commit: "b".repeat(40),
            build: 10,
            attempt: 1,
            targets: BTreeMap::new(),
        }
    }
    #[test]
    fn channels_versions_and_alpha_builds() {
        let stable = current(Channel::Stable);
        let mut next = manifest(Channel::Stable);
        assert!(next.newer_than(&stable, "v0.1.3").unwrap());
        next.version = "0.1.1".into();
        assert!(!next.newer_than(&stable, "v0.1.1").unwrap());
        assert!(next.newer_than(&stable, "alpha").is_err());
        assert!(manifest(Channel::Alpha)
            .newer_than(&stable, "alpha")
            .is_err());
        let mut alpha = manifest(Channel::Alpha);
        alpha.version = "0.1.2".into();
        assert!(alpha.newer_than(&current(Channel::Alpha), "alpha").unwrap());
        alpha.build = 9;
        alpha.attempt = 1;
        assert!(!alpha.newer_than(&current(Channel::Alpha), "alpha").unwrap());
        alpha.attempt = 2;
        assert!(alpha.newer_than(&current(Channel::Alpha), "alpha").unwrap());
        assert!(alpha
            .newer_than(&current(Channel::Development), "alpha")
            .is_err());
        assert!(alpha.package("x86_64-pc-windows-msvc", false).is_err());
    }
    #[test]
    fn platform_packages_select_dmg_zip_and_installer() {
        let mut manifest = manifest(Channel::Stable);
        for (target, kind, suffix) in [
            ("aarch64-apple-darwin", PackageKind::Dmg, ".dmg"),
            ("x86_64-pc-windows-msvc", PackageKind::Zip, ".zip"),
            ("x86_64-unknown-linux-gnu", PackageKind::TarGz, ".tar.gz"),
        ] {
            manifest.targets.insert(
                target.into(),
                TargetPackages {
                    archive: Package {
                        name: format!("kitonyterms-test{suffix}"),
                        size: 1,
                        sha256: "0".repeat(64),
                        kind,
                    },
                    installer: if target.contains("windows") {
                        Some(Package {
                            name: "kitonyterms-test-setup.exe".into(),
                            size: 1,
                            sha256: "0".repeat(64),
                            kind: PackageKind::Installer,
                        })
                    } else {
                        None
                    },
                },
            );
            assert_eq!(manifest.package(target, false).unwrap().kind, kind);
        }
        assert_eq!(
            manifest
                .package("x86_64-pc-windows-msvc", true)
                .unwrap()
                .kind,
            PackageKind::Installer
        );
        assert!(manifest.package("aarch64-pc-windows-msvc", false).is_err());
        assert!(manifest.package("x86_64-apple-darwin", false).is_err());
        manifest
            .targets
            .get_mut("aarch64-apple-darwin")
            .unwrap()
            .archive
            .kind = PackageKind::Zip;
        assert!(manifest.package("aarch64-apple-darwin", false).is_err());
    }
}
