//! SSH 握手前的 TCP 级代理支持。
//!
//! 在与 SSH 服务器握手之前，先经代理建立到目标 `host:port` 的 TCP 流：
//! - SOCKS5（RFC 1928 / 1929，通过 `tokio-socks`）
//! - HTTP CONNECT（手写，RFC 7231）
//! - System：读取环境变量代理（跨平台，无平台专有依赖）后再分派到上述两者
//!
//! 代理仅作用于最外层 TCP 连接。得到的 [`tokio::net::TcpStream`] 会交给
//! `russh::client::connect_stream` 完成 SSH 握手。
//!
//! 注意：为保持模块边界隔离，代理凭证不接入 vault——SOCKS5/HTTP 的密码始终为空，
//! 仅在提供 username 时以“用户名 + 空密码”方式尝试认证。

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_socks::tcp::Socks5Stream;

use kt_config::ProxyConfig;

use super::SshError;

/// 经代理建立到 `target_host:target_port` 的 TCP 流。
///
/// - `Direct`：返回 `Ok(None)`，调用方应走原有直连路径。
/// - `System`：解析环境变量代理；解析不到任何代理时同样返回 `Ok(None)` 回退直连。
/// - `Socks5` / `Http`：返回 `Ok(Some(stream))`，其中 `stream` 已连通目标，可直接握手。
pub(super) async fn connect_via_proxy(
    proxy: &ProxyConfig,
    target_host: &str,
    target_port: u16,
) -> Result<Option<TcpStream>, SshError> {
    // 先把 System 解析为具体代理类型（或直连）。
    let resolved = match proxy {
        ProxyConfig::Direct => return Ok(None),
        ProxyConfig::System => match system_proxy_from_env() {
            Some(p) => p,
            None => return Ok(None),
        },
        other => other.clone(),
    };

    match resolved {
        ProxyConfig::Direct | ProxyConfig::System => Ok(None),
        ProxyConfig::Socks5 {
            host,
            port,
            username,
        } => {
            let stream =
                connect_socks5(&host, port, username.as_deref(), target_host, target_port).await?;
            Ok(Some(stream))
        }
        ProxyConfig::Http {
            host,
            port,
            username,
        } => {
            let stream =
                connect_http(&host, port, username.as_deref(), target_host, target_port).await?;
            Ok(Some(stream))
        }
    }
}

/// 经 SOCKS5 代理 CONNECT 到目标，返回底层 TCP 流。
async fn connect_socks5(
    proxy_host: &str,
    proxy_port: u16,
    username: Option<&str>,
    target_host: &str,
    target_port: u16,
) -> Result<TcpStream, SshError> {
    let proxy_addr = (proxy_host, proxy_port);
    let target = (target_host, target_port);
    // 提供 username 时走 user/pass 认证（密码留空）；否则走无认证。
    let stream = match username {
        Some(user) if !user.is_empty() => {
            Socks5Stream::connect_with_password(proxy_addr, target, user, "")
                .await
                .map_err(|e| SshError::Proxy(format!("socks5 connect failed: {e}")))?
        }
        _ => Socks5Stream::connect(proxy_addr, target)
            .await
            .map_err(|e| SshError::Proxy(format!("socks5 connect failed: {e}")))?,
    };
    Ok(stream.into_inner())
}

/// 经 HTTP CONNECT 代理建立隧道到目标，返回 TCP 流。
async fn connect_http(
    proxy_host: &str,
    proxy_port: u16,
    username: Option<&str>,
    target_host: &str,
    target_port: u16,
) -> Result<TcpStream, SshError> {
    let mut stream = TcpStream::connect((proxy_host, proxy_port))
        .await
        .map_err(|e| SshError::Proxy(format!("http proxy connect failed: {e}")))?;

    let request = build_connect_request(target_host, target_port, username)
        .map_err(|e| SshError::Proxy(format!("http proxy CONNECT request invalid: {e}")))?;
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| SshError::Proxy(format!("http proxy write failed: {e}")))?;
    stream
        .flush()
        .await
        .map_err(|e| SshError::Proxy(format!("http proxy flush failed: {e}")))?;

    let head = read_http_head(&mut stream).await?;
    let status = parse_connect_status(&head)
        .map_err(|e| SshError::Proxy(format!("http proxy CONNECT rejected: {e}")))?;
    if !(200..300).contains(&status) {
        return Err(SshError::Proxy(format!(
            "http proxy CONNECT returned status {status}"
        )));
    }
    Ok(stream)
}

