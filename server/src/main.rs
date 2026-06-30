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
mod opencloud;
mod playtest;
mod publish;
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
    entry: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct DsSetArgs {
    datastore: String,
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
        description = "Read an instance's name, className, full path, and attributes. Read-only."
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
        description = "Find instances whose name contains a query, optionally filtered by className. Read-only."
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
        description = "Find instances whose property equals a primitive value, optionally filtered by className. Read-only."
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
        description = "Return the running playtest's output log, aggregated across the server and client peers and tagged by peer."
    )]
    async fn get_playtest_output(&self) -> Result<CallToolResult, McpError> {
        Ok(as_text(playtest::get_playtest_output(&self.bridge).await))
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
                a.prefix.as_deref(),
                a.max_page_size,
                a.page_token.as_deref(),
            )
            .await,
        ))
    }

    #[tool(
        description = "Read a data store entry's value and metadata (scope universe-datastores.objects:read)."
    )]
    async fn get_datastore_entry(
        &self,
        Parameters(a): Parameters<DsEntryArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(oc_text(
            opencloud::get_datastore_entry(&self.http, &a.datastore, &a.entry).await,
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
            opencloud::delete_datastore_entry(&self.http, &a.datastore, &a.entry).await,
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
