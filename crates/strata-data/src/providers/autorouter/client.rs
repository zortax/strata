//! Authenticated REST client for `api.autorouter.aero`.
//!
//! Auth follows <https://www.autorouter.aero/wiki/api/authentication/>
//! exactly: `POST {base}/oauth2/token`, form-encoded, with
//! `grant_type=client_credentials` and the account email/password as
//! `client_id`/`client_secret`. The response carries `access_token` +
//! `expires_in` (documented one hour); subsequent calls send
//! `Authorization: Bearer <token>`. The token is fetched on first use and
//! cached until expiry minus a margin; the documented expiry signal is
//! HTTP 403 (401 handled too, being the OAuth2 standard) — either
//! invalidates the cache and the request is retried once with a fresh
//! token. Failed authentication surfaces the endpoint's documented
//! `error_description`. Credentials and tokens are never logged.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::sync::Mutex;
use tracing::debug;

use crate::Error;
use crate::domain::{IcaoCode, Notam};
use crate::providers::{NotamProvider, TimeWindow};

use super::PROVIDER;
use super::rows::{self, NotamResponse};

/// Page size — the documented maximum.
const PAGE_LIMIT: u32 = 100;
/// Refresh the token this long before its advertised expiry.
const TOKEN_EXPIRY_MARGIN: Duration = Duration::from_secs(60);

struct CachedToken {
    access_token: String,
    expires_at: Instant,
}

pub struct AutorouterClient {
    http: reqwest::Client,
    base_url: String,
    /// Account email (OAuth2 `client_id`).
    client_id: String,
    /// Account password (OAuth2 `client_secret`) — never logged.
    client_secret: String,
    token: Mutex<Option<CachedToken>>,
}

impl AutorouterClient {
    pub const DEFAULT_BASE_URL: &'static str = "https://api.autorouter.aero/v1.0";

