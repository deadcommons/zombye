//! Who a player is on a server (DCM-66): an Ed25519 key pair made once per install, kept in the
//! saves folder beside `settings.ron`. Joining, the server sends a challenge and the client signs
//! it, so a server knows the key without ever learning anything that would let it sign for us
//! elsewhere. Survivors and bans belong to the key; the name a player types is what others see.
//! No accounts and no service of ours: any server checks the signature itself.

use std::path::Path;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

/// The file in a game's saves folder that holds its key.
pub const FILE: &str = "identity.key";

/// A player's public key, as servers know it.
pub type Key = [u8; 32];
/// A server's challenge.
pub type Nonce = [u8; 32];

/// What is signed: a tag of its own (so the signature is good for nothing else), the challenge
/// and the name joined as.
fn message(nonce: &Nonce, name: &str) -> Vec<u8> {
    let mut m = b"zombye-join-v1\0".to_vec();
    m.extend_from_slice(nonce);
    m.extend_from_slice(name.as_bytes());
    m
}

/// A key pair (the secret half never leaves the machine).
#[derive(Clone)]
pub struct Identity(SigningKey);

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Identity({})", short(&self.key()))
    }
}

impl Identity {
    /// A new key pair from the operating system's randomness.
    pub fn generate() -> Result<Self, String> {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|e| format!("no randomness for a player key: {e}"))?;
        Ok(Self::from_seed(seed))
    }

    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self(SigningKey::from_bytes(&seed))
    }

    /// A key worked out from a name alone: for bots and tests, which have no install of their own.
    /// Anyone can work it out, so it proves nothing about who plays; a game client never uses it.
    pub fn for_name(name: &str) -> Self {
        let mut bytes = b"zombye-test-identity\0".to_vec();
        bytes.extend_from_slice(name.as_bytes());
        Self::from_seed(crate::modpack::sha256(&bytes))
    }

    /// The key kept at `path`, made and written there (readable by this user alone) the first time.
    /// The file holds the secret seed as 64 hex digits.
    pub fn load_or_create(path: &Path) -> Result<Self, String> {
        if let Ok(text) = std::fs::read_to_string(path) {
            return from_hex(text.trim()).map(Self::from_seed).ok_or_else(|| format!("{} isn't a player key: move it away and a new one is made (your survivors on servers stay with the old one)", path.display()));
        }
        let id = Self::generate()?;
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        write_private(path, &hex(&id.0.to_bytes())).map_err(|e| format!("can't keep the player key in {}: {e}", path.display()))?;
        Ok(id)
    }

    /// The public key.
    pub fn key(&self) -> Key {
        self.0.verifying_key().to_bytes()
    }

    /// The answer to a server's challenge, joining as `name`.
    pub fn sign(&self, nonce: &Nonce, name: &str) -> Vec<u8> {
        self.0.sign(&message(nonce, name)).to_bytes().to_vec()
    }
}

/// Did `key`'s owner sign `nonce` for `name`?
pub fn verify(key: &Key, nonce: &Nonce, name: &str, signature: &[u8]) -> bool {
    let (Ok(key), Ok(sig)) = (VerifyingKey::from_bytes(key), <[u8; 64]>::try_from(signature)) else { return false };
    key.verify_strict(&message(nonce, name), &Signature::from_bytes(&sig)).is_ok()
}

/// A fresh challenge.
pub fn nonce() -> Nonce {
    let mut n = [0u8; 32];
    if getrandom::fill(&mut n).is_err() {
        // No randomness: a clock is a poor challenge, but still never the same twice.
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        n = crate::modpack::sha256(&t.to_le_bytes());
    }
    n
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn from_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// The first 8 hex digits, for logs and the admin's lists.
pub fn short(key: &Key) -> String {
    hex(&key[..4])
}

#[cfg(unix)]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?.write_all(text.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_proves_the_key_the_challenge_and_the_name() {
        let me = Identity::for_name("ada");
        let n = nonce();
        let sig = me.sign(&n, "ada");
        assert!(verify(&me.key(), &n, "ada", &sig));
        assert!(!verify(&me.key(), &nonce(), "ada", &sig), "another challenge");
        assert!(!verify(&me.key(), &n, "bob", &sig), "another name");
        assert!(!verify(&Identity::for_name("bob").key(), &n, "ada", &sig), "another key");
        assert!(!verify(&me.key(), &n, "ada", &sig[..63]));
        assert_ne!(nonce(), nonce());
    }

    #[test]
    fn a_key_is_made_once_and_kept() {
        let dir = std::env::temp_dir().join(format!("zombye-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("identity.key");
        let a = Identity::load_or_create(&path).unwrap();
        let b = Identity::load_or_create(&path).unwrap();
        assert_eq!(a.key(), b.key());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::write(&path, "not a key").unwrap();
        assert!(Identity::load_or_create(&path).is_err(), "a damaged key is never replaced silently");
        assert_eq!(from_hex(&hex(&a.key())), Some(a.key()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
