//! 更新通道的代理设置：作用于 reqwest 客户端，同时覆盖 Release 查询与包下载。
//!
//! 与 `kt-core` 的 TCP 代理（SSH 握手前）分层：这里只在 HTTP 客户端生效，语义
//! 保持一致：
//! - [`UpdateProxy::System`] 读取环境变量代理（`ALL_PROXY`/`HTTPS_PROXY`/
//!   `HTTP_PROXY`/`NO_PROXY`），未设置时直连；
//! - 显式代理只保存用户名、密码留空（与 `kt-core` 的 TCP 代理相同，代理凭证不
//!   接入 vault）。需要密码认证的代理请使用系统代理模式，并在环境变量 URL 中
//!   携带凭证，凭证不落盘。

use anyhow::{ensure, Result};
use reqwest::{ClientBuilder, Proxy};
use serde::{Deserialize, Serialize};

/// 代理主机长度上限（域名上限 253，另留 IPv6 字面量余量）。
const MAX_HOST_LEN: usize = 255;
const MAX_USERNAME_LEN: usize = 128;

/// 更新通道使用的代理。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UpdateProxy {
    /// 使用系统/环境变量代理设置；没有任何代理时直连。
    #[default]
    System,
    /// 忽略环境变量代理，强制直连。
    Direct,
    /// HTTP 代理（HTTPS 目标经 CONNECT 隧道）。
    Http {
        host: String,
        port: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        username: Option<String>,
    },
    /// HTTPS 代理：与代理之间也使用 TLS。
    Https {
        host: String,
        port: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        username: Option<String>,
    },
    /// SOCKS5 代理；域名交由代理解析（`socks5h`）。
    Socks5 {
        host: String,
        port: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        username: Option<String>,
    },
}

impl UpdateProxy {
    /// 保存前校验：配置错误必须在这里失败，不能带着坏地址去请求。
    pub fn validate(&self) -> Result<()> {
        let (host, port, username) = match self {
            Self::System | Self::Direct => return Ok(()),
            Self::Http {
                host,
                port,
                username,
            }
            | Self::Https {
                host,
                port,
                username,
            }
            | Self::Socks5 {
                host,
                port,
                username,
            } => (host, *port, username),
        };
        ensure!(
            !host.trim().is_empty(),
            "代理主机不能为空 / Proxy host must not be empty"
        );
        ensure!(
            host.len() <= MAX_HOST_LEN,
            "代理主机过长 / Proxy host is too long"
        );
        ensure!(
            host.trim() == host,
            "代理主机不能包含首尾空白 / Proxy host must not have surrounding spaces"
        );
        ensure!(
            !host.chars().any(|character| character.is_whitespace()
                || character.is_control()
                || matches!(character, '/' | '@' | '?' | '#' | '\\')),
            "代理主机包含非法字符 / Proxy host contains invalid characters"
        );
        ensure!(port != 0, "代理端口无效 / Invalid proxy port");
        if let Some(username) = username {
            let trimmed = username.trim();
            ensure!(
                username.len() <= MAX_USERNAME_LEN,
                "代理用户名过长 / Proxy username is too long"
            );
            ensure!(
                trimmed == username.as_str(),
                "代理用户名不能包含首尾空白 / Proxy username must not have surrounding spaces"
            );
            ensure!(
                !username.chars().any(char::is_control),
                "代理用户名包含控制字符 / Proxy username contains control characters"
            );
        }
        Ok(())
    }

    /// 是否配置了显式代理（用于失败提示是否需要指向代理设置）。
    pub fn is_explicit(&self) -> bool {
        !matches!(self, Self::System | Self::Direct)
    }

    /// 显式代理失败时的可操作提示。
    pub(crate) fn hint(&self) -> Option<&'static str> {
        self.is_explicit()
            .then_some("请检查更新代理的地址、端口与可达性 / Check the update proxy address, port and reachability")
    }

    fn fields(&self) -> Option<(&str, u16, Option<&str>)> {
        match self {
            Self::System | Self::Direct => None,
            Self::Http {
                host,
                port,
                username,
            }
            | Self::Https {
                host,
                port,
                username,
            }
            | Self::Socks5 {
                host,
                port,
                username,
            } => Some((
                host.as_str(),
                *port,
                username
                    .as_deref()
                    .map(str::trim)
                    .filter(|username| !username.is_empty()),
            )),
        }
    }

    /// 代理 URL：HTTP(S) 走请求头认证，SOCKS5 认证只能放在 URL userinfo 里。
    fn endpoint(&self) -> Option<String> {
        let (host, port, username) = self.fields()?;
        let authority = host_authority(host);
        Some(match self {
            Self::Http { .. } => format!("http://{authority}:{port}"),
            Self::Https { .. } => format!("https://{authority}:{port}"),
            Self::Socks5 { .. } => match username {
                Some(username) => {
                    let username: String =
                        url::form_urlencoded::byte_serialize(username.as_bytes()).collect();
                    // 密码按约定留空：显式代理不保存代理密码。
                    format!("socks5h://{username}:@{authority}:{port}")
                }
                None => format!("socks5h://{authority}:{port}"),
            },
            Self::System | Self::Direct => unreachable!("fields() 已排除非显式代理"),
        })
    }

    /// 把代理设置应用到 reqwest 客户端构造器。
    pub(crate) fn apply(&self, builder: ClientBuilder) -> Result<ClientBuilder> {
        self.validate()?;
        match self {
            // reqwest 默认即读取环境变量代理；这里不调用 proxy()/no_proxy()。
            Self::System => Ok(builder),
            Self::Direct => Ok(builder.no_proxy()),
            Self::Http { .. } | Self::Https { .. } => {
                let proxy = Proxy::all(self.endpoint().expect("显式代理必有地址"))?;
                Ok(
                    builder.proxy(match self.fields().and_then(|(_, _, username)| username) {
                        // 只发送用户名（密码留空）与既有的 TCP 代理行为保持一致。
                        Some(username) => proxy.basic_auth(username, ""),
                        None => proxy,
                    }),
                )
            }
            // SOCKS5 的用户名已在 endpoint() 的 userinfo 中编码。
            Self::Socks5 { .. } => {
                Ok(builder.proxy(Proxy::all(self.endpoint().expect("显式代理必有地址"))?))
            }
        }
    }
}

