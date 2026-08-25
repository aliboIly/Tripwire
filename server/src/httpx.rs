// Shared HTTP plumbing for the Open Cloud clients: one pooled reqwest client and a
// small retry on transient failures (network errors, 429, 5xx), mirroring the
// hand-rolled retry the Node server used. The API key travels only in the per-request
// x-api-key header.

use std::time::{Duration, Instant};

use reqwest::multipart::{Form, Part};
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use serde_json::Value;

/// Every Open Cloud API lives under this host. The generic request tool is pinned to
/// it so the key can never be sent anywhere else.
pub const API_HOST: &str = "https://apis.roblox.com";

const ATTEMPTS: u32 = 3;
const BASE_DELAY: Duration = Duration::from_secs(1);
const OPERATION_POLL_INTERVAL: Duration = Duration::from_secs(2);

pub fn client() -> Client {
    Client::builder()
        .build()
        .expect("reqwest client builds with the rustls backend")
}

fn is_transient(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

/// Sends a request, retrying transient failures. `build` returns a fresh builder
/// each attempt because a request body is consumed on send. A non-transient error
/// status is returned as an `Err` carrying the status and body, so callers surface
/// Roblox's own message (for example an insufficient-scope 401).
pub async fn send_retrying(build: impl Fn() -> RequestBuilder) -> Result<Response, String> {
    let mut last = String::new();
    for attempt in 1..=ATTEMPTS {
        match build().send().await {
            Ok(res) => {
                let status = res.status();
                if status.is_success() {
                    return Ok(res);
                }
                let body = res.text().await.unwrap_or_default();
                let detail = format!("{} {}", status.as_u16(), body.trim());
                if !is_transient(status) {
                    return Err(detail.trim().to_string());
                }
                last = detail;
            }
            Err(err) => last = err.to_string(),
        }
        if attempt < ATTEMPTS {
            tokio::time::sleep(BASE_DELAY * attempt).await;
        }
    }
    Err(format!("failed after {ATTEMPTS} attempts: {last}"))
}

/// A JSON request with the auth header, optional query params, and optional JSON
/// body. An empty 2xx body (common for deletes and message publishes) becomes
/// `Value::Null` rather than a parse error.
pub async fn request_json(
    http: &Client,
    key: &str,
    method: Method,
    url: &str,
    query: &[(&str, String)],
    body: Option<&Value>,
) -> Result<Value, String> {
    let res = send_retrying(|| {
        let mut rb = http.request(method.clone(), url).header("x-api-key", key);
        if !query.is_empty() {
            rb = rb.query(query);
        }
        if let Some(b) = body {
            rb = rb.json(b);
        }
        rb
    })
    .await?;
    let text = res.text().await.map_err(|e| e.to_string())?;
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).map_err(|e| format!("invalid JSON in response: {e}"))
}

/// Polls a long-running Operation (`{ path, done, error, response }`) at an absolute
/// URL until `done`, then returns the whole operation. A populated `error` becomes an
/// `Err` so callers surface Roblox's message.
pub async fn poll_operation(
    http: &Client,
    key: &str,
    url: &str,
    deadline: Duration,
) -> Result<Value, String> {
    let stop = Instant::now() + deadline;
    loop {
        let op = request_json(http, key, Method::GET, url, &[], None).await?;
        if is_done(&op) {
            return finished_operation(op);
        }
        if Instant::now() > stop {
            return Err(format!(
                "operation did not finish within {}s",
                deadline.as_secs()
            ));
        }
        tokio::time::sleep(OPERATION_POLL_INTERVAL).await;
    }
}

pub fn is_done(op: &Value) -> bool {
    op.get("done").and_then(Value::as_bool).unwrap_or(false)
}

/// A finished operation is an error when its `error` field is populated.
pub fn finished_operation(op: Value) -> Result<Value, String> {
    match op.get("error") {
        Some(err) if !err.is_null() => Err(format!("operation failed: {err}")),
        _ => Ok(op),
    }
}

/// Text fields for a multipart form, skipping absent optionals. Bools and numbers are
/// sent as their plain string form, which is what the form-based APIs expect.
pub fn form_fields(fields: &[(&str, Option<String>)]) -> Form {
    let mut form = Form::new();
    for (name, value) in fields {
        if let Some(v) = value {
            form = form.text(name.to_string(), v.clone());
        }
    }
    form
}

/// A file part read into memory, with a mime type, for a multipart upload.
pub fn file_part(bytes: Vec<u8>, file_name: &str, mime: &str) -> Part {
    Part::bytes(bytes)
        .file_name(file_name.to_string())
        .mime_str(mime)
        .expect("caller passes a valid mime type")
}
