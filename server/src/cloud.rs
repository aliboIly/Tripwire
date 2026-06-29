// Open Cloud Luau Execution client. Runs a Luau script in the configured place on a
// real RCC server through Open Cloud, then returns its console logs and any return
// values. The backend the headless test harness builds on.
//
// Contract verified against the official guide:
//   https://create.roblox.com/docs/cloud/reference/features/luau-execution
// The place must be published; Open Cloud cannot see unsaved Studio edits.

use std::time::{Duration, Instant};

use reqwest::{Client, Method};
use serde_json::{json, Value};

use crate::env::CloudCreds;
use crate::httpx;

const CLOUD_BASE: &str = "https://apis.roblox.com/cloud/v2";
const POLL_INTERVAL: Duration = Duration::from_secs(3);
const POLL_DEADLINE: Duration = Duration::from_secs(5 * 60 + 30); // task cap plus queue headroom
const LOG_PAGE_SIZE: u32 = 10_000;

pub struct LuauResult {
    pub ok: bool,
    pub logs: Vec<String>,
    pub results: Vec<Value>,
    pub error: Option<String>,
}

impl LuauResult {
    fn failed(error: String, logs: Vec<String>) -> Self {
        Self {
            ok: false,
            logs,
            results: Vec::new(),
            error: Some(error),
        }
    }
}

pub async fn run_luau(http: &Client, creds: &CloudCreds, script: &str) -> LuauResult {
    let create_url = format!(
        "{CLOUD_BASE}/universes/{}/places/{}/luau-execution-session-tasks",
        creds.universe, creds.place
    );
    let task = match httpx::request_json(
        http,
        &creds.key,
        Method::POST,
        &create_url,
        &[],
        Some(&json!({ "script": script })),
    )
    .await
    {
        Ok(task) => task,
        Err(e) => return LuauResult::failed(format!("create task failed: {e}"), Vec::new()),
    };

    let path = match task.get("path").and_then(Value::as_str) {
        Some(path) => path.to_string(),
        None => return LuauResult::failed("create response had no task path".into(), Vec::new()),
    };

    let final_task = match poll_until_terminal(http, &creds.key, &path).await {
        Ok(task) => task,
        Err(e) => return LuauResult::failed(format!("polling failed: {e}"), Vec::new()),
    };

    // Logs are a separate, best-effort channel; a failed log fetch must not turn an
    // otherwise good run into a failure.
    let logs = fetch_logs(http, &creds.key, &path)
        .await
        .unwrap_or_default();

    let state = final_task
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("STATE_UNSPECIFIED");
    if state == "COMPLETE" {
        let results = final_task
            .get("output")
            .and_then(|o| o.get("results"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        return LuauResult {
            ok: true,
            logs,
            results,
            error: None,
        };
    }
    let reason = if state == "FAILED" {
        final_task
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_string()
    } else {
        state.to_lowercase()
    };
    LuauResult::failed(format!("task {state}: {reason}"), logs)
}

async fn poll_until_terminal(http: &Client, key: &str, path: &str) -> Result<Value, String> {
    let url = format!("{CLOUD_BASE}/{path}");
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let task = httpx::request_json(http, key, Method::GET, &url, &[], None).await?;
        let state = task.get("state").and_then(Value::as_str).unwrap_or("");
        if matches!(state, "COMPLETE" | "FAILED" | "CANCELLED") {
            return Ok(task);
        }
        if Instant::now() > deadline {
            return Err(format!(
                "task did not finish within {}s (last state {state})",
                POLL_DEADLINE.as_secs()
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn fetch_logs(http: &Client, key: &str, path: &str) -> Result<Vec<String>, String> {
    let url = format!("{CLOUD_BASE}/{path}/logs");
    let mut lines = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let mut query: Vec<(&str, String)> = vec![("maxPageSize", LOG_PAGE_SIZE.to_string())];
        if let Some(token) = &page_token {
            query.push(("pageToken", token.clone()));
        }
        let body = httpx::request_json(http, key, Method::GET, &url, &query, None).await?;
        if let Some(entries) = body
            .get("luauExecutionSessionTaskLogs")
            .and_then(Value::as_array)
        {
            for entry in entries {
                if let Some(messages) = entry.get("messages").and_then(Value::as_array) {
                    lines.extend(
                        messages
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string),
                    );
                }
            }
        }
        page_token = body
            .get("nextPageToken")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if page_token.is_none() {
            break;
        }
    }
    Ok(lines)
}
