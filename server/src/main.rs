// Tripwire MCP server (Rust). Speaks MCP over stdio; stdout carries the JSON-RPC
// stream, so nothing else may write to it (diagnostics go to stderr). The local
// bridge runs alongside on a fixed loopback port for the Studio plugin to long-poll.

mod assets;
mod backdoor;
mod bridge;
mod classinfo;
mod cloud;
mod env;
mod harness;
mod httpx;
mod monetization;
mod opencloud;
mod playtest;
mod publish;
mod screenshot;
mod secrets;
mod security;

use std::sync::Arc;
use std::time::Duration;

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
    ErrorData as McpError, ServerHandler, ServiceExt,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use bridge::{Bridge, BridgeResult, Role, DEFAULT_TIMEOUT};

const WRITE_BATCH_TIMEOUT: Duration = Duration::from_secs(60);
const INPUT_TIMEOUT: Duration = Duration::from_secs(40);
// `review --strict` exits with this when the reviewer found something, so CI can gate on it.
const REVIEW_FINDINGS_EXIT_CODE: i32 = 2;
// A screenshot capture waits on a Studio render, so it gets longer than the default.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(30);

fn text(body: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![Content::text(body.into())])
}

// Renders a bridge command result: pretty JSON of the data, or a clear error.
fn as_text(result: Result<BridgeResult, String>) -> CallToolResult {
    match result {
        Ok(r) if r.ok => {
            text(serde_json::to_string_pretty(&r.data).unwrap_or_else(|_| r.data.to_string()))
        }
        Ok(r) => text(format!(
            "Error: {}",
            r.error.unwrap_or_else(|| "command failed".into())
        )),
        Err(e) => text(format!("Error: {e}")),
    }
}

// Renders a captured frame as an MCP image block, or a clear error. The plugin sends
// raw RGBA; the JPEG encoding happens in screenshot::encode_capture.
fn as_image(result: Result<BridgeResult, String>) -> CallToolResult {
    match result {
        Ok(r) if r.ok => match screenshot::encode_capture(&r.data) {
            Ok(jpeg_base64) => {
                CallToolResult::success(vec![Content::image(jpeg_base64, "image/jpeg")])
            }
            Err(e) => text(format!("Error: {e}")),
        },
        Ok(r) => text(format!(
            "Error: {}",
            r.error.unwrap_or_else(|| "command failed".into())
        )),
        Err(e) => text(format!("Error: {e}")),
    }
}

// Renders an Open Cloud result: pretty JSON, or a clear error.
fn oc_text(result: Result<Value, String>) -> CallToolResult {
    match result {
        Ok(v) => text(serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string())),
        Err(e) => text(format!("Error: {e}")),
    }
}

// A free-form JSON field. serde_json::Value derives a bare `true` schema, which some
// MCP clients reject when they validate a tool's input schema; emit an explicit
// permissive schema for those fields instead.
fn any_json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(serde_json::json!({
        "type": ["object", "array", "string", "number", "boolean", "null"]
    }))
    .expect("static schema is valid")
}

// A Roblox property value is a tagged union, so the schema has to spell out the shape
// or a model will send a raw value (true, [x,y,z]) that the plugin can't decode.
fn wire_value_json() -> Value {
    json!({
        "type": "object",
        "description": "A typed Roblox value, tagged by `type`. Shapes: \
    primitive {\"type\":\"primitive\",\"value\":true|42|\"text\"}; \
    Vector3 {\"type\":\"Vector3\",\"value\":[x,y,z]}; \
    Color3 {\"type\":\"Color3\",\"value\":[r,g,b],\"rgb255\":true} (rgb255 true means 0-255, omit for 0-1); \
    UDim2 {\"type\":\"UDim2\",\"value\":[sx,ox,sy,oy]}; \
    CFrame {\"type\":\"CFrame\",\"value\":[x,y,z]} (3 numbers) or 12 numbers for orientation; \
    EnumItem {\"type\":\"EnumItem\",\"enum\":\"Material\",\"item\":\"Neon\"}; \
    instance {\"type\":\"instance\",\"path\":\"Workspace.Part\"}.",
        "properties": {
            "type": { "type": "string", "enum": ["primitive", "Vector3", "Color3", "UDim2", "CFrame", "EnumItem", "instance"] },
            "value": { "description": "payload for primitive / Vector3 / Color3 / UDim2 / CFrame" },
            "enum": { "type": "string", "description": "enum group for EnumItem, e.g. Material" },
            "item": { "type": "string", "description": "enum item for EnumItem, e.g. Neon" },
            "path": { "type": "string", "description": "instance path when type is instance" },
            "rgb255": { "type": "boolean", "description": "set true when Color3 components are 0-255" }
        },
        "required": ["type"]
    })
}

fn wire_value_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(wire_value_json()).expect("static schema is valid")
}

fn properties_json() -> Value {
    json!({
        "type": "array",
        "description": "Initial properties: a list of { name, value } where value is a typed datatype.",
        "items": {
            "type": "object",
            "properties": { "name": { "type": "string" }, "value": wire_value_json() },
            "required": ["name", "value"]
        }
    })
}

fn properties_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(properties_json()).expect("static schema is valid")
}

fn mass_create_items_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(json!({
        "type": "array",
        "description": "Instances to create: a list of { className, parentPath?, name?, properties? }.",
        "items": {
            "type": "object",
            "properties": {
                "className": { "type": "string" },
                "parentPath": { "type": "string" },
                "name": { "type": "string" },
                "properties": properties_json()
            },
            "required": ["className"]
        }
    }))
    .expect("static schema is valid")
}

fn mass_set_items_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(json!({
        "type": "array",
        "description": "Property writes: a list of { path, name, value } where value is a typed datatype.",
        "items": {
            "type": "object",
            "properties": { "path": { "type": "string" }, "name": { "type": "string" }, "value": wire_value_json() },
            "required": ["path", "name", "value"]
        }
    }))
    .expect("static schema is valid")
}

fn primitive_value_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(json!({
        "type": ["string", "number", "boolean"],
        "description": "A primitive value to match (string, number, or boolean)."
    }))
    .expect("static schema is valid")
}

