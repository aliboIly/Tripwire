// Open Cloud Assets upload client. Reads a local file, posts a multipart create to
// apis.roblox.com/assets/v1/assets, polls the operation, and returns the assetId (or
// a moderation/rejection error). The multipart boundary is set by reqwest; the key
// is sent in x-api-key only. Creator id from ROBLOX_CREATOR_USER_ID / GROUP_ID.

use std::path::Path;
use std::time::{Duration, Instant};

use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde_json::{json, Value};

use crate::env;
use crate::httpx;

const ASSETS_BASE: &str = "https://apis.roblox.com/assets/v1";
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const POLL_DEADLINE: Duration = Duration::from_secs(120);
const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

pub type UploadResult = Result<Value, String>;

fn creator() -> Result<Value, String> {
    if let Some(user) = env::var("ROBLOX_CREATOR_USER_ID") {
        Ok(json!({ "userId": user }))
    } else if let Some(group) = env::var("ROBLOX_CREATOR_GROUP_ID") {
        Ok(json!({ "groupId": group }))
    } else {
        Err("Missing env: set ROBLOX_CREATOR_USER_ID or ROBLOX_CREATOR_GROUP_ID.".into())
    }
}

fn content_type_for(path: &str, override_type: Option<&str>) -> Result<String, String> {
    if let Some(ct) = override_type {
        return Ok(ct.to_string());
    }
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpeg" | "jpg" => "image/jpeg",
        "bmp" => "image/bmp",
        "tga" => "image/tga",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "fbx" => "model/fbx",
        "gltf" => "model/gltf+json",
        "glb" => "model/gltf-binary",
        "rbxm" => "model/x-rbxm",
        "rbxmx" => "model/x-rbxmx",
        "mp4" => "video/mp4",
        "mov" => "video/mov",
        _ => {
            return Err(format!(
                "cannot infer a content type for {path}; pass contentType explicitly."
            ))
        }
    };
    Ok(mime.to_string())
}

pub async fn upload_asset(
    http: &Client,
    file_path: &str,
    asset_type: &str,
    display_name: &str,
    description: Option<&str>,
    content_type: Option<&str>,
) -> UploadResult {
    let key = env::var("ROBLOX_OPEN_CLOUD_KEY").ok_or("Missing env: set ROBLOX_OPEN_CLOUD_KEY.")?;
    let creator = creator()?;

    let bytes = std::fs::read(file_path).map_err(|e| format!("cannot read {file_path}: {e}"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "file is {} bytes; the per-upload limit is {MAX_FILE_BYTES}.",
            bytes.len()
        ));
    }
    let mime = content_type_for(file_path, content_type)?;
    let file_name = Path::new(file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("upload")
        .to_string();
    let request = serde_json::to_string(&json!({
        "assetType": asset_type,
        "displayName": display_name,
        "description": description.unwrap_or(""),
        "creationContext": { "creator": creator },
    }))
    .map_err(|e| e.to_string())?;

    let created = httpx::send_retrying(|| {
        // Built fresh each attempt: a multipart body is consumed on send and the
        // bytes are read in once, so cloning here keeps retries replayable.
        let request_part = Part::text(request.clone())
            .mime_str("application/json")
            .expect("application/json is a valid mime");
        let file_part = Part::bytes(bytes.clone())
            .file_name(file_name.clone())
            .mime_str(&mime)
            .expect("inferred mime is valid");
        let form = Form::new()
            .part("request", request_part)
            .part("fileContent", file_part);
        http.post(format!("{ASSETS_BASE}/assets"))
            .header("x-api-key", &key)
            .multipart(form)
    })
    .await
    .map_err(|e| format!("create asset failed: {e}"))?;

    let operation: Value = created.json().await.map_err(|e| e.to_string())?;
    let op_path = operation
        .get("path")
        .and_then(Value::as_str)
        .ok_or("create asset returned no operation path")?
        .to_string();

    let done = poll_operation(http, &key, &op_path).await?;
    if let Some(error) = done.get("error") {
        if !error.is_null() {
            return Err(format!("asset rejected: {error}"));
        }
    }
    let response = done.get("response").cloned().unwrap_or(Value::Null);
    Ok(json!({
        "assetId": response.get("assetId"),
        "revisionId": response.get("revisionId"),
    }))
}

async fn poll_operation(http: &Client, key: &str, path: &str) -> Result<Value, String> {
    let url = format!("{ASSETS_BASE}/{path}");
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let op = httpx::send_retrying(|| http.get(&url).header("x-api-key", key)).await?;
        let body: Value = op.json().await.map_err(|e| e.to_string())?;
        if body.get("done").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(body);
        }
        if Instant::now() > deadline {
            return Err(format!(
                "asset operation did not finish within {}s",
                POLL_DEADLINE.as_secs()
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
