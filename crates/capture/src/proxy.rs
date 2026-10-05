//! The local capture proxy.
//!
//! Claude Code is pointed at `http://127.0.0.1:47821` with
//! `ANTHROPIC_BASE_URL`. Every request is forwarded to the fixed upstream with
//! its headers and body unchanged (except hop-by-hop headers), the response is
//! streamed back chunk by chunk as it arrives, and a copy of the exchange is
//! handed to a [`CaptureSink`] once the response has ended.

use bytes::Bytes;
use capture_core::{
    Body, CaptureRecord, CapturedRequest, CapturedResponse, filter_headers, rebuild_message,
};
use http_body_util::channel::{Channel, Sender};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{self, HeaderMap, HeaderName, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;

/// The local port the proxy listens on.
pub const PROXY_PORT: u16 = 47821;

/// The only upstream the proxy forwards to in the app.
pub const UPSTREAM: &str = "https://api.anthropic.com";

/// Body text of the answer sent when the upstream cannot be reached.
const UNREACHABLE: &str = "snitchcraft proxy: upstream unreachable";

/// How long to wait for a connection to the upstream before giving up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How many response chunks may wait for a slow client before the proxy
/// pauses reading from the upstream.
const CHANNEL_CAPACITY: usize = 32;

/// Headers that describe one connection, not the message. They are not
/// forwarded in either direction, except that an upstream `content-length`
/// is passed back to the client.
const HOP_BY_HOP: [&str; 10] = [
    "host",
    "connection",
    "content-length",
    "transfer-encoding",
    "keep-alive",
    "upgrade",
    "proxy-connection",
    "te",
    "trailer",
    "accept-encoding",
];

/// Process-wide counter that makes record ids unique.
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// Receives each finished exchange. Called on a blocking thread, so it may do
/// slow work such as writing to disk without stalling the proxy.
pub trait CaptureSink: Send + Sync + 'static {
    /// Take one finished record.
    fn record(&self, record: CaptureRecord);
}

/// Why the proxy could not start.
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    /// The port could not be bound, usually because another program uses it.
    #[error("could not listen on 127.0.0.1:{port}: {source}")]
    Bind {
        /// The port that was asked for.
        port: u16,
        /// The error from the operating system.
        source: std::io::Error,
    },
    /// The HTTPS client for the upstream could not be built.
    #[error("could not set up the HTTPS client: {0}")]
    Client(String),
}

/// A bound proxy, ready to serve.
pub struct Proxy {
    listener: TcpListener,
    local_addr: SocketAddr,
    shared: Arc<Shared>,
}

/// What every request handler needs.
struct Shared {
    client: reqwest::Client,
    upstream: String,
    sink: Arc<dyn CaptureSink>,
}

type ProxyBody = BoxBody<Bytes, std::io::Error>;

impl Proxy {
    /// Binds 127.0.0.1:PROXY_PORT and prepares the client for UPSTREAM.
    pub async fn bind(sink: Arc<dyn CaptureSink>) -> Result<Proxy, ProxyError> {
        let addr = SocketAddr::from(([127, 0, 0, 1], PROXY_PORT));
        Proxy::bind_with(addr, UPSTREAM.to_string(), sink).await
    }