// ===== tool input shapes (camelCase on the wire; absent optionals are omitted) =====

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct PingArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct StudioArgs {
    /// instanceId, a unique id prefix, or a place name.
    studio: String,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct TreeArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_depth: Option<i64>,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct PathArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ScriptPathArgs {
    path: String,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct SearchObjectsArgs {
    query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    class_name: Option<String>,
    /// Match a class and its subclasses via Instance:IsA, for example "BasePart" or "GuiObject".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    is_a: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<i64>,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct SearchByPropertyArgs {
    property: String,
    /// A primitive value (string, number, or boolean).
    #[schemars(schema_with = "primitive_value_schema")]
    value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    class_name: Option<String>,
    /// Match a class and its subclasses via Instance:IsA, for example "BasePart".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    is_a: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<i64>,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct GrepArgs {
    pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<i64>,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct LimitArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<i64>,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ClassInfoArgs {
    /// The Roblox class name, case-sensitive, for example "Part" or "Humanoid".
    class_name: String,
    /// Include members inherited from superclasses. Defaults to true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    include_inherited: Option<bool>,
    /// Keep only members of this kind: Property, Function, Event, or Callback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    member_kind: Option<String>,
    /// Keep only members whose name contains this substring (case-insensitive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name_filter: Option<String>,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
struct Vec3Arg {
    x: f64,
    y: f64,
    z: f64,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct RaycastArgs {
    /// World position the ray starts from.
    origin: Vec3Arg,
    /// World direction to cast. Its length is the ray distance unless maxDistance is set.
    direction: Vec3Arg,
    /// Cast this many studs along the direction instead of using its length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_distance: Option<f64>,
    /// Exclude this instance and its descendants from the ray, for example the character.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ignore_path: Option<String>,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct RunLuauLiveArgs {
    /// Luau to evaluate in the live playtest server. `return <expr>` to get a value back.
    code: String,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
struct OutputCursor {
    #[serde(default)]
    server: i64,
    #[serde(default)]
    client: i64,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct PlaytestOutputArgs {
    /// The `cursor` from the previous call, to fetch only newer lines. Omit for all buffered output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    since: Option<OutputCursor>,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct CreateInstanceArgs {
    class_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Initial properties: a list of { name, value } where value is a typed datatype.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "properties_schema")]
    properties: Option<Value>,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct SetPropertyArgs {
    path: String,
    name: String,
    /// A typed datatype (primitive, Vector3, Color3, UDim2, CFrame, EnumItem, or an instance path).
    #[schemars(schema_with = "wire_value_schema")]
    value: Value,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct UpdateSourceArgs {
    path: String,
    source: String,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct InsertModelArgs {
    asset_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unpack: Option<bool>,
    /// A CFrame datatype to reposition the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "wire_value_schema")]
    pivot_to: Option<Value>,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct MassCreateArgs {
    /// A list of create specs ({ className, parentPath?, name?, properties? }).
    #[schemars(schema_with = "mass_create_items_schema")]
    items: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    atomic: Option<bool>,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct MassSetArgs {
    /// A list of { path, name, value } specs.
    #[schemars(schema_with = "mass_set_items_schema")]
    items: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    atomic: Option<bool>,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct MouseArgs {
    x: f64,
    y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    button: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    action: Option<String>,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct KeyboardArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    action: Option<String>,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
struct NavArgs {
    x: f64,
    y: f64,
    z: f64,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ReviewArgs {
    /// The Rojo source tree to scan (default sample-game/src).
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct RunTestFileArgs {
    /// A spec ModuleScript name, for example 'economy.spec'.
    file: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct WriteTestArgs {
    name: String,
    source: String,
    #[serde(default)]
    dir: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct RunLuauArgs {
    /// The Luau source to execute headlessly in the published place.
    script: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct UploadAssetArgs {
    file_path: String,
    /// One of Decal, Audio, Model, Animation, Video.
    asset_type: String,
    display_name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    content_type: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct PublishPlaceArgs {
    file_path: String,
    /// 'Published' (default) or 'Saved'.
    #[serde(default)]
    version_type: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct DsListArgs {
    #[serde(default)]
    prefix: Option<String>,
    #[serde(default)]
    max_page_size: Option<i64>,
    #[serde(default)]
    page_token: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct DsEntriesArgs {
    datastore: String,
    /// Data store scope. Omit for the global scope; "-" lists entries across all scopes.
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    prefix: Option<String>,
    #[serde(default)]
    max_page_size: Option<i64>,
    #[serde(default)]
    page_token: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct DsEntryArgs {
    datastore: String,
    /// Data store scope; omit for the global scope.
    #[serde(default)]
    scope: Option<String>,
    entry: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct DsGetArgs {
    datastore: String,
    /// Data store scope; omit for the global scope.
    #[serde(default)]
    scope: Option<String>,
    entry: String,
    /// Read this revision (from list_datastore_entry_revisions) instead of the latest.
    #[serde(default)]
    revision: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct DsRevisionsArgs {
    datastore: String,
    #[serde(default)]
    scope: Option<String>,
    entry: String,
    #[serde(default)]
    max_page_size: Option<i64>,
    #[serde(default)]
    page_token: Option<String>,
    /// CEL on revision_create_time only, e.g. `revision_create_time >= "2026-01-01T00:00:00Z"`.
    #[serde(default)]
    filter: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct DsNameArgs {
    datastore: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct DsSetArgs {
    datastore: String,
    /// Data store scope; omit for the global scope.
    #[serde(default)]
    scope: Option<String>,
    entry: String,
    #[schemars(schema_with = "any_json_schema")]
    value: Value,
    #[serde(default)]
    users: Option<Vec<String>>,
    #[serde(default)]
    #[schemars(schema_with = "any_json_schema")]
    attributes: Option<Value>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct DsIncrementArgs {
    datastore: String,
    /// Data store scope; omit for the global scope.
    #[serde(default)]
    scope: Option<String>,
    entry: String,
    amount: i64,
    #[serde(default)]
    users: Option<Vec<String>>,
    #[serde(default)]
    #[schemars(schema_with = "any_json_schema")]
    attributes: Option<Value>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct OrderedListArgs {
    store: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    descending: Option<bool>,
    #[serde(default)]
    max_page_size: Option<i64>,
    #[serde(default)]
    page_token: Option<String>,
    #[serde(default)]
    filter: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct OrderedEntryArgs {
    store: String,
    #[serde(default)]
    scope: Option<String>,
    entry: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct OrderedSetArgs {
    store: String,
    #[serde(default)]
    scope: Option<String>,
    entry: String,
    value: i64,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct OrderedIncrementArgs {
    store: String,
    #[serde(default)]
    scope: Option<String>,
    entry: String,
    amount: i64,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct PublishMessageArgs {
    topic: String,
    message: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct SortedMapSetArgs {
    map: String,
    item: String,
    #[schemars(schema_with = "any_json_schema")]
    value: Value,
    #[serde(default)]
    ttl_seconds: Option<i64>,
    #[serde(default)]
    string_sort_key: Option<String>,
    #[serde(default)]
    numeric_sort_key: Option<f64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct SortedMapItemArgs {
    map: String,
    item: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct SortedMapListArgs {
    map: String,
    #[serde(default)]
    descending: Option<bool>,
    #[serde(default)]
    max_page_size: Option<i64>,
    #[serde(default)]
    page_token: Option<String>,
    #[serde(default)]
    filter: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct QueueAddArgs {
    queue: String,
    #[schemars(schema_with = "any_json_schema")]
    data: Value,
    #[serde(default)]
    priority: Option<f64>,
    #[serde(default)]
    ttl_seconds: Option<i64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct QueueReadArgs {
    queue: String,
    #[serde(default)]
    count: Option<i64>,
    #[serde(default)]
    invisibility_seconds: Option<i64>,
    #[serde(default)]
    all_or_nothing: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct QueueDiscardArgs {
    queue: String,
    read_id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct UserArgs {
    user_id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct GroupArgs {
    group_id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct InventoryArgs {
    user_id: String,
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    max_page_size: Option<i64>,
    #[serde(default)]
    page_token: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct NotificationArgs {
    user_id: String,
    message_id: String,
    #[serde(default)]
    #[schemars(schema_with = "any_json_schema")]
    parameters: Option<Value>,
    #[serde(default)]
    launch_data: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct SubscriptionArgs {
    subscription_product_id: String,
    user_id: String,
    #[serde(default)]
    full: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct FlushMemoryArgs {
    /// LIVE (default) or TEST.
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct UpdateUniverseArgs {
    #[serde(default)]
    voice_chat_enabled: Option<bool>,
    /// Robux price of private servers. Only settable when private servers are already enabled.
    #[serde(default)]
    private_server_price_robux: Option<i64>,
    #[serde(default)]
    desktop_enabled: Option<bool>,
    #[serde(default)]
    mobile_enabled: Option<bool>,
    #[serde(default)]
    tablet_enabled: Option<bool>,
    #[serde(default)]
    console_enabled: Option<bool>,
    #[serde(default)]
    vr_enabled: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct UpdatePlaceArgs {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    /// Maximum players per server.
    #[serde(default)]
    server_size: Option<i64>,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct RestartServersArgs {
    /// Restrict to these place ids; omit for every active place in the universe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    place_ids: Option<Vec<i64>>,
    /// Restart servers on the newest version too, not only outdated ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    close_all_versions: Option<bool>,
    /// Stop matchmaking into old servers and keep them up for bleedOffDurationMinutes first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bleed_off_servers: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bleed_off_duration_minutes: Option<i64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct TranslateArgs {
    text: String,
    /// IETF BCP-47 codes, for example ["es", "fr", "ja"].
    target_language_codes: Vec<String>,
    /// Omit to auto-detect.
    #[serde(default)]
    source_language_code: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct GameServersArgs {
    /// The place version number the servers run.
    version: String,
    #[serde(default)]
    max_page_size: Option<i64>,
    #[serde(default)]
    page_token: Option<String>,
    /// A field, optionally with " desc", for example "uptime desc".
    #[serde(default)]
    order_by: Option<String>,
    /// CEL over the server fields.
    #[serde(default)]
    filter: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct GameServerLogsArgs {
    version: String,
    job_id: String,
    #[serde(default)]
    max_page_size: Option<i64>,
    #[serde(default)]
    page_token: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct InstanceIdArgs {
    /// An instance id from a previous listing, or "root" for the DataModel.
    instance_id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct InstanceChildrenArgs {
    /// An instance id from a previous listing, or "root" for the DataModel.
    instance_id: String,
    #[serde(default)]
    max_page_size: Option<i64>,
    #[serde(default)]
    page_token: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct UpdateInstanceArgs {
    instance_id: String,
    /// The partial engineInstance, e.g. {"Name": "X"} or {"Details": {"Script": {"Source": "print(1)", "Enabled": true}}}.
    #[schemars(schema_with = "any_json_schema")]
    engine_instance: Value,
    /// Optional field mask, e.g. "engineInstance.Details.Script.Source".
    #[serde(default)]
    update_mask: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct RestrictionListArgs {
    /// Restrict to a place's own bans; omit for universe-wide.
    #[serde(default)]
    place_id: Option<String>,
    #[serde(default)]
    max_page_size: Option<i64>,
    #[serde(default)]
    page_token: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct RestrictionGetArgs {
    user_id: String,
    #[serde(default)]
    place_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct SetRestrictionArgs {
    user_id: String,
    /// true bans, false unbans.
    active: bool,
    /// Ban length in seconds; omit for permanent.
    #[serde(default)]
    duration_seconds: Option<i64>,
    /// Internal note, never shown to the user (max 1000 chars).
    #[serde(default)]
    private_reason: Option<String>,
    /// Shown to the user (max 400 chars).
    #[serde(default)]
    display_reason: Option<String>,
    /// Do not extend the ban to suspected alt accounts.
    #[serde(default)]
    exclude_alt_accounts: Option<bool>,
    /// Ban from one place only; omit for the whole universe.
    #[serde(default)]
    place_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct RestrictionLogsArgs {
    #[serde(default)]
    max_page_size: Option<i64>,
    #[serde(default)]
    page_token: Option<String>,
    /// CEL on user or place, e.g. `user == 'users/123'`.
    #[serde(default)]
    filter: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct SecretsListArgs {
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct SecretPutArgs {
    /// The secret name scripts will ask HttpService:GetSecret for, e.g. "discord".
    id: String,
    /// The plaintext. It is sealed with the universe public key before it leaves this machine.
    content: String,
    /// Domain wildcard the secret may be sent to, e.g. "*.discord.com".
    #[serde(default)]
    domain: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct SecretIdArgs {
    id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct AssetGetArgs {
    asset_id: String,
    /// Fields to include, e.g. "description,displayName,previews".
    #[serde(default)]
    read_mask: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct AssetIdArgs {
    asset_id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct UpdateAssetArgs {
    asset_id: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    /// New content file. Roblox only supports content replacement for Models.
    #[serde(default)]
    file_path: Option<String>,
    #[serde(default)]
    content_type: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct AssetVersionsArgs {
    asset_id: String,
    #[serde(default)]
    max_page_size: Option<i64>,
    #[serde(default)]
    page_token: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct RollbackAssetArgs {
    asset_id: String,
    /// A version number from list_asset_versions.
    version_number: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct AssetQuotasArgs {
    /// Defaults to ROBLOX_CREATOR_USER_ID.
    #[serde(default)]
    user_id: Option<String>,
    /// CEL on quotaType and assetType, e.g. `quotaType == 'RATE_LIMIT_UPLOAD' && assetType == 'Audio'`.
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    max_page_size: Option<i64>,
    #[serde(default)]
    page_token: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct DownloadAssetArgs {
    asset_id: String,
    /// Local path to write the content to.
    out_path: String,
    #[serde(default)]
    version_number: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct GrantAssetPermissionsArgs {
    asset_ids: Vec<i64>,
    /// User, Group, GroupRoleset, Universe, or All.
    subject_type: String,
    /// The user, group, roleset, or universe id; omit for All.
    #[serde(default)]
    subject_id: Option<String>,
    /// Use, Edit, Download, CopyFromRcc, or UpdateFromRcc.
    action: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ProductListArgs {
    #[serde(default)]
    page_size: Option<i64>,
    #[serde(default)]
    page_token: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ProductIdArgs {
    product_id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ProductCreateArgs {
    name: String,
    #[serde(default)]
    description: Option<String>,
    /// Robux price.
    #[serde(default)]
    price: Option<i64>,
    #[serde(default)]
    is_for_sale: Option<bool>,
    #[serde(default)]
    is_regional_pricing_enabled: Option<bool>,
    #[serde(default)]
    is_managed_pricing_enabled: Option<bool>,
    /// Icon image (.png or .jpg) to upload.
    #[serde(default)]
    image_path: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ProductUpdateArgs {
    product_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    price: Option<i64>,
    #[serde(default)]
    is_for_sale: Option<bool>,
    #[serde(default)]
    is_regional_pricing_enabled: Option<bool>,
    #[serde(default)]
    is_managed_pricing_enabled: Option<bool>,
    /// Developer products only: list it on the external store page.
    #[serde(default)]
    store_page_enabled: Option<bool>,
    #[serde(default)]
    image_path: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct SearchCreatorStoreArgs {
    /// Search terms.
    #[serde(default)]
    query: Option<String>,
    /// Model, Plugin, Audio, Decal, MeshPart, Video, or FontFamily. Required unless categoryPath is set.
    #[serde(default)]
    asset_type: Option<String>,
    #[serde(default)]
    max_page_size: Option<i64>,
    #[serde(default)]
    page_token: Option<String>,
    /// Only assets by this user.
    #[serde(default)]
    user_id: Option<i64>,
    /// Only assets by this group.
    #[serde(default)]
    group_id: Option<i64>,
    #[serde(default)]
    verified_creators_only: Option<bool>,
    /// A Creator Store category path, as an alternative to assetType.
    #[serde(default)]
    category_path: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct GroupListArgs {
    group_id: String,
    #[serde(default)]
    max_page_size: Option<i64>,
    #[serde(default)]
    page_token: Option<String>,
    /// CEL, e.g. `user == 'users/123'` or `role == 'groups/1/roles/2'`.
    #[serde(default)]
    filter: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct JoinRequestArgs {
    group_id: String,
    join_request_id: String,
    /// true accepts, false declines.
    accept: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct GroupRoleArgs {
    group_id: String,
    /// The membership id from list_group_memberships.
    membership_id: String,
    /// The role id from list_group_roles.
    role_id: String,
    /// Remove the role instead of assigning it.
    #[serde(default)]
    unassign: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct AnalyticsArgs {
    /// A metric name from the Analytics guide, e.g. "DAU", "Sessions", "AverageSessionLength".
    metric: String,
    /// OneMinute, HalfHour, OneHour, OneDay, OneWeek, OneMonth, or None.
    granularity: String,
    /// Inclusive ISO 8601 start.
    start_time: String,
    /// Exclusive ISO 8601 end.
    end_time: String,
    /// Dimensions to group by, e.g. ["Platform"].
    #[serde(default)]
    breakdown: Option<Vec<String>>,
    /// Filters as in the Analytics guide, e.g. [{"dimension": "Platform", "operator": "In", "values": ["Desktop"]}].
    #[serde(default)]
    #[schemars(schema_with = "any_json_schema")]
    filter: Option<Value>,
    #[serde(default)]
    limit: Option<i64>,
    /// Set to ask for the possible values of these dimensions instead of metric data.
    #[serde(default)]
    dimensions: Option<Vec<String>>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ThumbnailArgs {
    user_id: String,
    /// 48, 50, 60, 75, 100, 110, 150, 180, 352, 420, or 720. Default 420.
    #[serde(default)]
    size: Option<i64>,
    /// PNG (default) or JPEG.
    #[serde(default)]
    format: Option<String>,
    /// ROUND (default) or SQUARE.
    #[serde(default)]
    shape: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct OpenCloudRequestArgs {
    /// GET, POST, PATCH, PUT, or DELETE.
    method: String,
    /// Host-relative path under apis.roblox.com, e.g. "/cloud/v2/universes/123/places/456".
    path: String,
    /// Query parameters as a flat object.
    #[serde(default)]
    #[schemars(schema_with = "any_json_schema")]
    query: Option<Value>,
    /// JSON request body.
    #[serde(default)]
    #[schemars(schema_with = "any_json_schema")]
    body: Option<Value>,
}

#[derive(Clone)]
struct Tripwire {
    // Read by the generated #[tool_handler] dispatch; dead-code analysis cannot see that.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    http: reqwest::Client,
    bridge: Arc<Bridge>,
    port: u16,
}

#[tool_router]
impl Tripwire {
    fn new(bridge: Arc<Bridge>, port: u16) -> Self {
        Self {
            tool_router: Self::tool_router(),
            http: httpx::client(),
            bridge,
            port,
        }
    }

    async fn relay(
        &self,
        kind: &str,
        args: impl Serialize,
        target: Role,
        timeout: Duration,
    ) -> CallToolResult {
        let payload = serde_json::to_value(args).unwrap_or(Value::Null);
        as_text(self.bridge.send(kind, payload, target, timeout).await)
    }

    // --- connection ---

    #[tool(
        description = "Report whether a Studio plugin is connected, the active place, and any other connected studios."
    )]
    async fn studio_status(&self) -> Result<CallToolResult, McpError> {
        Ok(text(self.bridge.status_text()))
    }

    #[tool(
        description = "Round-trip a ping through the Studio plugin to confirm the live bridge works."
    )]
    async fn ping_studio(
        &self,
        Parameters(args): Parameters<PingArgs>,
    ) -> Result<CallToolResult, McpError> {
        let payload = serde_json::to_value(&args).unwrap_or(Value::Null);
        match self
            .bridge
            .send("ping", payload, Role::Plugin, DEFAULT_TIMEOUT)
            .await
        {
            Ok(r) if r.ok => Ok(text(format!("Studio replied: {}", r.data))),
            Ok(r) => Ok(text(format!("Error: {}", r.error.unwrap_or_default()))),
            Err(e) => Ok(text(format!("Error: {e}"))),
        }
    }

    #[tool(
        description = "List every connected (or recently seen) Studio: instanceId, place, connected/active, last-seen, and whether a playtest is running."
    )]
    async fn list_studios(&self) -> Result<CallToolResult, McpError> {
        Ok(text(
            serde_json::to_string_pretty(&self.bridge.list_studios()).unwrap_or_default(),
        ))
    }

    #[tool(
        description = "Choose which connected Studio subsequent tools target, by exact instanceId, a unique id prefix, or a unique place name."
    )]
    async fn set_active_studio(
        &self,
        Parameters(args): Parameters<StudioArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(match self.bridge.set_active_studio(&args.studio) {
            Ok(msg) => text(msg),
            Err(e) => text(format!("Error: {e}")),
        })
    }

    // --- read and inspect ---

    #[tool(
        description = "List the instance tree from a path (default the whole game), bounded by depth. Read-only."
    )]
    async fn get_file_tree(
        &self,
        Parameters(a): Parameters<TreeArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("get_file_tree", a, Role::Plugin, DEFAULT_TIMEOUT)
            .await)
    }

    #[tool(
        description = "List the direct children (name and className) of the instance at a path. Read-only."
    )]
    async fn get_instance_children(
        &self,
        Parameters(a): Parameters<PathArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("get_instance_children", a, Role::Plugin, DEFAULT_TIMEOUT)
            .await)
    }

    #[tool(
        description = "Read an instance's name, className, full path, attributes, and a curated set of common engine properties (Position, Size, Color, Material, Anchored, Transparency, Text, and so on, whichever the class has). Read-only."
    )]
    async fn get_instance_properties(
        &self,
        Parameters(a): Parameters<PathArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("get_instance_properties", a, Role::Plugin, DEFAULT_TIMEOUT)
            .await)
    }

    #[tool(
        description = "Find instances whose name contains a query, optionally filtered by exact className or by class-and-subclasses with isA (for example isA \"BasePart\" matches Part, WedgePart, MeshPart). Read-only."
    )]
    async fn search_objects(
        &self,
        Parameters(a): Parameters<SearchObjectsArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("search_objects", a, Role::Plugin, DEFAULT_TIMEOUT)
            .await)
    }

    #[tool(
        description = "Find instances whose property equals a primitive value, optionally filtered by exact className or by class-and-subclasses with isA. Read-only."
    )]
    async fn search_by_property(
        &self,
        Parameters(a): Parameters<SearchByPropertyArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("search_by_property", a, Role::Plugin, DEFAULT_TIMEOUT)
            .await)
    }

    #[tool(description = "Read the source of a Script, LocalScript, or ModuleScript. Read-only.")]
    async fn get_script_source(
        &self,
        Parameters(a): Parameters<ScriptPathArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("get_script_source", a, Role::Plugin, DEFAULT_TIMEOUT)
            .await)
    }

    #[tool(
        description = "Search script sources for a substring; returns path, line, and line text. Read-only."
    )]
    async fn grep_scripts(
        &self,
        Parameters(a): Parameters<GrepArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("grep_scripts", a, Role::Plugin, DEFAULT_TIMEOUT)
            .await)
    }

    #[tool(
        description = "Return recent Studio output log entries (message, type, timestamp). Read-only."
    )]
    async fn get_output_log(
        &self,
        Parameters(a): Parameters<LimitArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("get_output_log", a, Role::Plugin, DEFAULT_TIMEOUT)
            .await)
    }

    #[tool(description = "List the instances currently selected in Studio. Read-only.")]
    async fn get_selection(&self) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("get_selection", json!({}), Role::Plugin, DEFAULT_TIMEOUT)
            .await)
    }

    #[tool(
        description = "Look up a Roblox class's members (properties, methods, events) with their types, inherited members folded in by default. Answered from a bundled API reflection dump, so it needs no Studio and no key. Read-only."
    )]
    async fn get_class_info(
        &self,
        Parameters(a): Parameters<ClassInfoArgs>,
    ) -> Result<CallToolResult, McpError> {
        let include_inherited = a.include_inherited.unwrap_or(true);
        Ok(
            match classinfo::class_info(
                &a.class_name,
                include_inherited,
                a.member_kind.as_deref(),
                a.name_filter.as_deref(),
            ) {
                Ok(report) => text(report),
                Err(e) => text(format!("Error: {e}")),
            },
        )
    }

    #[tool(
        description = "Cast a ray through the world and report the first hit: instance, position, surface normal, material, and distance, or no hit. Give an origin and a direction (its length is the ray distance, or set maxDistance). Optionally exclude an instance subtree (for example the character). Read-only."
    )]
    async fn raycast(
        &self,
        Parameters(a): Parameters<RaycastArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("raycast", a, Role::Plugin, DEFAULT_TIMEOUT)
            .await)
    }

    #[tool(
        description = "Report the world-space bounding box (center and size) of a Model or a BasePart at a path. Useful for sizing and placing things. Read-only."
    )]
    async fn get_bounding_box(
        &self,
        Parameters(a): Parameters<PathArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("get_bounding_box", a, Role::Plugin, DEFAULT_TIMEOUT)
            .await)
    }

    #[tool(
        description = "List the SpawnLocations under a path (default the whole game): position, whether each is enabled, and whether it is neutral. Read-only."
    )]
    async fn find_spawns(
        &self,
        Parameters(a): Parameters<PathArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("find_spawns", a, Role::Plugin, DEFAULT_TIMEOUT)
            .await)
    }

    #[tool(
        description = "Capture the Studio viewport and return it as a JPEG image, so you can see the scene. Needs the plugin connected and Game Settings > Security > Allow Mesh / Image APIs enabled. Edit mode only."
    )]
    async fn capture_screenshot(&self) -> Result<CallToolResult, McpError> {
        Ok(as_image(
            self.bridge
                .send(
                    "capture_screenshot",
                    json!({}),
                    Role::Plugin,
                    CAPTURE_TIMEOUT,
                )
                .await,
        ))
    }

    // --- edit (one undo step each) ---

    #[tool(
        description = "Create an instance of a class under a parent path (default the whole game), with an optional name and initial properties. One undo step."
    )]
    async fn create_instance(
        &self,
        Parameters(a): Parameters<CreateInstanceArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("create_instance", a, Role::Plugin, DEFAULT_TIMEOUT)
            .await)
    }

    #[tool(
        description = "Destroy the instance at the given path and its descendants. One undo step."
    )]
    async fn delete_instance(
        &self,
        Parameters(a): Parameters<ScriptPathArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("delete_instance", a, Role::Plugin, DEFAULT_TIMEOUT)
            .await)
    }

    #[tool(
        description = "Set one typed property on the instance at the given path. One undo step."
    )]
    async fn set_property(
        &self,
        Parameters(a): Parameters<SetPropertyArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("set_property", a, Role::Plugin, DEFAULT_TIMEOUT)
            .await)
    }

    #[tool(
        description = "Replace the source of a Script, LocalScript, or ModuleScript via the script editor."
    )]
    async fn update_script_source(
        &self,
        Parameters(a): Parameters<UpdateSourceArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("update_script_source", a, Role::Plugin, DEFAULT_TIMEOUT)
            .await)
    }

    #[tool(
        description = "Insert an asset by id under a parent path (default Workspace). method 'load_asset' (owned/Roblox) or 'load_asset_async' (public free, needs the place setting). Optional name, pivotTo, unpack. One undo step."
    )]
    async fn insert_model(
        &self,
        Parameters(a): Parameters<InsertModelArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("insert_model", a, Role::Plugin, WRITE_BATCH_TIMEOUT)
            .await)
    }

    #[tool(
        description = "Create many instances in one undo step. atomic:true rolls all back on any failure; otherwise best-effort with per-item results."
    )]
    async fn mass_create(
        &self,
        Parameters(a): Parameters<MassCreateArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("mass_create", a, Role::Plugin, WRITE_BATCH_TIMEOUT)
            .await)
    }

    #[tool(
        description = "Set one property on each of many instances in one undo step. atomic:true rolls all back on any failure; otherwise best-effort."
    )]
    async fn mass_set_property(
        &self,
        Parameters(a): Parameters<MassSetArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("mass_set_property", a, Role::Plugin, WRITE_BATCH_TIMEOUT)
            .await)
    }

    // --- playtest and input ---

    #[tool(
        description = "Start an F5 playtest (server and client DataModels with a player); injects the in-play runner. Live Studio only."
    )]
    async fn start_playtest(&self) -> Result<CallToolResult, McpError> {
        Ok(as_text(
            playtest::start_playtest(&self.bridge, self.port).await,
        ))
    }

    #[tool(
        description = "Stop an F5 playtest. Best-effort: F5 teardown can outlast the confirmation window."
    )]
    async fn stop_playtest(&self) -> Result<CallToolResult, McpError> {
        Ok(as_text(playtest::stop_playtest(&self.bridge).await))
    }

    #[tool(
        description = "Start an F8 run (server-only simulation, no client peer or player). Live Studio only."
    )]
    async fn start_simulation(&self) -> Result<CallToolResult, McpError> {
        Ok(as_text(
            playtest::start_simulation(&self.bridge, self.port).await,
        ))
    }

    #[tool(
        description = "Stop an F8 run. The server runner calls EndTest; a clean, reliable stop."
    )]
    async fn stop_simulation(&self) -> Result<CallToolResult, McpError> {
        Ok(as_text(playtest::stop_simulation(&self.bridge).await))
    }

    #[tool(
        description = "Simulate a mouse click or move at screen coordinates during an F5 playtest. action 'click' presses and releases; 'move' just moves the cursor."
    )]
    async fn simulate_mouse_input(
        &self,
        Parameters(a): Parameters<MouseArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("mouse_input", a, Role::Server, INPUT_TIMEOUT)
            .await)
    }

    #[tool(
        description = "Simulate keyboard input during an F5 playtest: a key by KeyCode name with action tap/press/release, or typed text."
    )]
    async fn simulate_keyboard_input(
        &self,
        Parameters(a): Parameters<KeyboardArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("keyboard_input", a, Role::Server, INPUT_TIMEOUT)
            .await)
    }

    #[tool(
        description = "Walk the local player's character toward a world position during an F5 playtest. Returns whether it reached the goal."
    )]
    async fn character_navigation(
        &self,
        Parameters(a): Parameters<NavArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .relay("character_navigation", a, Role::Server, INPUT_TIMEOUT)
            .await)
    }

    #[tool(
        description = "Return the running playtest's output log, aggregated across the server and client peers and tagged by peer. Pass the `cursor` from the previous call as `since` to get only new lines; the buffer is bounded, so this no longer grows without limit. Use reset_playtest_output to clear it."
    )]
    async fn get_playtest_output(
        &self,
        Parameters(a): Parameters<PlaytestOutputArgs>,
    ) -> Result<CallToolResult, McpError> {
        let since = a.since.unwrap_or_default();
        Ok(as_text(
            playtest::get_playtest_output(&self.bridge, since.server, since.client).await,
        ))
    }

    #[tool(
        description = "Clear the running playtest's output buffers (server and client) so the next get_playtest_output starts fresh, without restarting Studio."
    )]
    async fn reset_playtest_output(&self) -> Result<CallToolResult, McpError> {
        Ok(as_text(playtest::reset_playtest_output(&self.bridge).await))
    }

    #[tool(
        description = "Evaluate Luau in the LIVE F5 playtest server and return the result, so you can inspect the running game (a Humanoid's state, an NPC's position, a path's waypoints) without adding a print and replaying. Use `return <expr>` to get a value. Needs an active playtest and ServerScriptService.LoadStringEnabled enabled in the test place. Distinct from run_luau, which runs headless via Open Cloud against the published place."
    )]
    async fn run_luau_live(
        &self,
        Parameters(a): Parameters<RunLuauLiveArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(as_text(
            playtest::run_luau_live(&self.bridge, &a.code).await,
        ))
    }

    // --- headless execution (Open Cloud) ---

    #[tool(
        description = "Run a Luau script headlessly in the configured place via Open Cloud Luau Execution; returns return values and logs."
    )]
    async fn run_luau(
        &self,
        Parameters(a): Parameters<RunLuauArgs>,
    ) -> Result<CallToolResult, McpError> {
        let creds = match env::cloud_creds() {
            Ok(creds) => creds,
            Err(e) => return Ok(text(format!("FAILED: {e}"))),
        };
        Ok(text(format_luau(
            &cloud::run_luau(&self.http, &creds, &a.script).await,
        )))
    }

    #[tool(
        description = "Run the headless test suite in the published place via Open Cloud and report passed/failed with failure messages."
    )]
    async fn run_tests(&self) -> Result<CallToolResult, McpError> {
        let creds = match env::cloud_creds() {
            Ok(creds) => creds,
            Err(e) => return Ok(text(format!("Harness error: {e}"))),
        };
        Ok(text(harness::format_outcome(
            &harness::run_tests(&self.http, &creds).await,
        )))
    }

    #[tool(
        description = "Run a single spec by its ModuleScript name (for example 'economy.spec') headlessly via Open Cloud."
    )]
    async fn run_test_file(
        &self,
        Parameters(a): Parameters<RunTestFileArgs>,
    ) -> Result<CallToolResult, McpError> {
        let creds = match env::cloud_creds() {
            Ok(creds) => creds,
            Err(e) => return Ok(text(format!("Harness error: {e}"))),
        };
        Ok(text(harness::format_outcome(
            &harness::run_test_file(&self.http, &creds, &a.file).await,
        )))
    }

    #[tool(
        description = "List the spec files and their cases discovered in the published place. Runs no tests."
    )]
    async fn list_tests(&self) -> Result<CallToolResult, McpError> {
        let creds = match env::cloud_creds() {
            Ok(creds) => creds,
            Err(e) => return Ok(text(format!("Error: {e}"))),
        };
        Ok(match harness::list_tests(&self.http, &creds).await {
            Ok(specs) => text(harness::format_test_list(&specs)),
            Err(e) => text(format!("Error: {e}")),
        })
    }

    #[tool(
        description = "Write a roblox-ts test spec to disk as <name>.spec.ts (default sample-game/src/shared). Rebuild and publish, then run_tests picks it up."
    )]
    async fn write_test(
        &self,
        Parameters(a): Parameters<WriteTestArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(
            match harness::write_test(&a.name, &a.source, a.dir.as_deref()) {
                Ok(path) => text(format!(
                    "Wrote {path}. Rebuild (rbxtsc) and publish the place, then run_tests."
                )),
                Err(e) => text(format!("Error: {e}")),
            },
        )
    }

    #[tool(
        description = "Upload a local file as a Roblox asset via Open Cloud (Decal, Audio, Model, Animation, or Video) and return its assetId. Needs the assets scope and ROBLOX_CREATOR_USER_ID/GROUP_ID."
    )]
    async fn upload_asset(
        &self,
        Parameters(a): Parameters<UploadAssetArgs>,
    ) -> Result<CallToolResult, McpError> {
        let result = assets::upload_asset(
            &self.http,
            &a.file_path,
            &a.asset_type,
            &a.display_name,
            a.description.as_deref(),
            a.content_type.as_deref(),
        )
        .await;
        Ok(match result {
            Ok(v) => text(format!(
                "Uploaded. assetId: {}, revisionId: {}",
                v.get("assetId").unwrap_or(&Value::Null),
                v.get("revisionId").unwrap_or(&Value::Null)
            )),
            Err(e) => text(format!("Error: {e}")),
        })
    }

    #[tool(
        description = "Publish a local place file (.rbxl/.rbxlx) as a new version of the configured experience via Open Cloud (universe-places write scope). Conflicts if Studio holds the place open."
    )]
    async fn publish_place(
        &self,
        Parameters(a): Parameters<PublishPlaceArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(
            match publish::publish_place(&self.http, &a.file_path, a.version_type.as_deref()).await
            {
                Ok(v) => text(format!(
                    "Published version {}.",
                    v.get("versionNumber").unwrap_or(&Value::Null)
                )),
                Err(e) => text(format!("Error: {e}")),
            },
        )
    }

    // --- security review (static, no key) ---

    #[tool(
        description = "Review a Rojo source tree (default sample-game/src) for client-trust and unvalidated-remote issues, each with a suggested server-side fix."
    )]
    async fn review_security(
        &self,
        Parameters(a): Parameters<ReviewArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(
            match security::review(a.path.as_deref().unwrap_or("sample-game/src")) {
                Ok(report) => text(security::format_report(&report)),
                Err(e) => text(format!("Error: {e}")),
            },
        )
    }

    #[tool(
        description = "List RemoteEvent and RemoteFunction server handlers in a Rojo source tree (default sample-game/src), with the client-controlled parameters of each."
    )]
    async fn scan_remotes(
        &self,
        Parameters(a): Parameters<ReviewArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(
            match security::review(a.path.as_deref().unwrap_or("sample-game/src")) {
                Ok(report) => {
                    let rows: Vec<Value> = report.handlers.iter().map(|h| json!({ "file": h.file, "line": h.line, "remote": h.remote, "hook": h.hook, "clientParams": h.client_params })).collect();
                    text(serde_json::to_string_pretty(&rows).unwrap_or_default())
                }
                Err(e) => text(format!("Error: {e}")),
            },
        )
    }

    #[tool(
        description = "Scan a Rojo source tree (default sample-game/src) for client-trust holes: server handlers that use client-supplied values without validating them."
    )]
    async fn scan_client_trust(
        &self,
        Parameters(a): Parameters<ReviewArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(
            match security::review(a.path.as_deref().unwrap_or("sample-game/src")) {
                Ok(report) => {
                    let rows: Vec<Value> = report.findings.iter().map(|f| json!({ "file": f.file, "line": f.line, "remote": f.remote, "param": f.param, "severity": f.severity, "issue": f.issue, "fix": f.fix })).collect();
                    text(serde_json::to_string_pretty(&rows).unwrap_or_default())
                }
                Err(e) => text(format!("Error: {e}")),
            },
        )
    }

    #[tool(
        description = "Scan the live place's scripts for free-model backdoor patterns: runtime code execution (loadstring), environment tampering (getfenv/setfenv), fetching code over HTTP (HttpGet), require by asset id, and obfuscated payloads. Reads script source through the plugin (default the whole game), so it is worth running after insert_model. Read-only."
    )]
    async fn scan_backdoors(
        &self,
        Parameters(a): Parameters<PathArgs>,
    ) -> Result<CallToolResult, McpError> {
        let payload = json!({ "keywords": backdoor::PREFILTER_KEYWORDS, "path": a.path });
        Ok(
            match self
                .bridge
                .send("scan_backdoors", payload, Role::Plugin, DEFAULT_TIMEOUT)
                .await
            {
                Ok(r) if r.ok => text(backdoor::scan_collected(&r.data)),
                Ok(r) => text(format!("Error: {}", r.error.unwrap_or_default())),
                Err(e) => text(format!("Error: {e}")),
            },
        )
    }

    // --- Open Cloud: data stores, messaging, memory stores, platform, engagement ---

    #[tool(
        description = "List the standard data stores in the configured universe (scope universe-datastores.control:list). Optional name prefix and pagination."
    )]
    async fn list_datastores(
        &self,
        Parameters(a): Parameters<DsListArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::list_datastores(
                &self.http,
                a.prefix.as_deref(),
                a.max_page_size,
                a.page_token.as_deref(),
            )
            .await,
        ))
    }

    #[tool(
        description = "List entry keys in a data store (scope universe-datastores.objects:list). Keys only; read a value with get_datastore_entry."
    )]
    async fn list_datastore_entries(
        &self,
        Parameters(a): Parameters<DsEntriesArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::list_datastore_entries(
                &self.http,
                &a.datastore,
                a.scope.as_deref(),
                a.prefix.as_deref(),
                a.max_page_size,
                a.page_token.as_deref(),
            )
            .await,
        ))
    }

    #[tool(
        description = "Read a data store entry's value and metadata (scope universe-datastores.objects:read). Pass revision to read an older version (universe-datastores.versions:read)."
    )]
    async fn get_datastore_entry(
        &self,
        Parameters(a): Parameters<DsGetArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::get_datastore_entry(
                &self.http,
                &a.datastore,
                a.scope.as_deref(),
                &a.entry,
                a.revision.as_deref(),
            )
            .await,
        ))
    }

    #[tool(
        description = "Create or overwrite a data store entry (upsert). value is any JSON; users/attributes are cleared if omitted, so pass them when you set them."
    )]
    async fn set_datastore_entry(
        &self,
        Parameters(a): Parameters<DsSetArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::set_datastore_entry(
                &self.http,
                &a.datastore,
                a.scope.as_deref(),
                &a.entry,
                &a.value,
                a.users,
                a.attributes,
            )
            .await,
        ))
    }

    #[tool(
        description = "Soft-delete a data store entry (scope universe-datastores.objects:delete); it is purged after 30 days."
    )]
    async fn delete_datastore_entry(
        &self,
        Parameters(a): Parameters<DsEntryArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::delete_datastore_entry(
                &self.http,
                &a.datastore,
                a.scope.as_deref(),
                &a.entry,
            )
            .await,
        ))
    }

    #[tool(
        description = "Atomically add an integer to a numeric data store entry; creates it if missing. The existing value must be an integer."
    )]
    async fn increment_datastore_entry(
        &self,
        Parameters(a): Parameters<DsIncrementArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::increment_datastore_entry(
                &self.http,
                &a.datastore,
                a.scope.as_deref(),
                &a.entry,
                a.amount,
                a.users,
                a.attributes,
            )
            .await,
        ))
    }

    #[tool(
        description = "List ordered data store entries by value (scope universe.ordered-data-store.scope.entry:read). descending sorts high to low. filter is a numeric range like 'entry >= 10 && entry <= 50'."
    )]
    async fn list_ordered_entries(
        &self,
        Parameters(a): Parameters<OrderedListArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::list_ordered_entries(
                &self.http,
                &a.store,
                a.scope.as_deref(),
                a.descending.unwrap_or(false),
                a.max_page_size,
                a.page_token.as_deref(),
                a.filter.as_deref(),
            )
            .await,
        ))
    }

    #[tool(description = "Read one ordered data store entry's integer value.")]
    async fn get_ordered_entry(
        &self,
        Parameters(a): Parameters<OrderedEntryArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::get_ordered_entry(&self.http, &a.store, a.scope.as_deref(), &a.entry).await,
        ))
    }

    #[tool(
        description = "Set (overwrite/upsert) an ordered data store entry to a non-negative integer."
    )]
    async fn set_ordered_entry(
        &self,
        Parameters(a): Parameters<OrderedSetArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::set_ordered_entry(
                &self.http,
                &a.store,
                a.scope.as_deref(),
                &a.entry,
                a.value,
            )
            .await,
        ))
    }

    #[tool(
        description = "Atomically add an integer to an ordered data store entry; the result must stay non-negative."
    )]
    async fn increment_ordered_entry(
        &self,
        Parameters(a): Parameters<OrderedIncrementArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::increment_ordered_entry(
                &self.http,
                &a.store,
                a.scope.as_deref(),
                &a.entry,
                a.amount,
            )
            .await,
        ))
    }

    #[tool(
        description = "Publish a message to a MessagingService topic in the universe (scope universe-messaging-service:publish). Reaches running production servers; no read side. topic <= 80 chars, message <= 1 KiB."
    )]
    async fn publish_message(
        &self,
        Parameters(a): Parameters<PublishMessageArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::publish_message(&self.http, &a.topic, &a.message).await,
        ))
    }

    #[tool(
        description = "Set (upsert) a Memory Store sorted-map item (scope memory-store.sorted-map:write). value is any JSON; ttlSeconds sets expiry; stringSortKey/numericSortKey set the order."
    )]
    async fn memory_sorted_map_set(
        &self,
        Parameters(a): Parameters<SortedMapSetArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::memory_sorted_map_set(
                &self.http,
                &a.map,
                &a.item,
                &a.value,
                a.ttl_seconds,
                a.string_sort_key.as_deref(),
                a.numeric_sort_key,
            )
            .await,
        ))
    }

    #[tool(
        description = "Read a Memory Store sorted-map item (scope memory-store.sorted-map:read)."
    )]
    async fn memory_sorted_map_get(
        &self,
        Parameters(a): Parameters<SortedMapItemArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::memory_sorted_map_get(&self.http, &a.map, &a.item).await,
        ))
    }

    #[tool(
        description = "List Memory Store sorted-map items in sort order (scope memory-store.sorted-map:read). descending reverses; filter is a CEL range over id/sortKey."
    )]
    async fn memory_sorted_map_list(
        &self,
        Parameters(a): Parameters<SortedMapListArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::memory_sorted_map_list(
                &self.http,
                &a.map,
                a.descending.unwrap_or(false),
                a.max_page_size,
                a.page_token.as_deref(),
                a.filter.as_deref(),
            )
            .await,
        ))
    }

    #[tool(
        description = "Delete a Memory Store sorted-map item (scope memory-store.sorted-map:write)."
    )]
    async fn memory_sorted_map_delete(
        &self,
        Parameters(a): Parameters<SortedMapItemArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::memory_sorted_map_delete(&self.http, &a.map, &a.item).await,
        ))
    }

    #[tool(
        description = "Add an item to a Memory Store queue (scope memory-store.queue:add). data is any JSON; higher priority dequeues first; ttlSeconds sets expiry."
    )]
    async fn memory_queue_add(
        &self,
        Parameters(a): Parameters<QueueAddArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::memory_queue_add(&self.http, &a.queue, &a.data, a.priority, a.ttl_seconds)
                .await,
        ))
    }

    #[tool(
        description = "Read items from a Memory Store queue (scope memory-store.queue:dequeue); returns a readId. Pass it to memory_queue_discard before the invisibility window elapses."
    )]
    async fn memory_queue_read(
        &self,
        Parameters(a): Parameters<QueueReadArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::memory_queue_read(
                &self.http,
                &a.queue,
                a.count,
                a.invisibility_seconds,
                a.all_or_nothing.unwrap_or(false),
            )
            .await,
        ))
    }

    #[tool(
        description = "Permanently remove the items from a memory_queue_read batch (scope memory-store.queue:discard), using its readId."
    )]
    async fn memory_queue_discard(
        &self,
        Parameters(a): Parameters<QueueDiscardArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::memory_queue_discard(&self.http, &a.queue, &a.read_id).await,
        ))
    }

    #[tool(
        description = "Get the configured universe's metadata (name, visibility, owner, root place, etc.)."
    )]
    async fn get_universe(&self) -> Result<CallToolResult, McpError> {
        Ok(oc_text(opencloud::get_universe(&self.http).await))
    }

    #[tool(description = "Get the configured place's metadata (name, server size, etc.).")]
    async fn get_place(&self) -> Result<CallToolResult, McpError> {
        Ok(oc_text(opencloud::get_place(&self.http).await))
    }

    #[tool(
        description = "Get a user's public profile. idVerified and social profiles need the user.advanced:read / user.social:read scopes."
    )]
    async fn get_user(
        &self,
        Parameters(a): Parameters<UserArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(opencloud::get_user(&self.http, &a.user_id).await))
    }

    #[tool(description = "Get a group's metadata (name, owner, member count, etc.).")]
    async fn get_group(
        &self,
        Parameters(a): Parameters<GroupArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(opencloud::get_group(&self.http, &a.group_id).await))
    }

    #[tool(
        description = "List a user's inventory items (scope user.inventory-item:read; also gated by the user's inventory privacy). filter selects types or ids."
    )]
    async fn list_inventory(
        &self,
        Parameters(a): Parameters<InventoryArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::list_inventory(
                &self.http,
                &a.user_id,
                a.filter.as_deref(),
                a.max_page_size,
                a.page_token.as_deref(),
            )
            .await,
        ))
    }

    #[tool(
        description = "Send an experience notification to a user (scope user.user-notification:write). messageId is a Creator Dashboard template; parameters fills its placeholders. One per user per day per experience."
    )]
    async fn send_notification(
        &self,
        Parameters(a): Parameters<NotificationArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::send_notification(
                &self.http,
                &a.user_id,
                &a.message_id,
                a.parameters,
                a.launch_data.as_deref(),
            )
            .await,
        ))
    }

    #[tool(
        description = "Read a user's subscription to a subscription product. userId is the subscriber. full returns state and timestamps."
    )]
    async fn get_subscription(
        &self,
        Parameters(a): Parameters<SubscriptionArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::get_subscription(
                &self.http,
                &a.subscription_product_id,
                &a.user_id,
                a.full.unwrap_or(false),
            )
            .await,
        ))
    }

    // --- Open Cloud: data store history and lifecycle ---

    #[tool(
        description = "List an entry's revisions, newest first (scope universe-datastores.versions:list). Each revision id can be passed to get_datastore_entry."
    )]
    async fn list_datastore_entry_revisions(
        &self,
        Parameters(a): Parameters<DsRevisionsArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::list_datastore_entry_revisions(
                &self.http,
                &a.datastore,
                a.scope.as_deref(),
                &a.entry,
                a.max_page_size,
                a.page_token.as_deref(),
                a.filter.as_deref(),
            )
            .await,
        ))
    }

    #[tool(
        description = "Schedule a whole data store for deletion in 30 days (scope universe-datastores.control:delete). Reversible with undelete_datastore until then. Affects live player data."
    )]
    async fn delete_datastore(
        &self,
        Parameters(a): Parameters<DsNameArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::delete_datastore(&self.http, &a.datastore).await,
        ))
    }

    #[tool(
        description = "Cancel a pending data store deletion (scope universe-datastores.control:delete)."
    )]
    async fn undelete_datastore(
        &self,
        Parameters(a): Parameters<DsNameArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::undelete_datastore(&self.http, &a.datastore).await,
        ))
    }

    #[tool(
        description = "Take a snapshot of every data store so the next write to each key keeps a versioned backup (scope universe-datastores.control:snapshot). One per experience per UTC day; run it before a risky migration."
    )]
    async fn snapshot_datastores(&self) -> Result<CallToolResult, McpError> {
        Ok(oc_text(opencloud::snapshot_datastores(&self.http).await))
    }

    #[tool(
        description = "Delete an ordered data store entry (scope universe.ordered-data-store.scope.entry:write)."
    )]
    async fn delete_ordered_entry(
        &self,
        Parameters(a): Parameters<OrderedEntryArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::delete_ordered_entry(&self.http, &a.store, a.scope.as_deref(), &a.entry)
                .await,
        ))
    }

    #[tool(
        description = "Flush every Memory Store structure in the universe and wait for it (scope memory-store:flush). scope LIVE (default) wipes production state; TEST is the Studio test scope."
    )]
    async fn flush_memory_store(
        &self,
        Parameters(a): Parameters<FlushMemoryArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::flush_memory_store(&self.http, a.scope.as_deref()).await,
        ))
    }

    // --- Open Cloud: universe and place management ---

    #[tool(
        description = "Update the configured universe's settings (scope universe:write): voice chat, private server price, and per-platform join toggles. Only the fields you pass change. Name and description are set through update_place on the root place."
    )]
    async fn update_universe(
        &self,
        Parameters(a): Parameters<UpdateUniverseArgs>,
    ) -> Result<CallToolResult, McpError> {
        let mut fields = serde_json::Map::new();
        let bools = [
            ("voiceChatEnabled", a.voice_chat_enabled),
            ("desktopEnabled", a.desktop_enabled),
            ("mobileEnabled", a.mobile_enabled),
            ("tabletEnabled", a.tablet_enabled),
            ("consoleEnabled", a.console_enabled),
            ("vrEnabled", a.vr_enabled),
        ];
        for (name, value) in bools {
            if let Some(v) = value {
                fields.insert(name.into(), json!(v));
            }
        }
        if let Some(p) = a.private_server_price_robux {
            fields.insert("privateServerPriceRobux".into(), json!(p));
        }
        if fields.is_empty() {
            return Ok(text("Error: pass at least one field to update."));
        }
        Ok(oc_text(
            opencloud::update_universe(&self.http, fields).await,
        ))
    }

    #[tool(
        description = "Update the configured place's name, description, or server size (scope universe.place:write). Fails with 409 while the place is open in an active Team Create session."
    )]
    async fn update_place(
        &self,
        Parameters(a): Parameters<UpdatePlaceArgs>,
    ) -> Result<CallToolResult, McpError> {
        let mut fields = serde_json::Map::new();
        if let Some(n) = a.display_name {
            fields.insert("displayName".into(), json!(n));
        }
        if let Some(d) = a.description {
            fields.insert("description".into(), json!(d));
        }
        if let Some(size) = a.server_size {
            fields.insert("serverSize".into(), json!(size));
        }
        if fields.is_empty() {
            return Ok(text("Error: pass at least one field to update."));
        }
        Ok(oc_text(opencloud::update_place(&self.http, fields).await))
    }

    #[tool(
        description = "Restart the universe's live servers so players move to the newest published version (scope universe:write). By default only outdated servers restart. This kicks real players; use bleedOffServers to drain instead."
    )]
    async fn restart_servers(
        &self,
        Parameters(a): Parameters<RestartServersArgs>,
    ) -> Result<CallToolResult, McpError> {
        let body = serde_json::to_value(&a).unwrap_or_else(|_| json!({}));
        Ok(oc_text(opencloud::restart_servers(&self.http, body).await))
    }

    #[tool(
        description = "Translate text into one or more languages with Roblox's translation service (scope universe:write). Returns a map of language code to translation."
    )]
    async fn translate_text(
        &self,
        Parameters(a): Parameters<TranslateArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::translate_text(
                &self.http,
                &a.text,
                a.source_language_code.as_deref(),
                &a.target_language_codes,
            )
            .await,
        ))
    }

    #[tool(
        description = "List the live game servers running a place version, with player counts, uptime, and job ids (scope universe:read). Beta API."
    )]
    async fn list_game_servers(
        &self,
        Parameters(a): Parameters<GameServersArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::list_game_servers(
                &self.http,
                &a.version,
                a.max_page_size,
                a.page_token.as_deref(),
                a.order_by.as_deref(),
                a.filter.as_deref(),
            )
            .await,
        ))
    }

    #[tool(
        description = "Read a live game server's log lines by job id (scope universe:read). Beta API. The production counterpart of get_playtest_output."
    )]
    async fn get_game_server_logs(
        &self,
        Parameters(a): Parameters<GameServerLogsArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::get_game_server_logs(
                &self.http,
                &a.version,
                &a.job_id,
                a.max_page_size,
                a.page_token.as_deref(),
            )
            .await,
        ))
    }

    // --- Open Cloud: Engine Instance API (the published place, no Studio needed) ---

    #[tool(
        description = "Read an instance in the published place via the Engine Instance API (scope universe.place.instance:read). Distinct from get_instance_properties, which reads the open Studio session. Beta: only Folder and script classes expose details."
    )]
    async fn cloud_get_instance(
        &self,
        Parameters(a): Parameters<InstanceIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::get_instance(&self.http, &a.instance_id).await,
        ))
    }

    #[tool(
        description = "List an instance's children in the published place (scope universe.place.instance:read). Start from instanceId \"root\". Returns each child's id, name, and class."
    )]
    async fn cloud_list_instance_children(
        &self,
        Parameters(a): Parameters<InstanceChildrenArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::list_instance_children(
                &self.http,
                &a.instance_id,
                a.max_page_size,
                a.page_token.as_deref(),
            )
            .await,
        ))
    }

    #[tool(
        description = "Update an instance in the published place (scope universe.place.instance:write): rename it, or set a Script/LocalScript/ModuleScript's Source, Enabled, or RunContext. Writes to the published place, not the Studio session or the Rojo tree."
    )]
    async fn cloud_update_instance(
        &self,
        Parameters(a): Parameters<UpdateInstanceArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::update_instance(
                &self.http,
                &a.instance_id,
                a.engine_instance,
                a.update_mask.as_deref(),
            )
            .await,
        ))
    }

    // --- Open Cloud: user restrictions (bans) ---

    #[tool(
        description = "List users who have ever been banned from the universe, or from one place (scope universe.user-restriction:read)."
    )]
    async fn list_user_restrictions(
        &self,
        Parameters(a): Parameters<RestrictionListArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::list_user_restrictions(
                &self.http,
                a.place_id.as_deref(),
                a.max_page_size,
                a.page_token.as_deref(),
            )
            .await,
        ))
    }

    #[tool(
        description = "Read one user's ban state and reasons (scope universe.user-restriction:read)."
    )]
    async fn get_user_restriction(
        &self,
        Parameters(a): Parameters<RestrictionGetArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::get_user_restriction(&self.http, a.place_id.as_deref(), &a.user_id).await,
        ))
    }

    #[tool(
        description = "Ban or unban a user from the universe or one place (scope universe.user-restriction:write). active=true with an optional duration in seconds bans (omit duration for permanent); active=false lifts it. Kicks the user from live servers."
    )]
    async fn set_user_restriction(
        &self,
        Parameters(a): Parameters<SetRestrictionArgs>,
    ) -> Result<CallToolResult, McpError> {
        let mut restriction = serde_json::Map::new();
        restriction.insert("active".into(), json!(a.active));
        if let Some(d) = a.duration_seconds {
            restriction.insert("duration".into(), json!(format!("{d}s")));
        }
        if let Some(r) = a.private_reason {
            restriction.insert("privateReason".into(), json!(r));
        }
        if let Some(r) = a.display_reason {
            restriction.insert("displayReason".into(), json!(r));
        }
        if let Some(x) = a.exclude_alt_accounts {
            restriction.insert("excludeAltAccounts".into(), json!(x));
        }
        Ok(oc_text(
            opencloud::set_user_restriction(
                &self.http,
                a.place_id.as_deref(),
                &a.user_id,
                restriction,
            )
            .await,
        ))
    }

    #[tool(
        description = "List the audit log of ban and unban changes across the universe (scope universe.user-restriction:read)."
    )]
    async fn list_user_restriction_logs(
        &self,
        Parameters(a): Parameters<RestrictionLogsArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::list_user_restriction_logs(
                &self.http,
                a.max_page_size,
                a.page_token.as_deref(),
                a.filter.as_deref(),
            )
            .await,
        ))
    }

    // --- Open Cloud: secrets store ---

    #[tool(
        description = "List the universe's secrets, metadata only (scope universe.secret:read). Secret values are never returned."
    )]
    async fn list_secrets(
        &self,
        Parameters(a): Parameters<SecretsListArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            secrets::list_secrets(&self.http, a.limit, a.cursor.as_deref()).await,
        ))
    }

    #[tool(
        description = "Create a secret for HttpService:GetSecret (scope universe.secret:write). The content is encrypted locally with the universe's public key (libsodium sealed box) before upload. Max 500 per universe."
    )]
    async fn create_secret(
        &self,
        Parameters(a): Parameters<SecretPutArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            secrets::put_secret(&self.http, &a.id, &a.content, a.domain.as_deref(), false).await,
        ))
    }

    #[tool(
        description = "Replace an existing secret's content or domain (scope universe.secret:write). Encrypted locally before upload."
    )]
    async fn update_secret(
        &self,
        Parameters(a): Parameters<SecretPutArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            secrets::put_secret(&self.http, &a.id, &a.content, a.domain.as_deref(), true).await,
        ))
    }

    #[tool(
        description = "Permanently delete a secret (scope universe.secret:write). Irreversible."
    )]
    async fn delete_secret(
        &self,
        Parameters(a): Parameters<SecretIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(secrets::delete_secret(&self.http, &a.id).await))
    }

    // --- Open Cloud: asset management ---

    #[tool(
        description = "Read an asset's metadata: type, name, description, moderation state, current revision (scope asset:read)."
    )]
    async fn get_asset(
        &self,
        Parameters(a): Parameters<AssetGetArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            assets::get_asset(&self.http, &a.asset_id, a.read_mask.as_deref()).await,
        ))
    }

    #[tool(
        description = "Update an asset's display name or description, or (Models only) upload new content as a new version (scope asset:write)."
    )]
    async fn update_asset(
        &self,
        Parameters(a): Parameters<UpdateAssetArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            assets::update_asset(
                &self.http,
                &a.asset_id,
                a.display_name.as_deref(),
                a.description.as_deref(),
                a.file_path.as_deref(),
                a.content_type.as_deref(),
            )
            .await,
        ))
    }

    #[tool(
        description = "List an asset's versions with their moderation state (scope asset:read)."
    )]
    async fn list_asset_versions(
        &self,
        Parameters(a): Parameters<AssetVersionsArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            assets::list_asset_versions(
                &self.http,
                &a.asset_id,
                a.max_page_size,
                a.page_token.as_deref(),
            )
            .await,
        ))
    }

    #[tool(
        description = "Roll an asset back to an earlier version number (scope asset:write). Creates a new version whose content is the old one."
    )]
    async fn rollback_asset_version(
        &self,
        Parameters(a): Parameters<RollbackAssetArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            assets::rollback_asset_version(&self.http, &a.asset_id, &a.version_number).await,
        ))
    }

    #[tool(
        description = "Archive an asset so it disappears from the site and stops loading in experiences (scope asset:write). restore_asset reverses it."
    )]
    async fn archive_asset(
        &self,
        Parameters(a): Parameters<AssetIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            assets::set_asset_archived(&self.http, &a.asset_id, true).await,
        ))
    }

    #[tool(description = "Restore an archived asset (scope asset:write).")]
    async fn restore_asset(
        &self,
        Parameters(a): Parameters<AssetIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            assets::set_asset_archived(&self.http, &a.asset_id, false).await,
        ))
    }

    #[tool(
        description = "List a user's asset upload quotas and how much of each is used (scope asset:read). Defaults to ROBLOX_CREATOR_USER_ID."
    )]
    async fn list_asset_quotas(
        &self,
        Parameters(a): Parameters<AssetQuotasArgs>,
    ) -> Result<CallToolResult, McpError> {
        let user = match a.user_id.or_else(|| env::var("ROBLOX_CREATOR_USER_ID")) {
            Some(u) => u,
            None => return Ok(text("Error: pass userId or set ROBLOX_CREATOR_USER_ID.")),
        };
        Ok(oc_text(
            assets::list_asset_quotas(
                &self.http,
                &user,
                a.filter.as_deref(),
                a.max_page_size,
                a.page_token.as_deref(),
            )
            .await,
        ))
    }

    #[tool(
        description = "Download an asset's content (a model .rbxm, an image, a place file) to a local path via the asset delivery API (scope legacy-asset:manage). The key must be allowed to read the asset."
    )]
    async fn download_asset(
        &self,
        Parameters(a): Parameters<DownloadAssetArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            assets::download_asset(
                &self.http,
                &a.asset_id,
                &a.out_path,
                a.version_number.as_deref(),
            )
            .await,
        ))
    }

    #[tool(
        description = "Grant a user, group, roleset, universe, or everyone a permission (Use, Edit, Download, CopyFromRcc, UpdateFromRcc) on a batch of assets you own (scope asset-permissions:write). Use=Universe is how an experience gets to load your private audio or mesh."
    )]
    async fn grant_asset_permissions(
        &self,
        Parameters(a): Parameters<GrantAssetPermissionsArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            assets::grant_asset_permissions(
                &self.http,
                &a.asset_ids,
                &a.subject_type,
                a.subject_id.as_deref(),
                &a.action,
            )
            .await,
        ))
    }

    // --- Open Cloud: monetization ---

    #[tool(
        description = "List the universe's developer products with prices and sale state (scope developer-product:read)."
    )]
    async fn list_developer_products(
        &self,
        Parameters(a): Parameters<ProductListArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            monetization::list_products(
                &self.http,
                monetization::Product::DeveloperProduct,
                a.page_size,
                a.page_token.as_deref(),
            )
            .await,
        ))
    }

    #[tool(
        description = "Read one developer product's configuration (scope developer-product:read)."
    )]
    async fn get_developer_product(
        &self,
        Parameters(a): Parameters<ProductIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            monetization::get_product(
                &self.http,
                monetization::Product::DeveloperProduct,
                &a.product_id,
            )
            .await,
        ))
    }

    #[tool(
        description = "Create a developer product (scope developer-product:write): name, description, Robux price, sale state, optional icon image. Returns the productId scripts pass to MarketplaceService."
    )]
    async fn create_developer_product(
        &self,
        Parameters(a): Parameters<ProductCreateArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            monetization::save_product(
                &self.http,
                monetization::Product::DeveloperProduct,
                None,
                product_fields(
                    Some(a.name),
                    a.description,
                    a.price,
                    a.is_for_sale,
                    a.is_regional_pricing_enabled,
                    a.is_managed_pricing_enabled,
                    None,
                    a.image_path,
                ),
            )
            .await,
        ))
    }

    #[tool(
        description = "Update a developer product's fields; only the ones you pass change (scope developer-product:write)."
    )]
    async fn update_developer_product(
        &self,
        Parameters(a): Parameters<ProductUpdateArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            monetization::save_product(
                &self.http,
                monetization::Product::DeveloperProduct,
                Some(&a.product_id),
                product_fields(
                    a.name,
                    a.description,
                    a.price,
                    a.is_for_sale,
                    a.is_regional_pricing_enabled,
                    a.is_managed_pricing_enabled,
                    a.store_page_enabled,
                    a.image_path,
                ),
            )
            .await,
        ))
    }

    #[tool(
        description = "List the universe's game passes with prices and sale state (scope game-pass:read)."
    )]
    async fn list_game_passes(
        &self,
        Parameters(a): Parameters<ProductListArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            monetization::list_products(
                &self.http,
                monetization::Product::GamePass,
                a.page_size,
                a.page_token.as_deref(),
            )
            .await,
        ))
    }

    #[tool(description = "Read one game pass's configuration (scope game-pass:read).")]
    async fn get_game_pass(
        &self,
        Parameters(a): Parameters<ProductIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            monetization::get_product(&self.http, monetization::Product::GamePass, &a.product_id)
                .await,
        ))
    }

    #[tool(
        description = "Create a game pass (scope game-pass:write): name, description, Robux price, sale state, optional icon image. Returns the gamePassId."
    )]
    async fn create_game_pass(
        &self,
        Parameters(a): Parameters<ProductCreateArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            monetization::save_product(
                &self.http,
                monetization::Product::GamePass,
                None,
                product_fields(
                    Some(a.name),
                    a.description,
                    a.price,
                    a.is_for_sale,
                    a.is_regional_pricing_enabled,
                    a.is_managed_pricing_enabled,
                    None,
                    a.image_path,
                ),
            )
            .await,
        ))
    }

    #[tool(
        description = "Update a game pass's fields; only the ones you pass change (scope game-pass:write)."
    )]
    async fn update_game_pass(
        &self,
        Parameters(a): Parameters<ProductUpdateArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            monetization::save_product(
                &self.http,
                monetization::Product::GamePass,
                Some(&a.product_id),
                product_fields(
                    a.name,
                    a.description,
                    a.price,
                    a.is_for_sale,
                    a.is_regional_pricing_enabled,
                    a.is_managed_pricing_enabled,
                    None,
                    a.image_path,
                ),
            )
            .await,
        ))
    }

    // --- Open Cloud: Creator Store ---

    #[tool(
        description = "Search the Creator Store for models, plugins, audio, decals, meshes, video, or fonts (scope creator-store-product:read). Returns asset ids you can pass to insert_model or download_asset."
    )]
    async fn search_creator_store(
        &self,
        Parameters(a): Parameters<SearchCreatorStoreArgs>,
    ) -> Result<CallToolResult, McpError> {
        let mut q: Vec<(&str, String)> = Vec::new();
        if let Some(v) = a.query {
            q.push(("query", v));
        }
        if let Some(v) = a.asset_type {
            q.push(("searchCategoryType", v));
        }
        if let Some(v) = a.category_path {
            q.push(("categoryPath", v));
        }
        if let Some(v) = a.max_page_size {
            q.push(("maxPageSize", v.to_string()));
        }
        if let Some(v) = a.page_token {
            q.push(("pageToken", v));
        }
        if let Some(v) = a.user_id {
            q.push(("userId", v.to_string()));
        }
        if let Some(v) = a.group_id {
            q.push(("groupId", v.to_string()));
        }
        if let Some(v) = a.verified_creators_only {
            q.push(("includeOnlyVerifiedCreators", v.to_string()));
        }
        Ok(oc_text(
            monetization::search_creator_store(&self.http, q).await,
        ))
    }

    #[tool(
        description = "Read a Creator Store asset's listing: creator, votes, price, and asset details (scope creator-store-product:read)."
    )]
    async fn get_creator_store_asset(
        &self,
        Parameters(a): Parameters<AssetIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            monetization::get_creator_store_asset(&self.http, &a.asset_id).await,
        ))
    }

    #[tool(
        description = "Read one of your Creator Store products: base and purchase price, published state, restrictions (scope creator-store-product:read). Create or update a product with open_cloud_request on /cloud/v2/creator-store-products."
    )]
    async fn get_creator_store_product(
        &self,
        Parameters(a): Parameters<ProductIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            monetization::get_creator_store_product(&self.http, &a.product_id).await,
        ))
    }

    // --- Open Cloud: groups ---

    #[tool(
        description = "List a group's members with their roles; filter by user or role (public data, no scope needed beyond the key)."
    )]
    async fn list_group_memberships(
        &self,
        Parameters(a): Parameters<GroupListArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::list_group_collection(
                &self.http,
                &a.group_id,
                "memberships",
                a.max_page_size,
                a.page_token.as_deref(),
                a.filter.as_deref(),
            )
            .await,
        ))
    }

    #[tool(
        description = "List a group's roles with rank and, where the key allows, permissions (scope group:read)."
    )]
    async fn list_group_roles(
        &self,
        Parameters(a): Parameters<GroupListArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::list_group_collection(
                &self.http,
                &a.group_id,
                "roles",
                a.max_page_size,
                a.page_token.as_deref(),
                a.filter.as_deref(),
            )
            .await,
        ))
    }

    #[tool(
        description = "List pending requests to join a group (scope group:read). Filter by user with `user == 'users/123'`."
    )]
    async fn list_group_join_requests(
        &self,
        Parameters(a): Parameters<GroupListArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::list_group_collection(
                &self.http,
                &a.group_id,
                "join-requests",
                a.max_page_size,
                a.page_token.as_deref(),
                a.filter.as_deref(),
            )
            .await,
        ))
    }

    #[tool(description = "Accept or decline a group join request (scope group:write).")]
    async fn resolve_group_join_request(
        &self,
        Parameters(a): Parameters<JoinRequestArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::resolve_group_join_request(
                &self.http,
                &a.group_id,
                &a.join_request_id,
                a.accept,
            )
            .await,
        ))
    }

    #[tool(
        description = "Assign a role to a group member, or remove one with unassign=true (scope group:write). The key's owner must outrank the role."
    )]
    async fn set_group_role(
        &self,
        Parameters(a): Parameters<GroupRoleArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::set_group_role(
                &self.http,
                &a.group_id,
                &a.membership_id,
                &a.role_id,
                !a.unassign.unwrap_or(false),
            )
            .await,
        ))
    }

    // --- Open Cloud: analytics and users ---

    #[tool(
        description = "Query the universe's analytics: a metric over a time range at a granularity, optionally broken down and filtered by dimension (scope universe.analytics:read). Pass dimensions to list a dimension's possible values instead. Waits for the query to finish."
    )]
    async fn query_analytics(
        &self,
        Parameters(a): Parameters<AnalyticsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let mut body = serde_json::Map::new();
        body.insert("metric".into(), json!(a.metric));
        body.insert("granularity".into(), json!(a.granularity));
        body.insert("startTime".into(), json!(a.start_time));
        body.insert("endTime".into(), json!(a.end_time));
        if let Some(f) = a.filter {
            body.insert("filter".into(), f);
        }
        if let Some(l) = a.limit {
            body.insert("limit".into(), json!(l));
        }
        let kind = match a.dimensions {
            Some(dims) => {
                body.insert("dimensions".into(), json!(dims));
                "dimension-values"
            }
            None => {
                if let Some(b) = a.breakdown {
                    body.insert("breakdown".into(), json!(b));
                }
                "metrics"
            }
        };
        Ok(oc_text(
            opencloud::query_analytics(&self.http, kind, Value::Object(body)).await,
        ))
    }

    #[tool(
        description = "Render a user's avatar headshot and return its image URL (no extra scope). Size, PNG or JPEG, round or square."
    )]
    async fn generate_user_thumbnail(
        &self,
        Parameters(a): Parameters<ThumbnailArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::generate_user_thumbnail(
                &self.http,
                &a.user_id,
                a.size,
                a.format.as_deref(),
                a.shape.as_deref(),
            )
            .await,
        ))
    }

    // --- Open Cloud: everything else ---

    #[tool(
        description = "Call any Open Cloud endpoint under apis.roblox.com with the configured key: method, host-relative path, query, JSON body. Covers the permissions without a typed tool (legacy badges, localization, team create, game events, thumbnails, ads, place version history, creator store products, and new APIs). Consult https://create.roblox.com/docs/cloud/reference for the path and scope; Roblox's own error comes back when the key lacks it."
    )]
    async fn open_cloud_request(
        &self,
        Parameters(a): Parameters<OpenCloudRequestArgs>,
    ) -> Result<CallToolResult, McpError> {
        let query = match a.query {
            Some(Value::Object(m)) => Some(m),
            Some(Value::Null) | None => None,
            Some(_) => return Ok(text("Error: query must be a flat JSON object.")),
        };
        Ok(oc_text(
            opencloud::open_cloud_request(&self.http, &a.method, &a.path, query, a.body).await,
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn product_fields(
    name: Option<String>,
    description: Option<String>,
    price: Option<i64>,
    is_for_sale: Option<bool>,
    is_regional_pricing_enabled: Option<bool>,
    is_managed_pricing_enabled: Option<bool>,
    store_page_enabled: Option<bool>,
    image_path: Option<String>,
) -> monetization::ProductFields {
    monetization::ProductFields {
        name,
        description,
        price,
        is_for_sale,
        is_regional_pricing_enabled,
        is_managed_pricing_enabled,
        store_page_enabled,
        image_path,
    }
}

fn format_luau(result: &cloud::LuauResult) -> String {
    if result.ok {
        let returned = serde_json::to_string(&result.results).unwrap_or_else(|_| "[]".to_string());
        let mut out = format!("OK\nreturn: {returned}");
        if !result.logs.is_empty() {
            out.push_str("\nlogs:\n");
            out.push_str(&result.logs.join("\n"));
        }
        out
    } else {
        let mut out = format!(
            "FAILED: {}",
            result.error.as_deref().unwrap_or("unknown error")
        );
        if !result.logs.is_empty() {
            out.push('\n');
            out.push_str(&result.logs.join("\n"));
        }
        out
    }
}

#[tool_handler]
impl ServerHandler for Tripwire {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::V_2024_11_05;
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        // from_build_env() reports the rmcp crate, not this binary; identify as Tripwire.
        let mut server_info = Implementation::from_build_env();
        server_info.name = "tripwire-server".into();
        server_info.version = env!("CARGO_PKG_VERSION").into();
        info.server_info = server_info;
        info.instructions = Some("Tripwire drives Roblox Studio and Roblox Open Cloud.".into());
        info
    }
}

// Loads a .env: the current directory and its ancestors, plus the repo root relative
// to the binary, so credentials are present however the server is launched. Existing
// process env still wins.
fn load_env() {
    dotenvy::dotenv().ok();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(root) = exe.ancestors().nth(4) {
            let _ = dotenvy::from_path(root.join(".env"));
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // One-shot CLI mode for CI: `tripwire-server review [path] [--strict] [--json]` prints
    // the security report and exits, instead of starting the MCP server. --json emits a
    // machine-readable report; --strict exits non-zero when there are findings so a CI
    // step can fail the build, not only comment.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("review") {
        let strict = args.iter().any(|a| a == "--strict");
        let json = args.iter().any(|a| a == "--json");
        let path = args
            .iter()
            .skip(2)
            .find(|a| !a.starts_with("--"))
            .map(String::as_str)
            .unwrap_or("sample-game/src");
        match security::review(path) {
            Ok(report) => {
                if json {
                    println!("{}", security::format_report_json(&report));
                } else {
                    println!("{}", security::format_report(&report));
                }
                if strict && !report.findings.is_empty() {
                    std::process::exit(REVIEW_FINDINGS_EXIT_CODE);
                }
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    load_env();
    let port = env::bridge_port();
    let bridge = Bridge::new();

    let app = bridge.clone().router();
    let bridge_for_serve = bridge.clone();
    tokio::spawn(async move {
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => {
                bridge_for_serve.set_ready(true);
                if let Err(err) = axum::serve(listener, app).await {
                    eprintln!("[tripwire] bridge server error: {err}");
                    bridge_for_serve.set_ready(false);
                }
            }
            Err(err) => {
                bridge_for_serve.set_error(format!(
                    "bridge port {port} is already in use, most likely by another Tripwire server. Close the other one, then reconnect; Studio tools are unavailable until then. ({err})"
                ));
            }
        }
    });

    let service = Tripwire::new(bridge, port).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
