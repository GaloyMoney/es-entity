//! Helpers for event-sourced entities that own per-entity encrypted data keys.
//!
//! The crate deliberately does not prescribe a cryptography implementation.
//! Applications can store their own encrypted data-key type on the entity state
//! and use this trait to expose the key lifecycle consistently.

/// Whether an entity still has an encrypted data key or has been crypto-shredded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerEntityDataKeyState {
    Present,
    Shredded,
}

/// Contract for entities whose sensitive event/state payloads are encrypted by
/// a per-entity data key.
///
/// Implementors should return `None` after the data key has been intentionally
/// removed. Historical ciphertext may remain in the event stream, but without
/// the data key the application should treat sensitive payloads as unrecoverable.
pub trait PerEntityDataKey {
    type EncryptedDataKey;

    fn encrypted_data_key(&self) -> Option<&Self::EncryptedDataKey>;

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
    }

    #[test]
    fn data_key_state_tracks_key_presence() {
        let present = EntityWithKey(Some("wrapped-key".to_string()));
        assert_eq!(present.data_key_state(), PerEntityDataKeyState::Present);
        assert!(!present.is_crypto_shredded());

        let shredded = EntityWithKey(None);
        assert_eq!(shredded.data_key_state(), PerEntityDataKeyState::Shredded);
        assert!(shredded.is_crypto_shredded());
    }
}