    /// Bind `addr` and forward to `upstream`. The app only reaches this
    /// through [`Proxy::bind`]; tests use it with a local fake upstream.
    async fn bind_with(
        addr: SocketAddr,
        upstream: String,
        sink: Arc<dyn CaptureSink>,
    ) -> Result<Proxy, ProxyError> {
        install_crypto_provider();
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|e| ProxyError::Client(error_chain(&e)))?;
        let bind_error = |source| ProxyError::Bind {
            port: addr.port(),
            source,
        };
        let listener = TcpListener::bind(addr).await.map_err(bind_error)?;
        let local_addr = listener.local_addr().map_err(bind_error)?;
        Ok(Proxy {
            listener,
            local_addr,
            shared: Arc::new(Shared {
                client,
                upstream: upstream.trim_end_matches('/').to_string(),
                sink,
            }),
        })
    }

    /// The address the proxy listens on.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Serves until the task is dropped. Accept errors are logged and the loop continues.
    pub async fn serve(self) {
        tracing::info!(addr = %self.local_addr, "capture proxy listening");
        loop {
            let stream = match self.listener.accept().await {
                Ok((stream, _)) => stream,
                Err(error) => {
                    tracing::warn!(%error, "capture proxy could not accept a connection");
                    // Avoid a busy loop if the error repeats.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let shared = self.shared.clone();
            tokio::spawn(async move {
                let service = service_fn(move |req| handle(req, shared.clone()));
                if let Err(error) = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await
                {
                    tracing::debug!(%error, "capture proxy connection ended with an error");
                }
            });
        }
    }
}

/// Install rustls's `ring` crypto provider for the process. reqwest is built
/// without a provider of its own and uses the installed one. Does nothing if
/// a provider is already installed.
fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn is_hop_by_hop(name: &HeaderName) -> bool {
    HOP_BY_HOP.contains(&name.as_str())
}

/// Headers as they are stored: through [`filter_headers`], so credentials
/// are dropped. Values that are not valid text are kept as lossy UTF-8.
fn filtered(headers: &HeaderMap) -> Vec<capture_core::Header> {
    let pairs: Vec<(&str, String)> = headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    filter_headers(pairs.iter().map(|(n, v)| (*n, v.as_str())))
}

fn plain_response(status: StatusCode, text: &'static str) -> Response<ProxyBody> {
    let body = Full::new(Bytes::from_static(text.as_bytes()))
        .map_err(|never| match never {})
        .boxed();
    let mut response = Response::new(body);
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

/// Hand a record to the sink on a blocking thread, so a slow disk never
/// stalls the proxy.
fn deliver(shared: &Shared, record: CaptureRecord) {
    let sink = shared.sink.clone();
    tokio::task::spawn_blocking(move || sink.record(record));
}

/// Finish a record for an exchange that got no upstream response.
fn fail(shared: &Shared, mut record: CaptureRecord, error: String) {
    record.error = Some(error);
    record.ended_at_ms = Some(now_ms());
    deliver(shared, record);
}

/// Forward one request and stream its response back.
async fn handle(
    req: Request<Incoming>,
    shared: Arc<Shared>,
) -> Result<Response<ProxyBody>, Infallible> {
    let started_at_ms = now_ms();
    let id = format!(
        "{started_at_ms}-{}",
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    );
    let (parts, incoming) = req.into_parts();
    let path = parts
        .uri
        .path_and_query()
        .map_or_else(|| parts.uri.path().to_string(), |p| p.as_str().to_string());

    let mut record = CaptureRecord {
        id,
        started_at_ms,
        first_byte_at_ms: None,
        ended_at_ms: None,
        request: CapturedRequest {
            method: parts.method.to_string(),
            path: path.clone(),
            headers: filtered(&parts.headers),
            body: Body::Empty,
        },
        response: None,
        message_id: None,
        error: None,
    };

    let body_bytes = match incoming.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            fail(
                &shared,
                record,
                format!("could not read the request body: {error}"),
            );
            return Ok(plain_response(
                StatusCode::BAD_REQUEST,
                "snitchcraft proxy: could not read the request body",
            ));
        }
    };
    record.request.body = Body::from_bytes(&body_bytes);

    // The forwarded request keeps every header, credentials included, except
    // the hop-by-hop ones. The upstream is asked not to compress, so the
    // stream can be recorded as text.
    let mut forward_headers = HeaderMap::new();
    for (name, value) in &parts.headers {
        if !is_hop_by_hop(name) {
            forward_headers.append(name.clone(), value.clone());
        }
    }
    forward_headers.insert(
        header::ACCEPT_ENCODING,
        HeaderValue::from_static("identity"),
    );

    let is_head = parts.method == Method::HEAD;
    let mut upstream_req = shared
        .client
        .request(parts.method.clone(), format!("{}{path}", shared.upstream))
        .headers(forward_headers);
    if parts.method != Method::GET && !is_head {
        upstream_req = upstream_req.body(body_bytes);
    }

    let upstream_res = match upstream_req.send().await {
        Ok(res) => res,
        Err(error) => {
            let text = error_chain(&error);
            tracing::warn!(error = %text, "capture proxy could not reach the upstream");
            fail(&shared, record, format!("upstream unreachable: {text}"));
            return Ok(plain_response(StatusCode::BAD_GATEWAY, UNREACHABLE));
        }
    };

    let status = upstream_res.status();
    let upstream_headers = upstream_res.headers().clone();
    let mut builder = Response::builder().status(status);
    if let Some(headers) = builder.headers_mut() {
        for (name, value) in &upstream_headers {
            if !is_hop_by_hop(name) || name == header::CONTENT_LENGTH {
                headers.append(name.clone(), value.clone());
            }
        }
    }
    let (tx, channel) = Channel::<Bytes, std::io::Error>::new(CHANNEL_CAPACITY);
    let response = match builder.body(channel.boxed()) {
        Ok(response) => response,
        Err(error) => {
            fail(
                &shared,
                record,
                format!("could not build the response: {error}"),
            );
            return Ok(plain_response(StatusCode::BAD_GATEWAY, UNREACHABLE));
        }
    };

    record.response = Some(CapturedResponse {
        status: status.as_u16(),
        headers: filtered(&upstream_headers),
        stream: None,
        message: None,
    });
    let content_type = upstream_headers
        .get(header::CONTENT_TYPE)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
    tokio::spawn(pump(PumpJob {
        upstream_res,
        tx,
        record,
        content_type,
        is_head,
        shared,
    }));
    Ok(response)
}

