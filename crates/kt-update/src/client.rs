//! 固定仓库的 Release 查询和有界 HTTP 读取。
use crate::metadata::{BuildInfo, Channel, Manifest, Package};
use crate::proxy::UpdateProxy;
use anyhow::{ensure, Result};
use reqwest::{header, Client, Response, StatusCode};
use serde::Deserialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const REPOSITORY: &str = "nzesupb/KitonyTerms";
pub const RELEASE_PAGE: &str = "https://github.com/nzesupb/KitonyTerms/releases";
const META_LIMIT: usize = 2 * 1024 * 1024;
/// 生产超时：连接（经代理时是到代理的连接）、单次读取静默、整次请求总时限。
/// 代理不可达或黑洞时分别由连接与读取超时兜底，失败会经 `describe_failure` 归类。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// 更新重定向白名单：只允许官方 GitHub 域名，且必须仍是 HTTPS。
fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 5 {
            return attempt.error("too many redirects");
        }
        let url = attempt.url();
        let allowed = matches!(
            url.host_str(),
            Some(
                "api.github.com"
                    | "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        );
        if url.scheme() == "https" && allowed {
            attempt.follow()
        } else {
            attempt.error("untrusted update redirect")
        }
    })
}
pub const MAX_RELEASE_NOTES: usize = 64 * 1024;
#[derive(Debug)]
pub struct RetryAfter(pub u64);
impl std::fmt::Display for RetryAfter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GitHub 请求受限，请 {} 秒后重试 / GitHub rate limit: retry in {}s",
            self.0, self.0
        )
    }
}
impl std::error::Error for RetryAfter {}
#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub digest: Option<String>,
}
#[derive(Deserialize)]
struct Release {
    id: u64,
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
    body: Option<String>,
    published_at: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    pub info: BuildInfo,
    pub package: Package,
    pub asset_id: u64,
    pub release_id: u64,
    pub tag: String,
    pub notes: String,
    pub notes_truncated: bool,
    pub published_at: Option<String>,
}
#[derive(Clone)]
pub struct ReleaseClient {
    client: Client,
    base: String,
}
impl ReleaseClient {
    pub fn new() -> Result<Self> {
        Self::configured(&UpdateProxy::System)
    }
    /// 按代理设置构造生产客户端：官方仓库地址 + 生产超时 + 重定向白名单。
    pub fn configured(proxy: &UpdateProxy) -> Result<Self> {
        Self::build(
            format!("https://api.github.com/repos/{REPOSITORY}"),
            proxy,
            redirect_policy(),
            (CONNECT_TIMEOUT, READ_TIMEOUT, TOTAL_TIMEOUT),
        )
    }
    fn build(
        base: String,
        proxy: &UpdateProxy,
        redirects: reqwest::redirect::Policy,
        timeouts: (Duration, Duration, Duration),
    ) -> Result<Self> {
        let (connect, read, total) = timeouts;
        let builder = Client::builder()
            .user_agent("KitonyTerms-updater/1")
            .connect_timeout(connect)
            .read_timeout(read)
            .timeout(total)
            .redirect(redirects);
        Ok(Self {
            client: proxy.apply(builder)?.build()?,
            base,
        })
    }
    #[cfg(test)]
    pub(crate) fn testing(base: String, timeout: Duration) -> Self {
        Self {
            client: Client::builder()
                .no_proxy()
                .pool_max_idle_per_host(0)
                .timeout(timeout)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            base,
        }
    }
    /// 测试专用：真实代理接线（含重定向白名单）但指向本地测试服务与短超时。
    #[cfg(test)]
    pub(crate) fn testing_with_proxy(
        base: String,
        proxy: &UpdateProxy,
        timeout: Duration,
    ) -> Result<Self> {
        Self::build(base, proxy, redirect_policy(), (timeout, timeout, timeout))
    }
    async fn get(&self, path: &str, binary: bool) -> Result<Response> {
        let response = self
            .client
            .get(format!("{}{path}", self.base))
            .header(
                header::ACCEPT,
                if binary {
                    "application/octet-stream"
                } else {
                    "application/vnd.github+json"
                },
            )
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await?;
        if matches!(
            response.status(),
            StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
        ) {
            let headers = response.headers();
            let seconds = headers
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .or_else(|| {
                    headers
                        .get("x-ratelimit-reset")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok())
                        .map(|v| v.saturating_sub(now()).saturating_add(1))
                })
                .unwrap_or(300)
                .max(1);
            return Err(RetryAfter(seconds).into());
        }
        Ok(response.error_for_status()?)
    }
    pub async fn asset(&self, id: u64) -> Result<Response> {
        ensure!(id > 0, "无效资产 ID / Invalid asset ID");
        self.get(&format!("/releases/assets/{id}"), true).await
    }
    pub async fn check(&self, current: &BuildInfo, installed: bool) -> Result<Option<Available>> {
        let endpoint = match current.channel {
            Channel::Stable => "/releases/latest",
            Channel::Alpha => "/releases/tags/alpha",
            Channel::Development => {
                anyhow::bail!("开发构建不支持更新 / Development builds cannot update")
            }
        };
        let release: Release =
            serde_json::from_slice(&bounded(self.get(endpoint, false).await?, META_LIMIT).await?)?;
        ensure!(
            !release.draft
                && release.id > 0
                && release.prerelease == (current.channel == Channel::Alpha),
            "发布状态不匹配 / Release state mismatch"
        );
        let asset = unique_asset(&release.assets, "update-manifest.json")?;
        ensure!(
            asset.size > 0 && asset.size <= META_LIMIT as u64,
            "更新清单过大 / Manifest too large"
        );
        let bytes = bounded(self.asset(asset.id).await?, META_LIMIT).await?;
        ensure!(
            bytes.len() as u64 == asset.size,
            "更新清单截断 / Truncated manifest"
        );
        if let Some(digest) = &asset.digest {
            verify_digest(&bytes, digest)?;
        }
        let manifest: Manifest = serde_json::from_slice(&bytes)?;
        if !manifest.newer_than(current, &release.tag_name)? {
            return Ok(None);
        }
        let package = manifest.package(&current.target, installed)?;
        let asset = unique_asset(&release.assets, &package.name)?;
        ensure!(
            asset.size == package.size,
            "发布包大小不一致 / Asset size mismatch"
        );
        if let Some(digest) = &asset.digest {
            ensure!(
                digest == &format!("sha256:{}", package.sha256.to_lowercase()),
                "发布摘要不一致 / Asset digest mismatch"
            );
        }
        let mut notes = release.body.unwrap_or_default();
        let notes_truncated = notes.len() > MAX_RELEASE_NOTES;
        if notes_truncated {
            let mut end = MAX_RELEASE_NOTES;
            while !notes.is_char_boundary(end) {
                end -= 1;
            }
            notes.truncate(end);
        }
        Ok(Some(Available {
            info: BuildInfo {
                channel: manifest.channel,
                version: manifest.version,
                commit: manifest.commit,
                build: manifest.build,
                attempt: manifest.attempt,
                target: current.target.clone(),
            },
            package,
            asset_id: asset.id,
            release_id: release.id,
            tag: release.tag_name,
            notes,
            notes_truncated,
            published_at: release.published_at,
        }))
    }
}
fn unique_asset<'a>(assets: &'a [Asset], name: &str) -> Result<&'a Asset> {
    let mut found = assets.iter().filter(|asset| asset.name == name);
    let asset = found
        .next()
        .ok_or_else(|| anyhow::anyhow!("缺少发布资产 / Missing release asset: {name}"))?;
    ensure!(
        found.next().is_none(),
        "重复发布资产 / Duplicate release asset"
    );
    Ok(asset)
}
async fn bounded(mut response: Response, limit: usize) -> Result<Vec<u8>> {
    ensure!(
        response
            .content_length()
            .is_none_or(|size| size <= limit as u64),
        "响应过大 / Response too large"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= limit,
            "响应过大 / Response too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
fn verify_digest(bytes: &[u8], digest: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    ensure!(
        digest == format!("sha256:{}", hex::encode(Sha256::digest(bytes))),
        "更新清单校验失败 / Manifest digest mismatch"
    );
    Ok(())
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    fn server(status: &str, headers: &str, body: &str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n{body}",
            body.len()
        );
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0; 2048];
            let _ = stream.read(&mut buffer);
            stream.write_all(response.as_bytes()).unwrap();
        });
        url
    }
    #[tokio::test]
    async fn limits_errors_and_truncation() {
        let mut client = ReleaseClient::new().unwrap();
        client.base = server("429 Too Many Requests", "Retry-After: 42\r\n", "");
        assert_eq!(
            client
                .asset(1)
                .await
                .unwrap_err()
                .downcast_ref::<RetryAfter>()
                .unwrap()
                .0,
            42
        );
        client.base = server("404 Not Found", "", "");
        assert!(client.asset(1).await.is_err());
        client.base = server("200 OK", "", "1234");
        assert!(bounded(client.asset(1).await.unwrap(), 3).await.is_err());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        client.base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut b = [0; 2048];
            let _ = s.read(&mut b);
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\n123")
                .unwrap();
        });
        assert!(bounded(client.asset(1).await.unwrap(), 20).await.is_err());
    }
}
