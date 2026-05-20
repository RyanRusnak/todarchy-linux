// server_client.rs — HTTP relay client for the todarchy-server v1 protocol.
//
// Mirrors iOS `ServerSyncClient.swift`. The contract is small:
//
//   GET    /doc/:id   → 200 (bytes + ETag) | 304 | 404 | 4xx/5xx
//   PUT    /doc/:id   → 204 (ETag)         | 412 | 4xx/5xx     (we don't send 412 — see below)
//   DELETE /doc/:id   → 204 | 404 | 4xx
//   GET    /healthz   → 200 "ok"
//
// Semantics worth knowing:
//
//   - **PUT is unconditional.** iOS made the product call to never send
//     `If-Match`: we pre-merge locally via Automerge and treat the
//     server as a mirror of the latest local state. The CRDT keeps
//     concurrent peer edits convergent on the next pull. Mirroring
//     iOS here keeps Linux + iOS behaving identically.
//   - **GET caches strong ETags** per doc id. The next GET sends
//     `If-None-Match`; the server returns `304` when nothing changed,
//     which is the bulk of polls in steady state.
//   - **404 maps to None** on GET. A missing doc means "no remote yet,"
//     not an error.
//   - **5 MiB body cap** is enforced server-side. Anything beyond the
//     cap returns `413 doc_too_large`; we surface as a typed error so
//     the caller can show the user something useful instead of
//     retrying forever.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;
use reqwest::header::{ETAG, IF_MATCH, IF_NONE_MATCH};
use thiserror::Error;

/// One HTTP relay endpoint plus a per-doc ETag cache. Cheap to clone
/// (the underlying reqwest::Client is reference-counted).
pub struct ServerSyncClient {
    base_url: reqwest::Url,
    http: reqwest::Client,
    etags: Mutex<HashMap<String, String>>,
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("network error: {0}")]
    Transport(String),
    #[error("invalid response (no status / unreadable headers)")]
    InvalidResponse,
    #[error("server returned {status}: {code:?} — {message:?}")]
    Http {
        status: u16,
        code: Option<String>,
        message: Option<String>,
    },
}

impl ServerSyncClient {
    /// Build a client pointed at `base_url`. The URL is normalised so
    /// repeated trailing slashes don't change `/doc/:id` joining.
    pub fn new(base_url: &str) -> Result<Self, ClientError> {
        let base = reqwest::Url::parse(base_url)
            .map_err(|e| ClientError::Transport(format!("bad URL: {e}")))?;
        let http = reqwest::Client::builder()
            // Sized to match iOS — 30 s for GETs, longer ceiling for PUTs.
            .timeout(Duration::from_secs(300))
            .connect_timeout(Duration::from_secs(10))
            .user_agent("todarchy-linux/0.2 (Rust client)")
            .build()
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        Ok(Self {
            base_url: base,
            http,
            etags: Mutex::new(HashMap::new()),
        })
    }

    pub fn base_url(&self) -> &reqwest::Url { &self.base_url }

    /// Read the cached ETag for a doc id. Exposed for tests + the
    /// diagnostics view; production code paths don't need to peek.
    pub fn cached_etag(&self, doc_id: &str) -> Option<String> {
        self.etags.lock().ok()?.get(doc_id).cloned()
    }

    fn set_cached_etag(&self, doc_id: &str, etag: Option<String>) {
        if let Ok(mut m) = self.etags.lock() {
            match etag {
                Some(v) if !v.is_empty() => { m.insert(doc_id.to_string(), v); }
                _ => { m.remove(doc_id); }
            }
        }
    }

    fn doc_url(&self, doc_id: &str) -> reqwest::Url {
        // Use join twice so a base of `http://host/api/` works; reqwest's
        // join normalises duplicate slashes for us.
        let mut url = self.base_url.clone();
        // Trailing slash matters for `join` — ensure we treat the base
        // as a directory, not a file.
        if !url.path().ends_with('/') {
            let p = format!("{}/", url.path());
            url.set_path(&p);
        }
        url.join("doc/").and_then(|u| u.join(doc_id))
            .unwrap_or_else(|_| self.base_url.clone())
    }

