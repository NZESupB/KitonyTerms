//! KitonyTerms secret vault.
//!
//! Stores sensitive strings (SSH passwords, private-key passphrases) encrypted
//! at rest with a user-supplied **master password**. The scheme is deliberately
//! boring and built from vetted primitives:
//!
//! * master password → 32-byte key via **Argon2id** (per-vault random salt)
//! * payload encrypted with **XChaCha20-Poly1305** (per-write random 24-byte nonce)
//! * decrypted plaintext and derived keys are wiped with [`zeroize`]
//!
//! The on-disk format is a small, self-describing binary blob (see [`VaultFile`])
//! so it is portable across macOS / Windows / Linux without depending on any OS
//! keychain service.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use argon2::Argon2;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

mod format;
use format::VaultFile;

pub use format::FORMAT_VERSION;

const KEY_LEN: usize = 32;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;

/// Errors that can arise while working with the vault.
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("vault file not found at {0}")]
    NotFound(PathBuf),

    #[error("wrong master password or corrupted vault")]
    BadPasswordOrCorrupt,

    #[error("unsupported vault format version {0} (this build supports {FORMAT_VERSION})")]
    UnsupportedVersion(u8),

    #[error("vault file is malformed: {0}")]
    Malformed(&'static str),

    #[error("key derivation failed: {0}")]
    KeyDerivation(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

type Result<T> = std::result::Result<T, VaultError>;

/// The decrypted secret store. Holds an in-memory map of `id → secret` plus the
/// derived key, so it can re-encrypt on save without re-prompting for the
/// master password. Drop wipes the key material.
pub struct Vault {
    path: PathBuf,
    salt: [u8; SALT_LEN],
    /// Derived symmetric key. Wrapped so it is zeroized on drop.
    key: Zeroizing<[u8; KEY_LEN]>,
    /// id → secret. Secrets are zeroized on drop via `Zeroizing`.
    entries: BTreeMap<String, Zeroizing<String>>,
    dirty: bool,
}

/// Serializable inner payload (what actually gets encrypted).
#[derive(Serialize, Deserialize, Default)]
struct Payload {
    entries: BTreeMap<String, String>,
}

// Manual, redacting Debug — never expose key material or secret values.
impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vault")
            .field("path", &self.path)
            .field("entries", &self.entries.len())
            .field("dirty", &self.dirty)
            .field("key", &"<redacted>")
            .finish()
    }
}

impl Vault {
    /// Argon2id with sensible interactive parameters (64 MiB, 3 passes).
    ///
    /// These mirror the OWASP "second" recommendation and keep unlock latency
    /// under ~100 ms on a modern laptop while staying expensive to brute-force.
    fn argon2<'a>() -> Argon2<'a> {
        let params = argon2::Params::new(
            64 * 1024, // m_cost: 64 MiB
            3,         // t_cost: iterations
            1,         // p_cost: lanes
            Some(KEY_LEN),
        )
        .expect("static Argon2 params are valid");
        Argon2::new(
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            params,
        )
    }

    fn derive_key(password: &str, salt: &[u8; SALT_LEN]) -> Result<Zeroizing<[u8; KEY_LEN]>> {
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        Self::argon2()
            .hash_password_into(password.as_bytes(), salt, key.as_mut())
            .map_err(|e| VaultError::KeyDerivation(e.to_string()))?;
        Ok(key)
    }

    /// Create a brand-new, empty vault protected by `master_password`.
    ///
    /// Does not touch disk until [`Vault::save`] is called.
    pub fn create(path: impl Into<PathBuf>, master_password: &str) -> Result<Self> {
        let mut salt = [0u8; SALT_LEN];
        OsRng.fill_bytes(&mut salt);
        let key = Self::derive_key(master_password, &salt)?;
        Ok(Self {
            path: path.into(),
            salt,
            key,
            entries: BTreeMap::new(),
            dirty: true,
        })
    }

