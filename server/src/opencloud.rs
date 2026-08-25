// Open Cloud REST clients beyond Luau execution: DataStores, Ordered DataStores,
// MessagingService, Memory Stores, universe and place management, the Engine
// Instance API, user restrictions, groups, analytics, and a generic authenticated
// request for everything else. Same env-only key as cloud.rs. Each call works when
// the key has the matching scope and surfaces Roblox's own error otherwise, so
// capabilities are gated by the key, not by us.
//
// cloud/v2 carries value/users/attributes as JSON body fields (the content-md5 and
// roblox-entry-* headers were the old v1 API). Ordered DataStores live under a
// separate ordered-data-stores/v1 base. Shapes were confirmed against Roblox's
// published OpenAPI spec (creator-docs, content/en-us/reference/cloud/openapi.json).

use std::time::Duration;

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use reqwest::{Client, Method};
use serde_json::{json, Map, Value};

use crate::env;
use crate::httpx::{self, API_HOST};

const CLOUD: &str = "https://apis.roblox.com/cloud/v2";
const ORDERED: &str = "https://apis.roblox.com/ordered-data-stores/v1";
const DEFAULT_SCOPE: &str = "global";
// Instance reads, thumbnail renders, analytics queries, and memory-store flushes
// all come back as long-running operations; they normally finish in seconds.
const OPERATION_DEADLINE: Duration = Duration::from_secs(120);

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

fn place_id() -> Result<String, String> {
    env::var("ROBLOX_PLACE_ID").ok_or_else(|| "Missing env: set ROBLOX_PLACE_ID.".into())
}

// Operation paths are relative to cloud/v2, except a few (memory-store flush) that
// already carry the prefix; build the absolute URL either way.
fn operation_url(path: &str) -> String {
    let path = path.trim_start_matches('/');
    if path.starts_with("cloud/v2/") {
        format!("{API_HOST}/{path}")
    } else {
        format!("{CLOUD}/{path}")
    }
}

// Waits out an Operation returned inline: done already, or poll its path.
async fn resolve_operation(http: &Client, key: &str, op: Value) -> OcResult {
    if httpx::is_done(&op) {
        return httpx::finished_operation(op);
    }
    let path = op
        .get("path")
        .and_then(Value::as_str)
        .ok_or("operation response had no path")?;
    httpx::poll_operation(http, key, &operation_url(path), OPERATION_DEADLINE).await
}

fn response_of(op: Value) -> Value {
    op.get("response").cloned().unwrap_or(Value::Null)
}

// PATCH bodies here are partial resources; the updateMask names exactly the fields
// present so nothing else is touched.
fn update_mask(fields: &Map<String, Value>) -> String {
    fields.keys().cloned().collect::<Vec<_>>().join(",")
}