/// Everything the task that copies a response body needs.
struct PumpJob {
    upstream_res: reqwest::Response,
    tx: Sender<Bytes, std::io::Error>,
    record: CaptureRecord,
    content_type: Option<String>,
    is_head: bool,
    shared: Arc<Shared>,
}

/// Copy the upstream body to the client chunk by chunk as it arrives,
/// keeping a copy, then finish the record and hand it to the sink.
async fn pump(job: PumpJob) {
    let PumpJob {
        mut upstream_res,
        tx,
        mut record,
        content_type,
        is_head,
        shared,
    } = job;
    let mut tx = Some(tx);
    let mut buffer: Vec<u8> = Vec::new();
    let mut read_error = None;
    loop {
        match upstream_res.chunk().await {
            Ok(Some(chunk)) => {
                record.first_byte_at_ms.get_or_insert_with(now_ms);
                buffer.extend_from_slice(&chunk);
                // If the client went away, keep reading so the record is complete.
                if let Some(sender) = tx.as_mut()
                    && sender.send_data(chunk).await.is_err()
                {
                    tx = None;
                }
            }
            Ok(None) => break,
            Err(error) => {
                let text = error_chain(&error);
                tracing::warn!(error = %text, "capture proxy lost the upstream response");
                read_error = Some(format!("upstream response failed: {text}"));
                // Tell the client the body is incomplete instead of ending it cleanly.
                if let Some(sender) = tx.take() {
                    sender.abort(std::io::Error::other("upstream response failed"));
                }
                break;
            }
        }
    }
    // Ends the client body.
    drop(tx);

    let text = String::from_utf8_lossy(&buffer).into_owned();
    let rebuilt = rebuild_message(content_type.as_deref(), &text);
    if let Some(response) = record.response.as_mut() {
        response.stream = (!is_head && !buffer.is_empty()).then_some(text);
        response.message = rebuilt.message;
    }
    record.message_id = rebuilt.message_id;
    record.error = read_error.or(rebuilt.error);
    record.ended_at_ms = Some(now_ms());
    deliver(&shared, record);
}

