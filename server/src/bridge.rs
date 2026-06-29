// The local long-poll bridge the Studio plugin and injected runners talk to.
//
// One HTTP server on a fixed loopback port. A per-Studio registry is keyed by the
// plugin's session instanceId; within a Studio, commands route by peer role
// (plugin / server / client) through per-role queues, each with a Notify that wakes
// a parked /poll. Liveness is a lazy last-seen check stamped on every poll, so
// studio_status self-heals; a poll for an unknown studio gets 205 so the peer
// re-announces after a server restart. The plugin is the only HTTP client for the
// "plugin" role; the in-play client is reached by the server runner over a relay,
// so no client peer ever polls here.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{oneshot, Notify};

pub const PROTOCOL_VERSION: i64 = 2;
const POLL_PARK: Duration = Duration::from_secs(20);
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const STALE: Duration = Duration::from_secs(45);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Plugin,
    Server,
    Client,
}

impl Role {
    fn parse(value: Option<&str>) -> Role {
        match value {
            Some("server") => Role::Server,
            Some("client") => Role::Client,
            _ => Role::Plugin,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Role::Plugin => "plugin",
            Role::Server => "server",
            Role::Client => "client",
        }
    }
}

#[derive(Clone, Serialize)]
struct Command {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    payload: Value,
}

pub struct BridgeResult {
    pub ok: bool,
    pub data: Value,
    pub error: Option<String>,
}

struct PeerChannel {
    queue: VecDeque<Command>,
    notify: Arc<Notify>,
}

impl PeerChannel {
    fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            notify: Arc::new(Notify::new()),
        }
    }
}

struct Studio {
    instance_id: String,
    install_id: Option<String>,
    place_name: String,
    place_id: Option<Value>,
    user_id: Option<Value>,
    connected_at: Instant,
    channels: HashMap<Role, PeerChannel>,
    peers: HashMap<Role, Instant>,
}

impl Studio {
    fn new(instance_id: String, place_name: String) -> Self {
        Self {
            instance_id,
            install_id: None,
            place_name,
            place_id: None,
            user_id: None,
            connected_at: Instant::now(),
            channels: HashMap::new(),
            peers: HashMap::new(),
        }
    }
}

fn live(studio: &Studio, role: Role) -> bool {
    studio
        .peers
        .get(&role)
        .map(|seen| seen.elapsed() < STALE)
        .unwrap_or(false)
}

struct Inner {
    studios: HashMap<String, Studio>,
    pending: HashMap<String, oneshot::Sender<BridgeResult>>,
    active_id: Option<String>,
    last_error: Option<String>,
}

impl Inner {
    // The active Studio if live, else the most-recently-connected live one (adopted).
    fn resolve_active(&mut self) -> Option<String> {
        if let Some(id) = self.active_id.clone() {
            if self
                .studios
                .get(&id)
                .map(|s| live(s, Role::Plugin))
                .unwrap_or(false)
            {
                return Some(id);
            }
        }
        let best = self
            .studios
            .values()
            .filter(|s| live(s, Role::Plugin))
            .max_by_key(|s| s.connected_at)
            .map(|s| s.instance_id.clone());
        if let Some(id) = &best {
            self.active_id = Some(id.clone());
        }
        best
    }
}

pub struct Bridge {
    inner: Mutex<Inner>,
    counter: AtomicU64,
    ready: AtomicBool,
}

impl Bridge {
    pub fn new() -> Arc<Bridge> {
        Arc::new(Bridge {
            inner: Mutex::new(Inner {
                studios: HashMap::new(),
                pending: HashMap::new(),
                active_id: None,
                last_error: None,
            }),
            counter: AtomicU64::new(0),
            ready: AtomicBool::new(false),
        })
    }

