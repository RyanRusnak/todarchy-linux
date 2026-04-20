// sync.rs — end-to-end encrypted sync.
//
// See docs/SYNC.md for the full protocol. This module exposes the Tauri
// commands the frontend dialog needs:
//
//   enroll(username, passphrase)        → { device_id, recovery_phrase }
//   sign_in(username, passphrase)       → { device_id }
//   recover_from_phrase(phrase, new_pp) → { device_id }
//   push(blob)                          → server_rev
//   pull(since_rev)                     → Vec<blob>
//
// Crypto:
//   KDF       Argon2id(passphrase, salt=username, 256MB/3/4)
//   Identity  age X25519 derived from KDF output
//   Payload   age encrypt to self-identity (XChaCha20-Poly1305)
//   Recovery  BIP39 24-word encoding of the raw KDF key, shown once
//
// The relay server is dumb: it stores ciphertext blobs keyed by
// (user_bucket_hash, rev). It NEVER sees the passphrase or plaintext.
//
// NOTE: this file is a scaffold. The real implementations are marked TODO
// and referenced in docs/SYNC.md. Claude Code: land them in this order:
//   1. derive_key() — Argon2id over passphrase+username
//   2. enroll() — generates recovery phrase, writes sync.age
//   3. push/pull against the relay (docs/RELAY.md has endpoints)

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct EnrollResult {
    pub device_id: String,
    pub recovery_phrase: String, // 24 words, shown exactly once
}

#[tauri::command]
pub async fn enroll(_username: String, _passphrase: String) -> Result<EnrollResult, String> {
    // TODO: derive key, persist sync.age, generate BIP39 phrase
    Err("not implemented — see docs/SYNC.md".into())
}

#[tauri::command]
pub async fn sign_in(_username: String, _passphrase: String) -> Result<String, String> {
    Err("not implemented".into())
}

#[tauri::command]
pub async fn recover_from_phrase(
    _phrase: String,
    _new_passphrase: String,
) -> Result<String, String> {
    Err("not implemented".into())
}

#[tauri::command]
pub async fn push(_blob: Vec<u8>) -> Result<u64, String> {
    Err("not implemented".into())
}

#[tauri::command]
pub async fn pull(_since_rev: u64) -> Result<Vec<Vec<u8>>, String> {
    Err("not implemented".into())
}
