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
const EVAL_TIMEOUT: Duration = Duration::from_secs(30);

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

// Evaluates Luau in the live playtest server via the runner, so an agent can ask a
// runtime question (a Humanoid's state, a path's waypoints) without adding a print and
// replaying. Only reaches the server runner, so it errors cleanly when no playtest is up.
pub async fn run_luau_live(bridge: &Bridge, code: &str) -> Result<BridgeResult, String> {
    bridge
        .send("eval", json!({ "code": code }), Role::Server, EVAL_TIMEOUT)
        .await
}

// Asks the server runner for output newer than the caller's per-peer cursor. The runner
// keeps a bounded ring buffer fed by LogService.MessageOut, so each call transfers only
// new lines instead of the whole (unbounded) history that used to make this freeze.
pub async fn get_playtest_output(
    bridge: &Bridge,
    since_server: i64,
    since_client: i64,
) -> Result<BridgeResult, String> {
    let result = bridge
        .send(
            "get_logs",
            json!({ "sinceServer": since_server, "sinceClient": since_client }),
            Role::Server,
            LOGS_TIMEOUT,
        )
        .await?;
    if !result.ok {
        return Ok(result);
    }
    Ok(BridgeResult {
        ok: true,
        data: merge_output(&result.data),
        error: None,
    })
}

// Clears the runner's log buffers (both peers) so a fresh read starts from empty, without
// restarting Studio.
pub async fn reset_playtest_output(bridge: &Bridge) -> Result<BridgeResult, String> {
    bridge
        .send("reset_output", json!({}), Role::Server, STOP_TIMEOUT)
        .await
}

// Merges the two peers' entries (dedup by timestamp+message, sort by time) and carries
// the per-peer cursors so the next call can ask for only newer lines.
fn merge_output(data: &Value) -> Value {
    let server_entries = array_field(data, "serverEntries");
    let client_entries = array_field(data, "clientEntries");
    let server_cursor = data
        .get("serverCursor")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let client_cursor = data
        .get("clientCursor")
        .and_then(Value::as_i64)
        .unwrap_or(0);

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
    json!({
        "entries": entries,
        "cursor": { "server": server_cursor, "client": client_cursor },
    })
}

fn array_field(data: &Value, key: &str) -> Vec<Value> {
    data.get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_dedups_sorts_and_carries_cursor() {
        let data = json!({
            "serverEntries": [
                { "message": "a", "type": "Output", "timestamp": 2.0 },
                { "message": "shared", "type": "Output", "timestamp": 1.0 },
            ],
            "clientEntries": [
                { "message": "shared", "type": "Output", "timestamp": 1.0 },
                { "message": "c", "type": "Output", "timestamp": 3.0 },
            ],
            "serverCursor": 7,
            "clientCursor": 4,
        });
        let out = merge_output(&data);
        let entries = out["entries"].as_array().unwrap();
        // The shared line appears once; sorted by timestamp: shared(1), a(2), c(3).
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0]["message"], "shared");
        assert_eq!(entries[1]["message"], "a");
        assert_eq!(entries[2]["message"], "c");
        // Server wins the dedup, so the shared line is tagged server.
        assert_eq!(entries[0]["peer"], "server");
        assert_eq!(out["cursor"]["server"], 7);
        assert_eq!(out["cursor"]["client"], 4);
    }

    #[test]
    fn merge_handles_missing_fields() {
        let out = merge_output(&json!({}));
        assert_eq!(out["entries"].as_array().unwrap().len(), 0);
        assert_eq!(out["cursor"]["server"], 0);
        assert_eq!(out["cursor"]["client"], 0);
    }
}
