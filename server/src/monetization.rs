// Open Cloud monetization and Creator Store clients: developer products, game
// passes (both multipart form APIs with an optional icon upload), and the Creator
// Store catalogue (search and product details). Same env-only key as cloud.rs.

use std::path::Path;

use reqwest::{Client, Method};
use serde_json::Value;

use crate::env;
use crate::httpx;

const CLOUD: &str = "https://apis.roblox.com/cloud/v2";
const DEVELOPER_PRODUCTS: &str = "https://apis.roblox.com/developer-products/v2/universes";
const GAME_PASSES: &str = "https://apis.roblox.com/game-passes/v1/universes";
const TOOLBOX: &str = "https://apis.roblox.com/toolbox-service/v2";

pub type MonetizationResult = Result<Value, String>;

/// Which of the two form-based product APIs a call targets.
#[derive(Clone, Copy)]
pub enum Product {
    DeveloperProduct,
    GamePass,
}

impl Product {
    fn base(self, universe: &str) -> String {
        match self {
            Product::DeveloperProduct => {
                format!("{DEVELOPER_PRODUCTS}/{universe}/developer-products")
            }
            Product::GamePass => format!("{GAME_PASSES}/{universe}/game-passes"),
        }
    }
}

/// The editable fields of a developer product or game pass. Absent fields are left
/// untouched on update.
#[derive(Default)]
pub struct ProductFields {
    pub name: Option<String>,
    pub description: Option<String>,
    pub price: Option<i64>,
    pub is_for_sale: Option<bool>,
    pub is_regional_pricing_enabled: Option<bool>,
    pub is_managed_pricing_enabled: Option<bool>,
    pub store_page_enabled: Option<bool>,
    pub image_path: Option<String>,
}

fn key_and_universe() -> Result<(String, String), String> {
    let key = env::var("ROBLOX_OPEN_CLOUD_KEY").ok_or("Missing env: set ROBLOX_OPEN_CLOUD_KEY.")?;
    let universe = env::var("ROBLOX_UNIVERSE_ID").ok_or("Missing env: set ROBLOX_UNIVERSE_ID.")?;
    Ok((key, universe))
}

pub async fn list_products(
    http: &Client,
    kind: Product,
    page_size: Option<i64>,
    token: Option<&str>,
) -> MonetizationResult {
    let (key, universe) = key_and_universe()?;
    let url = format!("{}/creator", kind.base(&universe));
    let mut q: Vec<(&str, String)> = Vec::new();
    if let Some(p) = page_size {
        q.push(("pageSize", p.to_string()));
    }
    if let Some(t) = token {
        q.push(("pageToken", t.to_string()));
    }
    httpx::request_json(http, &key, Method::GET, &url, &q, None).await
}

pub async fn get_product(http: &Client, kind: Product, id: &str) -> MonetizationResult {
    let (key, universe) = key_and_universe()?;
    let url = format!("{}/{id}/creator", kind.base(&universe));
    httpx::request_json(http, &key, Method::GET, &url, &[], None).await
}

/// Creates (`id = None`) or updates a product. The icon, when given, is read once
/// and re-attached per retry because a multipart body is consumed on send.
pub async fn save_product(
    http: &Client,
    kind: Product,
    id: Option<&str>,
    fields: ProductFields,
) -> MonetizationResult {
    let (key, universe) = key_and_universe()?;
    let (method, url) = match id {
        Some(id) => (Method::PATCH, format!("{}/{id}", kind.base(&universe))),
        None => (Method::POST, kind.base(&universe)),
    };
    let image = match &fields.image_path {
        Some(path) => Some(read_image(path)?),
        None => None,
    };
    let text_fields = [
        ("name", fields.name.clone()),
        ("description", fields.description.clone()),
        ("price", fields.price.map(|p| p.to_string())),
        ("isForSale", fields.is_for_sale.map(|b| b.to_string())),
        (
            "isRegionalPricingEnabled",
            fields.is_regional_pricing_enabled.map(|b| b.to_string()),
        ),
        (
            "isManagedPricingEnabled",
            fields.is_managed_pricing_enabled.map(|b| b.to_string()),
        ),
        (
            "storePageEnabled",
            fields.store_page_enabled.map(|b| b.to_string()),
        ),
    ];
    let res = httpx::send_retrying(|| {
        let mut form = httpx::form_fields(&text_fields);
        if let Some((bytes, name, mime)) = &image {
            form = form.part("imageFile", httpx::file_part(bytes.clone(), name, mime));
        }
        http.request(method.clone(), &url)
            .header("x-api-key", &key)
            .multipart(form)
    })
    .await?;
    let text = res.text().await.map_err(|e| e.to_string())?;
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).map_err(|e| format!("invalid JSON in response: {e}"))
}

fn read_image(path: &str) -> Result<(Vec<u8>, String, String), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("icon.png")
        .to_string();
    let mime = match Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        _ => return Err(format!("{path}: the icon must be a .png or .jpg file")),
    };
    Ok((bytes, name, mime.to_string()))
}

// ===== Creator Store =====

pub async fn search_creator_store(http: &Client, query: Vec<(&str, String)>) -> MonetizationResult {
    let key = env::var("ROBLOX_OPEN_CLOUD_KEY").ok_or("Missing env: set ROBLOX_OPEN_CLOUD_KEY.")?;
    let url = format!("{TOOLBOX}/assets:search");
    httpx::request_json(http, &key, Method::GET, &url, &query, None).await
}

pub async fn get_creator_store_asset(http: &Client, asset_id: &str) -> MonetizationResult {
    let key = env::var("ROBLOX_OPEN_CLOUD_KEY").ok_or("Missing env: set ROBLOX_OPEN_CLOUD_KEY.")?;
    let url = format!("{TOOLBOX}/assets/{asset_id}");
    httpx::request_json(http, &key, Method::GET, &url, &[], None).await
}

pub async fn get_creator_store_product(http: &Client, product_id: &str) -> MonetizationResult {
    let key = env::var("ROBLOX_OPEN_CLOUD_KEY").ok_or("Missing env: set ROBLOX_OPEN_CLOUD_KEY.")?;
    let url = format!("{CLOUD}/creator-store-products/{product_id}");
    httpx::request_json(http, &key, Method::GET, &url, &[], None).await
}
