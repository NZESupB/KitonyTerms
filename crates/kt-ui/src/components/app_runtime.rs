//! App 与 core/store 之间的运行时适配。

use std::sync::Arc;

use kt_config::{ConnectParams, KnownHostCheck};
use kt_core::ssh::HostKeyDecision;
use kt_core::{AuthProvider, HostKeyVerifier};

use crate::store::Store;

/// AuthProvider 实现(从 Store 读取密码)。
struct StoreAuthProvider {
    store: Arc<Store>,
    vault_id: String,
}

impl AuthProvider for StoreAuthProvider {
    fn password(&mut self, user: &str, host: &str, port: u16) -> Option<String> {
        let scoped_vault_id = format!("{user}@{host}:{port}");
        match self.store.get_secret(&scoped_vault_id) {
            Ok(Some(password)) => Some(password),
            Ok(None) => match self.store.get_secret(&self.vault_id) {
                Ok(password) => password,
                Err(err) => {
                    tracing::warn!("读取保存密码失败: {err}");
                    None
                }
            },
            Err(err) => {
                tracing::warn!("读取保存密码失败: {err}");
                None
            }
        }
    }

    fn key_passphrase(&mut self, key_path: &str) -> Option<String> {
        let vault_id = format!("key:{key_path}");
        match self.store.get_secret(&vault_id) {
            Ok(passphrase) => passphrase,
            Err(err) => {
                tracing::warn!("读取私钥口令失败: {err}");
                None
            }
        }
    }

    fn keyboard_interactive(
        &mut self,
        _name: &str,
        _instructions: &str,
        _prompts: &[(String, bool)],
    ) -> Option<Vec<String>> {
        None
    }
}

/// AuthProviderFactory 实现。
pub struct StoreAuthFactory {
    store: Arc<Store>,
}

impl StoreAuthFactory {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }
}

impl kt_core::session::AuthProviderFactory for StoreAuthFactory {
    fn create(&self, _id: kt_core::SessionId, params: &ConnectParams) -> Box<dyn AuthProvider> {
        Box::new(StoreAuthProvider {
            store: self.store.clone(),
            vault_id: params.effective_vault_id(),
        })
    }
}

/// 持久化 known_hosts 校验器。
pub struct KnownHostsVerifier {
    store: Arc<Store>,
}

impl KnownHostsVerifier {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }
}

impl HostKeyVerifier for KnownHostsVerifier {
    fn verify(
        &self,
        host: &str,
        port: u16,
        _key: &russh::keys::PublicKey,
        fingerprint: &str,
    ) -> HostKeyDecision {
        if self
            .store
            .consume_temporary_host_key(host, port, fingerprint)
        {
            tracing::warn!("临时允许主机密钥一次: {}:{} {}", host, port, fingerprint);
            return HostKeyDecision::Accept;
        }

        match self.store.check_host_key(host, port, fingerprint) {
            Ok(KnownHostCheck::Trusted) => HostKeyDecision::Accept,
            Ok(KnownHostCheck::Unknown { fingerprint }) => {
                let decision = KnownHostCheck::Unknown {
                    fingerprint: fingerprint.clone(),
                };
                self.store.record_pending_host_key(host, port, decision);
                tracing::warn!(
                    "未知主机密钥等待用户确认: {}:{} fingerprint={}",
                    host,
                    port,
                    fingerprint
                );
                HostKeyDecision::Reject
            }
            Ok(KnownHostCheck::NewlyTrusted) => HostKeyDecision::Accept,
            Ok(KnownHostCheck::Changed { expected, actual }) => {
                self.store.record_pending_host_key(
                    host,
                    port,
                    KnownHostCheck::Changed {
                        expected: expected.clone(),
                        actual: actual.clone(),
                    },
                );
                tracing::error!(
                    "主机密钥已变化，等待用户确认: {}:{}, stored={}, received={}",
                    host,
                    port,
                    expected,
                    actual
                );
                HostKeyDecision::Reject
            }
            Err(err) => {
                tracing::error!("known_hosts 校验失败: {}:{} {}", host, port, err);
                HostKeyDecision::Reject
            }
        }
    }
}
