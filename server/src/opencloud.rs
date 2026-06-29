// Open Cloud REST clients beyond Luau execution: DataStores, Ordered DataStores,
// MessagingService, Memory Stores, platform info, and engagement. Same env-only key
// as cloud.rs. Each call works when the key has the matching scope and surfaces
// Roblox's own error otherwise, so capabilities are gated by the key, not by us.
//
// cloud/v2 carries value/users/attributes as JSON body fields (the content-md5 and
// roblox-entry-* headers were the old v1 API). Ordered DataStores live under a
// separate ordered-data-stores/v1 base.

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use reqwest::{Client, Method};
use serde_json::{json, Value};

use crate::env;
use crate::httpx;

const CLOUD: &str = "https://apis.roblox.com/cloud/v2";
const ORDERED: &str = "https://apis.roblox.com/ordered-data-stores/v1";
const DEFAULT_SCOPE: &str = "global";

pub type OcResult = Result<Value, String>;

// RFC 3986 unreserved set is kept; everything else in a path segment is encoded.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

fn enc(segment: &str) -> String {
    utf8_percent_encode(segment, SEGMENT).to_string()
}

fn key() -> Result<String, String> {
    env::var("ROBLOX_OPEN_CLOUD_KEY")
        .ok_or_else(|| "Missing env: set ROBLOX_OPEN_CLOUD_KEY.".into())
}

fn key_and_universe() -> Result<(String, String), String> {
    let key = key()?;
    let universe = env::var("ROBLOX_UNIVERSE_ID").ok_or("Missing env: set ROBLOX_UNIVERSE_ID.")?;
    Ok((key, universe))
}

fn obj(value: &Value) -> &serde_json::Map<String, Value> {
    static EMPTY: std::sync::OnceLock<serde_json::Map<String, Value>> = std::sync::OnceLock::new();
    value
        .as_object()
        .unwrap_or_else(|| EMPTY.get_or_init(serde_json::Map::new))
}

// ===== Standard DataStores =====

pub async fn list_datastores(
    http: &Client,
    prefix: Option<&str>,
    max: Option<i64>,
    token: Option<&str>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!("{CLOUD}/universes/{universe}/data-stores");
    httpx::request_json(
        http,
        &key,
        Method::GET,
        &url,
        &page_query(prefix, max, token),
        None,
    )
    .await
}

pub async fn list_datastore_entries(
    http: &Client,
    datastore: &str,
    prefix: Option<&str>,
    max: Option<i64>,
    token: Option<&str>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{CLOUD}/universes/{universe}/data-stores/{}/entries",
        enc(datastore)
    );
    httpx::request_json(
        http,
        &key,
        Method::GET,
        &url,
        &page_query(prefix, max, token),
        None,
    )
    .await
}

pub async fn get_datastore_entry(http: &Client, datastore: &str, entry: &str) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{CLOUD}/universes/{universe}/data-stores/{}/entries/{}",
        enc(datastore),
        enc(entry)
    );
    httpx::request_json(http, &key, Method::GET, &url, &[], None).await
}

pub async fn set_datastore_entry(
    http: &Client,
    datastore: &str,
    entry: &str,
    value: &Value,
    users: Option<Vec<String>>,
    attributes: Option<Value>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{CLOUD}/universes/{universe}/data-stores/{}/entries/{}",
        enc(datastore),
        enc(entry)
    );
    let body = json!({ "value": value, "users": users.unwrap_or_default(), "attributes": attributes.unwrap_or_else(|| json!({})) });
    httpx::request_json(
        http,
        &key,
        Method::PATCH,
        &url,
        &[("allowMissing", "true".into())],
        Some(&body),
    )
    .await
}

pub async fn delete_datastore_entry(http: &Client, datastore: &str, entry: &str) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{CLOUD}/universes/{universe}/data-stores/{}/entries/{}",
        enc(datastore),
        enc(entry)
    );
    httpx::request_json(http, &key, Method::DELETE, &url, &[], None).await
}