    pub fn router(self: Arc<Self>) -> Router {
        Router::new()
            .route("/hello", post(handle_hello))
            .route("/poll", get(handle_poll))
            .route("/result", post(handle_result))
            .with_state(self)
    }

    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::SeqCst);
    }

    pub fn set_error(&self, message: String) {
        self.inner.lock().unwrap().last_error = Some(message);
    }

    /// Queues a command for the active Studio's target peer and waits for its result.
    /// Fails fast (rather than waiting out the timeout) when no Studio is connected or
    /// the target peer is not live.
    pub async fn send(
        &self,
        kind: &str,
        payload: Value,
        target: Role,
        timeout: Duration,
    ) -> Result<BridgeResult, String> {
        let request_id = format!("req-{}", self.counter.fetch_add(1, Ordering::Relaxed));
        let rx = {
            let mut inner = self.inner.lock().unwrap();
            if !self.ready.load(Ordering::SeqCst) {
                return Err(inner
                    .last_error
                    .clone()
                    .unwrap_or_else(|| "Tripwire bridge is not running.".into()));
            }
            let active_id = inner.resolve_active().ok_or_else(|| {
                "No Studio connected. Open Studio, click the Tripwire button, and enable Allow HTTP Requests.".to_string()
            })?;
            {
                let studio = inner
                    .studios
                    .get(&active_id)
                    .expect("resolved studio exists");
                if target == Role::Plugin {
                    if !live(studio, Role::Plugin) {
                        return Err(stale_message(studio));
                    }
                } else if !live(studio, target) {
                    return Err(format!(
                        "No live {} peer for '{}'. Start a playtest first.",
                        target.as_str(),
                        studio.place_name
                    ));
                }
            }
            let (tx, rx) = oneshot::channel();
            inner.pending.insert(request_id.clone(), tx);
            let studio = inner
                .studios
                .get_mut(&active_id)
                .expect("resolved studio exists");
            let channel = studio
                .channels
                .entry(target)
                .or_insert_with(PeerChannel::new);
            channel.queue.push_back(Command {
                id: request_id.clone(),
                kind: kind.to_string(),
                payload,
            });
            let notify = channel.notify.clone();
            drop(inner);
            notify.notify_one();
            rx
        };

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => Err("bridge closed before a reply".into()),
            Err(_) => {
                let mut inner = self.inner.lock().unwrap();
                inner.pending.remove(&request_id);
                if let Some(active_id) = inner.active_id.clone() {
                    if let Some(studio) = inner.studios.get_mut(&active_id) {
                        if let Some(channel) = studio.channels.get_mut(&target) {
                            channel.queue.retain(|c| c.id != request_id);
                        }
                    }
                }
                Err(format!(
                    "Tripwire bridge: command '{kind}' to {} timed out after {}ms.",
                    target.as_str(),
                    timeout.as_millis()
                ))
            }
        }
    }

    pub fn status_text(&self) -> String {
        let mut inner = self.inner.lock().unwrap();
        match inner.resolve_active() {
            None => inner
                .last_error
                .clone()
                .map(|e| format!("Not connected: {e}"))
                .unwrap_or_else(|| {
                    "No Studio connected. Open Studio, click the Tripwire button, and enable Allow HTTP Requests.".into()
                }),
            Some(active) => {
                let others = inner
                    .studios
                    .values()
                    .filter(|s| s.instance_id != active && live(s, Role::Plugin))
                    .count();
                let name = &inner.studios.get(&active).unwrap().place_name;
                if others > 0 {
                    format!("Connected. Active place: {name} (+{others} other connected studio{})", if others == 1 { "" } else { "s" })
                } else {
                    format!("Connected. Active place: {name}")
                }
            }
        }
    }

    pub fn list_studios(&self) -> Value {
        let inner = self.inner.lock().unwrap();
        let mut rows: Vec<Value> = inner
            .studios
            .values()
            .map(|s| {
                let last_seen = s
                    .peers
                    .get(&Role::Plugin)
                    .map(|t| t.elapsed().as_millis())
                    .unwrap_or(u128::MAX);
                json!({
                    "instanceId": s.instance_id,
                    "placeName": s.place_name,
                    "placeId": s.place_id,
                    "userId": s.user_id,
                    "connected": live(s, Role::Plugin),
                    "lastSeenMsAgo": last_seen,
                    "active": inner.active_id.as_deref() == Some(s.instance_id.as_str()),
                    "playtestActive": live(s, Role::Server) || live(s, Role::Client),
                })
            })
            .collect();
        rows.sort_by_key(|r| {
            r.get("lastSeenMsAgo")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX)
        });
        Value::Array(rows)
    }

    pub fn set_active_studio(&self, identifier: &str) -> Result<String, String> {
        let mut inner = self.inner.lock().unwrap();
        let live_ids: Vec<(String, String)> = inner
            .studios
            .values()
            .filter(|s| live(s, Role::Plugin))
            .map(|s| (s.instance_id.clone(), s.place_name.clone()))
            .collect();
        if live_ids.is_empty() {
            return Err("no Studio is currently connected".into());
        }
        let mut matches: Vec<&(String, String)> =
            live_ids.iter().filter(|(id, _)| id == identifier).collect();
        if matches.is_empty() {
            matches = live_ids
                .iter()
                .filter(|(id, _)| id.starts_with(identifier))
                .collect();
        }
        if matches.is_empty() {
            matches = live_ids
                .iter()
                .filter(|(_, name)| name == identifier)
                .collect();
        }
        match matches.as_slice() {
            [] => Err(format!("no connected studio matches '{identifier}'")),
            [(id, name)] => {
                let short = id.chars().take(8).collect::<String>();
                inner.active_id = Some(id.clone());
                Ok(format!("active studio set to {name} ({short})"))
            }
            many => Err(format!(
                "'{identifier}' matches {} studios; be more specific",
                many.len()
            )),
        }
    }
}