    /// Open an existing vault from disk and decrypt it with `master_password`.
    pub fn open(path: impl AsRef<Path>, master_password: &str) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(VaultError::NotFound(path.to_path_buf()));
        }
        let bytes = std::fs::read(path)?;
        let file = VaultFile::decode(&bytes)?;

        let key = Self::derive_key(master_password, &file.salt)?;

        let cipher = XChaCha20Poly1305::new(key.as_ref().into());
        let nonce = XNonce::from_slice(&file.nonce);
        let mut plaintext = cipher
            .decrypt(nonce, file.ciphertext.as_ref())
            .map_err(|_| VaultError::BadPasswordOrCorrupt)?;

        let payload: Payload = serde_json::from_slice(&plaintext)
            .map_err(|_| VaultError::Malformed("payload not valid JSON"))?;
        plaintext.zeroize();

        let entries = payload
            .entries
            .into_iter()
            .map(|(k, v)| (k, Zeroizing::new(v)))
            .collect();

        Ok(Self {
            path: path.to_path_buf(),
            salt: file.salt,
            key,
            entries,
            dirty: false,
        })
    }

    /// Open the vault if it exists, otherwise create a fresh one. Useful for
    /// first-run flows where the same master password sets up the vault.
    pub fn open_or_create(path: impl AsRef<Path>, master_password: &str) -> Result<Self> {
        let path = path.as_ref();
        if path.exists() {
            Self::open(path, master_password)
        } else {
            Self::create(path.to_path_buf(), master_password)
        }
    }

    /// Path this vault is bound to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether there are unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Look up a secret by id.
    pub fn get(&self, id: &str) -> Option<&str> {
        self.entries.get(id).map(|s| s.as_str())
    }

    /// Insert or replace a secret. Marks the vault dirty.
    pub fn set(&mut self, id: impl Into<String>, secret: impl Into<String>) {
        self.entries.insert(id.into(), Zeroizing::new(secret.into()));
        self.dirty = true;
    }

    /// Remove a secret. Returns whether it existed. Marks the vault dirty.
    pub fn remove(&mut self, id: &str) -> bool {
        let existed = self.entries.remove(id).is_some();
        self.dirty |= existed;
        existed
    }

    /// All secret ids currently stored (sorted).
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(|s| s.as_str())
    }

    /// Encrypt and write the vault to disk atomically (write temp + rename).
    pub fn save(&mut self) -> Result<()> {
        let payload = Payload {
            entries: self
                .entries
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().to_owned()))
                .collect(),
        };
        let mut plaintext = serde_json::to_vec(&payload)
            .map_err(|_| VaultError::Malformed("failed to serialize payload"))?;

        let mut nonce_bytes = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);
        let cipher = XChaCha20Poly1305::new(self.key.as_ref().into());
        let nonce = XNonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_ref())
            .map_err(|_| VaultError::Malformed("encryption failed"))?;
        plaintext.zeroize();

        let file = VaultFile {
            salt: self.salt,
            nonce: nonce_bytes,
            ciphertext,
        };
        let encoded = file.encode();

        // Atomic write: temp file in the same dir, then rename over the target.
        let dir = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join(format!(
            ".{}.tmp",
            self.path
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| "vault".into())
        ));
        std::fs::write(&tmp, &encoded)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &self.path)?;
        self.dirty = false;
        Ok(())
    }

    /// Change the master password by re-deriving the key (keeps a fresh salt).
    /// Caller should [`Vault::save`] afterwards to persist.
    pub fn change_master_password(&mut self, new_password: &str) -> Result<()> {
        let mut salt = [0u8; SALT_LEN];
        OsRng.fill_bytes(&mut salt);
        self.key = Self::derive_key(new_password, &salt)?;
        self.salt = salt;
        self.dirty = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_vault_path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.vault");
        (dir, path)
    }

    #[test]
    fn create_set_save_open_roundtrip() {
        let (_dir, path) = tmp_vault_path();
        {
            let mut v = Vault::create(&path, "correct horse battery staple").unwrap();
            v.set("host:example.com:alice", "s3cr3t-pw");
            v.set("key:/home/alice/.ssh/id_ed25519", "passphrase-123");
            v.save().unwrap();
            assert!(!v.is_dirty());
        }
        let v = Vault::open(&path, "correct horse battery staple").unwrap();
        assert_eq!(v.get("host:example.com:alice"), Some("s3cr3t-pw"));
        assert_eq!(v.get("key:/home/alice/.ssh/id_ed25519"), Some("passphrase-123"));
        assert_eq!(v.get("missing"), None);
    }

    #[test]
    fn wrong_password_fails() {
        let (_dir, path) = tmp_vault_path();
        let mut v = Vault::create(&path, "right-password").unwrap();
        v.set("a", "b");
        v.save().unwrap();

        let err = Vault::open(&path, "wrong-password").unwrap_err();
        assert!(matches!(err, VaultError::BadPasswordOrCorrupt));
    }

    #[test]
    fn corrupted_file_fails_cleanly() {
        let (_dir, path) = tmp_vault_path();
        let mut v = Vault::create(&path, "pw").unwrap();
        v.set("a", "b");
        v.save().unwrap();

        // Flip a byte in the ciphertext region (well past the header).
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&path, &bytes).unwrap();

        let err = Vault::open(&path, "pw").unwrap_err();
        assert!(matches!(err, VaultError::BadPasswordOrCorrupt));
    }

    #[test]
    fn remove_and_ids() {
        let (_dir, path) = tmp_vault_path();
        let mut v = Vault::create(&path, "pw").unwrap();
        v.set("z", "1");
        v.set("a", "2");
        v.set("m", "3");
        assert_eq!(v.ids().collect::<Vec<_>>(), vec!["a", "m", "z"]); // sorted
        assert!(v.remove("m"));
        assert!(!v.remove("m"));
        assert_eq!(v.ids().collect::<Vec<_>>(), vec!["a", "z"]);
    }

    #[test]
    fn change_master_password_then_reopen() {
        let (_dir, path) = tmp_vault_path();
        {
            let mut v = Vault::create(&path, "old-pw").unwrap();
            v.set("k", "v");
            v.save().unwrap();
            v.change_master_password("new-pw").unwrap();
            v.save().unwrap();
        }
        assert!(matches!(
            Vault::open(&path, "old-pw").unwrap_err(),
            VaultError::BadPasswordOrCorrupt
        ));
        let v = Vault::open(&path, "new-pw").unwrap();
        assert_eq!(v.get("k"), Some("v"));
    }

    #[test]
    fn open_missing_is_notfound() {
        let (_dir, path) = tmp_vault_path();
        assert!(matches!(
            Vault::open(&path, "pw").unwrap_err(),
            VaultError::NotFound(_)
        ));
    }
}