pub async fn increment_datastore_entry(
    http: &Client,
    datastore: &str,
    entry: &str,
    amount: i64,
    users: Option<Vec<String>>,
    attributes: Option<Value>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{CLOUD}/universes/{universe}/data-stores/{}/entries/{}:increment",
        enc(datastore),
        enc(entry)
    );
    let body = json!({ "amount": amount, "users": users.unwrap_or_default(), "attributes": attributes.unwrap_or_else(|| json!({})) });
    httpx::request_json(http, &key, Method::POST, &url, &[], Some(&body)).await
}

fn page_query(
    prefix: Option<&str>,
    max: Option<i64>,
    token: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut q = Vec::new();
    if let Some(m) = max {
        q.push(("maxPageSize", m.to_string()));
    }
    if let Some(t) = token {
        q.push(("pageToken", t.to_string()));
    }
    if let Some(p) = prefix.filter(|p| !p.is_empty()) {
        q.push(("filter", format!("id.startsWith(\"{p}\")")));
    }
    q
}

// ===== Ordered DataStores (separate v1 base; non-negative integers) =====

pub async fn list_ordered_entries(
    http: &Client,
    store: &str,
    scope: Option<&str>,
    descending: bool,
    max: Option<i64>,
    token: Option<&str>,
    filter: Option<&str>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let scope = scope.unwrap_or(DEFAULT_SCOPE);
    let url = format!(
        "{ORDERED}/universes/{universe}/orderedDataStores/{}/scopes/{}/entries",
        enc(store),
        enc(scope)
    );
    let mut q: Vec<(&str, String)> = Vec::new();
    if let Some(m) = max {
        q.push(("max_page_size", m.to_string()));
    }
    if let Some(t) = token {
        q.push(("page_token", t.to_string()));
    }
    if descending {
        q.push(("order_by", "desc".into()));
    }
    if let Some(f) = filter {
        q.push(("filter", f.to_string()));
    }
    httpx::request_json(http, &key, Method::GET, &url, &q, None).await
}

pub async fn get_ordered_entry(
    http: &Client,
    store: &str,
    scope: Option<&str>,
    entry: &str,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let scope = scope.unwrap_or(DEFAULT_SCOPE);
    let url = format!(
        "{ORDERED}/universes/{universe}/orderedDataStores/{}/scopes/{}/entries/{}",
        enc(store),
        enc(scope),
        enc(entry)
    );
    httpx::request_json(http, &key, Method::GET, &url, &[], None).await
}

pub async fn set_ordered_entry(
    http: &Client,
    store: &str,
    scope: Option<&str>,
    entry: &str,
    value: i64,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let scope = scope.unwrap_or(DEFAULT_SCOPE);
    let url = format!(
        "{ORDERED}/universes/{universe}/orderedDataStores/{}/scopes/{}/entries/{}",
        enc(store),
        enc(scope),
        enc(entry)
    );
    httpx::request_json(
        http,
        &key,
        Method::PATCH,
        &url,
        &[("allow_missing", "true".into())],
        Some(&json!({ "value": value })),
    )
    .await
}

pub async fn increment_ordered_entry(
    http: &Client,
    store: &str,
    scope: Option<&str>,
    entry: &str,
    amount: i64,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let scope = scope.unwrap_or(DEFAULT_SCOPE);
    let url = format!(
        "{ORDERED}/universes/{universe}/orderedDataStores/{}/scopes/{}/entries/{}:increment",
        enc(store),
        enc(scope),
        enc(entry)
    );
    httpx::request_json(
        http,
        &key,
        Method::POST,
        &url,
        &[],
        Some(&json!({ "amount": amount })),
    )
    .await
}

// ===== MessagingService =====

pub async fn publish_message(http: &Client, topic: &str, message: &str) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!("{CLOUD}/universes/{universe}:publishMessage");
    httpx::request_json(
        http,
        &key,
        Method::POST,
        &url,
        &[],
        Some(&json!({ "topic": topic, "message": message })),
    )
    .await
}

