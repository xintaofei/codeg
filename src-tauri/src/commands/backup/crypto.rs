//! Optional passphrase encryption for backup archives.
//!
//! When a passphrase is supplied, the plaintext ZIP payload is wrapped in a
//! `.codegbak` envelope: an unencrypted header (magic + KDF params + salt +
//! nonce prefix) followed by the ZIP encrypted with AES-256-GCM in a chunked
//! STREAM construction. The header is plaintext because the salt/nonce must be
//! readable before the key can be derived; the GCM tag on the first chunk is
//! what authenticates the passphrase (a wrong passphrase fails to decrypt).
//!
//! Streaming (64 KiB chunks) keeps memory bounded — backups can be large.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::aead::stream::{DecryptorBE32, EncryptorBE32};
use aes_gcm::{Aes256Gcm, KeyInit};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::app_error::{AppCommandError, BACKUP_I18N_KEY_BAD_PASSPHRASE};

use super::cancelled_error;

/// First 8 bytes of an encrypted backup.
pub const ENVELOPE_MAGIC: &[u8; 8] = b"CODEGBAK";
/// First 4 bytes of a plaintext ZIP (`PK\x03\x04`), used to disambiguate.
pub const ZIP_MAGIC: &[u8; 4] = b"PK\x03\x04";
pub const ENVELOPE_HEADER_VERSION: u8 = 1;

/// Plaintext chunk size fed to the STREAM cipher. Ciphertext chunks are this
/// plus [`GCM_TAG_LEN`].
pub const DEFAULT_CHUNK_SIZE: usize = 64 * 1024;
const GCM_TAG_LEN: usize = 16;
/// STREAM-BE32 reserves 5 of the 12 GCM nonce bytes (4 counter + 1 last-block
/// flag), leaving a 7-byte random prefix.
const NONCE_PREFIX_LEN: usize = 7;
const SALT_LEN: usize = 16;

// Bounds enforced on a decrypted envelope's attacker-controlled header.
const MIN_SALT_LEN: usize = 8;
const MAX_SALT_LEN: usize = 64;
const MIN_CHUNK_SIZE: usize = 4 * 1024;
const MAX_CHUNK_SIZE: usize = 1024 * 1024;
// Bound an attacker-controlled encrypted header to the product-supported KDF
// envelope (we only ever emit m=64 MiB / t=3 / p=1). A hostile file therefore
// can't drive >256 MiB / 10-pass Argon2 work during inspect/restore before the
// GCM tag fails. Widen these only alongside a format_version bump.
const MAX_M_COST: u32 = 256 * 1024; // 256 MiB (Argon2 m_cost is in KiB)
const MAX_T_COST: u32 = 10;
const MAX_P_COST: u32 = 4;

// Argon2id defaults. 64 MiB / 3 passes / 1 lane is a reasonable interactive
// cost that still meaningfully slows brute force on a leaked archive.
const DEFAULT_M_COST: u32 = 64 * 1024;
const DEFAULT_T_COST: u32 = 3;
const DEFAULT_P_COST: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KdfParams {
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
    /// Argon2 version constant: `0x13` (19) for the modern V0x13.
    pub version: u32,
}

impl Default for KdfParams {
    fn default() -> Self {
        Self {
            m_cost: DEFAULT_M_COST,
            t_cost: DEFAULT_T_COST,
            p_cost: DEFAULT_P_COST,
            version: 0x13,
        }
    }
}

/// Cleartext header at the front of a `.codegbak` file. Carries everything
/// needed to re-derive the key and decrypt — it is the single source of truth
/// for crypto parameters (the in-archive manifest stays crypto-agnostic).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvelopeHeader {
    pub algo: String,
    pub kdf: String,
    pub kdf_params: KdfParams,
    pub salt_b64: String,
    pub nonce_prefix_b64: String,
    pub chunk_size: usize,
}

/// Cheap probe of the first bytes of `path` to tell an encrypted envelope from
/// a plaintext ZIP.
pub fn is_encrypted(path: &Path) -> Result<bool, AppCommandError> {
    let mut f = File::open(path).map_err(AppCommandError::io)?;
    let mut head = [0u8; 8];
    let n = read_fill(&mut f, &mut head).map_err(AppCommandError::io)?;
    Ok(n >= ENVELOPE_MAGIC.len() && &head[..ENVELOPE_MAGIC.len()] == ENVELOPE_MAGIC)
}

