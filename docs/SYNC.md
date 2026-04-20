# Sync protocol — end-to-end encrypted

## Goals
- Server learns **nothing** about task contents, titles, counts, or structure.
- Losing your device should not lose your data, *provided* you have the 24-word phrase.
- Losing your passphrase AND phrase is unrecoverable. That's the point.

## Primitives
- **KDF**: Argon2id. `m=256 MiB, t=3, p=4`. Salt = SHA-256(username). Output: 32 bytes.
- **Identity**: the 32 bytes are used directly as an `age` X25519 scalar (the crate provides `IdentityFile` helpers to wrap raw keys).
- **Payload encryption**: `age` with the self-identity as the sole recipient — internally XChaCha20-Poly1305 + Poly1305 MAC with per-blob random nonce. No AEAD reuse.
- **Recovery phrase**: the raw 32-byte KDF output encoded as BIP39 (24 words, 256-bit entropy). Shown exactly once, on enrollment. Never persisted server-side.

## Enrollment
1. User picks `username` + `passphrase` (≥ 8 chars; UI hints to use a passphrase, not a password).
2. Client derives `K = Argon2id(passphrase, sha256(username))`.
3. Client converts `K` → BIP39 24-word phrase, shows it, requires checkbox "I've written this down".
4. Client derives `device_id = blake3(K || hostname)` and writes `~/.config/todarchy/sync.age` (age-encrypted blob containing `K`; passphrase as scrypt recipient — so `K` can be decrypted offline on boot without re-running Argon2).
5. First push: client encrypts current `tasks.json` with `K` and POSTs to the relay.

## Sign-in (new device)
1. Enter username + passphrase.
2. Re-run Argon2 to derive `K'`.
3. Pull `user_bucket_hash = blake3(K' || "bucket")` from relay. If the bucket returns 404, the passphrase is wrong (server does not distinguish "wrong pass" from "no account" — same error).
4. Decrypt latest blob with `K'`. If decryption fails, passphrase was wrong.
5. Write `sync.age` locally.

## Recovery (lost passphrase, have phrase)
1. Paste 24 words → get back raw `K`.
2. User picks a new passphrase.
3. Re-derive bucket hash from `K`, decrypt state, re-encrypt `sync.age` with new passphrase.
4. **No re-encryption of server blobs needed** — `K` hasn't changed.

## Server (relay)
Endpoints. Authentication is by bucket-hash + MAC of request body; no accounts.

```
POST   /b/:bucket_hash         body: ciphertext blob          → { rev }
GET    /b/:bucket_hash?since=N                                → [{rev, blob}]
DELETE /b/:bucket_hash/:rev
```

Rate-limit per bucket (1 req/s sustained, burst 30).
Max blob = 1 MiB. Reject larger (force client to chunk).
Storage: SQLite file, `(bucket_hash, rev, blob, created_at)`.
Retention: 30 days of history, configurable.

## Merge (client side)
Each task carries `{id, updated_at, author_device, tombstone}`. On pull:
- Group by `id`. Latest `updated_at` wins (ties broken by lexicographic `author_device`).
- A task with `tombstone=true` older than 90 days is GCed.

## Threat model
| Adversary | Can they? |
|---|---|
| Relay operator | No — sees only opaque blobs + traffic metadata |
| Passive network | No — TLS + blobs encrypted anyway |
| Active network (MITM) | Only DoS — they can't forge blobs (MAC fails) |
| Device thief (no passphrase) | No — `sync.age` is scrypt-protected |
| Shoulder-surfer of passphrase | Yes — this is a passphrase auth system, protect it |

## Claude Code implementation order
1. `derive_key()` + BIP39 round-trip test.
2. `enroll()` writes `sync.age`, returns the phrase.
3. Add a `--relay-url` config key; default to a dev relay.
4. Implement `push` / `pull` with hyper/reqwest.
5. Wire the React `SyncDialog` (port from `design-mocks/src/sync-dialog.jsx`) to the commands.
6. Merge logic + tombstone GC.
7. Separate `relay/` repo — axum + sqlx, ~200 LOC.
