//! russh client [`Handler`] implementation.
//!
//! Its main job for the MVP is **host-key verification** using a
//! trust-on-first-use (TOFU) policy backed by a caller-supplied
//! [`HostKeyVerifier`]. Channel data is *not* consumed here — we drive the
//! channel directly via [`russh::Channel`] in the session loop, which gives us
//! the byte stream without routing it through handler callbacks.

use russh::client::{Handler, Session};
use russh::keys::PublicKey;

/// Decision returned by a [`HostKeyVerifier`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyDecision {
    /// Key is trusted — proceed.
    Accept,
    /// Key is unknown/changed — reject the connection.
    Reject,
}

/// Pluggable host-key trust policy.
///
/// Implementations decide whether to trust a server's public key. The headless
/// example prompts on the terminal; the GUI shows a dialog; tests can hard-code
/// a decision.
pub trait HostKeyVerifier: Send + Sync {
    /// Called once during the handshake with the server's public key and the
    /// `host:port` we connected to. `fingerprint` is the SHA256 fingerprint
    /// string (e.g. `SHA256:abc...`) for display.
    fn verify(&self, host: &str, port: u16, key: &PublicKey, fingerprint: &str)
        -> HostKeyDecision;
}

/// A verifier that accepts everything. **Insecure** — only for tests / opt-in.
pub struct AcceptAllVerifier;

impl HostKeyVerifier for AcceptAllVerifier {
    fn verify(&self, _: &str, _: u16, _: &PublicKey, _: &str) -> HostKeyDecision {
        HostKeyDecision::Accept
    }
}

/// The russh handler. Holds the host identity and the verifier.
pub struct ClientHandler {
    pub host: String,
    pub port: u16,
    pub verifier: std::sync::Arc<dyn HostKeyVerifier>,
}

impl Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        let fingerprint = server_public_key
            .fingerprint(Default::default())
            .to_string();
        let decision = self
            .verifier
            .verify(&self.host, self.port, server_public_key, &fingerprint);
        Ok(matches!(decision, HostKeyDecision::Accept))
    }

    async fn data(
        &mut self,
        _channel: russh::ChannelId,
        _data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Intentionally empty: we consume channel data via `Channel::wait()` in
        // the session loop, not through this callback.
        Ok(())
    }
}