fn derive_key(passphrase: &str, salt: &[u8], params: &KdfParams) -> Result<[u8; 32], AppCommandError> {
    let p = Params::new(params.m_cost, params.t_cost, params.p_cost, Some(32))
        .map_err(|e| AppCommandError::task_execution_failed("Invalid KDF parameters").with_detail(e.to_string()))?;
    let version = if params.version == 0x10 {
        Version::V0x10
    } else {
        Version::V0x13
    };
    let argon2 = Argon2::new(Algorithm::Argon2id, version, p);
    let mut key = [0u8; 32];
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| AppCommandError::task_execution_failed("Key derivation failed").with_detail(e.to_string()))?;
    Ok(key)
}

/// Encrypt the plaintext ZIP at `src` into a `.codegbak` envelope at `dest`.
/// Synchronous — run under `spawn_blocking`.
pub fn encrypt_file(
    src: &Path,
    dest: &Path,
    passphrase: &str,
    cancel: &CancellationToken,
) -> Result<(), AppCommandError> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce_prefix = [0u8; NONCE_PREFIX_LEN];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    rand::rngs::OsRng.fill_bytes(&mut nonce_prefix);

    let kdf_params = KdfParams::default();
    let key = derive_key(passphrase, &salt, &kdf_params)?;

    let header = EnvelopeHeader {
        algo: "AES-256-GCM".to_string(),
        kdf: "Argon2id".to_string(),
        kdf_params,
        salt_b64: B64.encode(salt),
        nonce_prefix_b64: B64.encode(nonce_prefix),
        chunk_size: DEFAULT_CHUNK_SIZE,
    };

    let reader = File::open(src).map_err(AppCommandError::io)?;
    let mut reader = BufReader::new(reader);
    let out = File::create(dest).map_err(AppCommandError::io)?;
    let mut out = BufWriter::new(out);

    // Write the cleartext header.
    out.write_all(ENVELOPE_MAGIC).map_err(AppCommandError::io)?;
    out.write_all(&[ENVELOPE_HEADER_VERSION]).map_err(AppCommandError::io)?;
    let header_json = serde_json::to_vec(&header)
        .map_err(|e| AppCommandError::task_execution_failed("Serialize envelope header").with_detail(e.to_string()))?;
    out.write_all(&(header_json.len() as u32).to_le_bytes()).map_err(AppCommandError::io)?;
    out.write_all(&header_json).map_err(AppCommandError::io)?;

    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| AppCommandError::task_execution_failed("Cipher init failed").with_detail(e.to_string()))?;
    let nonce = GenericArray::from_slice(&nonce_prefix);
    let mut enc = EncryptorBE32::from_aead(cipher, nonce);

    let chunk = DEFAULT_CHUNK_SIZE;
    let mut cur = read_block(&mut reader, chunk).map_err(AppCommandError::io)?;
    loop {
        if cancel.is_cancelled() {
            return Err(cancelled_error());
        }
        let nxt = read_block(&mut reader, chunk).map_err(AppCommandError::io)?;
        if nxt.is_empty() {
            let ct = enc
                .encrypt_last(cur.as_slice())
                .map_err(|_| AppCommandError::task_execution_failed("Encryption failed"))?;
            out.write_all(&ct).map_err(AppCommandError::io)?;
            break;
        }
        let ct = enc
            .encrypt_next(cur.as_slice())
            .map_err(|_| AppCommandError::task_execution_failed("Encryption failed"))?;
        out.write_all(&ct).map_err(AppCommandError::io)?;
        cur = nxt;
    }
    out.flush().map_err(AppCommandError::io)?;
    Ok(())
}

