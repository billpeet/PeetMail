//! Settings sync's encryption (docs/SYNC.md → Encryption). The server only ever sees what
//! comes out of here: item ids that are HMACs and blobs that are AES-256-GCM ciphertext.
//!
//! - A random 256-bit master key per sync account, made by the first device.
//! - The passphrase goes through Argon2id into a key that wraps the master key; the wrapped
//!   key lives on the server so a new device can unwrap it.
//! - Two subkeys come from the master key by HKDF: one encrypts items, one names them.
//! - The recovery key is the master key itself, written in base32 groups.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::{hkdf, hmac};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

pub const KEY_LEN: usize = 32;

/// The KDF's parameters, stored on the server with the wrapped key (opaque to it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KdfParams {
    pub alg: String,
    /// Memory in KiB.
    pub m: u32,
    /// Iterations.
    pub t: u32,
    /// Lanes.
    pub p: u32,
    /// base64url.
    pub salt: String,
}

impl KdfParams {
    /// Fresh parameters for a new passphrase: OWASP's Argon2id recommendation (64 MiB, 3
    /// passes), a random 16-byte salt.
    pub fn new() -> Self {
        Self::with_cost(64 * 1024, 3)
    }

    pub fn with_cost(m: u32, t: u32) -> Self {
        Self { alg: "argon2id".into(), m, t, p: 1, salt: URL_SAFE_NO_PAD.encode(random::<16>()) }
    }
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("OS RNG unavailable");
    b
}

/// A sync account's master key.
#[derive(Clone, PartialEq)]
pub struct MasterKey([u8; KEY_LEN]);

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterKey(..)")
    }
}

impl MasterKey {
    pub fn generate() -> Self {
        Self(random())
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        Ok(Self(b.try_into().map_err(|_| AppError::Other("a sync key has the wrong length".into()))?))
    }

    pub fn bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    fn subkey(&self, info: &'static [u8]) -> [u8; KEY_LEN] {
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, b"peetmail-sync").extract(&self.0);
        let mut out = [0u8; KEY_LEN];
        prk.expand(&[info], hkdf::HKDF_SHA256)
            .and_then(|okm| okm.fill(&mut out))
            .expect("HKDF output of one hash length");
        out
    }

    /// The server's name for a settings key: an HMAC, so it can't tell what the setting is.
    pub fn item_id(&self, settings_key: &str) -> String {
        let key = hmac::Key::new(hmac::HMAC_SHA256, &self.subkey(b"item ids v1"));
        // 128 bits is plenty to keep a user's few hundred ids apart, and shorter on the wire.
        URL_SAFE_NO_PAD.encode(&hmac::sign(&key, settings_key.as_bytes()).as_ref()[..16])
    }

    /// Encrypts an item's plaintext. The item id is the associated data, so the server can't
    /// pass one item's blob off as another's.
    pub fn seal_item(&self, item_id: &str, plaintext: &[u8]) -> String {
        seal(&self.subkey(b"items v1"), item_id.as_bytes(), plaintext)
    }

    pub fn open_item(&self, item_id: &str, blob: &str) -> Result<Vec<u8>> {
        open(&self.subkey(b"items v1"), item_id.as_bytes(), blob).ok_or_else(|| AppError::Other("a synced setting couldn't be decrypted".into()))
    }

    /// The master key, wrapped with the key derived from a passphrase.
    pub fn wrap(&self, kek: &[u8; KEY_LEN]) -> String {
        seal(kek, b"master key v1", &self.0)
    }

    /// None when the key-encryption key is wrong (a mistyped passphrase).
    pub fn unwrap(kek: &[u8; KEY_LEN], wrapped: &str) -> Option<Self> {
        open(kek, b"master key v1", wrapped).and_then(|b| Self::from_bytes(&b).ok())
    }

    /// The recovery key: base32 in dash-separated groups of four, e.g. `ABCD-EFGH-…`.
    pub fn recovery_key(&self) -> String {
        let s = base32_encode(&self.0);
        s.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap_or_default()).collect::<Vec<_>>().join("-")
    }

    /// Reads a recovery key, forgiving case, spaces and dashes. None if it isn't one.
    pub fn from_recovery_key(text: &str) -> Option<Self> {
        let clean: String = text.chars().filter(|c| !c.is_whitespace() && *c != '-').collect::<String>().to_ascii_uppercase();
        if clean.len() != RECOVERY_CHARS {
            return None;
        }
        base32_decode(&clean).and_then(|b| Self::from_bytes(&b).ok())
    }
}

/// Characters in a recovery key without its dashes (256 bits in base32).
const RECOVERY_CHARS: usize = KEY_LEN * 8 / 5 + 1;

/// The key a passphrase gives under these parameters.
pub fn derive_kek(passphrase: &str, params: &KdfParams) -> Result<[u8; KEY_LEN]> {
    if params.alg != "argon2id" {
        return Err(AppError::Other(format!("this sync account uses a key derivation PeetMail doesn't know ({})", params.alg)));
    }
    let salt = URL_SAFE_NO_PAD.decode(&params.salt).map_err(|_| AppError::Other("the sync account's salt is unreadable".into()))?;
    let p = argon2::Params::new(params.m, params.t, params.p, Some(KEY_LEN)).map_err(|e| AppError::Other(format!("unusable key derivation settings: {e}")))?;
    let mut out = [0u8; KEY_LEN];
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, p)
        .hash_password_into(passphrase.as_bytes(), &salt, &mut out)
        .map_err(|e| AppError::Other(format!("key derivation failed: {e}")))?;
    Ok(out)
}

