//! Encrypts secrets at rest (saved sign-ins), scoped to the current user.
//!
//! - **Windows:** DPAPI. Credential Manager caps blobs at 2.5KB, which Microsoft refresh
//!   tokens can exceed, so tokens are DPAPI-encrypted and stored in SQLite instead.
//! - **Linux:** ChaCha20-Poly1305 under a random 256-bit key. The key lives in the desktop's
//!   keyring through the Secret Service API (GNOME Keyring, KWallet, KeePassXC). A desktop
//!   with no keyring gets the key in a file only the user can read, like Chromium's "basic"
//!   store. Tokens still sit in SQLite, so the database layer is the same on both.

use crate::error::{AppError, Result};

#[cfg(windows)]
pub fn protect(data: &[u8]) -> Result<Vec<u8>> {
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    unsafe {
        let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: ptr::null_mut() };
        let ok = CryptProtectData(
            &input,
            ptr::null(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        );
        if ok == 0 {
            return Err(AppError::Other(format!("DPAPI protect failed: {}", std::io::Error::last_os_error())));
        }
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as _);
        Ok(v)
    }
}

#[cfg(windows)]
pub fn unprotect(data: &[u8]) -> Result<Vec<u8>> {
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    unsafe {
        let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: ptr::null_mut() };
        let ok = CryptUnprotectData(
            &input,
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        );
        if ok == 0 {
            return Err(AppError::Other(format!("DPAPI unprotect failed: {}", std::io::Error::last_os_error())));
        }
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as _);
        Ok(v)
    }
}

/// Where the Linux key may be kept. Called once from setup, before anything is encrypted.
/// `keyring` is off for test runs, so they never touch the user's real keyring.
#[cfg(windows)]
pub fn init(_data_dir: &std::path::Path, _keyring: bool) {}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub use linux::init;

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn protect(data: &[u8]) -> Result<Vec<u8>> {
    // Signing in: the user is there to answer an unlock prompt.
    sealed::seal(&linux::key(true)?, data)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn unprotect(data: &[u8]) -> Result<Vec<u8>> {
    sealed::open(&linux::key(false)?, data)
}

/// For unit tests that encrypt: on Linux the key goes in a key file in a temp dir (never the
/// keyring). The first call sets it up for the whole test run.
#[cfg(test)]
pub fn init_for_tests() {
    let dir = std::env::temp_dir().join(format!("peetmail-secure-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    init(&dir, false);
}

#[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
pub fn init(_data_dir: &std::path::Path, _keyring: bool) {}

#[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
pub fn protect(_: &[u8]) -> Result<Vec<u8>> {
    Err(AppError::Other("secure storage isn't implemented on this platform".into()))
}

#[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
pub fn unprotect(_: &[u8]) -> Result<Vec<u8>> {
    Err(AppError::Other("secure storage isn't implemented on this platform".into()))
}

/// Authenticated encryption under a 256-bit key: `version (1) | nonce (12) | ciphertext+tag`.
#[cfg(any(target_os = "linux", target_os = "android"))]
mod sealed {
    use super::{AppError, Result};
    use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305, NONCE_LEN};

    const VERSION: u8 = 1;

    fn cipher(key: &[u8; 32]) -> LessSafeKey {
        LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, key).expect("a 32-byte key fits ChaCha20-Poly1305"))
    }

    pub fn seal(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|e| AppError::Other(format!("OS RNG unavailable: {e}")))?;
        let mut body = data.to_vec();
        cipher(key)
            .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from([VERSION]), &mut body)
            .map_err(|_| AppError::Other("encrypting failed".into()))?;
        let mut out = Vec::with_capacity(1 + NONCE_LEN + body.len());
        out.push(VERSION);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&body);
        Ok(out)
    }

    pub fn open(key: &[u8; 32], blob: &[u8]) -> Result<Vec<u8>> {
        let bad = || AppError::Other("saved sign-in can't be decrypted (the key changed); sign in again".into());
        let (&version, rest) = blob.split_first().ok_or_else(bad)?;
        if version != VERSION || rest.len() < NONCE_LEN {
            return Err(bad());
        }
        let (nonce, body) = rest.split_at(NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| bad())?;
        let mut body = body.to_vec();
        let plain = cipher(key).open_in_place(nonce, Aad::from([VERSION]), &mut body).map_err(|_| bad())?;
        Ok(plain.to_vec())
    }
}

