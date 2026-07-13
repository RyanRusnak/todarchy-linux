// cryptobox.rs — Symmetric envelope for shared Automerge docs.
//
// Byte-for-byte wire compatible with the iOS app's `CryptoBox.swift`.
// Any change here MUST be mirrored there (and vice versa); the two
// implementations have to seal/open each other's bytes interchangeably
// or shared-project sync silently breaks.
//
// Envelope layout:
//
//     ┌────────┬──────┬────────┬────────────────────┬──────────┐
//     │ "TDAR" │ ver  │ nonce  │    ciphertext      │ auth tag │
//     │  4B    │  1B  │  12B   │    variable        │   16B    │
//     └────────┴──────┴────────┴────────────────────┴──────────┘
//
// - Magic ("TDAR") lets a directory scan reject junk without trying to
//   decrypt it.
// - Version byte is for future cipher / header changes.
// - Nonce: 12 random bytes per seal. Critical: never reuse a
//   (key, nonce) pair — ChaCha20-Poly1305 loses confidentiality under
//   nonce reuse. Always pull from the OS CSPRNG.
// - Ciphertext + tag is the AEAD output (RFC 8439).

use chacha20poly1305::{
    aead::{Aead, KeyInit, OsRng},
    AeadCore, ChaCha20Poly1305, Key, Nonce,
};
use rand::RngCore;
use thiserror::Error;

/// 4-byte magic prefix on every envelope.
pub const MAGIC: [u8; 4] = *b"TDAR";
/// Envelope version. Bump when layout or cipher changes.
pub const CURRENT_VERSION: u8 = 0x01;
pub const NONCE_SIZE: usize = 12;
pub const TAG_SIZE: usize = 16;
pub const HEADER_SIZE: usize = MAGIC.len() + 1 + NONCE_SIZE;
pub const MIN_ENVELOPE_SIZE: usize = HEADER_SIZE + TAG_SIZE;

/// 256-bit key length matching iOS's `SymmetricKey(size: .bits256)`.
pub const KEY_BYTES: usize = 32;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BoxError {
    #[error("Not a todarchy encrypted envelope (magic bytes missing)")]
    BadMagic,
    #[error("Unsupported envelope version {0}")]
    UnsupportedVersion(u8),
    #[error("Envelope bytes are truncated")]
    Truncated,
    #[error("Decryption failed — wrong key or tampered bytes")]
    DecryptionFailed,
}

/// Encrypt `plaintext` under `key` and produce a full envelope.
///
/// A fresh random nonce is generated on every call. The iOS side uses
/// `ChaChaPoly.Nonce()` (CryptoKit) which is also OS-CSPRNG-backed —
/// both implementations satisfy the never-reuse-a-nonce invariant
/// transparently.
pub fn seal(plaintext: &[u8], key: &[u8; KEY_BYTES]) -> Vec<u8> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .expect("ChaCha20-Poly1305 encryption is infallible for valid keys + nonces");

    // ChaCha20Poly1305::encrypt returns ciphertext || tag concatenated.
    let mut out = Vec::with_capacity(HEADER_SIZE + ciphertext.len());
    out.extend_from_slice(&MAGIC);
    out.push(CURRENT_VERSION);
    out.extend_from_slice(nonce.as_slice());
    out.extend_from_slice(&ciphertext);
    out
}

/// Decrypt a previously-sealed envelope. Returns the original plaintext
/// or a typed error: bad magic / unknown version / truncated / auth
/// failure. Tamper detection is automatic via the Poly1305 tag.
pub fn open(envelope: &[u8], key: &[u8; KEY_BYTES]) -> Result<Vec<u8>, BoxError> {
    if envelope.len() < MIN_ENVELOPE_SIZE {
        return Err(BoxError::Truncated);
    }
    if envelope[0..MAGIC.len()] != MAGIC {
        return Err(BoxError::BadMagic);
    }
    let version = envelope[MAGIC.len()];
    if version != CURRENT_VERSION {
        return Err(BoxError::UnsupportedVersion(version));
    }
    let nonce_start = MAGIC.len() + 1;
    let nonce_end = nonce_start + NONCE_SIZE;
    let nonce = Nonce::from_slice(&envelope[nonce_start..nonce_end]);
    // The chacha20poly1305 crate expects ciphertext||tag concatenated,
    // which is exactly what `seal()` appended.
    let ciphertext_and_tag = &envelope[nonce_end..];

    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(nonce, ciphertext_and_tag)
        .map_err(|_| BoxError::DecryptionFailed)
}

