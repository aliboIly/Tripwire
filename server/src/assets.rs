// Open Cloud Assets client (asset:read / asset:write, asset-permissions:write, and
// the legacy asset delivery scope). Upload reads a local file, posts a multipart
// create to apis.roblox.com/assets/v1/assets, polls the operation, and returns the
// assetId (or a moderation/rejection error). The rest is metadata and version
// management over the same base. The multipart boundary is set by reqwest; the key
// is sent in x-api-key only. Creator id from ROBLOX_CREATOR_USER_ID / GROUP_ID.

use std::path::Path;
use std::time::Duration;

use reqwest::multipart::{Form, Part};
use reqwest::{Client, Method};
use serde_json::{json, Value};

use crate::env;
use crate::httpx::{self, API_HOST};

const ASSETS_BASE: &str = "https://apis.roblox.com/assets/v1";
const CLOUD: &str = "https://apis.roblox.com/cloud/v2";
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
    httpx::poll_operation(http, key, &format!("{ASSETS_BASE}/{path}"), POLL_DEADLINE).await
}

fn key() -> Result<String, String> {
    env::var("ROBLOX_OPEN_CLOUD_KEY")
        .ok_or_else(|| "Missing env: set ROBLOX_OPEN_CLOUD_KEY.".into())
}

fn asset_url(asset_id: &str) -> String {
    format!("{ASSETS_BASE}/assets/{asset_id}")
}

fn parse_json(text: &str) -> Result<Value, String> {
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(text).map_err(|e| format!("invalid JSON in response: {e}"))
}

// ===== Asset management =====

pub async fn get_asset(http: &Client, asset_id: &str, read_mask: Option<&str>) -> UploadResult {
    let key = key()?;
    let q: Vec<(&str, String)> = read_mask
        .map(|m| vec![("readMask", m.to_string())])
        .unwrap_or_default();
    httpx::request_json(http, &key, Method::GET, &asset_url(asset_id), &q, None).await
}

/// Updates the display name and description, and for Models optionally the
/// content itself. Polls the returned operation like upload does.
pub async fn update_asset(
    http: &Client,
    asset_id: &str,
    display_name: Option<&str>,
    description: Option<&str>,
    file_path: Option<&str>,
    content_type: Option<&str>,
) -> UploadResult {
    let key = key()?;
    let mut request = serde_json::Map::new();
    request.insert("assetId".into(), json!(asset_id));
    let mut mask: Vec<&str> = Vec::new();
    if let Some(n) = display_name {
        request.insert("displayName".into(), json!(n));
        mask.push("displayName");
    }
    if let Some(d) = description {
        request.insert("description".into(), json!(d));
        mask.push("description");
    }
    if mask.is_empty() && file_path.is_none() {
        return Err("nothing to update: pass displayName, description, or filePath".into());
    }
    let request = Value::Object(request).to_string();
    let file = match file_path {
        Some(path) => {
            let bytes = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
            let mime = content_type_for(path, content_type)?;
            let name = Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("upload")
                .to_string();
            Some((bytes, name, mime))
        }
        None => None,
    };
    let mask = mask.join(",");
    let res = httpx::send_retrying(|| {
        let mut form = Form::new().part(
            "request",
            Part::text(request.clone())
                .mime_str("application/json")
                .expect("application/json is a valid mime"),
        );
        if let Some((bytes, name, mime)) = &file {
            form = form.part("fileContent", httpx::file_part(bytes.clone(), name, mime));
        }
        let mut rb = http
            .patch(asset_url(asset_id))
            .header("x-api-key", &key)
            .multipart(form);
        if !mask.is_empty() {
            rb = rb.query(&[("updateMask", &mask)]);
        }
        rb
    })
    .await
    .map_err(|e| format!("update asset failed: {e}"))?;
    let op = parse_json(&res.text().await.map_err(|e| e.to_string())?)?;
    let done = if httpx::is_done(&op) {
        httpx::finished_operation(op)?
    } else {
        let path = op
            .get("path")
            .and_then(Value::as_str)
            .ok_or("update asset returned no operation path")?
            .to_string();
        poll_operation(http, &key, &path).await?
    };
    Ok(done.get("response").cloned().unwrap_or(Value::Null))
}