/// 从流中读取 HTTP 响应头，读到 `\r\n\r\n` 即止，避免吞掉后续 SSH 握手字节。
async fn read_http_head(stream: &mut TcpStream) -> Result<String, SshError> {
    // 头部通常很小，逐字节读取以精确停在头部结束边界。
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        let n = stream
            .read(&mut byte)
            .await
            .map_err(|e| SshError::Proxy(format!("http proxy read failed: {e}")))?;
        if n == 0 {
            return Err(SshError::Proxy(
                "http proxy closed connection before response completed".to_string(),
            ));
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
        // 防御：异常代理不断发送数据时避免无限增长。
        if buf.len() > 16 * 1024 {
            return Err(SshError::Proxy(
                "http proxy response header too large".to_string(),
            ));
        }
    }
    String::from_utf8(buf)
        .map_err(|_| SshError::Proxy("http proxy sent invalid header".to_string()))
}

/// 构造 HTTP CONNECT 请求。提供 username 时附带 `Proxy-Authorization: Basic`（空密码）。
fn build_connect_request(host: &str, port: u16, username: Option<&str>) -> Result<String, String> {
    validate_http_authority_host(host)?;
    let authority = format!("{host}:{port}");
    let mut req = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: keep-alive\r\n"
    );
    if let Some(user) = username.filter(|u| !u.is_empty()) {
        // 凭证不接入 vault，密码固定为空 -> base64("user:")。
        let token = BASE64.encode(format!("{user}:").as_bytes());
        req.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
    }
    req.push_str("\r\n");
    Ok(req)
}

fn validate_http_authority_host(host: &str) -> Result<(), String> {
    if host.is_empty() {
        return Err("host is empty".to_string());
    }
    if host
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err("host contains whitespace or control characters".to_string());
    }
    Ok(())
}

/// 从 HTTP 响应头首行解析状态码。
fn parse_connect_status(head: &str) -> Result<u16, String> {
    let first_line = head.lines().next().ok_or("empty response")?;
    let mut parts = first_line.split_whitespace();
    let version = parts.next().ok_or("missing status line")?;
    if !version.starts_with("HTTP/") {
        return Err(format!("unexpected status line: {first_line}"));
    }
    let code = parts.next().ok_or("missing status code")?;
    code.parse::<u16>()
        .map_err(|_| format!("invalid status code: {code}"))
}

/// 从环境变量解析系统代理。
///
/// 依次检查 `ALL_PROXY`/`all_proxy`、`HTTPS_PROXY`/`https_proxy`、
/// `HTTP_PROXY`/`http_proxy`、`SOCKS_PROXY`/`socks_proxy`，返回首个可解析的代理。
fn system_proxy_from_env() -> Option<ProxyConfig> {
    const VARS: &[&str] = &[
        "ALL_PROXY",
        "all_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "SOCKS_PROXY",
        "socks_proxy",
    ];
    for var in VARS {
        if let Ok(value) = std::env::var(var) {
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            if let Some(cfg) = parse_proxy_url(value) {
                return Some(cfg);
            }
        }
    }
    None
}