/// An error and its sources as one line.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(inner) = source {
        text.push_str(": ");
        text.push_str(&inner.to_string());
        source = inner.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full, channel::Channel};
    use hyper::body::Incoming;
    use hyper::header::HeaderMap;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use tokio::net::TcpListener;
    use tokio::sync::Notify;

    /// The invented stream from the capture-core tests, split in three pieces.
    const SSE_PARTS: [&str; 3] = [
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_test1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"test-model\",\"content\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello \"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"there\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":5}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ];

    /// Gap between the pieces of the fake stream.
    const GAP: Duration = Duration::from_millis(150);

    const ERROR_BODY: &str =
        r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;

    /// What the fake upstream received.
    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        path_and_query: String,
        headers: HeaderMap,
        body: Bytes,
    }

    type Shared = Arc<Mutex<Vec<Seen>>>;
    type TestBody = http_body_util::combinators::BoxBody<Bytes, std::io::Error>;

    fn full(text: &'static str) -> TestBody {
        Full::new(Bytes::from_static(text.as_bytes()))
            .map_err(|never| match never {})
            .boxed()
    }

    async fn fake_handler(
        req: Request<Incoming>,
        seen: Shared,
    ) -> Result<Response<TestBody>, String> {
        let method = req.method().to_string();
        let path = req.uri().path().to_string();
        let path_and_query = req
            .uri()
            .path_and_query()
            .map(|p| p.as_str().to_string())
            .unwrap_or_default();
        let headers = req.headers().clone();
        let body = req
            .into_body()
            .collect()
            .await
            .map_err(|e| e.to_string())?
            .to_bytes();
        seen.lock().map_err(|e| e.to_string())?.push(Seen {
            method: method.clone(),
            path_and_query,
            headers,
            body,
        });
        let response = match (method.as_str(), path.as_str()) {
            (_, "/stream") => {
                let (mut tx, body) = Channel::<Bytes, std::io::Error>::new(4);
                tokio::spawn(async move {
                    for (i, part) in SSE_PARTS.iter().enumerate() {
                        if i > 0 {
                            tokio::time::sleep(GAP).await;
                        }
                        if tx
                            .send_data(Bytes::from_static(part.as_bytes()))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                });
                Response::builder()
                    .status(200)
                    .header("content-type", "text/event-stream")
                    .header("request-id", "req_test")
                    .body(body.boxed())
            }
            ("HEAD", "/api/hello") => Response::builder().status(200).body(full("")),
            (_, "/error") => Response::builder()
                .status(529)
                .header("content-type", "application/json")
                .body(full(ERROR_BODY)),
            _ => Response::builder().status(404).body(full("not found")),
        };
        response.map_err(|e| e.to_string())
    }

    /// Start a fake upstream on a free local port. Returns its base URL and what it saw.
    async fn fake_upstream() -> (String, Shared) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fake");
        let addr = listener.local_addr().expect("fake addr");
        let seen: Shared = Arc::new(Mutex::new(Vec::new()));
        let seen_for_task = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let seen = seen_for_task.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |req| fake_handler(req, seen.clone()));
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        (format!("http://{addr}"), seen)
    }

    #[derive(Default)]
    struct CollectingSink {
        records: Mutex<Vec<CaptureRecord>>,
        notify: Notify,
    }

    impl CaptureSink for CollectingSink {
        fn record(&self, record: CaptureRecord) {
            if let Ok(mut records) = self.records.lock() {
                records.push(record);
            }
            self.notify.notify_one();
        }
    }

    impl CollectingSink {
        /// Wait until `n` records arrived, or fail after a few seconds.
        async fn wait_for(&self, n: usize) -> Vec<CaptureRecord> {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                {
                    let records = self.records.lock().expect("lock");
                    if records.len() >= n {
                        return records.clone();
                    }
                }
                let left = deadline.saturating_duration_since(Instant::now());
                assert!(!left.is_zero(), "timed out waiting for {n} record(s)");
                let _ = tokio::time::timeout(left, self.notify.notified()).await;
            }
        }
    }

    /// Start a proxy in front of `upstream`. Returns its base URL and the sink.
    async fn start_proxy(upstream: String) -> (String, Arc<CollectingSink>) {
        let sink = Arc::new(CollectingSink::default());
        let addr: SocketAddr = "127.0.0.1:0".parse().expect("addr");
        let proxy = Proxy::bind_with(addr, upstream, sink.clone())
            .await
            .expect("bind proxy");
        let url = format!("http://{}", proxy.local_addr());
        tokio::spawn(proxy.serve());
        (url, sink)
    }

    fn client() -> reqwest::Client {
        install_crypto_provider();
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test client")
    }

    fn sse_text() -> String {
        SSE_PARTS.concat()
    }

    async fn post_stream(proxy: &str) -> reqwest::Response {
        client()
            .post(format!("{proxy}/stream?beta=true"))
            .header("authorization", "Bearer test-secret")
            .header("x-claude-code-session-id", "s1")
            .header("anthropic-beta", "b1")
            .header("accept-encoding", "gzip")
            .header("content-type", "application/json")
            .body(r#"{"model":"m","messages":[]}"#)
            .send()
            .await
            .expect("send through proxy")
    }

    #[tokio::test]
    async fn request_is_forwarded_unchanged_except_hop_headers() {
        let (upstream, seen) = fake_upstream().await;
        let (proxy, sink) = start_proxy(upstream).await;

        let response = post_stream(&proxy).await;
        assert_eq!(response.status(), 200);
        response.bytes().await.expect("body");
        sink.wait_for(1).await;

        let seen = seen.lock().expect("lock").clone();
        assert_eq!(seen.len(), 1);
        let got = &seen[0];
        assert_eq!(got.method, "POST");
        assert_eq!(got.path_and_query, "/stream?beta=true");
        assert_eq!(&got.body[..], br#"{"model":"m","messages":[]}"#);
        let header = |name: &str| got.headers.get(name).and_then(|v| v.to_str().ok());
        assert_eq!(header("authorization"), Some("Bearer test-secret"));
        assert_eq!(header("x-claude-code-session-id"), Some("s1"));
        assert_eq!(header("anthropic-beta"), Some("b1"));
        assert_eq!(header("content-type"), Some("application/json"));
        let encodings: Vec<_> = got.headers.get_all("accept-encoding").iter().collect();
        assert_eq!(encodings, ["identity"]);
        // The host is the upstream's, not the proxy's.
        let host = header("host").expect("host");
        assert!(
            !proxy.ends_with(host),
            "host {host} was forwarded from the client"
        );
    }

    #[tokio::test]
    async fn streamed_response_arrives_in_pieces_and_unchanged() {
        let (upstream, _seen) = fake_upstream().await;
        let (proxy, _sink) = start_proxy(upstream).await;

        let sent = Instant::now();
        let mut response = post_stream(&proxy).await;
        let mut body = Vec::new();
        let mut first_at = None;
        while let Some(chunk) = response.chunk().await.expect("chunk") {
            first_at.get_or_insert_with(|| sent.elapsed());
            body.extend_from_slice(&chunk);
        }
        let ended_at = sent.elapsed();
        let first_at = first_at.expect("at least one chunk");

        // The upstream needs 2 x 150 ms to finish. The first piece must arrive
        // before that, and well before the end of the body.
        assert!(
            first_at < GAP * 2,
            "first chunk took {first_at:?}, so the proxy buffered"
        );
        assert!(
            ended_at - first_at >= GAP,
            "first chunk at {first_at:?} but end at {ended_at:?}, so the proxy buffered"
        );
        assert_eq!(String::from_utf8(body).expect("utf8"), sse_text());
    }

    #[tokio::test]
    async fn record_has_rebuilt_message_and_timing() {
        let (upstream, _seen) = fake_upstream().await;
        let (proxy, sink) = start_proxy(upstream).await;

        let response = post_stream(&proxy).await;
        response.bytes().await.expect("body");
        let records = sink.wait_for(1).await;
        assert_eq!(records.len(), 1);
        let r = &records[0];

        assert_eq!(r.message_id.as_deref(), Some("msg_test1"));
        assert_eq!(r.error, None);
        assert_eq!(r.request.method, "POST");
        assert_eq!(r.request.path, "/stream?beta=true");
        assert_eq!(
            r.request.body,
            Body::Json(serde_json::json!({"model":"m","messages":[]}))
        );
        assert_eq!(r.header("x-claude-code-session-id"), Some("s1"));
        assert!(r.id.starts_with(&format!("{}-", r.started_at_ms)));

        let response = r.response.as_ref().expect("response");
        assert_eq!(response.status, 200);
        assert_eq!(response.stream.as_deref(), Some(sse_text().as_str()));
        let message = response.message.as_ref().expect("message");
        assert_eq!(message["content"][0]["text"], "Hello there");
        assert_eq!(message["stop_reason"], "end_turn");
        assert!(
            response
                .headers
                .iter()
                .any(|h| h.name == "request-id" && h.value == "req_test")
        );

        let first = r.first_byte_at_ms.expect("first byte");
        let ended = r.ended_at_ms.expect("ended");
        assert!(r.started_at_ms <= first, "started after first byte");
        assert!(first <= ended, "first byte after end");
        assert!(
            ended - first >= 250,
            "the record should span the stream gaps"
        );
    }

    #[tokio::test]
    async fn stored_bytes_never_contain_the_credential() {
        let (upstream, _seen) = fake_upstream().await;
        let (proxy, sink) = start_proxy(upstream).await;

        let response = post_stream(&proxy).await;
        response.bytes().await.expect("body");
        let records = sink.wait_for(1).await;

        let text = serde_json::to_string(&records[0]).expect("serialise");
        assert!(!text.contains("test-secret"), "credential stored: {text}");
        assert!(
            !text.to_ascii_lowercase().contains("authorization"),
            "credential header stored: {text}"
        );
    }

    #[tokio::test]
    async fn head_request_passes_through() {
        let (upstream, seen) = fake_upstream().await;
        let (proxy, sink) = start_proxy(upstream).await;

        let response = client()
            .head(format!("{proxy}/api/hello"))
            .send()
            .await
            .expect("head");
        assert_eq!(response.status(), 200);

        let records = sink.wait_for(1).await;
        let r = &records[0];
        assert_eq!(r.request.method, "HEAD");
        assert_eq!(r.request.path, "/api/hello");
        assert_eq!(r.request.body, Body::Empty);
        assert_eq!(r.error, None);
        let response = r.response.as_ref().expect("response");
        assert_eq!(response.status, 200);
        assert_eq!(response.stream, None);
        assert_eq!(seen.lock().expect("lock")[0].method, "HEAD");
    }

    #[tokio::test]
    async fn upstream_error_status_is_passed_back_and_recorded() {
        let (upstream, _seen) = fake_upstream().await;
        let (proxy, sink) = start_proxy(upstream).await;

        let response = client()
            .post(format!("{proxy}/error"))
            .body("{}")
            .send()
            .await
            .expect("send");
        assert_eq!(response.status(), 529);
        assert_eq!(response.text().await.expect("text"), ERROR_BODY);

        let records = sink.wait_for(1).await;
        let r = &records[0];
        assert_eq!(r.error.as_deref(), Some("Overloaded"));
        let response = r.response.as_ref().expect("response");
        assert_eq!(response.status, 529);
        assert_eq!(response.stream.as_deref(), Some(ERROR_BODY));
    }

    #[tokio::test]
    async fn unreachable_upstream_gives_502_and_a_recorded_error() {
        let (proxy, sink) = start_proxy("http://127.0.0.1:1".to_string()).await;

        let response = client()
            .post(format!("{proxy}/v1/messages"))
            .header("authorization", "Bearer test-secret")
            .body("{}")
            .send()
            .await
            .expect("send");
        assert_eq!(response.status(), 502);
        assert_eq!(
            response.text().await.expect("text"),
            "snitchcraft proxy: upstream unreachable"
        );

        let records = sink.wait_for(1).await;
        let r = &records[0];
        assert!(r.error.is_some(), "no error recorded");
        assert!(r.response.is_none(), "a response was recorded");
        assert!(r.ended_at_ms.is_some());
        let text = serde_json::to_string(r).expect("serialise");
        assert!(!text.contains("test-secret"));
    }
}