/// IPv6 字面量需要方括号，其余主机名原样使用。
fn host_authority(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(host: &str, port: u16, username: Option<&str>) -> UpdateProxy {
        UpdateProxy::Http {
            host: host.to_string(),
            port,
            username: username.map(str::to_string),
        }
    }

    #[test]
    fn defaults_to_system_and_roundtrips_every_variant() {
        assert_eq!(UpdateProxy::default(), UpdateProxy::System);

        for proxy in [
            UpdateProxy::System,
            UpdateProxy::Direct,
            http("127.0.0.1", 8080, None),
            http("proxy.local", 3128, Some("me")),
            UpdateProxy::Https {
                host: "proxy.local".into(),
                port: 8443,
                username: None,
            },
            UpdateProxy::Socks5 {
                host: "127.0.0.1".into(),
                port: 1080,
                username: Some("me".into()),
            },
        ] {
            let encoded = serde_json::to_string(&proxy).unwrap();
            let decoded: UpdateProxy = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, proxy);
        }
    }

    #[test]
    fn legacy_preferences_without_proxy_load_as_system() {
        let encoded = r#"{"auto_check":true,"auto_download":true}"#;
        let preferences: crate::Preferences = serde_json::from_str(encoded).unwrap();
        assert_eq!(preferences.proxy, UpdateProxy::System);
    }

    #[test]
    fn proxy_kind_is_tagged_and_username_is_optional() {
        let encoded = serde_json::to_string(&http("127.0.0.1", 8080, None)).unwrap();
        assert!(encoded.contains("\"kind\":\"http\""), "{encoded}");
        assert!(!encoded.contains("username"), "{encoded}");
    }

    #[test]
    fn validation_rejects_unusable_addresses() {
        assert!(http("", 8080, None).validate().is_err());
        assert!(http("   ", 8080, None).validate().is_err());
        assert!(http(" 127.0.0.1", 8080, None).validate().is_err());
        assert!(http("127.0.0.1/24", 8080, None).validate().is_err());
        assert!(http("user@host", 8080, None).validate().is_err());
        assert!(http("127.0.0.1", 0, None).validate().is_err());
        assert!(http("127.0.0.1", 8080, Some("bad\nname"))
            .validate()
            .is_err());
        assert!(http(&"a".repeat(MAX_HOST_LEN + 1), 8080, None)
            .validate()
            .is_err());
        assert!(http(&"a".repeat(MAX_HOST_LEN), 65535, None)
            .validate()
            .is_ok());
        // 系统代理与直连不需要地址。
        assert!(UpdateProxy::System.validate().is_ok());
        assert!(UpdateProxy::Direct.validate().is_ok());
    }

    #[test]
    fn endpoints_use_the_expected_scheme_and_userinfo() {
        assert_eq!(
            http("127.0.0.1", 8080, Some("me")).endpoint().unwrap(),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            UpdateProxy::Https {
                host: "proxy.local".into(),
                port: 8443,
                username: None,
            }
            .endpoint()
            .unwrap(),
            "https://proxy.local:8443"
        );
        // SOCKS5 认证只能经 URL userinfo 传递，用户名需要百分号编码。
        assert_eq!(
            UpdateProxy::Socks5 {
                host: "127.0.0.1".into(),
                port: 1080,
                username: Some("me@example.com".into()),
            }
            .endpoint()
            .unwrap(),
            "socks5h://me%40example.com:@127.0.0.1:1080"
        );
        assert_eq!(
            UpdateProxy::Socks5 {
                host: "127.0.0.1".into(),
                port: 1080,
                username: None,
            }
            .endpoint()
            .unwrap(),
            "socks5h://127.0.0.1:1080"
        );
        assert_eq!(
            http("::1", 8080, None).endpoint().unwrap(),
            "http://[::1]:8080"
        );
        assert_eq!(UpdateProxy::System.endpoint(), None);
        assert_eq!(UpdateProxy::Direct.endpoint(), None);
    }

    #[test]
    fn explicit_proxy_reports_a_hint_and_system_does_not() {
        assert!(http("127.0.0.1", 8080, None).is_explicit());
        assert!(UpdateProxy::Socks5 {
            host: "127.0.0.1".into(),
            port: 1080,
            username: None,
        }
        .is_explicit());
        assert!(!UpdateProxy::System.is_explicit());
        assert!(!UpdateProxy::Direct.is_explicit());
        assert!(http("127.0.0.1", 8080, None).hint().is_some());
        assert!(UpdateProxy::System.hint().is_none());
    }
}
