//! Helpers for event-sourced entities that use per-entity encrypted data keys.
//!
//! The crate deliberately does not prescribe a cryptography implementation.
//! Applications can store their own encrypted data-key type in mutable
//! projection state and use this trait to expose the key lifecycle consistently.

/// Whether an entity still has an encrypted data key or has been crypto-shredded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerEntityDataKeyState {
    Present,
    Shredded,
}

/// Contract for entities whose sensitive payloads are encrypted by a
/// per-entity data key.
///
/// Implementors should keep the encrypted data key outside immutable event
/// payloads, attach it to hydrated state when the projection row is loaded, and
/// return `None` after the data key has been intentionally removed. Historical
/// ciphertext may remain in the event stream, but without the data key the
/// application should treat sensitive payloads as unrecoverable.
pub trait PerEntityDataKey {
    type EncryptedDataKey;

    fn encrypted_data_key(&self) -> Option<&Self::EncryptedDataKey>;

    fn attach_encrypted_data_key(&mut self, encrypted_data_key: Self::EncryptedDataKey);

    fn clear_encrypted_data_key(&mut self);

    fn data_key_state(&self) -> PerEntityDataKeyState {
        if self.encrypted_data_key().is_some() {
            PerEntityDataKeyState::Present
        } else {
            PerEntityDataKeyState::Shredded
        }
    }

    fn is_crypto_shredded(&self) -> bool {
        self.data_key_state() == PerEntityDataKeyState::Shredded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EntityWithKey(Option<String>);

    impl PerEntityDataKey for EntityWithKey {
        type EncryptedDataKey = String;

        fn encrypted_data_key(&self) -> Option<&Self::EncryptedDataKey> {
            self.0.as_ref()
        }

        fn attach_encrypted_data_key(&mut self, encrypted_data_key: Self::EncryptedDataKey) {
            self.0 = Some(encrypted_data_key);
        }

        fn clear_encrypted_data_key(&mut self) {
            self.0 = None;
        }
    }

    #[test]
    fn data_key_state_tracks_key_lifecycle() {
        let mut entity = EntityWithKey(None);
        assert_eq!(entity.data_key_state(), PerEntityDataKeyState::Shredded);
        assert!(entity.is_crypto_shredded());

        entity.attach_encrypted_data_key("wrapped-key".to_string());
        assert_eq!(entity.data_key_state(), PerEntityDataKeyState::Present);
        assert!(!entity.is_crypto_shredded());

        entity.clear_encrypted_data_key();
        assert_eq!(entity.data_key_state(), PerEntityDataKeyState::Shredded);
        assert!(entity.is_crypto_shredded());
    }
}
