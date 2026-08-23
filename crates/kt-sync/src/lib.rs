//! 非机密配置同步。
//!
//! 本 crate 只处理 [`kt_config::Config`]。密码、私钥口令、known_hosts、
//! vault key 和运行时会话状态不属于同步载荷。

use std::{
    collections::{HashSet, VecDeque},
    convert::Infallible,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    ops::Deref,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bytes::Bytes;
use chacha20poly1305::{
    aead::{Aead, Payload},
    ChaCha20Poly1305, KeyInit, Nonce,
};
use hmac::{Hmac, Mac};
use http_body_util::Full;
use hyper::{
    body::Incoming,
    header::{
        CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, ETAG, IF_MATCH, IF_NONE_MATCH,
        WWW_AUTHENTICATE,
    },
    server::conn::http1,
    service::service_fn,
    Method, Request, Response, StatusCode,
};
use hyper_util::rt::TokioIo;
use if_addrs::{IfAddr, Interface};
use kt_config::Config;
use rand_core::{OsRng, RngCore};
use reqwest::{redirect::Policy, Client};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::{net::TcpListener, sync::Semaphore, task::JoinHandle, time::Instant};
use tokio_util::sync::CancellationToken;
use url::Url;

pub const MAX_ENVELOPE_BYTES: usize = 1024 * 1024;
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub const DEFAULT_SHARE_TTL: Duration = Duration::from_secs(10 * 60);
const ENVELOPE_FORMAT: &str = "kitonyterms.config";
const ENVELOPE_VERSION: u16 = 1;
const LAN_PROTOCOL_VERSION: &str = "2";
const LAN_CONFIG_PATH: &str = "/v2/config";
const LAN_ACK_PATH: &str = "/v2/ack";
const LAN_AUTH_DOMAIN: &[u8] = b"kitonyterms.lan.auth.v2";
const LAN_ENCRYPTION_DOMAIN: &[u8] = b"kitonyterms.lan.encryption.v2";
const LAN_RESPONSE_DOMAIN: &[u8] = b"kitonyterms.lan.response.v2";
const LAN_HEADER_PROTOCOL: &str = "x-kitonyterms-protocol";
const LAN_HEADER_NONCE: &str = "x-kitonyterms-nonce";
const LAN_HEADER_AUTH: &str = "x-kitonyterms-auth";
const LAN_HEADER_DELIVERY: &str = "x-kitonyterms-delivery";
const LAN_HEADER_RESPONSE_NONCE: &str = "x-kitonyterms-response-nonce";
const MAX_ENCRYPTED_ENVELOPE_BYTES: usize = MAX_ENVELOPE_BYTES + 16;
const MAX_LAN_CONNECTIONS: usize = 16;
const LAN_CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);
const LAN_REQUEST_HEAD_BYTES: usize = 16 * 1024;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("同步地址无效: {0}")]
    InvalidUrl(String),
    #[error("HTTP 地址不能携带认证信息，请使用 HTTPS")]
    InsecureCredentials,
    #[error("同步载荷超过 {MAX_ENVELOPE_BYTES} 字节限制")]
    TooLarge,
    #[error("不支持的同步载荷: {0}")]
    UnsupportedEnvelope(String),
    #[error("远端配置已变化，请先重新下载")]
    Conflict,
    #[error("远端返回 HTTP {status}: {message}")]
    HttpStatus { status: u16, message: String },
    #[error("同步请求超时")]
    Timeout,
    #[error("同步已取消")]
    Cancelled,
    #[error("网络请求失败: {0}")]
    Network(#[from] reqwest::Error),
    #[error("同步载荷解析失败: {0}")]
    Json(#[from] serde_json::Error),
    #[error("局域网分享失败: {0}")]
    Share(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEnvelope {
    pub format: String,
    pub version: u16,
    pub created_unix: u64,
    pub config: Config,
}

impl SyncEnvelope {
    pub fn new(config: Config) -> Self {
        Self {
            format: ENVELOPE_FORMAT.to_string(),
            version: ENVELOPE_VERSION,
            created_unix: unix_now(),
            config,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, SyncError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() > MAX_ENVELOPE_BYTES {
            return Err(SyncError::TooLarge);
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, SyncError> {
        if bytes.len() > MAX_ENVELOPE_BYTES {
            return Err(SyncError::TooLarge);
        }
        let envelope: Self = serde_json::from_slice(bytes)?;
        envelope.validate()?;
        Ok(envelope)
    }

    fn validate(&self) -> Result<(), SyncError> {
        if self.format != ENVELOPE_FORMAT {
            return Err(SyncError::UnsupportedEnvelope(format!(
                "format {}",
                self.format
            )));
        }
        if self.version != ENVELOPE_VERSION {
            return Err(SyncError::UnsupportedEnvelope(format!(
                "version {}",
                self.version
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct WebDavEndpoint {
    url: Url,
    username: Option<String>,
    password: Option<String>,
}

impl WebDavEndpoint {
    pub fn parse(
        url: &str,
        username: Option<String>,
        password: Option<String>,
    ) -> Result<Self, SyncError> {
        let url = validate_http_url(url)?;
        let username = non_empty(username);
        let password = non_empty(password);
        if url.scheme() == "http" && (username.is_some() || password.is_some()) {
            return Err(SyncError::InsecureCredentials);
        }
        Ok(Self {
            url,
            username,
            password,
        })
    }

    pub fn url(&self) -> &Url {
        &self.url
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRevision(pub String);

#[derive(Debug, Clone)]
pub struct RemoteConfig {
    pub envelope: SyncEnvelope,
    pub revision: Option<RemoteRevision>,
}

#[derive(Debug, Clone, Copy)]
pub enum PutPrecondition<'a> {
    IfMatch(&'a str),
    CreateOnly,
    Unconditional,
}

#[derive(Clone)]
pub struct WebDavClient {
    client: Client,
    request_timeout: Duration,
}

impl WebDavClient {
    pub fn new() -> Result<Self, SyncError> {
        let client = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            client,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        })
    }

    pub async fn download(
        &self,
        endpoint: &WebDavEndpoint,
        cancel: &CancellationToken,
    ) -> Result<RemoteConfig, SyncError> {
        run_cancelled(cancel, self.request_timeout, self.download_inner(endpoint)).await
    }

    async fn download_inner(&self, endpoint: &WebDavEndpoint) -> Result<RemoteConfig, SyncError> {
        let request = with_basic_auth(self.client.get(endpoint.url.clone()), endpoint);
        let mut response = request.send().await?;
        ensure_success(response.status())?;
        if response
            .content_length()
            .is_some_and(|size| size > MAX_ENVELOPE_BYTES as u64)
        {
            return Err(SyncError::TooLarge);
        }
        let revision = response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .map(|value| RemoteRevision(value.to_string()));
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len().saturating_add(chunk.len()) > MAX_ENVELOPE_BYTES {
                return Err(SyncError::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(RemoteConfig {
            envelope: SyncEnvelope::decode(&bytes)?,
            revision,
        })
    }

    pub async fn upload(
        &self,
        endpoint: &WebDavEndpoint,
        envelope: &SyncEnvelope,
        precondition: PutPrecondition<'_>,
        cancel: &CancellationToken,
    ) -> Result<Option<RemoteRevision>, SyncError> {
        let bytes = envelope.encode()?;
        run_cancelled(
            cancel,
            self.request_timeout,
            self.upload_inner(endpoint, bytes, precondition),
        )
        .await
    }

    async fn upload_inner(
        &self,
        endpoint: &WebDavEndpoint,
        bytes: Vec<u8>,
        precondition: PutPrecondition<'_>,
    ) -> Result<Option<RemoteRevision>, SyncError> {
        let mut request = self
            .client
            .put(endpoint.url.clone())
            .header(CONTENT_TYPE, "application/json")
            .body(bytes);
        request = match precondition {
            PutPrecondition::IfMatch(etag) => request.header(IF_MATCH, etag),
            PutPrecondition::CreateOnly => request.header(IF_NONE_MATCH, "*"),
            PutPrecondition::Unconditional => request,
        };
        let response = with_basic_auth(request, endpoint).send().await?;
        ensure_success(response.status())?;
        Ok(response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .map(|value| RemoteRevision(value.to_string())))
    }
}

fn with_basic_auth(
    request: reqwest::RequestBuilder,
    endpoint: &WebDavEndpoint,
) -> reqwest::RequestBuilder {
    match endpoint.username.as_deref() {
        Some(username) => request.basic_auth(username, endpoint.password.as_deref()),
        None => request,
    }
}

fn ensure_success(status: reqwest::StatusCode) -> Result<(), SyncError> {
    if status == reqwest::StatusCode::PRECONDITION_FAILED {
        return Err(SyncError::Conflict);
    }
    if status.is_success() {
        Ok(())
    } else {
        Err(SyncError::HttpStatus {
            status: status.as_u16(),
            message: status.canonical_reason().unwrap_or("unknown").to_string(),
        })
    }
}

async fn run_cancelled<T>(
    cancel: &CancellationToken,
    timeout: Duration,
    future: impl std::future::Future<Output = Result<T, SyncError>>,
) -> Result<T, SyncError> {
    tokio::select! {
        _ = cancel.cancelled() => Err(SyncError::Cancelled),
        result = tokio::time::timeout(timeout, future) => result.map_err(|_| SyncError::Timeout)?,
    }
}

#[derive(Clone)]
pub struct ShareInfo {
    pub url: String,
    pub pairing_code: String,
    pub expires_at_unix: u64,
}

pub struct ShareHandle {
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl ShareHandle {
    pub fn stop(&self) {
        self.cancel.cancel();
    }

    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

impl Drop for ShareHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

pub struct PendingShareImport {
    envelope: SyncEnvelope,
    client: Client,
    ack_url: Url,
    auth_key: [u8; 32],
    delivery_id: [u8; 16],
}

impl PendingShareImport {
    pub fn envelope(&self) -> &SyncEnvelope {
        &self.envelope
    }

    /// 在配置已经安全落盘后确认接收成功。
    ///
    /// 分享端只会在该确认响应成功写出后消费分享；确认失败时调用方可重新下载并重试。
    pub async fn acknowledge(self, cancel: &CancellationToken) -> Result<(), SyncError> {
        let future = async {
            for attempt in 0..4 {
                let nonce = random_bytes::<16>();
                let request = authenticated_request(
                    self.client.post(self.ack_url.clone()),
                    &Method::POST,
                    LAN_ACK_PATH,
                    &self.auth_key,
                    &nonce,
                    Some(&self.delivery_id),
                );
                let response = request.send().await?;
                let status = response.status();
                if status == reqwest::StatusCode::CONFLICT && attempt < 3 {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    continue;
                }
                return ensure_success(status);
            }
            unreachable!("ack retry loop always returns")
        };
        run_cancelled(cancel, DEFAULT_REQUEST_TIMEOUT, future).await
    }
}

struct NonceCache {
    capacity: usize,
    order: VecDeque<[u8; 17]>,
    entries: HashSet<[u8; 17]>,
}

impl NonceCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::with_capacity(capacity),
            entries: HashSet::with_capacity(capacity),
        }
    }

    fn claim(&mut self, scope: u8, nonce: [u8; 16]) -> bool {
        let mut key = [0u8; 17];
        key[0] = scope;
        key[1..].copy_from_slice(&nonce);
        if !self.entries.insert(key) {
            return false;
        }
        self.order.push_back(key);
        if self.order.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
        true
    }
}

impl Deref for PendingShareImport {
    type Target = SyncEnvelope;

    fn deref(&self) -> &Self::Target {
        &self.envelope
    }
}

#[derive(Clone)]
struct LanKeys {
    auth: [u8; 32],
    encryption: [u8; 32],
}

impl LanKeys {
    fn from_pairing_code(pairing_code: &str) -> Result<Self, SyncError> {
        let secret = decode_pairing_code(pairing_code)?;
        Ok(Self {
            auth: derive_lan_key(&secret, LAN_AUTH_DOMAIN),
            encryption: derive_lan_key(&secret, LAN_ENCRYPTION_DOMAIN),
        })
    }
}

#[derive(Clone)]
struct EncryptedDelivery {
    id: [u8; 16],
    nonce: [u8; 12],
    ciphertext: Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryOrigin {
    Available,
    AwaitingAck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShareState {
    Available,
    Delivering(DeliveryOrigin),
    AwaitingAck,
    Acking,
    Consumed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionTransition {
    Delivery(DeliveryOrigin),
    Acknowledgement,
}

#[derive(Debug, Clone, Copy)]
struct LanServerOptions {
    max_connections: usize,
    connection_timeout: Duration,
}

impl Default for LanServerOptions {
    fn default() -> Self {
        Self {
            max_connections: MAX_LAN_CONNECTIONS,
            connection_timeout: LAN_CONNECTION_TIMEOUT,
        }
    }
}

pub async fn start_share(
    config: Config,
    ttl: Duration,
) -> Result<(ShareHandle, ShareInfo), SyncError> {
    let host = discover_lan_ip()
        .ok_or_else(|| SyncError::Share("无法确定局域网地址，请检查网络连接后重试".to_string()))?;
    let bind_ip = match host {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    };
    start_share_for_host(config, ttl, bind_ip, host, LanServerOptions::default()).await
}

/// 在指定本地地址启动一次性分享服务。
///
/// 测试、受限网络或用户明确选择本机分享时可传 loopback 地址，避免无意
/// 暴露到其它接口。传入未指定地址时，会从同地址族的局域网接口中选择 URL 主机。
pub async fn start_share_on(
    config: Config,
    ttl: Duration,
    bind_ip: IpAddr,
) -> Result<(ShareHandle, ShareInfo), SyncError> {
    let host = if bind_ip.is_unspecified() {
        discover_lan_ips()
            .into_iter()
            .find(|address| address.is_ipv4() == bind_ip.is_ipv4())
            .ok_or_else(|| {
                SyncError::Share("无法确定局域网地址，请检查网络连接后重试".to_string())
            })?
    } else {
        bind_ip
    };
    start_share_for_host(config, ttl, bind_ip, host, LanServerOptions::default()).await
}

async fn start_share_for_host(
    config: Config,
    ttl: Duration,
    bind_ip: IpAddr,
    host: IpAddr,
    options: LanServerOptions,
) -> Result<(ShareHandle, ShareInfo), SyncError> {
    if ttl.is_zero() {
        return Err(SyncError::Share("分享有效期必须大于 0".to_string()));
    }
    if options.max_connections == 0 || options.connection_timeout.is_zero() {
        return Err(SyncError::Share("局域网服务参数无效".to_string()));
    }
    let payload = SyncEnvelope::new(config).encode()?;
    let listener = TcpListener::bind(SocketAddr::new(bind_ip, 0))
        .await
        .map_err(|error| SyncError::Share(error.to_string()))?;
    let port = listener
        .local_addr()
        .map_err(|error| SyncError::Share(error.to_string()))?
        .port();
    let pairing_code = random_pairing_code();
    let keys = LanKeys::from_pairing_code(&pairing_code)?;
    let delivery = Arc::new(encrypt_delivery(&payload, &keys.encryption)?);
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let state = Arc::new(Mutex::new(ShareState::Available));
    let used_nonces = Arc::new(Mutex::new(NonceCache::new(1024)));
    let connection_limit = Arc::new(Semaphore::new(options.max_connections));
    let handlers = tokio::task::JoinSet::new();
    let expires_at_unix = unix_now().saturating_add(ttl.as_secs());
    let deadline = Instant::now() + ttl;
    let task = tokio::spawn(async move {
        let mut handlers = handlers;
        loop {
            tokio::select! {
                _ = task_cancel.cancelled() => break,
                _ = tokio::time::sleep_until(deadline) => {
                    task_cancel.cancel();
                    break;
                },
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { break };
                    let Ok(permit) = Arc::clone(&connection_limit).try_acquire_owned() else {
                        drop(stream);
                        continue;
                    };
                    let io = TokioIo::new(stream);
                    let delivery = Arc::clone(&delivery);
                    let auth_key = keys.auth;
                    let state = Arc::clone(&state);
                    let used_nonces = Arc::clone(&used_nonces);
                    let request_cancel = task_cancel.clone();
                    let completion_cancel = task_cancel.clone();
                    handlers.spawn(async move {
                        let _permit = permit;
                        let transition = Arc::new(Mutex::new(None));
                        let response_transition = Arc::clone(&transition);
                        let response_state = Arc::clone(&state);
                        let response_used_nonces = Arc::clone(&used_nonces);
                        let service = service_fn(move |request| {
                            share_response(
                                request,
                                ShareServer {
                                    delivery: Arc::clone(&delivery),
                                    auth_key,
                                    state: Arc::clone(&response_state),
                                    used_nonces: Arc::clone(&response_used_nonces),
                                },
                                request_cancel.clone(),
                                Arc::clone(&response_transition),
                            )
                        });
                        let mut builder = http1::Builder::new();
                        builder.max_buf_size(LAN_REQUEST_HEAD_BYTES);
                        let succeeded = matches!(
                            tokio::time::timeout(
                                options.connection_timeout,
                                builder.serve_connection(io, service),
                            )
                            .await,
                            Ok(Ok(()))
                        );
                        let transition = lock_unpoisoned(&transition).take();
                        if finish_connection_transition(&state, transition, succeeded) {
                            completion_cancel.cancel();
                        }
                    });
                }
                completed = handlers.join_next(), if !handlers.is_empty() => {
                    if completed.is_none() {
                        break;
                    }
                }
            }
        }
        handlers.abort_all();
        while handlers.join_next().await.is_some() {}
    });
    let info = ShareInfo {
        url: format!("http://{}:{port}{LAN_CONFIG_PATH}", format_url_host(host)),
        pairing_code,
        expires_at_unix,
    };
    Ok((ShareHandle { cancel, task }, info))
}

/// 一次分享服务的共享状态。按连接克隆，内部都是 `Arc`。
#[derive(Clone)]
struct ShareServer {
    delivery: Arc<EncryptedDelivery>,
    auth_key: [u8; 32],
    state: Arc<Mutex<ShareState>>,
    used_nonces: Arc<Mutex<NonceCache>>,
}

async fn share_response(
    request: Request<Incoming>,
    server: ShareServer,
    cancel: CancellationToken,
    transition: Arc<Mutex<Option<ConnectionTransition>>>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let ShareServer {
        delivery,
        auth_key,
        state,
        used_nonces,
    } = server;
    if cancel.is_cancelled() {
        return Ok(text_response(StatusCode::GONE, "share expired"));
    }
    match (request.method(), request.uri().path()) {
        (&Method::GET, LAN_CONFIG_PATH) => {
            let Some(nonce) = authenticated_request_nonce(&request, &auth_key, None) else {
                return Ok(authentication_required_response());
            };
            if !claim_request_nonce(&used_nonces, 1, nonce) {
                return Ok(text_response(StatusCode::CONFLICT, "request replayed"));
            }
            let origin = {
                let mut state = lock_unpoisoned(&state);
                let origin = match *state {
                    ShareState::Available => DeliveryOrigin::Available,
                    ShareState::AwaitingAck => DeliveryOrigin::AwaitingAck,
                    ShareState::Delivering(_) | ShareState::Acking => {
                        return Ok(text_response(StatusCode::CONFLICT, "share busy"));
                    }
                    ShareState::Consumed => {
                        return Ok(text_response(StatusCode::GONE, "share already consumed"));
                    }
                };
                *state = ShareState::Delivering(origin);
                origin
            };
            *lock_unpoisoned(&transition) = Some(ConnectionTransition::Delivery(origin));
            Ok(delivery_response(&delivery))
        }
        (&Method::POST, LAN_ACK_PATH) => {
            let Some(nonce) = authenticated_request_nonce(&request, &auth_key, Some(&delivery.id))
            else {
                return Ok(authentication_required_response());
            };
            if !claim_request_nonce(&used_nonces, 2, nonce) {
                return Ok(text_response(StatusCode::CONFLICT, "request replayed"));
            }
            {
                let mut state = lock_unpoisoned(&state);
                match *state {
                    ShareState::AwaitingAck => {
                        *state = ShareState::Acking;
                    }
                    ShareState::Delivering(_) => {
                        return Ok(text_response(StatusCode::CONFLICT, "delivery pending"));
                    }
                    ShareState::Available => {
                        return Ok(text_response(StatusCode::CONFLICT, "nothing delivered"));
                    }
                    ShareState::Acking => {
                        return Ok(text_response(StatusCode::CONFLICT, "share busy"));
                    }
                    ShareState::Consumed => {
                        return Ok(text_response(StatusCode::GONE, "share already consumed"));
                    }
                }
            }
            *lock_unpoisoned(&transition) = Some(ConnectionTransition::Acknowledgement);
            Ok(empty_response(StatusCode::NO_CONTENT))
        }
        _ => Ok(text_response(StatusCode::NOT_FOUND, "not found")),
    }
}

fn text_response(status: StatusCode, body: &'static str) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from_static(body.as_bytes())));
    *response.status_mut() = status;
    add_common_lan_response_headers(&mut response);
    if let Ok(length) = hyper::header::HeaderValue::from_str(&body.len().to_string()) {
        response.headers_mut().insert(CONTENT_LENGTH, length);
    }
    response
}

fn empty_response(status: StatusCode) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::new()));
    *response.status_mut() = status;
    add_common_lan_response_headers(&mut response);
    response
        .headers_mut()
        .insert(CONTENT_LENGTH, hyper::header::HeaderValue::from_static("0"));
    response
}

fn authentication_required_response() -> Response<Full<Bytes>> {
    let mut response = text_response(StatusCode::UNAUTHORIZED, "authentication required");
    response.headers_mut().insert(
        WWW_AUTHENTICATE,
        hyper::header::HeaderValue::from_static("KitonyTerms-HMAC"),
    );
    response
}

fn delivery_response(delivery: &EncryptedDelivery) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(delivery.ciphertext.clone()));
    *response.status_mut() = StatusCode::OK;
    add_common_lan_response_headers(&mut response);
    response.headers_mut().insert(
        CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("application/octet-stream"),
    );
    insert_header(
        response.headers_mut(),
        LAN_HEADER_RESPONSE_NONCE,
        &hex::encode(delivery.nonce),
    );
    insert_header(
        response.headers_mut(),
        LAN_HEADER_DELIVERY,
        &hex::encode(delivery.id),
    );
    insert_header(
        response.headers_mut(),
        CONTENT_LENGTH.as_str(),
        &delivery.ciphertext.len().to_string(),
    );
    response
}

fn add_common_lan_response_headers(response: &mut Response<Full<Bytes>>) {
    response.headers_mut().insert(
        CACHE_CONTROL,
        hyper::header::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        hyper::header::CONNECTION,
        hyper::header::HeaderValue::from_static("close"),
    );
    response.headers_mut().insert(
        hyper::header::HeaderName::from_static(LAN_HEADER_PROTOCOL),
        hyper::header::HeaderValue::from_static(LAN_PROTOCOL_VERSION),
    );
}

pub async fn import_share(
    url: &str,
    pairing_code: &str,
    cancel: &CancellationToken,
) -> Result<PendingShareImport, SyncError> {
    let mut url = validate_http_url(url)?;
    if url.path() != LAN_CONFIG_PATH {
        return Err(SyncError::InvalidUrl(format!(
            "局域网分享路径必须为 {LAN_CONFIG_PATH}"
        )));
    }
    let keys = LanKeys::from_pairing_code(pairing_code)?;
    let client = Client::builder()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .build()?;
    let future = async {
        let request_nonce = random_bytes::<16>();
        let request = authenticated_request(
            client.get(url.clone()),
            &Method::GET,
            LAN_CONFIG_PATH,
            &keys.auth,
            &request_nonce,
            None,
        );
        let mut response = request.send().await?;
        ensure_success(response.status())?;
        if response
            .content_length()
            .is_some_and(|size| size > MAX_ENCRYPTED_ENVELOPE_BYTES as u64)
        {
            return Err(SyncError::TooLarge);
        }
        let response_nonce = parse_response_header::<12>(&response, LAN_HEADER_RESPONSE_NONCE)?;
        let delivery_id = parse_response_header::<16>(&response, LAN_HEADER_DELIVERY)?;
        let protocol = response
            .headers()
            .get(LAN_HEADER_PROTOCOL)
            .and_then(|value| value.to_str().ok());
        if protocol != Some(LAN_PROTOCOL_VERSION) {
            return Err(SyncError::Share("局域网响应协议版本无效".to_string()));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len().saturating_add(chunk.len()) > MAX_ENCRYPTED_ENVELOPE_BYTES {
                return Err(SyncError::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        let plaintext = decrypt_delivery(&bytes, &keys.encryption, &response_nonce, &delivery_id)?;
        let envelope = SyncEnvelope::decode(&plaintext)?;
        url.set_path(LAN_ACK_PATH);
        url.set_query(None);
        Ok(PendingShareImport {
            envelope,
            client,
            ack_url: url,
            auth_key: keys.auth,
            delivery_id,
        })
    };
    run_cancelled(cancel, DEFAULT_REQUEST_TIMEOUT, future).await
}

fn authenticated_request(
    request: reqwest::RequestBuilder,
    method: &Method,
    path: &str,
    auth_key: &[u8; 32],
    nonce: &[u8; 16],
    delivery_id: Option<&[u8; 16]>,
) -> reqwest::RequestBuilder {
    let mac = request_mac(auth_key, method, path, nonce, delivery_id);
    let mut request = request
        .header(LAN_HEADER_PROTOCOL, LAN_PROTOCOL_VERSION)
        .header(LAN_HEADER_NONCE, hex::encode(nonce))
        .header(LAN_HEADER_AUTH, hex::encode(mac));
    if let Some(delivery_id) = delivery_id {
        request = request.header(LAN_HEADER_DELIVERY, hex::encode(delivery_id));
    }
    request
}

fn authenticated_request_nonce(
    request: &Request<Incoming>,
    auth_key: &[u8; 32],
    expected_delivery_id: Option<&[u8; 16]>,
) -> Option<[u8; 16]> {
    if request
        .headers()
        .get(LAN_HEADER_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        != Some(LAN_PROTOCOL_VERSION)
    {
        return None;
    }
    let nonce = parse_hex_header::<16>(request.headers(), LAN_HEADER_NONCE)?;
    let provided_mac = parse_hex_header::<32>(request.headers(), LAN_HEADER_AUTH)?;
    let delivery_id = match expected_delivery_id {
        Some(expected) => {
            let actual = parse_hex_header::<16>(request.headers(), LAN_HEADER_DELIVERY)?;
            if actual != *expected {
                return None;
            }
            Some(actual)
        }
        None => None,
    };
    let expected_mac = request_mac(
        auth_key,
        request.method(),
        request.uri().path(),
        &nonce,
        delivery_id.as_ref(),
    );
    bool::from(expected_mac.ct_eq(&provided_mac)).then_some(nonce)
}

fn claim_request_nonce(used_nonces: &Mutex<NonceCache>, scope: u8, nonce: [u8; 16]) -> bool {
    lock_unpoisoned(used_nonces).claim(scope, nonce)
}

fn request_mac(
    auth_key: &[u8; 32],
    method: &Method,
    path: &str,
    nonce: &[u8; 16],
    delivery_id: Option<&[u8; 16]>,
) -> [u8; 32] {
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(auth_key).expect("HMAC accepts any key length");
    mac.update(LAN_AUTH_DOMAIN);
    mac.update(&[0]);
    mac.update(method.as_str().as_bytes());
    mac.update(&[0]);
    mac.update(path.as_bytes());
    mac.update(&[0]);
    mac.update(nonce);
    mac.update(&[0]);
    if let Some(delivery_id) = delivery_id {
        mac.update(delivery_id);
    }
    mac.finalize().into_bytes().into()
}

fn derive_lan_key(pairing_code: &[u8], domain: &[u8]) -> [u8; 32] {
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(pairing_code).expect("HMAC accepts any key length");
    mac.update(domain);
    mac.finalize().into_bytes().into()
}

fn encrypt_delivery(
    plaintext: &[u8],
    encryption_key: &[u8; 32],
) -> Result<EncryptedDelivery, SyncError> {
    let id = random_bytes::<16>();
    let nonce = random_bytes::<12>();
    let cipher = ChaCha20Poly1305::new_from_slice(encryption_key)
        .map_err(|_| SyncError::Share("局域网加密密钥无效".to_string()))?;
    let aad = response_aad(&id);
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| SyncError::Share("局域网配置加密失败".to_string()))?;
    Ok(EncryptedDelivery {
        id,
        nonce,
        ciphertext: Bytes::from(ciphertext),
    })
}

fn decrypt_delivery(
    ciphertext: &[u8],
    encryption_key: &[u8; 32],
    nonce: &[u8; 12],
    delivery_id: &[u8; 16],
) -> Result<Vec<u8>, SyncError> {
    let cipher = ChaCha20Poly1305::new_from_slice(encryption_key)
        .map_err(|_| SyncError::Share("局域网加密密钥无效".to_string()))?;
    let aad = response_aad(delivery_id);
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| SyncError::Share("局域网响应认证失败".to_string()))
}

fn response_aad(delivery_id: &[u8; 16]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(LAN_RESPONSE_DOMAIN.len() + 1 + delivery_id.len());
    aad.extend_from_slice(LAN_RESPONSE_DOMAIN);
    aad.push(0);
    aad.extend_from_slice(delivery_id);
    aad
}

fn finish_connection_transition(
    state: &Mutex<ShareState>,
    transition: Option<ConnectionTransition>,
    succeeded: bool,
) -> bool {
    let Some(transition) = transition else {
        return false;
    };
    let mut state = lock_unpoisoned(state);
    match (transition, *state) {
        (ConnectionTransition::Delivery(origin), ShareState::Delivering(current_origin))
            if origin == current_origin =>
        {
            *state = if succeeded {
                ShareState::AwaitingAck
            } else {
                match origin {
                    DeliveryOrigin::Available => ShareState::Available,
                    DeliveryOrigin::AwaitingAck => ShareState::AwaitingAck,
                }
            };
            false
        }
        (ConnectionTransition::Acknowledgement, ShareState::Acking) => {
            *state = if succeeded {
                ShareState::Consumed
            } else {
                ShareState::AwaitingAck
            };
            succeeded
        }
        _ => false,
    }
}

fn parse_response_header<const N: usize>(
    response: &reqwest::Response,
    name: &'static str,
) -> Result<[u8; N], SyncError> {
    parse_hex_header(response.headers(), name)
        .ok_or_else(|| SyncError::Share(format!("局域网响应头 {name} 无效")))
}

fn parse_hex_header<const N: usize>(
    headers: &hyper::HeaderMap,
    name: &'static str,
) -> Option<[u8; N]> {
    let value = headers.get(name)?.to_str().ok()?;
    let bytes = hex::decode(value).ok()?;
    bytes.try_into().ok()
}

fn insert_header(headers: &mut hyper::HeaderMap, name: &'static str, value: &str) {
    let Ok(value) = hyper::header::HeaderValue::from_str(value) else {
        return;
    };
    headers.insert(hyper::header::HeaderName::from_static(name), value);
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn validate_http_url(value: &str) -> Result<Url, SyncError> {
    let url = Url::parse(value.trim()).map_err(|error| SyncError::InvalidUrl(error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(SyncError::InvalidUrl("仅支持 http/https".to_string()));
    }
    if url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
        return Err(SyncError::InvalidUrl(
            "地址必须包含主机，且认证信息不能写入 URL".to_string(),
        ));
    }
    if url.fragment().is_some() {
        return Err(SyncError::InvalidUrl("地址不能包含 fragment".to_string()));
    }
    Ok(url)
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    })
}

/// 配对负载的 URI scheme。二维码里同时带上地址与配对码，扫一次即可完成导入。
const SHARE_PAYLOAD_SCHEME: &str = "kitonyterms://lan-share";

/// 把地址与配对码编码为二维码负载。
///
/// 用自定义 scheme 而不是裸 URL：避免第三方扫码器把它当网页直接打开，同时让
/// 本应用能明确识别这是配对信息。
pub fn encode_share_payload(url: &str, pairing_code: &str) -> String {
    format!(
        "{SHARE_PAYLOAD_SCHEME}?v={LAN_PROTOCOL_VERSION}&url={}&code={}",
        urlencoding_minimal(url),
        normalize_pairing_code(pairing_code)
    )
}

/// 解析扫码结果，返回 `(地址, 配对码)`。
///
/// 同时容忍用户直接粘贴分享地址（此时配对码为空，需要手动补充）。
pub fn parse_share_payload(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    if let Some(query) = text.strip_prefix(&format!("{SHARE_PAYLOAD_SCHEME}?")) {
        let mut version = None;
        let mut url = None;
        let mut code = None;
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=')?;
            match key {
                "v" => version = Some(value),
                "url" => url = Some(urldecoding_minimal(value)),
                "code" => code = Some(normalize_pairing_code(value)),
                _ => {}
            }
        }
        if version != Some(LAN_PROTOCOL_VERSION) {
            return None;
        }
        return Some((url?, code.unwrap_or_default()));
    }
    // 裸地址：只认 http(s)，配对码留空由用户输入。
    if text.starts_with("http://") || text.starts_with("https://") {
        return Some((text.to_string(), String::new()));
    }
    None
}

/// 只转义会破坏负载结构的字符。配对负载里的 URL 形如
/// `http://192.168.1.20:12345/v2/config`，不含需要完整百分号编码的字符。
fn urlencoding_minimal(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '%' => "%25".to_string(),
            '&' => "%26".to_string(),
            '=' => "%3D".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

fn urldecoding_minimal(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        let hi = chars.next();
        let lo = chars.next();
        match (hi, lo) {
            (Some(hi), Some(lo)) => {
                match u8::from_str_radix(&format!("{hi}{lo}"), 16) {
                    Ok(byte) => out.push(byte as char),
                    // 非法转义原样保留，让上层的 URL 校验去报错。
                    Err(_) => {
                        out.push('%');
                        out.push(hi);
                        out.push(lo);
                    }
                }
            }
            _ => out.push('%'),
        }
    }
    out
}
/// 配对码字母表：Crockford Base32 去掉 I/L/O/U，避免与 1/0 混淆且不会拼出词。
const PAIRING_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// 26 个字符提供 130 位随机熵，可直接作为现有 HMAC/AEAD 派生的根秘密。
pub const PAIRING_CODE_LEN: usize = 26;

/// 生成人可输入的配对码。32 整除 256，字节取模无模偏，无需拒绝采样。
fn random_pairing_code() -> String {
    random_bytes::<PAIRING_CODE_LEN>()
        .into_iter()
        .map(|byte| PAIRING_ALPHABET[(byte % 32) as usize] as char)
        .collect()
}

/// 归一化用户输入的配对码：忽略大小写、空格与连字符，并把易混字符映射回
/// 字母表（O→0、I/L→1）。U 和其它非法字符由解码器明确拒绝。
pub fn normalize_pairing_code(input: &str) -> String {
    input
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace() && *ch != '-')
        .map(|ch| match ch.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .collect()
}

/// 把配对码解码为派生密钥的原始秘密。
fn decode_pairing_code(pairing_code: &str) -> Result<Vec<u8>, SyncError> {
    let normalized = normalize_pairing_code(pairing_code);
    if normalized.len() != PAIRING_CODE_LEN {
        return Err(SyncError::Share(format!(
            "配对码应为 {PAIRING_CODE_LEN} 位字符"
        )));
    }
    if !normalized
        .bytes()
        .all(|byte| PAIRING_ALPHABET.contains(&byte))
    {
        return Err(SyncError::Share("配对码包含无效字符".to_string()));
    }
    Ok(normalized.into_bytes())
}

fn discover_lan_ip() -> Option<IpAddr> {
    discover_lan_ips().into_iter().next()
}

/// 候选局域网地址，按「越可能是真实局域网」排序。
///
/// VPN 开启时系统路由表会把默认路由指向隧道，靠 UDP connect 探测出口地址只会拿到
/// 隧道内网地址，对端根本连不上。所以这里不看路由，只按接口自身特征排序：真实的
/// 以太网/Wi-Fi 接口是广播型且地址在 RFC1918 私有段内，隧道接口通常是点对点、
/// 名字形如 utun/tun/wg，或地址落在 CGNAT、fake-ip 等隧道专用段。
fn discover_lan_ips() -> Vec<IpAddr> {
    let mut candidates = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|interface| !interface.is_loopback())
        .filter(|interface| is_usable_lan_ip(interface.ip()))
        .map(|interface| (interface_rank(&interface), interface.ip()))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(rank, address)| (*rank, *address));
    let mut addresses = candidates
        .into_iter()
        .map(|(_, address)| address)
        .collect::<Vec<_>>();
    addresses.dedup();
    addresses
}

/// 排序键：先把疑似 VPN/隧道的接口排到后面，再套用地址族优先级。
fn interface_rank(interface: &Interface) -> (u8, u8) {
    let tunnel = u8::from(is_tunnel_interface(interface));
    (tunnel, lan_ip_priority(interface.ip()))
}

/// 隧道接口名前缀。覆盖各平台的通用 VPN 虚拟网卡与常见客户端自建网卡。
const TUNNEL_NAME_PREFIXES: &[&str] = &[
    "utun",
    "tun",
    "tap",
    "ppp",
    "ipsec",
    "wg",
    "nordlynx",
    "proton",
    "tailscale",
    "zt",
    "clash",
    "surge",
    "wireguard",
    "openvpn",
];

/// 判断接口是否疑似 VPN/隧道。三个独立信号任一命中即降权：
/// 名字前缀、IPv4 缺少广播地址（点对点），以及隧道专用地址段。
fn is_tunnel_interface(interface: &Interface) -> bool {
    let name = interface.name.to_ascii_lowercase();
    if TUNNEL_NAME_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        return true;
    }
    // Windows 用的是可读别名，只能靠关键字兜底。
    if name.contains("vpn") || name.contains("tunnel") {
        return true;
    }
    match &interface.addr {
        // 真实以太网/Wi-Fi 一定有广播地址；点对点隧道没有。
        IfAddr::V4(v4) => v4.broadcast.is_none() || is_tunnel_only_v4(v4.ip),
        IfAddr::V6(_) => false,
    }
}

/// 只可能出现在隧道里的 IPv4 段：CGNAT（Tailscale 等）与 benchmark 段
/// （Clash/Surge 的 fake-ip 默认段）。这些地址不可能是家用/办公局域网。
fn is_tunnel_only_v4(address: Ipv4Addr) -> bool {
    let [a, b, ..] = address.octets();
    matches!((a, b), (100, 64..=127) | (198, 18 | 19))
}

fn is_usable_lan_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            !address.is_unspecified()
                && !address.is_loopback()
                && !address.is_multicast()
                && address != Ipv4Addr::BROADCAST
        }
        IpAddr::V6(address) => {
            !address.is_unspecified()
                && !address.is_loopback()
                && !address.is_multicast()
                && !is_ipv6_link_local(address)
        }
    }
}

fn lan_ip_priority(address: IpAddr) -> u8 {
    match address {
        IpAddr::V4(address) if address.is_private() => 0,
        IpAddr::V6(address) if address.is_unique_local() => 1,
        IpAddr::V4(address) if !address.is_link_local() => 2,
        IpAddr::V6(_) => 3,
        IpAddr::V4(_) => 4,
    }
}

fn is_ipv6_link_local(address: Ipv6Addr) -> bool {
    address.segments()[0] & 0xffc0 == 0xfe80
}

fn format_url_host(host: IpAddr) -> String {
    match host {
        IpAddr::V4(host) => host.to_string(),
        IpAddr::V6(host) => format!("[{host}]"),
    }
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kt_config::{AppSettings, ConnectParams, SessionProfile};

    fn sample_config() -> Config {
        Config {
            settings: AppSettings {
                font_size: 17.0,
                ..AppSettings::default()
            },
            groups: vec!["production".to_string()],
            sessions: vec![SessionProfile {
                name: "prod".to_string(),
                group: Some("production".to_string()),
                params: ConnectParams::new("server.example", "root"),
            }],
        }
    }

    const TEST_PAIRING_CODE: &str = "0123456789ABCDEFGHJKMNPQRS";

    #[test]
    fn envelope_round_trip_keeps_non_secret_config() {
        let bytes = SyncEnvelope::new(sample_config()).encode().unwrap();
        let decoded = SyncEnvelope::decode(&bytes).unwrap();
        assert_eq!(decoded.config.settings.font_size, 17.0);
        assert_eq!(decoded.config.groups, ["production"]);
        assert_eq!(decoded.config.sessions.len(), 1);
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains("secrets.vault"));
        assert!(!text.contains("known_hosts"));
    }

    #[test]
    fn envelope_rejects_unknown_version_and_oversize() {
        let mut envelope = SyncEnvelope::new(Config::default());
        envelope.version = 99;
        assert!(matches!(
            envelope.encode(),
            Err(SyncError::UnsupportedEnvelope(_))
        ));
        assert!(matches!(
            SyncEnvelope::decode(&vec![b'x'; MAX_ENVELOPE_BYTES + 1]),
            Err(SyncError::TooLarge)
        ));
    }

    #[test]
    fn webdav_rejects_credentials_over_plain_http() {
        assert!(matches!(
            WebDavEndpoint::parse(
                "http://example.test/config.json",
                Some("user".to_string()),
                Some("password".to_string())
            ),
            Err(SyncError::InsecureCredentials)
        ));
        assert!(WebDavEndpoint::parse(
            "https://example.test/config.json?rev=1",
            Some("user".to_string()),
            Some("password".to_string())
        )
        .is_ok());
    }

    #[tokio::test]
    async fn lan_share_requires_hmac_without_global_failure_cancellation() {
        let (handle, info) = start_share_on(
            sample_config(),
            Duration::from_secs(5),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        )
        .await
        .unwrap();
        let client = Client::new();
        for _ in 0..16 {
            let unauthorized = client.get(&info.url).send().await.unwrap();
            assert_eq!(unauthorized.status(), reqwest::StatusCode::UNAUTHORIZED);
        }

        let cancel = CancellationToken::new();
        let imported = import_share(&info.url, &info.pairing_code, &cancel)
            .await
            .unwrap();
        assert_eq!(imported.config.settings.font_size, 17.0);

        let second = import_share(&info.url, &info.pairing_code, &cancel)
            .await
            .unwrap();
        assert_eq!(second.config.settings.font_size, 17.0);
        drop(second);

        imported.acknowledge(&cancel).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !handle.is_finished() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let third = import_share(&info.url, &info.pairing_code, &cancel).await;
        assert!(third.is_err());
    }

    #[tokio::test]
    async fn lan_share_rejects_wrong_pairing_code_without_consuming() {
        let (handle, info) = start_share_on(
            sample_config(),
            Duration::from_secs(5),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        )
        .await
        .unwrap();
        let cancel = CancellationToken::new();
        let wrong_code = if info.pairing_code.starts_with('0') {
            "1".repeat(PAIRING_CODE_LEN)
        } else {
            "0".repeat(PAIRING_CODE_LEN)
        };
        assert!(import_share(&info.url, &wrong_code, &cancel).await.is_err());
        assert!(import_share(&info.url, &info.pairing_code, &cancel)
            .await
            .is_ok());
        handle.stop();
    }

    #[test]
    fn encrypted_delivery_rejects_tampering() {
        let keys = LanKeys::from_pairing_code(TEST_PAIRING_CODE).unwrap();
        let delivery = encrypt_delivery(b"config payload", &keys.encryption).unwrap();
        let mut tampered = delivery.ciphertext.to_vec();
        tampered[0] ^= 1;
        assert!(
            decrypt_delivery(&tampered, &keys.encryption, &delivery.nonce, &delivery.id).is_err()
        );
    }

    #[test]
    fn lan_ip_filter_supports_private_ipv4_and_unique_local_ipv6() {
        assert!(is_usable_lan_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 4))));
        assert!(is_usable_lan_ip(IpAddr::V6("fd00::4".parse().unwrap())));
        assert!(!is_usable_lan_ip(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(!is_usable_lan_ip(IpAddr::V6("fe80::4".parse().unwrap())));
        assert_eq!(
            format_url_host(IpAddr::V6("fd00::4".parse().unwrap())),
            "[fd00::4]"
        );
    }

    fn v4_interface(name: &str, ip: Ipv4Addr, broadcast: Option<Ipv4Addr>) -> Interface {
        Interface {
            name: name.to_string(),
            addr: IfAddr::V4(if_addrs::Ifv4Addr {
                ip,
                netmask: Ipv4Addr::new(255, 255, 255, 0),
                prefixlen: 24,
                broadcast,
            }),
            index: None,
            #[cfg(windows)]
            adapter_name: name.to_string(),
        }
    }

    #[test]
    fn share_payload_round_trips_url_and_code() {
        let url = "http://192.168.1.20:12345/v2/config";
        let payload = encode_share_payload(url, "0123-4567-89ab-cdef-ghjk-mnpq-rs");
        let (parsed_url, parsed_code) = parse_share_payload(&payload).unwrap();
        assert_eq!(parsed_url, url);
        // 负载里保存的是归一化后的配对码。
        assert_eq!(parsed_code, TEST_PAIRING_CODE);
        assert!(LanKeys::from_pairing_code(&parsed_code).is_ok());
    }

    #[test]
    fn share_payload_accepts_a_bare_url_without_a_code() {
        let (url, code) = parse_share_payload(" http://192.168.1.20:12345/v2/config ").unwrap();
        assert_eq!(url, "http://192.168.1.20:12345/v2/config");
        assert!(code.is_empty());
    }

    #[test]
    fn share_payload_rejects_unrelated_text() {
        assert!(parse_share_payload("hello world").is_none());
        assert!(parse_share_payload("ftp://example.com").is_none());
        assert!(parse_share_payload(
            "kitonyterms://lan-share?v=1&url=http://host/v1/config&code=ABCD2345"
        )
        .is_none());
    }

    #[test]
    fn share_payload_escapes_structural_characters() {
        // URL 里出现 & / = 时不能破坏负载的键值结构。
        let payload = encode_share_payload("http://h/v2/config?a=1&b=2", TEST_PAIRING_CODE);
        let (url, code) = parse_share_payload(&payload).unwrap();
        assert_eq!(url, "http://h/v2/config?a=1&b=2");
        assert_eq!(code, TEST_PAIRING_CODE);
    }

    #[test]
    fn pairing_code_has_high_entropy_and_uses_unambiguous_alphabet() {
        for _ in 0..64 {
            let code = random_pairing_code();
            assert_eq!(code.len(), PAIRING_CODE_LEN);
            assert!(code.bytes().all(|byte| PAIRING_ALPHABET.contains(&byte)));
            // 生成的配对码必须能被自己的解码器接受。
            assert!(decode_pairing_code(&code).is_ok());
        }
    }

    #[test]
    fn pairing_code_input_tolerates_case_separators_and_lookalikes() {
        // 小写、空格、连字符都应归一化到同一个秘密。
        let canonical = decode_pairing_code(TEST_PAIRING_CODE).unwrap();
        assert_eq!(
            decode_pairing_code("0123-4567-89ab-cdef-ghjk-mnpq-rs").unwrap(),
            canonical
        );
        assert_eq!(
            decode_pairing_code(" 0123456789abcdefghjkmnpqrs ").unwrap(),
            canonical
        );
        // O/I/L 映射回 0/1，避免手抄歧义。
        assert_eq!(
            decode_pairing_code("OI23456789ABCDEFGHJKMNPQRS").unwrap(),
            decode_pairing_code("0123456789ABCDEFGHJKMNPQRS").unwrap()
        );
    }

    #[test]
    fn pairing_code_rejects_wrong_length_and_invalid_characters() {
        assert!(decode_pairing_code("ABC").is_err());
        assert!(decode_pairing_code("ABCD234567890ABCDEFGHJKMNPQRS").is_err());
        assert!(decode_pairing_code("0123456789ABCDEFGHJKMNPQRSU").is_err());
        assert!(decode_pairing_code("").is_err());
    }

    #[test]
    fn legacy_pairing_codes_are_rejected() {
        assert!(decode_pairing_code(&"AB".repeat(16)).is_err());
        assert!(LanKeys::from_pairing_code(&"AB".repeat(16)).is_err());
    }

    /// 在真实网络环境上跑一遍：如果本机同时存在普通网卡与隧道网卡，
    /// 首选地址必须来自非隧道网卡，否则开着 VPN 时分享出去的地址对方连不上。
    #[test]
    fn discovery_prefers_a_non_tunnel_address_on_this_host() {
        let interfaces = if_addrs::get_if_addrs().unwrap_or_default();
        let has_real_lan = interfaces.iter().any(|interface| {
            !interface.is_loopback()
                && is_usable_lan_ip(interface.ip())
                && !is_tunnel_interface(interface)
        });
        let discovered = discover_lan_ips();

        if !has_real_lan {
            // CI 容器里可能只有回环，此时不做断言。
            return;
        }
        let first = discovered.first().expect("存在可用网卡时必须发现地址");
        let owner = interfaces
            .iter()
            .find(|interface| interface.ip() == *first)
            .expect("首选地址必须来自某个真实网卡");
        assert!(
            !is_tunnel_interface(owner),
            "首选地址来自隧道网卡 {}（{first}），VPN 开启时会导致对端无法连接",
            owner.name
        );
    }

    #[test]
    fn real_lan_interface_outranks_vpn_tunnel() {
        // 典型的 VPN 场景：utun 隧道与真实 Wi-Fi 同时在线。
        let wifi = v4_interface(
            "en0",
            Ipv4Addr::new(192, 168, 32, 117),
            Some(Ipv4Addr::new(192, 168, 32, 255)),
        );
        let tunnel = v4_interface("utun6", Ipv4Addr::new(198, 18, 0, 1), None);
        assert!(!is_tunnel_interface(&wifi));
        assert!(is_tunnel_interface(&tunnel));
        assert!(interface_rank(&wifi) < interface_rank(&tunnel));
    }

    #[test]
    fn tunnel_detection_uses_name_broadcast_and_address_range() {
        // 名字命中。
        assert!(is_tunnel_interface(&v4_interface(
            "wg0",
            Ipv4Addr::new(192, 168, 9, 2),
            Some(Ipv4Addr::new(192, 168, 9, 255)),
        )));
        // 缺少广播地址（点对点）。
        assert!(is_tunnel_interface(&v4_interface(
            "ppp0",
            Ipv4Addr::new(10, 8, 0, 2),
            None,
        )));
        // CGNAT 段：Tailscale 之类的隧道专用地址。
        assert!(is_tunnel_interface(&v4_interface(
            "eth9",
            Ipv4Addr::new(100, 100, 5, 5),
            Some(Ipv4Addr::new(100, 100, 5, 255)),
        )));
        // 正常私有段的广播型接口不应被误判。
        assert!(!is_tunnel_interface(&v4_interface(
            "eth0",
            Ipv4Addr::new(10, 0, 1, 20),
            Some(Ipv4Addr::new(10, 0, 1, 255)),
        )));
    }

    #[test]
    fn request_mac_changes_with_path_and_nonce() {
        let key = [7u8; 32];
        let nonce = [8u8; 16];
        let other_nonce = [9u8; 16];
        assert_ne!(
            request_mac(&key, &Method::GET, LAN_CONFIG_PATH, &nonce, None),
            request_mac(&key, &Method::GET, LAN_CONFIG_PATH, &other_nonce, None)
        );
        assert_ne!(
            request_mac(&key, &Method::GET, LAN_CONFIG_PATH, &nonce, None),
            request_mac(&key, &Method::POST, LAN_ACK_PATH, &nonce, None)
        );
    }

    #[test]
    fn authenticated_request_nonce_rejects_replay_claims() {
        let used_nonces = Mutex::new(NonceCache::new(2));
        let nonce = [4u8; 16];
        assert!(claim_request_nonce(&used_nonces, 1, nonce));
        assert!(!claim_request_nonce(&used_nonces, 1, nonce));
        assert!(claim_request_nonce(&used_nonces, 2, nonce));
        assert!(claim_request_nonce(&used_nonces, 1, [5u8; 16]));
        assert!(claim_request_nonce(&used_nonces, 1, nonce));
    }

    #[test]
    fn failed_connection_transitions_roll_back_for_retry() {
        let state = Mutex::new(ShareState::Delivering(DeliveryOrigin::Available));
        assert!(!finish_connection_transition(
            &state,
            Some(ConnectionTransition::Delivery(DeliveryOrigin::Available)),
            false,
        ));
        assert_eq!(*lock_unpoisoned(&state), ShareState::Available);

        *lock_unpoisoned(&state) = ShareState::Delivering(DeliveryOrigin::AwaitingAck);
        assert!(!finish_connection_transition(
            &state,
            Some(ConnectionTransition::Delivery(DeliveryOrigin::AwaitingAck)),
            false,
        ));
        assert_eq!(*lock_unpoisoned(&state), ShareState::AwaitingAck);

        *lock_unpoisoned(&state) = ShareState::Acking;
        assert!(!finish_connection_transition(
            &state,
            Some(ConnectionTransition::Acknowledgement),
            false,
        ));
        assert_eq!(*lock_unpoisoned(&state), ShareState::AwaitingAck);
    }

    #[test]
    fn successful_ack_is_the_only_consuming_transition() {
        let state = Mutex::new(ShareState::Delivering(DeliveryOrigin::Available));
        assert!(!finish_connection_transition(
            &state,
            Some(ConnectionTransition::Delivery(DeliveryOrigin::Available)),
            true,
        ));
        assert_eq!(*lock_unpoisoned(&state), ShareState::AwaitingAck);

        *lock_unpoisoned(&state) = ShareState::Acking;
        assert!(finish_connection_transition(
            &state,
            Some(ConnectionTransition::Acknowledgement),
            true,
        ));
        assert_eq!(*lock_unpoisoned(&state), ShareState::Consumed);
    }

    #[tokio::test]
    async fn lan_request_does_not_send_pairing_code() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let pairing_code = TEST_PAIRING_CODE.to_string();
        let pairing_code_for_server = pairing_code.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0u8; LAN_REQUEST_HEAD_BYTES];
            let size = stream.read(&mut request).await.unwrap();
            request.truncate(size);
            stream
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            assert!(!String::from_utf8_lossy(&request).contains(&pairing_code_for_server));
        });
        let result = import_share(
            &format!("http://{address}{LAN_CONFIG_PATH}"),
            &pairing_code,
            &CancellationToken::new(),
        )
        .await;
        assert!(result.is_err());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn lan_connection_limit_releases_after_timeout() {
        use tokio::net::TcpStream;

        let (handle, info) = start_share_for_host(
            sample_config(),
            Duration::from_secs(5),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            LanServerOptions {
                max_connections: 1,
                connection_timeout: Duration::from_millis(150),
            },
        )
        .await
        .unwrap();
        let address = Url::parse(&info.url).unwrap();
        let slow = TcpStream::connect((address.host_str().unwrap(), address.port().unwrap()))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;

        let busy = import_share(&info.url, &info.pairing_code, &CancellationToken::new()).await;
        assert!(busy.is_err());

        tokio::time::sleep(Duration::from_millis(180)).await;
        let imported = import_share(&info.url, &info.pairing_code, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(imported.config.settings.font_size, 17.0);
        drop(slow);
        handle.stop();
    }

    #[test]
    fn pairing_code_is_not_part_of_request_mac_or_ciphertext() {
        let pairing_code = TEST_PAIRING_CODE.to_string();
        let keys = LanKeys::from_pairing_code(&pairing_code).unwrap();
        let delivery = encrypt_delivery(pairing_code.as_bytes(), &keys.encryption).unwrap();
        assert!(!delivery
            .ciphertext
            .windows(pairing_code.len())
            .any(|window| window == pairing_code.as_bytes()));
        assert!(!hex::encode(request_mac(
            &keys.auth,
            &Method::GET,
            LAN_CONFIG_PATH,
            &[0; 16],
            None
        ))
        .contains(&pairing_code));
    }

    #[tokio::test]
    async fn webdav_upload_sends_etag_precondition_and_maps_conflict() {
        use std::sync::Mutex;
        use tokio::sync::oneshot;

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let (header_tx, header_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let io = TokioIo::new(stream);
            let sender = Arc::new(Mutex::new(Some(header_tx)));
            let service = service_fn(move |request: Request<Incoming>| {
                let sender = Arc::clone(&sender);
                async move {
                    let value = request
                        .headers()
                        .get(IF_MATCH)
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_string);
                    if let Some(sender) = sender.lock().unwrap().take() {
                        let _ = sender.send(value);
                    }
                    Ok::<_, Infallible>(
                        Response::builder()
                            .status(StatusCode::PRECONDITION_FAILED)
                            .header(hyper::header::CONNECTION, "close")
                            .body(Full::new(Bytes::new()))
                            .unwrap(),
                    )
                }
            });
            http1::Builder::new()
                .serve_connection(io, service)
                .await
                .unwrap();
        });

        let endpoint =
            WebDavEndpoint::parse(&format!("http://{address}/config.json"), None, None).unwrap();
        let client = WebDavClient::new().unwrap();
        let result = client
            .upload(
                &endpoint,
                &SyncEnvelope::new(Config::default()),
                PutPrecondition::IfMatch("\"revision-7\""),
                &CancellationToken::new(),
            )
            .await;

        assert!(matches!(result, Err(SyncError::Conflict)));
        assert_eq!(header_rx.await.unwrap().as_deref(), Some("\"revision-7\""));
        drop(client);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn lan_share_rejects_zero_ttl() {
        let result = start_share_on(
            Config::default(),
            Duration::ZERO,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        )
        .await;
        assert!(matches!(result, Err(SyncError::Share(_))));
    }
}