    /// Credentials come from the config's `autorouter` section (the
    /// account email and password double as the OAuth2 client id/secret).
    /// This crate never reads the environment; the binaries load config.
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self::with_base_url(client_id, client_secret, Self::DEFAULT_BASE_URL)
    }

    /// Override the API root (fixture/local-server tests).
    pub fn with_base_url(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        // Timeouts are mandatory (reqwest defaults to none); failure to
        // build only occurs when the TLS backend cannot initialize.
        let http = reqwest::Client::builder()
            .user_agent(concat!("strata-data/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            token: Mutex::new(None),
        }
    }

    /// Returns a valid Bearer token, requesting a fresh one if the cache
    /// is empty or near expiry.
    async fn bearer_token(&self) -> Result<String, Error> {
        let mut slot = self.token.lock().await;
        if let Some(cached) = slot.as_ref()
            && cached.expires_at > Instant::now()
        {
            return Ok(cached.access_token.clone());
        }
        let fresh = self.request_token().await?;
        let access_token = fresh.access_token.clone();
        *slot = Some(fresh);
        Ok(access_token)
    }

    async fn request_token(&self) -> Result<CachedToken, Error> {
        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            expires_in: u64,
        }

        debug!(base_url = %self.base_url, "requesting autorouter oauth2 token");
        let response = self
            .http
            .post(format!("{}/oauth2/token", self.base_url))
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
            ])
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            // The documented error shape: {"error": "invalid_client",
            // "error_description": "The client credentials are invalid"}.
            // Surface the description — it is the user-facing explanation
            // the settings "Test connection" button shows.
            let body = response.text().await.unwrap_or_default();
            return Err(Error::provider(
                PROVIDER,
                format!(
                    "authentication failed: {}",
                    oauth_error_detail(status, &body)
                ),
            ));
        }
        let token: TokenResponse = response.json().await?;
        Ok(CachedToken {
            access_token: token.access_token,
            expires_at: Instant::now() + token_lifetime(token.expires_in),
        })
    }

    /// Authenticates and performs the documentation's example
    /// authenticated call (`GET {base}/aircraft/templates`, a small
    /// account-independent list) — the settings "Test connection" button.
    pub async fn test_connection(&self) -> Result<(), Error> {
        let token = self.bearer_token().await?;
        self.http
            .get(format!("{}/aircraft/templates", self.base_url))
            .bearer_auth(&token)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Fetches one page; on 401/403 (expired/revoked token) the cached
    /// token is dropped and the request retried once.
    async fn fetch_page(&self, params: &[(&'static str, String)]) -> Result<NotamResponse, Error> {
        let url = format!("{}/notam", self.base_url);
        for attempt in 0..2 {
            let token = self.bearer_token().await?;
            let response = self
                .http
                .get(&url)
                .bearer_auth(&token)
                .query(params)
                .send()
                .await?;
            let status = response.status();
            if attempt == 0
                && (status == reqwest::StatusCode::UNAUTHORIZED
                    || status == reqwest::StatusCode::FORBIDDEN)
            {
                debug!(%status, "autorouter token rejected, refreshing");
                *self.token.lock().await = None;
                continue;
            }
            let response = response.error_for_status()?;
            let page: NotamResponse = response.json().await?;
            return Ok(page);
        }
        unreachable!("loop returns on the second attempt");
    }

    /// Force the cached token past its expiry (the lifecycle tests'
    /// stand-in for waiting out the real one-hour lifetime).
    #[cfg(test)]
    async fn expire_cached_token(&self) {
        if let Some(cached) = self.token.lock().await.as_mut() {
            // Strictly-greater comparison in `bearer_token`: "expires
            // right now" is already expired.
            cached.expires_at = Instant::now();
        }
    }

    /// All NOTAMs whose item A is in `item_as`, validity-filtered
    /// server-side to `window`, paginated to completion.
    async fn fetch_notams(
        &self,
        item_as: &[IcaoCode],
        window: TimeWindow,
    ) -> Result<Vec<Notam>, Error> {
        if item_as.is_empty() {
            return Ok(Vec::new());
        }
        let mut notams = Vec::new();
        let mut offset: u64 = 0;
        loop {
            let params =
                page_params(item_as, window, offset).map_err(|e| Error::provider(PROVIDER, e))?;
            let page = self.fetch_page(&params).await?;
            let fetched = page.rows.len() as u64;
            notams.extend(rows::normalize(page.rows));
            offset += fetched;
            if fetched == 0 || offset >= page.total {
                break;
            }
        }
        debug!(count = notams.len(), "fetched autorouter NOTAMs");
        Ok(notams)
    }
}

/// How long a token with the advertised `expires_in` is cached: the
/// advertised lifetime minus [`TOKEN_EXPIRY_MARGIN`], floored at one
/// second so a pathological response still caches briefly instead of
/// hammering the token endpoint.
fn token_lifetime(expires_in: u64) -> Duration {
    Duration::from_secs(expires_in)
        .saturating_sub(TOKEN_EXPIRY_MARGIN)
        .max(Duration::from_secs(1))
}

/// The user-facing detail of a failed token request: the documented
/// `error_description` (fallback `error`) from the response body, else
/// the bare HTTP status. Never includes the credentials.
fn oauth_error_detail(status: reqwest::StatusCode, body: &str) -> String {
    #[derive(Deserialize)]
    struct OauthError {
        error: String,
        error_description: Option<String>,
    }
    match serde_json::from_str::<OauthError>(body) {
        Ok(e) => e.error_description.unwrap_or(e.error),
        Err(_) => format!("HTTP {status}"),
    }
}

/// Query parameters for one page: `itemas` is a JSON-encoded ICAO list,
/// the validity bounds are epoch seconds (clamped to the API's u32 range).
fn page_params(
    item_as: &[IcaoCode],
    window: TimeWindow,
    offset: u64,
) -> Result<Vec<(&'static str, String)>, serde_json::Error> {
    let codes: Vec<&str> = item_as.iter().map(IcaoCode::as_str).collect();
    let clamp = |t: DateTime<Utc>| t.timestamp().clamp(0, i64::from(u32::MAX));
    Ok(vec![
        ("itemas", serde_json::to_string(&codes)?),
        ("startvalidity", clamp(window.from).to_string()),
        ("endvalidity", clamp(window.to).to_string()),
        ("offset", offset.to_string()),
        ("limit", PAGE_LIMIT.to_string()),
    ])
}

#[async_trait]
impl NotamProvider for AutorouterClient {
    async fn notams_by_locations(
        &self,
        locations: &[IcaoCode],
        window: TimeWindow,
    ) -> Result<Vec<Notam>, Error> {
        self.fetch_notams(locations, window).await
    }

    async fn notams_by_fir(&self, fir: &IcaoCode, window: TimeWindow) -> Result<Vec<Notam>, Error> {
        // FIR-wide NOTAMs are filed with the FIR itself as item A.
        self.fetch_notams(std::slice::from_ref(fir), window).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn icao(code: &str) -> IcaoCode {
        IcaoCode::new(code).expect("valid test ICAO code")
    }

    pub(super) fn window() -> TimeWindow {
        TimeWindow {
            from: DateTime::<Utc>::from_timestamp(1_750_000_000, 0).expect("valid"),
            to: DateTime::<Utc>::from_timestamp(1_750_086_400, 0).expect("valid"),
        }
    }

    #[test]
    fn page_params_encode_itemas_as_json_and_window_as_epochs() {
        let params =
            page_params(&[icao("EDDF"), icao("EDDM")], window(), 200).expect("params build");
        assert_eq!(
            params,
            vec![
                ("itemas", "[\"EDDF\",\"EDDM\"]".to_owned()),
                ("startvalidity", "1750000000".to_owned()),
                ("endvalidity", "1750086400".to_owned()),
                ("offset", "200".to_owned()),
                ("limit", "100".to_owned()),
            ]
        );
    }

    #[test]
    fn page_params_clamp_pre_epoch_and_far_future_windows() {
        let window = TimeWindow {
            from: DateTime::<Utc>::from_timestamp(-100, 0).expect("valid"),
            to: DateTime::<Utc>::from_timestamp(i64::from(u32::MAX) + 5, 0).expect("valid"),
        };
        let params = page_params(&[icao("EDGG")], window, 0).expect("params build");
        assert_eq!(params[1], ("startvalidity", "0".to_owned()));
        assert_eq!(params[2], ("endvalidity", u32::MAX.to_string()));
    }

    #[test]
    fn base_url_trailing_slash_is_trimmed() {
        let client = AutorouterClient::with_base_url(
            "user@example.com",
            "secret",
            "http://localhost:1/v1.0/",
        );
        assert_eq!(client.base_url, "http://localhost:1/v1.0");
    }

    #[tokio::test]
    async fn empty_location_list_short_circuits_without_io() {
        // Unroutable base URL: any actual request would error, so an Ok
        // result proves the early return.
        let client =
            AutorouterClient::with_base_url("user@example.com", "secret", "http://localhost:1");
        let notams = client
            .notams_by_locations(&[], window())
            .await
            .expect("no request issued");
        assert!(notams.is_empty());
    }

    #[test]
    fn token_lifetime_applies_the_margin_with_a_one_second_floor() {
        // The documented one-hour token refreshes a minute early.
        assert_eq!(token_lifetime(3600), Duration::from_secs(3540));
        // Lifetimes at/under the margin still cache briefly.
        assert_eq!(token_lifetime(60), Duration::from_secs(1));
        assert_eq!(token_lifetime(0), Duration::from_secs(1));
    }

    #[test]
    fn oauth_error_detail_prefers_the_documented_description() {
        let status = reqwest::StatusCode::BAD_REQUEST;
        assert_eq!(
            oauth_error_detail(
                status,
                r#"{"error":"invalid_client","error_description":"The client credentials are invalid"}"#,
            ),
            "The client credentials are invalid"
        );
        assert_eq!(
            oauth_error_detail(status, r#"{"error":"invalid_client"}"#),
            "invalid_client"
        );
        assert_eq!(
            oauth_error_detail(status, "<html>gateway error</html>"),
            "HTTP 400 Bad Request"
        );
    }
}

/// Token-lifecycle tests against a local mock HTTP server (a hand-rolled
/// `tokio::net::TcpListener` loop — the lightest thing that satisfies
/// reqwest; no live autorouter calls anywhere).
#[cfg(test)]
mod lifecycle_tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::tests::{icao, window};
    use super::*;

    const EMAIL: &str = "pilot@example.com";
    const PASSWORD: &str = "hunter2-super-secret";

    /// One request as the mock server saw it.
    #[derive(Debug, Clone)]
    struct Recorded {
        method: String,
        /// Path + query, as sent.
        target: String,
        authorization: Option<String>,
        body: String,
    }

    /// Serves a fixed queue of `(status, body)` responses, one per
    /// request in order, recording every request. Connections are closed
    /// after each response (`connection: close`), so reqwest reconnects —
    /// fine for these serial tests.
    struct MockServer {
        base_url: String,
        requests: Arc<Mutex<Vec<Recorded>>>,
    }

    impl MockServer {
        async fn start(responses: Vec<(u16, &str)>) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind mock server");
            let addr = listener.local_addr().expect("local addr");
            let requests = Arc::new(Mutex::new(Vec::new()));
            let queue: VecDeque<(u16, String)> = responses
                .into_iter()
                .map(|(status, body)| (status, body.to_owned()))
                .collect();
            let queue = Arc::new(Mutex::new(queue));
            let recorded = Arc::clone(&requests);
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    serve_one(stream, &recorded, &queue).await;
                }
            });
            Self {
                base_url: format!("http://{addr}"),
                requests,
            }
        }

        fn client(&self) -> AutorouterClient {
            AutorouterClient::with_base_url(EMAIL, PASSWORD, &self.base_url)
        }

        fn requests(&self) -> Vec<Recorded> {
            self.requests.lock().expect("requests lock").clone()
        }

        /// The recorded requests hitting the token endpoint.
        fn token_requests(&self) -> Vec<Recorded> {
            self.requests()
                .into_iter()
                .filter(|r| r.target == "/oauth2/token")
                .collect()
        }
    }

    /// Reads one HTTP/1.1 request (head + content-length body), records
    /// it, answers with the next canned response, closes the connection.
    async fn serve_one(
        mut stream: tokio::net::TcpStream,
        recorded: &Mutex<Vec<Recorded>>,
        queue: &Mutex<VecDeque<(u16, String)>>,
    ) {
        let mut raw = Vec::new();
        let mut buf = [0u8; 1024];
        let (head, body_start) = loop {
            let Ok(n) = stream.read(&mut buf).await else {
                return;
            };
            if n == 0 {
                return;
            }
            raw.extend_from_slice(&buf[..n]);
            if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                break (String::from_utf8_lossy(&raw[..end]).into_owned(), end + 4);
            }
        };
        let mut lines = head.lines();
        let request_line = lines.next().unwrap_or_default();
        let mut parts = request_line.split(' ');
        let method = parts.next().unwrap_or_default().to_owned();
        let target = parts.next().unwrap_or_default().to_owned();
        let mut authorization = None;
        let mut content_length = 0usize;
        for line in lines {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            match name.to_ascii_lowercase().as_str() {
                "authorization" => authorization = Some(value.trim().to_owned()),
                "content-length" => content_length = value.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
        let mut body = raw[body_start..].to_vec();
        while body.len() < content_length {
            let Ok(n) = stream.read(&mut buf).await else {
                return;
            };
            if n == 0 {
                break;
            }
            body.extend_from_slice(&buf[..n]);
        }
        recorded.lock().expect("recorded lock").push(Recorded {
            method,
            target,
            authorization,
            body: String::from_utf8_lossy(&body).into_owned(),
        });
        let (status, payload) = queue.lock().expect("queue lock").pop_front().unwrap_or((
            500,
            r#"{"error":"mock response queue exhausted"}"#.to_owned(),
        ));
        let response = format!(
            "HTTP/1.1 {status} Mock\r\ncontent-type: application/json\r\n\
             content-length: {}\r\nconnection: close\r\n\r\n{payload}",
            payload.len(),
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
    }

    const TOKEN_1: &str =
        r#"{"access_token":"tok-one","expires_in":3600,"token_type":"Bearer","scope":null}"#;
    const TOKEN_2: &str =
        r#"{"access_token":"tok-two","expires_in":3600,"token_type":"Bearer","scope":null}"#;
    const EMPTY_PAGE: &str = r#"{"total":0,"rows":[]}"#;

    /// First use fetches a token with the documented form fields; the
    /// cached token then serves every request until expiry.
    #[tokio::test]
    async fn token_is_fetched_on_first_use_and_cached() {
        let server =
            MockServer::start(vec![(200, TOKEN_1), (200, EMPTY_PAGE), (200, EMPTY_PAGE)]).await;
        let client = server.client();
        for _ in 0..2 {
            client
                .notams_by_locations(&[icao("EDDF")], window())
                .await
                .expect("fetch succeeds");
        }

        let requests = server.requests();
        assert_eq!(
            requests
                .iter()
                .map(|r| r.target.as_str())
                .filter(|t| *t == "/oauth2/token")
                .count(),
            1,
            "one token request serves both fetches: {requests:#?}"
        );
        // The documented token request: form-encoded client-credentials
        // grant with the account email/password as id/secret.
        let token = &requests[0];
        assert_eq!(token.method, "POST");
        assert!(
            token.body.contains("grant_type=client_credentials"),
            "{}",
            token.body
        );
        assert!(
            token.body.contains("client_id=pilot%40example.com"),
            "{}",
            token.body
        );
        assert!(
            token.body.contains("client_secret=hunter2-super-secret"),
            "{}",
            token.body
        );
        // The documented usage: Authorization: Bearer <token> on each call.
        for notam in requests.iter().filter(|r| r.target.starts_with("/notam")) {
            assert_eq!(notam.method, "GET");
            assert_eq!(notam.authorization.as_deref(), Some("Bearer tok-one"));
        }
    }

    /// An expired cache re-authenticates before the next request.
    #[tokio::test]
    async fn expired_token_is_refreshed_before_the_next_request() {
        let server = MockServer::start(vec![
            (200, TOKEN_1),
            (200, EMPTY_PAGE),
            (200, TOKEN_2),
            (200, EMPTY_PAGE),
        ])
        .await;
        let client = server.client();
        client
            .notams_by_locations(&[icao("EDDF")], window())
            .await
            .expect("first fetch");
        client.expire_cached_token().await;
        client
            .notams_by_locations(&[icao("EDDF")], window())
            .await
            .expect("second fetch");

        assert_eq!(server.token_requests().len(), 2, "expiry forced a re-auth");
        let last = server.requests().last().expect("requests recorded").clone();
        assert_eq!(last.authorization.as_deref(), Some("Bearer tok-two"));
    }

    /// The documented expiry signal: a rejected token (the wiki names
    /// 403; 401 is handled identically) is dropped, a fresh one fetched,
    /// and the request retried once — transparently to the caller.
    #[tokio::test]
    async fn rejected_token_reauthenticates_and_retries_once() {
        for rejection in [401, 403] {
            let server = MockServer::start(vec![
                (200, TOKEN_1),
                (rejection, r#"{"error":"invalid_grant"}"#),
                (200, TOKEN_2),
                (200, EMPTY_PAGE),
            ])
            .await;
            let client = server.client();
            client
                .notams_by_locations(&[icao("EDDF")], window())
                .await
                .expect("retry with a fresh token succeeds");

            let requests = server.requests();
            assert_eq!(
                server.token_requests().len(),
                2,
                "{rejection}: re-authenticated"
            );
            assert_eq!(
                requests.last().expect("requests").authorization.as_deref(),
                Some("Bearer tok-two"),
                "{rejection}: retried with the fresh token"
            );
        }
    }

    /// One retry only — a second rejection fails the fetch instead of
    /// looping on the token endpoint.
    #[tokio::test]
    async fn a_second_rejection_fails_the_fetch() {
        let server = MockServer::start(vec![
            (200, TOKEN_1),
            (401, r#"{"error":"invalid_grant"}"#),
            (200, TOKEN_2),
            (401, r#"{"error":"invalid_grant"}"#),
        ])
        .await;
        let error = server
            .client()
            .notams_by_locations(&[icao("EDDF")], window())
            .await
            .expect_err("second rejection surfaces");
        assert!(error.to_string().contains("401"), "{error}");
        assert_eq!(server.token_requests().len(), 2, "exactly one re-auth");
    }

    /// Failed authentication surfaces the endpoint's documented
    /// `error_description` — and never the credentials.
    #[tokio::test]
    async fn invalid_credentials_surface_the_documented_description() {
        let server = MockServer::start(vec![(
            400,
            r#"{"error":"invalid_client","error_description":"The client credentials are invalid"}"#,
        )])
        .await;
        let error = server
            .client()
            .notams_by_locations(&[icao("EDDF")], window())
            .await
            .expect_err("authentication fails");
        let message = error.to_string();
        assert!(
            message.contains("The client credentials are invalid"),
            "{message}"
        );
        assert!(!message.contains(PASSWORD), "{message}");
        assert!(!message.contains(EMAIL), "{message}");
    }

    /// `test_connection` = auth + the documented cheap authenticated call.
    #[tokio::test]
    async fn test_connection_authenticates_and_calls_aircraft_templates() {
        let server = MockServer::start(vec![(200, TOKEN_1), (200, "[]")]).await;
        server
            .client()
            .test_connection()
            .await
            .expect("connection ok");
        let requests = server.requests();
        assert_eq!(requests[0].target, "/oauth2/token");
        assert_eq!(requests[1].target, "/aircraft/templates");
        assert_eq!(requests[1].authorization.as_deref(), Some("Bearer tok-one"));

        let failing = MockServer::start(vec![(200, TOKEN_1), (500, "{}")]).await;
        failing
            .client()
            .test_connection()
            .await
            .expect_err("server error surfaces");
    }

    // The never-logged-secrets assertion lives in its own integration
    // test (`tests/autorouter_never_logs_secrets.rs`): capturing a
    // tracing subscriber is only deterministic when no parallel test in
    // the same binary races the global callsite-interest cache.
}