fn page(max: Option<i64>, token: Option<&str>) -> Vec<(&'static str, String)> {
    page_query(None, max, token)
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

// A data store entry lives in a scope. The unscoped path is the "global" scope; a
// named scope goes through the scopes collection, and "-" lists across all scopes.
fn ds_base(universe: &str, datastore: &str, scope: Option<&str>) -> String {
    match scope.filter(|s| !s.is_empty()) {
        Some(s) => format!(
            "{CLOUD}/universes/{universe}/data-stores/{}/scopes/{}",
            enc(datastore),
            enc(s)
        ),
        None => format!(
            "{CLOUD}/universes/{universe}/data-stores/{}",
            enc(datastore)
        ),
    }
}

pub async fn list_datastore_entries(
    http: &Client,
    datastore: &str,
    scope: Option<&str>,
    prefix: Option<&str>,
    max: Option<i64>,
    token: Option<&str>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!("{}/entries", ds_base(&universe, datastore, scope));
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

pub async fn get_datastore_entry(
    http: &Client,
    datastore: &str,
    scope: Option<&str>,
    entry: &str,
    revision: Option<&str>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    // A revision is addressed as `entry@revisionId`; the `@` is part of the path
    // grammar, so it is appended after encoding the two halves.
    let mut id = enc(entry);
    if let Some(rev) = revision.filter(|r| !r.is_empty()) {
        id.push('@');
        id.push_str(&enc(rev));
    }
    let url = format!("{}/entries/{id}", ds_base(&universe, datastore, scope));
    httpx::request_json(http, &key, Method::GET, &url, &[], None).await
}

pub async fn set_datastore_entry(
    http: &Client,
    datastore: &str,
    scope: Option<&str>,
    entry: &str,
    value: &Value,
    users: Option<Vec<String>>,
    attributes: Option<Value>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{}/entries/{}",
        ds_base(&universe, datastore, scope),
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

pub async fn delete_datastore_entry(
    http: &Client,
    datastore: &str,
    scope: Option<&str>,
    entry: &str,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{}/entries/{}",
        ds_base(&universe, datastore, scope),
        enc(entry)
    );
    httpx::request_json(http, &key, Method::DELETE, &url, &[], None).await
}

pub async fn increment_datastore_entry(
    http: &Client,
    datastore: &str,
    scope: Option<&str>,
    entry: &str,
    amount: i64,
    users: Option<Vec<String>>,
    attributes: Option<Value>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{}/entries/{}:increment",
        ds_base(&universe, datastore, scope),
        enc(entry)
    );
    let body = json!({ "amount": amount, "users": users.unwrap_or_default(), "attributes": attributes.unwrap_or_else(|| json!({})) });
    httpx::request_json(http, &key, Method::POST, &url, &[], Some(&body)).await
}

pub async fn list_datastore_entry_revisions(
    http: &Client,
    datastore: &str,
    scope: Option<&str>,
    entry: &str,
    max: Option<i64>,
    token: Option<&str>,
    filter: Option<&str>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{}/entries/{}:listRevisions",
        ds_base(&universe, datastore, scope),
        enc(entry)
    );
    let mut q = page_query(None, max, token);
    if let Some(f) = filter {
        q.push(("filter", f.to_string()));
    }
    httpx::request_json(http, &key, Method::GET, &url, &q, None).await
}

/// Schedules the whole data store for deletion 30 days out; undelete cancels it.
pub async fn delete_datastore(http: &Client, datastore: &str) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = ds_base(&universe, datastore, None);
    httpx::request_json(http, &key, Method::DELETE, &url, &[], None).await
}

pub async fn undelete_datastore(http: &Client, datastore: &str) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!("{}:undelete", ds_base(&universe, datastore, None));
    httpx::request_json(http, &key, Method::POST, &url, &[], Some(&json!({}))).await
}

pub async fn snapshot_datastores(http: &Client) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!("{CLOUD}/universes/{universe}/data-stores:snapshot");
    httpx::request_json(http, &key, Method::POST, &url, &[], Some(&json!({}))).await
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

pub async fn delete_ordered_entry(
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
    httpx::request_json(http, &key, Method::DELETE, &url, &[], None).await
}

/// Flushes every memory store structure in the universe (LIVE or TEST scope) and
/// waits for the operation to finish.
pub async fn flush_memory_store(http: &Client, scope: Option<&str>) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!("{CLOUD}/universes/{universe}/memory-store:flush");
    let q: Vec<(&str, String)> = scope
        .map(|s| vec![("scope", s.to_string())])
        .unwrap_or_default();
    let op = httpx::request_json(http, &key, Method::POST, &url, &q, Some(&json!({}))).await?;
    resolve_operation(http, &key, op).await
}

// ===== Universe and place management =====

pub async fn update_universe(http: &Client, fields: Map<String, Value>) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!("{CLOUD}/universes/{universe}");
    let mask = update_mask(&fields);
    httpx::request_json(
        http,
        &key,
        Method::PATCH,
        &url,
        &[("updateMask", mask)],
        Some(&Value::Object(fields)),
    )
    .await
}

