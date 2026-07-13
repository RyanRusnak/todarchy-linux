// sharelink.rs — encode/decode `todarchy://share/<id>#k=<base64url-key>` URLs.
//
// Wire-compatible with the iOS `ShareLink.swift`. The two implementations
// must round-trip each other's links — the fragment-with-key design is
// load-bearing: fragments never reach HTTP servers, so a future
// `https://todarchy.app/...` landing page that redirects into the
// custom scheme still can't leak keys into server logs.
//
// Forward compat: the fragment is a `&`-joined list of `key=value` pairs.
// Decoder accepts `#k=X&v=2&exp=...` and ignores unknown keys, so older
// clients keep parsing newer links.

use base64::Engine;
use thiserror::Error;

use crate::cryptobox::KEY_BYTES;

pub const SCHEME: &str = "todarchy";
pub const HOST: &str = "share";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("Not a todarchy share link")]
    WrongScheme,
    #[error("Share link is missing the project id")]
    MalformedPath,
    #[error("Share link has no key fragment (#k=…)")]
    MissingKey,
    #[error("Share link's key isn't a valid 256-bit value")]
    BadKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    pub project_id: String,
    pub key: [u8; KEY_BYTES],
}

/// Encode `(project_id, key)` into a `todarchy://share/<id>#k=<...>` URL.
///
/// The project id is percent-encoded for URL safety. The key is encoded
/// as 43-char unpadded base64url (RFC 4648 §5) — alphabet `A-Z a-z 0-9
/// - _`, no `+`/`/`/`=`.
pub fn encode(project_id: &str, key: &[u8; KEY_BYTES]) -> String {
    let id = percent_encode_path(project_id);
    let key_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key);
    format!("{SCHEME}://{HOST}/{id}#k={key_b64}")
}

/// Decode a `todarchy://share/<id>#k=<...>` URL into its components.
pub fn decode(input: &str) -> Result<Payload, DecodeError> {
    // Split off the fragment first. Form: <prefix>#<fragment>.
    let (prefix, fragment) = match input.split_once('#') {
        Some((p, f)) => (p, f),
        None => (input, ""),
    };

    // Scheme + host. Lower-case the scheme check since some
    // platforms canonicalize to lowercase, but iOS encodes exactly
    // "todarchy://" so we accept both.
    let prefix_lower = prefix.to_ascii_lowercase();
    let expected_prefix = format!("{SCHEME}://{HOST}/");
    if !prefix_lower.starts_with(&expected_prefix) {
        return Err(DecodeError::WrongScheme);
    }
    let raw_path = &prefix[expected_prefix.len()..];
    let trimmed_path = raw_path.trim_matches('/');
    if trimmed_path.is_empty() {
        return Err(DecodeError::MalformedPath);
    }
    let project_id = percent_decode(trimmed_path);

    // Fragment: k=<value>[&k2=v2...]. We ignore anything we don't
    // recognize so future fields ride along without breaking old clients.
    if fragment.is_empty() {
        return Err(DecodeError::MissingKey);
    }
    let key_b64 = fragment_value("k", fragment).ok_or(DecodeError::MissingKey)?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(key_b64.as_bytes())
        .map_err(|_| DecodeError::BadKey)?;
    if raw.len() != KEY_BYTES {
        return Err(DecodeError::BadKey);
    }
    let mut key = [0u8; KEY_BYTES];
    key.copy_from_slice(&raw);
    Ok(Payload { project_id, key })
}

/// Find `name=value` inside a `key=value&key=value` fragment. Returns the
/// first occurrence; later occurrences (which would mean a malformed
/// link) are ignored.
fn fragment_value(name: &str, fragment: &str) -> Option<String> {
    for part in fragment.split('&') {
        let mut kv = part.splitn(2, '=');
        let k = kv.next()?;
        let v = kv.next()?;
        if k == name {
            return Some(v.to_string());
        }
    }
    None
}