/// Fresh 256-bit key for a new shared project. Calls the OS CSPRNG.
pub fn generate_key() -> [u8; KEY_BYTES] {
    let mut key = [0u8; KEY_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut key);
    key
}

/// Lightweight magic-byte check without authentication. Useful when
/// scanning a directory: we can skip files that aren't ours before
/// touching keyring or attempting an AEAD decrypt.
pub fn is_envelope(data: &[u8]) -> bool {
    data.len() >= MIN_ENVELOPE_SIZE
        && data[0..MAGIC.len()] == MAGIC
        && data[MAGIC.len()] == CURRENT_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_key() -> [u8; KEY_BYTES] {
        let mut k = [0u8; KEY_BYTES];
        for (i, b) in k.iter_mut().enumerate() {
            *b = i as u8;
        }
        k
    }

    #[test]
    fn round_trip_recovers_plaintext() {
        let key = fixed_key();
        let plaintext = b"the quick brown fox jumps over the lazy dog";
        let envelope = seal(plaintext, &key);
        let recovered = open(&envelope, &key).unwrap();
        assert_eq!(recovered, plaintext);
    }

    #[test]
    fn envelope_starts_with_magic_then_version_byte() {
        let envelope = seal(b"abc", &fixed_key());
        assert_eq!(&envelope[0..4], &MAGIC);
        assert_eq!(envelope[4], CURRENT_VERSION);
    }

    #[test]
    fn fresh_nonce_per_seal_produces_distinct_ciphertexts() {
        // Same key + same plaintext on two seals must produce different
        // bytes — otherwise we're reusing a nonce, which is a
        // catastrophic ChaCha20-Poly1305 failure mode.
        let key = fixed_key();
        let a = seal(b"identical input", &key);
        let b = seal(b"identical input", &key);
        assert_ne!(a, b, "nonce must be unique per seal");
        // Plaintexts still recover correctly under both.
        assert_eq!(open(&a, &key).unwrap(), b"identical input");
        assert_eq!(open(&b, &key).unwrap(), b"identical input");
    }

    #[test]
    fn tamper_in_ciphertext_fails() {
        let key = fixed_key();
        let mut envelope = seal(b"sensitive content", &key);
        // Flip a bit in the middle of the ciphertext.
        let mid = envelope.len() / 2;
        envelope[mid] ^= 0x01;
        assert_eq!(open(&envelope, &key), Err(BoxError::DecryptionFailed));
    }

    #[test]
    fn tamper_in_auth_tag_fails() {
        let key = fixed_key();
        let mut envelope = seal(b"sensitive content", &key);
        let last = envelope.len() - 1;
        envelope[last] ^= 0x80;
        assert_eq!(open(&envelope, &key), Err(BoxError::DecryptionFailed));
    }

    #[test]
    fn wrong_key_fails_cleanly() {
        let envelope = seal(b"hello", &fixed_key());
        let mut wrong = fixed_key();
        wrong[0] ^= 0xff;
        assert_eq!(open(&envelope, &wrong), Err(BoxError::DecryptionFailed));
    }

    #[test]
    fn bad_magic_is_rejected_without_aead() {
        let key = fixed_key();
        let mut envelope = seal(b"hello", &key);
        envelope[0] = b'X';
        assert_eq!(open(&envelope, &key), Err(BoxError::BadMagic));
    }

    #[test]
    fn unknown_version_is_rejected() {
        let key = fixed_key();
        let mut envelope = seal(b"hello", &key);
        envelope[4] = 0xff;
        assert_eq!(open(&envelope, &key), Err(BoxError::UnsupportedVersion(0xff)));
    }

    #[test]
    fn truncated_envelope_is_rejected() {
        // Anything shorter than MIN_ENVELOPE_SIZE is truncated by
        // definition — there's no room for a valid header + tag.
        let short = vec![0u8; MIN_ENVELOPE_SIZE - 1];
        assert_eq!(open(&short, &fixed_key()), Err(BoxError::Truncated));
    }

    #[test]
    fn is_envelope_distinguishes_our_format_from_random_bytes() {
        let envelope = seal(b"x", &fixed_key());
        assert!(is_envelope(&envelope));
        assert!(!is_envelope(&[0u8; MIN_ENVELOPE_SIZE]));
        assert!(!is_envelope(b"TDAR\x02 not version 1 ..............."));
    }

    #[test]
    fn generated_keys_are_distinct() {
        // Sanity check that we're not seeding with a constant.
        let a = generate_key();
        let b = generate_key();
        assert_ne!(a, b);
    }
}
