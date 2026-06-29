// Playtest tools. They route through the bridge to the right peer: start goes to the
// plugin (which injects the runner and starts the test); stop and the log fetch go to
// the server runner. The runner template is embedded so the binary is self-contained;
// the server fills the port and protocol version, the plugin fills the instanceId and
// build version at injection time.

use std::time::Duration;

use serde_json::{json, Value};

use crate::bridge::{Bridge, BridgeResult, Role, DEFAULT_TIMEOUT, PROTOCOL_VERSION};

const RUNNER_TEMPLATE: &str = include_str!("../../runner/runner.luau");
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
const LOGS_TIMEOUT: Duration = Duration::from_secs(40);

fn runner_source(port: u16) -> String {
    RUNNER_TEMPLATE
        .replace("{{PORT}}", &port.to_string())
        .replace("{{PROTOCOL_VERSION}}", &PROTOCOL_VERSION.to_string())
}

pub async fn start_playtest(bridge: &Bridge, port: u16) -> Result<BridgeResult, String> {
    bridge
        .send(
            "start_playtest",
            json!({ "runnerSource": runner_source(port) }),
            Role::Plugin,
            DEFAULT_TIMEOUT,
        )
        .await
}

pub async fn start_simulation(bridge: &Bridge, port: u16) -> Result<BridgeResult, String> {
    bridge
        .send(
            "start_simulation",
            json!({ "runnerSource": runner_source(port) }),
            Role::Plugin,
            DEFAULT_TIMEOUT,
        )
        .await
}

pub async fn stop_playtest(bridge: &Bridge) -> Result<BridgeResult, String> {
    request_stop(bridge, "playtest").await
}

pub async fn stop_simulation(bridge: &Bridge) -> Result<BridgeResult, String> {
    request_stop(bridge, "simulation").await
}

// Both stops ask the server runner to call EndTest. F8 stops cleanly; F5 is
// best-effort, so a timeout is reported as "sent, may need a manual Stop" rather than
// a hard failure.
async fn request_stop(bridge: &Bridge, kind: &str) -> Result<BridgeResult, String> {
    match bridge
        .send("stop", json!({ "kind": kind }), Role::Server, STOP_TIMEOUT)
        .await
    {
        Ok(result) => Ok(result),
        Err(reason) if kind == "playtest" => Ok(BridgeResult {
            ok: true,
            data: json!({
                "stopped": "unconfirmed",
                "note": format!(
                    "Stop was sent but not confirmed within {}s (F5 teardown is best-effort). If the playtest is still running, press Stop. ({reason})",
                    STOP_TIMEOUT.as_secs()
                ),
            }),
            error: None,
        }),
        Err(reason) => Err(reason),
    }
}

// Asks the server runner for the output log. It returns its own entries plus the
// client's (over the relay) as two arrays; the merge and dedup happen here.
pub async fn get_playtest_output(bridge: &Bridge) -> Result<BridgeResult, String> {
    let result = bridge
        .send("get_logs", json!({}), Role::Server, LOGS_TIMEOUT)
        .await?;
    if !result.ok {
        return Ok(result);
    }
    let server_entries = result
        .data
        .get("serverEntries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let client_entries = result
        .data
        .get("clientEntries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut entries: Vec<Value> = Vec::new();
    let mut server_keys = std::collections::HashSet::new();
    for entry in &server_entries {
        server_keys.insert(entry_key(entry));
        entries.push(tagged(entry, "server"));
    }
    for entry in &client_entries {
        if server_keys.contains(&entry_key(entry)) {
            continue;
        }
        entries.push(tagged(entry, "client"));
    }
    entries.sort_by(|a, b| {
        let ta = a.get("timestamp").and_then(Value::as_f64).unwrap_or(0.0);
        let tb = b.get("timestamp").and_then(Value::as_f64).unwrap_or(0.0);
        ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(BridgeResult {
        ok: true,
        data: json!({ "entries": entries }),
        error: None,
    })
}

fn entry_key(entry: &Value) -> String {
    let ts = entry
        .get("timestamp")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let msg = entry.get("message").and_then(Value::as_str).unwrap_or("");
    format!("{ts}|{msg}")
}

fn tagged(entry: &Value, peer: &str) -> Value {
    let mut out = entry.clone();
    if let Some(map) = out.as_object_mut() {
        map.insert("peer".into(), json!(peer));
    }
    out
}