/// Decrypt a `.codegbak` envelope at `src` into a plaintext ZIP at `dest`.
/// A wrong passphrase (or tampering) surfaces as an authentication error.
pub fn decrypt_file(
    src: &Path,
    dest: &Path,
    passphrase: &str,
    cancel: &CancellationToken,
) -> Result<(), AppCommandError> {
    let reader = File::open(src).map_err(AppCommandError::io)?;
    let mut reader = BufReader::new(reader);

    let header = read_header(&mut reader)?;
    // The envelope header is attacker-controlled; validate every field that
    // drives an allocation (chunk_size) or CPU/memory work (Argon2 params)
    // BEFORE deriving the key or allocating buffers, to deny a malformed file a
    // memory/CPU DoS during inspect/restore.
    validate_header(&header)?;
    let salt = B64
        .decode(header.salt_b64.as_bytes())
        .map_err(|_| corrupt_header_error())?;
    let nonce_prefix = B64
        .decode(header.nonce_prefix_b64.as_bytes())
        .map_err(|_| corrupt_header_error())?;
    if nonce_prefix.len() != NONCE_PREFIX_LEN {
        return Err(corrupt_header_error());
    }
    if !(MIN_SALT_LEN..=MAX_SALT_LEN).contains(&salt.len()) {
        return Err(corrupt_header_error());
    }

    let key = derive_key(passphrase, &salt, &header.kdf_params)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| AppCommandError::task_execution_failed("Cipher init failed").with_detail(e.to_string()))?;
    let nonce = GenericArray::from_slice(&nonce_prefix);
    let mut dec = DecryptorBE32::from_aead(cipher, nonce);

    let out = File::create(dest).map_err(AppCommandError::io)?;
    let mut out = BufWriter::new(out);

    let block = header.chunk_size + GCM_TAG_LEN;
    let mut cur = read_block(&mut reader, block).map_err(AppCommandError::io)?;
    loop {
        if cancel.is_cancelled() {
            return Err(cancelled_error());
        }
        let nxt = read_block(&mut reader, block).map_err(AppCommandError::io)?;
        if nxt.is_empty() {
            let pt = dec
                .decrypt_last(cur.as_slice())
                .map_err(|_| bad_passphrase_error())?;
            out.write_all(&pt).map_err(AppCommandError::io)?;
            break;
        }
        let pt = dec
            .decrypt_next(cur.as_slice())
            .map_err(|_| bad_passphrase_error())?;
        out.write_all(&pt).map_err(AppCommandError::io)?;
        cur = nxt;
    }
    out.flush().map_err(AppCommandError::io)?;
    Ok(())
}

/// Reject an envelope header whose fields are out of the bounds we ever
/// produce, before any of them drives an allocation or KDF work.
fn validate_header(h: &EnvelopeHeader) -> Result<(), AppCommandError> {
    if h.algo != "AES-256-GCM" || h.kdf != "Argon2id" {
        return Err(corrupt_header_error());
    }
    if !(MIN_CHUNK_SIZE..=MAX_CHUNK_SIZE).contains(&h.chunk_size) {
        return Err(corrupt_header_error());
    }
    let p = &h.kdf_params;
    if !(8..=MAX_M_COST).contains(&p.m_cost)
        || !(1..=MAX_T_COST).contains(&p.t_cost)
        || !(1..=MAX_P_COST).contains(&p.p_cost)
    {
        return Err(corrupt_header_error());
    }
    Ok(())
}

fn read_header<R: Read>(reader: &mut R) -> Result<EnvelopeHeader, AppCommandError> {
    let mut magic = [0u8; 8];
    read_fill(reader, &mut magic).map_err(AppCommandError::io)?;
    if &magic != ENVELOPE_MAGIC {
        return Err(corrupt_header_error());
    }
    let mut ver = [0u8; 1];
    read_fill(reader, &mut ver).map_err(AppCommandError::io)?;
    if ver[0] != ENVELOPE_HEADER_VERSION {
        return Err(corrupt_header_error());
    }
    let mut len_buf = [0u8; 4];
    read_fill(reader, &mut len_buf).map_err(AppCommandError::io)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    // Guard against an absurd declared header length (corruption / hostile file).
    if len > 1024 * 1024 {
        return Err(corrupt_header_error());
    }
    let mut json = vec![0u8; len];
    if read_fill(reader, &mut json).map_err(AppCommandError::io)? != len {
        return Err(corrupt_header_error());
    }
    serde_json::from_slice(&json).map_err(|_| corrupt_header_error())
}