pub async fn update_place(http: &Client, fields: Map<String, Value>) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!("{CLOUD}/universes/{universe}/places/{}", place_id()?);
    let mask = update_mask(&fields);
    httpx::request_json(
        http,
        &key,
        Method::PATCH,
        &url,
        &[("updateMask", mask)],
        Some(&Value::Object(fields)),
    )
    .await
}

pub async fn restart_servers(http: &Client, body: Value) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!("{CLOUD}/universes/{universe}:restartServers");
    httpx::request_json(http, &key, Method::POST, &url, &[], Some(&body)).await
}

pub async fn translate_text(
    http: &Client,
    text: &str,
    source: Option<&str>,
    targets: &[String],
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!("{CLOUD}/universes/{universe}:translateText");
    let mut body = Map::new();
    body.insert("text".into(), json!(text));
    body.insert("targetLanguageCodes".into(), json!(targets));
    if let Some(s) = source {
        body.insert("sourceLanguageCode".into(), json!(s));
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

pub async fn list_game_servers(
    http: &Client,
    version: &str,
    max: Option<i64>,
    token: Option<&str>,
    order_by: Option<&str>,
    filter: Option<&str>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{API_HOST}/server-management/v1/universes/{universe}/places/{}/versions/{}/game-servers",
        place_id()?,
        enc(version)
    );
    let mut q = page(max, token);
    if let Some(o) = order_by {
        q.push(("orderBy", o.to_string()));
    }
    if let Some(f) = filter {
        q.push(("filter", f.to_string()));
    }
    httpx::request_json(http, &key, Method::GET, &url, &q, None).await
}

pub async fn get_game_server_logs(
    http: &Client,
    version: &str,
    job_id: &str,
    max: Option<i64>,
    token: Option<&str>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{API_HOST}/server-management/v1/universes/{universe}/places/{}/versions/{}/game-servers/{}/logs",
        place_id()?,
        enc(version),
        enc(job_id)
    );
    httpx::request_json(http, &key, Method::GET, &url, &page(max, token), None).await
}

// ===== Engine Instance API (the published place, not the Studio session) =====

fn instance_url(universe: &str, place: &str, instance_id: &str) -> String {
    format!(
        "{CLOUD}/universes/{universe}/places/{place}/instances/{}",
        enc(instance_id)
    )
}

pub async fn get_instance(http: &Client, instance_id: &str) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = instance_url(&universe, &place_id()?, instance_id);
    let op = httpx::request_json(http, &key, Method::GET, &url, &[], None).await?;
    Ok(response_of(resolve_operation(http, &key, op).await?))
}

pub async fn list_instance_children(
    http: &Client,
    instance_id: &str,
    max: Option<i64>,
    token: Option<&str>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!(
        "{}:listChildren",
        instance_url(&universe, &place_id()?, instance_id)
    );
    let op = httpx::request_json(http, &key, Method::GET, &url, &page(max, token), None).await?;
    Ok(response_of(resolve_operation(http, &key, op).await?))
}

pub async fn update_instance(
    http: &Client,
    instance_id: &str,
    engine_instance: Value,
    mask: Option<&str>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = instance_url(&universe, &place_id()?, instance_id);
    let q: Vec<(&str, String)> = mask
        .map(|m| vec![("updateMask", m.to_string())])
        .unwrap_or_default();
    let body = json!({ "engineInstance": engine_instance });
    let op = httpx::request_json(http, &key, Method::PATCH, &url, &q, Some(&body)).await?;
    Ok(response_of(resolve_operation(http, &key, op).await?))
}

// ===== User restrictions (bans) =====

// A restriction is universe-wide unless a place is named. Its id is the user id.
fn restrictions_base(universe: &str, place: Option<&str>) -> String {
    match place.filter(|p| !p.is_empty()) {
        Some(p) => format!(
            "{CLOUD}/universes/{universe}/places/{}/user-restrictions",
            enc(p)
        ),
        None => format!("{CLOUD}/universes/{universe}/user-restrictions"),
    }
}