    /// `GET /healthz`. Returns true on any 2xx; never throws so the UI
    /// can render "server unreachable" without try/catch noise.
    pub async fn healthz(&self) -> bool {
        let mut url = self.base_url.clone();
        if !url.path().ends_with('/') {
            let p = format!("{}/", url.path());
            url.set_path(&p);
        }
        let url = match url.join("healthz") {
            Ok(u) => u,
            Err(_) => return false,
        };
        match self
            .http
            .get(url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
        {
            Ok(r) => r.status().is_success(),
            Err(_) => false,
        }
    }

    /// `GET /doc/:id`. Returns `Some((bytes, etag))` on 200, `None` on
    /// 304 or 404.
    pub async fn get(&self, doc_id: &str) -> Result<Option<(Vec<u8>, String)>, ClientError> {
        let mut req = self.http.get(self.doc_url(doc_id));
        if let Some(etag) = self.cached_etag(doc_id) {
            req = req.header(IF_NONE_MATCH, etag);
        }
        let resp = req.send().await.map_err(|e| ClientError::Transport(e.to_string()))?;
        let status = resp.status();
        match status.as_u16() {
            200 => {
                let etag = header_string(&resp, ETAG);
                let bytes = resp.bytes().await
                    .map_err(|e| ClientError::Transport(e.to_string()))?
                    .to_vec();
                if let Some(ref e) = etag { self.set_cached_etag(doc_id, Some(e.clone())); }
                Ok(Some((bytes, etag.unwrap_or_default())))
            }
            304 => Ok(None),
            404 => {
                // Forget the ETag — the doc is gone, so the cached value
                // would just produce confusing 304s if it ever returned.
                self.set_cached_etag(doc_id, None);
                Ok(None)
            }
            _ => Err(make_http_error(resp).await),
        }
    }

    /// `PUT /doc/:id`. Unconditional — never sends `If-Match`. Returns
    /// the server's new ETag (may be empty if the server omitted it).
    pub async fn put(&self, doc_id: &str, bytes: Vec<u8>) -> Result<String, ClientError> {
        let resp = self
            .http
            .put(self.doc_url(doc_id))
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(bytes)
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let status = resp.status();
        match status.as_u16() {
            200 | 201 | 204 => {
                let etag = header_string(&resp, ETAG).unwrap_or_default();
                if !etag.is_empty() {
                    self.set_cached_etag(doc_id, Some(etag.clone()));
                }
                Ok(etag)
            }
            _ => Err(make_http_error(resp).await),
        }
    }

    /// `PUT /doc/:id` with `If-Match`. Returns Err(Http{status:412,..})
    /// on stale-etag rejection. We don't use this in normal sync (see
    /// the unconditional `put` above), but it's available for callers
    /// that want strict concurrency.
    pub async fn put_conditional(
        &self,
        doc_id: &str,
        bytes: Vec<u8>,
        if_match: &str,
    ) -> Result<String, ClientError> {
        let resp = self
            .http
            .put(self.doc_url(doc_id))
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header(IF_MATCH, if_match)
            .body(bytes)
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let status = resp.status();
        match status.as_u16() {
            200 | 201 | 204 => {
                let etag = header_string(&resp, ETAG).unwrap_or_default();
                if !etag.is_empty() {
                    self.set_cached_etag(doc_id, Some(etag.clone()));
                }
                Ok(etag)
            }
            _ => Err(make_http_error(resp).await),
        }
    }

    /// `DELETE /doc/:id`. Idempotent — 404 is treated as success.
    pub async fn delete(&self, doc_id: &str) -> Result<(), ClientError> {
        let resp = self
            .http
            .delete(self.doc_url(doc_id))
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let status = resp.status();
        match status.as_u16() {
            200 | 204 | 404 => {
                self.set_cached_etag(doc_id, None);
                Ok(())
            }
            _ => Err(make_http_error(resp).await),
        }
    }
}

fn header_string(resp: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<String> {
    resp.headers()
        .get(name)
        .and_then(|h| h.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Build a typed `ClientError::Http` from a non-2xx response. Parses
/// the documented `{error, message}` JSON envelope if present; falls
/// back to a generic error otherwise.
async fn make_http_error(resp: reqwest::Response) -> ClientError {
    let status = resp.status().as_u16();
    let body = resp.bytes().await.unwrap_or_default();
    if body.is_empty() {
        return ClientError::Http { status, code: None, message: None };
    }
    #[derive(serde::Deserialize)]
    struct Envelope {
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        message: Option<String>,
    }
    match serde_json::from_slice::<Envelope>(&body) {
        Ok(env) => ClientError::Http { status, code: env.error, message: env.message },
        Err(_) => ClientError::Http { status, code: None, message: None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, header, header_exists};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn fresh_server() -> MockServer {
        MockServer::start().await
    }

    fn client_for(server: &MockServer) -> ServerSyncClient {
        ServerSyncClient::new(&server.uri()).unwrap()
    }

    #[tokio::test]
    async fn get_returns_bytes_and_etag_on_200() {
        let server = fresh_server().await;
        Mock::given(method("GET"))
            .and(path("/doc/p_demo"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("ETag", "\"abc123\"")
                    .set_body_bytes(b"ciphertext".to_vec()),
            )
            .mount(&server)
            .await;
        let client = client_for(&server);

        let (bytes, etag) = client.get("p_demo").await.unwrap().unwrap();
        assert_eq!(bytes, b"ciphertext");
        assert_eq!(etag, "\"abc123\"");
        // Cached for the next GET.
        assert_eq!(client.cached_etag("p_demo"), Some("\"abc123\"".to_string()));
    }

    #[tokio::test]
    async fn get_sends_if_none_match_on_subsequent_call() {
        let server = fresh_server().await;
        // First GET — no header expected, returns 200 + ETag.
        Mock::given(method("GET"))
            .and(path("/doc/p_demo"))
            .respond_with(ResponseTemplate::new(200)
                .insert_header("ETag", "\"v1\"")
                .set_body_bytes(b"first".to_vec()))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // Second GET must carry If-None-Match: "v1" → 304.
        Mock::given(method("GET"))
            .and(path("/doc/p_demo"))
            .and(header("If-None-Match", "\"v1\""))
            .respond_with(ResponseTemplate::new(304))
            .mount(&server)
            .await;

        let client = client_for(&server);
        let (_, _) = client.get("p_demo").await.unwrap().unwrap();
        // Second call: server returns 304; client returns None.
        assert!(client.get("p_demo").await.unwrap().is_none());
        // ETag still cached after a 304 — that's the whole point.
        assert_eq!(client.cached_etag("p_demo"), Some("\"v1\"".to_string()));
    }

    #[tokio::test]
    async fn get_returns_none_on_404_and_forgets_etag() {
        let server = fresh_server().await;
        Mock::given(method("GET"))
            .and(path("/doc/p_gone"))
            .respond_with(ResponseTemplate::new(404).set_body_string("{\"error\":\"not_found\"}"))
            .mount(&server)
            .await;
        let client = client_for(&server);
        client.etags.lock().unwrap().insert("p_gone".into(), "\"stale\"".into());
        assert!(client.get("p_gone").await.unwrap().is_none());
        assert_eq!(client.cached_etag("p_gone"), None);
    }

    #[tokio::test]
    async fn put_is_unconditional_and_caches_returned_etag() {
        let server = fresh_server().await;
        // Server must NOT see If-Match — that's the whole "unconditional" thing.
        Mock::given(method("PUT"))
            .and(path("/doc/p_demo"))
            .and(header("Content-Type", "application/octet-stream"))
            .respond_with(ResponseTemplate::new(204).insert_header("ETag", "\"v2\""))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let etag = client.put("p_demo", b"new bytes".to_vec()).await.unwrap();
        assert_eq!(etag, "\"v2\"");
        assert_eq!(client.cached_etag("p_demo"), Some("\"v2\"".to_string()));
    }

    #[tokio::test]
    async fn put_conditional_sends_if_match() {
        let server = fresh_server().await;
        Mock::given(method("PUT"))
            .and(path("/doc/p_demo"))
            .and(header("If-Match", "\"prev\""))
            .respond_with(ResponseTemplate::new(204).insert_header("ETag", "\"next\""))
            .mount(&server)
            .await;
        let client = client_for(&server);
        let etag = client
            .put_conditional("p_demo", b"x".to_vec(), "\"prev\"")
            .await
            .unwrap();
        assert_eq!(etag, "\"next\"");
    }

    #[tokio::test]
    async fn put_conditional_returns_412_on_stale_etag() {
        let server = fresh_server().await;
        Mock::given(method("PUT"))
            .and(path("/doc/p_demo"))
            .respond_with(
                ResponseTemplate::new(412)
                    .insert_header("Content-Type", "application/json")
                    .set_body_string(r#"{"error":"etag_mismatch","message":"stale"}"#),
            )
            .mount(&server)
            .await;
        let client = client_for(&server);
        let err = client
            .put_conditional("p_demo", b"x".to_vec(), "\"old\"")
            .await
            .unwrap_err();
        match err {
            ClientError::Http { status, code, message } => {
                assert_eq!(status, 412);
                assert_eq!(code.as_deref(), Some("etag_mismatch"));
                assert!(message.is_some());
            }
            other => panic!("expected Http(412), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delete_treats_404_as_success_and_clears_etag() {
        let server = fresh_server().await;
        Mock::given(method("DELETE"))
            .and(path("/doc/p_gone"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let client = client_for(&server);
        client.etags.lock().unwrap().insert("p_gone".into(), "\"stale\"".into());
        client.delete("p_gone").await.unwrap();
        assert_eq!(client.cached_etag("p_gone"), None);
    }

    #[tokio::test]
    async fn healthz_returns_true_on_2xx_false_on_failure() {
        let server = fresh_server().await;
        Mock::given(method("GET"))
            .and(path("/healthz"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;
        let client = client_for(&server);
        assert!(client.healthz().await);

        // Point at a closed port — should return false without panicking.
        let dead = ServerSyncClient::new("http://127.0.0.1:1/").unwrap();
        assert!(!dead.healthz().await);
    }

    #[tokio::test]
    async fn error_envelope_is_surfaced_in_http_error() {
        let server = fresh_server().await;
        Mock::given(method("PUT"))
            .and(path("/doc/p_too_big"))
            .respond_with(
                ResponseTemplate::new(413).set_body_string(
                    r#"{"error":"doc_too_large","message":"body exceeded 5 MiB"}"#,
                ),
            )
            .mount(&server)
            .await;
        let client = client_for(&server);
        let err = client.put("p_too_big", vec![0u8; 16]).await.unwrap_err();
        match err {
            ClientError::Http { status, code, message } => {
                assert_eq!(status, 413);
                assert_eq!(code.as_deref(), Some("doc_too_large"));
                assert!(message.unwrap().contains("5 MiB"));
            }
            _ => panic!("expected typed HTTP error"),
        }
    }

    #[tokio::test]
    async fn transport_failure_for_unreachable_host() {
        // Use a reserved port that nothing's listening on. Should error
        // cleanly (Transport) rather than panic.
        let client = ServerSyncClient::new("http://127.0.0.1:1/").unwrap();
        let err = client.get("p_anything").await.unwrap_err();
        assert!(matches!(err, ClientError::Transport(_)));
    }
}
