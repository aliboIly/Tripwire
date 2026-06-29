// Headless test harness. Sends a tiny Luau entry shim through the Open Cloud client,
// which requires the in-place runner and returns its summary on the task's return
// value. Three outcomes are kept distinct: the run itself failed, the tests ran with
// failures, or all passed. Tests live in the published place, so the place must be
// published and the Open Cloud env set.

use std::path::Path;

use reqwest::Client;
use serde_json::Value;

use crate::cloud::{self, LuauResult};
use crate::env::CloudCreds;

const RUNNER: &str = "require(game:GetService(\"ReplicatedStorage\").TS.harness.runTests)";

pub struct Summary {
    pub passed: i64,
    pub failed: i64,
    pub failures: Vec<(String, String)>,
}

pub enum Outcome {
    HarnessError { error: String, logs: Vec<String> },
    Tests { summary: Summary },
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

pub async fn run_tests(http: &Client, creds: &CloudCreds) -> Outcome {
    parse_summary(cloud::run_luau(http, creds, &format!("return {RUNNER}.runTests(nil)")).await)
}

pub async fn run_test_file(http: &Client, creds: &CloudCreds, spec: &str) -> Outcome {
    if !valid_name(spec) {
        return Outcome::HarnessError {
            error: format!("invalid spec name: {spec}"),
            logs: Vec::new(),
        };
    }
    parse_summary(
        cloud::run_luau(
            http,
            creds,
            &format!("return {RUNNER}.runTests(\"{spec}\")"),
        )
        .await,
    )
}

pub async fn list_tests(http: &Client, creds: &CloudCreds) -> Result<Value, String> {
    let result = cloud::run_luau(http, creds, &format!("return {RUNNER}.listTests()")).await;
    if !result.ok {
        return Err(result
            .error
            .unwrap_or_else(|| "Open Cloud run failed".into()));
    }
    match result.results.into_iter().next() {
        Some(raw @ Value::Array(_)) => Ok(raw),
        _ => Err("the runner returned no test list".into()),
    }
}

fn parse_summary(result: LuauResult) -> Outcome {
    if !result.ok {
        return Outcome::HarnessError {
            error: result
                .error
                .unwrap_or_else(|| "Open Cloud run failed".into()),
            logs: result.logs,
        };
    }
    match result.results.first().and_then(to_summary) {
        Some(summary) => Outcome::Tests { summary },
        None => Outcome::HarnessError {
            error: "the test runner returned no summary (it may have crashed before returning)"
                .into(),
            logs: result.logs,
        },
    }
}

fn to_summary(raw: &Value) -> Option<Summary> {
    let obj = raw.as_object()?;
    let passed = obj.get("passed")?.as_i64()?;
    let failed = obj.get("failed")?.as_i64()?;
    let mut failures = Vec::new();
    if let Some(list) = obj.get("failures").and_then(Value::as_array) {
        for f in list {
            let name = f
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("(unnamed)")
                .to_string();
            let message = f
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            failures.push((name, message));
        }
    }
    Some(Summary {
        passed,
        failed,
        failures,
    })
}

pub fn format_outcome(outcome: &Outcome) -> String {
    match outcome {
        Outcome::HarnessError { error, logs } => {
            let tail = if logs.is_empty() {
                String::new()
            } else {
                format!("\nlogs:\n{}", logs.join("\n"))
            };
            format!("Harness error: {error}{tail}")
        }
        Outcome::Tests { summary } => {
            let mut lines = vec![format!(
                "Tests: {} passed, {} failed.",
                summary.passed, summary.failed
            )];
            for (name, message) in &summary.failures {
                lines.push(format!("  FAIL {name}: {message}"));
            }
            lines.join("\n")
        }
    }
}

pub fn format_test_list(specs: &Value) -> String {
    let list = match specs.as_array() {
        Some(list) if !list.is_empty() => list,
        _ => return "No specs found. Build and publish the place first.".into(),
    };
    let mut lines = Vec::new();
    for spec in list {
        lines.push(
            spec.get("file")
                .and_then(Value::as_str)
                .unwrap_or("(unknown)")
                .to_string(),
        );
        if let Some(cases) = spec.get("cases").and_then(Value::as_array) {
            for c in cases.iter().filter_map(Value::as_str) {
                lines.push(format!("  - {c}"));
            }
        }
    }
    lines.join("\n")
}

// Writes a spec source file to disk; rebuild and publish before run_tests sees it.
pub fn write_test(name: &str, source: &str, dir: Option<&str>) -> Result<String, String> {
    if !valid_name(name) {
        return Err(format!("invalid test name: {name}"));
    }
    let dir = dir.unwrap_or("sample-game/src/shared");
    let file_name = if name.ends_with(".spec.ts") {
        name.to_string()
    } else {
        format!("{name}.spec.ts")
    };
    let path = Path::new(dir).join(&file_name);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let body = if source.ends_with('\n') {
        source.to_string()
    } else {
        format!("{source}\n")
    };
    std::fs::write(&path, body).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}