/// 解析代理 URL 为 [`ProxyConfig`]。
///
/// 支持的 scheme：`socks5://`、`socks5h://`、`socks://`、`http://`、`https://`。
/// 无 scheme 时按 SOCKS5 处理。形如 `scheme://[user[:pass]@]host:port`，
/// 端口缺省时按 scheme 推断（SOCKS5=1080，HTTP=8080）。
fn parse_proxy_url(url: &str) -> Option<ProxyConfig> {
    let url = url.trim();
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        // 无 scheme：默认 SOCKS5。
        None => ("socks5".to_string(), url),
    };

    // 去掉可能的路径部分。
    let rest = rest.split(['/', '?', '#']).next().unwrap_or(rest);

    // 拆分 userinfo。
    let (userinfo, host_port) = match rest.rsplit_once('@') {
        Some((userinfo, host_port)) => (Some(userinfo), host_port),
        None => (None, rest),
    };
    // userinfo 可能是 user 或 user:pass，只取 user（密码不接入 vault）。
    let username = userinfo
        .map(|ui| ui.split_once(':').map(|(u, _)| u).unwrap_or(ui))
        .filter(|u| !u.is_empty())
        .map(|u| u.to_string());

    let is_http = matches!(scheme.as_str(), "http" | "https");
    let default_port = if is_http { 8080 } else { 1080 };
    let (host, port) = parse_authority(host_port, default_port)?;
    if host.is_empty() {
        return None;
    }

    if is_http {
        Some(ProxyConfig::Http {
            host,
            port,
            username,
        })
    } else {
        // socks5 / socks5h / socks / 其它未知 scheme 一律按 SOCKS5。
        Some(ProxyConfig::Socks5 {
            host,
            port,
            username,
        })
    }
}

