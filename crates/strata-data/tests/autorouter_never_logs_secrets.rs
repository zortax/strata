//! The autorouter client never logs credentials or tokens.
//!
//! Lives alone in its own integration-test binary on purpose: the
//! capturing subscriber relies on tracing's global callsite-interest
//! cache, which parallel tests in the same process race (callsites first
//! hit with no subscriber cache as never-interested). One test, one
//! process, deterministic capture.
//!
//! The exercised flow is the full token lifecycle against a local mock
//! server — first-use token fetch, a 401 rejection, the re-auth, the
//! retried page fetch — i.e. every log site the client has.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use strata_data::providers::autorouter::AutorouterClient;
use strata_data::providers::{NotamProvider as _, TimeWindow};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const EMAIL: &str = "pilot@example.com";
const PASSWORD: &str = "hunter2-super-secret";

/// Minimal canned-response HTTP server (the lib tests' mock, reduced to
/// what this assertion needs — no request recording).
async fn start_mock(responses: Vec<(u16, &str)>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server");
    let addr = listener.local_addr().expect("local addr");
    let queue: Arc<Mutex<VecDeque<(u16, String)>>> = Arc::new(Mutex::new(
        responses
            .into_iter()
            .map(|(status, body)| (status, body.to_owned()))
            .collect(),
    ));
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            // Read one request: head, then content-length body bytes.
            let mut raw = Vec::new();
            let mut buf = [0u8; 1024];
            let body_start = loop {
                let Ok(n) = stream.read(&mut buf).await else {
                    break 0;
                };
                if n == 0 {
                    break 0;
                }
                raw.extend_from_slice(&buf[..n]);
                if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            if body_start == 0 {
                continue;
            }
            let head = String::from_utf8_lossy(&raw[..body_start]).into_owned();
            let content_length: usize = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().ok())?
                })
                .unwrap_or(0);
            let mut body_len = raw.len() - body_start;
            while body_len < content_length {
                let Ok(n) = stream.read(&mut buf).await else {
                    break;
                };
                if n == 0 {
                    break;
                }
                body_len += n;
            }
            let (status, payload) = queue
                .lock()
                .expect("queue lock")
                .pop_front()
                .unwrap_or((500, r#"{"error":"mock queue exhausted"}"#.to_owned()));
            let response = format!(
                "HTTP/1.1 {status} Mock\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{payload}",
                payload.len(),
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn credentials_and_tokens_never_reach_the_logs() {
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<String>>);
    impl std::io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("capture lock")
                .push_str(&String::from_utf8_lossy(buf));
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let capture = Capture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::level_filters::LevelFilter::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let server = start_mock(vec![
        (
            200,
            r#"{"access_token":"tok-one","expires_in":3600,"token_type":"Bearer","scope":null}"#,
        ),
        (401, r#"{"error":"invalid_grant"}"#),
        (
            200,
            r#"{"access_token":"tok-two","expires_in":3600,"token_type":"Bearer","scope":null}"#,
        ),
        (200, r#"{"total":0,"rows":[]}"#),
    ])
    .await;

    let client = AutorouterClient::with_base_url(EMAIL, PASSWORD, server);
    let window = TimeWindow {
        from: chrono::DateTime::from_timestamp(1_750_000_000, 0).expect("valid"),
        to: chrono::DateTime::from_timestamp(1_750_086_400, 0).expect("valid"),
    };
    let eddf = strata_data::domain::IcaoCode::new("EDDF").expect("valid");
    client
        .notams_by_locations(&[eddf], window)
        .await
        .expect("fetch succeeds after the 401 re-auth");

    let logged = capture.0.lock().expect("capture lock").clone();
    // Not vacuous: the client's own events demonstrably reached the
    // capture — the token request and the rejection re-auth both logged.
    assert!(logged.contains("requesting autorouter oauth2 token"), "{logged}");
    assert!(logged.contains("autorouter token rejected"), "{logged}");
    // The actual property: neither credential nor any token ever appears.
    for secret in [PASSWORD, EMAIL, "tok-one", "tok-two"] {
        assert!(!logged.contains(secret), "{secret:?} leaked into logs:\n{logged}");
    }
}
