//! Pure-Rust (RustCrypto) backend for the `ece` crate, so that decrypting
//! web pushes does not need OpenSSL (which is not available on Windows CI).

use std::any::Any;
use std::sync::Once;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use ece::{Cryptographer, EcKeyComponents, LocalKeyPair, RemotePublicKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use rand::RngCore;
use sha2::Sha256;

type EceResult<T> = std::result::Result<T, ece::Error>;

struct Local(p256::SecretKey);
struct Remote(p256::PublicKey);

fn raw_point(pk: &p256::PublicKey) -> Vec<u8> {
    pk.to_encoded_point(false).as_bytes().to_vec()
}

impl LocalKeyPair for Local {
    fn pub_as_raw(&self) -> EceResult<Vec<u8>> {
        Ok(raw_point(&self.0.public_key()))
    }
    fn raw_components(&self) -> EceResult<EcKeyComponents> {
        Ok(EcKeyComponents::new(self.0.to_bytes().to_vec(), raw_point(&self.0.public_key())))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl RemotePublicKey for Remote {
    fn as_raw(&self) -> EceResult<Vec<u8>> {
        Ok(raw_point(&self.0))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct RustCryptoBackend;

impl Cryptographer for RustCryptoBackend {
    fn generate_ephemeral_keypair(&self) -> EceResult<Box<dyn LocalKeyPair>> {
        Ok(Box::new(Local(p256::SecretKey::random(&mut rand::thread_rng()))))
    }

    fn import_key_pair(&self, c: &EcKeyComponents) -> EceResult<Box<dyn LocalKeyPair>> {
        let sk = p256::SecretKey::from_slice(c.private_key()).map_err(|_| ece::Error::InvalidKeyLength)?;
        Ok(Box::new(Local(sk)))
    }

    fn import_public_key(&self, raw: &[u8]) -> EceResult<Box<dyn RemotePublicKey>> {
        let pk = p256::PublicKey::from_sec1_bytes(raw).map_err(|_| ece::Error::InvalidKeyLength)?;
        Ok(Box::new(Remote(pk)))
    }

    fn compute_ecdh_secret(&self, remote: &dyn RemotePublicKey, local: &dyn LocalKeyPair) -> EceResult<Vec<u8>> {
        let r = remote.as_any().downcast_ref::<Remote>().ok_or(ece::Error::CryptoError)?;
        let l = local.as_any().downcast_ref::<Local>().ok_or(ece::Error::CryptoError)?;
        let shared = p256::ecdh::diffie_hellman(l.0.to_nonzero_scalar(), r.0.as_affine());
        Ok(shared.raw_secret_bytes().to_vec())
    }

    fn hkdf_sha256(&self, salt: &[u8], secret: &[u8], info: &[u8], len: usize) -> EceResult<Vec<u8>> {
        let mut out = vec![0u8; len];
        hkdf::Hkdf::<Sha256>::new(Some(salt), secret).expand(info, &mut out).map_err(|_| ece::Error::CryptoError)?;
        Ok(out)
    }

    fn aes_gcm_128_encrypt(&self, key: &[u8], iv: &[u8], data: &[u8]) -> EceResult<Vec<u8>> {
        let c = Aes128Gcm::new_from_slice(key).map_err(|_| ece::Error::InvalidKeyLength)?;
        c.encrypt(Nonce::from_slice(iv), data).map_err(|_| ece::Error::CryptoError)
    }

    fn aes_gcm_128_decrypt(&self, key: &[u8], iv: &[u8], ciphertext_and_tag: &[u8]) -> EceResult<Vec<u8>> {
        let c = Aes128Gcm::new_from_slice(key).map_err(|_| ece::Error::InvalidKeyLength)?;
        c.decrypt(Nonce::from_slice(iv), ciphertext_and_tag).map_err(|_| ece::Error::CryptoError)
    }

    fn random_bytes(&self, dest: &mut [u8]) -> EceResult<()> {
        rand::thread_rng().fill_bytes(dest);
        Ok(())
    }
}

/// Registers the backend once; safe to call before every decryption.
pub fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = ece::crypto::set_boxed_cryptographer(Box::new(RustCryptoBackend));
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn passes_ece_backend_test_suite() {
        // The suite also goes through the global backend.
        super::init();
        ece::crypto::test_cryptographer(super::RustCryptoBackend);
    }
}