/// Also Android's, which has no keyring: the key lives in a private file in the app's own
/// folder, which Android's app sandbox keeps from every other app (and backups are off,
/// AndroidManifest.xml). TODO(android stage 4): wrap it with an Android Keystore key.
#[cfg(any(target_os = "linux", target_os = "android"))]
mod linux {
    use super::{AppError, Result};
    #[cfg(target_os = "linux")]
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    /// The key itself, when the desktop has no keyring.
    pub const KEY_FILE: &str = "token.key";
    /// Says the key is in the keyring. Once it's there, a keyring that won't answer is an
    /// error, not a reason to make a second key that the saved sign-ins don't match.
    pub const MARKER_FILE: &str = "token.key.keyring";
    /// After the keyring refuses (a dismissed unlock prompt), background work doesn't ask
    /// again for this long, so a sync every minute doesn't mean a prompt every minute.
    const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

    static CONFIG: OnceLock<(PathBuf, bool)> = OnceLock::new();
    static KEY: Mutex<Option<[u8; 32]>> = Mutex::new(None);
    static REFUSED: Mutex<Option<(Instant, String)>> = Mutex::new(None);

    pub fn init(data_dir: &Path, keyring: bool) {
        let _ = CONFIG.set((data_dir.to_path_buf(), keyring));
    }