// ===== Memory Stores (ttl is a "300s" duration string) =====

pub async fn memory_sorted_map_set(
    http: &Client,
    map: &str,
    item: &str,
    value: &Value,
    ttl: Option<i64>,
    string_sort_key: Option<&str>,
    numeric_sort_key: Option<f64>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{CLOUD}/universes/{universe}/memory-store/sorted-maps/{}/items/{}",
        enc(map),
        enc(item)
    );
    let mut body = serde_json::Map::new();
    body.insert("value".into(), value.clone());
    if let Some(t) = ttl {
        body.insert("ttl".into(), json!(format!("{t}s")));
    }
    if let Some(s) = string_sort_key {
        body.insert("stringSortKey".into(), json!(s));
    }
    if let Some(n) = numeric_sort_key {
        body.insert("numericSortKey".into(), json!(n));
    }
    httpx::request_json(
        http,
        &key,
        Method::PATCH,
        &url,
        &[("allowMissing", "true".into())],
        Some(&Value::Object(body)),
    )
    .await
}

pub async fn memory_sorted_map_get(http: &Client, map: &str, item: &str) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{CLOUD}/universes/{universe}/memory-store/sorted-maps/{}/items/{}",
        enc(map),
        enc(item)
    );
    httpx::request_json(http, &key, Method::GET, &url, &[], None).await
}

pub async fn memory_sorted_map_list(
    http: &Client,
    map: &str,
    descending: bool,
    max: Option<i64>,
    token: Option<&str>,
    filter: Option<&str>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{CLOUD}/universes/{universe}/memory-store/sorted-maps/{}/items",
        enc(map)
    );
    let mut q: Vec<(&str, String)> = Vec::new();
    if let Some(m) = max {
        q.push(("maxPageSize", m.to_string()));
    }
    if let Some(t) = token {
        q.push(("pageToken", t.to_string()));
    }
    if descending {
        q.push(("orderBy", "value desc".into()));
    }
    if let Some(f) = filter {
        q.push(("filter", f.to_string()));
    }
    let data = httpx::request_json(http, &key, Method::GET, &url, &q, None).await?;
    // Live spec drift: the array is documented as memoryStoreSortedMapItems but the
    // server has returned it as items; surface both.
    let items = obj(&data)
        .get("items")
        .or_else(|| obj(&data).get("memoryStoreSortedMapItems"))
        .cloned()
        .unwrap_or_else(|| json!([]));
    Ok(json!({ "items": items, "nextPageToken": obj(&data).get("nextPageToken") }))
}

pub async fn memory_sorted_map_delete(http: &Client, map: &str, item: &str) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{CLOUD}/universes/{universe}/memory-store/sorted-maps/{}/items/{}",
        enc(map),
        enc(item)
    );
    httpx::request_json(http, &key, Method::DELETE, &url, &[], None).await
}

pub async fn memory_queue_add(
    http: &Client,
    queue: &str,
    data: &Value,
    priority: Option<f64>,
    ttl: Option<i64>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{CLOUD}/universes/{universe}/memory-store/queues/{}/items",
        enc(queue)
    );
    let mut body = serde_json::Map::new();
    body.insert("data".into(), data.clone());
    if let Some(p) = priority {
        body.insert("priority".into(), json!(p));
    }
    if let Some(t) = ttl {
        body.insert("ttl".into(), json!(format!("{t}s")));
    }
    httpx::request_json(
        http,
        &key,
        Method::POST,
        &url,
        &[],
        Some(&Value::Object(body)),
    )
    .await
}

