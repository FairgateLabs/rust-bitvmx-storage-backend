use crate::error::StorageError;
use cocoon::MiniCocoon;
use rand::{rngs::SysRng, TryRng};

pub(crate) const DEK_LEN: usize = 32;

/// Data encryption key. Always exactly `DEK_LEN` bytes.
pub(crate) struct Dek([u8; DEK_LEN]);

impl Dek {
    pub(crate) fn generate() -> Result<Self, StorageError> {
        let mut bytes = [0u8; DEK_LEN];
        SysRng.try_fill_bytes(&mut bytes)?;
        Ok(Dek(bytes))
    }

    pub(crate) fn as_bytes(&self) -> &[u8; DEK_LEN] {
        &self.0
    }

    pub(crate) fn encrypt(&self, data: &[u8]) -> Result<Vec<u8>, StorageError> {
        let mut nonce_seed = [0u8; 32];
        SysRng.try_fill_bytes(&mut nonce_seed)?;
        MiniCocoon::from_key(&self.0, &nonce_seed)
            .wrap(data)
            .map_err(|error| StorageError::FailedToEncryptData { error })
    }

    pub(crate) fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>, StorageError> {
        // Nonce seed is not used during unwrap, the nonce is read from the ciphertext.
        MiniCocoon::from_key(&self.0, &[0u8; 32])
            .unwrap(data)
            .map_err(|error| StorageError::FailedToDecryptData { error })
    }
}

impl TryFrom<Vec<u8>> for Dek {
    type Error = StorageError;

    fn try_from(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        let len = bytes.len();
        bytes
            .try_into()
            .map(Dek)
            .map_err(|_| StorageError::InvalidDekLength(len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_from_rejects_wrong_length() {
        assert!(matches!(
            Dek::try_from(vec![0u8; 31]),
            Err(StorageError::InvalidDekLength(31))
        ));
        assert!(Dek::try_from(vec![0u8; DEK_LEN]).is_ok());
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let dek = Dek::generate().unwrap();
        let ciphertext = dek.encrypt(b"hello").unwrap();
        assert_eq!(dek.decrypt(&ciphertext).unwrap(), b"hello");
    }
}
