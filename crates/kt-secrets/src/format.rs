//! On-disk binary format for the vault.
//!
//! Layout (all integers little-endian):
//!
//! ```text
//! magic:      4 bytes  "KTVT"
//! version:    1 byte   FORMAT_VERSION
//! salt:      16 bytes  Argon2 salt
//! nonce:     24 bytes  XChaCha20-Poly1305 nonce
//! ct_len:     4 bytes  u32 ciphertext length
//! ciphertext: ct_len bytes (includes the Poly1305 tag)
//! ```

use super::{VaultError, NONCE_LEN, SALT_LEN};

/// Magic bytes identifying a KitonyTerms vault file ("KiTony VaulT").
const MAGIC: &[u8; 4] = b"KTVT";

/// Current on-disk format version.
pub const FORMAT_VERSION: u8 = 1;

const HEADER_LEN: usize = 4 + 1 + SALT_LEN + NONCE_LEN + 4;

/// Parsed view of a vault file's framing (still encrypted).
pub(crate) struct VaultFile {
    pub salt: [u8; SALT_LEN],
    pub nonce: [u8; NONCE_LEN],
    pub ciphertext: Vec<u8>,
}

impl VaultFile {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.ciphertext.len());
        out.extend_from_slice(MAGIC);
        out.push(FORMAT_VERSION);
        out.extend_from_slice(&self.salt);
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&(self.ciphertext.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.ciphertext);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, VaultError> {
        if bytes.len() < HEADER_LEN {
            return Err(VaultError::Malformed("file shorter than header"));
        }
        if &bytes[0..4] != MAGIC {
            return Err(VaultError::Malformed("bad magic"));
        }
        let version = bytes[4];
        if version != FORMAT_VERSION {
            return Err(VaultError::UnsupportedVersion(version));
        }

        let mut off = 5;
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&bytes[off..off + SALT_LEN]);
        off += SALT_LEN;

        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&bytes[off..off + NONCE_LEN]);
        off += NONCE_LEN;

        let ct_len = u32::from_le_bytes(
            bytes[off..off + 4]
                .try_into()
                .map_err(|_| VaultError::Malformed("bad ct_len"))?,
        ) as usize;
        off += 4;

        if bytes.len() != off + ct_len {
            return Err(VaultError::Malformed("ciphertext length mismatch"));
        }
        let ciphertext = bytes[off..].to_vec();

        Ok(Self {
            salt,
            nonce,
            ciphertext,
        })
    }
}
