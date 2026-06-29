// Open Cloud place publishing. Uploads a local .rbxl/.rbxlx as a new version of the
// configured experience (needs the universe-places write scope). Same env-only key.
// Binary places use octet-stream; XML places use application/xml.

use std::path::Path;

use reqwest::Client;
use serde_json::Value;

use crate::env;
use crate::httpx;

const BASE: &str = "https://apis.roblox.com/universes/v1";

pub type PublishResult = Result<Value, String>;

pub async fn publish_place(
    http: &Client,
    file_path: &str,
    version_type: Option<&str>,
) -> PublishResult {
    let key = env::var("ROBLOX_OPEN_CLOUD_KEY").ok_or("Missing env: set ROBLOX_OPEN_CLOUD_KEY.")?;
    let universe = env::var("ROBLOX_UNIVERSE_ID").ok_or("Missing env: set ROBLOX_UNIVERSE_ID.")?;
    let place = env::var("ROBLOX_PLACE_ID").ok_or("Missing env: set ROBLOX_PLACE_ID.")?;

    let bytes = std::fs::read(file_path).map_err(|e| format!("cannot read {file_path}: {e}"))?;
    let is_xml = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("rbxlx"))
        .unwrap_or(false);
    let content_type = if is_xml {
        "application/xml"
    } else {
        "application/octet-stream"
    };
    let version = version_type.unwrap_or("Published");
    let url = format!("{BASE}/{universe}/places/{place}/versions");

    let res = httpx::send_retrying(|| {
        http.post(&url)
            .header("x-api-key", &key)
            .header("content-type", content_type)
            .query(&[("versionType", version)])
            .body(bytes.clone())
    })
    .await
    .map_err(|e| format!("publish failed: {e}"))?;

    res.json::<Value>().await.map_err(|e| e.to_string())
}