pub async fn list_user_restrictions(
    http: &Client,
    place: Option<&str>,
    max: Option<i64>,
    token: Option<&str>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = restrictions_base(&universe, place);
    httpx::request_json(http, &key, Method::GET, &url, &page(max, token), None).await
}

pub async fn get_user_restriction(http: &Client, place: Option<&str>, user_id: &str) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!("{}/{}", restrictions_base(&universe, place), enc(user_id));
    httpx::request_json(http, &key, Method::GET, &url, &[], None).await
}

pub async fn set_user_restriction(
    http: &Client,
    place: Option<&str>,
    user_id: &str,
    restriction: Map<String, Value>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!("{}/{}", restrictions_base(&universe, place), enc(user_id));
    // The whole gameJoinRestriction is replaced atomically; the API rejects masks
    // that index into it.
    httpx::request_json(
        http,
        &key,
        Method::PATCH,
        &url,
        &[("updateMask", "gameJoinRestriction".into())],
        Some(&json!({ "gameJoinRestriction": Value::Object(restriction) })),
    )
    .await
}

pub async fn list_user_restriction_logs(
    http: &Client,
    max: Option<i64>,
    token: Option<&str>,
    filter: Option<&str>,
) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let url = format!("{CLOUD}/universes/{universe}/user-restrictions:listLogs");
    let mut q = page(max, token);
    if let Some(f) = filter {
        q.push(("filter", f.to_string()));
    }
    httpx::request_json(http, &key, Method::GET, &url, &q, None).await
}

// ===== Groups =====

pub async fn list_group_collection(
    http: &Client,
    group_id: &str,
    collection: &str,
    max: Option<i64>,
    token: Option<&str>,
    filter: Option<&str>,
) -> OcResult {
    let key = key()?;
    let url = format!("{CLOUD}/groups/{}/{collection}", enc(group_id));
    let mut q = page(max, token);
    if let Some(f) = filter {
        q.push(("filter", f.to_string()));
    }
    httpx::request_json(http, &key, Method::GET, &url, &q, None).await
}

pub async fn resolve_group_join_request(
    http: &Client,
    group_id: &str,
    request_id: &str,
    accept: bool,
) -> OcResult {
    let key = key()?;
    let verb = if accept { "accept" } else { "decline" };
    let url = format!(
        "{CLOUD}/groups/{}/join-requests/{}:{verb}",
        enc(group_id),
        enc(request_id)
    );
    httpx::request_json(http, &key, Method::POST, &url, &[], Some(&json!({}))).await
}

pub async fn set_group_role(
    http: &Client,
    group_id: &str,
    membership_id: &str,
    role_id: &str,
    assign: bool,
) -> OcResult {
    let key = key()?;
    let verb = if assign { "assignRole" } else { "unassignRole" };
    let url = format!(
        "{CLOUD}/groups/{}/memberships/{}:{verb}",
        enc(group_id),
        enc(membership_id)
    );
    let role = format!("groups/{group_id}/roles/{role_id}");
    httpx::request_json(
        http,
        &key,
        Method::POST,
        &url,
        &[],
        Some(&json!({ "role": role })),
    )
    .await
}

// ===== Analytics =====

/// `kind` is "metrics" or "dimension-values"; the query body follows the Analytics
/// guide. The result is polled to completion.
pub async fn query_analytics(http: &Client, kind: &str, body: Value) -> OcResult {
    let (key, universe) = key_and_universe()?;
    let base = format!("{API_HOST}/analytics-query-api/v1/universes/{universe}");
    let op = httpx::request_json(
        http,
        &key,
        Method::POST,
        &format!("{base}/{kind}"),
        &[],
        Some(&body),
    )
    .await?;
    let done = if httpx::is_done(&op) {
        op
    } else {
        // The operation path is not documented as absolute, so poll the documented
        // operations endpoint by the operation's own id.
        let id = op
            .get("path")
            .and_then(Value::as_str)
            .and_then(|p| p.rsplit('/').next())
            .ok_or("analytics query returned no operation path")?
            .to_string();
        httpx::poll_operation(
            http,
            &key,
            &format!("{base}/operations/{kind}/{id}"),
            OPERATION_DEADLINE,
        )
        .await?
    };
    Ok(response_of(done))
}