fn stale_message(studio: &Studio) -> String {
    let ago = studio
        .peers
        .get(&Role::Plugin)
        .map(|t| format!("{}s ago", t.elapsed().as_secs()))
        .unwrap_or_else(|| "never".into());
    format!(
        "active Studio '{}' last seen {ago}; click the Tripwire button and enable Allow HTTP Requests.",
        studio.place_name
    )
}

#[derive(Deserialize)]
struct HelloInfo {
    #[serde(rename = "protocolVersion")]
    protocol_version: Option<i64>,
    #[serde(rename = "instanceId")]
    instance_id: Option<String>,
    role: Option<String>,
    #[serde(rename = "installId")]
    install_id: Option<String>,
    #[serde(rename = "placeName")]
    place_name: Option<String>,
    #[serde(rename = "placeId")]
    place_id: Option<Value>,
    #[serde(rename = "userId")]
    user_id: Option<Value>,
}

async fn handle_hello(State(bridge): State<Arc<Bridge>>, Json(info): Json<HelloInfo>) -> Response {
    if info.protocol_version != Some(PROTOCOL_VERSION) {
        let msg = format!(
            "protocol mismatch: peer sent {}, server speaks {PROTOCOL_VERSION}. Rebuild and reinstall the plugin.",
            info.protocol_version.map(|v| v.to_string()).unwrap_or_else(|| "none".into())
        );
        bridge.inner.lock().unwrap().last_error = Some(msg.clone());
        return (
            StatusCode::CONFLICT,
            Json(json!({ "ok": false, "error": msg })),
        )
            .into_response();
    }
    let instance_id = match info.instance_id {
        Some(id) if !id.is_empty() => id,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": "hello is missing instanceId" })),
            )
                .into_response()
        }
    };
    let role = Role::parse(info.role.as_deref());

    let mut inner = bridge.inner.lock().unwrap();
    {
        let studio = inner.studios.entry(instance_id.clone()).or_insert_with(|| {
            Studio::new(
                instance_id.clone(),
                info.place_name
                    .clone()
                    .unwrap_or_else(|| "(unknown)".into()),
            )
        });
        if let Some(install) = info.install_id {
            studio.install_id = Some(install);
        }
        if let Some(name) = info.place_name {
            studio.place_name = name;
        }
        if info.place_id.is_some() {
            studio.place_id = info.place_id;
        }
        if info.user_id.is_some() {
            studio.user_id = info.user_id;
        }
        studio.peers.insert(role, Instant::now());
    }
    if role == Role::Plugin {
        inner.last_error = None;
        let active_live = inner
            .active_id
            .as_ref()
            .and_then(|id| inner.studios.get(id))
            .map(|s| live(s, Role::Plugin))
            .unwrap_or(false);
        if !active_live {
            inner.active_id = Some(instance_id);
        }
    }
    (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
}

#[derive(Deserialize)]
struct PollParams {
    studio: Option<String>,
    role: Option<String>,
}

async fn handle_poll(
    State(bridge): State<Arc<Bridge>>,
    Query(params): Query<PollParams>,
) -> Response {
    let role = Role::parse(params.role.as_deref());
    let deadline = Instant::now() + POLL_PARK;
    loop {
        let notify = {
            let mut inner = bridge.inner.lock().unwrap();
            let studio = match params
                .studio
                .as_ref()
                .and_then(|id| inner.studios.get_mut(id))
            {
                Some(studio) => studio,
                // No registration for this peer (server restarted): tell it to re-announce.
                None => return StatusCode::RESET_CONTENT.into_response(),
            };
            studio.peers.insert(role, Instant::now()); // heartbeat at arrival, before parking
            let channel = studio.channels.entry(role).or_insert_with(PeerChannel::new);
            if let Some(cmd) = channel.queue.pop_front() {
                return (StatusCode::OK, Json(cmd)).into_response();
            }
            channel.notify.clone()
        };

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return StatusCode::NO_CONTENT.into_response();
        }
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if tokio::time::timeout(remaining, notified).await.is_err() {
            return StatusCode::NO_CONTENT.into_response();
        }
        // Woken: loop and re-check the queue under the lock (the wake is only a hint).
    }
}

#[derive(Deserialize)]
struct ResultBody {
    id: Option<String>,
    ok: Option<bool>,
    data: Option<Value>,
    error: Option<String>,
}

async fn handle_result(
    State(bridge): State<Arc<Bridge>>,
    Json(body): Json<ResultBody>,
) -> Response {
    if let Some(id) = &body.id {
        let mut inner = bridge.inner.lock().unwrap();
        if let Some(tx) = inner.pending.remove(id) {
            let _ = tx.send(BridgeResult {
                ok: body.ok.unwrap_or(false),
                data: body.data.unwrap_or(Value::Null),
                error: body.error,
            });
        }
    }
    (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
}