/// base64url(nonce ‖ ciphertext ‖ tag).
fn seal(key: &[u8; KEY_LEN], aad: &[u8], plaintext: &[u8]) -> String {
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).expect("a 256-bit key"));
    let nonce = random::<NONCE_LEN>();
    let mut buf = plaintext.to_vec();
    key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(aad), &mut buf).expect("AES-GCM seal");
    let mut out = nonce.to_vec();
    out.extend_from_slice(&buf);
    URL_SAFE_NO_PAD.encode(out)
}

fn open(key: &[u8; KEY_LEN], aad: &[u8], blob: &str) -> Option<Vec<u8>> {
    let raw = URL_SAFE_NO_PAD.decode(blob).ok()?;
    if raw.len() < NONCE_LEN {
        return None;
    }
    let (nonce, ct) = raw.split_at(NONCE_LEN);
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).ok()?);
    let mut buf = ct.to_vec();
    let plain = key.open_in_place(Nonce::try_assume_unique_for_key(nonce).ok()?, Aad::from(aad), &mut buf).ok()?;
    Some(plain.to_vec())
}

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

fn base32_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let (mut buf, mut bits) = (0u32, 0);
    for &b in data {
        buf = (buf << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32[((buf >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(BASE32[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buf, mut bits) = (0u32, 0);
    for c in text.bytes() {
        let v = BASE32.iter().position(|&b| b == c)? as u32;
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cheap parameters: these tests are about the plumbing, not Argon2's cost.
    fn quick() -> KdfParams {
        KdfParams::with_cost(256, 1)
    }

    #[test]
    fn items_round_trip_and_are_bound_to_their_id() {
        let k = MasterKey::generate();
        let id = k.item_id("snippet:thanks");
        let blob = k.seal_item(&id, b"{\"body\":\"Thanks!\"}");
        assert_eq!(k.open_item(&id, &blob).unwrap(), b"{\"body\":\"Thanks!\"}");
        // The server swapping blobs between items is caught.
        assert!(k.open_item(&k.item_id("snippet:other"), &blob).is_err());
        // So is tampering, and another account's key.
        let mut bytes = URL_SAFE_NO_PAD.decode(&blob).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        assert!(k.open_item(&id, &URL_SAFE_NO_PAD.encode(bytes)).is_err());
        assert!(MasterKey::generate().open_item(&id, &blob).is_err());
        assert!(k.open_item(&id, "!!").is_err());
        // Each seal has its own nonce.
        assert_ne!(k.seal_item(&id, b"x"), k.seal_item(&id, b"x"));
    }

    #[test]
    fn item_ids_are_stable_opaque_and_per_account() {
        let k = MasterKey::generate();
        assert_eq!(k.item_id("signature:priya@fabrikam.io"), k.item_id("signature:priya@fabrikam.io"));
        assert_ne!(k.item_id("a"), k.item_id("b"));
        assert_ne!(k.item_id("a"), MasterKey::generate().item_id("a"));
        let id = k.item_id("signature:priya@fabrikam.io");
        assert!(!id.contains("priya") && id.len() == 22 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{id}");
    }

    #[test]
    fn the_passphrase_unwraps_the_key_and_a_wrong_one_doesnt() {
        let params = quick();
        let k = MasterKey::generate();
        let wrapped = k.wrap(&derive_kek("correct horse", &params).unwrap());
        assert_eq!(MasterKey::unwrap(&derive_kek("correct horse", &params).unwrap(), &wrapped), Some(k));
        assert_eq!(MasterKey::unwrap(&derive_kek("Correct horse", &params).unwrap(), &wrapped), None);
        // Same passphrase, another salt: another key.
        assert_ne!(derive_kek("correct horse", &quick()).unwrap(), derive_kek("correct horse", &params).unwrap());
        assert!(derive_kek("x", &KdfParams { alg: "scrypt".into(), ..quick() }).is_err());
    }

    #[test]
    fn recovery_keys_round_trip_forgivingly() {
        let k = MasterKey::generate();
        let r = k.recovery_key();
        assert_eq!(r.len(), RECOVERY_CHARS + RECOVERY_CHARS.div_ceil(4) - 1, "{r}");
        assert!(r.split('-').all(|g| g.len() <= 4));
        assert_eq!(MasterKey::from_recovery_key(&r), Some(k.clone()));
        assert_eq!(MasterKey::from_recovery_key(&format!("  {} ", r.to_lowercase().replace('-', " "))), Some(k));
        assert_eq!(MasterKey::from_recovery_key("correct horse battery staple"), None);
        assert_eq!(MasterKey::from_recovery_key(&r[..r.len() - 1]), None);
    }

    #[test]
    fn base32_matches_rfc_4648() {
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32_decode("MZXW6YTBOI").unwrap(), b"foobar");
    }

    #[test]
    fn the_default_cost_is_owasps() {
        let p = KdfParams::new();
        assert_eq!((p.alg.as_str(), p.m, p.t, p.p), ("argon2id", 65536, 3, 1));
        assert_eq!(URL_SAFE_NO_PAD.decode(&p.salt).unwrap().len(), 16);
    }
}