// ===== Users =====

pub async fn generate_user_thumbnail(
    http: &Client,
    user_id: &str,
    size: Option<i64>,
    format: Option<&str>,
    shape: Option<&str>,
) -> OcResult {
    let key = key()?;
    let url = format!("{CLOUD}/users/{}:generateThumbnail", enc(user_id));
    let mut q: Vec<(&str, String)> = Vec::new();
    if let Some(s) = size {
        q.push(("size", s.to_string()));
    }
    if let Some(f) = format {
        q.push(("format", f.to_string()));
    }
    if let Some(s) = shape {
        q.push(("shape", s.to_string()));
    }
    let op = httpx::request_json(http, &key, Method::GET, &url, &q, None).await?;
    Ok(response_of(resolve_operation(http, &key, op).await?))
}

// ===== Generic authenticated request =====

/// Calls any Open Cloud endpoint under apis.roblox.com with the configured key. The
/// escape hatch for permissions without a typed tool (legacy, experimental, and new
/// APIs). `path` is host-relative, so the key cannot be sent to another host.
pub async fn open_cloud_request(
    http: &Client,
    method: &str,
    path: &str,
    query: Option<Map<String, Value>>,
    body: Option<Value>,
) -> OcResult {
    let key = key()?;
    let method: Method = method
        .to_ascii_uppercase()
        .parse()
        .map_err(|_| format!("unsupported HTTP method {method}"))?;
    if !path.starts_with('/') || path.contains("://") || path.starts_with("//") {
        return Err("path must be host-relative, for example /cloud/v2/universes/123".into());
    }
    let url = format!("{API_HOST}{path}");
    let q: Vec<(&str, String)> = query
        .as_ref()
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    let s = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    (k.as_str(), s)
                })
                .collect()
        })
        .unwrap_or_default();
    httpx::request_json(http, &key, method, &url, &q, body.as_ref()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datastore_paths_switch_on_scope() {
        assert_eq!(
            ds_base("1", "Player Data", None),
            "https://apis.roblox.com/cloud/v2/universes/1/data-stores/Player%20Data"
        );
        assert_eq!(
            ds_base("1", "ds", Some("-")),
            "https://apis.roblox.com/cloud/v2/universes/1/data-stores/ds/scopes/-"
        );
        assert_eq!(ds_base("1", "ds", Some("")), ds_base("1", "ds", None));
    }

    #[test]
    fn operation_url_keeps_an_existing_prefix() {
        assert_eq!(
            operation_url("universes/1/places/2/instances/root/operations/abc"),
            "https://apis.roblox.com/cloud/v2/universes/1/places/2/instances/root/operations/abc"
        );
        assert_eq!(
            operation_url("cloud/v2/universes/1/memory-store/operations/xyz"),
            "https://apis.roblox.com/cloud/v2/universes/1/memory-store/operations/xyz"
        );
    }

    #[test]
    fn update_mask_names_the_provided_fields() {
        let mut fields = Map::new();
        fields.insert("displayName".into(), json!("x"));
        fields.insert("serverSize".into(), json!(20));
        assert_eq!(update_mask(&fields), "displayName,serverSize");
    }

    #[test]
    fn restriction_paths_switch_on_place() {
        assert_eq!(
            restrictions_base("1", None),
            "https://apis.roblox.com/cloud/v2/universes/1/user-restrictions"
        );
        assert_eq!(
            restrictions_base("1", Some("9")),
            "https://apis.roblox.com/cloud/v2/universes/1/places/9/user-restrictions"
        );
    }
}