/// 解析 `host:port` 权威部分，支持 IPv6 方括号形式，端口缺省时回退 `default_port`。
fn parse_authority(authority: &str, default_port: u16) -> Option<(String, u16)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = after
            .strip_prefix(':')
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(default_port);
        return Some((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            Some((host.to_string(), port.parse::<u16>().ok()?))
        }
        _ => Some((authority.to_string(), default_port)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_connect_request_without_auth() {
        let req = build_connect_request("example.com", 22, None).unwrap();
        assert!(req.starts_with("CONNECT example.com:22 HTTP/1.1\r\n"));
        assert!(req.contains("Host: example.com:22\r\n"));
        assert!(!req.contains("Proxy-Authorization"));
        assert!(req.ends_with("\r\n\r\n"));
    }

    #[test]
    fn build_connect_request_with_username_uses_empty_password() {
        let req = build_connect_request("10.0.0.1", 2222, Some("alice")).unwrap();
        // base64("alice:") == "YWxpY2U6"
        assert!(req.contains("Proxy-Authorization: Basic YWxpY2U6\r\n"));
        assert!(req.ends_with("\r\n\r\n"));
    }

    #[test]
    fn build_connect_request_rejects_header_injection_host() {
        let err = build_connect_request("example.com\r\nX-Evil: 1", 22, None).unwrap_err();
        assert!(err.contains("control"));
    }

    #[test]
    fn parse_connect_status_ok() {
        let head = "HTTP/1.1 200 Connection established\r\nX-Foo: bar\r\n\r\n";
        assert_eq!(parse_connect_status(head).unwrap(), 200);
    }

    #[test]
    fn parse_connect_status_rejects_non_http() {
        let head = "GARBAGE 200 ok\r\n\r\n";
        assert!(parse_connect_status(head).is_err());
    }

    #[test]
    fn parse_connect_status_407() {
        let head = "HTTP/1.1 407 Proxy Authentication Required\r\n\r\n";
        assert_eq!(parse_connect_status(head).unwrap(), 407);
    }

    #[test]
    fn parse_proxy_url_socks5_with_user() {
        let cfg = parse_proxy_url("socks5://user@1.2.3.4:1080").unwrap();
        assert_eq!(
            cfg,
            ProxyConfig::Socks5 {
                host: "1.2.3.4".to_string(),
                port: 1080,
                username: Some("user".to_string()),
            }
        );
    }

    #[test]
    fn parse_proxy_url_socks5h_scheme() {
        let cfg = parse_proxy_url("socks5h://proxy.local:1080").unwrap();
        assert_eq!(
            cfg,
            ProxyConfig::Socks5 {
                host: "proxy.local".to_string(),
                port: 1080,
                username: None,
            }
        );
    }

    #[test]
    fn parse_proxy_url_http_with_userpass_drops_password() {
        let cfg = parse_proxy_url("http://bob:secret@proxy:3128").unwrap();
        assert_eq!(
            cfg,
            ProxyConfig::Http {
                host: "proxy".to_string(),
                port: 3128,
                username: Some("bob".to_string()),
            }
        );
    }

    #[test]
    fn parse_proxy_url_no_scheme_defaults_socks5() {
        let cfg = parse_proxy_url("127.0.0.1:1080").unwrap();
        assert_eq!(
            cfg,
            ProxyConfig::Socks5 {
                host: "127.0.0.1".to_string(),
                port: 1080,
                username: None,
            }
        );
    }

    #[test]
    fn parse_proxy_url_default_ports() {
        let socks = parse_proxy_url("socks5://host").unwrap();
        assert_eq!(
            socks,
            ProxyConfig::Socks5 {
                host: "host".to_string(),
                port: 1080,
                username: None,
            }
        );
        let http = parse_proxy_url("http://host").unwrap();
        assert_eq!(
            http,
            ProxyConfig::Http {
                host: "host".to_string(),
                port: 8080,
                username: None,
            }
        );
    }

    #[test]
    fn parse_proxy_url_ipv6_brackets() {
        let cfg = parse_proxy_url("socks5://[2001:db8::1]:1080").unwrap();
        assert_eq!(
            cfg,
            ProxyConfig::Socks5 {
                host: "2001:db8::1".to_string(),
                port: 1080,
                username: None,
            }
        );
    }

    /// 端到端：假 HTTP CONNECT 代理 -> 后端目标，验证隧道打通且不吞掉首字节。
    #[tokio::test]
    async fn http_connect_tunnels_to_target() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        // 后端目标：接受连接后先发一段“SSH 横幅”，再回显收到的内容。
        let target = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let target_port = target.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut sock, _) = target.accept().await.unwrap();
            sock.write_all(b"SSH-2.0-Fake\r\n").await.unwrap();
            let mut buf = [0u8; 5];
            sock.read_exact(&mut buf).await.unwrap();
            sock.write_all(&buf).await.unwrap();
        });

        // 假代理：读取 CONNECT 头，回 200，然后在客户端与目标之间双向转发。
        let proxy = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let proxy_port = proxy.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut client, _) = proxy.accept().await.unwrap();
            // 读到头部结束。
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                client.read_exact(&mut byte).await.unwrap();
                head.push(byte[0]);
                if head.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let head = String::from_utf8(head).unwrap();
            assert!(head.starts_with(&format!("CONNECT 127.0.0.1:{target_port} HTTP/1.1")));
            client
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await
                .unwrap();
            // 打通到后端目标并双向转发。
            let mut upstream = TcpStream::connect(("127.0.0.1", target_port))
                .await
                .unwrap();
            tokio::io::copy_bidirectional(&mut client, &mut upstream)
                .await
                .ok();
        });

        // 客户端经代理连到目标。
        let mut stream = connect_http("127.0.0.1", proxy_port, None, "127.0.0.1", target_port)
            .await
            .expect("proxy connect");

        // 首字节（SSH 横幅）必须完整保留，未被 CONNECT 头解析吞掉。
        let mut banner = [0u8; 14];
        stream.read_exact(&mut banner).await.unwrap();
        assert_eq!(&banner, b"SSH-2.0-Fake\r\n");

        // 双向数据可用。
        stream.write_all(b"hello").await.unwrap();
        let mut echo = [0u8; 5];
        stream.read_exact(&mut echo).await.unwrap();
        assert_eq!(&echo, b"hello");
    }
}