pub async fn memory_queue_read(
    http: &Client,
    queue: &str,
    count: Option<i64>,
    invisibility: Option<i64>,
    all_or_nothing: bool,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{CLOUD}/universes/{universe}/memory-store/queues/{}/items:read",
        enc(queue)
    );
    let mut q: Vec<(&str, String)> = Vec::new();
    if let Some(c) = count {
        q.push(("count", c.to_string()));
    }
    if all_or_nothing {
        q.push(("allOrNothing", "true".into()));
    }
    if let Some(w) = invisibility {
        q.push(("invisibilityWindow", format!("{w}s")));
    }
    let data = httpx::request_json(http, &key, Method::GET, &url, &q, None).await?;
    // Live spec drift: readId has come back as id, items as queueItems.
    let read_id = obj(&data)
        .get("readId")
        .or_else(|| obj(&data).get("id"))
        .cloned()
        .unwrap_or(Value::Null);
    let items = obj(&data)
        .get("items")
        .or_else(|| obj(&data).get("queueItems"))
        .cloned()
        .unwrap_or_else(|| json!([]));
    Ok(json!({ "readId": read_id, "items": items }))
}

pub async fn memory_queue_discard(http: &Client, queue: &str, read_id: &str) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{CLOUD}/universes/{universe}/memory-store/queues/{}/items:discard",
        enc(queue)
    );
    httpx::request_json(
        http,
        &key,
        Method::POST,
        &url,
        &[],
        Some(&json!({ "readId": read_id })),
    )
    .await
}

// ===== Platform info (read-only) =====

pub async fn get_universe(http: &Client) -> OcResult {
    let (key, universe) = key_and_universe()?;
    httpx::request_json(
        http,
        &key,
        Method::GET,
        &format!("{CLOUD}/universes/{universe}"),
        &[],
        None,
    )
    .await
}

pub async fn get_place(http: &Client) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let place = env::var("ROBLOX_PLACE_ID").ok_or("Missing env: set ROBLOX_PLACE_ID.")?;
    httpx::request_json(
        http,
        &key,
        Method::GET,
        &format!("{CLOUD}/universes/{universe}/places/{place}"),
        &[],
        None,
    )
    .await
}

pub async fn get_user(http: &Client, user_id: &str) -> OcResult {
    let key = key()?;
    httpx::request_json(
        http,
        &key,
        Method::GET,
        &format!("{CLOUD}/users/{}", enc(user_id)),
        &[],
        None,
    )
    .await
}

pub async fn get_group(http: &Client, group_id: &str) -> OcResult {
    let key = key()?;
    httpx::request_json(
        http,
        &key,
        Method::GET,
        &format!("{CLOUD}/groups/{}", enc(group_id)),
        &[],
        None,
    )
    .await
}

pub async fn list_inventory(
    http: &Client,
    user_id: &str,
    filter: Option<&str>,
    max: Option<i64>,
    token: Option<&str>,
) -> OcResult {
    let key = key()?;
    let url = format!("{CLOUD}/users/{}/inventory-items", enc(user_id));
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

// ===== Engagement =====

pub async fn send_notification(
    http: &Client,
    user_id: &str,
    message_id: &str,
    parameters: Option<Value>,
    launch_data: Option<&str>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!("{CLOUD}/users/{}/notifications", enc(user_id));
    let mut payload = serde_json::Map::new();
    payload.insert("type".into(), json!("MOMENT"));
    payload.insert("messageId".into(), json!(message_id));
    if let Some(p) = parameters {
        payload.insert("parameters".into(), p);
    }
    if let Some(l) = launch_data {
        payload.insert("joinExperience".into(), json!({ "launchData": l }));
    }
    let body = json!({ "source": { "universe": format!("universes/{universe}") }, "payload": Value::Object(payload) });
    httpx::request_json(http, &key, Method::POST, &url, &[], Some(&body)).await
}

pub async fn get_subscription(http: &Client, product: &str, user_id: &str, full: bool) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{CLOUD}/universes/{universe}/subscription-products/{}/subscriptions/{}",
        enc(product),
        enc(user_id)
    );
    let q: Vec<(&str, String)> = if full {
        vec![("view", "FULL".into())]
    } else {
        Vec::new()
    };
    httpx::request_json(http, &key, Method::GET, &url, &q, None).await
}