/// Percent-encode characters that aren't allowed in a URL path. We use a
/// minimal allowlist (alphanumerics + a few path-safe punctuation chars)
/// matching Swift's `.urlPathAllowed` set, so encoded ids round-trip.
fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if is_path_safe(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn is_path_safe(b: u8) -> bool {
    matches!(b,
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' |
        b'-' | b'_' | b'.' | b'~' |   // unreserved (RFC 3986 §2.3)
        b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' |
        b'+' | b',' | b';' | b'=' | b':' | b'@'
    )
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_key() -> [u8; KEY_BYTES] {
        let mut k = [0u8; KEY_BYTES];
        for (i, b) in k.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(31);
        }
        k
    }

    #[test]
    fn encode_then_decode_round_trips() {
        let key = fixed_key();
        let url = encode("p_abc12345", &key);
        let payload = decode(&url).unwrap();
        assert_eq!(payload.project_id, "p_abc12345");
        assert_eq!(payload.key, key);
    }

    #[test]
    fn encoded_url_has_the_expected_shape() {
        let url = encode("p_abc12345", &fixed_key());
        assert!(url.starts_with("todarchy://share/p_abc12345#k="));
    }

    #[test]
    fn key_in_link_uses_base64url_alphabet_no_padding() {
        // Pin the alphabet — `+`/`/`/`=` would break URL embedding and
        // diverge from the iOS encoder.
        let url = encode("p_test", &fixed_key());
        let frag = url.split_once('#').unwrap().1;
        let key_part = frag.strip_prefix("k=").unwrap();
        assert!(!key_part.contains('+'));
        assert!(!key_part.contains('/'));
        assert!(!key_part.contains('='));
        // 256 bits / 6 = 42.66 → 43 base64url chars unpadded.
        assert_eq!(key_part.len(), 43);
    }

    #[test]
    fn decode_rejects_other_schemes() {
        let url = format!("https://share/p_abc#k={}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(fixed_key()));
        assert_eq!(decode(&url), Err(DecodeError::WrongScheme));
    }

    #[test]
    fn decode_requires_share_host() {
        let url = format!("todarchy://other/p_abc#k={}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(fixed_key()));
        assert_eq!(decode(&url), Err(DecodeError::WrongScheme));
    }

    #[test]
    fn decode_rejects_empty_project_id() {
        let url = format!("todarchy://share/#k={}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(fixed_key()));
        assert_eq!(decode(&url), Err(DecodeError::MalformedPath));
    }

    #[test]
    fn decode_rejects_missing_key() {
        assert_eq!(decode("todarchy://share/p_abc"), Err(DecodeError::MissingKey));
        // Fragment present but no `k=`.
        assert_eq!(decode("todarchy://share/p_abc#v=2"), Err(DecodeError::MissingKey));
    }

    #[test]
    fn decode_rejects_wrong_length_key() {
        // 16 bytes, not 32 — still valid base64url but wrong length.
        let short = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7u8; 16]);
        let url = format!("todarchy://share/p_abc#k={short}");
        assert_eq!(decode(&url), Err(DecodeError::BadKey));
    }

    #[test]
    fn decode_rejects_invalid_base64url() {
        let url = "todarchy://share/p_abc#k=not!base64";
        assert_eq!(decode(url), Err(DecodeError::BadKey));
    }

    #[test]
    fn decode_ignores_unknown_fragment_keys_for_forward_compat() {
        // Future versions may add &v=2 or &exp=<ms>. Our decoder must
        // still extract `k` without complaint.
        let key = fixed_key();
        let key_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key);
        let url = format!("todarchy://share/p_abc#k={key_b64}&v=2&exp=1700000000000");
        let payload = decode(&url).unwrap();
        assert_eq!(payload.project_id, "p_abc");
        assert_eq!(payload.key, key);
    }

    #[test]
    fn decode_handles_percent_encoded_project_id() {
        let url = format!("todarchy://share/p%20space#k={}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(fixed_key()));
        assert_eq!(decode(&url).unwrap().project_id, "p space");
    }

    #[test]
    fn encode_percent_encodes_unsafe_path_chars() {
        // A space isn't allowed in a URL path — must come out percent-encoded.
        let url = encode("has space", &fixed_key());
        assert!(url.contains("has%20space"), "got {url}");
    }
}