pub async fn list_asset_versions(
    http: &Client,
    asset_id: &str,
    max: Option<i64>,
    token: Option<&str>,
) -> UploadResult {
    let key = key()?;
    let mut q: Vec<(&str, String)> = Vec::new();
    if let Some(m) = max {
        q.push(("maxPageSize", m.to_string()));
    }
    if let Some(t) = token {
        q.push(("pageToken", t.to_string()));
    }
    let url = format!("{}/versions", asset_url(asset_id));
    httpx::request_json(http, &key, Method::GET, &url, &q, None).await
}

pub async fn rollback_asset_version(http: &Client, asset_id: &str, version: &str) -> UploadResult {
    let key = key()?;
    let url = format!("{}/versions:rollback", asset_url(asset_id));
    let version_path = format!("assets/{asset_id}/versions/{version}");
    let res = httpx::send_retrying(|| {
        http.post(&url)
            .header("x-api-key", &key)
            .multipart(Form::new().text("assetVersion", version_path.clone()))
    })
    .await?;
    parse_json(&res.text().await.map_err(|e| e.to_string())?)
}

/// `archive` hides the asset from the site and experiences; `restore` reverses it.
pub async fn set_asset_archived(http: &Client, asset_id: &str, archive: bool) -> UploadResult {
    let key = key()?;
    let verb = if archive { "archive" } else { "restore" };
    let url = format!("{}:{verb}", asset_url(asset_id));
    httpx::request_json(http, &key, Method::POST, &url, &[], Some(&json!({}))).await
}

pub async fn list_asset_quotas(
    http: &Client,
    user_id: &str,
    filter: Option<&str>,
    max: Option<i64>,
    token: Option<&str>,
) -> UploadResult {
    let key = key()?;
    let url = format!("{CLOUD}/users/{user_id}/asset-quotas");
    let mut q: Vec<(&str, String)> = Vec::new();
    if let Some(m) = max {
        q.push(("maxPageSize", m.to_string()));
    }
    if let Some(t) = token {
        q.push(("pageToken", t.to_string()));
    }
    if let Some(f) = filter {
        q.push(("filter", f.to_string()));
    }
    httpx::request_json(http, &key, Method::GET, &url, &q, None).await
}

/// Grants one permission on a batch of assets to a subject (User, Group,
/// GroupRoleset, Universe, or All). Returns the ids that succeeded and any errors.
pub async fn grant_asset_permissions(
    http: &Client,
    asset_ids: &[i64],
    subject_type: &str,
    subject_id: Option<&str>,
    action: &str,
) -> UploadResult {
    let key = key()?;
    let url = format!("{API_HOST}/asset-permissions-api/v1/assets/permissions");
    let body = json!({
        "subjectType": subject_type,
        "subjectId": subject_id.unwrap_or(""),
        "action": action,
        "assetIds": asset_ids,
    });
    httpx::request_json(http, &key, Method::PATCH, &url, &[], Some(&body)).await
}

/// Downloads an asset's content (legacy-asset:manage) to a local file: the delivery
/// API returns a short-lived CDN location, which is fetched and written out.
pub async fn download_asset(
    http: &Client,
    asset_id: &str,
    out_path: &str,
    version: Option<&str>,
) -> UploadResult {
    let key = key()?;
    let url = match version {
        Some(v) => format!("{API_HOST}/asset-delivery-api/v1/assetId/{asset_id}/version/{v}"),
        None => format!("{API_HOST}/asset-delivery-api/v1/assetId/{asset_id}"),
    };
    let info = httpx::request_json(http, &key, Method::GET, &url, &[], None).await?;
    let location = info
        .get("location")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("no download location in response: {info}"))?;
    // The CDN URL is pre-signed; the API key is not sent with it. reqwest transparently
    // decompresses the gzip the CDN serves.
    let res = httpx::send_retrying(|| http.get(location)).await?;
    let bytes = res.bytes().await.map_err(|e| e.to_string())?;
    std::fs::write(out_path, &bytes).map_err(|e| format!("cannot write {out_path}: {e}"))?;
    Ok(json!({ "path": out_path, "bytes": bytes.len(), "assetId": asset_id }))
}