    /// The key, looked up once and then cached. Looking it up can mean the keyring's unlock
    /// prompt. `asked` is true when the user is doing something that needs it (signing in):
    /// that asks again even straight after a refusal; background reads wait `RETRY_AFTER`.
    pub fn key(asked: bool) -> Result<[u8; 32]> {
        let mut cached = KEY.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(k) = *cached {
            return Ok(k);
        }
        if !asked
            && let Some((at, why)) = REFUSED.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
            && at.elapsed() < RETRY_AFTER
        {
            return Err(AppError::Other(why.clone()));
        }
        let (dir, keyring) = CONFIG.get().ok_or_else(|| AppError::Other("secure storage used before setup".into()))?;
        #[cfg(target_os = "linux")]
        let found = if *keyring { load_or_create(dir, &SecretService) } else { load_or_create(dir, &NoKeyring) };
        #[cfg(target_os = "android")]
        let found = {
            let _ = keyring;
            load_or_create(dir, &NoKeyring)
        };
        match found {
            Ok(k) => {
                *cached = Some(k);
                *REFUSED.lock().unwrap_or_else(|e| e.into_inner()) = None;
                Ok(k)
            }
            Err(e) => {
                *REFUSED.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), e.to_string()));
                Err(e)
            }
        }
    }

    /// Why the keyring couldn't help.
    #[derive(Debug)]
    pub enum KeyringError {
        /// There's no keyring: no Secret Service on the session bus, or no collection to use.
        Absent(String),
        /// There is one, but it said no: locked and the prompt dismissed, or a failed call.
        #[cfg_attr(target_os = "android", allow(dead_code))] // Android has no keyring to refuse
        Refused(String),
    }

    /// Somewhere to keep the key that isn't a plain file.
    pub trait Keyring {
        fn get(&self) -> std::result::Result<Option<Vec<u8>>, KeyringError>;
        fn set(&self, key: &[u8]) -> std::result::Result<(), KeyringError>;
    }

    pub struct NoKeyring;
    impl Keyring for NoKeyring {
        fn get(&self) -> std::result::Result<Option<Vec<u8>>, KeyringError> {
            Err(KeyringError::Absent("keyring not used".into()))
        }
        fn set(&self, _: &[u8]) -> std::result::Result<(), KeyringError> {
            Err(KeyringError::Absent("keyring not used".into()))
        }
    }

    fn new_key() -> Result<[u8; 32]> {
        let mut k = [0u8; 32];
        getrandom::fill(&mut k).map_err(|e| AppError::Other(format!("OS RNG unavailable: {e}")))?;
        Ok(k)
    }

    fn as_key(bytes: &[u8]) -> Option<[u8; 32]> {
        bytes.try_into().ok()
    }

    fn refused(why: &str) -> AppError {
        AppError::Other(format!("The desktop keyring that keeps PeetMail's sign-ins is locked or not answering ({why}). Unlock it, then sync again."))
    }

    /// A key file wins (the desktop had no keyring when PeetMail first needed one). Else the
    /// keyring's key, or a new one put there. Only a desktop with no keyring at all, before
    /// the keyring was ever used, gets a key file; a keyring that says no is an error.
    pub fn load_or_create(dir: &Path, keyring: &dyn Keyring) -> Result<[u8; 32]> {
        let file = dir.join(KEY_FILE);
        if file.exists() {
            let bytes = std::fs::read(&file)?;
            if let Some(k) = as_key(&bytes) {
                return Ok(k);
            }
            // The sign-ins it encrypted are lost either way; a new key lets the user sign in again.
            log::warn!("the key file for saved sign-ins is damaged; setting it aside and making a new key");
            std::fs::rename(&file, dir.join(format!("{KEY_FILE}.damaged")))?;
        }
        let marker = dir.join(MARKER_FILE);
        let stored = match keyring.get() {
            Ok(Some(bytes)) if as_key(&bytes).is_some() => {
                if !marker.exists() {
                    std::fs::write(&marker, b"")?;
                }
                return Ok(as_key(&bytes).expect("checked above"));
            }
            Ok(found) => {
                if found.is_some() {
                    log::warn!("the keyring's PeetMail key has the wrong length; making a new one");
                }
                let key = new_key()?;
                keyring.set(&key).map(|()| key)
            }
            Err(e) => Err(e),
        };
        match stored {
            Ok(key) => {
                std::fs::write(&marker, b"")?;
                log::info!("saved sign-ins are encrypted with a key in the desktop keyring");
                Ok(key)
            }
            Err(KeyringError::Absent(why)) if !marker.exists() => {
                log::warn!("no desktop keyring ({why}); keeping the key for saved sign-ins in a private file");
                let key = new_key()?;
                write_private(&file, &key)?;
                Ok(key)
            }
            Err(KeyringError::Absent(why) | KeyringError::Refused(why)) => Err(refused(&why)),
        }
    }

    /// Writes `path` readable and writable by the owner only, whole or not at all.
    fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let tmp = path.with_extension("tmp");
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// The Secret Service (org.freedesktop.secrets) on the session bus.
    #[cfg(target_os = "linux")]
    struct SecretService;

    #[cfg(target_os = "linux")]
    fn attributes() -> HashMap<&'static str, &'static str> {
        HashMap::from([("application", crate::datadir::IDENTIFIER), ("purpose", "token-key")])
    }

    #[cfg(target_os = "linux")]
    fn connect() -> std::result::Result<secret_service::blocking::SecretService<'static>, KeyringError> {
        secret_service::blocking::SecretService::connect(secret_service::EncryptionType::Dh).map_err(|e| KeyringError::Absent(e.to_string()))
    }

    #[cfg(target_os = "linux")]
    fn refusal(e: secret_service::Error) -> KeyringError {
        KeyringError::Refused(e.to_string())
    }

    #[cfg(target_os = "linux")]
    impl Keyring for SecretService {
        fn get(&self) -> std::result::Result<Option<Vec<u8>>, KeyringError> {
            let ss = connect()?;
            let found = ss.search_items(attributes()).map_err(refusal)?;
            let Some(item) = found.unlocked.first().or(found.locked.first()) else { return Ok(None) };
            // `unlock` shows the keyring's password prompt and waits for the answer.
            if item.is_locked().map_err(refusal)? {
                item.unlock().map_err(refusal)?;
            }
            item.get_secret().map(Some).map_err(refusal)
        }

        fn set(&self, key: &[u8]) -> std::result::Result<(), KeyringError> {
            let ss = connect()?;
            let collection = ss.get_default_collection().map_err(|e| match e {
                secret_service::Error::NoResult => KeyringError::Absent("the keyring has no default collection".into()),
                e => refusal(e),
            })?;
            if collection.is_locked().map_err(refusal)? {
                collection.unlock().map_err(refusal)?;
            }
            collection
                .create_item("PeetMail: key for saved sign-ins", attributes(), key, true, "application/octet-stream")
                .map(|_| ())
                .map_err(refusal)
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn round_trip() {
        let secret = b"refresh-token-\x00\xff-bytes".repeat(200);
        let enc = super::protect(&secret).unwrap();
        assert_ne!(enc, secret);
        assert_eq!(super::unprotect(&enc).unwrap(), secret);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::linux::{load_or_create, Keyring, KeyringError, NoKeyring, KEY_FILE, MARKER_FILE};
    use super::sealed::{open, seal};
    use std::cell::RefCell;
    use std::os::unix::fs::PermissionsExt;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("peetmail-secure-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[derive(Clone, Copy)]
    enum Mode {
        Works,
        Absent,
        Locked,
    }

    /// An in-memory keyring that can be missing or locked.
    struct FakeKeyring {
        stored: RefCell<Option<Vec<u8>>>,
        mode: Mode,
    }
    impl FakeKeyring {
        fn new(mode: Mode) -> Self {
            FakeKeyring { stored: RefCell::new(None), mode }
        }
        fn fail(&self) -> Option<KeyringError> {
            match self.mode {
                Mode::Works => None,
                Mode::Absent => Some(KeyringError::Absent("no service".into())),
                Mode::Locked => Some(KeyringError::Refused("prompt dismissed".into())),
            }
        }
    }
    impl Keyring for FakeKeyring {
        fn get(&self) -> Result<Option<Vec<u8>>, KeyringError> {
            match self.fail() {
                Some(e) => Err(e),
                None => Ok(self.stored.borrow().clone()),
            }
        }
        fn set(&self, key: &[u8]) -> Result<(), KeyringError> {
            if let Some(e) = self.fail() {
                return Err(e);
            }
            *self.stored.borrow_mut() = Some(key.to_vec());
            Ok(())
        }
    }

    #[test]
    fn round_trip_and_tamper() {
        let key = [7u8; 32];
        let secret = b"refresh-token-\x00\xff-bytes".repeat(200);
        let enc = seal(&key, &secret).unwrap();
        assert!(!enc.windows(14).any(|w| w == b"refresh-token-"), "ciphertext holds the plaintext");
        assert_eq!(open(&key, &enc).unwrap(), secret);
        assert_ne!(seal(&key, &secret).unwrap(), enc, "every seal gets a fresh nonce");

        let mut flipped = enc.clone();
        *flipped.last_mut().unwrap() ^= 1;
        assert!(open(&key, &flipped).is_err(), "a changed byte is refused");
        assert!(open(&[8u8; 32], &enc).is_err(), "another key is refused");
        assert!(open(&key, &[]).is_err());
        assert!(open(&key, &enc[..5]).is_err());
    }

    #[test]
    fn key_goes_to_the_keyring_and_stays_there() {
        let dir = temp_dir("keyring");
        let ring = FakeKeyring::new(Mode::Works);
        let k1 = load_or_create(&dir, &ring).unwrap();
        assert_eq!(ring.stored.borrow().as_deref(), Some(&k1[..]));
        assert!(dir.join(MARKER_FILE).exists());
        assert!(!dir.join(KEY_FILE).exists(), "no key on disk when there's a keyring");
        assert_eq!(load_or_create(&dir, &ring).unwrap(), k1, "the same key next time");
    }

    #[test]
    fn no_keyring_means_a_private_key_file() {
        let dir = temp_dir("file");
        let k1 = load_or_create(&dir, &NoKeyring).unwrap();
        let file = dir.join(KEY_FILE);
        assert_eq!(std::fs::read(&file).unwrap(), k1);
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        // A keyring showing up later doesn't move the key (the saved sign-ins match the file).
        let ring = FakeKeyring::new(Mode::Works);
        assert_eq!(load_or_create(&dir, &ring).unwrap(), k1);
        assert!(ring.stored.borrow().is_none());
    }

    #[test]
    fn a_locked_keyring_is_an_error_never_a_key_file() {
        // First run with the keyring locked (autologin) and the unlock prompt dismissed.
        let dir = temp_dir("locked");
        let err = load_or_create(&dir, &FakeKeyring::new(Mode::Locked)).unwrap_err();
        assert!(err.to_string().contains("Unlock it"), "{err}");
        assert!(!dir.join(KEY_FILE).exists(), "a locked keyring must not push the key into a file");
        // Unlocked next time: the key goes to the keyring as normal.
        let ring = FakeKeyring::new(Mode::Works);
        load_or_create(&dir, &ring).unwrap();
        assert!(ring.stored.borrow().is_some());
    }

    #[test]
    fn a_keyring_that_stops_answering_is_an_error_not_a_new_key() {
        let dir = temp_dir("gone");
        load_or_create(&dir, &FakeKeyring::new(Mode::Works)).unwrap();
        assert!(load_or_create(&dir, &FakeKeyring::new(Mode::Absent)).is_err());
        assert!(load_or_create(&dir, &FakeKeyring::new(Mode::Locked)).is_err());
        assert!(!dir.join(KEY_FILE).exists());
    }

    #[test]
    fn a_damaged_key_file_is_set_aside_so_the_user_can_sign_in_again() {
        let dir = temp_dir("damaged");
        std::fs::write(dir.join(KEY_FILE), b"").unwrap();
        let k = load_or_create(&dir, &NoKeyring).unwrap();
        assert_eq!(std::fs::read(dir.join(KEY_FILE)).unwrap(), k);
        assert!(dir.join(format!("{KEY_FILE}.damaged")).exists());
    }
}