/// Read exactly `buf.len()` bytes, or fewer at EOF. Returns the count read.
fn read_fill<R: Read>(reader: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = reader.read(&mut buf[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}

/// Read up to `size` bytes into a fresh Vec (shorter only at EOF; empty == EOF).
fn read_block<R: Read>(reader: &mut R, size: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; size];
    let n = read_fill(reader, &mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

fn bad_passphrase_error() -> AppCommandError {
    AppCommandError::authentication_failed("Incorrect passphrase or corrupted backup")
        .with_i18n(BACKUP_I18N_KEY_BAD_PASSPHRASE, Default::default())
}

fn corrupt_header_error() -> AppCommandError {
    AppCommandError::invalid_input("Malformed backup envelope header")
        .with_i18n(BACKUP_I18N_KEY_BAD_PASSPHRASE, Default::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = dir.join(name);
        let mut f = File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    fn roundtrip(plain: &[u8]) {
        let dir = tempfile::tempdir().unwrap();
        let src = write_tmp(dir.path(), "plain.zip", plain);
        let enc = dir.path().join("out.codegbak");
        let dec = dir.path().join("back.zip");
        let cancel = CancellationToken::new();

        encrypt_file(&src, &enc, "hunter2", &cancel).unwrap();
        assert!(is_encrypted(&enc).unwrap());
        assert!(!is_encrypted(&src).unwrap());

        decrypt_file(&enc, &dec, "hunter2", &cancel).unwrap();
        let got = std::fs::read(&dec).unwrap();
        assert_eq!(got, plain);
    }

    #[test]
    fn roundtrip_various_sizes() {
        roundtrip(b"");
        roundtrip(b"hello");
        roundtrip(&vec![7u8; DEFAULT_CHUNK_SIZE]); // exactly one chunk
        roundtrip(&vec![9u8; DEFAULT_CHUNK_SIZE + 1]); // one chunk + 1
        roundtrip(&vec![3u8; DEFAULT_CHUNK_SIZE * 2 + 123]); // multi-chunk + partial
    }

    #[test]
    fn validate_header_rejects_out_of_bounds_fields() {
        let base = EnvelopeHeader {
            algo: "AES-256-GCM".to_string(),
            kdf: "Argon2id".to_string(),
            kdf_params: KdfParams::default(),
            salt_b64: B64.encode([0u8; SALT_LEN]),
            nonce_prefix_b64: B64.encode([0u8; NONCE_PREFIX_LEN]),
            chunk_size: DEFAULT_CHUNK_SIZE,
        };
        assert!(validate_header(&base).is_ok());

        let mut huge_chunk = base.clone();
        huge_chunk.chunk_size = 1 << 30; // 1 GiB buffer → reject
        assert!(validate_header(&huge_chunk).is_err());

        let mut huge_mem = base.clone();
        huge_mem.kdf_params.m_cost = 1 << 30; // absurd Argon2 memory → reject
        assert!(validate_header(&huge_mem).is_err());

        let mut over_envelope = base.clone();
        over_envelope.kdf_params.m_cost = 512 * 1024; // 512 MiB > product cap
        assert!(validate_header(&over_envelope).is_err());

        let mut high_t = base.clone();
        high_t.kdf_params.t_cost = 100;
        assert!(validate_header(&high_t).is_err());

        let mut bad_algo = base.clone();
        bad_algo.algo = "rot13".to_string();
        assert!(validate_header(&bad_algo).is_err());
    }

    #[test]
    fn wrong_passphrase_fails_authentication() {
        let dir = tempfile::tempdir().unwrap();
        let src = write_tmp(dir.path(), "plain.zip", b"secret payload");
        let enc = dir.path().join("out.codegbak");
        let dec = dir.path().join("back.zip");
        let cancel = CancellationToken::new();

        encrypt_file(&src, &enc, "correct horse", &cancel).unwrap();
        let err = decrypt_file(&enc, &dec, "battery staple", &cancel).unwrap_err();
        assert_eq!(err.i18n_key.as_deref(), Some(BACKUP_I18N_KEY_BAD_PASSPHRASE));
    }
}
