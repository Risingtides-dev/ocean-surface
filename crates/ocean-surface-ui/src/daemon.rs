//! Daemon connection layer.
//!
//! Single point of contact between the UI and `ocean-daemon`. We speak the
//! product agent API:
//!
//!   POST /v1/agent/turns   → start a turn (returns metadata only)
//!   GET  /v1/agent/events  → SSE stream of AgentTurnEvent
//!   GET  /v1/agent/sessions → list sessions
//!
//! All reply text and tool output arrives as events on the SSE stream; the
//! POST returns once the turn completes but carries no payload beyond
//! turn_id / session_id / status. We push events into a Leptos signal so
//! the rest of the UI reacts naturally.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
#[cfg(target_arch = "wasm32")]
use std::sync::{Arc, Mutex};

use futures_util::future::{abortable, AbortHandle, LocalBoxFuture};
use futures_util::{FutureExt, StreamExt};
use gloo_net::eventsource::futures::EventSource;
use gloo_net::eventsource::State as EventSourceState;
use gloo_net::http::Request;
use leptos::prelude::*;
#[cfg(target_arch = "wasm32")]
use send_wrapper::SendWrapper;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

use crate::canvas::CanvasContext;
use crate::model::{Block, ComponentPlacement, Role, ToolStatus, Turn};

pub const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:4780";

/// The turn's `guidance` field (TASK-76/86).
///
/// Always `None`, and it is a FUNCTION rather than a literal at the call site
/// so the invariant is testable behavior instead of a string in a file.
///
/// Why the invariant exists: `guidance` is rendered by the daemon under
/// "Operator guidance for this turn:", so anything placed here reaches the
/// model wearing operator authority. The surface previously fed it
/// page-controlled browser tab titles — any website the operator had open
/// could author text that arrived with that authority, at a runtime whose
/// tools execute ungated. Browser state now travels only as the structured
/// client context, which the daemon sanitizes.
///
/// A future operator-AUTHORED guidance feature may legitimately return
/// `Some` here — but it must carry text the operator typed, never text a page
/// supplied. Change this function, not the call site, so the test below moves
/// with it.
fn turn_guidance() -> Option<Vec<String>> {
    None
}

/// Percent-encode a path segment before it is interpolated into a daemon URL
/// (TASK-77).
///
/// Session ids are daemon- or localStorage-sourced today, so this is not a
/// live traversal — but building daemon paths by raw string interpolation is
/// exactly the pattern that produced the CONFIRMED proxy traversal in TASK-71,
/// and it becomes reachable the moment an id is ever user-typed, URL-sourced,
/// or copied between surfaces. `rooms.rs` already encodes its keys this way;
/// this makes the session paths consistent rather than leaving one module
/// disciplined and its neighbour not.
///
/// Pure Rust (no js_sys) so it runs under native `cargo test`.
fn encode_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(target_arch = "wasm32")]
fn now_millis() -> f64 {
    js_sys::Date::now()
}

#[cfg(not(target_arch = "wasm32"))]
fn now_millis() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must be after Unix epoch")
        .as_secs_f64()
        * 1000.0
}

// Browsers cap concurrent HTTP/1.1 connections per origin. Each focused
// session owns two long-lived EventSource requests (agent events + permission
// events), so merely generation-gating stale readers is not enough: an idle
// stale stream never yields another frame, remains open, and eventually blocks
// the snapshot request for a later session. Keep explicit handles so every
// focus change can close both transports before opening their replacements.
#[cfg(target_arch = "wasm32")]
type EventSourceSlot = Arc<Mutex<Option<SendWrapper<EventSource>>>>;

#[cfg(target_arch = "wasm32")]
fn replace_event_source(slot: &EventSourceSlot, next: EventSource) {
    let previous = slot
        .lock()
        .expect("event source slot poisoned")
        .replace(SendWrapper::new(next));
    if let Some(previous) = previous {
        previous.take().close();
    }
}

#[cfg(target_arch = "wasm32")]
fn close_event_source(slot: &EventSourceSlot) {
    let previous = slot.lock().expect("event source slot poisoned").take();
    if let Some(previous) = previous {
        previous.take().close();
    }
}

/// The throwaway working directory for a projectless Chat session. Chat is
/// deliberately NOT a project: it clears the project selection and pins the
/// next lazy session's cwd to `/tmp` so the agent runs in a real directory
/// without claiming ownership of one. A project session is the opposite — it
/// explicitly pins the catalogue or section/session fallback root alongside
/// the selected `project_id`. Keeping this as a named constant (rather than a
/// bare literal) pins the projectless fallback at the type level.
pub const CHAT_WORKSPACE_ROOT: &str = "/tmp";

/// Advance the shared session-list request ticket. Equality against the latest
/// claimed ticket is the write authority for every session-list fetch path.
fn next_session_fetch_ticket(current: u64) -> u64 {
    current.wrapping_add(1)
}

fn session_fetch_ticket_is_current(current: u64, claimed: u64) -> bool {
    current == claimed
}

/// RFC 3986 query-component encoding for daemon-owned pagination cursors.
/// Keeping this pure makes reserved-character behavior testable off-WASM.
fn percent_encode_query_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

fn sessions_page_url(base: &str, cursor: Option<&str>) -> String {
    let mut url = format!(
        "{}/v1/agent/sessions?limit=1000",
        base.trim_end_matches('/')
    );
    if let Some(cursor) = cursor {
        url.push_str("&cursor=");
        url.push_str(&percent_encode_query_component(cursor));
    }
    url
}

/// Normalize an explicit project-session root. Empty roots have no authority:
/// callers must suppress the action instead of inheriting ambient cwd or
/// silently creating a project session under `/tmp`.
fn project_session_cwd(root: &str) -> Option<String> {
    let root = root.trim();
    if root.is_empty() {
        None
    } else {
        Some(root.to_string())
    }
}

/// Default LiveKit room the header "Join room call" entry targets when no
/// proxy config seeded one (native shell has no proxy in front). Mirrors the
/// proxy's `DEFAULT_LIVEKIT_ROOM_ID`.
const DEFAULT_LIVEKIT_ROOM_ID: &str = "project:surface-main";

/// Shape of the proxy's GET /api/config — the zero-config bootstrap payload.
#[derive(Debug, Clone, Deserialize)]
struct ProxyConfig {
    #[serde(default)]
    daemon_url: String,
    #[serde(default)]
    has_auth: bool,
    #[allow(dead_code)]
    #[serde(default)]
    voice_profile: String,
    #[serde(default)]
    maps_key: String,
    #[serde(default)]
    maps_map_id: String,
    #[serde(default)]
    livekit_room_id: String,
    #[serde(default)]
    livekit_token_path: String,
    #[serde(default)]
    tldraw_sync_uri: String,
    /// Who the proxy says is signed in. Empty in single-operator mode.
    #[serde(default)]
    user_id: String,
    #[serde(default)]
    user_display_name: String,
}

/// A component interaction event sent from the client to the daemon.
#[derive(Debug, Clone, Serialize)]
pub struct ComponentEventRequest {
    pub session_id: String,
    pub component_id: String,
    pub event: Value,
}

// ---------------------------------------------------------------------------
// Surface patch wire types (OCEAN-178)
//
// Self-contained mirror of `ocean-agent-sdk::surface` (GPUI Masterbuild Slice 1)
// so this WASM crate can deserialize the EXACT JSON the daemon streams on a
// `surface_patch` SSE frame — without taking a build dependency on the ocean-os
// workspace (this crate already mirrors every other daemon wire type the same
// way: `ToolCallSummary`, `ToolResult`, etc.). The GPUI shell carries the same
// mirror in `ocean-gui/src/shell/canvas/patch.rs`; the wire contract is fixed in
// `ocean-os/crates/ocean-agent-sdk/src/lib.rs`.
//
// Wire-contract notes (must match the SDK or patches silently route to `Other`):
// - Ids are `serde(transparent)` over `String` — on the wire a `ComponentId` is
//   just `"brief-1"`.
// - `SurfacePatch` is internally tagged on `"op"`, `snake_case`.
// - `SurfacePatchEnvelope.session_id` is carried as a plain `String` here
//   (this crate's convention); the SDK uses a `serde(transparent)` uuid newtype,
//   so the JSON is byte-identical.
// ---------------------------------------------------------------------------

/// A string-backed, `serde(transparent)` id newtype (matches the SDK exactly).
macro_rules! surface_string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        // `PartialOrd, Ord` (OCEAN-270): the convergent-merge version vector keys a
        // `BTreeMap` on `ComponentId`, matching the native id macro's derives.
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        #[allow(dead_code)]
        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

surface_string_id!(
    /// Identifies a *surface* — one client face onto a session (e.g. `gpui:local`).
    SurfaceId
);
surface_string_id!(
    /// Identifies a *canvas* within a surface (e.g. `canvas:main`).
    CanvasId
);
surface_string_id!(
    /// Identifies a *component* (card, node, frame, …) on a canvas.
    ComponentId
);
surface_string_id!(
    /// Identifies a single emitted *patch*.
    PatchId
);
surface_string_id!(
    /// Identifies an *edge* between two endpoints.
    EdgeId
);

/// Axis-aligned rectangle in canvas space. All fields roundtrip as JSON numbers.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Pan/zoom state of a canvas viewport.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    #[serde(default = "viewport_default_zoom")]
    pub zoom: f32,
}

fn viewport_default_zoom() -> f32 {
    1.0
}

/// Reference to the actor that originated a patch.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ActorRef {
    /// Coarse actor class, e.g. `"agent"`, `"human"`, `"system"`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Upsert payload for a component. `rect`/`content` are optional so an agent can
/// create a component and let the app allocate placement.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CanvasComponentPatch {
    pub id: ComponentId,
    /// Component kind or template name, e.g. `"card"`, `"brief_card"`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect: Option<Rect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z_index: Option<i32>,
    /// Free-form content payload (title/body/etc). Defaults to `null`.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub content: Value,
    /// Free-form metadata that survives a roundtrip untouched.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

/// Endpoint of an edge — either a bare component or a specific port on it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Endpoint {
    pub component_id: ComponentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
}

/// Create/update payload for an edge between two endpoints.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CanvasEdgePatch {
    pub id: EdgeId,
    pub from: Endpoint,
    pub to: Endpoint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

/// Target of a [`SurfacePatch::Focus`] operation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusTarget {
    Component { component_id: ComponentId },
    Edge { edge_id: EdgeId },
    Canvas,
}

/// Target of a [`SurfacePatch::Layout`] operation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutTarget {
    Canvas,
    Component { component_id: ComponentId },
    Components { ids: Vec<ComponentId> },
}

/// Layout strategy. Open string set so new strategies don't break the wire.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutStrategy {
    Grid,
    Stack,
    Row,
    Column,
    Tree,
    Graph,
    /// Any strategy not in the known set, carried as its raw name.
    #[serde(untagged)]
    Other(String),
}

/// A single structured mutation to an Ocean surface canvas. Internally tagged on
/// `"op"` with `snake_case` discriminants — `{ "op": "upsert_component", … }`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SurfacePatch {
    UpsertComponent {
        component: CanvasComponentPatch,
    },
    MoveComponent {
        component_id: ComponentId,
        x: f32,
        y: f32,
    },
    ResizeComponent {
        component_id: ComponentId,
        width: f32,
        height: f32,
    },
    DeleteComponent {
        component_id: ComponentId,
    },
    Connect {
        edge: CanvasEdgePatch,
    },
    Disconnect {
        edge_id: EdgeId,
    },
    Focus {
        target: FocusTarget,
    },
    Select {
        ids: Vec<ComponentId>,
    },
    SetViewport {
        viewport: Viewport,
    },
    Layout {
        target: LayoutTarget,
        strategy: LayoutStrategy,
    },
    Group {
        frame_id: ComponentId,
        children: Vec<ComponentId>,
    },
}

impl SurfacePatch {
    /// The single component this patch contends for under the convergent merge
    /// (OCEAN-270), or `None` when the op doesn't last-write-wins one component.
    /// Mirrors the SDK's `SurfacePatch::target_component`: the per-component ops
    /// return their target; edge / view-state / multi-component ops return `None`
    /// and apply directly (never gated).
    pub fn target_component(&self) -> Option<&ComponentId> {
        match self {
            SurfacePatch::UpsertComponent { component } => Some(&component.id),
            SurfacePatch::MoveComponent { component_id, .. }
            | SurfacePatch::ResizeComponent { component_id, .. }
            | SurfacePatch::DeleteComponent { component_id } => Some(component_id),
            SurfacePatch::Connect { .. }
            | SurfacePatch::Disconnect { .. }
            | SurfacePatch::Focus { .. }
            | SurfacePatch::Select { .. }
            | SurfacePatch::SetViewport { .. }
            | SurfacePatch::Layout { .. }
            | SurfacePatch::Group { .. } => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Convergent-merge types (OCEAN-270) — mirror of
// `ocean-agent-sdk::surface_merge`, exactly as the native `ocean-gui` mirror.
//
// The web surface folds the agent patch stream into a `WebCanvasLedger`; this is
// the per-component LWW that makes two concurrent edits to the same component
// converge to one deterministic winner (higher rev; equal rev → higher actor id)
// regardless of arrival order, while edits to different components both land.
// ---------------------------------------------------------------------------

/// A stable, orderable identity for whoever originated a write, derived from an
/// [`ActorRef`]. `serde(transparent)`: on the wire just `"agent:sage"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(transparent)]
pub struct ActorId(pub String);

#[allow(dead_code)]
impl ActorId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Derive a stable id from an [`ActorRef`]: `"{kind}:{id}"` with an id, else
    /// the bare kind. Same kind+id always yields the same id (deterministic
    /// tiebreak).
    pub fn from_actor(actor: &ActorRef) -> Self {
        match &actor.id {
            Some(id) => Self(format!("{}:{}", actor.kind, id)),
            None => Self(actor.kind.clone()),
        }
    }
}

/// Per-component logical version: a Lamport-style `rev` plus the [`ActorId`] that
/// authored it. Derived `Ord` is lexicographic, so **`rev` must stay before
/// `actor`**: higher `rev` wins; equal `rev` → higher `actor` string wins.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub struct ComponentVersion {
    pub rev: u64,
    pub actor: ActorId,
}

#[allow(dead_code)]
impl ComponentVersion {
    pub fn new(rev: u64, actor: ActorId) -> Self {
        Self { rev, actor }
    }

    /// Strictly greater in the `(rev, actor)` total order — the merge decision.
    pub fn supersedes(&self, other: &ComponentVersion) -> bool {
        self > other
    }
}

/// A per-actor Lamport logical clock. `tick()` on a local write; `observe(rev)`
/// jumps past a remote revision so the next tick is strictly greater.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct LamportClock {
    counter: u64,
}

#[allow(dead_code)]
impl LamportClock {
    pub fn new() -> Self {
        Self { counter: 0 }
    }

    pub fn at(value: u64) -> Self {
        Self { counter: value }
    }

    pub fn now(&self) -> u64 {
        self.counter
    }

    pub fn tick(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    pub fn observe(&mut self, remote_rev: u64) -> u64 {
        self.counter = self.counter.max(remote_rev);
        self.counter
    }
}

/// The decision when a versioned write is offered to [`CanvasMergeState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeDecision {
    /// The write won — apply the patch.
    Applied,
    /// The write lost (lower/stale/replay) — skip it. Skipping is how replicas
    /// converge on the same winner regardless of arrival order.
    Superseded,
}

#[allow(dead_code)]
impl MergeDecision {
    pub fn applied(self) -> bool {
        matches!(self, MergeDecision::Applied)
    }
}

/// The per-canvas version vector: `ComponentId -> winning ComponentVersion`.
/// [`merge`](CanvasMergeState::merge) accepts a write iff it strictly supersedes
/// the stored one, so the end state is order-independent; different components
/// never contend.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CanvasMergeState {
    versions: std::collections::BTreeMap<ComponentId, ComponentVersion>,
}

#[allow(dead_code)]
impl CanvasMergeState {
    pub fn new() -> Self {
        Self {
            versions: std::collections::BTreeMap::new(),
        }
    }

    pub fn version(&self, id: &ComponentId) -> Option<&ComponentVersion> {
        self.versions.get(id)
    }

    pub fn len(&self) -> usize {
        self.versions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.versions.is_empty()
    }

    /// Offer a versioned write for `id`: first write always
    /// [`Applied`](MergeDecision::Applied); otherwise `Applied` iff it strictly
    /// supersedes the stored version, else [`Superseded`](MergeDecision::Superseded).
    /// On `Applied` the stored version advances. The single commutative op the
    /// convergence guarantee rests on.
    pub fn merge(&mut self, id: &ComponentId, incoming: ComponentVersion) -> MergeDecision {
        match self.versions.get(id) {
            Some(stored) if !incoming.supersedes(stored) => MergeDecision::Superseded,
            _ => {
                self.versions.insert(id.clone(), incoming);
                MergeDecision::Applied
            }
        }
    }

    pub fn max_rev(&self) -> u64 {
        self.versions.values().map(|v| v.rev).max().unwrap_or(0)
    }
}

/// A patch plus the session/surface/canvas/actor context needed to route and
/// persist it. This is exactly what the daemon streams inside a `surface_patch`
/// event's `patches` array.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SurfacePatchEnvelope {
    pub patch_id: PatchId,
    /// Plain `String` here (this crate's convention); the SDK's `AgentSessionId`
    /// is `serde(transparent)` over the same string, so the JSON is identical.
    #[serde(default)]
    pub session_id: String,
    pub surface_id: SurfaceId,
    pub canvas_id: CanvasId,
    pub actor: ActorRef,
    pub created_at_ms: i64,
    pub patch: SurfacePatch,
    /// Optional per-component version for the convergent merge (OCEAN-270).
    /// `#[serde(default, skip_serializing_if)]` keeps it additive: the daemon
    /// relays `None`, and the surface ledger assigns/observes the version as the
    /// merge point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<ComponentVersion>,
}

/// One canvas patch the web surface has received and stored for rendering. The
/// full GPUI-style canvas (a `CanvasLedger` with placement, hit-testing, and a
/// rendered scene) is not yet ported to the web; this keeps the daemon's patch
/// stream visible (and inspectable) on the web surface instead of dropping it.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasPatchEntry {
    /// Canvas this patch targets (e.g. `canvas:main`).
    pub canvas_id: String,
    /// One-line, human-readable summary of the op (e.g. `upsert_component brief-1`).
    pub summary: String,
    /// The full stamped envelope, kept so a richer renderer can use it later.
    pub envelope: SurfacePatchEnvelope,
}

/// The shape of every event the daemon publishes on /v1/agent/events.
/// Mirrors `AgentTurnEvent` in crates/ocean-agent-sdk.
///
/// The `title`/`cwd` on `SessionCreated` are rendered in the header (OCEAN-236).
/// Per-event `turn_id`/`session_id` ids stay parsed so the variants match the
/// daemon's wire shape 1:1 (the drift-guard test deserializes the daemon's exact
/// JSON), and several are read by the reducer; the few that are purely
/// wire-contract documentation carry a field-level `#[allow(dead_code)]` rather
/// than a blanket suppression over the whole enum.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    SessionCreated {
        session_id: String,
        title: String,
        #[serde(default)]
        cwd: String,
    },
    TurnStarted {
        turn_id: String,
        session_id: String,
        #[serde(default)]
        model: Option<String>,
    },
    AssistantTextDelta {
        // session_id added daemon-side so scoped SSE clients can verify the
        // frame still belongs to their active session. `default` keeps us
        // compatible with daemons that predate the field.
        #[serde(default)]
        session_id: String,
        turn_id: String,
        delta: String,
    },
    ThinkingDelta {
        #[serde(default)]
        session_id: String,
        turn_id: String,
        delta: String,
    },
    ToolCallStarted {
        #[serde(default)]
        session_id: String,
        turn_id: String,
        call: ToolCallSummary,
    },
    ToolCallChunk {
        #[serde(default)]
        session_id: String,
        turn_id: String,
        call_id: String,
        chunk: String,
    },
    ToolCallFinished {
        #[serde(default)]
        session_id: String,
        turn_id: String,
        call_id: String,
        result: ToolResult,
    },
    TurnFinished {
        #[serde(default)]
        session_id: String,
        // The reducer compares this against the running `active_turn_id` for
        // terminal admission (TASK-45): only a matching id clears the live turn
        // state, and it also scopes the per-turn transcript/tool sweep.
        turn_id: String,
        status: String,
        #[serde(default)]
        error: Option<String>,
        // Server-reported wall time; the header derives duration from tokens/rate
        // today, so this is dev/wire-contract only for now (OCEAN-236).
        #[serde(default)]
        #[allow(dead_code)]
        wall_ms: Option<u64>,
        #[serde(default)]
        output_tokens: Option<u64>,
        #[serde(default)]
        input_tokens: Option<u64>,
        #[serde(default)]
        cache_read_tokens: Option<u64>,
        #[serde(default)]
        tokens_per_second: Option<f64>,
    },
    /// The agent wants to mount or update an interactive component.
    ComponentRender {
        session_id: String,
        component_id: String,
        kind: String,
        props: Value,
        #[serde(default)]
        replace: bool,
    },
    /// The agent wants to unmount a previously rendered component.
    ComponentUnmount {
        session_id: String,
        component_id: String,
    },
    /// Ocean started (`active: true`) or finished (`active: false`) driving the
    /// browser. The side-panel cockpit uses this to auto-focus while browser
    /// work happens, then release back to the origin surface.
    BrowserActivity { session_id: String, active: bool },
    /// The agent applied one or more validated patches to a canvas surface
    /// (GPUI Masterbuild Slice 3, daemon side). Each patch is a fully-stamped
    /// [`SurfacePatchEnvelope`] (session/surface/canvas/actor/timestamp). The
    /// GPUI native shell applies these to its `CanvasLedger`; the web surface
    /// records them into `canvas_patches` and renders a basic representation.
    ///
    /// This mirrors the daemon's `AgentTurnEvent::SurfacePatch` wire shape
    /// exactly (`ocean-os/crates/ocean-agent-sdk/src/lib.rs`, internally tagged
    /// on `"type" = "surface_patch"`, `snake_case`). Before OCEAN-178 the web
    /// `AgentEvent` had no such variant AND `surface_patch` was absent from
    /// `AGENT_EVENT_NAMES`, so `EventSource` dropped the frame at the transport
    /// layer — the web PWA (and the extension that bundles this WASM) were blind
    /// to agent-rendered canvases. A drift-guard test deserializes the daemon's
    /// exact JSON into this variant so future wire drift fails loudly.
    SurfacePatch {
        #[serde(default)]
        session_id: String,
        // Wire-contract field (the drift-guard test asserts it deserializes); the
        // web surface records patches per-canvas, not per-turn (OCEAN-236).
        #[allow(dead_code)]
        turn_id: String,
        canvas_id: CanvasId,
        patches: Vec<SurfacePatchEnvelope>,
    },
    /// Catch-all for extension / council events (e.g. Longhouse). Carries the
    /// raw payload and an optional session `scope` (OCEAN-56). A scoped event
    /// (`scope: Some`) belongs to a session and is filtered like any
    /// session-bearing event; an unscoped one (`scope: None`) is council-wide
    /// and only reaches the `?all=1` firehose. We don't render these yet, but
    /// we name the variant so they deserialize cleanly instead of being mapped
    /// to `Other` (or, on a stricter enum, failing) — then log + ignore them.
    Extension {
        extension: String,
        // Carried for completeness; per the variant doc above these aren't
        // rendered yet (only logged), so the payload is wire-contract only.
        #[serde(default)]
        #[allow(dead_code)]
        payload: Value,
        #[serde(default)]
        scope: Option<String>,
    },
    #[serde(other)]
    Other,
}

/// SSE `event:` names the daemon emits on `GET /v1/agent/events`, one per
/// [`AgentEvent`] variant.
///
/// ⚠️ DRIFT HAZARD — this list MUST stay in lockstep with the daemon's
/// `agent_event_type_name` match in
/// `ocean-os/crates/ocean-daemon/src/main.rs:3782` (the function that names each
/// SSE frame). gloo-net's `EventSource` only delivers frames whose `event:` name
/// was explicitly `subscribe()`d, so any name the daemon emits that is NOT in
/// this list is **dropped at the transport layer before serde ever sees it** —
/// the `#[serde(other)] Other` catch-all on `AgentEvent` cannot save it, because
/// the JSON never arrives. This has already bitten Extension events (OCEAN-62)
/// and permission events (OCEAN-64), each needing a manual addition here.
///
/// Adding a daemon event is therefore a ONE-LINE change in this file: add its
/// snake_case name below. The `agent_event_names_cover_all_variants` test fails
/// the build if an `AgentEvent` variant has no matching entry here, so drift
/// surfaces at `cargo test` rather than as a silent runtime drop.
///
/// NOTE: the daemon also emits an out-of-band `error` frame on broadcast lag /
/// serialize failure (`Event::default().event("error")`). It is deliberately
/// NOT subscribed here — it is not an `AgentEvent` and carries no transcript
/// state; the per-name subscription simply ignores it.
///
/// FOLLOW-UP (needs a daemon change, #54-sequenced): the robust fix is for the
/// daemon to emit every frame under a single SSE name (e.g. the default
/// `message` channel) with the type already in the JSON payload, so the surface
/// can wire one catch-all listener and route purely on the `type` tag via serde
/// — eliminating this allow-list entirely. See OCEAN-102 PR for the ticket note.
pub(crate) const AGENT_EVENT_NAMES: &[&str] = &[
    "session_created",
    "turn_started",
    "assistant_text_delta",
    "thinking_delta",
    "tool_call_started",
    "tool_call_chunk",
    "tool_call_finished",
    "turn_finished",
    "component_render",
    "component_unmount",
    "browser_activity",
    "surface_patch",
    "extension",
];

impl AgentEvent {
    /// The session this event belongs to, if it carries one. Used to drop
    /// events from other sessions if a proxy or stale stream misbehaves. Returns
    /// `None` for `Other` and (from older daemons) for any event whose
    /// `session_id` came through empty via serde default.
    fn session_id(&self) -> Option<&str> {
        let sid = match self {
            AgentEvent::SessionCreated { session_id, .. }
            | AgentEvent::TurnStarted { session_id, .. }
            | AgentEvent::AssistantTextDelta { session_id, .. }
            | AgentEvent::ThinkingDelta { session_id, .. }
            | AgentEvent::ToolCallStarted { session_id, .. }
            | AgentEvent::ToolCallChunk { session_id, .. }
            | AgentEvent::ToolCallFinished { session_id, .. }
            | AgentEvent::TurnFinished { session_id, .. }
            | AgentEvent::ComponentRender { session_id, .. }
            | AgentEvent::ComponentUnmount { session_id, .. }
            | AgentEvent::BrowserActivity { session_id, .. }
            | AgentEvent::SurfacePatch { session_id, .. } => session_id.as_str(),
            // An extension event's scope (when set) is its session id; a
            // council-wide one has no scope and is treated as unscoped.
            AgentEvent::Extension { scope, .. } => scope.as_deref().unwrap_or(""),
            AgentEvent::Other => return None,
        };
        (!sid.is_empty()).then_some(sid)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolCallSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub args_json: Value,
}

/// Authoritative cross-session request state from `GET /v1/requests`.
/// Kept separate from the focused session's live `active_turn_id`: the Island
/// uses this read-only snapshot to show work elsewhere without opening one SSE
/// connection per background session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestSnapshotState {
    Queued,
    Running,
    WaitingForPermission,
    Cancelling,
    Cancelled,
    Completed,
    Errored,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RequestSnapshot {
    pub request_id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    pub state: RequestSnapshotState,
    #[serde(default)]
    pub permission_id: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
}

/// Pending cross-session permission metadata from `GET /v1/permissions`.
/// Deliberately omits the daemon's raw `args` payload: the Island is a compact
/// read-only signal, while the focused transcript permission card remains the
/// detail and decision authority.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PermissionSnapshot {
    pub permission_id: String,
    pub request_id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub created_at: String,
}

/// One bounded transcript-history match from the daemon-owned Recall search.
/// The daemon searches persisted display transcript text; the Surface only
/// renders results and opens their source session.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HistorySearchHit {
    pub hit_id: String,
    pub session_id: String,
    #[serde(default)]
    pub session_title: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub excerpt: String,
    #[serde(default)]
    pub timestamp_ms: Option<i64>,
    #[serde(default)]
    pub workspace_root: Option<String>,
    #[serde(default)]
    pub score: f64,
    #[serde(default)]
    pub match_kind: String,
}

#[derive(Debug, Clone, Deserialize)]
struct HistorySearchResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    hits: Vec<HistorySearchHit>,
    #[serde(default)]
    error: Option<String>,
}

/// A pending permission request awaiting the operator's allow/deny.
///
/// Mirrors the daemon's `OceanEvent::PermissionRequest` plus the envelope's
/// `permission_id` / `session_id`. When daemon permission-gating is on, a
/// mutating tool call (write/edit/bash) BLOCKS until a decision is POSTed to
/// `/v1/permissions/{id}/decision`. The web surface renders one card per
/// pending entry so a gated turn doesn't silently hang here.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingPermission {
    pub permission_id: String,
    pub session_id: String,
    pub tool: String,
    pub reason: String,
    /// Pretty-printed args summary for display.
    pub args_summary: String,
    /// True once a decision POST is in flight, so the buttons can disable.
    pub deciding: bool,
    /// The daemon request id that raised this gate. The daemon mints
    /// `AgentTurnId(request_id)`, so this equals the turn id. Present from a
    /// current daemon (the control envelope's `request_id` and
    /// `GET /v1/permissions` both carry it); `None` only from an older daemon.
    /// Used to resolve per-card decision authority so a later turn's token can
    /// never be replayed against an earlier turn's gate (TASK-44, finding 4).
    pub request_id: Option<String>,
    /// True when THIS surface holds the decision token bound to `request_id`
    /// (it submitted the originating turn). A card raised by another surface —
    /// or one whose request has no local token — is non-actionable: the buttons
    /// render read-only and `decide_permission` refuses to POST, so no
    /// wrong/unbound token ever reaches the daemon (TASK-44, finding 4).
    pub actionable: bool,
}

/// TASK-69: two-state pending-permission view. The bare `Vec<PendingPermission>`
/// could not distinguish "genuinely zero pending" (snapshot settled, list empty
/// — show nothing, correct) from "couldn't load" (fetch failed / revision churn
/// — which previously ALSO degraded to an empty vec and showed nothing). A gate
/// that PREDATES a session-load lives only in the snapshot; if that snapshot
/// degrades to empty the blocked tool call is invisible with no live frame to
/// rescue it (live `permission_request` frames only re-enqueue gates raised
/// AFTER the load). The operator sees a clean idle screen while a tool call
/// hangs. This models the distinction explicitly.
///
/// `cards` always holds the fully-materialized, renderable, actionable cards —
/// including, under `Unavailable`, any live cards that arrived over the control
/// stream after degradation (codex contract: "preserves admitted same-session
/// live cards"). `Unavailable.known_pending_ids` records ids last known pending
/// that could NOT be re-materialized, so the UI warns ("N may be pending —
/// couldn't refresh") instead of silently showing nothing. Only a settled
/// successful snapshot (`Fresh`) may establish an authoritative empty set or
/// clear degradation.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PermissionView {
    cards: Vec<PendingPermission>,
    availability: PermissionAvailability,
}

#[derive(Debug, Clone, PartialEq, Default)]
enum PermissionAvailability {
    #[default]
    Fresh,
    Unavailable {
        reason: String,
        known_pending_ids: Vec<String>,
    },
}

impl PermissionView {
    /// The authoritative complete pending set as of a settled snapshot.
    fn fresh(cards: Vec<PendingPermission>) -> Self {
        Self {
            cards,
            availability: PermissionAvailability::Fresh,
        }
    }

    /// Test-only constructor for a degraded view with no live cards (production
    /// degrades go through [`PermissionView::degraded_preserving`], which
    /// preserves live cards). Used by seam tests to seed a prior Unavailable view.
    #[cfg(test)]
    fn unavailable(reason: String, known_pending_ids: Vec<String>) -> Self {
        Self {
            cards: Vec::new(),
            availability: PermissionAvailability::Unavailable {
                reason,
                known_pending_ids,
            },
        }
    }

    /// TASK-69 fix-forward: build the degraded view for a session-load projection
    /// that could not settle the rich card snapshot. This is what a Degraded
    /// commit publishes, and it must (codex HOLD on 8b41fee):
    ///
    /// 1. PRESERVE the same-session live cards already admitted into `prior_view`
    ///    (frames that arrived over the control stream during the snapshot await)
    ///    — they are real and actionable and must survive the degrade, not be
    ///    flattened to an id and overwritten.
    /// 2. Seed the warning set from `detail_pending_ids` — the daemon's
    ///    authoritative `SessionDetail.pending_permissions` — so a gate that
    ///    predates the load is visible on a FIRST load where the local
    ///    `prior_view` is empty; unioned with whatever the operator was already
    ///    shown locally, minus anything already covered by a preserved live card.
    fn degraded_preserving(
        reason: String,
        prior_view: &PermissionView,
        detail_pending_ids: &[String],
    ) -> Self {
        let cards = prior_view.cards().to_vec();
        let card_ids: HashSet<&str> = cards.iter().map(|c| c.permission_id.as_str()).collect();
        let mut known_pending_ids: Vec<String> = Vec::new();
        for id in detail_pending_ids
            .iter()
            .chain(prior_view.all_known_pending_ids().iter())
        {
            if !card_ids.contains(id.as_str()) && !known_pending_ids.contains(id) {
                known_pending_ids.push(id.clone());
            }
        }
        Self {
            cards,
            availability: PermissionAvailability::Unavailable {
                reason,
                known_pending_ids,
            },
        }
    }

    /// The fully-materialized, actionable cards (renderable exactly as before).
    pub(crate) fn cards(&self) -> &[PendingPermission] {
        &self.cards
    }

    pub(crate) fn is_unavailable(&self) -> bool {
        matches!(
            self.availability,
            PermissionAvailability::Unavailable { .. }
        )
    }

    /// The degrade reason, for the "couldn't refresh" affordance.
    pub(crate) fn reason(&self) -> Option<&str> {
        match &self.availability {
            PermissionAvailability::Unavailable { reason, .. } => Some(reason.as_str()),
            PermissionAvailability::Fresh => None,
        }
    }

    /// Ids known pending but NOT present as a live card — what the "N may be
    /// pending — couldn't refresh" warning counts. Empty under `Fresh`.
    pub(crate) fn unconfirmed_ids(&self) -> Vec<String> {
        match &self.availability {
            PermissionAvailability::Unavailable {
                known_pending_ids, ..
            } => known_pending_ids
                .iter()
                .filter(|id| !self.cards.iter().any(|card| &card.permission_id == *id))
                .cloned()
                .collect(),
            PermissionAvailability::Fresh => Vec::new(),
        }
    }

    /// Every id the operator has been shown as pending (live cards ∪ still-
    /// unconfirmed known ids). Seeds `known_pending_ids` at the NEXT degrade so a
    /// warning survives a chain of failed refreshes without inventing ids.
    fn all_known_pending_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .cards
            .iter()
            .map(|card| card.permission_id.clone())
            .collect();
        if let PermissionAvailability::Unavailable {
            known_pending_ids, ..
        } = &self.availability
        {
            for id in known_pending_ids {
                if !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
        }
        ids
    }

    /// Seam 4: a live `permission_request` frame. Push the real, actionable card
    /// (dedup by id) and drop its id from `known_pending_ids` (now
    /// materialized). Degradation is NOT cleared here — only a settled snapshot
    /// may do that — so a frame for C while {A,B} were known yields
    /// `cards = [.., C]` with the {A,B} warning retained.
    fn ingest_request(&mut self, card: PendingPermission) {
        if self
            .cards
            .iter()
            .any(|existing| existing.permission_id == card.permission_id)
        {
            // The daemon reuses one PermissionId for an identical tool+args retry
            // within a turn, so a replayed frame must not stack a second card.
            return;
        }
        if let PermissionAvailability::Unavailable {
            known_pending_ids, ..
        } = &mut self.availability
        {
            known_pending_ids.retain(|id| id != &card.permission_id);
        }
        self.cards.push(card);
    }

    /// Seam 5: a decision recorded (or a daemon-resolved `permission_decision`
    /// frame). Drop the id from BOTH the live cards and `known_pending_ids` so a
    /// decided gate can never resurrect through the degrade warning.
    fn resolve(&mut self, permission_id: &str) {
        self.cards
            .retain(|card| card.permission_id != permission_id);
        if let PermissionAvailability::Unavailable {
            known_pending_ids, ..
        } = &mut self.availability
        {
            known_pending_ids.retain(|id| id != permission_id);
        }
    }

    /// Toggle a card's in-flight `deciding` flag (a decision POST started or
    /// failed). No-op if the id is not a live card.
    fn set_deciding(&mut self, permission_id: &str, deciding: bool) {
        if let Some(card) = self
            .cards
            .iter_mut()
            .find(|card| card.permission_id == permission_id)
        {
            card.deciding = deciding;
        }
    }
}

/// The control-plane event envelope on `/v1/events`. Unlike `/v1/agent/events`
/// (which streams `AgentTurnEvent` and serializes only the inner event), this
/// stream serializes the FULL `EventEnvelope`, so `permission_id` / `session_id`
/// ride alongside the flattened `OceanEvent`. We only model the two permission
/// frames; every other `type` falls into `Other` and is ignored.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ControlEvent {
    PermissionRequest {
        #[serde(default)]
        permission_id: Option<String>,
        #[serde(default)]
        session_id: Option<String>,
        /// The originating request/turn id, carried on the flattened envelope
        /// (`EventEnvelope::request_id`). Used to bind the card to this
        /// surface's decision token (TASK-44). `None` only from an older daemon.
        #[serde(default)]
        request_id: Option<String>,
        tool: String,
        #[serde(default)]
        reason: String,
        #[serde(default)]
        args: Value,
    },
    PermissionDecision {
        #[serde(default)]
        permission_id: Option<String>,
        #[serde(default)]
        session_id: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolResult {
    pub ok: bool,
    #[serde(default)]
    pub output: String,
}

#[derive(Debug, Deserialize)]
struct RequestCancelResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    message: String,
}

fn request_cancel_result(http_ok: bool, raw: &str) -> Result<(), String> {
    match serde_json::from_str::<RequestCancelResponse>(raw) {
        Ok(response) if http_ok && response.ok => Ok(()),
        Ok(response) => Err(if response.message.trim().is_empty() {
            "request cancellation rejected".to_string()
        } else {
            response.message
        }),
        Err(_) => Err(if raw.trim().is_empty() {
            "unreadable cancellation response".to_string()
        } else {
            raw.to_string()
        }),
    }
}

/// One image attached to a turn (OCEAN-138). Serializes to the exact wire shape
/// the daemon's `TurnImage` (ocean-agent-sdk) deserializes: a `mime_type` and a
/// base64 `data` body (a `data:<mime>;base64,` prefix is tolerated — the daemon
/// strips it). On the first user message of the turn the daemon emits one
/// `Content::Image` block per entry alongside the prompt text (shipped in
/// OCEAN-115), so a screenshot/picked image actually reaches the model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnImage {
    /// MIME type of the image, e.g. `"image/png"` or `"image/jpeg"`.
    pub mime_type: String,
    /// Base64-encoded image bytes, or a `data:<mime>;base64,` URL (the daemon
    /// strips the prefix, keeping only the base64 body).
    pub data: String,
}

/// A single open tab as the surface sees it. Mirrors the daemon's
/// `ocean_agent_sdk::BrowserTab` wire shape (`{url, title, active}`) so the
/// structured `client_context.browser.tabs` round-trips exactly. The surface
/// has no dep on `ocean-agent-sdk` (it's a wasm CSR crate), so the DTO is
/// re-declared locally — the serde field names/types are the contract.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct BrowserTab {
    url: String,
    title: String,
    /// True for the tab the surface considers active/foregrounded. Always
    /// emitted (the SDK's `BrowserTab::active` is `#[serde(default)]`, so a
    /// flat `false` is the correct, lossless default for non-active tabs).
    active: bool,
}

/// Active-tab / open-tab browser state attached to a turn (OCEAN-40 / OCEAN-377).
/// Mirrors the daemon's `ocean_agent_sdk::BrowserContext`. Every field is
/// additive + optional so an empty context serializes to `{}` and older
/// daemons ignore it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
struct BrowserContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    active_tab_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_tab_title: Option<String>,
    /// The current window's open-tab list. Empty/omitted when the surface only
    /// knows its own active tab. Matches the SDK's `Vec<BrowserTab>` with
    /// `skip_serializing_if = "Vec::is_empty"`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tabs: Vec<BrowserTab>,
}

/// Per-turn client/browser context (OCEAN-40 / OCEAN-377). Mirrors the daemon's
/// `ocean_agent_sdk::ClientContext`. Lets the in-browser surface ship its live
/// active-tab / open-tab state structurally (alongside, not replacing, the
/// freeform `guidance` lines) so the agent can resolve "this tab" without a
/// round-trip. `None` on the detached web app and any non-extension surface.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
struct ClientContext {
    /// The client surface kind. Mirrors the flat `AgentTurnRequest::client_type`
    /// (kept for back-compat) so the context object is self-describing.
    client_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    browser: Option<BrowserContext>,
}

#[derive(Debug, Clone, Serialize)]
struct AgentTurnRequest<'a> {
    prompt: &'a str,
    cwd: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<&'a str>,
    /// Selected project. When set, the daemon binds the turn to the project's
    /// workspace_root (the web client sends "/" as cwd, so without this every
    /// session lands in the daemon's launch dir).
    #[serde(skip_serializing_if = "Option::is_none")]
    project_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_type: Option<&'a str>,
    /// Optional guidance hints passed to the agent (e.g. "focus on tests").
    /// Matches the daemon's `AgentTurnRequest::guidance: Option<Vec<String>>`.
    /// The web UI doesn't surface this yet, so it serializes as `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    guidance: Option<Vec<String>>,
    /// Optional Track-0 room id for room-scoped turns. Mirrors the daemon's
    /// `room_id: Option<String>`, which only accepts the closed Track-0 set
    /// (`pm`/`writers`/`orch_mesh`/`review`) and 400s anything else. Always
    /// `None` from this surface: the UI's persistent rooms are a different
    /// concept (keyed rooms posting via
    /// `POST /v1/rooms/persistent/{key}/messages`, never through this turn
    /// path — room mode unmounts the composer entirely).
    #[serde(skip_serializing_if = "Option::is_none")]
    room_id: Option<&'a str>,
    /// Per-turn reasoning effort override. Mirrors the daemon's
    /// `thinking_level: Option<ThinkingLevel>` — serialized as the lowercase
    /// `ThinkingLevel` string the daemon expects. Populated from the
    /// composer's reasoning-effort control (OCEAN-79); `None` leaves the
    /// daemon's global default in force.
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_level: Option<&'a str>,
    /// Per-turn / per-session model override (OCEAN-36). Mirrors the daemon's
    /// `model_id: Option<String>`. Populated from the composer's model
    /// override pill (OCEAN-79); `None` keeps the daemon's global model.
    #[serde(skip_serializing_if = "Option::is_none")]
    model_id: Option<&'a str>,
    /// Images attached to the turn (OCEAN-138). Mirrors the daemon's
    /// `images: Option<Vec<TurnImage>>` (OCEAN-115) — when present the daemon
    /// emits one `Content::Image` block per entry on the first user message,
    /// enabling vision end-to-end. Omitted (and the daemon defaults to `None`)
    /// when no image was captured/picked for this turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    images: Option<Vec<TurnImage>>,
    /// Per-turn secret binding the daemon permission gate to this submitter
    /// (OCEAN-185 / OCEAN-314). Minted via `mint_decision_token()` (32 random
    /// hex bytes from `window.crypto.getRandomValues`). The same value must be
    /// replayed on every `/v1/permissions/{id}/decision` POST for this turn or
    /// the daemon returns 403. `None` leaves the gate unbound (legacy path).
    #[serde(skip_serializing_if = "Option::is_none")]
    decision_token: Option<&'a str>,
    /// Phase-2 structured client/browser context (OCEAN-40 / OCEAN-377). For the
    /// Chrome side panel this carries the same active-tab + open-tab snapshot we
    /// already emit as freeform `guidance` lines, but in the structured shape the
    /// daemon now consumes (`ocean_agent_sdk::ClientContext`). `None` on the
    /// detached web app and any non-extension surface, so the wire shape is
    /// byte-for-byte unchanged there and older daemons (which ignore an unknown
    /// field) stay back-compatible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_context: Option<ClientContext>,
    /// Named folder-as-agent to run this turn (OCEAN-378). Mirrors the daemon's
    /// `AgentTurnRequest::agent: Option<String>`: when set, the daemon resolves
    /// `<agents_root>/<agent>/instructions.md` and composes it as the turn's
    /// system prompt, overriding the surface profile (see
    /// `ocean-os/docs/specs/folder-as-agent.md`). `None` (the default, and every
    /// turn today — no UI selector ships yet) keeps the surface-profile behavior,
    /// so the wire shape is byte-for-byte unchanged and older daemons that ignore
    /// the field stay back-compatible. Discover names via `GET /v1/agents`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent: Option<&'a str>,
    /// Live canvas snapshot injected into THIS turn as surface context — the
    /// read-back half of the canvas loop ("Canvas ledger/state injected into
    /// turns as surface context", CLAUDE.md). A compact semantic summary
    /// ([`CanvasContext`]) of what the operator's canvas render currently shows:
    /// per-canvas component ids/kinds/text-slots/rects plus edges, folded from
    /// the same recorded patch log (`Daemon::canvas_patches`) the render derives
    /// from — so operator-won merges are visible to the next turn. `None`
    /// whenever no canvas holds a placed component, keeping the wire payload
    /// byte-for-byte unchanged for non-canvas sessions; daemons that don't know
    /// the field yet ignore it (the SDK's `AgentTurnRequest` sets no
    /// `deny_unknown_fields`). Daemon-side consumption spec lives with the
    /// ocean-os team (inject as turn context, never as the prompt itself).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    canvas: Option<CanvasContext>,
}

#[derive(Debug, Clone, Serialize)]
struct PlannerRealtimeContextRequest<'a> {
    project_id: &'a str,
    workspace_root: &'a str,
}

#[derive(Debug, Clone, Serialize)]
struct PlannerRealtimeSecretRequest<'a> {
    purpose: &'static str,
    planner_context: PlannerRealtimeContextRequest<'a>,
}

#[derive(Debug, Clone, Serialize)]
struct PlannerHandoffRequest<'a> {
    role: &'static str,
    kind: &'static str,
    content: &'a str,
}

#[derive(Debug, Clone, Serialize)]
struct AgentSessionCreateRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    /// Workspace anchor for the session. The daemon's
    /// `AgentSessionCreateRequest` deserializes this as a **required**
    /// `workspace_root` field (no serde alias for `cwd`) — sending `cwd` here
    /// made POST /v1/agent/sessions fail to deserialize, silently breaking
    /// surface session creation. Send `workspace_root` to match (OCEAN-62b).
    workspace_root: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_type: Option<&'a str>,
}

/// Default for [`AgentSessionCreateResponse::ok`]. The daemon's canonical
/// create response (`ocean-agent-sdk::AgentSessionCreateResponse`) is
/// `{session_id, cwd, client_type}` and carries **no `ok` field** — a 200 with
/// a `session_id` *is* the success signal. Requiring `ok` here made serde fail
/// with `missing field 'ok'`, so the surface never got a session id and chat
/// was 100% dead (OCEAN-124). Default a missing `ok` to `true` so the daemon's
/// real response decodes as success, while any future `ok:false` error shape is
/// still honored.
fn default_true() -> bool {
    true
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
struct AgentSessionCreateResponse {
    #[serde(default = "default_true")]
    ok: bool,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    workspace_root: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// One project in the picker catalogue (from `GET /v1/projects`).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ProjectInfo {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub workspace_root: String,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub git_dirty: Option<bool>,
    #[serde(default)]
    pub worktrees: Vec<WorktreeInfo>,
}

/// One worktree attached to a project (from `GET /v1/projects` enrichment).
/// Excludes the main worktree (the project root itself).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct WorktreeInfo {
    pub path: String,
    #[serde(default)]
    pub branch: Option<String>,
}

/// `POST /v1/projects` request body — mirrors `ocean-daemon::CreateProjectRequest`
/// (`ocean-os/crates/ocean-daemon/src/main.rs`). Field names are snake_case and
/// travel verbatim (the daemon deserializes plain Rust field names, no rename),
/// so the wire shape is exactly `{ "name": …, "workspace_root": … }`. The
/// daemon's optional `config` field is `#[serde(default)]` server-side, so we
/// OMIT it here and let the daemon apply its empty default — sending a partial
/// config would be surface-side guesswork the daemon doesn't need. A focused
/// test pins this exact two-field shape so a rename or leaked field fails the
/// build, not a silent 400 at create time.
#[derive(Debug, Clone, Serialize)]
struct ProjectCreateRequest {
    name: String,
    workspace_root: String,
}
/// `POST /v1/projects` response body — mirrors `ocean-daemon::ProjectResponse`.
/// `ok` is the authoritative success flag (the daemon sets it on both the 201
/// success and the 500 failure paths); `project` is present only on success;
/// `error` carries the daemon's reason on failure. All three are
/// `#[serde(default)]` so an unexpected proxy shape degrades to a clean
/// "project create failed" status instead of a serde panic.
#[derive(Debug, Clone, Deserialize)]
struct ProjectCreateResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    project: Option<ProjectInfo>,
    #[serde(default)]
    error: Option<String>,
}

// The POST response carries only metadata; reply text/ids arrive via SSE.
// We read `ok`/`error` for failure handling and ignore the rest.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct AgentTurnResponse {
    pub ok: bool,
    pub turn_id: String,
    pub session_id: String,
    pub status: String,
    /// Prefix the daemon stamps on this turn's SSE event ids so a client can
    /// correlate the HTTP response with the `GET /v1/agent/events` stream.
    /// `Option` + `serde(default)` for forward-compat with older daemons that
    /// don't emit it (OCEAN-81).
    #[serde(default)]
    pub event_id_prefix: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

// ── Filesystem directory listing (GET /v1/fs/dirs) ────────────────────

/// One directory entry from the daemon's filesystem listing.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct FsDirEntry {
    /// Basename of the directory (e.g. "ocean-os").
    pub name: String,
    /// Canonical absolute path of this directory.
    pub path: String,
    /// True when this directory contains a `.git` sub-directory.
    #[serde(default)]
    pub git: bool,
    /// True when the daemon reports this directory as a git repo root.
    #[serde(default)]
    pub is_repo: bool,
    /// Current branch when this is a git repo, if the daemon can resolve it.
    #[serde(default)]
    pub git_branch: Option<String>,
}
/// One file entry from the daemon's filesystem listing (`files=1`).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct FsFileEntry {
    /// Basename of the file (e.g. "main.rs").
    pub name: String,
    /// Canonical absolute path of this file.
    pub path: String,
    /// Size in bytes.
    #[serde(default)]
    pub size: u64,
}

/// Response from `GET /v1/fs/dirs?path=<path>`.
#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)]
pub struct FsDirsResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub home: String,
    #[serde(default)]
    pub dirs: Vec<FsDirEntry>,
    /// File entries — present only when the request opts in via `&files=1`.
    /// Absent (empty) for plain dir listings, so existing callers that ignore
    /// files are unaffected.
    #[serde(default)]
    pub files: Vec<FsFileEntry>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Fetch a directory listing from the daemon. `path` may include a leading
/// `~` for home-relative requests. Returns `None` on network or decode
/// errors — the caller handles the empty state.
///
/// Retained for historical compatibility; callers should prefer
/// `fetch_fs_dirs_with_files` which returns both dirs and files in one
/// daemon round-trip.
#[allow(dead_code)]
pub async fn fetch_fs_dirs(base_url: &str, path: &str) -> Option<FsDirsResponse> {
    let encoded = js_sys::encode_uri_component(path);
    let url = format!(
        "{}/v1/fs/dirs?path={}",
        base_url.trim_end_matches('/'),
        encoded.as_string().unwrap_or_else(|| path.to_string())
    );
    let resp = gloo_net::http::Request::get(&url).send().await.ok()?;
    resp.json::<FsDirsResponse>().await.ok()
}
/// Fetch a directory listing WITH file entries (`&files=1`). The desktop
/// workspace tree shows files alongside folders; everything else mirrors
/// [`fetch_fs_dirs`] exactly. The daemon's success responses carry no `ok`
/// field (serde default = false) — failure is signalled by `error`, so the
/// caller uses `error.is_none()` as the success predicate (same as the
/// files-panel helper).
pub async fn fetch_fs_dirs_with_files(base_url: &str, path: &str) -> Option<FsDirsResponse> {
    let encoded = js_sys::encode_uri_component(path);
    let url = format!(
        "{}/v1/fs/dirs?path={}&files=1",
        base_url.trim_end_matches('/'),
        encoded.as_string().unwrap_or_else(|| path.to_string())
    );
    let resp = gloo_net::http::Request::get(&url).send().await.ok()?;
    resp.json::<FsDirsResponse>().await.ok()
}

/// Response from `GET /v1/fs/file?path=<path>` — a single file's contents.
/// `binary` is true (with empty `content`) when the daemon detects a NUL in
/// the leading bytes; `truncated` is true when `content` hit the daemon's
/// cap (512 KiB). As with the dirs response, failure is signalled by `error`,
/// not the absence of `ok`.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct FsFileResponse {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub binary: bool,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub error: Option<String>,
}

/// Fetch a single file's contents from the daemon. `path` may include a
/// leading `~` for home-relative requests. Returns `None` on network or decode
/// errors — the caller handles the empty state.
pub async fn fetch_fs_file(base_url: &str, path: &str) -> Option<FsFileResponse> {
    let encoded = js_sys::encode_uri_component(path);
    let url = format!(
        "{}/v1/fs/file?path={}",
        base_url.trim_end_matches('/'),
        encoded.as_string().unwrap_or_else(|| path.to_string())
    );
    let resp = gloo_net::http::Request::get(&url).send().await.ok()?;
    resp.json::<FsFileResponse>().await.ok()
}

// ── GitHub read-model types (Lane C / TASK-32) ──────────────────────────
// Consumed by the repo panel's GitHubSection. All four routes are read-only
// public projections from the daemon's /v1/repo/github/{project_id}/* family.
// Every envelope carries a `rate` block for the embedded rate-limit bar.
//
// Naming convention: `Gh` prefix on surface-local DTOs to distinguish from
// upstream daemon types. Fields omit unknown (serde default) for forward compat.

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GhRate {
    pub remaining: u64,
    pub reset: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GhLabel {
    pub name: String,
    pub color: String,
    #[serde(default)]
    pub name_truncated: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GhPullListItem {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub title_truncated: bool,
    pub author: String,
    #[serde(default)]
    pub author_truncated: bool,
    pub branch: String,
    pub base_branch: String,
    pub head_sha: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub labels: Vec<GhLabel>,
    #[serde(default)]
    pub labels_truncated: bool,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GhPullListEnvelope {
    pub ok: bool,
    #[serde(default)]
    pub pulls: Vec<GhPullListItem>,
    #[serde(default)]
    pub pulls_truncated: bool,
    pub page: u32,
    pub per_page: u32,
    pub has_more: bool,
    #[serde(default)]
    pub rate: GhRate,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GhPullDetail {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub title_truncated: bool,
    pub author: String,
    #[serde(default)]
    pub author_truncated: bool,
    pub branch: String,
    pub base_branch: String,
    pub head_sha: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub body_truncated: bool,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub labels: Vec<GhLabel>,
    #[serde(default)]
    pub labels_truncated: bool,
    pub state: String,
    pub mergeable: Option<bool>,
    #[serde(default)]
    pub requested_reviewers: Vec<String>,
    #[serde(default)]
    pub requested_reviewers_truncated: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GhPullDetailEnvelope {
    pub ok: bool,
    pub pull: GhPullDetail,
    #[serde(default)]
    pub rate: GhRate,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GhCheckRun {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub name_truncated: bool,
    pub status: String,
    pub conclusion: Option<String>,
    pub details_url: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GhChecksEnvelope {
    pub ok: bool,
    #[serde(default)]
    pub checks: Vec<GhCheckRun>,
    #[serde(default)]
    pub checks_truncated: bool,
    #[serde(default)]
    pub rate: GhRate,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GhReview {
    pub reviewer: String,
    #[serde(default)]
    pub reviewer_truncated: bool,
    pub state: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub body_truncated: bool,
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GhReviewsEnvelope {
    pub ok: bool,
    #[serde(default)]
    pub reviews: Vec<GhReview>,
    #[serde(default)]
    pub reviews_truncated: bool,
    pub page: u32,
    pub per_page: u32,
    pub has_more: bool,
    #[serde(default)]
    pub rate: GhRate,
}

/// Fetch pull requests for a project.
///
/// `state` is "open", "closed", or "all". The daemon caps decimals in
/// `page`/`per_page` for us.
pub async fn fetch_gh_pulls(
    base_url: &str,
    project_id: &str,
    state: &str,
    page: u32,
    per_page: u32,
) -> Option<GhPullListEnvelope> {
    let url = format!(
        "{}/v1/repo/github/{}/pulls?state={}&page={}&per_page={}",
        base_url.trim_end_matches('/'),
        project_id,
        state,
        page,
        per_page
    );
    let resp = gloo_net::http::Request::get(&url).send().await.ok()?;
    resp.json::<GhPullListEnvelope>().await.ok()
}

/// Fetch one pull request's detail body + metadata.
pub async fn fetch_gh_pull_detail(
    base_url: &str,
    project_id: &str,
    number: u64,
) -> Option<GhPullDetailEnvelope> {
    let url = format!(
        "{}/v1/repo/github/{}/pulls/{}",
        base_url.trim_end_matches('/'),
        project_id,
        number
    );
    let resp = gloo_net::http::Request::get(&url).send().await.ok()?;
    resp.json::<GhPullDetailEnvelope>().await.ok()
}

/// Fetch check runs for a full 40-hex head SHA.
pub async fn fetch_gh_checks(
    base_url: &str,
    project_id: &str,
    head_sha: &str,
) -> Option<GhChecksEnvelope> {
    let url = format!(
        "{}/v1/repo/github/{}/head-sha/{}/checks",
        base_url.trim_end_matches('/'),
        project_id,
        head_sha
    );
    let resp = gloo_net::http::Request::get(&url).send().await.ok()?;
    resp.json::<GhChecksEnvelope>().await.ok()
}

/// Fetch reviews for a pull request (paginated).
pub async fn fetch_gh_reviews(
    base_url: &str,
    project_id: &str,
    number: u64,
    page: u32,
    per_page: u32,
) -> Option<GhReviewsEnvelope> {
    let url = format!(
        "{}/v1/repo/github/{}/pulls/{}/reviews?page={}&per_page={}",
        base_url.trim_end_matches('/'),
        project_id,
        number,
        page,
        per_page
    );
    let resp = gloo_net::http::Request::get(&url).send().await.ok()?;
    resp.json::<GhReviewsEnvelope>().await.ok()
}
/// `component_id` (upsert — last write wins); rendered by the same
/// [`ComponentView`](crate::components::ComponentView) as an inline component.
#[derive(Debug, Clone)]
pub struct PinnedWidget {
    pub component_id: String,
    pub kind: String,
    pub props: Value,
}

/// Reactive handle to the daemon. Owns the live turns vec + connection
/// status; surfaces APIs to send prompts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlannerStreamKind {
    Agent,
    Permission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlannerStreamLifecycle {
    agent: EventSourceState,
    permission: EventSourceState,
}

impl Default for PlannerStreamLifecycle {
    fn default() -> Self {
        Self {
            agent: EventSourceState::Closed,
            permission: EventSourceState::Closed,
        }
    }
}

impl PlannerStreamLifecycle {
    fn transition(&mut self, kind: PlannerStreamKind, state: EventSourceState) {
        match kind {
            PlannerStreamKind::Agent => self.agent = state,
            PlannerStreamKind::Permission => self.permission = state,
        }
    }

    fn ready(&self) -> bool {
        self.agent == EventSourceState::Open && self.permission == EventSourceState::Open
    }
}

#[derive(Default)]
struct PlannerStreamSources {
    session_id: String,
    generation: u64,
    agent: Option<EventSource>,
    permission: Option<EventSource>,
    lifecycle: PlannerStreamLifecycle,
}

#[derive(Clone)]
pub struct Daemon {
    pub url: RwSignal<String>,
    pub turns: RwSignal<Vec<Turn>>,
    pub streaming: RwSignal<bool>,
    pub session_id: RwSignal<Option<String>>,
    pub status: RwSignal<String>,
    /// `(concise status, full raw error payload)` for the header status chip's
    /// `title` tooltip. The visible chip stays concise; transcript Err blocks
    /// carry the same detail inline (design doc §3 Failure surfaces). The view
    /// shows the tooltip only while `status` equals the stored concise string.
    pub status_detail: RwSignal<Option<(String, String)>>,
    pub cwd: RwSignal<String>,
    /// Whether the proxy reports a usable xAI key (voice STT/TTS available).
    /// Rendered independently of the SSE `status` string so it isn't clobbered
    /// by connect()'s "connecting…"/"connected" transitions.
    pub voice_ready: RwSignal<bool>,
    /// Who `/api/config` says is signed in. Empty until bootstrap answers, and
    /// empty in single-operator mode.
    ///
    /// This is a SIGNAL, not a boot snapshot, because bootstrap is a network
    /// round-trip that resolves AFTER the rooms panel is constructed. Reading
    /// it once at construction is how the first session after every login ended
    /// up acting under the previous identity.
    pub adopted_user_id: RwSignal<String>,
    /// Display name for [`Self::adopted_user_id`].
    pub adopted_display_name: RwSignal<String>,
    /// Google Maps JS API key from /api/config, used by the map component to
    /// load the Maps script. Empty until bootstrap (and when no key is set).
    pub maps_key: RwSignal<String>,
    /// Map ID for the map's visual style (from /api/config).
    pub maps_map_id: RwSignal<String>,
    /// Default LiveKit room id for Ocean collaboration surfaces.
    pub livekit_room_id: RwSignal<String>,
    /// Same-origin token path for joining the configured LiveKit room.
    pub livekit_token_path: RwSignal<String>,
    /// tldraw sync endpoint hint, empty when canvases should stay local-only.
    pub tldraw_sync_uri: RwSignal<String>,
    /// Monotonic connection generation. Incremented before opening an SSE stream
    /// so reconnect/switch/new-session calls retire older streams instead of
    /// applying every delta multiple times.
    sse_generation: RwSignal<u64>,
    /// Monotonic session-INTENT generation, distinct from `sse_generation`. It is
    /// bumped whenever the user establishes a new cockpit focus — a New Session or
    /// a switch to another session — but NOT by a session's own `connect()`. A
    /// fresh-session create captures this at submit time and adopts its result
    /// only while the captured value is still current, so a stale create response
    /// cannot steal focus from a session started while its POST was in flight
    /// (TASK-43, finding 1). See [`admit_create_intent`].
    session_intent_generation: RwSignal<u64>,
    /// Which machine the catalogues on screen belong to.
    ///
    /// Bumped by [`Self::reattach_to_selected_device`]. Every fetch that
    /// REPLACES a catalogue wholesale captures it before its await and drops a
    /// response that arrives after a switch — otherwise the old machine's
    /// models or projects win the last write and the picker offers choices the
    /// attached daemon does not own, which a later turn then submits.
    device_epoch: RwSignal<u64>,
    /// First-open readiness for the active agent and permission streams. Voice
    /// Planner awaits both before posting its first normal turn, then reconciles
    /// the durable permission snapshot so no gate can be missed in the handoff.
    /// The ordinary fresh-session create awaits the same markers before its first
    /// turn so `turn_started` and early frames are not tailed into the void
    /// (TASK-43, finding 5).
    agent_stream_ready: RwSignal<Option<(String, u64)>>,
    permission_stream_ready: RwSignal<Option<(String, u64)>>,
    permission_revision: RwSignal<u64>,
    planner_stream_sources: send_wrapper::SendWrapper<Rc<RefCell<PlannerStreamSources>>>,
    /// Explicit handles for the two browser EventSource transports. A generation
    /// guard prevents stale events from mutating state, but cannot wake an idle
    /// stream; closing these on focus change releases the browser connection
    /// slots immediately.
    #[cfg(target_arch = "wasm32")]
    agent_event_source: EventSourceSlot,
    #[cfg(target_arch = "wasm32")]
    permission_event_source: EventSourceSlot,
    /// Legacy guard retained for older daemon/proxy builds. New surfaces create
    /// sessions explicitly before posting turns, so this should stay false.
    awaiting_session_adoption: RwSignal<bool>,
    /// Current session title (set on SessionCreated or when switching).
    pub session_title: RwSignal<String>,
    /// Fetched session list from the daemon.
    pub session_list: RwSignal<Vec<SessionSummary>>,
    /// Global newest-request generation shared by the thin refresh spawner and the
    /// sessions-panel A1 poll rail. Only the latest claimant may replace
    /// `session_list`, so overlapping paginated drains cannot land out of order.
    session_fetch_ticket: RwSignal<u64>,
    /// Cross-session runtime snapshots used by the read-only Island activity
    /// projection. The daemon remains authoritative; these are replaced only
    /// after a successful snapshot decode.
    pub request_list: RwSignal<Vec<RequestSnapshot>>,
    pub permission_list: RwSignal<Vec<PermissionSnapshot>>,
    /// TASK-69 seam 2: `None` when the cross-session permission attention list is
    /// authoritative (last poll settled); `Some(reason)` when the poll failed and
    /// the last-known list may be stale (already-resolved gates could still show).
    /// The Island renders a "couldn't refresh" note rather than presenting the
    /// stale set as confidently authoritative.
    pub permission_list_stale: RwSignal<Option<String>>,
    /// Daemon-owned transcript recall results. A monotonically increasing
    /// generation prevents a slow older query from replacing a newer one.
    pub history_results: RwSignal<Vec<HistorySearchHit>>,
    pub history_searching: RwSignal<bool>,
    pub history_error: RwSignal<Option<String>>,
    history_search_generation: RwSignal<u64>,
    /// Prevent the ambient three-second refresh from stacking requests when a
    /// slow/offline daemon has not completed the previous snapshot yet.
    attention_fetching: RwSignal<bool>,
    /// Request ids whose explicit cancel POST is in flight. This is transport
    /// state only; the rendered lifecycle state still comes from the daemon's
    /// request snapshot.
    pub request_cancellations: RwSignal<Vec<String>>,
    /// Token usage from the most recently finished turn (real provider numbers
    /// when available). `None` until the first turn finishes.
    pub last_turn_tokens: RwSignal<Option<TokenStats>>,
    /// Running token total across all turns in this session. Reset on
    /// new_session / switch_session.
    pub session_tokens: RwSignal<TokenStats>,
    /// Current model id, learned from TurnStarted (and GET /v1/models). Shown
    /// live in the header so a mid-session swap is visible.
    pub model: RwSignal<Option<String>>,
    /// The catalogue of selectable models from GET /v1/models.
    pub models: RwSignal<Vec<ModelInfo>>,
    /// The selected project id, sent as `project_id` on every turn so the daemon
    /// binds to that project's directory. Persisted in localStorage so the
    /// choice survives reload. `None` = no project (turns then need a real cwd).
    pub project: RwSignal<Option<String>>,
    /// The catalogue of projects from GET /v1/projects.
    pub projects: RwSignal<Vec<ProjectInfo>>,
    /// turn_id of the in-flight turn, captured from TurnStarted — the halt
    /// button cancels this via POST /v1/requests/{id}/cancel.
    pub active_turn_id: RwSignal<Option<String>>,
    /// True while Ocean is actively driving the browser. Set from the daemon's
    /// `browser_activity` SSE event. The extension side panel uses this to take
    /// focus during browser work and release afterward; other surfaces can show
    /// a passive "Ocean is driving the browser" cue.
    pub browser_active: RwSignal<bool>,
    /// The most recent live browser action the agent performed, e.g.
    /// `"browser_navigate"`. Captured from `ToolCallStarted` for any `browser_*`
    /// tool and shown next to the browser-control indicator so the user can see
    /// *what* the agent just did in the browser, not only that it's active. The
    /// `browser_activity` event itself only carries `{ active }`, so we derive
    /// the action label from the tool-call stream (OCEAN-92).
    pub browser_last_action: RwSignal<Option<String>>,
    // ── TASK-46: sync_pending — mid-turn content recovery on fresh attach ──────
    //
    // When a surface attaches or reconnects during an active turn, we enter
    // generation+session-scoped sync_pending: committed snapshot renders
    // immediately, partial active-turn tail is suppressed, and a bounded
    // abortable detail poll runs until terminal state, then atomically replaces
    // the transcript. Zero daemon changes. See `.stitchpad/artifacts/`.
    //
    /// True while the sync_pending poll is active — the apply_event suppression
    /// gate and the UI "syncing current turn" affordance both read this.
    pub sync_pending: RwSignal<bool>,
    /// Monotonic sync cycle counter, never read from `sse_generation`, never
    /// decremented. `stop_sync_poll` bumps it; every async write and
    /// `in_flight` release must match the captured value. ABA-proof.
    sync_cycle: RwSignal<u64>,
    /// Poll interval handle (leptos `set_interval_with_handle`). Cleared on
    /// stop/stalled/switch.
    sync_interval: RwSignal<Option<IntervalHandle>>,
    /// Abort handle for the in-flight detail fetch so a switch/close can cancel
    /// it immediately without waiting for the HTTP request to settle.
    sync_fetch_abort: RwSignal<Option<AbortHandle>>,
    /// Prevents overlapping poll ticks. Set before the fetch spawns, cleared
    /// after the fetch settles (success/error/abort). A tick that sees this
    /// true skips — the in-flight fetch will release it.
    sync_in_flight: RwSignal<bool>,
    /// Absolute deadline (`now_millis()` ms). When the current time
    /// exceeds this, the poll transitions to stalled and stops ticking.
    /// Checked BEFORE the `in_flight` guard so a hung fetch cannot block the
    /// timeout from being observed.
    pub sync_deadline: RwSignal<Option<f64>>,
    /// Monotonic activity revision bumped by admitted `TurnStarted` and
    /// `TurnFinished`. Captured at projection start; mismatch at commit
    /// → `ProjectionOutcome::Live` (re-quarantine). Covers both directions:
    /// live-detail-stale-when-terminal AND terminal-detail-stale-when-new-turn.
    activity_revision: RwSignal<u64>,
    /// Edge-safe kick counter for the sync poll. Bumped by `TurnFinished` during
    /// sync_pending so the live SSE frame triggers an immediate detail refetch
    /// without waiting for the next 2s interval tick. An Effect watches this.
    sync_poll_kick: RwSignal<u64>,
    /// Permission requests awaiting an allow/deny decision, oldest first. Each
    /// blocks its turn on the daemon until decided. Populated from the
    /// `/v1/events` control stream (`permission_request`) and cleared on
    /// `permission_decision` or a successful decision POST. Multiple can stack.
    pub permission_view: RwSignal<PermissionView>,
    /// TASK-69: permission ids RESOLVED (decided via a `permission_decision`
    /// frame or a successful decision POST) since the last authoritative Fresh
    /// snapshot — decision tombstones. A session-load projection whose rich
    /// snapshot degrades must fold the daemon's `SessionDetail.pending_permissions`
    /// MINUS these ids, so a gate decided DURING the load (its detail id is now
    /// stale) cannot resurrect as a warning, while an unrelated interleaved
    /// request still leaves a genuinely-open detail id visible (a scalar revision
    /// gate cannot tell those apart — codex HOLD on 4cfb5c2). Cleared on an
    /// authoritative Fresh commit and on session switch; a reused permission id
    /// (a fresh `permission_request` for the same id) clears its own tombstone.
    permission_tombstones: RwSignal<Vec<String>>,
    /// TASK-69: a monotonically-increasing claim ticket ordering the TWO
    /// independent publishers of permission/tombstone state — the agent loop's
    /// `commit_session_projection` and the permission loop's standalone reconcile.
    /// Each permission-fetching op CLAIMS an epoch at its START; the shared
    /// `publish_permission_view` publishes ONLY if the claimant is still the
    /// latest, so a stale (older-claim) result cannot overwrite a newer
    /// authoritative one and resurrect a decided gate (codex HOLD on 8b41fee1:
    /// nothing else orders the two publishers — `permission_revision` moves only
    /// on frames/POST, not on snapshot publication).
    permission_epoch: RwSignal<u64>,
    /// Per-turn reasoning-effort override (OCEAN-79). Holds the serialized
    /// `ThinkingLevel` string (`off` / `low` / `medium` / `high`) the daemon
    /// expects, or `None` to leave the daemon's global default in force. Set
    /// from the composer's thinking-level selector and sent on every
    /// `AgentTurnRequest::thinking_level`. Persisted in localStorage so the
    /// choice survives reload. `None` = unchanged behavior.
    pub thinking_level: RwSignal<Option<String>>,
    /// Per-turn model override (OCEAN-79). Distinct from `model` /
    /// [`set_model`], which globally hot-swaps the daemon via `POST /v1/model`;
    /// this rides on `AgentTurnRequest::model_id` so the turn runs with the
    /// chosen model without mutating the daemon's global selection. `None` =
    /// daemon default. Persisted in localStorage. Drawn from the same `models`
    /// catalogue fetched from `GET /v1/models`.
    pub model_override: RwSignal<Option<String>>,
    /// Named folder-as-agent for the NEXT turn (OCEAN-378). Rides on
    /// `AgentTurnRequest::agent` so the turn runs under
    /// `<agents_root>/<agent>/instructions.md` as a system-prompt override,
    /// without mutating any global daemon state. `None` (the default) leaves the
    /// surface-profile behavior untouched. No composer selector ships yet — this
    /// signal exists so a future dropdown (fed by `GET /v1/agents`) can set it via
    /// [`set_agent`]; until then every turn sends `None` and the wire shape is
    /// unchanged. Not persisted: a named agent is a deliberate per-turn choice.
    pub agent_override: RwSignal<Option<String>>,
    /// Images staged for the NEXT turn (OCEAN-138). A captured visible tab (or a
    /// picked image) lands here and rides along on the next `send_prompt` as
    /// `AgentTurnRequest::images`, then is drained. Empty = no attachment, so the
    /// field is omitted and the daemon behaves exactly as before.
    pub pending_images: RwSignal<Vec<TurnImage>>,
    /// Canvas patches the agent has applied this session, oldest first
    /// (OCEAN-178). Populated from the daemon's `surface_patch` SSE event. The
    /// GPUI native shell renders these on a full `CanvasLedger`; the web surface
    /// renders a basic representation of the patch stream so the data is no
    /// longer silently dropped at the transport layer. Reset on session change.
    pub canvas_patches: RwSignal<Vec<CanvasPatchEntry>>,
    /// Pinned widgets docked in the persistent rail (map/player/metrics that
    /// stay visible across turns, outside the chat scroll). A `component_render`
    /// whose `props.placement == "pinned"` docks here instead of into the
    /// transcript, so it never duplicates inline. Id-keyed via [`PinnedWidget`]
    /// (Vec-backed for ordered, Leptos-idiomatic reactivity). Session-scoped —
    /// cleared on switch/new/clear, rebuilt on reconnect/reload.
    pub pinned_widgets: RwSignal<Vec<PinnedWidget>>,
    /// Per-turn CSPRNG secret sent on the turn submission and replayed on
    /// every permission-decision POST for that turn (OCEAN-185 / OCEAN-314).
    /// Minted by `mint_decision_token()` at `dispatch_prompt` time and stored
    /// here so `decide_permission` can retrieve the same value. Cleared when a
    /// new turn begins.
    pub active_decision_token: RwSignal<Option<String>>,
    /// Per-request decision authority (TASK-44, finding 4). Maps a daemon
    /// request id (identical to the turn id — the daemon mints
    /// `AgentTurnId(request_id)`) to the CSPRNG decision token THIS surface
    /// minted when it submitted that turn, plus the session it belongs to.
    /// `decide_permission` resolves a card's token from this map by the card's
    /// originating `request_id`, so a later turn's token can never be replayed
    /// against an earlier turn's gate, and a card raised by another surface (no
    /// entry here) is non-actionable. Entries are pruned to the session's
    /// still-live requests on every projection commit; the map is never sent
    /// over SSE and never reaches any daemon read model.
    decision_authority: RwSignal<HashMap<String, DecisionGrant>>,
    /// True while a `create_project` POST is in flight. Drives the Sessions
    /// panel's Create button label/disabled state and gates the modal close on
    /// the success edge. Reset to false on every terminal branch.
    pub project_create_pending: RwSignal<bool>,
    /// Inline error for the Sessions create form. `Some(msg)` keeps the form
    /// open with the error rendered under the inputs; `None` otherwise.
    pub project_create_error: RwSignal<Option<String>>,
    /// File-preview intent routed from the host (Tauri file-open, transcript
    /// path-click, or a future deep link). `(path, generation)` — the workspace
    /// rail consumes this once, then resets to None so it cannot re-fire.
    pub preview_file_intent: RwSignal<Option<(String, u64)>>,
}

/// A selectable model, mirroring the daemon's KnownModel plus the readiness
/// the daemon reports for it (`GET /v1/models` serializes ReadyModel flat:
/// id/provider/label top-level, `ready` and optional `credential_source`
/// alongside).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub label: String,
    /// Whether the daemon could route a turn to this model right now
    /// (credential visible to that process — configuration truth, not a
    /// liveness probe). Deliberately `Option`, not a defaulted bool: a daemon
    /// predating readiness omits the field, and defaulting to `false` would
    /// render its whole catalogue as unusable.
    #[serde(default)]
    pub ready: Option<bool>,
    /// Where the daemon resolved the credential, kept as loose JSON on
    /// purpose: `fetch_models` throws away the WHOLE catalogue on any decode
    /// error, so a typed enum here would let one new daemon-side variant
    /// blank the picker. Formatted by [`credential_source_hint`].
    #[serde(default)]
    pub credential_source: Option<Value>,
}

impl ModelInfo {
    /// Why this model cannot run, when the daemon has said so. `None` for a
    /// ready model — and for a daemon that never reported readiness, so an
    /// older daemon's catalogue renders exactly as it did before the field
    /// existed.
    pub fn unready_reason(&self) -> Option<String> {
        if self.ready != Some(false) {
            return None;
        }
        // Today's daemon attaches credential_source only to entries whose
        // credential actually resolved, so an unready entry usually names
        // nothing — fall back to the provider, which is what the operator
        // needs to go configure.
        Some(
            match self
                .credential_source
                .as_ref()
                .and_then(credential_source_hint)
            {
                Some(hint) => format!("no credential: {hint}"),
                None if self.provider.is_empty() => "no credential".to_string(),
                None => format!("no credential for {}", self.provider),
            },
        )
    }
}

/// Human-readable name for a `credential_source` wire value. The daemon's
/// enum is externally tagged snake_case — `{"env":{"name":"…"}}`,
/// `{"ocean_auth_file":{"path":"…"}}`, `{"codex_cli_auth_file":{"path":"…"}}`,
/// or the bare string `"not_required"` — and grows variants over time, so an
/// unrecognized shape degrades to its tag instead of failing anything.
fn credential_source_hint(source: &Value) -> Option<String> {
    match source {
        Value::String(tag) => Some(tag.replace('_', " ")),
        Value::Object(map) => {
            let (tag, body) = map.iter().next()?;
            match body
                .get("name")
                .or_else(|| body.get("path"))
                .and_then(Value::as_str)
            {
                Some(detail) => Some(detail.to_string()),
                None => Some(tag.replace('_', " ")),
            }
        }
        _ => None,
    }
}

/// Token usage for a turn (or summed for a session), mirrored from the daemon's
/// TurnFinished event. All counts are real provider usage when reported.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TokenStats {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    /// Tokens/sec for the last turn; not meaningful when summed, so a session
    /// total leaves this at 0.
    pub tokens_per_second: f64,
}

impl TokenStats {
    pub fn total(&self) -> u64 {
        self.input + self.output
    }
}

/// Minimal project reference the daemon attaches to a session (OCEAN-228).
/// The daemon serves it today for exact workspace-root matches; when absent
/// (worktree/subdir sessions, older daemons) grouping falls back to
/// `workspace_root`/`cwd` prefix matching against the catalogue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OwningProjectRef {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// Client-facing run state of an active turn, mirrored from
/// `ocean_core::SessionRunState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRunState {
    /// No active turn (or stored/archived session).
    Stored,
    /// Agent is actively generating a response.
    Running,
    /// Turn paused — waiting for operator permission on a tool.
    WaitingForPermission,
    /// Operator requested cancel; the turn is winding down.
    Cancelling,
    /// Turn was cancelled before completion.
    Cancelled,
    /// Turn completed successfully.
    Completed,
    /// Turn terminated with an error.
    Errored,
    /// Future variant from a newer daemon — treat as idle/unknown.
    #[serde(other)]
    Unknown,
}

impl SessionRunState {
    /// True while a turn is still live and cancellable — the daemon is
    /// generating, blocked on an operator gate, or winding down a cancel. A
    /// live snapshot must preserve a running/cancellable projection (Stop
    /// target intact); only a terminal state clears the active turn (TASK-44,
    /// verdict finding 3). `Stored`/`Cancelled`/`Completed`/`Errored`/`Unknown`
    /// are all terminal for projection purposes.
    fn is_live(self) -> bool {
        matches!(
            self,
            Self::Running | Self::WaitingForPermission | Self::Cancelling
        )
    }
}

/// Summary of a session, matching the daemon's AgentSessionSummary. `cwd`,
/// `workspace_root`, and `owning_project` are all `#[serde(default)]` — an
/// old daemon serves only `cwd`, and the sessions panel groups on whatever
/// project signal is present (owning_project first, else root match).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub workspace_root: Option<String>,
    #[serde(default)]
    pub owning_project: Option<OwningProjectRef>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub turn_count: u32,
    /// Active turn id, present when an agent turn is in-flight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn: Option<String>,
    /// Run state of active_turn, derived atomically by the daemon.
    /// None when active_turn is None. Old daemons omit it —
    /// `#[serde(default)]` ensures backward compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_state: Option<SessionRunState>,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SessionDetailResponse {
    ok: bool,
    #[serde(default)]
    session: Option<SessionDetail>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct SessionDetail {
    id: String,
    title: String,
    model: String,
    #[serde(default)]
    workspace_root: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    transcript: Vec<SessionTranscriptEntry>,
    /// Per-message tool-call/result context the daemon derives alongside the
    /// transcript. We read it to recover `component_render` props on reconnect:
    /// `ComponentRender` is an SSE-only side-channel event that is never folded
    /// into a transcript text entry, but the assistant tool-call that produced it
    /// IS persisted here with its full `arguments` (id/kind/props). Replaying
    /// those lets rendered components survive a mid-turn SSE reconnect instead of
    /// vanishing (OCEAN-382).
    #[serde(default)]
    tool_context: Vec<SessionToolContext>,
    /// Ids of permissions currently awaiting a decision on this session. The
    /// rich card detail (tool/reason/args/request_id) comes from
    /// `GET /v1/permissions`; this list only proves the session has open gates.
    #[serde(default)]
    pending_permissions: Vec<String>,
    /// The daemon's atomically-derived run state for the live turn, if any
    /// (`SessionDetail::state`). `Option` + `#[serde(default)]` for backward
    /// compatibility with older daemons that omit it. Applied together with
    /// `active_requests` to restore a truthful running/waiting/cancelling
    /// projection on switch/reconnect (TASK-44, finding 3).
    #[serde(default)]
    state: Option<SessionRunState>,
    /// The daemon's still-live request ids for this session
    /// (`SessionDetail::active_requests`), most-recent first, terminal requests
    /// already filtered out. Each id equals a turn id (the daemon mints
    /// `AgentTurnId(request_id)`). The first entry is the live turn's Stop
    /// target restored on reconnect (TASK-44, finding 3).
    #[serde(default)]
    active_requests: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PermissionsResponse {
    ok: bool,
    #[serde(default)]
    permissions: Vec<PermissionStatusWire>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PermissionStatusWire {
    permission_id: String,
    /// Originating request/turn id (`PermissionStatus::request_id`). Required on
    /// the wire from a current daemon; `Option` + default for older daemons.
    /// Binds the card to this surface's decision token (TASK-44, finding 4).
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    tool: String,
    reason: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Clone, Deserialize)]
struct SessionTranscriptEntry {
    role: String,
    #[serde(default)]
    text: String,
    /// Correlates a `tool` transcript entry with its originating `call` in
    /// `tool_context` (so a `component_render` result can be matched to the
    /// tool-call args carrying its props). `None` for non-tool entries.
    #[serde(default)]
    tool_call_id: Option<String>,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    is_error: Option<bool>,
}

/// Tool-call / tool-result context for a persisted message, mirroring the
/// daemon's `SessionToolContext`. We only need the `call` entries' `arguments`
/// (which carry `component_render` props) for replay, but decode the full shape
/// for forward-compatibility.
#[derive(Debug, Clone, Deserialize)]
struct SessionToolContext {
    /// `"call"` or `"result"`.
    kind: String,
    tool_call_id: String,
    tool_name: String,
    #[serde(default)]
    arguments: Option<serde_json::Value>,
}

/// Outcome of `commit_session_projection`.
///
/// New session projections always return `Terminal` after committing the full
/// daemon transcript. `Live` remains only for the retired TASK-46 poll cleanup
/// path; session switches and reconnects no longer enter that quarantine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectionOutcome {
    Terminal,
    Live,
}

const SESSION_PROJECTION_ACTIVITY_CHANGED: &str =
    "session activity changed while loading; retrying authoritative snapshot";

/// A terminal HTTP snapshot can briefly race a TurnStarted frame received
/// before the daemon registers the request in its durable request map. Preserve
/// the already-admitted local Stop target while still committing the complete
/// transcript instead of deferring the whole projection.
fn should_preserve_local_live_state(query_is_live: bool, local_has_live_turn: bool) -> bool {
    !query_is_live && local_has_live_turn
}

impl Daemon {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: RwSignal::new(url.into()),
            turns: RwSignal::new(Vec::new()),
            streaming: RwSignal::new(false),
            session_id: RwSignal::new(None),
            adopted_user_id: RwSignal::new(String::new()),
            adopted_display_name: RwSignal::new(String::new()),
            status: RwSignal::new("disconnected".into()),
            status_detail: RwSignal::new(None),
            cwd: RwSignal::new(default_cwd()),
            voice_ready: RwSignal::new(false),
            maps_key: RwSignal::new(String::new()),
            maps_map_id: RwSignal::new(String::new()),
            livekit_room_id: RwSignal::new(String::new()),
            livekit_token_path: RwSignal::new(String::new()),
            tldraw_sync_uri: RwSignal::new(String::new()),
            sse_generation: RwSignal::new(0),
            session_intent_generation: RwSignal::new(0),
            device_epoch: RwSignal::new(0),
            agent_stream_ready: RwSignal::new(None),
            permission_stream_ready: RwSignal::new(None),
            permission_revision: RwSignal::new(0),
            planner_stream_sources: send_wrapper::SendWrapper::new(Rc::new(RefCell::new(
                PlannerStreamSources::default(),
            ))),
            #[cfg(target_arch = "wasm32")]
            agent_event_source: Arc::new(Mutex::new(None)),
            #[cfg(target_arch = "wasm32")]
            permission_event_source: Arc::new(Mutex::new(None)),
            awaiting_session_adoption: RwSignal::new(false),
            session_title: RwSignal::new(String::new()),
            session_list: RwSignal::new(Vec::new()),
            session_fetch_ticket: RwSignal::new(0),
            request_list: RwSignal::new(Vec::new()),
            permission_list: RwSignal::new(Vec::new()),
            permission_list_stale: RwSignal::new(None),
            history_results: RwSignal::new(Vec::new()),
            history_searching: RwSignal::new(false),
            history_error: RwSignal::new(None),
            history_search_generation: RwSignal::new(0),
            attention_fetching: RwSignal::new(false),
            request_cancellations: RwSignal::new(Vec::new()),
            last_turn_tokens: RwSignal::new(None),
            session_tokens: RwSignal::new(TokenStats::default()),
            model: RwSignal::new(None),
            models: RwSignal::new(Vec::new()),
            // Restore the last-selected project from localStorage so the choice
            // survives a reload.
            project: RwSignal::new(load_persisted_project()),
            projects: RwSignal::new(Vec::new()),
            active_turn_id: RwSignal::new(None),
            browser_active: RwSignal::new(false),
            browser_last_action: RwSignal::new(None),
            permission_view: RwSignal::new(PermissionView::default()),
            permission_tombstones: RwSignal::new(Vec::new()),
            permission_epoch: RwSignal::new(0),
            // Restore the last-selected per-turn overrides from localStorage so
            // the choices survive a reload (like `project`).
            thinking_level: RwSignal::new(load_persisted_thinking_level()),
            model_override: RwSignal::new(load_persisted_model_override()),
            // Not persisted: a named folder-as-agent is a deliberate per-turn
            // choice, not a sticky preference. Starts `None` (surface profile).
            agent_override: RwSignal::new(None),
            pending_images: RwSignal::new(Vec::new()),
            canvas_patches: RwSignal::new(Vec::new()),
            pinned_widgets: RwSignal::new(Vec::new()),
            active_decision_token: RwSignal::new(None),
            decision_authority: RwSignal::new(HashMap::new()),
            project_create_pending: RwSignal::new(false),
            project_create_error: RwSignal::new(None),
            preview_file_intent: RwSignal::new(None),
            sync_pending: RwSignal::new(false),
            sync_cycle: RwSignal::new(0),
            sync_interval: RwSignal::new(None),
            sync_fetch_abort: RwSignal::new(None),
            sync_in_flight: RwSignal::new(false),
            sync_deadline: RwSignal::new(None),
            activity_revision: RwSignal::new(0),
            sync_poll_kick: RwSignal::new(0),
        }
    }

    /// A dummy daemon that does nothing. Useful for component previews and
    /// tests — component interactions will no-op gracefully.
    #[allow(dead_code)]
    pub fn dummy() -> Self {
        Self {
            url: RwSignal::new("http://127.0.0.1:4780".into()),
            turns: RwSignal::new(Vec::new()),
            streaming: RwSignal::new(false),
            session_id: RwSignal::new(None),
            status: RwSignal::new("dummy".into()),
            adopted_user_id: RwSignal::new(String::new()),
            adopted_display_name: RwSignal::new(String::new()),
            status_detail: RwSignal::new(None),
            cwd: RwSignal::new("/".into()),
            voice_ready: RwSignal::new(false),
            maps_key: RwSignal::new(String::new()),
            maps_map_id: RwSignal::new(String::new()),
            livekit_room_id: RwSignal::new(String::new()),
            livekit_token_path: RwSignal::new(String::new()),
            tldraw_sync_uri: RwSignal::new(String::new()),
            sse_generation: RwSignal::new(0),
            session_intent_generation: RwSignal::new(0),
            device_epoch: RwSignal::new(0),
            agent_stream_ready: RwSignal::new(None),
            permission_stream_ready: RwSignal::new(None),
            permission_revision: RwSignal::new(0),
            planner_stream_sources: send_wrapper::SendWrapper::new(Rc::new(RefCell::new(
                PlannerStreamSources::default(),
            ))),
            #[cfg(target_arch = "wasm32")]
            agent_event_source: Arc::new(Mutex::new(None)),
            #[cfg(target_arch = "wasm32")]
            permission_event_source: Arc::new(Mutex::new(None)),
            awaiting_session_adoption: RwSignal::new(false),
            session_title: RwSignal::new(String::new()),
            session_list: RwSignal::new(Vec::new()),
            session_fetch_ticket: RwSignal::new(0),
            request_list: RwSignal::new(Vec::new()),
            permission_list: RwSignal::new(Vec::new()),
            permission_list_stale: RwSignal::new(None),
            history_results: RwSignal::new(Vec::new()),
            history_searching: RwSignal::new(false),
            history_error: RwSignal::new(None),
            history_search_generation: RwSignal::new(0),
            attention_fetching: RwSignal::new(false),
            request_cancellations: RwSignal::new(Vec::new()),
            last_turn_tokens: RwSignal::new(None),
            session_tokens: RwSignal::new(TokenStats::default()),
            model: RwSignal::new(None),
            models: RwSignal::new(Vec::new()),
            project: RwSignal::new(None),
            projects: RwSignal::new(Vec::new()),
            active_turn_id: RwSignal::new(None),
            browser_active: RwSignal::new(false),
            browser_last_action: RwSignal::new(None),
            permission_view: RwSignal::new(PermissionView::default()),
            permission_tombstones: RwSignal::new(Vec::new()),
            permission_epoch: RwSignal::new(0),
            thinking_level: RwSignal::new(None),
            model_override: RwSignal::new(None),
            agent_override: RwSignal::new(None),
            pending_images: RwSignal::new(Vec::new()),
            canvas_patches: RwSignal::new(Vec::new()),
            pinned_widgets: RwSignal::new(Vec::new()),
            active_decision_token: RwSignal::new(None),
            decision_authority: RwSignal::new(HashMap::new()),
            project_create_pending: RwSignal::new(false),
            project_create_error: RwSignal::new(None),
            preview_file_intent: RwSignal::new(None),
            sync_pending: RwSignal::new(false),
            sync_cycle: RwSignal::new(0),
            sync_interval: RwSignal::new(None),
            sync_fetch_abort: RwSignal::new(None),
            sync_in_flight: RwSignal::new(false),
            sync_deadline: RwSignal::new(None),
            activity_revision: RwSignal::new(0),
            sync_poll_kick: RwSignal::new(0),
        }
    }

    /// Zero-config boot. Fetch the same-origin proxy's /api/config to learn
    /// the daemon URL (and confirm auth is preconfigured server-side), set
    /// `url` from it, then open the SSE stream. If the proxy isn't reachable
    /// or doesn't answer, fall back to whatever `url` was constructed with.
    /// The user never types a URL or credential.
    pub fn bootstrap_then_connect(&self) {
        let daemon = self.clone();
        spawn_local(async move {
            // In a Chrome extension (side panel) there is no same-origin proxy:
            // the document is served from chrome-extension://, so a relative
            // `/api/config` resolves to the extension itself, not the daemon.
            // Detect that and talk to the daemon directly at its loopback URL,
            // skipping the proxy bootstrap entirely.
            let is_extension = running_as_extension();
            if is_extension {
                daemon.url.set(DEFAULT_DAEMON_URL.to_string());
                let identity = crate::rooms::RoomIdentity::direct_host();
                daemon.adopted_user_id.set(identity.id);
                daemon.adopted_display_name.set(identity.display_name);
                // No proxy fronts the side panel: voice goes daemon-direct to
                // `/v1/voice/*`, so readiness is host-neutral (offered) and any
                // missing-credential state surfaces per request. There is no
                // `/api/config` here to seed it from.
                daemon
                    .voice_ready
                    .set(crate::voice::transport::voice_ready_decision(None));
                // Restore persisted session before connecting fresh.
                if let Some(id) = should_restore_session(
                    load_persisted_session().as_deref(),
                    daemon.session_id.get_untracked().as_deref(),
                ) {
                    let intent = daemon.session_intent_generation.get_untracked();
                    if restore_session_or_clear(&daemon, id, intent).await
                        == RestoreOutcome::Restored
                    {
                        daemon.fetch_models();
                        daemon.fetch_projects();
                        return;
                    }
                }
                daemon.connect();
                daemon.fetch_models();
                daemon.fetch_projects();
                return;
            }
            // Voice readiness is host-neutral: `Some(has_auth)` when the proxy
            // answers `/api/config` (web/PWA), `None` when there's no proxy to
            // ask (Tauri, or proxy-less web) — in which case voice goes
            // daemon-direct and is offered, with credential state surfacing per
            // request. Seeded once after the config attempt so it never depends
            // on a proxy-only route.
            let mut proxy_has_auth: Option<bool> = None;
            match Request::get("/api/config").send().await {
                Ok(resp) => match resp.json::<ProxyConfig>().await {
                    Ok(cfg) => {
                        // Always honor the config's daemon_url, INCLUDING empty.
                        // Empty = "talk to the daemon through this same origin"
                        // (the proxy reverse-proxies /v1/agent/*). That's what
                        // makes the phone-via-tunnel case work: relative URLs,
                        // no localhost, no mixed content.
                        daemon.url.set(cfg.daemon_url.trim().to_string());
                        // Record voice readiness in its own signal so the SSE
                        // status transitions in connect() don't clobber it.
                        proxy_has_auth = Some(cfg.has_auth);
                        daemon.maps_key.set(cfg.maps_key.trim().to_string());
                        daemon.maps_map_id.set(cfg.maps_map_id.trim().to_string());
                        daemon
                            .livekit_room_id
                            .set(cfg.livekit_room_id.trim().to_string());
                        daemon
                            .livekit_token_path
                            .set(cfg.livekit_token_path.trim().to_string());
                        daemon
                            .tldraw_sync_uri
                            .set(cfg.tldraw_sync_uri.trim().to_string());
                        // The login IS the room identity. Without this the
                        // surface mints a per-browser `web-<random>` and a
                        // roster shows opaque client ids instead of people.
                        let identity = crate::rooms::RoomIdentity::from_proxy_config(
                            &cfg.user_id,
                            &cfg.user_display_name,
                        );
                        let adopted_id = identity.id;
                        let adopted_name = identity.display_name;
                        // Publish only after the authenticated config response.
                        // Until then browser Rooms remain unresolved and cannot
                        // act under identity state from a previous login.
                        daemon.adopted_user_id.set(adopted_id);
                        daemon.adopted_display_name.set(adopted_name);
                    }
                    Err(_) => {
                        // Non-JSON / unexpected shape — keep the fallback url.
                    }
                },
                Err(_) => {
                    // No proxy in front (e.g. trunk serve direct). Keep fallback.
                }
            }
            // Seed voice readiness from the host-neutral decision. Web with a
            // proxy honors its `has_auth`; Tauri and proxy-less web fall through
            // to `None` → offered (daemon-direct), credential state per request.
            daemon
                .voice_ready
                .set(crate::voice::transport::voice_ready_decision(
                    proxy_has_auth,
                ));
            // Native shell: no proxy fronts tauri://localhost, so the
            // `/api/config` fetch above lands on the SPA fallback and the
            // LiveKit signals stay empty, hiding the header "Join room call"
            // entry (QA-003). Seed the same default room the proxy would have
            // served; per-room joins still overwrite these via
            // `route_livekit_room_call`.
            if running_as_tauri() && daemon.livekit_token_path.get_untracked().trim().is_empty() {
                daemon
                    .livekit_room_id
                    .set(DEFAULT_LIVEKIT_ROOM_ID.to_string());
                daemon
                    .livekit_token_path
                    .set(crate::rooms::livekit_token_path_for_room(
                        DEFAULT_LIVEKIT_ROOM_ID,
                    ));
            }
            // Restore persisted session before connecting fresh.
            if let Some(id) = should_restore_session(
                load_persisted_session().as_deref(),
                daemon.session_id.get_untracked().as_deref(),
            ) {
                let intent = daemon.session_intent_generation.get_untracked();
                if restore_session_or_clear(&daemon, id, intent).await == RestoreOutcome::Restored {
                    daemon.fetch_models();
                    daemon.fetch_projects();
                    return;
                }
            }
            daemon.connect();
            // Re-fetch the model catalogue now that the daemon URL is resolved.
            // The eager fetch_models() at startup runs BEFORE bootstrap learns
            // the real origin, so remotely (phone via tunnel) it hits the wrong
            // URL and the picker ends up with an empty list (only the current
            // model, learned later from the turn stream). Fetching here, against
            // the now-correct origin, populates the full catalogue.
            daemon.fetch_models();
            // Same rule as fetch_models: only after the origin is resolved.
            daemon.fetch_projects();
        });
    }

    /// Open the SSE stream and pipe events into the turns signal. Reconnects
    /// on disconnect with a small backoff. Spawned once per session.
    pub fn connect(&self) {
        // Handle for the reconnect-path projection commit (TASK-44): a re-opened
        // agent stream reconciles the whole focused-session projection from the
        // daemon rather than blindly clearing live state.
        let daemon = self.clone();
        let url = self.url.get_untracked();
        let turns = self.turns;
        let streaming = self.streaming;
        let session_id = self.session_id;
        let status = self.status;
        let status_detail = self.status_detail;
        let sse_generation = self.sse_generation;
        let agent_stream_ready = self.agent_stream_ready;
        let planner_stream_sources = self.planner_stream_sources.clone();
        let last_turn_tokens = self.last_turn_tokens;
        let session_tokens = self.session_tokens;
        let model = self.model;
        let active_turn_id = self.active_turn_id;
        let browser_active = self.browser_active;
        let browser_last_action = self.browser_last_action;
        let canvas_patches = self.canvas_patches;
        let awaiting_session_adoption = self.awaiting_session_adoption;
        // Captured so the reconnect path can re-hydrate the transcript (and
        // restore title/cwd) after a stream gap — see the rehydrate call below.
        let session_title = self.session_title;
        let cwd = self.cwd;
        let sync_pending = self.sync_pending;
        let sync_poll_kick = self.sync_poll_kick;
        let activity_revision = self.activity_revision;
        let pinned_widgets = self.pinned_widgets;
        #[cfg(target_arch = "wasm32")]
        let agent_event_source = Arc::clone(&self.agent_event_source);

        let generation = sse_generation.get_untracked().wrapping_add(1);
        sse_generation.set(generation);
        self.agent_stream_ready.set(None);
        self.permission_stream_ready.set(None);
        #[cfg(target_arch = "wasm32")]
        {
            close_event_source(&self.agent_event_source);
            close_event_source(&self.permission_event_source);
        }
        let Some(active_session_id) = session_id.get_untracked() else {
            status.set("new session".into());
            return;
        };
        *planner_stream_sources.borrow_mut() = PlannerStreamSources {
            session_id: active_session_id.clone(),
            generation,
            agent: None,
            permission: None,
            lifecycle: PlannerStreamLifecycle::default(),
        };
        let seen_sse_ids: RwSignal<VecDeque<String>> = RwSignal::new(VecDeque::new());

        // Permission requests ride a SEPARATE stream. The product event stream
        // `/v1/agent/events` only carries `AgentTurnEvent` types and serializes
        // the inner event (no `permission_id`). The daemon emits
        // `OceanEvent::PermissionRequest` — with the envelope's `permission_id`
        // — onto the control stream `/v1/events`. Open that too, scoped to this
        // session/generation, so a gated mutating turn surfaces an approval card
        // instead of hanging. Decisions clear on `permission_decision` here.
        self.connect_permission_stream(active_session_id.clone(), generation);

        spawn_local(async move {
            // SSE is a live tail: the daemon does NOT replay frames emitted
            // while the client was disconnected. The FIRST open of this
            // generation is paired with a fresh transcript that the caller has
            // already hydrated (new/switch/create), so it needs no recovery.
            // Every RE-open after a drop, though, has a gap — any delta /
            // tool-call / turn_finished emitted during the outage is gone. So on
            // each reconnect we re-fetch the session snapshot (OCEAN-72's
            // hydration) to recover the missed events instead of leaving the UI
            // permanently stale/truncated (OCEAN-104).
            let mut connected_once = false;

            loop {
                if sse_generation.get_untracked() != generation {
                    break;
                }

                // On a reconnect (not the first open) the live tail had a gap:
                // any delta / tool-call / turn_finished emitted during the outage
                // is gone. Commit the daemon's authoritative projection to recover
                // them — transcript, pinned components, AND the live turn state
                // together. Unlike the old path this does NOT pre-clear
                // streaming/active_turn_id: a turn the daemon still reports as
                // running/waiting/cancelling stays live and cancellable (no idle
                // flicker, no lost Stop target), while a turn that terminated
                // during the gap is cleared by the terminal snapshot — the same
                // atomic commit either way (TASK-44, finding 3, and the 13:45
                // Tauri/web split it reproduces).
                if connected_once {
                    match daemon
                        .commit_session_projection(&active_session_id, generation)
                        .await
                    {
                        Ok(ProjectionOutcome::Terminal) => {
                            // Terminal — nothing to sync.
                            daemon.stop_sync_poll();
                        }
                        Ok(ProjectionOutcome::Live) => {
                            // Live turn — start the sync_pending poll.
                            daemon.start_sync_poll(active_session_id.clone(), generation);
                        }
                        Err(error) => {
                            log::warn!("reconnect projection commit skipped: {error}");
                        }
                    }
                    // A session switch can land while the reconnect snapshot is
                    // awaiting HTTP. Do not let that stale task install its old
                    // session EventSource into the shared slot and close the new
                    // generation's live stream.
                    if sse_generation.get_untracked() != generation {
                        break;
                    }
                }

                clear_stream_marker(agent_stream_ready, &active_session_id, generation);
                set_stream_source(
                    &planner_stream_sources,
                    &active_session_id,
                    generation,
                    false,
                    None,
                );
                // TASK-84: encode into the QUERY too. An `&` or `#` in an id
                // would otherwise split the query string and inject a
                // parameter — the same raw-interpolation class as the path
                // sites, with a different delimiter.
                let events_url = format!(
                    "{}/v1/agent/events?session_id={}",
                    url.trim_end_matches('/'),
                    encode_path_segment(active_session_id.as_str())
                );
                status.set(if connected_once {
                    "reconnecting…".into()
                } else {
                    "connecting…".to_string()
                });
                let mut es = match EventSource::new(&events_url) {
                    Ok(es) => es,
                    Err(err) => {
                        let raw = err.to_string();
                        log::error!("sse connect error: {raw}");
                        status.set(format!("sse connect error: {}", concise_error(&raw)));
                        gloo_timers::future::TimeoutFuture::new(2_000).await;
                        continue;
                    }
                };
                #[cfg(target_arch = "wasm32")]
                replace_event_source(&agent_event_source, es.clone());
                status.set("connected".into());
                status_detail.set(None);
                // Mark that we have opened the stream at least once for this
                // generation. From here on, any loop re-entry is a reconnect and
                // triggers the re-hydrate/stuck-streaming recovery at the top.
                connected_once = true;

                // EventSource delivers events by `event:` name. The daemon
                // names each frame by its AgentTurnEvent type, so we subscribe
                // per type and merge the streams. gloo-net has no
                // `subscribe_multiple`; we build the merged stream ourselves
                // with `futures::stream::select_all`. The name list is the
                // single source of truth `AGENT_EVENT_NAMES` (see its doc
                // comment — it MUST mirror the daemon's emitted names, and a
                // test guards against drift).
                let mut subs = Vec::with_capacity(AGENT_EVENT_NAMES.len());
                let mut sub_err = None;
                for name in AGENT_EVENT_NAMES {
                    match es.subscribe(*name) {
                        Ok(s) => subs.push(s),
                        Err(err) => {
                            sub_err = Some(format!("sse subscribe '{name}' error: {err}"));
                            break;
                        }
                    }
                }
                if let Some(err) = sub_err {
                    log::error!("{err}");
                    status.set(concise_error(&err));
                    gloo_timers::future::TimeoutFuture::new(2_000).await;
                    continue;
                }

                if !await_event_source_open(&es).await {
                    status.set("agent stream did not open".into());
                    continue;
                }
                agent_stream_ready.set(stream_marker_for_state(
                    es.state(),
                    &active_session_id,
                    generation,
                ));
                set_stream_source(
                    &planner_stream_sources,
                    &active_session_id,
                    generation,
                    false,
                    Some(es.clone()),
                );
                spawn_local(monitor_stream_open(
                    planner_stream_sources.clone(),
                    agent_stream_ready,
                    active_session_id.clone(),
                    generation,
                    false,
                ));
                let mut stream = futures_util::stream::select_all(subs);
                while let Some(msg) = stream.next().await {
                    if sse_generation.get_untracked() != generation {
                        break;
                    }

                    let Ok((_event_name, msg)) = msg else {
                        break;
                    };

                    // Tunnels/proxies can reconnect or replay a frame around
                    // connection churn. The daemon includes a stable SSE `id:`
                    // for each AgentTurnEvent, so apply each id only once per
                    // connection generation. Without this guard a replayed
                    // assistant_text_delta appends the same chunk again, which
                    // shows up as doubled words in the transcript.
                    let event_id = msg.last_event_id();
                    if !event_id.is_empty() && seen_recent_sse_id(seen_sse_ids, &event_id) {
                        continue;
                    }

                    let Some(data) = msg.data().as_string() else {
                        continue;
                    };
                    let Ok(evt) = serde_json::from_str::<AgentEvent>(&data) else {
                        log::warn!("unparseable sse event: {data}");
                        continue;
                    };
                    // NOTE: longhouse (council) frames are NOT captured here.
                    // The daemon does not deliver them to this surface's event
                    // stream; the council deck polls the durable
                    // `GET /v1/longhouse/topics` snapshot instead (OCEAN-58).

                    // Hard isolation: every renderable product event must carry
                    // exactly the active session id. If a proxy/global stream or
                    // older daemon sends an unscoped frame, drop it before any
                    // reducer code can mutate transcript, browser focus, tokens,
                    // components, or active turn state.
                    match evt.session_id() {
                        Some(evt_sid) if evt_sid == active_session_id.as_str() => {}
                        Some(evt_sid) => {
                            log::warn!(
                                "dropping sse event for session {evt_sid}; active session is {}",
                                active_session_id.as_str()
                            );
                            continue;
                        }
                        None => {
                            log::warn!("dropping unscoped sse event on session stream");
                            continue;
                        }
                    }

                    // If the visible surface has switched sessions since this
                    // connection was opened, this generation is stale. Stop it;
                    // the explicit session switch/create path will open a new
                    // scoped stream.
                    if session_id.get_untracked().as_deref() != Some(active_session_id.as_str()) {
                        break;
                    }

                    apply_event(
                        &evt,
                        turns,
                        session_id,
                        streaming,
                        status,
                        status_detail,
                        last_turn_tokens,
                        session_tokens,
                        model,
                        active_turn_id,
                        browser_active,
                        browser_last_action,
                        canvas_patches,
                        pinned_widgets,
                        awaiting_session_adoption,
                        sync_pending,
                        sync_poll_kick,
                        activity_revision,
                        session_title,
                        cwd,
                    );
                }

                clear_stream_marker(agent_stream_ready, &active_session_id, generation);
                set_stream_source(
                    &planner_stream_sources,
                    &active_session_id,
                    generation,
                    false,
                    None,
                );
                if sse_generation.get_untracked() != generation {
                    break;
                }

                // Brief backoff before re-opening. The status flips to
                // "reconnecting…" and the transcript re-hydrates at the top of
                // the next iteration (gated on `connected_once`).
                status.set("reconnecting…".into());
                gloo_timers::future::TimeoutFuture::new(1_000).await;
            }
        });
    }

    /// Subscribe to the daemon's control stream `/v1/events` for permission
    /// frames, scoped to `active_session_id` and the current SSE `generation`.
    ///
    /// `EventSource` delivers frames by their `event:` name, so we subscribe to
    /// `permission_request` and `permission_decision` by name (the same per-name
    /// pattern the agent stream uses). The control stream is NOT session-scoped
    /// server-side, so we drop any frame whose envelope `session_id` isn't ours.
    /// When gating is off the daemon never emits these, so this stream just sits
    /// idle — no behavior change for the ungated path.
    fn connect_permission_stream(&self, active_session_id: String, generation: u64) {
        let url = self.url.get_untracked();
        let sse_generation = self.sse_generation;
        let permission_stream_ready = self.permission_stream_ready;
        let permission_revision = self.permission_revision;
        let planner_stream_sources = self.planner_stream_sources.clone();
        let pending = self.permission_view;
        let tombstones = self.permission_tombstones;
        let decision_authority = self.decision_authority;
        // Handle for the post-reconnect reconciliation: a reopened control
        // stream has a gap, so a permission_request/decision emitted while it was
        // down is gone. Reconcile the durable /v1/permissions registry after each
        // reopen so a gate raised (or cleared) during the outage isn't missed
        // (TASK-44, finding 2).
        let daemon = self.clone();
        #[cfg(target_arch = "wasm32")]
        let permission_event_source = Arc::clone(&self.permission_event_source);

        spawn_local(async move {
            // The first open is paired with the switch/create commit that already
            // reconciled permissions; only a RE-open needs the gap recovery.
            let mut connected_once = false;
            loop {
                if sse_generation.get_untracked() != generation {
                    break;
                }
                clear_stream_marker(permission_stream_ready, &active_session_id, generation);
                set_stream_source(
                    &planner_stream_sources,
                    &active_session_id,
                    generation,
                    true,
                    None,
                );
                let events_url = format!("{}/v1/events", url.trim_end_matches('/'));
                let mut es = match EventSource::new(&events_url) {
                    Ok(es) => es,
                    Err(_) => {
                        gloo_timers::future::TimeoutFuture::new(2_000).await;
                        continue;
                    }
                };
                #[cfg(target_arch = "wasm32")]
                replace_event_source(&permission_event_source, es.clone());

                let mut subs = Vec::new();
                let mut sub_err = false;
                for name in ["permission_request", "permission_decision"] {
                    match es.subscribe(name) {
                        Ok(s) => subs.push(s),
                        Err(_) => {
                            sub_err = true;
                            break;
                        }
                    }
                }
                if sub_err {
                    gloo_timers::future::TimeoutFuture::new(2_000).await;
                    continue;
                }

                if !await_event_source_open(&es).await {
                    continue;
                }
                permission_stream_ready.set(stream_marker_for_state(
                    es.state(),
                    &active_session_id,
                    generation,
                ));
                set_stream_source(
                    &planner_stream_sources,
                    &active_session_id,
                    generation,
                    true,
                    Some(es.clone()),
                );
                spawn_local(monitor_stream_open(
                    planner_stream_sources.clone(),
                    permission_stream_ready,
                    active_session_id.clone(),
                    generation,
                    true,
                ));
                // Gap recovery on RE-open: reconcile the durable permission
                // registry so a gate raised or decided during the outage lands
                // (or clears) even though its live frame was never delivered
                // (TASK-44, finding 2). Skipped on the first open, which the
                // switch/create commit already reconciled.
                if connected_once && sse_generation.get_untracked() == generation {
                    if let Err(error) = daemon
                        .reconcile_pending_permissions(&active_session_id)
                        .await
                    {
                        log::warn!("permission reconcile after reconnect skipped: {error}");
                    }
                    if sse_generation.get_untracked() != generation {
                        clear_stream_marker(
                            permission_stream_ready,
                            &active_session_id,
                            generation,
                        );
                        set_stream_source(
                            &planner_stream_sources,
                            &active_session_id,
                            generation,
                            true,
                            None,
                        );
                        break;
                    }
                }
                connected_once = true;
                let mut stream = futures_util::stream::select_all(subs);
                while let Some(msg) = stream.next().await {
                    if sse_generation.get_untracked() != generation {
                        break;
                    }
                    let Ok((_event_name, msg)) = msg else {
                        break;
                    };
                    let Some(data) = msg.data().as_string() else {
                        continue;
                    };
                    let Ok(evt) = serde_json::from_str::<ControlEvent>(&data) else {
                        continue;
                    };
                    permission_revision.update(|revision| *revision = revision.wrapping_add(1));
                    let authority = decision_authority.get_untracked();
                    apply_control_event(&evt, &active_session_id, pending, tombstones, &authority);
                }

                clear_stream_marker(permission_stream_ready, &active_session_id, generation);
                set_stream_source(
                    &planner_stream_sources,
                    &active_session_id,
                    generation,
                    true,
                    None,
                );
                if sse_generation.get_untracked() != generation {
                    break;
                }
                gloo_timers::future::TimeoutFuture::new(1_000).await;
            }
        });
    }

    /// Record an allow/deny decision for a pending permission by POSTing
    /// `/v1/permissions/{id}/decision`. The body matches the daemon's
    /// `PermissionDecisionRequest`: `{ "permission_id": <id>, "decision":
    /// "allow" }` or `{ "permission_id": <id>, "decision": "deny" }` (the
    /// `decision` enum is `#[serde(tag = "decision")]`, flattened into the
    /// request). Includes `decision_token` (OCEAN-185 / OCEAN-314) — the same
    /// per-turn secret sent on the originating turn — so the daemon's gate can
    /// verify the decision comes from this surface and not a sniffed replay.
    /// On success the entry is removed; the daemon also broadcasts a
    /// `permission_decision` frame which removes it too (whichever lands first).
    pub fn decide_permission(&self, permission_id: String, allow: bool) {
        let url = self.url.get_untracked();
        let status = self.status;
        let pending = self.permission_view;
        let tombstones = self.permission_tombstones;
        let permission_revision = self.permission_revision;
        // Resolve decision authority for THIS card, not from a session-global
        // slot (TASK-44, finding 4). Look up the card's originating request id and
        // resolve the token bound to it. A card this surface never submitted —
        // remote-origin, or a request with no local grant — has no token and is
        // non-actionable: refuse the POST outright rather than send a
        // wrong/unbound token the daemon would 403 (or, worse, one bound to a
        // DIFFERENT turn).
        // Capture the token AND the exact card identity (session + request id):
        // every post-await completion must target THIS card instance, or write
        // nothing (codex HOLD on 9c584d0).
        let resolved = pending.with_untracked(|view| {
            view.cards()
                .iter()
                .find(|p| p.permission_id == permission_id)
                .and_then(|p| {
                    self.decision_authority.with_untracked(|authority| {
                        resolve_decision_authority(
                            p.request_id.as_deref(),
                            &p.session_id,
                            authority,
                        )
                        .map(|token| {
                            (
                                token.to_string(),
                                p.session_id.clone(),
                                p.request_id.clone(),
                            )
                        })
                    })
                })
        });
        let Some((token, card_session, card_request)) = resolved else {
            log::warn!(
                "refusing permission decision for {permission_id}: no local authority (remote-origin card)"
            );
            status.set("permission awaiting the surface that requested it".into());
            return;
        };

        // Mark the card as deciding so its buttons disable and it can't be
        // double-submitted.
        pending.update(|view| view.set_deciding(&permission_id, true));

        spawn_local(async move {
            let card_request = card_request.as_deref();
            let complete = |completion: DecisionCompletion| {
                apply_permission_decision_completion(
                    pending,
                    tombstones,
                    permission_revision,
                    status,
                    &permission_id,
                    &card_session,
                    card_request,
                    completion,
                );
            };
            let post_url = format!(
                "{}/v1/permissions/{permission_id}/decision",
                url.trim_end_matches('/')
            );
            // Replay the request-bound decision token so the daemon's OCEAN-185
            // gate can verify this decision came from the turn submitter.
            let decision = if allow { "allow" } else { "deny" };
            let body = json!({
                "permission_id": permission_id,
                "decision": decision,
                "decision_token": token,
            });
            let res = Request::post(&post_url)
                .header("content-type", "application/json")
                .json(&body);
            let res = match res {
                Ok(req) => req.send().await,
                Err(err) => {
                    let raw = err.to_string();
                    log::error!("permission encode error: {raw}");
                    complete(DecisionCompletion::Failed {
                        message: format!("permission encode error: {}", concise_error(&raw)),
                    });
                    return;
                }
            };
            match res {
                Ok(resp) if resp.ok() => {
                    complete(DecisionCompletion::Succeeded { allow });
                }
                Ok(resp) => {
                    let text = resp.text().await.unwrap_or_default();
                    log::error!("permission decision failed: {text}");
                    complete(DecisionCompletion::Failed {
                        message: format!("permission decision failed: {}", concise_error(&text)),
                    });
                }
                Err(err) => {
                    let raw = err.to_string();
                    log::error!("permission post error: {raw}");
                    complete(DecisionCompletion::Failed {
                        message: format!("permission post error: {}", concise_error(&raw)),
                    });
                }
            }
        });
    }

    pub fn send_prompt(&self, prompt: String) {
        self.send_prompt_with_display(prompt.clone(), prompt);
    }

    /// Send `prompt` to the model while echoing a concise `display_prompt` in the
    /// transcript. The composer uses this for explicitly attached text context:
    /// the wire prompt contains bounded file contents, while the local transcript
    /// shows the operator's instruction plus attachment names instead of dumping
    /// entire source files into the chat rail.
    pub fn send_prompt_with_display(&self, prompt: String, display_prompt: String) {
        if prompt.trim().is_empty() {
            return;
        }
        self.turns.update(|t| t.push(Turn::user(display_prompt)));
        self.streaming.set(true);
        self.dispatch_prompt(prompt, false, surface_client_type());
    }

    /// Send a voice-orb transcript. Identical to [`send_prompt`] except the turn
    /// is tagged `client_type="leo-voice"` so the daemon's voice system prompt
    /// (`voice_surface_prompt`, routed on `client_type == "leo-voice"`) applies —
    /// concise, speakable replies with no visual components. Without this the
    /// transcript would be tagged `surface-web`/`surface-extension` like a typed
    /// message and that voice guidance would be unreachable (OCEAN-181).
    pub fn send_voice_prompt(&self, prompt: String) {
        if prompt.trim().is_empty() {
            return;
        }
        self.turns.update(|t| t.push(Turn::user(prompt.clone())));
        self.streaming.set(true);
        self.dispatch_prompt(prompt, false, VOICE_CLIENT_TYPE);
    }

    /// Capture the visible browser tab and stage it for the next turn
    /// (OCEAN-138). Invokes the extension loader's
    /// `window.__ocean_capture_visible_tab()` — which returns a Promise of a
    /// `data:image/png;base64,...` URL via `chrome.tabs.captureVisibleTab` (a
    /// JS-only extension API the wasm app can't call directly) — then parses the
    /// data URL into a [`TurnImage`] and pushes it onto `pending_images`. On the
    /// next `send_prompt` the daemon emits it as a `Content::Image` block, so the
    /// agent can actually reason over the screenshot. No-op outside the
    /// extension. User-initiated only; never fires on a timer.
    pub fn capture_and_attach_visible_tab(&self) {
        if !running_as_extension() {
            return;
        }
        let pending_images = self.pending_images;
        let status = self.status;
        spawn_local(async move {
            let Some(window) = web_sys::window() else {
                return;
            };
            let Ok(func) = js_sys::Reflect::get(
                &window,
                &wasm_bindgen::JsValue::from_str("__ocean_capture_visible_tab"),
            ) else {
                return;
            };
            let Ok(func) = func.dyn_into::<js_sys::Function>() else {
                return;
            };
            let promise = match func.call0(&window) {
                Ok(v) => v,
                Err(_) => {
                    status.set("screenshot capture failed".into());
                    return;
                }
            };
            let Ok(promise) = promise.dyn_into::<js_sys::Promise>() else {
                return;
            };
            let data_url = match wasm_bindgen_futures::JsFuture::from(promise).await {
                Ok(v) => v.as_string(),
                Err(_) => {
                    status.set("screenshot capture failed".into());
                    return;
                }
            };
            let Some(data_url) = data_url else {
                status.set("screenshot capture returned no data".into());
                return;
            };
            match parse_data_url(&data_url) {
                Some(image) => {
                    pending_images.update(|imgs| imgs.push(image));
                    status.set("screenshot attached — it rides on your next message".into());
                }
                None => status.set("screenshot capture returned an unreadable image".into()),
            }
        });
    }

    /// Send a turn to the daemon. `is_retry` marks an auto-recovery resend (the
    /// user prompt was already echoed; don't echo again). If the daemon reports
    /// the supplied session is gone (strict resume), we clear the stale id and
    /// retry once as a fresh session — so a daemon restart is invisible to the
    /// user instead of dead-ending the turn.
    fn dispatch_prompt(&self, prompt: String, is_retry: bool, client_type: &'static str) {
        let url = self.url.get_untracked();
        let session_id = self.session_id.get_untracked();
        // Bind this submit to the current session-intent generation. If we take
        // the lazy-create path below, the created session is adopted only while
        // this captured value is still current — a switch or a second New Session
        // in flight bumps it and retires this create (TASK-43, finding 1).
        let launch_intent = self.session_intent_generation.get_untracked();
        self.awaiting_session_adoption.set(false);
        let project = self.project.get_untracked();
        // Always send the cwd the surface is displaying. Project sessions pin
        // this to the project's real workspace root in `begin_project_session`;
        // sending an empty cwd made the first turn fall back to the launch/dev
        // directory when the daemon could not resolve the project eagerly.
        let cwd = self.cwd.get_untracked();
        let streaming = self.streaming;
        let status = self.status;
        let status_detail = self.status_detail;
        // Per-turn overrides (OCEAN-79). Captured untracked so the async block
        // sends whatever was selected at dispatch time. Both default to `None`,
        // which omits the field and leaves the daemon's global defaults in force.
        let thinking_level = self.thinking_level.get_untracked();
        let model_override = self.model_override.get_untracked();
        // Named folder-as-agent for this turn (OCEAN-378). `None` until a future
        // composer selector sets it via `set_agent`; rides on the request below.
        let agent_override = self.agent_override.get_untracked();
        // Images staged for this turn (OCEAN-138). Read untracked at dispatch
        // time; an empty vec serializes as no `images` field. They are cleared
        // only after the turn POST succeeds (below), so a failed send keeps the
        // attachment around to retry rather than silently dropping it.
        let pending_images = self.pending_images.get_untracked();
        // Canvas ledger snapshot for this turn. Read untracked at dispatch time
        // — the same shape as the other per-turn signals above — and folded
        // through the identical merge-gated `MultiCanvasLedger` path the render
        // uses, so the agent sees exactly the converged state on screen.
        // `None` (the overwhelmingly common case: no canvas, or no placed
        // components) omits the field entirely, so non-canvas sessions keep a
        // byte-for-byte unchanged wire payload.
        let canvas_context = self
            .canvas_patches
            .with_untracked(|p| CanvasContext::from_entries(p));
        // Mint a fresh per-turn decision token (OCEAN-185 / OCEAN-314). Store
        // it in the signal so `decide_permission` can replay the same value.
        // Must be minted BEFORE the spawn so the same token covers both the
        // session-create + turn-submit path and the existing-session path.
        let decision_token = mint_decision_token();
        self.active_decision_token.set(Some(decision_token.clone()));
        let daemon = self.clone();

        spawn_local(async move {
            if session_id.is_none() {
                let title_hint = session_title_hint(&prompt);
                let body = AgentSessionCreateRequest {
                    title: title_hint.as_deref(),
                    workspace_root: &cwd,
                    project_id: project.as_deref(),
                    // A session's `client_type` is the stable surface MEDIUM
                    // (surface-web / surface-extension / surface-tauri), per the AGENTS.md session
                    // contract — never the per-turn routing tag. Even when the
                    // first interaction on a fresh surface is a voice transcript
                    // (so the threaded `client_type` is "leo-voice"), the session
                    // itself is still a web/extension surface; voice is a mode OF
                    // it, not its own surface. Only the AgentTurnRequest below
                    // carries the per-turn `client_type` (leo-voice for voice).
                    client_type: Some(surface_client_type()),
                };
                let create_url = format!("{}/v1/agent/sessions", url.trim_end_matches('/'));
                let res = Request::post(&create_url)
                    .header("content-type", "application/json")
                    .json(&body);
                let res = match res {
                    Ok(req) => req.send().await,
                    Err(err) => {
                        let raw = err.to_string();
                        log::error!("session encode error: {raw}");
                        status.set(format!("session encode error: {}", concise_error(&raw)));
                        streaming.set(false);
                        return;
                    }
                };

                match res {
                    Ok(resp) => match resp.json::<AgentSessionCreateResponse>().await {
                        Ok(r) if r.ok => {
                            let Some(new_session_id) = r.session_id else {
                                status.set("session create failed: missing session id".into());
                                streaming.set(false);
                                return;
                            };
                            // Phase 1 admission: the surface may have switched
                            // sessions or started a second New Session while this
                            // create POST was in flight. Adopt only when the
                            // captured intent is still current AND the active
                            // session is still empty. A rejected create may refresh
                            // the list so the new session is discoverable, but must
                            // not steal focus, dispatch this prompt, or clobber the
                            // status the newer intent now owns (TASK-43, finding 1).
                            if admit_create_intent(
                                launch_intent,
                                daemon.session_intent_generation.get_untracked(),
                                None,
                                daemon.session_id.get_untracked().as_deref(),
                            ) == CreateAdmission::Reject
                            {
                                daemon.fetch_sessions();
                                return;
                            }
                            persist_session(&new_session_id);
                            daemon.session_id.set(Some(new_session_id.clone()));
                            if let Some(title) = r.title.filter(|title| !title.trim().is_empty()) {
                                daemon.session_title.set(title);
                            }
                            if let Some(root) = r.workspace_root.or(r.cwd) {
                                if !root.is_empty() {
                                    daemon.cwd.set(root);
                                }
                            }
                            status.set("session ready".into());
                            // Connect BOTH live tails (agent + permission) for the
                            // freshly-installed session, then withhold the first
                            // turn until both EventSources are OPEN for this
                            // session+generation. The endpoint is a live tail with
                            // no replay, so dispatching before the surface is
                            // listening would lose `turn_started` and early frames
                            // (TASK-43, finding 5). `connect` bumps `sse_generation`
                            // and opens the permission stream internally.
                            daemon.connect();
                            daemon.fetch_sessions();
                            let generation = daemon.sse_generation.get_untracked();
                            match daemon
                                .await_streams_ready(&new_session_id, generation)
                                .await
                            {
                                Ok(()) => {
                                    // Phase 2 admission: a switch or New Session may
                                    // have settled during the handshake. Dispatch the
                                    // first turn only while this create still owns the
                                    // cockpit (intent unchanged, focus still ours).
                                    if admit_create_intent(
                                        launch_intent,
                                        daemon.session_intent_generation.get_untracked(),
                                        Some(&new_session_id),
                                        daemon.session_id.get_untracked().as_deref(),
                                    ) == CreateAdmission::Adopt
                                    {
                                        daemon.dispatch_prompt(prompt, is_retry, client_type);
                                    }
                                }
                                Err(error) => {
                                    // Timeout or focus move before both streams
                                    // opened. Never dispatch into an unobserved
                                    // stream. Surface the failure only while this
                                    // session is still focused, so a newer intent's
                                    // status is not clobbered (TASK-43, findings 1 + 5).
                                    if admit_create_intent(
                                        launch_intent,
                                        daemon.session_intent_generation.get_untracked(),
                                        Some(&new_session_id),
                                        daemon.session_id.get_untracked().as_deref(),
                                    ) == CreateAdmission::Adopt
                                    {
                                        log::error!("session stream not ready: {error}");
                                        status.set(format!(
                                            "session stream not ready: {}",
                                            concise_error(&error)
                                        ));
                                        streaming.set(false);
                                    }
                                }
                            }
                        }
                        Ok(r) => {
                            let raw = r.error.unwrap_or_else(|| "unknown error".into());
                            log::error!("session create failed: {raw}");
                            status.set(format!("session create failed: {}", concise_error(&raw)));
                            streaming.set(false);
                        }
                        Err(err) => {
                            let raw = err.to_string();
                            log::error!("session decode error: {raw}");
                            status.set(format!("session decode error: {}", concise_error(&raw)));
                            streaming.set(false);
                        }
                    },
                    Err(err) => {
                        let raw = err.to_string();
                        log::error!("session post error: {raw}");
                        status.set(format!("session post error: {}", concise_error(&raw)));
                        streaming.set(false);
                    }
                }
                return;
            }

            // TASK-76: the freeform browser-context GUIDANCE path is gone.
            //
            // Tab titles are page-controlled: any site the operator has open
            // authors `document.title`. That text was interpolated raw (no
            // escaping, no cap, no delimiter) and shipped as `guidance`, which
            // the daemon HONORS (`apply_turn_guidance`) and renders under
            // "Operator guidance for this turn:" — so a website's text reached
            // a tool-executing agent wearing OPERATOR authority. Zero-click:
            // the operator only had to have the tab open and type anything.
            //
            // The structured `client_context.browser` path below carries the
            // same snapshot and is sanitized daemon-side
            // (`sanitize_browser_field`: length cap, control chars collapsed,
            // markdown neutralized), so dropping the prose path loses no
            // capability — it removes a second, unsanitized channel that
            // existed only for historical reasons.
            //
            // Structured counterpart (OCEAN-377):
            // the daemon now consumes `client_context.browser` directly, so we
            // ship the same active-tab/open-tab snapshot in the structured shape.
            // `None` off the extension, leaving the wire shape unchanged there.
            //
            // Key it on the SURFACE identity (`surface-extension`/`surface-tauri`/`surface-web`),
            // NOT the flat turn `client_type`: a voice turn from the side panel
            // threads `VOICE_CLIENT_TYPE` ("leo-voice", a *routing* tag) as the
            // flat type, but the daemon's `apply_browser_context()` only treats
            // the context as a browser surface when its `client_type` is a
            // surface identity. Tagging it "leo-voice" would make the daemon
            // ignore the snapshot. Flat type stays the routing tag (unchanged);
            // the context carries the surface identity (OCEAN-377 P2).
            let client_context = browser_client_context(surface_client_type());
            let body = AgentTurnRequest {
                prompt: &prompt,
                cwd: &cwd,
                session_id: session_id.as_deref(),
                project_id: project.as_deref(),
                client_type: Some(client_type),
                // TASK-86: routed through `turn_guidance()` so the invariant
                // is pinned by a real unit test rather than by a string match
                // against this file (the previous assertion matched its own
                // source and could not fail).
                guidance: turn_guidance(),
                // Deliberately `None`: the daemon's turn `room_id` is the
                // closed Track-0 set (pm/writers/orch_mesh/review), NOT the
                // surface's persistent-room keys — those rooms post through
                // /v1/rooms/persistent/{key}/messages and never reach this
                // path (room mode unmounts the composer and voice orb).
                room_id: None,
                // Per-turn overrides selected in the composer (OCEAN-79). Both
                // are `None` until the user touches a control, preserving the
                // daemon-default behavior. `thinking_level` is already the
                // lowercase `ThinkingLevel` string the daemon deserializes.
                thinking_level: thinking_level.as_deref(),
                model_id: model_override.as_deref(),
                // Captured/picked images ride along on the FIRST user message of
                // the turn (OCEAN-138). Omitted when nothing was staged, so the
                // daemon's `images: Option<Vec<TurnImage>>` stays `None`.
                images: if pending_images.is_empty() {
                    None
                } else {
                    Some(pending_images.clone())
                },
                // Per-turn gate secret (OCEAN-185 / OCEAN-314). Minted once
                // per `dispatch_prompt` call and stored in the daemon signal
                // so `decide_permission` replays the same token.
                decision_token: Some(&decision_token),
                client_context,
                // Folder-as-agent override (OCEAN-378). `None` today (no selector
                // ships yet), so the daemon keeps the surface-profile system
                // prompt and the wire shape is unchanged; a value flows straight
                // through to `<agents_root>/<agent>/instructions.md` once set.
                agent: agent_override.as_deref(),
                // Live canvas state as surface context. Only present when the
                // canvas actually holds placed components (see the field doc
                // and `CanvasContext::from_entries`).
                canvas: canvas_context,
            };
            let post_url = format!("{}/v1/agent/turns", url.trim_end_matches('/'));
            let res = Request::post(&post_url)
                .header("content-type", "application/json")
                .json(&body);
            let res = match res {
                Ok(req) => req.send().await,
                Err(err) => {
                    let raw = err.to_string();
                    log::error!("encode error: {raw}");
                    status.set(format!("encode error: {}", concise_error(&raw)));
                    streaming.set(false);
                    daemon.awaiting_session_adoption.set(false);
                    return;
                }
            };
            match res {
                Ok(resp) => match resp.json::<AgentTurnResponse>().await {
                    Ok(r) if r.ok => {
                        // Bind decision authority for this turn (TASK-44, finding
                        // 4). The daemon's turn id IS the request id, so a later
                        // permission gate on this turn resolves to exactly the
                        // token minted here — never a subsequent turn's token. The
                        // grant is tagged with the turn's session so a projection
                        // commit prunes it per-session once the turn terminates.
                        // Bound before any gate can fire (the POST returns on turn
                        // acceptance, before the model emits a tool call).
                        daemon.decision_authority.update(|authority| {
                            authority.insert(
                                r.turn_id.clone(),
                                DecisionGrant {
                                    token: decision_token.clone(),
                                    session_id: r.session_id.clone(),
                                },
                            );
                        });
                        // Do not let a late HTTP response from an older submit
                        // switch the visible cockpit back to another session.
                        // Active session changes only via explicit create/select
                        // paths, not passive turn responses or SSE.
                        if daemon.session_id.get_untracked().as_deref() == session_id.as_deref() {
                            persist_session(&r.session_id);
                            daemon.session_id.set(Some(r.session_id));
                        }
                        // The staged images were accepted with this turn — drain
                        // them so they don't ride along on the next one
                        // (OCEAN-138). On any failure path we leave them in place
                        // so the user can retry without re-capturing.
                        if !pending_images.is_empty() {
                            daemon.pending_images.set(Vec::new());
                        }
                        // Reply text streams over the scoped SSE connection;
                        // streaming flips off on turn_finished.
                    }
                    Ok(r) => {
                        let err = r.error.unwrap_or_else(|| "unknown error".into());
                        // Strict-resume recovery: our session id is stale (e.g.
                        // the daemon restarted). Drop it and retry once fresh.
                        if !is_retry && session_id.is_some() && err.contains("session not found") {
                            daemon.session_id.set(None);
                            clear_persisted_session();
                            daemon.reset_token_stats();
                            // TASK-69 (codex HOLD on af82ee8): this is a
                            // session-IDENTITY transition too — it retires the
                            // expired session and adopts a fresh one below. Route
                            // through the same reset/invalidation helper as
                            // select/new so old-session cards + their request-bound
                            // authority never carry into the replacement session,
                            // and any in-flight publisher is superseded. Does not
                            // touch `turns`, so the prompt is not re-echoed.
                            daemon.retire_permission_state();
                            status.set("session expired — starting fresh".into());
                            daemon.dispatch_prompt(prompt, true, client_type);
                            return;
                        }
                        surface_turn_failure(
                            daemon.turns,
                            status,
                            status_detail,
                            &r.turn_id,
                            "turn failed",
                            &err,
                        );
                        streaming.set(false);
                        daemon.awaiting_session_adoption.set(false);
                    }
                    Err(err) => {
                        let raw = err.to_string();
                        let turn_id = daemon.active_turn_id.get_untracked().unwrap_or_else(|| {
                            format!("dispatch-error-{}", daemon.turns.get_untracked().len())
                        });
                        surface_turn_failure(
                            daemon.turns,
                            status,
                            status_detail,
                            &turn_id,
                            "decode error",
                            &raw,
                        );
                        streaming.set(false);
                        daemon.awaiting_session_adoption.set(false);
                    }
                },
                Err(err) => {
                    let raw = err.to_string();
                    let turn_id = daemon.active_turn_id.get_untracked().unwrap_or_else(|| {
                        format!("dispatch-error-{}", daemon.turns.get_untracked().len())
                    });
                    surface_turn_failure(
                        daemon.turns,
                        status,
                        status_detail,
                        &turn_id,
                        "post error",
                        &raw,
                    );
                    streaming.set(false);
                    daemon.awaiting_session_adoption.set(false);
                }
            }
        });
    }

    /// Claim the global session-list request ticket before starting a complete
    /// paginated drain. The thin spawner and A1 panel share this authority.
    pub(crate) fn claim_session_fetch_ticket(&self) -> u64 {
        let mut claimed = 0;
        self.session_fetch_ticket.update(|current| {
            *current = next_session_fetch_ticket(*current);
            claimed = *current;
        });
        claimed
    }

    /// Replace `session_list` only when `claimed` is still the newest global
    /// request. This check and write are synchronous on the WASM event loop.
    pub(crate) fn store_sessions_if_current(
        &self,
        claimed: u64,
        sessions: Vec<SessionSummary>,
    ) -> bool {
        if !session_fetch_ticket_is_current(self.session_fetch_ticket.get_untracked(), claimed) {
            return false;
        }
        self.session_list.set(sessions);
        true
    }

    /// Fetch the complete session list through the thin refresh spawner. It
    /// claims the same global write ticket as the sessions-panel A1 poll rail.
    pub fn fetch_sessions(&self) {
        let claimed = self.claim_session_fetch_ticket();
        let daemon = self.clone();
        spawn_local(async move {
            match daemon.fetch_all_sessions().await {
                Ok(all) => {
                    let _ = daemon.store_sessions_if_current(claimed, all);
                }
                Err(e) => log::warn!("sessions fetch error: {e}"),
            }
        });
    }

    /// Async pagination rail: drain all pages from the daemon and return the
    /// full session list. Owned by `async move` so the panel's 2s interval
    /// callback can clone the daemon and await this directly.
    ///
    /// Returns `Err(msg)` on network/parse failure so callers can decide
    /// whether to retry or suppress a stale write.
    ///
    /// The daemon paginates: each page caps at DEFAULT_LIST_LIMIT (100) and
    /// returns `next_cursor` while `has_more` (OCEAN-250). The panel groups
    /// by project and sessions come back sorted by workspace root, so a
    /// project's sessions can sit ANYWHERE in the list, well past the first
    /// 100 — fetching one page left every deeper-nested project invisible
    /// once the store grew past ~100 sessions. Bounded at 50 pages so a
    /// stuck cursor can never spin forever.
    pub async fn fetch_all_sessions(&self) -> Result<Vec<SessionSummary>, String> {
        let base = self.url.get_untracked();
        #[derive(Deserialize)]
        struct SessionsResponse {
            ok: bool,
            #[serde(default)]
            sessions: Vec<SessionSummary>,
            #[serde(default)]
            next_cursor: Option<String>,
            #[serde(default)]
            has_more: bool,
        }
        let mut all: Vec<SessionSummary> = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..50 {
            let get_url = sessions_page_url(&base, cursor.as_deref());
            let resp = Request::get(&get_url)
                .send()
                .await
                .map_err(|e| format!("fetch: {e}"))?;
            // The proxy answers a typed 503 when the machine this session is
            // attached to cannot serve it. Decoding that body as a session page
            // yields "expected value" and hides which machine is down — read it
            // as the sentence it is (`crate::devices::device_unavailable_error`).
            if resp.status() == 503 {
                return Err(crate::devices::device_unavailable_error(resp).await);
            }
            let r: SessionsResponse = resp.json().await.map_err(|e| format!("decode: {e}"))?;
            if !r.ok {
                return Err("sessions fetch not ok".into());
            }
            all.extend(r.sessions);
            match r.next_cursor.filter(|_| r.has_more) {
                Some(next) if !next.is_empty() => cursor = Some(next),
                _ => break,
            }
        }
        Ok(all)
    }

    /// Refresh the daemon's authoritative cross-session work snapshots.
    ///
    /// This intentionally polls the existing read-only request and permission
    /// registries instead of opening an SSE pair per background session. Each
    /// signal keeps its last good value when its endpoint or decoder fails, so
    /// a transient poll cannot erase real attention from the Island.
    pub fn fetch_attention(&self) {
        if self.attention_fetching.get_untracked() {
            return;
        }
        self.attention_fetching.set(true);
        let base = self.url.get_untracked().trim_end_matches('/').to_string();
        let request_list = self.request_list;
        let permission_list = self.permission_list;
        let permission_list_stale = self.permission_list_stale;
        let attention_fetching = self.attention_fetching;
        let request_cancellations = self.request_cancellations;
        spawn_local(async move {
            #[derive(Deserialize)]
            struct RequestsResponse {
                ok: bool,
                #[serde(default)]
                requests: Vec<RequestSnapshot>,
            }
            #[derive(Deserialize)]
            struct PermissionsResponse {
                ok: bool,
                #[serde(default)]
                permissions: Vec<PermissionSnapshot>,
            }

            let (requests_response, permissions_response) = futures_util::join!(
                Request::get(&format!("{base}/v1/requests")).send(),
                Request::get(&format!("{base}/v1/permissions")).send(),
            );

            match requests_response {
                Ok(resp) => match resp.json::<RequestsResponse>().await {
                    Ok(snapshot) if snapshot.ok => {
                        request_cancellations.update(|pending| {
                            pending.retain(|request_id| {
                                snapshot.requests.iter().any(|request| {
                                    request.request_id == *request_id
                                        && matches!(
                                            request.state,
                                            RequestSnapshotState::Queued
                                                | RequestSnapshotState::Running
                                        )
                                })
                            })
                        });
                        request_list.set(snapshot.requests);
                    }
                    Ok(_) => log::warn!("attention requests fetch not ok"),
                    Err(err) => log::warn!("attention requests decode error: {err}"),
                },
                Err(err) => log::warn!("attention requests fetch error: {err}"),
            }

            // TASK-69 seam 2: a failed poll no longer silently leaves the stale
            // list as authoritative. Classify the outcome; on error keep the
            // last-known list but publish a stale marker the Island surfaces.
            let permission_result: Result<Vec<PermissionSnapshot>, String> =
                match permissions_response {
                    Ok(resp) => match resp.json::<PermissionsResponse>().await {
                        Ok(snapshot) if snapshot.ok => Ok(snapshot.permissions),
                        Ok(_) => Err("attention permissions fetch not ok".into()),
                        Err(err) => Err(format!("attention permissions decode error: {err}")),
                    },
                    Err(err) => Err(format!("attention permissions fetch error: {err}")),
                };
            match classify_permission_poll(permission_result) {
                PermissionPollOutcome::Fresh(permissions) => {
                    permission_list.set(permissions);
                    permission_list_stale.set(None);
                }
                PermissionPollOutcome::Unavailable(reason) => {
                    log::warn!("{reason}");
                    permission_list_stale.set(Some(reason));
                }
            }
            attention_fetching.set(false);
        });
    }

    /// Invalidate any in-flight/debounced Recall query immediately. Input
    /// handlers call this before their debounce so stale query-A results cannot
    /// become actionable while query B is already visible in the field.
    pub fn invalidate_history_search(&self) {
        self.history_search_generation
            .update(|generation| *generation = generation.wrapping_add(1));
        self.history_results.set(Vec::new());
        self.history_error.set(None);
        self.history_searching.set(false);
    }

    /// Search persisted user/assistant transcript text through the daemon-owned
    /// Recall endpoint. Empty queries clear the view locally. Generation checks
    /// ensure an older, slower request can never replace newer results.
    pub fn search_history(&self, query: String) {
        let query = query.trim().to_string();
        let generation = self
            .history_search_generation
            .get_untracked()
            .wrapping_add(1);
        self.history_search_generation.set(generation);
        if query.is_empty() {
            self.history_results.set(Vec::new());
            self.history_error.set(None);
            self.history_searching.set(false);
            return;
        }

        self.history_searching.set(true);
        self.history_error.set(None);
        self.history_results.set(Vec::new());
        let base = self.url.get_untracked().trim_end_matches('/').to_string();
        let encoded = js_sys::encode_uri_component(&query)
            .as_string()
            .unwrap_or_else(|| query.clone());
        let url = format!("{base}/v1/agent/history/search?q={encoded}&limit=20");
        let active_generation = self.history_search_generation;
        let history_results = self.history_results;
        let history_searching = self.history_searching;
        let history_error = self.history_error;
        spawn_local(async move {
            let result = match Request::get(&url).send().await {
                Ok(response) => match response.json::<HistorySearchResponse>().await {
                    Ok(payload) if payload.ok => Ok(payload.hits),
                    Ok(payload) => Err(payload
                        .error
                        .filter(|error| !error.trim().is_empty())
                        .unwrap_or_else(|| "History search failed".to_string())),
                    Err(error) => Err(format!("History response was unreadable: {error}")),
                },
                Err(error) => Err(format!("History search is unavailable: {error}")),
            };
            if active_generation.get_untracked() != generation {
                return;
            }
            match result {
                Ok(hits) => {
                    history_results.set(hits);
                    history_error.set(None);
                }
                Err(error) => {
                    history_results.set(Vec::new());
                    history_error.set(Some(error));
                }
            }
            history_searching.set(false);
        });
    }

    /// Fetch the model catalogue + current selection from the daemon.
    pub fn fetch_models(&self) {
        let url = self.url.get_untracked();
        let models = self.models;
        let model = self.model;
        // The catalogue belongs to one machine; a reply that lands after a
        // switch describes the machine we left.
        let epoch = self.device_epoch;
        let asked_at = epoch.get_untracked();
        spawn_local(async move {
            #[derive(Deserialize)]
            struct Current {
                #[serde(default)]
                model: String,
            }
            #[derive(Deserialize)]
            struct ModelsResponse {
                #[serde(default)]
                models: Vec<ModelInfo>,
                #[serde(default)]
                current: Option<Current>,
            }
            let get_url = format!("{}/v1/models", url.trim_end_matches('/'));
            match Request::get(&get_url).send().await {
                Ok(resp) => match resp.json::<ModelsResponse>().await {
                    Ok(r) => {
                        if epoch.get_untracked() != asked_at {
                            log::debug!("model catalogue from a machine we left; dropped");
                            return;
                        }
                        if let Some(cur) = r.current {
                            if !cur.model.is_empty() {
                                model.set(Some(cur.model));
                            }
                        }
                        models.set(r.models);
                    }
                    Err(err) => log::warn!("models decode error: {err}"),
                },
                Err(err) => log::warn!("models fetch error: {err}"),
            }
        });
    }

    /// Fetch the project catalogue from the daemon. Like [`fetch_models`], call
    /// this only AFTER the daemon URL is resolved (see `bootstrap_then_connect`)
    /// — an eager pre-bootstrap fetch hits the wrong origin and silently fails.
    pub fn fetch_projects(&self) {
        let url = self.url.get_untracked();
        let projects = self.projects;
        let current = self.project;
        // Same rule as the model catalogue, and it matters more here: this
        // fetch also CLEARS a selection its list does not contain, so a late
        // reply from the old machine would drop a project the new one owns.
        let epoch = self.device_epoch;
        let asked_at = epoch.get_untracked();
        spawn_local(async move {
            #[derive(Deserialize)]
            struct ProjectsResponse {
                #[serde(default)]
                projects: Vec<ProjectInfo>,
            }
            let get_url = format!("{}/v1/projects", url.trim_end_matches('/'));
            match Request::get(&get_url).send().await {
                Ok(resp) => match resp.json::<ProjectsResponse>().await {
                    Ok(r) => {
                        if epoch.get_untracked() != asked_at {
                            log::debug!("project catalogue from a machine we left; dropped");
                            return;
                        }
                        // Drop a persisted selection that no longer exists.
                        if let Some(sel) = current.get_untracked() {
                            if !r.projects.iter().any(|p| p.id == sel) {
                                current.set(None);
                                clear_persisted_project();
                            }
                        }
                        projects.set(r.projects);
                    }
                    Err(err) => log::warn!("projects decode error: {err}"),
                },
                Err(err) => log::warn!("projects fetch error: {err}"),
            }
        });
    }

    /// Select the active project. Unlike the model, this is purely client-side:
    /// the choice rides on every turn's `project_id`. Persist it so it survives
    /// reload. Pass `None` to clear.
    pub fn set_project(&self, id: Option<String>) {
        self.project.set(id.clone());
        match id {
            Some(id) => persist_project(&id),
            None => clear_persisted_project(),
        }
    }

    /// Set the per-turn reasoning-effort override (OCEAN-79). `level` is the
    /// serialized `ThinkingLevel` string (`off` / `low` / `medium` / `high`);
    /// pass `None` to clear and fall back to the daemon's global default. Purely
    /// client-side — the value rides on the next turn's `thinking_level`.
    /// Persisted so the choice survives reload.
    pub fn set_thinking_level(&self, level: Option<String>) {
        self.thinking_level.set(level.clone());
        match level {
            Some(l) => persist_thinking_level(&l),
            None => clear_persisted_thinking_level(),
        }
    }

    /// Set the per-turn model override (OCEAN-79). Unlike [`set_model`] (a
    /// global `POST /v1/model` swap), this only sets the next turn's `model_id`
    /// and never mutates the daemon's global model. Pass `None` to clear and use
    /// the daemon default. Persisted so the choice survives reload.
    pub fn set_model_override(&self, id: Option<String>) {
        self.model_override.set(id.clone());
        match id {
            Some(id) => persist_model_override(&id),
            None => clear_persisted_model_override(),
        }
    }

    /// Select the per-turn folder-as-agent (OCEAN-378). Purely client-side: the
    /// name rides on the next turn's `AgentTurnRequest::agent`, so the daemon
    /// composes `<agents_root>/<agent>/instructions.md` as that turn's system
    /// prompt without touching any global state. Pass `None` to clear and fall
    /// back to the surface profile. Not persisted — a named agent is a deliberate
    /// per-turn choice, not a sticky preference. No composer control calls this
    /// yet (UI exposure via `GET /v1/agents` is out of scope for OCEAN-378); the
    /// setter exists so a future selector can wire straight into the turn path.
    #[allow(dead_code)]
    pub fn set_agent(&self, agent: Option<String>) {
        self.agent_override.set(agent);
    }

    /// Hot-swap the daemon's model. Optimistically updates the local `model`
    /// signal, POSTs the change, then re-reads to confirm. Retained as a daemon
    /// capability (global mid-session model swap via /v1/model) even though the
    /// header picker that called it was removed in OCEAN-202.
    #[allow(dead_code)]
    pub fn set_model(&self, id: String) {
        let url = self.url.get_untracked();
        let model = self.model;
        let status = self.status;
        let daemon = self.clone();
        model.set(Some(id.clone()));
        spawn_local(async move {
            let post_url = format!("{}/v1/model", url.trim_end_matches('/'));
            let body = serde_json::json!({ "model": id });
            match Request::post(&post_url)
                .header("content-type", "application/json")
                .json(&body)
            {
                Ok(req) => match req.send().await {
                    Ok(_) => {
                        // Confirm the authoritative selection.
                        daemon.fetch_models();
                    }
                    Err(err) => {
                        let raw = err.to_string();
                        log::error!("model swap error: {raw}");
                        status.set(format!("model swap error: {}", concise_error(&raw)));
                    }
                },
                Err(err) => {
                    let raw = err.to_string();
                    log::error!("model encode error: {raw}");
                    status.set(format!("model encode error: {}", concise_error(&raw)));
                }
            }
        });
    }

    /// Halt the focused in-flight turn, if any. The composer and Island share
    /// the same explicit request-cancel path so their status/error behavior
    /// cannot drift.
    pub fn halt(&self) {
        let Some(turn_id) = self.active_turn_id.get_untracked() else {
            return;
        };
        self.cancel_request(turn_id);
    }

    /// Request cancellation for any daemon-owned running request. This is the
    /// mutation seam used by Phase 3B Island cards; the daemon remains the
    /// lifecycle authority, while `request_cancellations` tracks only the POST
    /// in flight so the action cannot be double-submitted.
    pub fn cancel_request(&self, request_id: String) {
        if self
            .request_cancellations
            .with_untracked(|requests| requests.iter().any(|id| id == &request_id))
        {
            return;
        }
        self.request_cancellations
            .update(|requests| requests.push(request_id.clone()));

        let url = self.url.get_untracked();
        let status = self.status;
        let status_detail = self.status_detail;
        let streaming = self.streaming;
        let active_turn_id = self.active_turn_id;
        let request_cancellations = self.request_cancellations;
        let daemon = self.clone();
        spawn_local(async move {
            let post_url = format!(
                "{}/v1/requests/{request_id}/cancel",
                url.trim_end_matches('/')
            );
            let mut accepted = false;
            match Request::post(&post_url).send().await {
                Ok(response) => {
                    let http_ok = response.ok();
                    let raw = response.text().await.unwrap_or_default();
                    match request_cancel_result(http_ok, &raw) {
                        Ok(()) => {
                            accepted = true;
                            if active_turn_id.get_untracked().as_deref()
                                == Some(request_id.as_str())
                            {
                                status.set("halting…".into());
                                status_detail.set(None);
                                // The terminal SSE frame remains authoritative;
                                // release the focused composer's Stop state once
                                // the daemon accepted cancellation.
                                streaming.set(false);
                            }
                            daemon.fetch_attention();
                        }
                        Err(message) => {
                            let concise = format!("stop failed: {}", concise_error(&message));
                            log::error!("request cancellation failed: {raw}");
                            status.set(concise.clone());
                            status_detail.set(Some((concise, raw)));
                        }
                    }
                }
                Err(err) => {
                    let raw = err.to_string();
                    let concise = format!("stop failed: {}", concise_error(&raw));
                    log::error!("request cancellation error: {raw}");
                    status.set(concise.clone());
                    status_detail.set(Some((concise, raw)));
                }
            }
            if !accepted {
                request_cancellations.update(|requests| requests.retain(|id| id != &request_id));
            }
        });
    }

    // ── TASK-46: sync_pending helpers ──────────────────────────────────────────
    //
    // On first attach/reconnect where detail state is Running/WaitingForPermission/
    // Cancelling: committed snapshot renders immediately, partial active-turn tail
    // is suppressed, bounded abortable detail poll runs until terminal state.

    /// Stop the sync_pending poll: abort in-flight fetch, clear interval, bump
    /// sync_cycle, clear in_flight, clear deadline, clear sync_pending flag.
    /// Idempotent — safe to call when no poll is active.
    fn stop_sync_poll(&self) {
        if let Some(h) = self.sync_fetch_abort.get_untracked() {
            h.abort();
            self.sync_fetch_abort.set(None);
        }
        if let Some(handle) = self.sync_interval.get_untracked() {
            handle.clear();
            self.sync_interval.set(None);
        }
        self.sync_cycle.update(|g| *g = g.wrapping_add(1));
        self.sync_in_flight.set(false);
        self.sync_deadline.set(None);
        self.sync_pending.set(false);
    }

    /// Enter sync_pending mode for `(session_id, sse_generation)`. Commit the
    /// committed transcript immediately (already done by the caller), set the
    /// sync_pending flag, and start the bounded abortable detail poll at 2s
    /// intervals until the turn becomes terminal or the 5-minute deadline is
    /// reached (→ stalled with explicit refresh affordance).
    ///
    /// Must be called ONLY when the caller already committed the quarantined
    /// projection and the sync_pending flag is false (not already polling).
    fn start_sync_poll(&self, session_id: String, generation: u64) {
        // Stop any prior poll first (idempotent — no-op if none active).
        self.stop_sync_poll();

        let sync_cycle = self.sync_cycle.get_untracked();
        self.sync_pending.set(true);

        // 5-minute absolute deadline in js_sys milliseconds.
        let deadline = now_millis() + 300_000.0;
        self.sync_deadline.set(Some(deadline));

        // Kick counter effect: TurnFinished bumps sync_poll_kick; when it
        // changes, we fire an immediate poll tick.
        let kick = self.sync_poll_kick;
        let sync_pending = self.sync_pending;
        let d_kick = self.clone();
        let s_kick = session_id.clone();
        Effect::new(move |_| {
            // Read the kick counter — the effect re-runs on change.
            let _current = kick.get();
            // Only fire when sync_pending is still active; the effect outlives
            // the interval and could fire after stop_sync_poll cleared the flag.
            if !sync_pending.get_untracked() {
                return;
            }
            let d2 = d_kick.clone();
            let s2 = s_kick.clone();
            spawn_local(async move {
                d2.sync_poll_tick(&s2, generation, sync_cycle).await;
            });
        });

        // Set up the 2s interval poll.
        let d_interval = self.clone();
        let s_interval = session_id.clone();
        match set_interval_with_handle(
            move || {
                let d = d_interval.clone();
                let s = s_interval.clone();
                spawn_local(async move {
                    d.sync_poll_tick(&s, generation, sync_cycle).await;
                });
            },
            std::time::Duration::from_secs(2),
        ) {
            Ok(handle) => {
                self.sync_interval.set(Some(handle));
            }
            Err(err) => {
                log::error!("sync_poll interval creation failed: {err:?}");
                self.sync_pending.set(false);
            }
        }
    }

    /// Keyed cleanup guard shared by both `sync_poll_tick` completion arms.
    ///
    /// Returns `true` iff `(sse_generation, session_id, sync_cycle)` still
    /// match current — only a *matching* completion may clear the abort
    /// handle and in-flight flag. A **retired** completion (cycle bumped by
    /// a `stop_sync_poll` + restart) must leave the newer cycle's signals
    /// intact.
    ///
    /// On match this is the **sole** authority for both
    /// `sync_fetch_abort` (→ `None`) and `sync_in_flight` (→ `false`).
    /// There is no separate trailing `sync_in_flight` clear — all callers
    /// route through this helper.
    fn release_sync_fetch(&self, session_id: &str, sse_generation: u64, sync_cycle: u64) -> bool {
        let matched = self.sse_generation.get_untracked() == sse_generation
            && self.session_id.get_untracked().as_deref() == Some(session_id)
            && self.sync_cycle.get_untracked() == sync_cycle;

        if !matched {
            return false; // retired — touch nothing
        }

        // Matching completion: release both signals.
        self.sync_fetch_abort.set(None);
        self.sync_in_flight.set(false);
        true
    }

    /// One tick of the sync_poll: fetch the session detail, check guards,
    /// quarantine if live, atomically commit if terminal. Gen-gated — stale
    /// generations are rejected.
    async fn sync_poll_tick(&self, session_id: &str, sse_generation: u64, sync_cycle: u64) {
        // Guard 1: generation + session match.
        if self.sse_generation.get_untracked() != sse_generation
            || self.session_id.get_untracked().as_deref() != Some(session_id)
            || self.sync_cycle.get_untracked() != sync_cycle
        {
            return;
        }

        // Guard 2: deadline check BEFORE in_flight — a hung fetch cannot block
        // timeout from being observed.
        if let Some(deadline) = self.sync_deadline.get_untracked() {
            if now_millis() > deadline {
                // Deadline exceeded — transition to stalled.
                self.stop_sync_poll();
                self.sync_pending.set(true); // re-enable for stalled affordance
                self.status
                    .set("turn still active — refresh available".into());
                return;
            }
        }

        // Guard 3: prevent overlapping ticks.
        if self.sync_in_flight.get_untracked() {
            return;
        }
        self.sync_in_flight.set(true);

        // Abortable fetch. Capture the activity revision before the await: an
        // admitted TurnStarted/TurnFinished retires this snapshot even when the
        // focused session and SSE generation remain unchanged.
        let snapshot_activity_revision = self.activity_revision.get_untracked();
        let url = self.url.get_untracked();
        let get_url = format!(
            "{}/v1/sessions/{}",
            url.trim_end_matches('/'),
            encode_path_segment(session_id)
        );
        let (fut, abort_handle) = abortable(Self::fetch_session_detail_for_sync_impl(
            self.clone(),
            session_id.to_string(),
            get_url,
            sse_generation,
            sync_cycle,
            snapshot_activity_revision,
        ));
        self.sync_fetch_abort.set(Some(abort_handle));

        match fut.await {
            Ok(outcome) => {
                // Keyed guard — only a matching-cycle completion may touch
                // the abort handle or in_flight. Retired completions return
                // early, leaving the newer cycle's signals intact.
                if !self.release_sync_fetch(session_id, sse_generation, sync_cycle) {
                    return;
                }
                match outcome {
                    ProjectionOutcome::Terminal => {
                        // Turn finished — atomically replaced by the fetch, clear sync.
                        self.stop_sync_poll();
                        self.status.set("connected".into());
                    }
                    ProjectionOutcome::Live => {
                        // Still running — keep polling.
                    }
                }
            }
            Err(_aborted) => {
                // Release the handle + in_flight only if this cycle still
                // matches; a retired abort must not clear the newer cycle's
                // signals.
                self.release_sync_fetch(session_id, sse_generation, sync_cycle);
            }
        }
    }

    /// Fetch the session detail and commit with quarantine. Associated fn (not
    /// `&self` — takes a clone to survive the abortable wrapper). Returns the
    /// outcome for the tick to act on.
    async fn fetch_session_detail_for_sync_impl(
        self,
        session_id: String,
        get_url: String,
        sse_generation: u64,
        sync_cycle: u64,
        snapshot_activity_revision: u64,
    ) -> ProjectionOutcome {
        let resp = match Request::get(&get_url).send().await {
            Ok(r) => r,
            Err(error) => {
                log::warn!("sync_poll fetch error: {error}");
                return ProjectionOutcome::Live; // retry next tick
            }
        };
        if !resp.ok() {
            log::warn!("sync_poll fetch rejected: {}", resp.status());
            return ProjectionOutcome::Live;
        }
        let response = match resp.json::<SessionDetailResponse>().await {
            Ok(r) => r,
            Err(error) => {
                log::warn!("sync_poll decode error: {error}");
                return ProjectionOutcome::Live;
            }
        };
        if !response.ok {
            log::warn!("sync_poll daemon error: {:?}", response.error);
            return ProjectionOutcome::Live;
        }
        let Some(detail) = response.session else {
            log::warn!("sync_poll: session detail missing");
            return ProjectionOutcome::Live;
        };
        if detail.id != session_id {
            log::warn!("sync_poll: session id mismatch");
            return ProjectionOutcome::Live;
        }

        // Final admission happens inside the fetch helper, immediately before
        // any signal writes. The caller's post-await check is intentionally not
        // sufficient: this helper used to publish first and discover staleness
        // only after returning.
        self.try_commit_sync_snapshot(
            &detail,
            &session_id,
            sse_generation,
            sync_cycle,
            snapshot_activity_revision,
        )
    }

    /// Admission-gated commit of a parsed sync-poll snapshot. Returns
    /// [`ProjectionOutcome::Live`] when the snapshot is discarded (stale guard
    /// values) so the caller retries next tick. Extracted from the fetch helper
    /// so the production commit seam is independently testable (TASK-46 stage F).
    fn try_commit_sync_snapshot(
        &self,
        detail: &SessionDetail,
        session_id: &str,
        sse_generation: u64,
        sync_cycle: u64,
        snapshot_activity_revision: u64,
    ) -> ProjectionOutcome {
        if admit_sync_snapshot(
            session_id,
            sse_generation,
            sync_cycle,
            snapshot_activity_revision,
            self.session_id.get_untracked().as_deref(),
            self.sse_generation.get_untracked(),
            self.sync_cycle.get_untracked(),
            self.activity_revision.get_untracked(),
        ) == SnapshotAdmission::Discard
        {
            return ProjectionOutcome::Live;
        }

        // TASK-46 rollback: always commit the complete daemon transcript. The
        // prior live-state branch truncated at the last user entry and started a
        // detail poll, leaving legitimate long operations behind a "detail
        // syncing…" quarantine for their entire duration.
        let active_requests = detail.active_requests.clone();

        // Commit the full transcript atomically.
        let (rebuilt_turns, rebuilt_pinned) =
            turns_from_session_transcript(detail.transcript.clone(), &detail.tool_context);
        // Poll ticks reconcile only live-turn state; the permission view is owned
        // by the control stream / full projection, so this arm never publishes it
        // (Fresh(empty) here is discarded — only `active_turn_id`/`streaming` are
        // consumed below).
        let projection = build_session_projection(
            detail.state,
            &active_requests,
            SnapshotSettle::Fresh(Vec::new()),
            &[],
            &PermissionView::default(),
            &[],
            &self.decision_authority.get_untracked(),
        );

        self.turns.set(rebuilt_turns);
        self.pinned_widgets.set(rebuilt_pinned);
        self.active_turn_id.set(projection.active_turn_id);
        self.streaming.set(projection.streaming);
        self.session_title.set(detail.title.clone());
        if let Some(root) = detail.workspace_root.as_ref().or(detail.cwd.as_ref()) {
            if !root.is_empty() {
                self.cwd.set((*root).clone());
            }
        }
        if !detail.model.is_empty() {
            self.model.set(Some(detail.model.clone()));
        }

        ProjectionOutcome::Terminal
    }

    /// Switch to a different session. Clears the current turns, sets the
    /// session_id, opens the scoped SSE streams, then commits the daemon's
    /// authoritative projection for the new session. SSE is a live tail, not
    /// historical replay, so a switch must explicitly reconcile from the daemon.
    ///
    /// Ordering matters (TASK-44): `connect()` first, so the projection commits
    /// under the SSE generation the new streams run on and a second switch that
    /// races the fetch retires this one via the generation guard. The commit
    /// then restores transcript, pinned components, the live Stop target, and the
    /// pending-permission cards TOGETHER — a session that is running/gated
    /// elsewhere lands as a live, cancellable projection rather than a transient
    /// idle screen.
    pub fn switch_session(&self, id: String, title: String) {
        self.stop_sync_poll();
        self.select_session_state(&id, title);
        self.connect();
        let generation = self.sse_generation.get_untracked();
        self.spawn_session_projection(id, generation);
    }

    /// Spawn the authoritative projection commit for `(id, generation)`. Shared
    /// by explicit switch/load and by the reconnect paths so a single mechanism
    /// reconciles focus. A retired snapshot (focus or generation moved) is
    /// discarded silently; a genuine fetch failure surfaces only while this
    /// snapshot is still the current one.
    fn spawn_session_projection(&self, id: String, generation: u64) {
        let daemon = self.clone();
        spawn_local(async move {
            for attempt in 0..4 {
                match daemon.commit_session_projection(&id, generation).await {
                    Ok(ProjectionOutcome::Terminal) => {
                        // Turn is terminal — projection already committed, nothing to sync.
                        daemon.stop_sync_poll();
                        return;
                    }
                    Ok(ProjectionOutcome::Live) => {
                        // Turn is live — enter sync_pending mode.
                        daemon.start_sync_poll(id.clone(), generation);
                        return;
                    }
                    Err(error) if error == SESSION_PROJECTION_ACTIVITY_CHANGED && attempt < 3 => {
                        // An admitted lifecycle event raced the HTTP snapshot.
                        // Retry from a fresh revision rather than publishing it.
                        continue;
                    }
                    Err(error) => {
                        log::warn!("session projection commit skipped: {error}");
                        if admit_session_snapshot(
                            &id,
                            generation,
                            daemon.session_id.get_untracked().as_deref(),
                            daemon.sse_generation.get_untracked(),
                        ) == SnapshotAdmission::Commit
                        {
                            daemon
                                .status
                                .set(format!("session load failed: {}", concise_error(&error)));
                        }
                        return;
                    }
                }
            }
        });
    }

    fn select_session_state(&self, id: &str, title: String) {
        self.turns.set(Vec::new());
        self.streaming.set(false);
        self.active_turn_id.set(None);
        self.browser_active.set(false);
        self.browser_last_action.set(None);
        self.canvas_patches.set(Vec::new());
        self.pinned_widgets.set(Vec::new());
        self.pending_images.set(Vec::new());
        // TASK-69: clear permission cards/tombstones and advance the epoch so an
        // in-flight publisher for the session we're leaving is superseded (shared
        // with new-session + strict-resume via one helper — codex HOLD on af82ee8).
        self.retire_permission_state();
        self.active_decision_token.set(None);
        self.session_id.set(Some(id.to_string()));
        persist_session(id);
        // Switching focus is a new cockpit intent; retire any in-flight
        // fresh-session create so its late response cannot yank focus back
        // (TASK-43, finding 1).
        self.bump_session_intent();
        self.awaiting_session_adoption.set(false);
        self.session_title.set(title);
        self.status.set("loading session…".into());
        self.status_detail.set(None);
        self.reset_token_stats();
    }

    /// Fetch the daemon's authoritative snapshot for `id` and, if it is still the
    /// focused session on SSE `generation`, commit the WHOLE projection in one
    /// synchronous block: transcript, pinned components, live turn id + streaming
    /// state, and the pending-permission cards (with per-card decision authority
    /// resolved). This is the single reconciliation mechanism the switch and both
    /// reconnect paths share (TASK-44, findings 2/3/4).
    ///
    /// The two HTTP awaits (session detail, then pending permissions) happen
    /// up front; the projection is applied only after a final admission recheck,
    /// with no `await` between the signal writes, so no loop can publish a stale
    /// partial projection over another's fresh one. The live turn id/streaming
    /// come straight from the daemon's `state`/`active_requests`: a running,
    /// waiting-for-permission, or cancelling session is never transiently
    /// declared idle; only a terminal state clears the Stop target.
    ///
    /// Running/WaitingForPermission/Cancelling snapshots commit their complete
    /// persisted transcript immediately while preserving the daemon-owned live
    /// Stop state. Session switching never enters a client-side detail
    /// quarantine or requires a manual refresh.
    async fn commit_session_projection(
        &self,
        id: &str,
        generation: u64,
    ) -> Result<ProjectionOutcome, String> {
        let snapshot_activity_revision = self.activity_revision.get_untracked();
        // Claim this session-load's permission-projection epoch up front, BEFORE
        // either fetch. A standalone reconcile that starts later claims a higher
        // epoch and supersedes us, so our (older) permission publish will be
        // skipped rather than resurrect a gate the newer reconcile resolved
        // (codex HOLD on 8b41fee1). The transcript projection commits regardless.
        let permission_epoch_claim = self.claim_permission_epoch();
        let url = self.url.get_untracked();

        // 1. Authoritative session snapshot (transcript + atomic live-turn state).
        let get_url = format!(
            "{}/v1/sessions/{}",
            url.trim_end_matches('/'),
            encode_path_segment(id)
        );
        let resp = Request::get(&get_url)
            .send()
            .await
            .map_err(|error| format!("session fetch error: {error}"))?;
        if !resp.ok() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("session fetch rejected: {}", concise_error(&text)));
        }
        let response = resp
            .json::<SessionDetailResponse>()
            .await
            .map_err(|error| format!("session decode error: {error}"))?;
        if !response.ok {
            return Err(response.error.unwrap_or_else(|| "unknown error".into()));
        }
        let detail = response
            .session
            .ok_or_else(|| "session snapshot missing".to_string())?;
        if detail.id != id {
            return Err("session snapshot id mismatch".into());
        }
        if admit_session_snapshot(
            id,
            generation,
            self.session_id.get_untracked().as_deref(),
            self.sse_generation.get_untracked(),
        ) == SnapshotAdmission::Discard
        {
            return Err("session snapshot retired before commit".into());
        }

        // The permission revision AS OF the session snapshot: `detail.pending_permissions`
        // 2. Rich pending-permission cards, revision-gated against control-stream
        //    churn (reuses the same fetch the reconciler uses, but returns the
        //    cards for the atomic commit instead of publishing them alone).
        let mut source = DaemonPermissionSnapshotSource {
            daemon: self,
            session_id: id,
            // Fetch-only: this source feeds `settle_permission_snapshot` and never
            // publishes (the session-load commit publishes via `commit_permission_view`
            // under `permission_epoch_claim`), so its epoch is unused.
            epoch_claim: 0,
        };
        // The card snapshot is AUXILIARY: a failure here (route missing on a
        // host, daemon hiccup, revision churn) must NEVER fail the whole session
        // projection (the 63bc9ea invariant — transcript survives). But TASK-69:
        // it must NOT silently degrade to a blank card set either, or a gate that
        // predates this load goes invisible with no live frame to rescue it.
        // `settle_permission_snapshot` yields Fresh(cards) or Degraded(reason);
        // the two-state view (built below, under the same admission) carries the
        // distinction to the operator.
        let settle = settle_permission_snapshot(&mut source).await;
        if let SnapshotSettle::Degraded(reason) = &settle {
            log::warn!("session projection permission snapshot degraded for {id}: {reason}");
        }

        // Both HTTP awaits are done; the rest is the synchronous post-fetch
        // commit (final admission recheck, full transcript rebuild, and
        // the atomic signal commit incl. the two-state permission publish),
        // extracted so it is drivable in a native test with injected fetch
        // results (codex HOLD on c2a1810).
        apply_session_projection(
            &self.commit_signals(),
            id,
            generation,
            snapshot_activity_revision,
            permission_epoch_claim,
            detail,
            settle,
        )
    }

    /// Bundle the live signals `apply_session_projection` reads and writes.
    fn commit_signals(&self) -> SessionCommitSignals {
        SessionCommitSignals {
            session_id: self.session_id,
            sse_generation: self.sse_generation,
            activity_revision: self.activity_revision,
            streaming: self.streaming,
            active_turn_id: self.active_turn_id,
            decision_authority: self.decision_authority,
            permission_view: self.permission_view,
            permission_tombstones: self.permission_tombstones,
            permission_epoch: self.permission_epoch,
            session_title: self.session_title,
            cwd: self.cwd,
            model: self.model,
            turns: self.turns,
            pinned_widgets: self.pinned_widgets,
            status: self.status,
        }
    }

    /// TASK-69: claim the next permission-projection epoch. Called at the START
    /// of a permission-publishing operation (session-load commit or standalone
    /// reconcile); the returned ticket is the claimant's identity. A later claim
    /// supersedes it, so `publish_permission_view` will then skip this claimant's
    /// (now stale) publish — ordering the two otherwise-unsynchronized publishers.
    fn claim_permission_epoch(&self) -> u64 {
        self.permission_epoch
            .update(|epoch| *epoch = epoch.wrapping_add(1));
        self.permission_epoch.get_untracked()
    }

    /// TASK-69: retire the focused-session permission state at a session-IDENTITY
    /// transition. Clears the pending-permission view and the decision tombstones,
    /// and ADVANCES the epoch so any in-flight publisher for the session being
    /// left is superseded and cannot publish over the replacement. EVERY path that
    /// changes the active session identity must route through this — select/switch,
    /// new session, AND strict-resume retirement of an expired session — so
    /// old-session cards (and their request-bound decision authority, which
    /// `decide_permission` resolves by card/request, not by active session) never
    /// carry into the new session (codex HOLD on af82ee8: the strict-resume path
    /// bypassed both `select_session_state` and `new_session`). Deliberately does
    /// NOT touch `turns` — the strict-resume path preserves its transcript for the
    /// retry, and the switch/new paths clear turns separately.
    fn retire_permission_state(&self) {
        self.permission_view.set(PermissionView::default());
        self.permission_tombstones.set(Vec::new());
        self.claim_permission_epoch();
    }

    async fn hydrate_active_session(&self, id: &str) -> Result<Vec<String>, String> {
        let url = self.url.get_untracked();
        let get_url = format!(
            "{}/v1/sessions/{}",
            url.trim_end_matches('/'),
            encode_path_segment(id)
        );
        let resp = Request::get(&get_url)
            .send()
            .await
            .map_err(|error| format!("session fetch error: {error}"))?;
        if !resp.ok() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("session fetch rejected: {}", concise_error(&text)));
        }
        let response = resp
            .json::<SessionDetailResponse>()
            .await
            .map_err(|error| format!("session decode error: {error}"))?;
        if !response.ok {
            return Err(response.error.unwrap_or_else(|| "unknown error".into()));
        }
        let detail = response
            .session
            .ok_or_else(|| "session snapshot missing".to_string())?;
        if detail.id != id || self.session_id.get_untracked().as_deref() != Some(id) {
            return Err("session changed before snapshot hydration completed".into());
        }
        self.session_title.set(detail.title.clone());
        if let Some(root) = detail.workspace_root.or(detail.cwd) {
            if !root.is_empty() {
                self.cwd.set(root);
            }
        }
        if !detail.model.is_empty() {
            self.model.set(Some(detail.model));
        }
        let (rebuilt_turns, rebuilt_pinned) =
            turns_from_session_transcript(detail.transcript, &detail.tool_context);
        self.turns.set(rebuilt_turns);
        self.pinned_widgets.set(rebuilt_pinned);
        self.status.set("session loaded".into());
        Ok(detail.pending_permissions)
    }

    /// Adopt a freshly-created planner session without the ordinary spawned
    /// hydration race. The durable transcript snapshot is applied first; then
    /// both session-scoped live tails are subscribed; finally the durable
    /// permission registry is reconciled. The caller may post the first turn
    /// only after this future resolves and its generation/session guards hold.
    pub async fn adopt_planner_session(&self, id: &str, title: String) -> Result<u64, String> {
        self.select_session_state(id, title);
        self.hydrate_active_session(id).await?;
        if self.session_id.get_untracked().as_deref() != Some(id) {
            return Err("planner session changed during adoption".into());
        }
        self.connect();
        let generation = self.sse_generation.get_untracked();
        self.await_streams_ready(id, generation).await?;
        self.reconcile_pending_permissions(id).await?;
        if self.streams_ready(id, generation) {
            return Ok(generation);
        }
        Err("planner session changed during permission reconciliation".into())
    }

    /// Are BOTH the agent and permission live tails currently OPEN for exactly
    /// `(id, generation)` — both readiness markers set and both underlying
    /// EventSources reporting `Open`? Pure snapshot of the two-stream handshake,
    /// shared by planner adoption and the fresh-session bootstrap.
    fn streams_ready(&self, id: &str, generation: u64) -> bool {
        self.session_id.get_untracked().as_deref() == Some(id)
            && self.sse_generation.get_untracked() == generation
            && planner_streams_ready(
                id,
                generation,
                self.agent_stream_ready.get_untracked().as_ref(),
                self.permission_stream_ready.get_untracked().as_ref(),
            )
            && planner_sources_open(&self.planner_stream_sources.borrow(), id, generation)
    }

    /// Bounded wait for BOTH the agent and permission EventSources to report an
    /// OPEN marker for `(id, generation)`. Generalized from `adopt_planner_session`
    /// so an ordinary fresh-session create can withhold its first turn until the
    /// live tail is actually being observed — the event endpoint has no replay, so
    /// a turn POSTed before the streams open would drop `turn_started` and early
    /// frames into the void (TASK-43, finding 5).
    ///
    /// Returns `Err` if focus/generation moves off `(id, generation)` while
    /// waiting, or if the streams do not both open within the bounded window
    /// (240 × 25ms ≈ 6s). The caller surfaces that visibly and never dispatches
    /// into an unobserved stream. The bounded delay is only a timeout/yield;
    /// correctness comes from the EventSource `state()` the markers reflect.
    async fn await_streams_ready(&self, id: &str, generation: u64) -> Result<(), String> {
        for _ in 0..240 {
            if self.session_id.get_untracked().as_deref() != Some(id)
                || self.sse_generation.get_untracked() != generation
            {
                return Err("session changed while streams connected".into());
            }
            if self.streams_ready(id, generation) {
                return Ok(());
            }
            gloo_timers::future::TimeoutFuture::new(25).await;
        }
        Err("session streams did not become ready".into())
    }

    pub async fn refresh_planner_session(&self, id: &str) -> Result<(), String> {
        self.hydrate_active_session(id).await.map(|_| ())
    }

    async fn reconcile_pending_permissions(&self, session_id: &str) -> Result<(), String> {
        // Claim this reconcile's epoch at the START (before the fetch), so it can
        // supersede an older in-flight session-load publish (codex HOLD on
        // 8b41fee1).
        let mut source = DaemonPermissionSnapshotSource {
            daemon: self,
            session_id,
            epoch_claim: self.claim_permission_epoch(),
        };
        reconcile_permission_snapshot(&mut source).await
    }

    /// Reset to a fresh, not-yet-created session. Clears state and leaves
    /// `session_id` as `None` so the next prompt lazily creates a session
    /// (see `dispatch_prompt`).
    ///
    /// This preserves the default single-session flow: a user who never opens
    /// the session UI still gets a session created on their first message.
    /// Advance the session-intent generation. Called by every path that
    /// establishes a new cockpit focus (New Session, session switch) so a
    /// fresh-session create that captured an earlier generation is retired
    /// (TASK-43, finding 1).
    fn bump_session_intent(&self) {
        self.session_intent_generation
            .update(|generation| *generation = generation.wrapping_add(1));
    }

    pub fn new_session(&self) {
        self.reset_session_local_state();
        clear_persisted_session();
        self.connect();
    }

    /// Drop every scrap of state that belonged to the session we are leaving,
    /// WITHOUT opening a stream and without forgetting the persisted session
    /// id.
    ///
    /// Two callers, and the split is exactly where they differ. [`new_session`]
    /// forgets the id (a fresh cockpit intent has no past) and connects.
    /// [`reattach_to_selected_device`] keeps the id, because whether the
    /// machine we just moved to has that session is the next question it asks
    /// — and it opens its stream through the restore path or `connect`, never
    /// both.
    fn reset_session_local_state(&self) {
        self.stop_sync_poll();
        self.turns.set(Vec::new());
        self.streaming.set(false);
        self.active_turn_id.set(None);
        self.browser_active.set(false);
        self.browser_last_action.set(None);
        self.canvas_patches.set(Vec::new());
        self.pinned_widgets.set(Vec::new());
        self.pending_images.set(Vec::new());
        // TASK-69: clear permission cards/tombstones and advance the epoch so an
        // in-flight publisher for the session we're leaving is superseded (shared
        // with new-session + strict-resume via one helper — codex HOLD on af82ee8).
        self.retire_permission_state();
        self.active_decision_token.set(None);
        self.session_id.set(None);
        // A New Session is a fresh cockpit intent. Bump the intent generation so
        // a still-in-flight fresh-session create — even one that also left the
        // active session `None` — is retired and cannot adopt over this intent
        // (TASK-43, finding 1). A device switch needs the same retirement, for
        // the same reason and more so: the in-flight create it retires was
        // aimed at a different machine.
        self.bump_session_intent();
        self.awaiting_session_adoption.set(false);
        self.session_title.set(String::new());
        self.status.set("new session".into());
        self.status_detail.set(None);
        self.reset_token_stats();
    }

    /// Follow the surface onto the machine this session was just attached to.
    ///
    /// The proxy has already recorded the choice, so every request from here
    /// on lands on the new daemon; what has to happen client-side is that
    /// nothing from the old one is left standing. The SSE stream, the
    /// transcript, the model catalogue, the project catalogue and the session
    /// list all belong to the machine we left — `connect` bumps the stream
    /// generation, which retires the old tail rather than racing it.
    ///
    /// The one thing that may cross is the remembered session id, and only if
    /// the new machine actually has it: `restore_session_or_clear` reopens it
    /// when it does and forgets it when it does not
    /// ([`crate::devices::classify_session_restore`]). A session that is not
    /// on this machine is the ordinary case of two machines with different
    /// histories — never an error put in front of the person.
    pub fn reattach_to_selected_device(&self) {
        let daemon = self.clone();
        spawn_local(async move {
            // Read the remembered id BEFORE the reset, which is what makes the
            // reset safe to share with `new_session`.
            let persisted = load_persisted_session();
            daemon.reset_session_local_state();
            // Everything on screen now describes a machine we are leaving.
            // Retiring the epoch here is what lets a catalogue fetch already
            // in flight against it be dropped rather than land on top of the
            // new machine's.
            daemon.device_epoch.update(|e| *e = e.wrapping_add(1));
            daemon.models.set(Vec::new());
            daemon.projects.set(Vec::new());
            daemon.status.set("switching device".into());
            // The intent this restore belongs to. A session-detail fetch is a
            // round trip, and the composer is live throughout it: whoever
            // starts a session in that window owns the focus, and a restore
            // that lands afterwards must not yank it back or clear the id that
            // session just persisted.
            let intent = daemon.session_intent_generation.get_untracked();
            let outcome = match should_restore_session(
                persisted.as_deref(),
                daemon.session_id.get_untracked().as_deref(),
            ) {
                Some(id) => restore_session_or_clear(&daemon, id, intent).await,
                None => RestoreOutcome::Cleared,
            };
            match outcome {
                // The machine has that session; its own workspace root and
                // model arrive with the projection commit.
                RestoreOutcome::Restored => {}
                RestoreOutcome::Cleared => {
                    // No session to inherit a workspace from, and the cwd and
                    // project still on the signals belong to the machine we
                    // left. A path from another machine either fails session
                    // creation or — worse — resolves to an unrelated directory
                    // that happens to share the name, and the next prompt would
                    // run the agent there. Fall back to the same projectless
                    // Chat root `begin_chat_session` uses; the new machine's
                    // own projects arrive with its catalogue.
                    daemon.set_project(None);
                    daemon.cwd.set(CHAT_WORKSPACE_ROOT.to_string());
                    daemon.connect();
                }
                // Somebody started another session while the restore was in
                // flight. That intent owns the focus and has opened its own
                // stream; touching either here is the yank this guard exists
                // to prevent.
                RestoreOutcome::Superseded => {}
            }
            // The catalogues are per-machine too: a model, project or session
            // list from the old daemon is a list of things that are not here.
            daemon.fetch_models();
            daemon.fetch_projects();
            daemon.fetch_sessions();
        });
    }

    /// Select project `id`, pin the working directory to the explicit section
    /// root resolved by the sessions panel, then reset to a lazy new session.
    /// The caller supplies catalogue root first and a session-derived fallback
    /// otherwise. Empty roots are rejected before project or cwd state changes.
    pub fn begin_project_session(&self, id: String, workspace_root: String) {
        let Some(workspace_root) = project_session_cwd(&workspace_root) else {
            log::warn!("project session requires a nonempty workspace root");
            return;
        };
        self.set_project(Some(id));
        self.cwd.set(workspace_root);
        self.new_session();
    }

    /// Start a projectless Chat session: clear the project selection and pin
    /// the working directory to [`CHAT_WORKSPACE_ROOT`] (`/tmp`), then reset to
    /// a lazy new session. Chat is never a fake project — clearing `project`
    /// and setting `cwd` BEFORE the lazy [`new_session`] means the next
    /// `new_session()` POST carries `/tmp` to the daemon as the session's real
    /// working directory, with no `project_id` attached. `new_session` does not
    /// reset `cwd`, so the `/tmp` set here survives into the session.
    pub fn begin_chat_session(&self) {
        self.set_project(None);
        self.cwd.set(CHAT_WORKSPACE_ROOT.to_string());
        self.new_session();
    }

    /// Create a real project on the daemon (`POST /v1/projects`) and land in a
    /// fresh session bound to it. The daemon is the authority for project state
    /// — we never `mkdir` or invent a project locally. Empty `name` or
    /// `workspace_root` (after trimming) is rejected with an inline error
    /// ([`project_create_error`]) instead of a POST. While the POST is in flight
    /// [`project_create_pending`] is true (drives the Create button label). On
    /// success the returned project is upserted into the `projects` catalogue
    /// and selected via `set_project` (persisted, no session created),
    /// and the status flips to a concise `project created`. On any failure the
    /// status becomes `project create failed: <concise reason>` and the same
    /// reason lands on [`project_create_error`] so the form stays open with an
    /// inline error.
    pub fn create_project(&self, name: String, workspace_root: String) {
        let name = name.trim().to_string();
        let workspace_root = workspace_root.trim().to_string();
        if name.is_empty() || workspace_root.is_empty() {
            self.status
                .set("project create failed: name and workspace required".into());
            self.project_create_error
                .set(Some("name and workspace required".into()));
            return;
        }
        self.project_create_error.set(None);
        self.project_create_pending.set(true);
        let url = self.url.get_untracked();
        let projects = self.projects;
        let status = self.status;
        let pending = self.project_create_pending;
        let error = self.project_create_error;
        let daemon = self.clone();
        let body = ProjectCreateRequest {
            name,
            workspace_root,
        };
        spawn_local(async move {
            let post_url = format!("{}/v1/projects", url.trim_end_matches('/'));
            let req = match Request::post(&post_url)
                .header("content-type", "application/json")
                .json(&body)
            {
                Ok(req) => req,
                Err(err) => {
                    let raw = err.to_string();
                    let msg = concise_error(&raw);
                    log::error!("project create encode error: {raw}");
                    status.set(format!("project create failed: {msg}"));
                    error.set(Some(msg));
                    pending.set(false);
                    return;
                }
            };
            match req.send().await {
                Ok(resp) => match resp.json::<ProjectCreateResponse>().await {
                    Ok(r) if r.ok => {
                        let Some(project) = r.project else {
                            log::error!("project create returned ok with no project");
                            status.set("project create failed: no project returned".into());
                            error.set(Some("no project returned".into()));
                            pending.set(false);
                            return;
                        };
                        // Upsert so the picker reflects the new project
                        // without a round-trip refresh: replace an existing id
                        // in place, else append.
                        projects.update(|list| {
                            if let Some(slot) = list.iter_mut().find(|p| p.id == project.id) {
                                *slot = project.clone();
                            } else {
                                list.push(project.clone());
                            }
                        });
                        status.set("project created".into());
                        daemon.set_project(Some(project.id.clone()));
                        pending.set(false);
                    }
                    Ok(r) => {
                        let raw = r.error.unwrap_or_else(|| "project create rejected".into());
                        let msg = concise_error(&raw);
                        log::error!("project create rejected: {raw}");
                        status.set(format!("project create failed: {msg}"));
                        error.set(Some(msg));
                        pending.set(false);
                    }
                    Err(err) => {
                        let raw = err.to_string();
                        let msg = concise_error(&raw);
                        log::error!("project create decode error: {raw}");
                        status.set(format!("project create failed: {msg}"));
                        error.set(Some(msg));
                        pending.set(false);
                    }
                },
                Err(err) => {
                    let raw = err.to_string();
                    let msg = concise_error(&raw);
                    log::error!("project create post error: {raw}");
                    status.set(format!("project create failed: {msg}"));
                    error.set(Some(msg));
                    pending.set(false);
                }
            }
        });
    }

    /// Clear per-turn and session token counters (on session change).
    fn reset_token_stats(&self) {
        self.last_turn_tokens.set(None);
        self.session_tokens.set(TokenStats::default());
    }

    /// Send a component interaction event back to the daemon.
    /// This is how the web surface tells the agent "user clicked a kanban card"
    /// or "user submitted a form". If a `component_wait` is pending on the
    /// agent side, it resolves immediately; otherwise the event is queued for
    /// the next turn.
    pub fn send_component_event(&self, component_id: String, payload: Value) {
        let sid = self.session_id.get_untracked();
        let Some(session_id) = sid else {
            self.status.set("no session — send a prompt first".into());
            return;
        };
        let url = self.url.get_untracked();
        let status = self.status;
        spawn_local(async move {
            let body = ComponentEventRequest {
                session_id,
                component_id,
                event: payload,
            };
            let post_url = format!("{}/v1/component/event", url.trim_end_matches('/'));
            let res = Request::post(&post_url)
                .header("content-type", "application/json")
                .json(&body);
            let res = match res {
                Ok(req) => req.send().await,
                Err(err) => {
                    let raw = err.to_string();
                    log::error!("component event encode error: {raw}");
                    status.set(format!(
                        "component event encode error: {}",
                        concise_error(&raw)
                    ));
                    return;
                }
            };
            match res {
                Ok(resp) => {
                    if !resp.ok() {
                        let text = resp.text().await.unwrap_or_default();
                        // A wait-miss is a benign race, not an operator-facing
                        // fault: the agent's `component_wait` already resolved
                        // (or was never armed) when the click landed — the
                        // daemon answers 404 "no pending wait for component" /
                        // 410 "nobody waiting". Surfacing that as a status
                        // toast made every stray click look like a failure.
                        if text.contains("no pending wait") || text.contains("nobody waiting") {
                            log::warn!("component event ignored (no waiter): {text}");
                        } else {
                            log::error!("component event error: {text}");
                            status.set(format!("component event error: {}", concise_error(&text)));
                        }
                    }
                }
                Err(err) => {
                    let raw = err.to_string();
                    log::error!("component event post error: {raw}");
                    status.set(format!(
                        "component event post error: {}",
                        concise_error(&raw)
                    ));
                }
            }
        });
    }
    /// Ask the daemon for an ephemeral OpenAI Realtime client secret for the
    /// existing conversation mode. Omitted purpose remains byte-compatible.
    pub async fn realtime_client_secret(
        &self,
        session_id: Option<String>,
    ) -> Result<RealtimeSecret, String> {
        self.post_realtime_secret(&json!({ "session_id": session_id }))
            .await
    }

    /// Mint a propose-only planner Realtime secret for daemon-validated frozen
    /// context. Project names are deliberately absent from the wire request.
    pub async fn realtime_planner_client_secret(
        &self,
        context: &crate::voice::planner::PlannerContext,
    ) -> Result<RealtimeSecret, String> {
        context.validate()?;
        let body = PlannerRealtimeSecretRequest {
            purpose: "planner",
            planner_context: PlannerRealtimeContextRequest {
                project_id: &context.project_id,
                workspace_root: &context.workspace_root,
            },
        };
        self.post_realtime_secret(&body).await
    }

    async fn post_realtime_secret(&self, body: &impl Serialize) -> Result<RealtimeSecret, String> {
        let url = self.url.get_untracked();
        let post_url = format!(
            "{}/v1/voice/realtime/client-secret",
            url.trim_end_matches('/')
        );
        let resp = Request::post(&post_url)
            .header("content-type", "application/json")
            .json(body)
            .map_err(|e| format!("secret request encode failed: {e}"))?
            .send()
            .await
            .map_err(|e| format!("secret request failed: {e}"))?;
        if !resp.ok() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("secret mint failed: {}", concise_error(&text)));
        }
        resp.json::<RealtimeSecret>()
            .await
            .map_err(|e| format!("secret decode failed: {e}"))
    }

    /// Explicitly create the one session authorized by a planner confirmation.
    /// Unlike ordinary lazy creation this request never carries a title hint.
    pub async fn create_planner_session(
        &self,
        context: &crate::voice::planner::PlannerContext,
    ) -> Result<String, String> {
        context.validate()?;
        let body = AgentSessionCreateRequest {
            title: None,
            workspace_root: &context.workspace_root,
            project_id: Some(&context.project_id),
            client_type: Some(surface_client_type()),
        };
        let url = self.url.get_untracked();
        let post_url = format!("{}/v1/agent/sessions", url.trim_end_matches('/'));
        let resp = Request::post(&post_url)
            .header("content-type", "application/json")
            .json(&body)
            .map_err(|e| format!("session encode failed: {e}"))?
            .send()
            .await
            .map_err(|e| format!("session create failed: {e}"))?;
        if !resp.ok() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("session create rejected: {}", concise_error(&text)));
        }
        let response = resp
            .json::<AgentSessionCreateResponse>()
            .await
            .map_err(|e| format!("session decode failed: {e}"))?;
        if !response.ok {
            return Err(response
                .error
                .unwrap_or_else(|| "session create failed".into()));
        }
        response
            .session_id
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| "session create failed: missing session id".into())
    }

    /// Persist the confirmed PRD as a non-executing planner handoff.
    pub async fn append_planner_handoff(
        &self,
        session_id: &str,
        markdown: &str,
    ) -> Result<(), String> {
        let url = self.url.get_untracked();
        // TASK-84: percent-encode — missed by TASK-77 because its regression
        // needle matched only the `{id}` binding, not `{session_id}`.
        let post_url = format!(
            "{}/v1/agent/sessions/{}/messages",
            url.trim_end_matches('/'),
            encode_path_segment(session_id)
        );
        let body = PlannerHandoffRequest {
            role: "user",
            kind: "planner_handoff",
            content: markdown,
        };
        let resp = Request::post(&post_url)
            .header("content-type", "application/json")
            .json(&body)
            .map_err(|e| format!("append encode failed: {e}"))?
            .send()
            .await
            .map_err(|e| format!("append failed: {e}"))?;
        if !resp.ok() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("append rejected: {}", concise_error(&text)));
        }
        Ok(())
    }

    /// Submit exactly one normal coding turn to the already-created and adopted
    /// planner session. No append, voice profile, or optional power controls are
    /// involved; the fresh token remains available to permission cards.
    pub async fn start_planner_turn(
        &self,
        session_id: &str,
        context: &crate::voice::planner::PlannerContext,
        markdown: &str,
    ) -> Result<(), String> {
        if self.session_id.get_untracked().as_deref() != Some(session_id) {
            return Err("created planner session is no longer active".into());
        }
        let generation = self.sse_generation.get_untracked();
        if !planner_streams_ready(
            session_id,
            generation,
            self.agent_stream_ready.get_untracked().as_ref(),
            self.permission_stream_ready.get_untracked().as_ref(),
        ) || !planner_sources_open(
            &self.planner_stream_sources.borrow(),
            session_id,
            generation,
        ) {
            return Err("planner session streams are not open".into());
        }
        let decision_token = mint_decision_token();
        if decision_token.len() != 64
            || !decision_token.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("secure decision token unavailable".into());
        }
        self.active_decision_token.set(Some(decision_token.clone()));
        self.turns.update(|turns| {
            if !planner_prompt_already_echoed(turns, markdown) {
                turns.push(Turn::user(markdown));
            }
        });
        self.streaming.set(true);
        let body = AgentTurnRequest {
            prompt: markdown,
            cwd: &context.workspace_root,
            session_id: Some(session_id),
            project_id: Some(&context.project_id),
            client_type: Some(surface_client_type()),
            guidance: None,
            room_id: None,
            thinking_level: None,
            model_id: None,
            images: None,
            decision_token: Some(&decision_token),
            client_context: None,
            agent: None,
            canvas: None,
        };
        let url = self.url.get_untracked();
        let post_url = format!("{}/v1/agent/turns", url.trim_end_matches('/'));
        let result = async {
            let resp = Request::post(&post_url)
                .header("content-type", "application/json")
                .json(&body)
                .map_err(|e| format!("turn encode failed: {e}"))?
                .send()
                .await
                .map_err(|e| format!("turn failed: {e}"))?;
            if !resp.ok() {
                let text = resp.text().await.unwrap_or_default();
                return Err(format!("turn rejected: {}", concise_error(&text)));
            }
            let response = resp
                .json::<AgentTurnResponse>()
                .await
                .map_err(|e| format!("turn decode failed: {e}"))?;
            if !response.ok {
                return Err(response.error.unwrap_or_else(|| "turn failed".into()));
            }
            if response.session_id != session_id {
                return Err("turn response returned a different session".into());
            }
            // Bind decision authority for this planner turn (TASK-44, finding 4),
            // keyed by the turn/request id so a gate on it resolves to exactly
            // this token.
            self.decision_authority.update(|authority| {
                authority.insert(
                    response.turn_id.clone(),
                    DecisionGrant {
                        token: decision_token.clone(),
                        session_id: session_id.to_string(),
                    },
                );
            });
            Ok(())
        }
        .await;
        if result.is_err() {
            if self.session_id.get_untracked().as_deref() == Some(session_id) {
                self.streaming.set(false);
            }
            if self.active_decision_token.get_untracked().as_deref()
                == Some(decision_token.as_str())
            {
                self.active_decision_token.set(None);
            }
        }
        result
    }

    /// Append an out-of-turn message to a chat session (the voice agent's
    /// `write_handoff` notes) via `POST /v1/agent/sessions/{id}/messages`.
    pub async fn append_session_message(
        &self,
        session_id: &str,
        content: &str,
        kind: Option<&str>,
    ) -> Result<(), String> {
        let url = self.url.get_untracked();
        // TASK-84: percent-encode — missed by TASK-77 because its regression
        // needle matched only the `{id}` binding, not `{session_id}`.
        let post_url = format!(
            "{}/v1/agent/sessions/{}/messages",
            url.trim_end_matches('/'),
            encode_path_segment(session_id)
        );
        let body = json!({ "role": "user", "content": content, "kind": kind });
        let resp = Request::post(&post_url)
            .header("content-type", "application/json")
            .json(&body)
            .map_err(|e| format!("append encode failed: {e}"))?
            .send()
            .await
            .map_err(|e| format!("append failed: {e}"))?;
        if !resp.ok() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("append rejected: {}", concise_error(&text)));
        }
        Ok(())
    }

    /// Fold a locally-originated component render into the transcript — the
    /// realtime voice agent's `render_component` tool arriving over the
    /// WebRTC data channel (voice phases 2/3). Mirrors the SSE
    /// `ComponentRender` reducer: upsert by id anywhere in the transcript,
    /// else attach to the current assistant turn.
    pub fn render_local_component(&self, component_id: String, kind: String, props: Value) {
        // Mirrors the SSE ComponentRender reducer: a pinned render docks into
        // the rail instead of the transcript.
        if component_placement(&props) == ComponentPlacement::Pinned {
            let widget = PinnedWidget {
                component_id: component_id.clone(),
                kind: kind.clone(),
                props: props.clone(),
            };
            self.pinned_widgets
                .update(|pinned| upsert_pinned(pinned, widget));
            return;
        }
        self.turns.update(|t| {
            for turn in t.iter_mut() {
                for block in turn.blocks.iter_mut() {
                    if let Block::Component {
                        component_id: id, ..
                    } = block
                    {
                        if *id == component_id {
                            *block = Block::Component {
                                component_id: component_id.clone(),
                                kind: kind.clone(),
                                props: props.clone(),
                            };
                            return;
                        }
                    }
                }
            }
            let turn = ensure_component_turn(t);
            turn.blocks.push(Block::Component {
                component_id,
                kind,
                props,
            });
        });
    }

    /// Remove a pinned widget from the rail (the card's unpin affordance).
    /// Session-scoped: the registry is cleared on session switch anyway, so
    /// this only undocks within the current session. A subsequent re-render of
    /// the same id re-pins it.
    pub fn unpin_widget(&self, component_id: &str) {
        self.pinned_widgets
            .update(|pinned| pinned.retain(|w| w.component_id != component_id));
    }
}

/// The daemon's `POST /v1/voice/realtime/client-secret` response (voice
/// phases 2/3): a short-lived secret the browser presents straight to
/// OpenAI's Realtime `calls` endpoint, plus the model it was minted for.
/// `expires_at` is upstream metadata we don't act on client-side.
#[derive(Debug, Clone, Deserialize)]
pub struct RealtimeSecret {
    pub client_secret: String,
    pub model: String,
    /// Canonical registered project root/live worktree resolved by the daemon
    /// for an ordinary conversation session. Omitted for planner, session-less,
    /// project-less, and older-daemon responses.
    #[serde(default)]
    pub workspace_root: Option<String>,
}

/// Whether a `TurnFinished` frame may clear the session's live turn state
/// (`streaming` + `active_turn_id`) and write the header status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalAdmission {
    /// The finishing turn is the active one — clear the live Stop target and
    /// running projection, and let it own the header status.
    ClearLive,
    /// A stale/superseded finish: a different turn than the active one, or a
    /// finish arriving with no active turn. Its own turn-scoped effects still
    /// apply, but it must not blank another turn's live state or steal its
    /// header status.
    KeepLive,
}

/// Decide whether a `TurnFinished` for `event_turn_id` may clear the live turn
/// state, given the currently-active `active_turn_id` (TASK-45).
///
/// A session transcript legitimately carries multiple turn ids (the turn-scoped
/// tool sweep below already relies on this), so a finish frame is not proof that
/// the *current* turn ended. Clear the live Stop target and running projection
/// only on an exact id match. A finish for any other turn — or one that arrives
/// when no turn is active — keeps the live state intact; its transcript block,
/// token accounting, and turn-scoped tool sweep remain scoped to
/// `event_turn_id` at the call site.
///
/// Pure over its two inputs (no signals, no `web-sys`) so it is unit-tested off
/// the browser, mirroring the voice capture-admission decider.
fn admit_turn_terminal(active_turn_id: Option<&str>, event_turn_id: &str) -> TerminalAdmission {
    match active_turn_id {
        Some(active) if active == event_turn_id => TerminalAdmission::ClearLive,
        _ => TerminalAdmission::KeepLive,
    }
}

// ── TASK-43: fresh-session bootstrap admission ────────────────────────────────
//
// `dispatch_prompt` creates a session lazily when `session_id` is `None`. Two
// hazards live in the gap between the create POST leaving and its response
// landing (findings 1 + 5): the user may switch sessions or start a second New
// Session while the POST is in flight — a stale response must not yank the
// cockpit, dispatch its prompt, or clobber status — and, because the event
// endpoint is a live tail with no replay, the first turn must not be POSTed until
// both live streams are actually being observed. Both are gated by binding the
// create to the session-INTENT generation captured when the prompt was
// submitted; a New Session or switch bumps that generation (see
// `bump_session_intent`), so even the New-Session case — which also leaves the
// active session `None` — is caught by the generation alone.

/// Whether a completed fresh-session create (or its post-readiness first-turn
/// dispatch) may still act on the cockpit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreateAdmission {
    /// The captured intent is still current and focus matches the caller's
    /// expectation — install the created session / dispatch its first turn.
    Adopt,
    /// A newer intent (a switch or a second New Session) superseded this create.
    /// The caller may refresh the session list, but must change no focus,
    /// dispatch no prompt, and clobber no status.
    Reject,
}

/// Pure admission for the fresh-session bootstrap (TASK-43, findings 1 + 5).
///
/// `launch_generation` is the session-intent generation captured at submit;
/// `current_generation` is the surface's intent generation now. `expected_session`
/// is the focus the caller requires: `None` before the created id is installed
/// (phase 1, immediately after the create POST returns), `Some(id)` after install
/// and stream readiness (phase 2, immediately before the first turn POST).
/// Adoption requires BOTH an unchanged generation AND a matching focus, so a
/// second New Session (generation bumped, focus back to `None`) is rejected in
/// phase 1 and a focus/intent move during the stream handshake is rejected in
/// phase 2.
fn admit_create_intent(
    launch_generation: u64,
    current_generation: u64,
    expected_session: Option<&str>,
    current_session: Option<&str>,
) -> CreateAdmission {
    if launch_generation == current_generation && current_session == expected_session {
        CreateAdmission::Adopt
    } else {
        CreateAdmission::Reject
    }
}

// ── TASK-44: atomic session projection — switch/reconnect reconciliation ──────
//
// The reducers below are pure over their inputs (no signals, no `web-sys`) so
// every admission decision — session+generation match, live-turn projection,
// per-card decision-authority resolution, and authority pruning — unit-tests off
// the browser, mirroring `admit_turn_terminal` above. `commit_session_projection`
// is the single async caller that fetches the daemon's authoritative snapshot,
// runs these deciders, and applies the whole projection in one synchronous block.

/// A decision token this surface minted for a turn it submitted, tagged with the
/// session it belongs to so authority can be pruned per session without
/// discarding another session's live grants (TASK-44, finding 4).
#[derive(Debug, Clone, PartialEq, Eq)]
struct DecisionGrant {
    token: String,
    session_id: String,
}

/// Whether a fetched session snapshot may still be committed, given the session
/// and SSE generation it was issued under versus the surface's current focus.
/// A switch or a newer reconnect moves focus or bumps the generation, retiring
/// any in-flight snapshot so a stale settle cannot overwrite the live projection
/// (TASK-44: "switch during an in-flight snapshot" / "old-generation snapshot
/// settling after a newer one").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotAdmission {
    Commit,
    Discard,
}

fn admit_session_snapshot(
    snapshot_session: &str,
    snapshot_generation: u64,
    current_session: Option<&str>,
    current_generation: u64,
) -> SnapshotAdmission {
    if current_session == Some(snapshot_session) && current_generation == snapshot_generation {
        SnapshotAdmission::Commit
    } else {
        SnapshotAdmission::Discard
    }
}

/// Extends the focus/generation admission with the session activity revision
/// captured before an async fetch. Any admitted TurnStarted or TurnFinished
/// retires the fetched snapshot, even when focus did not move.
fn admit_activity_snapshot(
    snapshot_session: &str,
    snapshot_generation: u64,
    snapshot_activity_revision: u64,
    current_session: Option<&str>,
    current_generation: u64,
    current_activity_revision: u64,
) -> SnapshotAdmission {
    if admit_session_snapshot(
        snapshot_session,
        snapshot_generation,
        current_session,
        current_generation,
    ) == SnapshotAdmission::Commit
        && current_activity_revision == snapshot_activity_revision
    {
        SnapshotAdmission::Commit
    } else {
        SnapshotAdmission::Discard
    }
}

/// Poll snapshots additionally belong to one sync cycle. Stopping or replacing
/// a poll bumps the cycle, so its late fetch can never publish into a newer poll.
#[allow(clippy::too_many_arguments)]
fn admit_sync_snapshot(
    snapshot_session: &str,
    snapshot_generation: u64,
    snapshot_cycle: u64,
    snapshot_activity_revision: u64,
    current_session: Option<&str>,
    current_generation: u64,
    current_cycle: u64,
    current_activity_revision: u64,
) -> SnapshotAdmission {
    if admit_activity_snapshot(
        snapshot_session,
        snapshot_generation,
        snapshot_activity_revision,
        current_session,
        current_generation,
        current_activity_revision,
    ) == SnapshotAdmission::Commit
        && current_cycle == snapshot_cycle
    {
        SnapshotAdmission::Commit
    } else {
        SnapshotAdmission::Discard
    }
}

/// Derive the live turn projection (`active_turn_id`, `streaming`) from the
/// daemon's atomic `state` + `active_requests`. A live state
/// (running / waiting-for-permission / cancelling) with a live request restores
/// a cancellable Stop target; a terminal or absent state clears it. Never
/// declares idle while the daemon reports the turn live (TASK-44, finding 3).
fn project_live_turn(
    state: Option<SessionRunState>,
    active_requests: &[String],
) -> (Option<String>, bool) {
    match state {
        Some(s) if s.is_live() => match active_requests.first() {
            Some(request_id) => (Some(request_id.clone()), true),
            // Daemon says live but named no request — treat as idle rather than
            // invent a Stop target we can't cancel.
            None => (None, false),
        },
        _ => (None, false),
    }
}

/// Resolve the decision token bound to a card's originating request, if this
/// surface holds it FOR THAT CARD'S SESSION. Returns the token to replay on the
/// decision POST, or `None` when the card is non-actionable (remote-origin, a
/// request this surface never submitted, or a grant belonging to a DIFFERENT
/// session) — in which case no decision is ever posted (TASK-44, finding 4).
///
/// The `card_session_id` gate (codex HOLD on af82ee8) is what prevents a leftover
/// grant from a retired session from authorizing a new-session card that happens
/// to reuse or collide on the request id: a token is bound to a session, not just
/// a request id.
fn resolve_decision_authority<'a>(
    request_id: Option<&str>,
    card_session_id: &str,
    authority: &'a HashMap<String, DecisionGrant>,
) -> Option<&'a str> {
    request_id
        .and_then(|id| authority.get(id))
        .filter(|grant| grant.session_id == card_session_id)
        .map(|grant| grant.token.as_str())
}

/// Stamp each card's `actionable` flag from local authority, so the view can
/// render a card from another surface read-only.
fn mark_cards_actionable(
    cards: &mut [PendingPermission],
    authority: &HashMap<String, DecisionGrant>,
) {
    for card in cards.iter_mut() {
        card.actionable =
            resolve_decision_authority(card.request_id.as_deref(), &card.session_id, authority)
                .is_some();
    }
}

/// Drop this session's authority grants whose request is no longer live, while
/// preserving grants for other sessions (their turns may still be running).
/// Keeping the map bounded to live requests is what lets a snapshot commit
/// "preserve matching local authority by request id" without leaking tokens for
/// finished turns (TASK-44, finding 4).
fn prune_session_authority(
    authority: &mut HashMap<String, DecisionGrant>,
    session_id: &str,
    live_requests: &HashSet<String>,
) {
    authority.retain(|request_id, grant| {
        grant.session_id != session_id || live_requests.contains(request_id)
    });
}

/// The whole focused-session projection derived from one authoritative snapshot:
/// the live-turn state and the pending-permission cards (with per-card
/// actionability resolved). Transcript + pinned components are rebuilt by
/// `turns_from_session_transcript` and committed alongside this in the same
/// synchronous block; they are omitted here so the projection stays cheap to
/// unit-test. Pure, so the cross-surface convergence test can feed two
/// independent authority maps the same snapshot and assert identical live state
/// with only per-card actionability differing.
#[derive(Debug, Clone, PartialEq)]
struct SessionProjection {
    active_turn_id: Option<String>,
    streaming: bool,
    permission: PermissionView,
}

/// The pure permission-and-live-turn decision the commit path delegates to. ALL
/// two-state permission logic lives HERE; the async `commit_session_projection`
/// only performs the two `fetch`es and hands the results to
/// `apply_session_projection`, which calls this — so a test driving the
/// post-fetch wrapper (over a `Daemon::dummy()`, with injected fetch results)
/// exercises the real production path (codex HOLD on 8b41fee/4cfb5c2/c2a1810).
///
/// `detail_pending_ids` = the daemon's authoritative `SessionDetail.pending_permissions`
/// (first-load proof). `prior_view` = the live view at commit time, whose
/// same-session cards must survive a degrade. `resolved_ids` = decision
/// tombstones: ids decided since the last Fresh snapshot. A degrade folds
/// `detail_pending_ids − resolved_ids`, so a gate decided during the load cannot
/// resurrect from its now-stale detail id (codex HOLD on 4cfb5c2, seam 5) while
/// an unrelated interleaved request still leaves a genuinely-open detail id
/// visible — a distinction a scalar revision gate cannot make.
fn build_session_projection(
    state: Option<SessionRunState>,
    active_requests: &[String],
    settle: SnapshotSettle,
    detail_pending_ids: &[String],
    prior_view: &PermissionView,
    resolved_ids: &[String],
    authority: &HashMap<String, DecisionGrant>,
) -> SessionProjection {
    let (active_turn_id, streaming) = project_live_turn(state, active_requests);
    // TASK-69 seam 1: a settled fetch establishes an authoritative Fresh view; a
    // degrade publishes Unavailable that preserves same-session live cards and
    // seeds the warning from the daemon's authoritative pending ids ∪ what the
    // operator was already shown — so a pre-load gate stays visible even on a
    // first load with an empty prior view, and an in-flight live card is not lost.
    let permission = match settle {
        SnapshotSettle::Fresh(mut cards) => {
            mark_cards_actionable(&mut cards, authority);
            PermissionView::fresh(cards)
        }
        SnapshotSettle::Degraded(reason) => {
            // Fold the daemon's detail ids EXCEPT those tombstoned as resolved, so
            // a gate decided during the load cannot resurrect while a genuinely-
            // open, non-decided detail id stays visible (seam 5).
            let trusted_detail_ids: Vec<String> = detail_pending_ids
                .iter()
                .filter(|id| !resolved_ids.iter().any(|resolved| resolved == *id))
                .cloned()
                .collect();
            PermissionView::degraded_preserving(reason, prior_view, &trusted_detail_ids)
        }
    };
    SessionProjection {
        active_turn_id,
        streaming,
        permission,
    }
}

/// The post-await permission commit, driving the live SIGNALS — the exact
/// plumbing `commit_session_projection` runs after both fetches. It reads the
/// prior view and the decision tombstones FROM their signals (so the tombstones
/// are the ones `apply_control_event`/`decide_permission` actually populated, not
/// a precomputed literal), folds `detail_ids − tombstones` via
/// [`build_session_projection`], publishes the resulting view, and — only on an
/// authoritative Fresh — clears the tombstones (they are now absorbed). Returned
/// so the caller can read `active_turn_id`/`streaming`/cards for the rest of the
/// atomic commit. Driven in tests via `apply_session_projection` over a
/// `Daemon::dummy()` with the tombstones populated by the real
/// `apply_control_event`/`decide_permission` plumbing — not precomputed literals
/// (codex HOLD on 4cfb5c2).
#[allow(clippy::too_many_arguments)]
fn commit_permission_view(
    permission_view: RwSignal<PermissionView>,
    tombstones: RwSignal<Vec<String>>,
    epoch: RwSignal<u64>,
    epoch_claim: u64,
    state: Option<SessionRunState>,
    active_requests: &[String],
    settle: SnapshotSettle,
    detail_pending_ids: &[String],
    authority: &HashMap<String, DecisionGrant>,
) -> SessionProjection {
    let prior_view = permission_view.get_untracked();
    let resolved_ids = tombstones.get_untracked();
    let projection = build_session_projection(
        state,
        active_requests,
        settle,
        detail_pending_ids,
        &prior_view,
        &resolved_ids,
        authority,
    );
    publish_permission_view(
        permission_view,
        tombstones,
        epoch,
        epoch_claim,
        projection.permission.clone(),
    );
    projection
}

/// The single place any settled permission view reaches the live signals — used
/// by BOTH the session-load commit (`commit_permission_view`) and the standalone
/// reconciler (`DaemonPermissionSnapshotSource`), so the two paths share one
/// lifecycle (codex HOLD on c2a1810): publish the view, and — only on an
/// authoritative Fresh — clear the decision tombstones.
///
/// TASK-69 (codex HOLD on 8b41fee1): the publish is EPOCH-GATED. `epoch_claim`
/// is the ticket this caller took when it started; if a newer permission op has
/// since claimed a later epoch (`epoch` advanced past `epoch_claim`), this
/// publisher is stale and is SKIPPED — it must not overwrite the newer
/// authoritative view or clear a tombstone the newer op depends on. Returns
/// whether it published.
fn publish_permission_view(
    permission_view: RwSignal<PermissionView>,
    tombstones: RwSignal<Vec<String>>,
    epoch: RwSignal<u64>,
    epoch_claim: u64,
    view: PermissionView,
) -> bool {
    // Only the latest claimant may publish. A stale claim (superseded by a newer
    // permission op) is dropped; the caller's transcript projection still commits.
    if epoch_claim != epoch.get_untracked() {
        return false;
    }
    let clears_tombstones = !view.is_unavailable();
    permission_view.set(view);
    if clears_tombstones {
        tombstones.set(Vec::new());
    }
    true
}

/// The live signals `apply_session_projection` reads and writes — the signal half
/// of `commit_session_projection`. Grouped so the post-fetch commit body can be a
/// free function driven in a native test over a `Daemon::dummy()` (via
/// `Daemon::commit_signals`) with injected fetch results (codex HOLD on c2a1810:
/// the regressions must drive the production wrapper). `RwSignal` is `Copy`, so
/// this struct is too.
#[derive(Clone, Copy)]
struct SessionCommitSignals {
    session_id: RwSignal<Option<String>>,
    sse_generation: RwSignal<u64>,
    activity_revision: RwSignal<u64>,
    streaming: RwSignal<bool>,
    active_turn_id: RwSignal<Option<String>>,
    decision_authority: RwSignal<HashMap<String, DecisionGrant>>,
    permission_view: RwSignal<PermissionView>,
    permission_tombstones: RwSignal<Vec<String>>,
    permission_epoch: RwSignal<u64>,
    session_title: RwSignal<String>,
    cwd: RwSignal<String>,
    model: RwSignal<Option<String>>,
    turns: RwSignal<Vec<Turn>>,
    pinned_widgets: RwSignal<Vec<PinnedWidget>>,
    status: RwSignal<String>,
}

/// The post-fetch body of `commit_session_projection`: the final admission
/// recheck, the live-state decision, the full transcript rebuild,
/// and the one atomic (no-`await`) commit of every session signal — including the
/// two-state permission publish. `detail` and `settle` are the two fetch RESULTS,
/// injected so this whole wrapper runs in a native test (codex HOLD on c2a1810:
/// the regressions must drive the production wrapper — transcript + admission +
/// permission — not just the permission helper). `commit_session_projection`
/// performs the two HTTP awaits, then calls this.
fn apply_session_projection(
    sig: &SessionCommitSignals,
    id: &str,
    generation: u64,
    snapshot_activity_revision: u64,
    permission_epoch_claim: u64,
    detail: SessionDetail,
    settle: SnapshotSettle,
) -> Result<ProjectionOutcome, String> {
    // 3. Re-admit after the second await: focus/generation may have moved.
    if admit_session_snapshot(
        id,
        generation,
        sig.session_id.get_untracked().as_deref(),
        sig.sse_generation.get_untracked(),
    ) == SnapshotAdmission::Discard
    {
        return Err("session snapshot retired before commit".into());
    }
    if admit_activity_snapshot(
        id,
        generation,
        snapshot_activity_revision,
        sig.session_id.get_untracked().as_deref(),
        sig.sse_generation.get_untracked(),
        sig.activity_revision.get_untracked(),
    ) == SnapshotAdmission::Discard
    {
        return Err(SESSION_PROJECTION_ACTIVITY_CHANGED.into());
    }

    // 4. Determine whether the daemon reports a live turn. A terminal snapshot
    // can briefly race an already-admitted TurnStarted frame, so remember that
    // local Stop target for the atomic commit below.
    let query_is_live = detail.state.is_some_and(|s| s.is_live());
    let local_has_live_turn =
        sig.streaming.get_untracked() && sig.active_turn_id.get_untracked().is_some();
    let preserve_local_live_state =
        should_preserve_local_live_state(query_is_live, local_has_live_turn);
    let local_active_turn_id = sig.active_turn_id.get_untracked();
    let local_streaming = sig.streaming.get_untracked();

    // 5. Pure projection, then one atomic commit — no `await` between writes.
    let SessionDetail {
        title,
        model,
        workspace_root,
        cwd,
        transcript,
        tool_context,
        state,
        active_requests,
        // The daemon's authoritative pending-permission ids for this session; the
        // only proof of a pre-load gate when the rich card snapshot degrades.
        pending_permissions: detail_pending_ids,
        ..
    } = detail;

    let (rebuilt_turns, rebuilt_pinned) = turns_from_session_transcript(transcript, &tool_context);
    let authority = sig.decision_authority.get_untracked();
    // Publish the two-state permission view through the shared signal seam: reads
    // prior view + tombstones from their signals, folds `detail_ids − tombstones`,
    // and publishes UNDER THE EPOCH CLAIM — if a newer permission op (a concurrent
    // standalone reconcile) has since claimed a later epoch, this stale publish is
    // SKIPPED so it cannot overwrite the newer authoritative view. No `await`, so a
    // decision/request that interleaved either fetch await is reflected.
    let projection = commit_permission_view(
        sig.permission_view,
        sig.permission_tombstones,
        sig.permission_epoch,
        permission_epoch_claim,
        state,
        &active_requests,
        settle,
        &detail_pending_ids,
        &authority,
    );
    // Live set = daemon live requests plus any request still holding an open gate.
    // Read from the CURRENT permission view (which may be the newer view a
    // concurrent publisher committed, if our permission publish was skipped).
    let mut live_requests: HashSet<String> = active_requests.iter().cloned().collect();
    sig.permission_view.with_untracked(|view| {
        for card in view.cards() {
            if let Some(request_id) = &card.request_id {
                live_requests.insert(request_id.clone());
            }
        }
    });

    sig.session_title.set(title);
    if let Some(root) = workspace_root.or(cwd) {
        if !root.is_empty() {
            sig.cwd.set(root);
        }
    }
    if !model.is_empty() {
        sig.model.set(Some(model));
    }
    sig.turns.set(rebuilt_turns);
    sig.pinned_widgets.set(rebuilt_pinned);
    if preserve_local_live_state {
        sig.active_turn_id.set(local_active_turn_id);
        sig.streaming.set(local_streaming);
    } else {
        sig.active_turn_id.set(projection.active_turn_id);
        sig.streaming.set(projection.streaming);
    }
    // permission_view + tombstones already published by commit_permission_view
    // (or skipped if a newer epoch superseded this claim).
    sig.decision_authority.update(|authority| {
        prune_session_authority(authority, id, &live_requests);
    });
    sig.status.set("session loaded".into());
    Ok(ProjectionOutcome::Terminal)
}

/// Build the `args_preview` stored on a `ToolCall` block.
///
/// Non-browser tools are truncated to 60 chars for a compact raw-args glance
/// (a bash/write call can carry huge args). Browser tools are kept WHOLE: the
/// cockpit's `deck::browser::summary_from_args` PARSES this string as JSON to
/// extract `url`/`selector`/`text`, and a mid-JSON truncation makes it
/// unparseable — degrading every real browser action (a full URL or
/// selector+text easily exceeds 60 chars) to a useless `"?"` (TASK-98). Browser
/// args are small and structured, so storing them whole is cheap and keeps the
/// summary accurate.
fn tool_args_preview(name: &str, args_json: &Value) -> String {
    let args = serde_json::to_string(args_json).unwrap_or_else(|_| "{}".into());
    if name.starts_with("browser_") {
        args
    } else {
        args.chars().take(60).collect()
    }
}

/// Mutate the turns vec in response to a single SSE event. Splits assistant
/// content into Text / Thinking / ToolCall blocks under one Turn per turn_id,
/// matching the TUI's `pm_*_assistant_turn_mut` logic.
#[allow(clippy::too_many_arguments)]
fn apply_event(
    event: &AgentEvent,
    turns: RwSignal<Vec<Turn>>,
    session_id: RwSignal<Option<String>>,
    streaming: RwSignal<bool>,
    status: RwSignal<String>,
    status_detail: RwSignal<Option<(String, String)>>,
    last_turn_tokens: RwSignal<Option<TokenStats>>,
    session_tokens: RwSignal<TokenStats>,
    model: RwSignal<Option<String>>,
    active_turn_id: RwSignal<Option<String>>,
    browser_active: RwSignal<bool>,
    browser_last_action: RwSignal<Option<String>>,
    canvas_patches: RwSignal<Vec<CanvasPatchEntry>>,
    pinned_widgets: RwSignal<Vec<PinnedWidget>>,
    awaiting_session_adoption: RwSignal<bool>,
    // TASK-46: sync_pending suppression gate — when true, content-mutating events
    // are suppressed but state/Stop/permissions/terminal detection still flow.
    sync_pending: RwSignal<bool>,
    // TASK-46: kicked by TurnFinished during sync_pending so the poll fires an
    // immediate detail refetch.
    sync_poll_kick: RwSignal<u64>,
    // TASK-46: bumped by admitted TurnStarted and TurnFinished; captured at
    // projection start so stale terminal detail cannot clear a new live turn
    // and stale live detail cannot clear a just-finished Stop target.
    activity_revision: RwSignal<u64>,
    // Header-bound session identity (OCEAN-236). A `session_created` frame is the
    // authoritative source for the live session's title and working directory; the
    // header already renders both signals, so threading them here stops the surface
    // from silently dropping the parsed `title`/`cwd` and showing a stale anchor
    // (the `cwd="/"` confusion) for sessions adopted purely over the event stream.
    session_title: RwSignal<String>,
    cwd: RwSignal<String>,
) {
    let Some(evt_sid) = event.session_id() else {
        log::warn!("dropping unscoped agent event before reducer");
        return;
    };
    if session_id.get_untracked().as_deref() != Some(evt_sid) {
        log::warn!("dropping agent event for non-active session {evt_sid}");
        return;
    }

    // Retired TASK-46 compatibility guard. New session projections never enable
    // sync_pending, but an already-running legacy poll still suppresses content
    // until its replacement bundle reloads. State/Stop/permissions/SurfacePatch
    // continue to flow.
    let sp = sync_pending.get_untracked();
    if sp {
        match event {
            // Content-mutating events — suppress; the poll will atomically
            // replace when the turn becomes terminal.
            AgentEvent::AssistantTextDelta { .. }
            | AgentEvent::ThinkingDelta { .. }
            | AgentEvent::ToolCallStarted { .. }
            | AgentEvent::ToolCallChunk { .. }
            | AgentEvent::ToolCallFinished { .. }
            | AgentEvent::ComponentRender { .. }
            | AgentEvent::ComponentUnmount { .. } => return,
            // State events still flow through.
            _ => {}
        }
    }

    match event {
        AgentEvent::SessionCreated {
            title, cwd: dir, ..
        } => {
            awaiting_session_adoption.set(false);
            // Reflect the daemon's authoritative session identity in the header.
            // Guard each field: a `session_created` frame can omit them (serde
            // default → empty), and clobbering a good local value (set when this
            // client created the session over REST) with an empty one would blank
            // the header. Non-empty wins.
            if !title.trim().is_empty() {
                session_title.set(title.clone());
            }
            if !dir.trim().is_empty() {
                cwd.set(dir.clone());
            }
            // Mirror the title into the browser tab so the OS-level window/tab
            // label tracks the live session too.
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(window) = web_sys::window() {
                    if let Some(doc) = window.document() {
                        doc.set_title(&format!("Ocean — {title}"));
                    }
                }
            }
        }
        AgentEvent::TurnStarted {
            turn_id, model: m, ..
        } => {
            awaiting_session_adoption.set(false);
            // Track the in-flight turn so the halt button can cancel it, and
            // reflect the live model (covers a mid-session swap).
            active_turn_id.set(Some(turn_id.clone()));
            streaming.set(true);
            activity_revision.update(|r| *r = r.wrapping_add(1));
            if let Some(m) = m {
                model.set(Some(m.clone()));
            }
            // Assistant turn will be lazily created on the first delta.
        }
        AgentEvent::AssistantTextDelta { turn_id, delta, .. } => {
            turns.update(|t| {
                let turn = ensure_assistant_turn(t, turn_id);
                match turn.blocks.last_mut() {
                    Some(Block::Text(buf)) => buf.push_str(delta),
                    _ => turn.blocks.push(Block::Text(delta.clone())),
                }
            });
        }
        AgentEvent::ThinkingDelta { turn_id, delta, .. } => {
            turns.update(|t| {
                let turn = ensure_assistant_turn(t, turn_id);
                match turn.blocks.last_mut() {
                    Some(Block::Thinking { content, .. }) => content.push_str(delta),
                    _ => turn.blocks.push(Block::Thinking {
                        content: delta.clone(),
                        expanded: false,
                    }),
                }
            });
        }
        AgentEvent::ToolCallStarted { turn_id, call, .. } => {
            // Mirror live browser actions onto the control indicator: any
            // `browser_*` tool call updates the "last action" label the header
            // shows next to the "driving the browser" cue (OCEAN-92).
            if call.name.starts_with("browser_") {
                browser_last_action.set(Some(call.name.clone()));
            }
            turns.update(|t| {
                let turn = ensure_assistant_turn(t, turn_id);
                let preview = tool_args_preview(&call.name, &call.args_json);
                turn.blocks.push(Block::ToolCall {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    args_preview: preview,
                    output: String::new(),
                    status: ToolStatus::Running,
                    expanded: false,
                });
            });
        }
        AgentEvent::ToolCallChunk {
            turn_id,
            call_id,
            chunk,
            ..
        } => {
            turns.update(|t| {
                let turn = ensure_assistant_turn(t, turn_id);
                for block in turn.blocks.iter_mut().rev() {
                    if let Block::ToolCall {
                        call_id: id,
                        output,
                        ..
                    } = block
                    {
                        if id == call_id {
                            output.push_str(chunk);
                            break;
                        }
                    }
                }
            });
        }
        AgentEvent::ToolCallFinished {
            turn_id,
            call_id,
            result,
            ..
        } => {
            turns.update(|t| {
                let turn = ensure_assistant_turn(t, turn_id);
                for block in turn.blocks.iter_mut().rev() {
                    if let Block::ToolCall {
                        call_id: id,
                        output,
                        status,
                        expanded,
                        ..
                    } = block
                    {
                        if id == call_id {
                            if output.is_empty() && !result.output.is_empty() {
                                output.push_str(&result.output);
                            }
                            *status = if result.ok {
                                ToolStatus::Ok
                            } else {
                                ToolStatus::Err
                            };
                            // Auto-expand failed tool calls so the error output
                            // is visible instead of hidden in a collapsed drawer.
                            if *status == ToolStatus::Err {
                                *expanded = true;
                            }
                            break;
                        }
                    }
                }
            });
        }
        AgentEvent::TurnFinished {
            turn_id,
            status: turn_status,
            error,
            output_tokens,
            input_tokens,
            cache_read_tokens,
            tokens_per_second,
            ..
        } => {
            // Terminal-admission gate (TASK-45): only the finish for the
            // *active* turn may clear the live Stop target / running projection
            // and own the header status. A stale/superseded finish (a different
            // turn, or one arriving with no active turn) keeps its own scoped
            // effects below but must not blank another turn's live state.
            let admission = admit_turn_terminal(active_turn_id.get_untracked().as_deref(), turn_id);
            if admission == TerminalAdmission::ClearLive {
                streaming.set(false);
                active_turn_id.set(None);
            }
            // TASK-46: bump activity revision so stale in-flight projections
            // (fetched while this turn was still live) are rejected at commit.
            activity_revision.update(|r| *r = r.wrapping_add(1));
            // Kick the sync_poll if active — the detail poll should refetch
            // immediately on termination instead of waiting for the next 2s tick.
            if sync_pending.get_untracked() {
                sync_poll_kick.update(|k| *k = k.wrapping_add(1));
            }
            // Adoption is session-scoped, not turn-scoped: the frame is already
            // gated to the active session above, so any finish for it resolves a
            // pending adoption regardless of which turn ended.
            awaiting_session_adoption.set(false);
            // Surface a failed turn instead of silently flipping `streaming`
            // off. The daemon reports `status`/`error` on the finish frame; a
            // turn that errored or was cancelled previously just stopped with no
            // feedback, leaving the user staring at a dead composer. Mirror the
            // GPUI shell, which puts the error in its status line (OCEAN-100).
            // Daemon `AgentTurnStatus` is one of completed/failed/cancelled.
            //
            // The header status line is part of the live-turn projection, so
            // only the admitted (active-turn) finish writes it — a stale
            // failed/cancelled frame must not steal the current turn's status.
            // A stale finish still records its OWN transcript error block
            // (scoped to `turn_id`); it just cannot touch the header.
            match admission {
                TerminalAdmission::ClearLive => {
                    if let Some(err) = error {
                        surface_turn_failure(
                            turns,
                            status,
                            status_detail,
                            turn_id,
                            "turn failed",
                            err,
                        );
                    } else if turn_status != "completed" {
                        // A non-success status with no error string (e.g. "cancelled").
                        status_detail.set(None);
                        status.set(format!("turn {turn_status}"));
                    } else {
                        status.set("connected".into());
                        status_detail.set(None);
                    }
                }
                TerminalAdmission::KeepLive => {
                    if let Some(err) = error {
                        log::error!("stale turn failed: {err}");
                        append_turn_error(turns, turn_id, err);
                    }
                }
            }
            // Sweep (OCEAN-319, mirrors the TUI): a cancelled/failed turn can
            // stop mid-tool without ever emitting ToolCallFinished, leaving a
            // ToolCall stuck in Running — the group's aggregate then shows a
            // permanent "running" badge on a dead turn. Close this turn's
            // unclosed tools to Err (cancel arrives as failed, not a distinct
            // Cancelled). Scoped to the finishing turn: a blanket sweep would
            // wrongly error a sibling turn's legitimately-running tool. Leave
            // completed turns alone — a stuck Running there is a dropped event,
            // a real bug we must not mask.
            if turn_status != "completed" {
                turns.update(|t| {
                    for turn in t.iter_mut() {
                        if turn.turn_id.as_deref() != Some(turn_id.as_str()) {
                            continue;
                        }
                        for block in turn.blocks.iter_mut() {
                            if let Block::ToolCall {
                                status, expanded, ..
                            } = block
                            {
                                if *status == ToolStatus::Running {
                                    *status = ToolStatus::Err;
                                    // Match every other error path: pre-expand
                                    // the row (the group itself NEVER auto-opens)
                                    // so opening it lands on the interrupted call.
                                    *expanded = true;
                                }
                            }
                        }
                    }
                });
            }
            // Record this turn's usage (real provider numbers when present) and
            // fold it into the running session total.
            let turn_stats = TokenStats {
                input: input_tokens.unwrap_or(0),
                output: output_tokens.unwrap_or(0),
                cache_read: cache_read_tokens.unwrap_or(0),
                tokens_per_second: tokens_per_second.unwrap_or(0.0),
            };
            last_turn_tokens.set(Some(turn_stats));
            session_tokens.update(|s| {
                s.input += turn_stats.input;
                s.output += turn_stats.output;
                s.cache_read += turn_stats.cache_read;
                // Session total isn't a rate; keep tokens_per_second at 0.
            });
        }
        AgentEvent::ComponentRender {
            component_id,
            kind,
            props,
            replace,
            ..
        } => {
            // A pinned component docks into the persistent rail instead of
            // the transcript: route it to the pinned registry and skip the
            // inline block so it never duplicates. `placement` defaults to
            // inline, so every existing render is unchanged.
            if component_placement(props) == ComponentPlacement::Pinned {
                let widget = PinnedWidget {
                    component_id: component_id.clone(),
                    kind: kind.clone(),
                    props: props.clone(),
                };
                pinned_widgets.update(|pinned| upsert_pinned(pinned, widget));
                return;
            }
            turns.update(|t| {
                if *replace {
                    // Replace existing component with same id.
                    for turn in t.iter_mut() {
                        for block in turn.blocks.iter_mut() {
                            if let Block::Component {
                                component_id: id, ..
                            } = block
                            {
                                if id == component_id {
                                    *block = Block::Component {
                                        component_id: component_id.clone(),
                                        kind: kind.clone(),
                                        props: props.clone(),
                                    };
                                    return;
                                }
                            }
                        }
                    }
                }
                // Attach to the CURRENT assistant turn (component events carry no
                // turn_id), not a fresh synthetic turn each time.
                let turn = ensure_component_turn(t);
                turn.blocks.push(Block::Component {
                    component_id: component_id.clone(),
                    kind: kind.clone(),
                    props: props.clone(),
                });
            });
        }
        AgentEvent::ComponentUnmount { component_id, .. } => {
            turns.update(|t| {
                for turn in t.iter_mut() {
                    turn.blocks.retain(|block| match block {
                        Block::Component {
                            component_id: id, ..
                        } => id != component_id,
                        _ => true,
                    });
                }
                // Remove empty turns.
                t.retain(|turn| !turn.blocks.is_empty());
            });
            // A pinned widget unmounts from the rail, not the transcript.
            pinned_widgets.update(|pinned| pinned.retain(|w| w.component_id != *component_id));
        }
        AgentEvent::BrowserActivity { active, .. } => {
            browser_active.set(*active);
            // In the extension side-panel context, focus pulls the cockpit
            // forward so the conversation visibly "follows" the browser work.
            // In a normal tab this is a harmless no-op.
            if *active {
                if let Some(win) = web_sys::window() {
                    let _ = win.focus();
                }
            }
        }
        AgentEvent::SurfacePatch {
            canvas_id, patches, ..
        } => {
            // The agent applied canvas patches this turn (OCEAN-178). The GPUI
            // native shell replays each envelope through a full `CanvasLedger`;
            // the web surface doesn't have that ledger/renderer yet, so we record
            // each patch into `canvas_patches` and render a basic representation.
            // The point of this ticket is that these frames are no longer dropped
            // at the transport layer — the data is now visible on the web surface.
            canvas_patches.update(|entries| {
                for envelope in patches {
                    entries.push(CanvasPatchEntry {
                        canvas_id: canvas_id.as_str().to_string(),
                        summary: summarize_surface_patch(&envelope.patch),
                        envelope: envelope.clone(),
                    });
                }
                // Keep the ledger bounded so a long, patch-heavy session can't
                // grow it without limit.
                let len = entries.len();
                if len > MAX_CANVAS_PATCHES {
                    entries.drain(0..len - MAX_CANVAS_PATCHES);
                }
            });
        }
        AgentEvent::Extension { extension, .. } => {
            // Extension frames carry no transcript state, so there is nothing
            // to reduce into turns — log and move on. Longhouse (council)
            // state is read via the daemon's durable `GET /v1/longhouse/topics`
            // snapshot (polled by the council deck), not from this stream.
            log::debug!("ignoring extension event: {extension}");
        }
        AgentEvent::Other => {
            // An event whose `type` tag this surface doesn't model fell into the
            // serde catch-all. We can't render it, but log it so a newly-added
            // daemon event type is visible in the console instead of vanishing
            // silently (OCEAN-100).
            log::debug!("ignoring unrecognized agent event type");
        }
    }
}

async fn await_event_source_open(source: &EventSource) -> bool {
    for _ in 0..200 {
        match source.state() {
            EventSourceState::Open => return true,
            EventSourceState::Closed => return false,
            EventSourceState::Connecting => {
                gloo_timers::future::TimeoutFuture::new(25).await;
            }
        }
    }
    false
}

fn stream_marker_for_state(
    state: EventSourceState,
    session_id: &str,
    generation: u64,
) -> Option<(String, u64)> {
    (state == EventSourceState::Open).then(|| (session_id.to_string(), generation))
}

fn set_stream_source(
    sources: &Rc<RefCell<PlannerStreamSources>>,
    session_id: &str,
    generation: u64,
    permission: bool,
    source: Option<EventSource>,
) {
    let mut sources = sources.borrow_mut();
    if sources.session_id != session_id || sources.generation != generation {
        return;
    }
    let state = source
        .as_ref()
        .map(EventSource::state)
        .unwrap_or(EventSourceState::Closed);
    let kind = if permission {
        sources.permission = source;
        PlannerStreamKind::Permission
    } else {
        sources.agent = source;
        PlannerStreamKind::Agent
    };
    sources.lifecycle.transition(kind, state);
}

async fn monitor_stream_open(
    sources: send_wrapper::SendWrapper<Rc<RefCell<PlannerStreamSources>>>,
    marker: RwSignal<Option<(String, u64)>>,
    session_id: String,
    generation: u64,
    permission: bool,
) {
    loop {
        gloo_timers::future::TimeoutFuture::new(25).await;
        let state = {
            let sources = sources.borrow();
            if sources.session_id != session_id || sources.generation != generation {
                return;
            }
            let source = if permission {
                sources.permission.as_ref()
            } else {
                sources.agent.as_ref()
            };
            source.map(EventSource::state)
        };
        if state == Some(EventSourceState::Open) {
            continue;
        }
        clear_stream_marker(marker, &session_id, generation);
        set_stream_source(&sources, &session_id, generation, permission, None);
        return;
    }
}

fn planner_sources_open(sources: &PlannerStreamSources, session_id: &str, generation: u64) -> bool {
    sources.session_id == session_id
        && sources.generation == generation
        && sources.lifecycle.ready()
        && sources
            .agent
            .as_ref()
            .is_some_and(|source| source.state() == EventSourceState::Open)
        && sources
            .permission
            .as_ref()
            .is_some_and(|source| source.state() == EventSourceState::Open)
}

fn clear_stream_marker(marker: RwSignal<Option<(String, u64)>>, session_id: &str, generation: u64) {
    if marker.with_untracked(|value| {
        value
            .as_ref()
            .is_some_and(|(id, current)| id == session_id && *current == generation)
    }) {
        marker.set(None);
    }
}

trait PermissionSnapshotSource {
    fn revision(&self) -> u64;
    fn fetch<'a>(&'a mut self) -> LocalBoxFuture<'a, Result<Vec<PendingPermission>, String>>;
    fn apply(&mut self, snapshot: Vec<PendingPermission>);
    /// TASK-69 seam 3: publish a visible `Unavailable` state (retaining the
    /// last-known pending ids) instead of leaving the caller to fail-open. Called
    /// when the snapshot cannot be settled.
    fn degrade(&mut self, reason: String);
}

/// TASK-69 seam 3: reconcile the durable permission registry. On a settled
/// fetch, publish the authoritative cards. On a fetch error OR revision
/// exhaustion, publish a visible `Unavailable` via [`PermissionSnapshotSource::degrade`]
/// and return `Ok` — a hard `Err` here used to bubble to callers that swallow it
/// to an empty set (a silent fail-open on a possibly-blocked session).
async fn reconcile_permission_snapshot<S: PermissionSnapshotSource>(
    source: &mut S,
) -> Result<(), String> {
    for _ in 0..4 {
        let revision = source.revision();
        match source.fetch().await {
            Ok(snapshot) => {
                if source.revision() != revision {
                    continue;
                }
                source.apply(snapshot);
                return Ok(());
            }
            Err(error) => {
                source.degrade(format!("permission refresh failed: {error}"));
                return Ok(());
            }
        }
    }
    source.degrade("permission snapshot changed repeatedly during reconciliation".into());
    Ok(())
}

/// TASK-69 seam 1: settle the auxiliary permission-card snapshot for a session
/// projection. Returns `Fresh(cards)` on a revision-stable fetch, or
/// `Degraded(reason)` on a fetch error / revision exhaustion — NEVER an `Err`, so
/// the caller commits the transcript regardless (the 63bc9ea invariant) and the
/// two-state view carries the degrade instead of silently blanking.
async fn settle_permission_snapshot<S: PermissionSnapshotSource>(source: &mut S) -> SnapshotSettle {
    for _ in 0..4 {
        let revision = source.revision();
        match source.fetch().await {
            Ok(snapshot) => {
                if source.revision() == revision {
                    return SnapshotSettle::Fresh(snapshot);
                }
            }
            Err(error) => {
                return SnapshotSettle::Degraded(format!(
                    "session projection: permission snapshot degraded: {error}"
                ));
            }
        }
    }
    SnapshotSettle::Degraded(
        "session projection: permission snapshot kept changing during load".into(),
    )
}

/// Outcome of [`settle_permission_snapshot`].
#[derive(Debug, Clone, PartialEq)]
enum SnapshotSettle {
    Fresh(Vec<PendingPermission>),
    Degraded(String),
}

/// TASK-69 seam 2: outcome of a cross-session permission ATTENTION poll (the
/// Island's `/v1/permissions` refresh, distinct from the focused-session view).
#[derive(Debug, Clone, PartialEq)]
enum PermissionPollOutcome {
    /// A settled `ok` response — authoritative. Apply the list, clear stale.
    Fresh(Vec<PermissionSnapshot>),
    /// Transport/decode error or not-ok body. The caller KEEPS the last-known
    /// list (a transient blip must not erase real attention) but marks it stale
    /// so the Island shows "couldn't refresh" rather than presenting
    /// possibly-resolved gates as confidently authoritative.
    Unavailable(String),
}

/// Pure classifier for [`Daemon::fetch_attention`]'s permission arm (seam 2).
fn classify_permission_poll(
    result: Result<Vec<PermissionSnapshot>, String>,
) -> PermissionPollOutcome {
    match result {
        Ok(permissions) => PermissionPollOutcome::Fresh(permissions),
        Err(reason) => PermissionPollOutcome::Unavailable(reason),
    }
}

struct DaemonPermissionSnapshotSource<'a> {
    daemon: &'a Daemon,
    session_id: &'a str,
    /// The permission-projection epoch claimed when this reconcile started; its
    /// `apply`/`degrade` publish under it, so a stale reconcile cannot overwrite a
    /// newer publisher (codex HOLD on 8b41fee1). Fetch-only sources (the
    /// session-load settle) pass 0 — they never publish, so it is unused.
    epoch_claim: u64,
}

impl PermissionSnapshotSource for DaemonPermissionSnapshotSource<'_> {
    fn revision(&self) -> u64 {
        self.daemon.permission_revision.get_untracked()
    }

    fn fetch<'a>(&'a mut self) -> LocalBoxFuture<'a, Result<Vec<PendingPermission>, String>> {
        async move {
            let url = self.daemon.url.get_untracked();
            let get_url = format!("{}/v1/permissions", url.trim_end_matches('/'));
            let resp = Request::get(&get_url)
                .send()
                .await
                .map_err(|error| format!("permission snapshot failed: {error}"))?;
            if !resp.ok() {
                let text = resp.text().await.unwrap_or_default();
                return Err(format!(
                    "permission snapshot rejected: {}",
                    concise_error(&text)
                ));
            }
            let response = resp
                .json::<PermissionsResponse>()
                .await
                .map_err(|error| format!("permission snapshot decode failed: {error}"))?;
            if !response.ok {
                return Err(response
                    .error
                    .unwrap_or_else(|| "permission snapshot failed".into()));
            }
            if self.daemon.session_id.get_untracked().as_deref() != Some(self.session_id) {
                return Err("session changed during permission snapshot".into());
            }
            Ok(response
                .permissions
                .into_iter()
                .filter(|permission| permission.session_id.as_deref() == Some(self.session_id))
                .map(|permission| PendingPermission {
                    permission_id: permission.permission_id,
                    session_id: self.session_id.to_string(),
                    tool: permission.tool,
                    reason: permission.reason,
                    args_summary: summarize_args(&permission.args),
                    deciding: false,
                    // Carry the originating request id so decision authority can
                    // be resolved per card; `actionable` is stamped on apply from
                    // the local authority map (TASK-44, finding 4).
                    request_id: permission.request_id,
                    actionable: false,
                })
                .collect())
        }
        .boxed_local()
    }

    fn apply(&mut self, mut snapshot: Vec<PendingPermission>) {
        // Stamp per-card actionability from this surface's decision authority
        // before publishing, so a card raised by another surface renders
        // read-only rather than sending an unbound token.
        let authority = self.daemon.decision_authority.get_untracked();
        mark_cards_actionable(&mut snapshot, &authority);
        // A settled fetch is authoritative: this establishes Fresh, clears any
        // prior degradation, AND absorbs the decision tombstones — same lifecycle
        // as the session-load commit (codex HOLD on c2a1810: a stable reconcile
        // Fresh must also clear tombstones, not only commit_permission_view).
        publish_permission_view(
            self.daemon.permission_view,
            self.daemon.permission_tombstones,
            self.daemon.permission_epoch,
            self.epoch_claim,
            PermissionView::fresh(snapshot),
        );
    }

    fn degrade(&mut self, reason: String) {
        // Only publish for the still-focused session — a degrade for a session
        // we've since left must not stamp a warning over the new focus.
        if self.daemon.session_id.get_untracked().as_deref() != Some(self.session_id) {
            return;
        }
        // PRESERVE already-admitted live cards, exactly as the session-load
        // degrade does (codex HOLD on c2a1810: a failed reconcile after a live
        // card was admitted must not wipe it into an id-only warning). No fresh
        // detail ids on this path — the prior view's live cards + known ids carry.
        let prior_view = self.daemon.permission_view.get_untracked();
        let view = PermissionView::degraded_preserving(reason, &prior_view, &[]);
        publish_permission_view(
            self.daemon.permission_view,
            self.daemon.permission_tombstones,
            self.daemon.permission_epoch,
            self.epoch_claim,
            view,
        );
    }
}

fn planner_prompt_already_echoed(turns: &[Turn], markdown: &str) -> bool {
    turns.iter().any(|turn| {
        turn.role == Role::User
            && turn
                .blocks
                .iter()
                .any(|block| matches!(block, Block::Text(text) if text == markdown))
    })
}

fn planner_streams_ready(
    session_id: &str,
    generation: u64,
    agent: Option<&(String, u64)>,
    permission: Option<&(String, u64)>,
) -> bool {
    let expected = &(session_id.to_string(), generation);
    agent == Some(expected) && permission == Some(expected)
}

/// Apply one control-stream frame to the pending-permission queue, scoped to the
/// active session. A `permission_request` enqueues a card (deduped by id); a
/// `permission_decision` removes the matching card (the daemon decided it,
/// possibly from another surface like the TUI). Frames for other sessions, or
/// without a `permission_id`, are dropped.
///
/// The enqueued card carries the envelope's `request_id` and is stamped
/// actionable only when `authority` holds the token for that request — a card
/// from another surface renders read-only (TASK-44, finding 4).
fn apply_control_event(
    event: &ControlEvent,
    active_session_id: &str,
    pending: RwSignal<PermissionView>,
    tombstones: RwSignal<Vec<String>>,
    authority: &HashMap<String, DecisionGrant>,
) {
    match event {
        ControlEvent::PermissionRequest {
            permission_id,
            session_id,
            request_id,
            tool,
            reason,
            args,
        } => {
            // Hard session isolation, matching the agent stream: a frame must
            // carry exactly the active session id, else drop it.
            if session_id.as_deref() != Some(active_session_id) {
                return;
            }
            let Some(permission_id) = permission_id.clone() else {
                return;
            };
            // A live (re)request for this id makes it pending again, so clear any
            // decision tombstone — a legitimately reused id must not stay
            // suppressed on a later degrade (codex HOLD on 4cfb5c2).
            tombstones.update(|ids| ids.retain(|id| id != &permission_id));
            let actionable =
                resolve_decision_authority(request_id.as_deref(), active_session_id, authority)
                    .is_some();
            let entry = PendingPermission {
                permission_id: permission_id.clone(),
                session_id: active_session_id.to_string(),
                tool: tool.clone(),
                reason: reason.clone(),
                args_summary: summarize_args(args),
                deciding: false,
                request_id: request_id.clone(),
                actionable,
            };
            // TASK-69 seam 4: merge into the view. Under `Unavailable` this
            // re-materializes a real, actionable card and drops its id from the
            // known-pending warning set, without prematurely clearing the
            // degradation (only a settled snapshot may do that).
            pending.update(|view| view.ingest_request(entry));
        }
        ControlEvent::PermissionDecision {
            permission_id,
            session_id,
        } => {
            if session_id.as_deref() != Some(active_session_id) {
                return;
            }
            if let Some(id) = permission_id {
                remove_pending_permission(pending, id);
                // Tombstone the decided id so a stale `SessionDetail.pending_permissions`
                // entry cannot resurrect it if a session-load snapshot degrades.
                tombstone_permission(tombstones, id);
            }
        }
        ControlEvent::Other => {}
    }
}

/// Record a decided permission id as a tombstone (dedup), so a stale detail id
/// cannot resurrect a resolved gate on a degraded session-load projection.
fn tombstone_permission(tombstones: RwSignal<Vec<String>>, permission_id: &str) {
    tombstones.update(|ids| {
        if !ids.iter().any(|id| id == permission_id) {
            ids.push(permission_id.to_string());
        }
    });
}

/// Remove a pending permission by id (decision recorded or daemon-resolved).
/// TASK-69 seam 5: drops the id from both live cards AND any `known_pending_ids`
/// so a decided gate cannot resurrect through the degrade warning.
fn remove_pending_permission(pending: RwSignal<PermissionView>, permission_id: &str) {
    pending.update(|view| view.resolve(permission_id));
}

/// Clear the `deciding` flag on a card (a decision POST failed; re-enable it).
fn clear_pending_deciding(pending: RwSignal<PermissionView>, permission_id: &str) {
    pending.update(|view| view.set_deciding(permission_id, false));
}

/// The outcome of a permission-decision POST, applied to the local signals by
/// [`apply_permission_decision_completion`].
enum DecisionCompletion {
    /// The daemon accepted the decision — remove + tombstone the card, bump the
    /// revision, and report allow/deny.
    Succeeded { allow: bool },
    /// The POST could not be encoded / was rejected / errored — re-enable the
    /// card and report the message.
    Failed { message: String },
}

/// Apply a permission-decision completion to the local signals, but ONLY while
/// the EXACT card instance we decided on is still present — same `permission_id`,
/// `session_id` AND `request_id`. A stale completion — the operator
/// switched/retired the session, OR the id was legitimately reused for a NEW
/// request in the same session, while the POST was in flight — must write
/// NOTHING; the daemon result / control frame remains authoritative (codex HOLD
/// on 9c584d0 + the 06:23 same-session-reuse pin). Deliberately NOT epoch-gated:
/// an ordinary snapshot claim must not suppress a same-session, same-request
/// decision. Returns whether it applied.
#[allow(clippy::too_many_arguments)]
fn apply_permission_decision_completion(
    pending: RwSignal<PermissionView>,
    tombstones: RwSignal<Vec<String>>,
    permission_revision: RwSignal<u64>,
    status: RwSignal<String>,
    permission_id: &str,
    card_session: &str,
    card_request: Option<&str>,
    completion: DecisionCompletion,
) -> bool {
    // Target the exact card instance, not just the session: session + request id
    // together distinguish the decided card from a same-id card reused for a new
    // request, and from a same-id card in a different (switched-in) session.
    let still_targets = pending.with_untracked(|view| {
        view.cards().iter().any(|card| {
            card.permission_id == permission_id
                && card.session_id == card_session
                && card.request_id.as_deref() == card_request
        })
    });
    if !still_targets {
        return false;
    }
    match completion {
        DecisionCompletion::Succeeded { allow } => {
            permission_revision.update(|revision| *revision = revision.wrapping_add(1));
            remove_pending_permission(pending, permission_id);
            tombstone_permission(tombstones, permission_id);
            status.set(if allow {
                "permission allowed".into()
            } else {
                "permission denied".into()
            });
        }
        DecisionCompletion::Failed { message } => {
            status.set(message);
            clear_pending_deciding(pending, permission_id);
        }
    }
    true
}

/// Mint a per-turn CSPRNG decision token using `window.crypto.getRandomValues`
/// (OCEAN-185 / OCEAN-314). Generates 32 random bytes and encodes them as 64
/// lowercase hex chars — matching the entropy level of
/// `ocean_core::mint_decision_token` (two v4 UUIDs concatenated).
///
/// `Uint8Array` lives in `js-sys` (an existing dependency); `Crypto` is
/// exposed via `web-sys` with the `"Crypto"` feature. Falls back to a
/// recognisable sentinel if the browser Crypto API is unavailable (unreachable
/// in any supported browser, but avoids a panic).
fn mint_decision_token() -> String {
    let crypto = web_sys::window().and_then(|w| w.crypto().ok());
    let Some(crypto) = crypto else {
        log::error!("OCEAN-314: window.crypto unavailable — decision token not minted");
        return "CRYPTO_UNAVAILABLE".to_string();
    };

    let buf = js_sys::Uint8Array::new_with_length(32);
    if crypto
        .get_random_values_with_array_buffer_view(&buf)
        .is_err()
    {
        log::error!("OCEAN-314: getRandomValues failed — decision token not minted");
        return "GETRANDOMVALUES_FAILED".to_string();
    }

    buf.to_vec()
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Upper bound on the per-session canvas-patch ledger (OCEAN-178). Oldest
/// entries are dropped past this so a long, patch-heavy session stays bounded.
const MAX_CANVAS_PATCHES: usize = 512;

/// A one-line, human-readable summary of a single surface patch op, for the
/// basic web canvas representation (OCEAN-178). Mirrors the daemon's op names.
fn summarize_surface_patch(patch: &SurfacePatch) -> String {
    match patch {
        SurfacePatch::UpsertComponent { component } => {
            format!(
                "upsert_component {} ({})",
                component.id.as_str(),
                component.kind
            )
        }
        SurfacePatch::MoveComponent { component_id, x, y } => {
            format!("move_component {} → ({x}, {y})", component_id.as_str())
        }
        SurfacePatch::ResizeComponent {
            component_id,
            width,
            height,
        } => format!(
            "resize_component {} → {width}×{height}",
            component_id.as_str()
        ),
        SurfacePatch::DeleteComponent { component_id } => {
            format!("delete_component {}", component_id.as_str())
        }
        SurfacePatch::Connect { edge } => format!(
            "connect {} ({} → {})",
            edge.id.as_str(),
            edge.from.component_id.as_str(),
            edge.to.component_id.as_str()
        ),
        SurfacePatch::Disconnect { edge_id } => format!("disconnect {}", edge_id.as_str()),
        SurfacePatch::Focus { .. } => "focus".to_string(),
        SurfacePatch::Select { ids } => format!("select {} component(s)", ids.len()),
        SurfacePatch::SetViewport { .. } => "set_viewport".to_string(),
        SurfacePatch::Layout { .. } => "layout".to_string(),
        SurfacePatch::Group { frame_id, children } => {
            format!("group {} ({} children)", frame_id.as_str(), children.len())
        }
    }
}

/// Render the tool's args JSON into a compact, human-readable summary for the
/// approval card. Objects render as `key: value` lines; everything else is
/// pretty-printed. Kept short so the card stays scannable.
fn summarize_args(args: &Value) -> String {
    match args {
        Value::Null => String::new(),
        Value::Object(map) if !map.is_empty() => map
            .iter()
            .map(|(k, v)| {
                let val = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                format!("{k}: {val}")
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

/// Append an expanded error tool block to the transcript for a failed turn.
/// Attaches to the existing assistant turn for `turn_id` when present.
fn append_turn_error(turns: RwSignal<Vec<Turn>>, turn_id: &str, raw_err: &str) {
    let err = raw_err.to_string();
    turns.update(|t| {
        let turn = ensure_assistant_turn(t, turn_id);
        if let Some(Block::ToolCall {
            output,
            status: ToolStatus::Err,
            expanded,
            ..
        }) = turn
            .blocks
            .iter_mut()
            .find(|b| matches!(b, Block::ToolCall { name, .. } if name == "assistant_error"))
        {
            *output = err;
            *expanded = true;
            return;
        }
        turn.blocks.push(Block::ToolCall {
            call_id: format!("turn-error-{turn_id}"),
            name: "assistant_error".into(),
            args_preview: String::new(),
            output: err,
            status: ToolStatus::Err,
            expanded: true,
        });
    });
}

/// Concise header chip + tooltip detail + transcript Err block for turn failures.
///
/// The detail is stored PAIRED with the exact concise status string it belongs
/// to; the view only shows the tooltip while the displayed status equals the
/// stored one, so any later `status.set` (error or benign) that bypasses this
/// helper invalidates the tooltip instead of leaking a stale payload.
fn surface_turn_failure(
    turns: RwSignal<Vec<Turn>>,
    status: RwSignal<String>,
    status_detail: RwSignal<Option<(String, String)>>,
    turn_id: &str,
    prefix: &str,
    raw: &str,
) {
    log::error!("{prefix}: {raw}");
    let concise = format!("{prefix}: {}", concise_error(raw));
    status.set(concise.clone());
    status_detail.set(Some((concise, raw.to_string())));
    append_turn_error(turns, turn_id, raw);
}

/// Collapse a raw error payload into one short *reason* for the header status
/// chip, returning ONLY the reason — call sites re-apply the status prefix
/// (e.g. `format!("turn failed: {}", concise_error(&raw))`).
///
/// Provider failures arrive as a multi-line blob with a JSON payload embedded
/// in prose, e.g.
///   `provider error: provider returned an error (status 401):
///    { "error": { "message": "Provided authentication token is expired..." } }`
/// The chip's register is `turn failed: <concise reason>` (design doc §3), so
/// the provider plumbing must NOT eat the ~120-char budget — we extract the
/// JSON `error.message` as the reason. With no JSON we take the first line.
/// The full raw payload is logged via `log::error!` at each call site and
/// rendered as an Err block in the transcript; this helper only shapes the
/// chip text: (1) extract `error.message` if a JSON object is embedded, else
/// the first line; (2) collapse whitespace; (3) hard-cap at ~120 chars with a
/// trailing ellipsis.
fn concise_error(err: &str) -> String {
    // (1) Lift the provider message out of an embedded JSON object. The blob is
    //     wrapped in prose like `... (status 401): { "error": { ... } }`, so
    //     parse the substring from the first '{' to the last '}' and read
    //     `error.message` (falling back to a top-level `message`).
    let extracted: Option<String> = (|| {
        let start = err.find('{')?;
        let end = err.rfind('}')?;
        if end <= start {
            return None;
        }
        let value = serde_json::from_str::<Value>(&err[start..=end]).ok()?;
        value
            .get("error")
            .and_then(|e| e.get("message"))
            .or_else(|| value.get("message"))
            .and_then(|m| m.as_str())
            .map(str::to_string)
    })();

    // No JSON → take the first line of the raw payload as the reason.
    let first = extracted
        .as_deref()
        .unwrap_or_else(|| err.lines().next().unwrap_or(""))
        .trim_end();

    // (2) Collapse whitespace runs to single spaces.
    let collapsed: String = first.split_whitespace().collect::<Vec<_>>().join(" ");

    // (3) Hard-cap at ~120 chars with a trailing ellipsis character.
    const CAP: usize = 120;
    if collapsed.chars().count() > CAP {
        let body: String = collapsed.chars().take(CAP).collect();
        format!("{body}…")
    } else {
        collapsed
    }
}

fn turns_from_session_transcript(
    entries: Vec<SessionTranscriptEntry>,
    tool_context: &[SessionToolContext],
) -> (Vec<Turn>, Vec<PinnedWidget>) {
    // Index `component_render` tool-CALL args by tool_call_id so a `component_render`
    // tool-RESULT entry in the transcript can recover its props. The transcript
    // text for these results is only the daemon's summary ("rendered component
    // 'x'…") — the actual id/kind/props live solely in the call arguments
    // (OCEAN-382). We also note `component_unmount` calls so a persisted unmount
    // removes the component on replay, mirroring the live SSE reducer.
    let render_calls: std::collections::HashMap<&str, &SessionToolContext> = tool_context
        .iter()
        .filter(|c| {
            c.kind == "call"
                && (c.tool_name == "component_render" || c.tool_name == "component_unmount")
        })
        .map(|c| (c.tool_call_id.as_str(), c))
        .collect();

    // TASK-99: index every tool CALL's arguments by tool_call_id so a rebuilt
    // tool block can recover its `args_preview` (the live SSE reducer stores it,
    // but the transcript rebuild dropped it to empty — so a RELOADED session lost
    // every browser-action summary, which `deck::browser::summary_from_args`
    // derives by parsing that preview).
    let call_args: std::collections::HashMap<&str, &SessionToolContext> = tool_context
        .iter()
        .filter(|c| c.kind == "call")
        .map(|c| (c.tool_call_id.as_str(), c))
        .collect();

    let mut turns = Vec::new();
    let mut pinned: Vec<PinnedWidget> = Vec::new();
    for entry in entries {
        // A component_render/unmount result carries a non-empty summary text, so
        // route on the tool name BEFORE the empty-text skip below. Only replay a
        // SUCCESSFUL result, though: an errored component_render never rendered
        // live (and an errored component_unmount never removed anything), so
        // re-applying its saved args would mount a phantom component (or wrongly
        // drop one) on reconnect. For a failed result we fall through and let it
        // hydrate as the failed ToolCall block the live session showed.
        if entry.role == "tool" && !entry.is_error.unwrap_or(false) {
            if let Some(component) = entry
                .tool_call_id
                .as_deref()
                .and_then(|id| render_calls.get(id))
            {
                replay_component_call(&mut turns, &mut pinned, component);
                continue;
            }
        }

        if entry.text.trim().is_empty() && entry.tool_name.is_none() {
            continue;
        }
        match entry.role.as_str() {
            "user" => turns.push(Turn::user(entry.text)),
            "assistant" => {
                let mut turn = Turn::assistant(format!("snapshot-{}", turns.len()));
                if entry.is_error.unwrap_or(false) {
                    turn.blocks.push(Block::ToolCall {
                        call_id: format!("snapshot-error-{}", turns.len()),
                        name: "assistant_error".into(),
                        args_preview: String::new(),
                        output: entry.text,
                        status: ToolStatus::Err,
                        expanded: true,
                    });
                } else {
                    turn.blocks.push(Block::Text(entry.text));
                }
                turns.push(turn);
            }
            "tool" => {
                let mut turn = Turn::assistant(format!("snapshot-tool-{}", turns.len()));
                let is_error = entry.is_error.unwrap_or(false);
                let name = entry.tool_name.unwrap_or_else(|| "tool".into());
                // Recover the call's args (by tool_call_id) so a reloaded browser
                // action still summarizes, matching the live path (TASK-99).
                let args_preview = entry
                    .tool_call_id
                    .as_deref()
                    .and_then(|id| call_args.get(id))
                    .and_then(|call| call.arguments.as_ref())
                    .map(|args| tool_args_preview(&name, args))
                    .unwrap_or_default();
                turn.blocks.push(Block::ToolCall {
                    call_id: format!("snapshot-tool-{}", turns.len()),
                    name,
                    args_preview,
                    output: entry.text,
                    status: if is_error {
                        ToolStatus::Err
                    } else {
                        ToolStatus::Ok
                    },
                    // Failed tool calls open by default so the error is visible.
                    expanded: is_error,
                });
                turns.push(turn);
            }
            _ => {}
        }
    }
    (turns, pinned)
}

/// Fold a persisted `component_render` / `component_unmount` tool call back into
/// the rebuilt transcript, reproducing what the live SSE reducer in
/// [`apply_event`] does for `ComponentRender` / `ComponentUnmount`. This is how a
/// rendered component (map, table, workflow card) survives a mid-turn SSE
/// reconnect: its props are read from the call's persisted `arguments` rather
/// than from a live frame that was dropped during the gap (OCEAN-382).
fn replay_component_call(
    turns: &mut Vec<Turn>,
    pinned: &mut Vec<PinnedWidget>,
    call: &SessionToolContext,
) {
    let args = call.arguments.as_ref();
    let Some(component_id) = args
        .and_then(|a| a.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
    else {
        return;
    };

    if call.tool_name == "component_unmount" {
        for turn in turns.iter_mut() {
            turn.blocks.retain(|block| match block {
                Block::Component {
                    component_id: id, ..
                } => id != &component_id,
                _ => true,
            });
        }
        turns.retain(|turn| !turn.blocks.is_empty());
        pinned.retain(|w| w.component_id != component_id);
        return;
    }

    // component_render. `props` defaults to null when absent so a malformed call
    // still renders an (empty) component rather than disappearing silently.
    let kind = args
        .and_then(|a| a.get("kind"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let props = args
        .and_then(|a| a.get("props"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let replace = args
        .and_then(|a| a.get("replace"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // A pinned render docks into the rail instead of the transcript (mirrors
    // the live reducer): route it to the pinned vec the caller folds back into
    // the registry alongside the rebuilt turns.
    if component_placement(&props) == ComponentPlacement::Pinned {
        upsert_pinned(
            pinned,
            PinnedWidget {
                component_id,
                kind,
                props,
            },
        );
        return;
    }

    // replace:true overwrites the existing block with the same id in place,
    // matching the live reducer. A render of an id already present (without
    // replace) still updates it on replay — re-running the original event stream
    // could only have produced one live block per id anyway.
    if replace {
        for turn in turns.iter_mut() {
            for block in turn.blocks.iter_mut() {
                if let Block::Component {
                    component_id: id, ..
                } = block
                {
                    if id == &component_id {
                        *block = Block::Component {
                            component_id: component_id.clone(),
                            kind: kind.clone(),
                            props: props.clone(),
                        };
                        return;
                    }
                }
            }
        }
    }

    let turn = ensure_component_turn(turns);
    turn.blocks.push(Block::Component {
        component_id,
        kind,
        props,
    });
}

/// Read a component's placement from its payload. `props.placement == "pinned"`
/// docks the component into the persistent rail; anything else (including a
/// missing key, an unknown value, or a non-string) renders inline as before.
/// Sourced from `props` so it flows through every path — live SSE event, voice
/// `render_component`, and persisted-tool-call replay — with no daemon/wire
/// change.
fn component_placement(props: &Value) -> ComponentPlacement {
    match props.get("placement").and_then(|v| v.as_str()) {
        Some("pinned") => ComponentPlacement::Pinned,
        _ => ComponentPlacement::Inline,
    }
}

/// Upsert a pinned widget by `component_id` (last write wins), preserving
/// insertion order. The Vec is treated as an id-keyed map — Leptos-idiomatic
/// for a small ordered reactive collection.
fn upsert_pinned(pinned: &mut Vec<PinnedWidget>, widget: PinnedWidget) {
    if let Some(existing) = pinned
        .iter_mut()
        .find(|w| w.component_id == widget.component_id)
    {
        *existing = widget;
    } else {
        pinned.push(widget);
    }
}

fn ensure_assistant_turn<'a>(turns: &'a mut Vec<Turn>, turn_id: &str) -> &'a mut Turn {
    let matches_last = turns
        .last()
        .map(|t| t.role == Role::Assistant && t.turn_id.as_deref() == Some(turn_id))
        .unwrap_or(false);
    if !matches_last {
        turns.push(Turn::assistant(turn_id.to_string()));
    }
    turns.last_mut().unwrap()
}

/// Attach a component render to the CURRENT assistant turn. `ComponentRender`
/// events carry no `turn_id`, so they must fold into whatever assistant turn is
/// active (the text the agent just streamed) rather than minting a fresh turn
/// every time — otherwise each card splinters one logical turn across the
/// transcript and the next text delta orphans the component. Falls back to a
/// synthetic turn only when there is no assistant turn to attach to (e.g. a
/// component emitted before any assistant text).
fn ensure_component_turn(turns: &mut Vec<Turn>) -> &mut Turn {
    let attach_to_last = matches!(turns.last(), Some(t) if t.role == Role::Assistant);
    if !attach_to_last {
        turns.push(Turn::assistant("component-render".to_string()));
    }
    turns.last_mut().unwrap()
}

/// Returns true if `event_id` has already been applied, otherwise records it.
///
/// The daemon sends stable SSE `id:` values for `AgentTurnEvent`s. Browser
/// EventSource/proxy reconnects may replay recent frames, and the streaming
/// accumulator is intentionally append-only for delta events, so replaying a
/// frame blindly duplicates visible text/tool output. Keep a bounded LRU-style
/// window so a re-delivered id is applied at most once without growing forever
/// during a long daemon session.
fn seen_recent_sse_id(seen: RwSignal<VecDeque<String>>, event_id: &str) -> bool {
    const MAX_SEEN_SSE_IDS: usize = 2048;

    if seen.with_untracked(|ids| ids.iter().any(|id| id == event_id)) {
        return true;
    }

    seen.update(|ids| {
        ids.push_back(event_id.to_string());
        while ids.len() > MAX_SEEN_SSE_IDS {
            ids.pop_front();
        }
    });
    false
}

/// Best-effort default cwd. In the browser there's no real cwd, and pinning a
/// machine-specific dev root here made every fresh launch open a broad
/// non-repo tree (QA-004). Empty means "no workspace yet": the daemon resolves
/// a non-empty turn cwd, else the project root, else its own default
/// (`resolve_cwd_for_turn`), the workspace pane renders its awaiting-session
/// state, and session restore / turn responses set the real cwd afterward.
fn default_cwd() -> String {
    String::new()
}

/// True when this bundle is running inside the Chrome extension side panel
/// (document served from `chrome-extension://`) rather than the browser PWA.
/// Drives both the daemon-URL bootstrap and the `client_type` we report so the
/// agent knows it's the in-Chrome cockpit, not a detached web app.
pub fn running_as_extension() -> bool {
    web_sys::window()
        .and_then(|w| w.location().protocol().ok())
        .map(|p| p.starts_with("chrome-extension"))
        .unwrap_or(false)
}

/// True when this Leptos bundle is running inside the Tauri native shell
/// (WKWebView on macOS, WebView2 elsewhere), detected by the presence of
/// the `__TAURI_INTERNALS__` global Tauri injects. The browser PWA and the
/// Chrome extension both lack it. Drives `surface_client_type()` so the
/// daemon's agent sees `surface-tauri` and the surface knows it may invoke
/// Tauri commands (`pick_folder`, `watch_paths`, ...) that browsers can't.
pub fn running_as_tauri() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    js_sys::Reflect::has(
        &window,
        &wasm_bindgen::JsValue::from_str("__TAURI_INTERNALS__"),
    )
    .unwrap_or(false)
}

/// Parse a `data:<mime>;base64,<body>` URL into a [`TurnImage`] (OCEAN-138).
/// `chrome.tabs.captureVisibleTab` hands us exactly this shape. We keep the full
/// `data:` URL in `data` — the daemon strips the `data:<mime>;base64,` prefix
/// itself, so either form is accepted; carrying the prefix means the same value
/// can drive an `<img src>` preview later without reconstruction. Returns `None`
/// for anything that isn't a base64 data URL.
fn parse_data_url(data_url: &str) -> Option<TurnImage> {
    let rest = data_url.strip_prefix("data:")?;
    let (meta, _body) = rest.split_once(',')?;
    // meta looks like `image/png;base64`. Require base64 and a non-empty mime.
    let mime_type = meta.split(';').next().unwrap_or("").trim().to_string();
    if mime_type.is_empty() || !meta.contains("base64") {
        return None;
    }
    Some(TurnImage {
        mime_type,
        data: data_url.to_string(),
    })
}

/// `client_type` for voice-orb turns. The daemon routes its concise, speakable,
/// no-visual-components voice system prompt (`voice_surface_prompt`) on exactly
/// this string, so it must stay in lockstep with the daemon (OCEAN-181).
pub(crate) const VOICE_CLIENT_TYPE: &str = "leo-voice";

/// The surface identity sent to the daemon as `client_type`, so the agent's
/// system prompt is scoped to where the user is actually talking from. This is
/// the ambient identity for typed turns; voice turns override it with
/// [`VOICE_CLIENT_TYPE`] via [`Daemon::send_voice_prompt`]. Returns
/// `"surface-extension"` in the Chrome side panel, `"surface-tauri"` inside the
/// Tauri native shell (detected via [`running_as_tauri`] / the
/// `__TAURI_INTERNALS__` global), or `"surface-web"` as the browser PWA
/// fallback.
fn surface_client_type() -> &'static str {
    if running_as_extension() {
        "surface-extension"
    } else if running_as_tauri() {
        "surface-tauri"
    } else {
        "surface-web"
    }
}

/// Maximum number of open tabs shipped in the structured browser context,
/// matching the extension loader's own cap. Keeps the snapshot bounded for an
/// operator with many tabs open. (Named for the removed guidance path it
/// originally bounded; still the cap for the structured path, TASK-76.)
const MAX_OPEN_TABS_GUIDANCE: usize = 24;

/// Ensure exactly one tab is flagged active when the loader snapshot carried no
/// per-tab `active` boolean (an older `sidepanel.js`, OCEAN-377 P2). The current
/// loader publishes Chrome's own per-tab flag, which is authoritative and may
/// legitimately mark zero tabs (the active tab was filtered out, e.g. the
/// extension's own page) — so when `snapshot_has_active_flag` is true we leave
/// the parsed flags untouched. Only on the flagless fallback do we flag the
/// FIRST tab whose URL matches the active-tab URL, never every duplicate, so a
/// window with several same-URL tabs still resolves to a single "this tab".
fn flag_active_tab_fallback(
    tabs: &mut [BrowserTab],
    snapshot_has_active_flag: bool,
    active_tab_url: Option<&str>,
) {
    if snapshot_has_active_flag {
        return;
    }
    if let Some(active_url) = active_tab_url {
        if let Some(first) = tabs.iter_mut().find(|t| t.url == active_url) {
            first.active = true;
        }
    }
}

/// Structured browser state for the turn (OCEAN-377) — since TASK-76 the ONLY
/// channel carrying browser state to the daemon. Reads the loader-published
/// globals —
/// `window.__ocean_active_tab` (`{url, title}`) and `window.__ocean_open_tabs`
/// (`[{url, title}, …]`, OCEAN-92) — and packs them into the daemon's
/// `ClientContext` wire shape. Guard rails: only the
/// Chrome extension, only the user's already-open tabs, never the extension's
/// own panel/new-tab pages. Returns `None` (so `client_context` is omitted) on
/// non-extension surfaces or when no browser state is available.
fn browser_client_context(client_type: &str) -> Option<ClientContext> {
    if !running_as_extension() {
        return None;
    }
    let window = web_sys::window()?;
    let read = |obj: &wasm_bindgen::JsValue, key: &str| -> Option<String> {
        js_sys::Reflect::get(obj, &wasm_bindgen::JsValue::from_str(key))
            .ok()
            .and_then(|v| v.as_string())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    // Active tab (OCEAN-70): skip the extension's own panel + empty new-tab.
    let (active_tab_url, active_tab_title) = {
        let tab = js_sys::Reflect::get(
            &window,
            &wasm_bindgen::JsValue::from_str("__ocean_active_tab"),
        )
        .ok()
        .filter(|t| t.is_object());
        match tab.as_ref().and_then(|t| read(t, "url")) {
            Some(url)
                if !url.starts_with("chrome-extension://")
                    && !url.starts_with("chrome://newtab") =>
            {
                let title = tab.as_ref().and_then(|t| read(t, "title"));
                (Some(url), title)
            }
            _ => (None, None),
        }
    };

    // Open-tab list (OCEAN-92): the current window's tabs, capped, with the
    // extension's own pages filtered out. The loader publishes Chrome's own
    // per-tab `active` boolean on each entry (`sidepanel.js`: `active:
    // !!t.active`); we trust THAT flag so the daemon can resolve "this tab" even
    // when several tabs share a URL — a URL match would flag every duplicate
    // (OCEAN-377 P2). We only fall back to a single-tab URL match below if the
    // snapshot carries no per-tab flag at all.
    let read_bool = |obj: &wasm_bindgen::JsValue, key: &str| -> Option<bool> {
        js_sys::Reflect::get(obj, &wasm_bindgen::JsValue::from_str(key))
            .ok()
            .filter(|v| !v.is_undefined() && !v.is_null())
            .and_then(|v| v.as_bool())
    };
    let mut tabs: Vec<BrowserTab> = Vec::new();
    let mut snapshot_has_active_flag = false;
    if let Ok(tabs_val) = js_sys::Reflect::get(
        &window,
        &wasm_bindgen::JsValue::from_str("__ocean_open_tabs"),
    ) {
        let arr = js_sys::Array::from(&tabs_val);
        for i in 0..arr.length().min(MAX_OPEN_TABS_GUIDANCE as u32) {
            let tab = arr.get(i);
            if !tab.is_object() {
                continue;
            }
            let Some(url) = read(&tab, "url") else {
                continue;
            };
            if url.starts_with("chrome-extension://") {
                continue;
            }
            // Prefer the snapshot's real per-tab flag; default false when the
            // entry omits it (handled by the URL fallback after the loop).
            let active = match read_bool(&tab, "active") {
                Some(flag) => {
                    snapshot_has_active_flag = true;
                    flag
                }
                None => false,
            };
            tabs.push(BrowserTab {
                title: read(&tab, "title").unwrap_or_default(),
                url,
                active,
            });
        }
    }

    // Fallback for an older loader snapshot with no per-tab `active` flag.
    flag_active_tab_fallback(
        &mut tabs,
        snapshot_has_active_flag,
        active_tab_url.as_deref(),
    );

    assemble_client_context(client_type, active_tab_url, active_tab_title, tabs)
}

/// Pack the parsed browser snapshot into a `ClientContext`, or `None` when
/// there's nothing usable (so the turn omits the field and the wire shape is
/// unchanged). `surface_client_type` is the SURFACE identity
/// (`surface-extension`/`surface-web`) — NOT the flat turn routing tag — so the
/// context the daemon's `apply_browser_context()` keys on is always recognized
/// as a browser surface, even for voice turns tagged `leo-voice` (OCEAN-377 P2).
fn assemble_client_context(
    surface_client_type: &str,
    active_tab_url: Option<String>,
    active_tab_title: Option<String>,
    tabs: Vec<BrowserTab>,
) -> Option<ClientContext> {
    if active_tab_url.is_none() && tabs.is_empty() {
        return None;
    }
    Some(ClientContext {
        client_type: surface_client_type.to_string(),
        browser: Some(BrowserContext {
            active_tab_url,
            active_tab_title,
            tabs,
        }),
    })
}

fn session_title_hint(prompt: &str) -> Option<String> {
    let title = prompt.trim().chars().take(60).collect::<String>();
    (!title.is_empty()).then_some(title)
}

const PROJECT_STORAGE_KEY: &str = "ocean.project_id";

/// localStorage handle, if available (it isn't in SSR / some embeddings).
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

/// The persisted project selection, restored on construction.
fn load_persisted_project() -> Option<String> {
    local_storage()
        .and_then(|s| s.get_item(PROJECT_STORAGE_KEY).ok().flatten())
        .filter(|s| !s.is_empty())
}

fn persist_project(id: &str) {
    if let Some(s) = local_storage() {
        let _ = s.set_item(PROJECT_STORAGE_KEY, id);
    }
}

fn clear_persisted_project() {
    if let Some(s) = local_storage() {
        let _ = s.remove_item(PROJECT_STORAGE_KEY);
    }
}

const THINKING_LEVEL_STORAGE_KEY: &str = "ocean.thinking_level";
const MODEL_OVERRIDE_STORAGE_KEY: &str = "ocean.model_override";

/// Valid serialized `ThinkingLevel` values the daemon accepts. We restrict the
/// persisted value to these so a stale/garbage localStorage entry can't ship a
/// bad `thinking_level` the daemon would reject. MUST stay in lockstep with
/// `ocean_protocol::ThinkingLevel` (serde lowercase) and with the composer's
/// dropdown in `app.rs` — otherwise a level the dropdown offers gets silently
/// dropped on reload by this restore filter. (OCEAN-202 added minimal + xhigh.)
const THINKING_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh"];

/// The persisted per-turn thinking level, restored on construction. Filtered to
/// known values so only a valid `ThinkingLevel` string is ever loaded.
fn load_persisted_thinking_level() -> Option<String> {
    local_storage()
        .and_then(|s| s.get_item(THINKING_LEVEL_STORAGE_KEY).ok().flatten())
        .filter(|v| THINKING_LEVELS.contains(&v.as_str()))
}

fn persist_thinking_level(level: &str) {
    if let Some(s) = local_storage() {
        let _ = s.set_item(THINKING_LEVEL_STORAGE_KEY, level);
    }
}

fn clear_persisted_thinking_level() {
    if let Some(s) = local_storage() {
        let _ = s.remove_item(THINKING_LEVEL_STORAGE_KEY);
    }
}

/// The persisted per-turn model override, restored on construction.
fn load_persisted_model_override() -> Option<String> {
    local_storage()
        .and_then(|s| s.get_item(MODEL_OVERRIDE_STORAGE_KEY).ok().flatten())
        .filter(|s| !s.is_empty())
}

fn persist_model_override(id: &str) {
    if let Some(s) = local_storage() {
        let _ = s.set_item(MODEL_OVERRIDE_STORAGE_KEY, id);
    }
}

fn clear_persisted_model_override() {
    if let Some(s) = local_storage() {
        let _ = s.remove_item(MODEL_OVERRIDE_STORAGE_KEY);
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const SESSION_STORAGE_KEY: &str = "ocean.session_id";

fn load_persisted_session() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        local_storage()
            .and_then(|s| s.get_item(SESSION_STORAGE_KEY).ok().flatten())
            .filter(|s| !s.is_empty())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

fn persist_session(_id: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(s) = local_storage() {
            let _ = s.set_item(SESSION_STORAGE_KEY, _id);
        }
    }
}

fn clear_persisted_session() {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(s) = local_storage() {
            let _ = s.remove_item(SESSION_STORAGE_KEY);
        }
    }
}

/// Pure decision helper: should we restore a persisted session on boot?
/// Returns the session id if `persisted` is present and non-empty AND no
/// session is already active. Testable off-wasm.
fn should_restore_session<'a>(persisted: Option<&'a str>, active: Option<&str>) -> Option<&'a str> {
    match persisted {
        Some(id) if !id.is_empty() && active.is_none() => Some(id),
        _ => None,
    }
}

/// Pre-flight fetch to verify a persisted session exists on the daemon, then
/// restore via [`Daemon::switch_session`]. On failure (non-200, decode error,
/// missing session) the persisted key is cleared and this returns `false` so
/// the caller falls through to the normal boot path with state untouched.
/// What became of a remembered session id on the machine we asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestoreOutcome {
    /// The machine had it and it is now the focused session.
    Restored,
    /// It is not there (or the reply was unreadable): forgotten, and the
    /// caller opens a fresh stream.
    Cleared,
    /// A different session intent took over while this was in flight. The
    /// answer is stale by definition — neither the focus nor the persisted id
    /// is ours to touch any more.
    Superseded,
}

async fn restore_session_or_clear(daemon: &Daemon, id: &str, intent: u64) -> RestoreOutcome {
    let url = daemon.url.get_untracked();
    let get_url = format!(
        "{}/v1/sessions/{}",
        url.trim_end_matches('/'),
        encode_path_segment(id)
    );
    match Request::get(&get_url).send().await {
        Ok(resp) => match resp.json::<SessionDetailResponse>().await {
            // One rule, two callers: boot restores the session this browser
            // remembers, and a device switch asks the same question of the
            // machine it just moved to. Absence is `Clear`, never an error
            // (`crate::devices::classify_session_restore`).
            Ok(r) => {
                // The round trip is over; whoever owns the intent NOW owns the
                // answer to what this browser should be looking at.
                if !crate::devices::claim_is_current(
                    intent,
                    daemon.session_intent_generation.get_untracked(),
                ) {
                    log::info!("session restore superseded by a newer intent");
                    return RestoreOutcome::Superseded;
                }
                if crate::devices::classify_session_restore(r.ok, r.session.is_some())
                    == crate::devices::SessionRestore::Restore
                {
                    if let Some(detail) = r.session {
                        daemon.switch_session(detail.id, detail.title);
                        return RestoreOutcome::Restored;
                    }
                }
            }
            Err(err) => {
                log::error!("session restore decode error: {err}");
            }
        },
        Err(err) => {
            log::error!("session restore fetch error: {err}");
        }
    }
    // Same guard on the clearing arm, and it is the half that bites hardest:
    // clearing here after a new session persisted its id would lose it.
    if !crate::devices::claim_is_current(intent, daemon.session_intent_generation.get_untracked()) {
        log::info!("session restore superseded before it could clear");
        return RestoreOutcome::Superseded;
    }
    clear_persisted_session();
    RestoreOutcome::Cleared
}

/// Resolve the daemon URL at startup from compile-time env → page origin → loopback.
///
/// The only case that legitimately needs `http://127.0.0.1:4780` is the
/// Chrome-extension side panel (served from `chrome-extension://`), which talks
/// to the daemon's loopback directly — that path is handled in
/// `bootstrap_then_connect` via `running_as_extension()`, and the http-localhost
/// dev page keeps the loopback fallback below.
pub(crate) fn daemon_url_from_env() -> String {
    // Compile-time override (Tauri builds can set OCEAN_DAEMON_URL).
    if let Some(url) = option_env!("OCEAN_DAEMON_URL") {
        return url.to_string();
    }
    if let Some(window) = web_sys::window() {
        let location = window.location();
        let protocol = location.protocol().unwrap_or_default();
        let host = location.host().unwrap_or_default();
        return daemon_url_fallback(&protocol, &host);
    }
    DEFAULT_DAEMON_URL.into()
}

/// Pure fallback resolver (testable off-target). Given the page's `protocol`
/// (e.g. `"https:"`) and `host` (e.g. `"ocean.agentsworld.org"`), return the
/// daemon URL to use until `/api/config` answers:
///   - https → `""` (same-origin, relative `/v1/...` via the proxy; an
///     `http://host:4780` URL would be mixed-content and blocked).
///   - http with a host → `http://{host_only}:4780` (LAN/localhost dev).
///   - otherwise → the loopback default.
pub(crate) fn daemon_url_fallback(protocol: &str, host: &str) -> String {
    if protocol == "https:" {
        return String::new();
    }
    if !host.is_empty() {
        let host_only = host.split(':').next().unwrap_or(host);
        // `localhost` resolves ::1-first on macOS while the daemon listens on
        // the IPv4 loopback only; WKWebView's v6->v4 fallback is inconsistent
        // (EventSource especially), so pin the IP and skip resolution games.
        // Hits the Tauri shell (tauri://localhost) and http://localhost dev.
        if host_only == "localhost" {
            return DEFAULT_DAEMON_URL.into();
        }
        return format!("http://{host_only}:4780");
    }
    DEFAULT_DAEMON_URL.into()
}

/// Component-boundary path-prefix check: `prefix` matches `candidate`
/// when candidate == prefix OR candidate starts with prefix + "/".
/// "/a/b" matches "/a/b/c" but NOT "/a/bc".
pub(crate) fn is_path_prefix(prefix: &str, candidate: &str) -> bool {
    candidate == prefix
        || candidate.starts_with(prefix) && candidate.as_bytes().get(prefix.len()) == Some(&b'/')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_entry(is_error: bool) -> SessionTranscriptEntry {
        SessionTranscriptEntry {
            role: "tool".into(),
            text: "boom".into(),
            tool_call_id: None,
            tool_name: Some("read_file".into()),
            is_error: Some(is_error),
        }
    }

    #[test]
    fn model_info_decodes_the_daemons_flat_readiness_payload() {
        // The daemon serializes ReadyModel FLAT (id/provider/label top-level
        // plus ready + optional credential_source) and its own wire test pins
        // that shape; this is the client half of the same contract, including
        // the bare-string "not_required" variant.
        let models: Vec<ModelInfo> = serde_json::from_value(serde_json::json!([
            {"id": "claude-opus-5", "provider": "anthropic", "label": "Opus 5",
             "ready": true, "credential_source": {"env": {"name": "ANTHROPIC_API_KEY"}}},
            {"id": "fake-local", "provider": "fake", "label": "Fake",
             "ready": true, "credential_source": "not_required"},
            {"id": "grok-4", "provider": "xai", "label": "Grok 4", "ready": false},
        ]))
        .expect("flat readiness payload decodes");

        assert_eq!(models[0].ready, Some(true));
        assert_eq!(models[0].unready_reason(), None);
        assert_eq!(
            models[1].credential_source,
            Some(Value::String("not_required".into()))
        );
        assert_eq!(models[1].unready_reason(), None);
        // Unready entries carry no credential_source on today's wire, so the
        // reason names the provider the operator has to configure.
        assert_eq!(models[2].ready, Some(false));
        assert_eq!(
            models[2].unready_reason().as_deref(),
            Some("no credential for xai")
        );
    }

    #[test]
    fn model_info_from_an_older_daemon_renders_exactly_as_before() {
        // A daemon predating readiness omits both fields. That must decode to
        // ready: None — NOT false — or every model shows as unusable.
        let models: Vec<ModelInfo> = serde_json::from_value(serde_json::json!([
            {"id": "claude-opus-5", "provider": "anthropic", "label": "Opus 5"}
        ]))
        .expect("pre-readiness payload decodes");
        assert_eq!(models[0].ready, None);
        assert_eq!(models[0].credential_source, None);
        assert_eq!(models[0].unready_reason(), None);
    }

    #[test]
    fn unknown_credential_source_variants_cannot_blank_the_picker() {
        // fetch_models keeps the OLD list on any decode error, so a daemon
        // that grows a credential_source variant must still decode here —
        // and the hint degrades to the variant's tag.
        let models: Vec<ModelInfo> = serde_json::from_value(serde_json::json!([
            {"id": "m", "provider": "p", "label": "M", "ready": false,
             "credential_source": {"os_keychain": {"service": "ocean"}}},
            {"id": "n", "provider": "q", "label": "N", "ready": false,
             "credential_source": {"env": {"name": "ANTHROPIC_API_KEY"}}},
        ]))
        .expect("unknown credential_source variant still decodes");
        assert_eq!(
            models[0].unready_reason().as_deref(),
            Some("no credential: os keychain")
        );
        // A recognized source on an unready entry names the concrete thing.
        assert_eq!(
            models[1].unready_reason().as_deref(),
            Some("no credential: ANTHROPIC_API_KEY")
        );
    }

    #[test]
    fn attention_request_snapshot_decodes_daemon_wire_state() {
        let snapshot: RequestSnapshot = serde_json::from_value(serde_json::json!({
            "request_id": "request-a",
            "session_id": "session-a",
            "state": "waiting_for_permission",
            "permission_id": "permission-a",
            "message": "waiting",
            "updated_at": "2026-07-14T12:00:00Z"
        }))
        .expect("request snapshot decodes");
        assert_eq!(snapshot.state, RequestSnapshotState::WaitingForPermission);
        assert_eq!(snapshot.session_id.as_deref(), Some("session-a"));
    }

    #[test]
    fn cancel_response_requires_body_ok_not_only_http_success() {
        assert_eq!(
            request_cancel_result(
                true,
                r#"{"ok":true,"request_id":"request-a","state":"cancelling","message":"accepted"}"#,
            ),
            Ok(())
        );
        assert_eq!(
            request_cancel_result(
                true,
                r#"{"ok":false,"request_id":"request-a","state":"completed","message":"request already terminal"}"#,
            ),
            Err("request already terminal".into())
        );
        assert!(request_cancel_result(false, r#"{"ok":true,"message":"unexpected"}"#).is_err());
    }

    #[test]
    fn recall_input_change_invalidates_stale_results_before_debounce() {
        let daemon = Daemon::dummy();
        daemon.history_results.set(vec![HistorySearchHit {
            hit_id: "query-a-hit".into(),
            session_id: "session-a".into(),
            session_title: "Old result".into(),
            role: "assistant".into(),
            excerpt: "stale".into(),
            timestamp_ms: None,
            workspace_root: None,
            score: 1.0,
            match_kind: "fuzzy".into(),
        }]);
        daemon.history_searching.set(true);
        let previous_generation = daemon.history_search_generation.get_untracked();

        daemon.invalidate_history_search();

        assert!(daemon.history_results.get_untracked().is_empty());
        assert!(!daemon.history_searching.get_untracked());
        assert_eq!(
            daemon.history_search_generation.get_untracked(),
            previous_generation.wrapping_add(1),
        );
    }

    #[test]
    fn history_search_response_decodes_bounded_daemon_hits() {
        let response: HistorySearchResponse = serde_json::from_value(serde_json::json!({
            "ok": true,
            "query": "dynamic island",
            "hits": [{
                "hit_id": "session-a:2",
                "session_id": "session-a",
                "session_title": "Island direction",
                "role": "assistant",
                "excerpt": "The Island should change shape around one intent.",
                "timestamp_ms": 1784100000000_i64,
                "workspace_root": "/workspace/ocean",
                "score": 3.25,
                "match_kind": "exact"
            }],
            "error": null
        }))
        .expect("history response decodes");
        assert!(response.ok);
        assert_eq!(response.hits.len(), 1);
        assert_eq!(response.hits[0].session_id, "session-a");
        assert_eq!(response.hits[0].match_kind, "exact");
    }

    #[test]
    fn attention_permission_snapshot_ignores_raw_args() {
        let snapshot: PermissionSnapshot = serde_json::from_value(serde_json::json!({
            "permission_id": "permission-a",
            "request_id": "request-a",
            "session_id": "session-a",
            "tool": "bash",
            "reason": "run a check",
            "args": { "command": "private payload stays outside the Island DTO" },
            "created_at": "2026-07-14T12:00:00Z"
        }))
        .expect("permission snapshot decodes while ignoring args");
        assert_eq!(snapshot.tool, "bash");
        assert_eq!(snapshot.reason, "run a check");
    }

    #[test]
    fn component_render_attaches_to_active_turn_not_a_new_one() {
        // The agent streamed text on turn "t1", then renders a card. The card
        // must fold into t1, not splinter into a separate synthetic turn.
        let mut turns = vec![Turn::assistant("t1".to_string())];
        turns[0].blocks.push(Block::Text("hello".into()));

        let turn = ensure_component_turn(&mut turns);
        turn.blocks.push(Block::Component {
            component_id: "c1".into(),
            kind: "card".into(),
            props: serde_json::Value::Null,
        });

        assert_eq!(
            turns.len(),
            1,
            "card folds into the active turn, no new turn"
        );
        assert_eq!(turns[0].turn_id.as_deref(), Some("t1"));
        assert_eq!(turns[0].blocks.len(), 2, "text + component in one turn");

        // A second card on the same turn still does not create a new turn.
        ensure_component_turn(&mut turns);
        assert_eq!(turns.len(), 1);

        // Fallback: with no assistant turn to attach to (last is a user turn),
        // a synthetic assistant turn is created so the card has a home.
        let mut empty = vec![Turn::user("hi")];
        ensure_component_turn(&mut empty);
        assert_eq!(empty.len(), 2);
        assert_eq!(empty[1].role, Role::Assistant);
    }

    #[test]
    fn failed_tool_call_hydrates_expanded() {
        let (turns, _pinned) = turns_from_session_transcript(vec![tool_entry(true)], &[]);
        let block = &turns[0].blocks[0];
        match block {
            Block::ToolCall {
                status, expanded, ..
            } => {
                assert_eq!(*status, ToolStatus::Err);
                assert!(*expanded, "failed tool call should auto-expand");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn successful_tool_call_hydrates_collapsed() {
        let (turns, _pinned) = turns_from_session_transcript(vec![tool_entry(false)], &[]);
        let block = &turns[0].blocks[0];
        match block {
            Block::ToolCall {
                status, expanded, ..
            } => {
                assert_eq!(*status, ToolStatus::Ok);
                assert!(!*expanded, "successful tool call should stay collapsed");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    fn apply_test_event(daemon: &Daemon, event: AgentEvent) {
        apply_event(
            &event,
            daemon.turns,
            daemon.session_id,
            daemon.streaming,
            daemon.status,
            daemon.status_detail,
            daemon.last_turn_tokens,
            daemon.session_tokens,
            daemon.model,
            daemon.active_turn_id,
            daemon.browser_active,
            daemon.browser_last_action,
            daemon.canvas_patches,
            daemon.pinned_widgets,
            daemon.awaiting_session_adoption,
            daemon.sync_pending,
            daemon.sync_poll_kick,
            daemon.activity_revision,
            daemon.session_title,
            daemon.cwd,
        );
    }

    fn daemon_with_session(session_id: &str) -> Daemon {
        let daemon = Daemon::dummy();
        daemon.session_id.set(Some(session_id.to_string()));
        daemon
    }

    fn start_running_tool(daemon: &Daemon, session_id: &str, turn_id: &str, call_id: &str) {
        apply_test_event(
            daemon,
            AgentEvent::ToolCallStarted {
                session_id: session_id.to_string(),
                turn_id: turn_id.to_string(),
                call: ToolCallSummary {
                    id: call_id.to_string(),
                    name: "bash".to_string(),
                    args_json: serde_json::json!({ "command": "sleep 60" }),
                },
            },
        );
    }

    fn finish_turn(daemon: &Daemon, session_id: &str, turn_id: &str, turn_status: &str) {
        apply_test_event(
            daemon,
            AgentEvent::TurnFinished {
                session_id: session_id.to_string(),
                turn_id: turn_id.to_string(),
                status: turn_status.to_string(),
                error: None,
                wall_ms: Some(120),
                output_tokens: None,
                input_tokens: None,
                cache_read_tokens: None,
                tokens_per_second: None,
            },
        );
    }

    fn tool_state(daemon: &Daemon, turn_id: &str, call_id: &str) -> (ToolStatus, bool) {
        let turns = daemon.turns.get_untracked();
        turns
            .iter()
            .find(|turn| turn.turn_id.as_deref() == Some(turn_id))
            .and_then(|turn| {
                turn.blocks.iter().find_map(|block| match block {
                    Block::ToolCall {
                        call_id: id,
                        status,
                        expanded,
                        ..
                    } if id == call_id => Some((*status, *expanded)),
                    _ => None,
                })
            })
            .expect("expected ToolCall block for turn")
    }

    #[test]
    fn cancelled_turn_closes_running_tool() {
        let session_id = "session-ocean-319-cancelled";
        let turn_id = "turn-cancelled";
        let call_id = "call-cancelled";
        let daemon = daemon_with_session(session_id);

        start_running_tool(&daemon, session_id, turn_id, call_id);
        assert_eq!(
            tool_state(&daemon, turn_id, call_id),
            (ToolStatus::Running, false),
            "ToolCallStarted must plant a collapsed Running tool before cancel"
        );

        finish_turn(&daemon, session_id, turn_id, "cancelled");

        assert_eq!(
            tool_state(&daemon, turn_id, call_id),
            (ToolStatus::Err, true),
            "cancelled TurnFinished must close and expand its still-running tool"
        );
    }

    #[test]
    fn completed_turn_leaves_running_tool_untouched() {
        let session_id = "session-ocean-319-completed";
        let turn_id = "turn-completed";
        let call_id = "call-completed";
        let daemon = daemon_with_session(session_id);

        start_running_tool(&daemon, session_id, turn_id, call_id);
        finish_turn(&daemon, session_id, turn_id, "completed");

        assert_eq!(
            tool_state(&daemon, turn_id, call_id),
            (ToolStatus::Running, false),
            "completed TurnFinished must not hide a missing ToolCallFinished bug"
        );
    }

    #[test]
    fn sweep_scoped_to_finishing_turn() {
        let session_id = "session-ocean-319-scoped";
        let turn_a = "turn-a";
        let call_a = "call-a";
        let turn_b = "turn-b";
        let call_b = "call-b";
        let daemon = daemon_with_session(session_id);

        start_running_tool(&daemon, session_id, turn_a, call_a);
        start_running_tool(&daemon, session_id, turn_b, call_b);

        finish_turn(&daemon, session_id, turn_a, "failed");

        assert_eq!(
            tool_state(&daemon, turn_a, call_a),
            (ToolStatus::Err, true),
            "failed TurnFinished must close only the finishing turn's tool"
        );
        assert_eq!(
            tool_state(&daemon, turn_b, call_b),
            (ToolStatus::Running, false),
            "sibling turn's still-running tool must not be swept"
        );
    }

    // TASK-45: terminal-admission decider + reducer regressions. A stale or
    // superseded `TurnFinished` must not blank the live turn's Stop target,
    // running projection, or header status.

    fn start_turn(daemon: &Daemon, session_id: &str, turn_id: &str) {
        apply_test_event(
            daemon,
            AgentEvent::TurnStarted {
                turn_id: turn_id.to_string(),
                session_id: session_id.to_string(),
                model: None,
            },
        );
    }

    fn finish_turn_failed(daemon: &Daemon, session_id: &str, turn_id: &str, err: &str) {
        apply_test_event(
            daemon,
            AgentEvent::TurnFinished {
                session_id: session_id.to_string(),
                turn_id: turn_id.to_string(),
                status: "failed".to_string(),
                error: Some(err.to_string()),
                wall_ms: Some(120),
                output_tokens: None,
                input_tokens: None,
                cache_read_tokens: None,
                tokens_per_second: None,
            },
        );
    }

    #[test]
    fn admit_turn_terminal_clears_only_on_exact_match() {
        assert_eq!(
            admit_turn_terminal(Some("turn-1"), "turn-1"),
            TerminalAdmission::ClearLive
        );
        assert_eq!(
            admit_turn_terminal(Some("turn-1"), "turn-2"),
            TerminalAdmission::KeepLive
        );
        assert_eq!(
            admit_turn_terminal(None, "turn-1"),
            TerminalAdmission::KeepLive
        );
    }

    // ── TASK-43: fresh-session bootstrap admission + two-stream handshake ──────
    //
    // Seven regressions for the create path (verdict TASK A). The admission logic
    // is a pure decider (`admit_create_intent`); the two-stream handshake gate is
    // `planner_streams_ready` over the readiness markers. Both are exercised off
    // the browser, mirroring the other daemon-client deciders.

    fn open_marker(session_id: &str, generation: u64) -> (String, u64) {
        (session_id.to_string(), generation)
    }

    // 1. Switch during create: a session switch bumped the intent generation AND
    // moved focus to another session. The late create response is rejected — no
    // focus change, no dispatch.
    #[test]
    fn switch_during_create_rejects_late_response() {
        // launch captured intent 5; a switch to "session-b" advanced it to 6.
        assert_eq!(
            admit_create_intent(5, 6, None, Some("session-b")),
            CreateAdmission::Reject
        );
    }

    // 2. New Session during create: a second New Session bumped the intent
    // generation but left the active session `None`. The generation alone must
    // catch this — the `None` focus check would pass. Load-bearing case.
    #[test]
    fn new_session_during_create_rejects_on_generation_alone() {
        assert_eq!(
            admit_create_intent(5, 6, None, None),
            CreateAdmission::Reject
        );
        // Sanity: with the generation unchanged, the same `None`/`None` focus is
        // admitted — proving it is the generation, not the focus, doing the work.
        assert_eq!(
            admit_create_intent(5, 5, None, None),
            CreateAdmission::Adopt
        );
    }

    // 3. Agent stream not ready blocks dispatch: permission tail is OPEN but the
    // agent tail has no marker, so the handshake gate is not satisfied.
    #[test]
    fn agent_stream_not_ready_blocks_dispatch() {
        let permission = open_marker("session-a", 7);
        assert!(!planner_streams_ready(
            "session-a",
            7,
            None,
            Some(&permission)
        ));
    }

    // 4. Permission stream not ready blocks dispatch: agent tail is OPEN but the
    // permission tail has no marker.
    #[test]
    fn permission_stream_not_ready_blocks_dispatch() {
        let agent = open_marker("session-a", 7);
        assert!(!planner_streams_ready("session-a", 7, Some(&agent), None));
    }

    // 5. Both ready dispatches: both markers OPEN for the exact session+generation
    // AND the create intent still owns the cockpit → the first turn is dispatched.
    #[test]
    fn both_streams_ready_admits_first_turn() {
        let agent = open_marker("session-a", 7);
        let permission = open_marker("session-a", 7);
        assert!(planner_streams_ready(
            "session-a",
            7,
            Some(&agent),
            Some(&permission)
        ));
        assert_eq!(
            admit_create_intent(5, 5, Some("session-a"), Some("session-a")),
            CreateAdmission::Adopt
        );
    }

    // 6. Stale generation after readiness does not dispatch: the streams opened,
    // but a newer intent (New Session / switch) settled before the first turn
    // POST. Focus still points at the created session, yet the generation moved —
    // phase-2 admission rejects, so no turn is sent.
    #[test]
    fn stale_generation_after_readiness_does_not_dispatch() {
        let agent = open_marker("session-a", 7);
        let permission = open_marker("session-a", 7);
        assert!(planner_streams_ready(
            "session-a",
            7,
            Some(&agent),
            Some(&permission)
        ));
        assert_eq!(
            admit_create_intent(5, 6, Some("session-a"), Some("session-a")),
            CreateAdmission::Reject
        );
    }

    // 7. Timeout leaves a visible failure and sends no turn: the streams never
    // both opened (agent marker absent), so the handshake gate never admits a
    // dispatch. While the created session is still focused, the failure IS
    // surfaced (phase-2 admission Adopt gates the status write); if a newer intent
    // took over, the failure is swallowed rather than clobbering its status.
    #[test]
    fn timeout_surfaces_failure_and_sends_no_turn() {
        let permission = open_marker("session-a", 7);
        // Handshake never completed → no dispatch path is ever taken.
        assert!(!planner_streams_ready(
            "session-a",
            7,
            None,
            Some(&permission)
        ));
        // Still focused on the created session → surface the timeout failure.
        assert_eq!(
            admit_create_intent(5, 5, Some("session-a"), Some("session-a")),
            CreateAdmission::Adopt
        );
        // A newer intent settled during the timeout → do not clobber its status.
        assert_eq!(
            admit_create_intent(5, 6, Some("session-a"), Some("session-a")),
            CreateAdmission::Reject
        );
    }

    // New Session and session switch must each advance the intent generation —
    // the mechanism tests 1, 2, and 6 rely on. `new_session` leaves the active
    // session `None`; both bump the generation. `connect()` early-returns on a
    // `None` session, so `new_session` is safe to drive off the browser here.
    #[test]
    fn focus_changes_bump_session_intent_generation() {
        let daemon = Daemon::dummy();
        let before = daemon.session_intent_generation.get_untracked();
        daemon.new_session();
        let after_new = daemon.session_intent_generation.get_untracked();
        assert_eq!(after_new, before.wrapping_add(1));
        assert!(daemon.session_id.get_untracked().is_none());

        daemon.select_session_state("session-x", "X".into());
        let after_switch = daemon.session_intent_generation.get_untracked();
        assert_eq!(after_switch, after_new.wrapping_add(1));
        assert_eq!(
            daemon.session_id.get_untracked().as_deref(),
            Some("session-x")
        );
    }

    // ── TASK-44: atomic session projection — switch/reconnect reconciliation ──

    fn task44_card(permission_id: &str, request_id: Option<&str>) -> PendingPermission {
        session_card(permission_id, request_id, "session-a")
    }

    /// A pending-permission card scoped to a specific session — for tests that
    /// exercise session-bound authority resolution (codex HOLD on af82ee8).
    fn session_card(
        permission_id: &str,
        request_id: Option<&str>,
        session: &str,
    ) -> PendingPermission {
        PendingPermission {
            permission_id: permission_id.into(),
            session_id: session.into(),
            tool: "bash".into(),
            reason: "run check".into(),
            args_summary: String::new(),
            deciding: false,
            request_id: request_id.map(str::to_string),
            actionable: false,
        }
    }

    fn task44_authority(pairs: &[(&str, &str, &str)]) -> HashMap<String, DecisionGrant> {
        pairs
            .iter()
            .map(|(request_id, token, session_id)| {
                (
                    (*request_id).to_string(),
                    DecisionGrant {
                        token: (*token).to_string(),
                        session_id: (*session_id).to_string(),
                    },
                )
            })
            .collect()
    }

    // 1. Switch into a session with a pending gate: the daemon reports the turn
    // waiting for permission, so the projection restores a live, cancellable Stop
    // target AND an actionable card (this surface holds the token).
    #[test]
    fn switch_into_session_with_pending_gate_projects_live_and_actionable() {
        let authority = task44_authority(&[("request-1", "token-1", "session-a")]);
        let projection = build_session_projection(
            Some(SessionRunState::WaitingForPermission),
            &["request-1".to_string()],
            SnapshotSettle::Fresh(vec![task44_card("permission-1", Some("request-1"))]),
            &[],
            &PermissionView::default(),
            &[],
            &authority,
        );
        assert_eq!(projection.active_turn_id.as_deref(), Some("request-1"));
        assert!(projection.streaming);
        assert_eq!(projection.permission.cards().len(), 1);
        assert!(projection.permission.cards()[0].actionable);
    }

    // 2. Agent reconnect mid-tool: the daemon still reports Running with a live
    // request, so the reconnect projection keeps the turn live — no idle flicker,
    // Stop target intact.
    #[test]
    fn agent_reconnect_mid_tool_preserves_live_turn() {
        assert_eq!(
            project_live_turn(
                Some(SessionRunState::Running),
                &["request-live".to_string()]
            ),
            (Some("request-live".to_string()), true)
        );
        // Cancelling is still live and cancellable.
        assert_eq!(
            project_live_turn(
                Some(SessionRunState::Cancelling),
                &["request-live".to_string()]
            ),
            (Some("request-live".to_string()), true)
        );
    }

    // 3. Permission-stream reconnect: reconciling the durable registry keeps a
    // card actionable when this surface holds its request's token, and read-only
    // otherwise — authority is preserved across the reconcile, never discarded.
    #[test]
    fn permission_stream_reconnect_marks_cards_from_preserved_authority() {
        let authority = task44_authority(&[("request-1", "token-1", "session-a")]);
        let mut cards = vec![
            task44_card("permission-1", Some("request-1")),
            task44_card("permission-2", Some("request-remote")),
        ];
        mark_cards_actionable(&mut cards, &authority);
        assert!(cards[0].actionable);
        assert!(!cards[1].actionable);
        // The grant survives the reconcile (mark does not mutate authority).
        assert_eq!(
            resolve_decision_authority(Some("request-1"), "session-a", &authority),
            Some("token-1")
        );
    }

    // 4. Switch during an in-flight snapshot: focus moved to another session, so
    // the settling snapshot is discarded and cannot overwrite the new projection.
    #[test]
    fn switch_during_in_flight_snapshot_discards() {
        assert_eq!(
            admit_session_snapshot("session-a", 5, Some("session-b"), 5),
            SnapshotAdmission::Discard
        );
        assert_eq!(
            admit_session_snapshot("session-a", 5, None, 5),
            SnapshotAdmission::Discard
        );
        // Same session and generation still commit.
        assert_eq!(
            admit_session_snapshot("session-a", 5, Some("session-a"), 5),
            SnapshotAdmission::Commit
        );
    }

    // 5. An old-generation snapshot settling after a newer reconnect is discarded
    // by the generation guard even though the session id still matches.
    #[test]
    fn old_generation_snapshot_settling_is_discarded() {
        assert_eq!(
            admit_session_snapshot("session-a", 4, Some("session-a"), 5),
            SnapshotAdmission::Discard
        );
    }

    #[test]
    fn lifecycle_activity_retires_in_flight_snapshot() {
        assert_eq!(
            admit_activity_snapshot("session-a", 5, 8, Some("session-a"), 5, 9),
            SnapshotAdmission::Discard,
            "TurnStarted/TurnFinished after fetch start must retire the snapshot"
        );
        assert_eq!(
            admit_activity_snapshot("session-a", 5, 9, Some("session-a"), 5, 9),
            SnapshotAdmission::Commit
        );
    }

    #[test]
    fn sync_snapshot_requires_focus_generation_cycle_and_activity() {
        let admit = |session, generation, cycle, activity| {
            admit_sync_snapshot("session-a", 5, 7, 11, session, generation, cycle, activity)
        };

        assert_eq!(
            admit(Some("session-a"), 5, 7, 11),
            SnapshotAdmission::Commit
        );
        assert_eq!(
            admit(Some("session-b"), 5, 7, 11),
            SnapshotAdmission::Discard
        );
        assert_eq!(
            admit(Some("session-a"), 6, 7, 11),
            SnapshotAdmission::Discard
        );
        assert_eq!(
            admit(Some("session-a"), 5, 8, 11),
            SnapshotAdmission::Discard
        );
        assert_eq!(
            admit(Some("session-a"), 5, 7, 12),
            SnapshotAdmission::Discard
        );
    }

    #[test]
    fn register_before_accepted_preserves_only_terminal_snapshot_with_local_live_turn() {
        assert!(should_preserve_local_live_state(false, true));
        assert!(!should_preserve_local_live_state(true, true));
        assert!(!should_preserve_local_live_state(false, false));
        assert!(!should_preserve_local_live_state(true, false));
    }

    // 6. Deciding an old card after a new dispatch resolves the ORIGINAL turn's
    // token, never the latest one — the core of finding 4.
    #[test]
    fn deciding_old_card_after_new_dispatch_uses_original_token() {
        let mut authority = task44_authority(&[("request-1", "token-1", "session-a")]);
        // A newer turn is submitted and bound.
        authority.insert(
            "request-2".into(),
            DecisionGrant {
                token: "token-2".into(),
                session_id: "session-a".into(),
            },
        );
        // The still-open card from request-1 resolves token-1, not token-2.
        assert_eq!(
            resolve_decision_authority(Some("request-1"), "session-a", &authority),
            Some("token-1")
        );
        assert_eq!(
            resolve_decision_authority(Some("request-2"), "session-a", &authority),
            Some("token-2")
        );
        // A grant only authorizes a card in its OWN session (codex HOLD on af82ee8).
        assert_eq!(
            resolve_decision_authority(Some("request-1"), "other-session", &authority),
            None
        );
    }

    // 7. A card raised by another surface (no local grant) is read-only: no token
    // resolves, and the projection marks it non-actionable.
    #[test]
    fn remote_origin_card_is_read_only() {
        let authority = task44_authority(&[("request-local", "token-local", "session-a")]);
        assert_eq!(
            resolve_decision_authority(Some("request-remote"), "session-a", &authority),
            None
        );
        // A card with no request id at all is also non-actionable.
        assert_eq!(
            resolve_decision_authority(None, "session-a", &authority),
            None
        );
        let projection = build_session_projection(
            Some(SessionRunState::WaitingForPermission),
            &["request-remote".to_string()],
            SnapshotSettle::Fresh(vec![task44_card(
                "permission-remote",
                Some("request-remote"),
            )]),
            &[],
            &PermissionView::default(),
            &[],
            &authority,
        );
        assert!(!projection.permission.cards()[0].actionable);
        // The turn is still shown live/cancellable even though the card is
        // read-only for us.
        assert_eq!(projection.active_turn_id.as_deref(), Some("request-remote"));
        assert!(projection.streaming);
    }

    // 8. A terminal-state snapshot clears the live turn: completed, cancelled,
    // errored, stored, unknown, and absent state all project idle.
    #[test]
    fn terminal_state_snapshot_clears_live_turn() {
        for state in [
            SessionRunState::Completed,
            SessionRunState::Cancelled,
            SessionRunState::Errored,
            SessionRunState::Stored,
            SessionRunState::Unknown,
        ] {
            assert_eq!(
                project_live_turn(Some(state), &["request-done".to_string()]),
                (None, false),
                "state {state:?} must project idle"
            );
        }
        assert_eq!(
            project_live_turn(None, &["request-done".to_string()]),
            (None, false)
        );
        // A live state with no live request also projects idle (nothing to cancel).
        assert_eq!(
            project_live_turn(Some(SessionRunState::Running), &[]),
            (None, false)
        );
    }

    // 9. Cross-surface convergence: two independent surfaces (web + Tauri) fed the
    // SAME daemon snapshot converge on the SAME truthful live projection; only
    // per-card actionability differs, because only the submitter holds the token.
    // This reproduces the 13:45 Tauri/web split — both now show the turn live.
    #[test]
    fn cross_surface_projection_converges_with_divergent_authority() {
        let state = Some(SessionRunState::WaitingForPermission);
        let active_requests = ["request-1".to_string()];
        let cards = || vec![task44_card("permission-1", Some("request-1"))];

        // Web submitted the turn, so it holds the token.
        let web_authority = task44_authority(&[("request-1", "token-1", "session-a")]);
        // Tauri only observes the shared session — no local grant.
        let tauri_authority: HashMap<String, DecisionGrant> = HashMap::new();

        let web = build_session_projection(
            state,
            &active_requests,
            SnapshotSettle::Fresh(cards()),
            &[],
            &PermissionView::default(),
            &[],
            &web_authority,
        );
        let tauri = build_session_projection(
            state,
            &active_requests,
            SnapshotSettle::Fresh(cards()),
            &[],
            &PermissionView::default(),
            &[],
            &tauri_authority,
        );

        // Both converge on the same live turn state (the split is gone).
        assert_eq!(web.active_turn_id, tauri.active_turn_id);
        assert_eq!(web.active_turn_id.as_deref(), Some("request-1"));
        assert!(web.streaming && tauri.streaming);
        assert_eq!(web.permission.cards().len(), tauri.permission.cards().len());
        assert_eq!(
            web.permission.cards()[0].permission_id,
            tauri.permission.cards()[0].permission_id
        );
        // They differ only in who may act on the card.
        assert!(web.permission.cards()[0].actionable);
        assert!(!tauri.permission.cards()[0].actionable);
    }

    // Wire contract: the surface SessionDetail DTO decodes the daemon's atomic
    // `state` + `active_requests` (verified against ocean-core::SessionDetail),
    // and GET /v1/permissions decodes `request_id`. These are the fields the
    // projection commit reads.
    #[test]
    fn session_detail_decodes_daemon_state_and_active_requests() {
        let detail: SessionDetail = serde_json::from_value(serde_json::json!({
            "id": "session-a",
            "title": "Live session",
            "model": "grok",
            "state": "waiting_for_permission",
            "active_requests": ["request-1", "request-2"],
            "pending_permissions": ["permission-1"],
        }))
        .expect("session detail decodes");
        assert_eq!(detail.state, Some(SessionRunState::WaitingForPermission));
        assert_eq!(detail.active_requests, vec!["request-1", "request-2"]);
        assert_eq!(detail.pending_permissions, vec!["permission-1"]);

        // An older daemon omitting the fields is backward compatible.
        let legacy: SessionDetail = serde_json::from_value(serde_json::json!({
            "id": "session-a",
            "title": "Legacy",
            "model": "grok",
        }))
        .expect("legacy session detail decodes");
        assert_eq!(legacy.state, None);
        assert!(legacy.active_requests.is_empty());
    }

    #[test]
    fn permission_status_wire_decodes_request_id() {
        let wire: PermissionStatusWire = serde_json::from_value(serde_json::json!({
            "permission_id": "permission-1",
            "request_id": "request-1",
            "session_id": "session-a",
            "tool": "bash",
            "reason": "run check",
            "args": { "cmd": "ls" },
        }))
        .expect("permission status decodes");
        assert_eq!(wire.request_id.as_deref(), Some("request-1"));
    }

    #[test]
    fn prune_session_authority_drops_terminated_but_keeps_live_and_other_sessions() {
        let mut authority = task44_authority(&[
            ("request-live", "token-live", "session-a"),
            ("request-done", "token-done", "session-a"),
            ("request-other", "token-other", "session-b"),
        ]);
        let live: HashSet<String> = ["request-live".to_string()].into_iter().collect();
        prune_session_authority(&mut authority, "session-a", &live);
        // The live request's grant survives.
        assert!(authority.contains_key("request-live"));
        // The terminated request's grant in this session is pruned.
        assert!(!authority.contains_key("request-done"));
        // Another session's grant is untouched (its turn may still be running).
        assert!(authority.contains_key("request-other"));
    }

    #[test]
    fn matching_finish_clears_live_state() {
        let session_id = "session-task45-match";
        let turn_id = "turn-live";
        let daemon = daemon_with_session(session_id);

        start_turn(&daemon, session_id, turn_id);
        daemon.streaming.set(true);
        assert_eq!(
            daemon.active_turn_id.get_untracked().as_deref(),
            Some(turn_id)
        );

        finish_turn(&daemon, session_id, turn_id, "completed");

        assert!(
            !daemon.streaming.get_untracked(),
            "matching finish stops streaming"
        );
        assert_eq!(
            daemon.active_turn_id.get_untracked(),
            None,
            "matching finish clears the Stop target"
        );
        assert_eq!(daemon.status.get_untracked(), "connected");
    }

    #[test]
    fn stale_finish_does_not_clear_live_turn() {
        let session_id = "session-task45-stale";
        let live_turn = "turn-live";
        let stale_turn = "turn-stale";
        let daemon = daemon_with_session(session_id);

        // The current, live turn owns the Stop target and running projection.
        start_turn(&daemon, session_id, live_turn);
        daemon.streaming.set(true);

        // A superseded earlier turn finishes late.
        finish_turn(&daemon, session_id, stale_turn, "completed");

        assert!(
            daemon.streaming.get_untracked(),
            "stale finish must not stop the live turn's streaming"
        );
        assert_eq!(
            daemon.active_turn_id.get_untracked().as_deref(),
            Some(live_turn),
            "stale finish must not blank the live turn's Stop target"
        );
    }

    #[test]
    fn finish_with_no_active_turn_is_unchanged() {
        let session_id = "session-task45-noactive";
        let daemon = daemon_with_session(session_id);

        // No turn is live: active_turn_id is None and streaming is false.
        assert_eq!(daemon.active_turn_id.get_untracked(), None);
        assert!(!daemon.streaming.get_untracked());

        finish_turn(&daemon, session_id, "turn-ghost", "completed");

        assert_eq!(
            daemon.active_turn_id.get_untracked(),
            None,
            "a finish with no active turn leaves active_turn_id None"
        );
        assert!(
            !daemon.streaming.get_untracked(),
            "a finish with no active turn leaves streaming false"
        );
    }

    #[test]
    fn stale_failed_finish_does_not_steal_status() {
        let session_id = "session-task45-status";
        let live_turn = "turn-live";
        let stale_turn = "turn-stale";
        let daemon = daemon_with_session(session_id);

        start_turn(&daemon, session_id, live_turn);
        daemon.streaming.set(true);
        daemon.status.set("connected".into());

        // A stale turn fails late; it must not hijack the header status chip or
        // detail tooltip away from the still-live current turn.
        finish_turn_failed(&daemon, session_id, stale_turn, "provider error: boom");

        assert_eq!(
            daemon.status.get_untracked(),
            "connected",
            "stale failed finish must not steal the header status"
        );
        assert_eq!(
            daemon.status_detail.get_untracked(),
            None,
            "stale failed finish must not plant a status detail tooltip"
        );
        assert!(
            daemon.streaming.get_untracked(),
            "stale failed finish must not stop the live turn"
        );
        assert_eq!(
            daemon.active_turn_id.get_untracked().as_deref(),
            Some(live_turn),
            "stale failed finish must not blank the live Stop target"
        );
        // The stale turn still records its OWN transcript error block.
        let has_stale_error = daemon.turns.get_untracked().iter().any(|turn| {
            turn.turn_id.as_deref() == Some(stale_turn)
                && turn.blocks.iter().any(|b| {
                    matches!(b, Block::ToolCall { name, output, .. }
                        if name == "assistant_error" && output.contains("boom"))
                })
        });
        assert!(
            has_stale_error,
            "stale failed finish must still append its own transcript error block"
        );
    }

    /// Helpers to build the persisted shapes a reconnect replay reads.
    fn assistant_text(text: &str) -> SessionTranscriptEntry {
        SessionTranscriptEntry {
            role: "assistant".into(),
            text: text.into(),
            tool_call_id: None,
            tool_name: None,
            is_error: None,
        }
    }

    fn component_result(call_id: &str) -> SessionTranscriptEntry {
        // The daemon persists a component_render tool RESULT as a `tool` entry
        // whose text is only the summary; props live in the matching call.
        SessionTranscriptEntry {
            role: "tool".into(),
            text: format!("rendered component via {call_id}"),
            tool_call_id: Some(call_id.into()),
            tool_name: Some("component_render".into()),
            is_error: Some(false),
        }
    }

    fn render_call(call_id: &str, args: serde_json::Value) -> SessionToolContext {
        SessionToolContext {
            kind: "call".into(),
            tool_call_id: call_id.into(),
            tool_name: "component_render".into(),
            arguments: Some(args),
        }
    }

    /// OCEAN-382: a ComponentRender emitted live is SSE-only and never lands in
    /// the transcript text. On reconnect the rebuild must recover the component
    /// from the persisted tool-call args (id/kind/props) so maps/cards re-render
    /// instead of vanishing — and the summary tool-result must NOT show up as a
    /// raw ToolCall block.
    #[test]
    fn component_render_replays_from_tool_context_on_reconnect() {
        let transcript = vec![assistant_text("here is the map"), component_result("c-1")];
        let tool_context = vec![render_call(
            "c-1",
            serde_json::json!({
                "id": "map-1",
                "kind": "map",
                "props": { "center": { "lat": 1.0, "lng": 2.0 } }
            }),
        )];

        let (turns, _pinned) = turns_from_session_transcript(transcript, &tool_context);

        // Text + component fold into the one assistant turn (no synthetic split).
        assert_eq!(turns.len(), 1, "component attaches to the text turn");
        assert_eq!(
            turns[0].blocks.len(),
            2,
            "text + component, no ToolCall block"
        );
        match &turns[0].blocks[1] {
            Block::Component {
                component_id,
                kind,
                props,
            } => {
                assert_eq!(component_id, "map-1");
                assert_eq!(kind, "map");
                assert_eq!(props["center"]["lat"], 1.0);
            }
            other => panic!("expected replayed Component block, got {other:?}"),
        }
        // No ToolCall block should leak from the component_render summary result.
        assert!(
            !turns
                .iter()
                .flat_map(|t| &t.blocks)
                .any(|b| matches!(b, Block::ToolCall { .. })),
            "component_render result must not render as a raw tool call"
        );
    }

    /// replace:true overwrites the existing component in place rather than
    /// appending a duplicate — mirroring the live SSE reducer.
    #[test]
    fn component_replace_overwrites_in_place_on_replay() {
        let transcript = vec![
            assistant_text("rendering"),
            component_result("c-1"),
            component_result("c-2"),
        ];
        let tool_context = vec![
            render_call(
                "c-1",
                serde_json::json!({ "id": "prog", "kind": "progress", "props": { "value": 0.2 } }),
            ),
            render_call(
                "c-2",
                serde_json::json!({
                    "id": "prog", "kind": "progress",
                    "props": { "value": 0.9 }, "replace": true
                }),
            ),
        ];

        let (turns, _pinned) = turns_from_session_transcript(transcript, &tool_context);
        let components: Vec<_> = turns
            .iter()
            .flat_map(|t| &t.blocks)
            .filter_map(|b| match b {
                Block::Component {
                    component_id,
                    props,
                    ..
                } => Some((component_id.clone(), props.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(components.len(), 1, "replace must not duplicate the id");
        assert_eq!(components[0].0, "prog");
        assert_eq!(components[0].1["value"], 0.9, "latest props win");
    }

    /// A persisted component_unmount removes the component on replay.
    #[test]
    fn component_unmount_removes_on_replay() {
        let transcript = vec![
            assistant_text("show then hide"),
            component_result("c-1"),
            SessionTranscriptEntry {
                role: "tool".into(),
                text: "unmounted component 'x'".into(),
                tool_call_id: Some("c-2".into()),
                tool_name: Some("component_unmount".into()),
                is_error: Some(false),
            },
        ];
        let tool_context = vec![
            render_call(
                "c-1",
                serde_json::json!({ "id": "x", "kind": "callout", "props": {} }),
            ),
            SessionToolContext {
                kind: "call".into(),
                tool_call_id: "c-2".into(),
                tool_name: "component_unmount".into(),
                arguments: Some(serde_json::json!({ "id": "x" })),
            },
        ];

        let (turns, _pinned) = turns_from_session_transcript(transcript, &tool_context);
        assert!(
            !turns
                .iter()
                .flat_map(|t| &t.blocks)
                .any(|b| matches!(b, Block::Component { .. })),
            "unmounted component must not survive replay"
        );
        // The surviving assistant text turn stays.
        assert_eq!(turns.len(), 1);
        assert!(matches!(turns[0].blocks[0], Block::Text(_)));
    }

    /// Codex P2: an ERRORED component_render never rendered live, so reconnect
    /// must NOT replay it as a phantom component — the failed call stays
    /// represented as the ToolCall block the live session showed.
    #[test]
    fn errored_component_render_is_not_replayed() {
        let transcript = vec![
            assistant_text("trying to render"),
            SessionTranscriptEntry {
                role: "tool".into(),
                text: "component render failed".into(),
                tool_call_id: Some("c-1".into()),
                tool_name: Some("component_render".into()),
                is_error: Some(true),
            },
        ];
        let tool_context = vec![render_call(
            "c-1",
            serde_json::json!({ "id": "map-1", "kind": "map", "props": {} }),
        )];

        let (turns, _pinned) = turns_from_session_transcript(transcript, &tool_context);

        // No phantom component mounted from the failed call.
        assert!(
            !turns
                .iter()
                .flat_map(|t| &t.blocks)
                .any(|b| matches!(b, Block::Component { .. })),
            "errored component_render must not mount a phantom component"
        );
        // The failed call stays visible as an (expanded) ToolCall, as it was live.
        let tool_call = turns
            .iter()
            .flat_map(|t| &t.blocks)
            .find_map(|b| match b {
                Block::ToolCall {
                    name,
                    status,
                    expanded,
                    ..
                } => Some((name.clone(), *status, *expanded)),
                _ => None,
            })
            .expect("failed component_render must remain a ToolCall block");
        assert_eq!(tool_call.0, "component_render");
        assert_eq!(tool_call.1, ToolStatus::Err);
        assert!(tool_call.2, "failed tool call auto-expands");
    }

    /// Live SSE reducer parity: `replace:true` must update the mounted component
    /// in-place, not append a second block with the same id. Reconnect replay has
    /// its own coverage above; this guards the live path the web surface uses
    /// while a turn is streaming.
    #[test]
    fn live_component_replace_overwrites_in_place() {
        let session_id = "session-component-live-replace";
        let turn_id = "turn-with-component";
        let daemon = daemon_with_session(session_id);

        apply_test_event(
            &daemon,
            AgentEvent::AssistantTextDelta {
                session_id: session_id.to_string(),
                turn_id: turn_id.to_string(),
                delta: "rendering".to_string(),
            },
        );
        apply_test_event(
            &daemon,
            AgentEvent::ComponentRender {
                session_id: session_id.to_string(),
                component_id: "status".to_string(),
                kind: "progress".to_string(),
                props: serde_json::json!({ "label": "Working", "value": 0.2 }),
                replace: false,
            },
        );
        apply_test_event(
            &daemon,
            AgentEvent::ComponentRender {
                session_id: session_id.to_string(),
                component_id: "status".to_string(),
                kind: "progress".to_string(),
                props: serde_json::json!({ "label": "Working", "value": 0.9 }),
                replace: true,
            },
        );

        let turns = daemon.turns.get_untracked();
        assert_eq!(
            turns.len(),
            1,
            "component stays attached to the active turn"
        );
        assert_eq!(turns[0].turn_id.as_deref(), Some(turn_id));
        assert_eq!(turns[0].blocks.len(), 2, "text + one replaced component");
        assert!(matches!(turns[0].blocks[0], Block::Text(_)));
        match &turns[0].blocks[1] {
            Block::Component {
                component_id,
                kind,
                props,
            } => {
                assert_eq!(component_id, "status");
                assert_eq!(kind, "progress");
                assert_eq!(props["value"], 0.9, "latest props win");
            }
            other => panic!("expected replaced Component block, got {other:?}"),
        }
    }

    /// A live component can be unmounted before any assistant text exists (for
    /// example, a purely visual status card). The reducer creates a synthetic turn
    /// for the render; unmounting must prune the now-empty turn so stale blank
    /// assistant rows do not survive on the web surface.
    #[test]
    fn live_component_unmount_prunes_empty_component_turn() {
        let session_id = "session-component-live-unmount";
        let daemon = daemon_with_session(session_id);

        apply_test_event(
            &daemon,
            AgentEvent::ComponentRender {
                session_id: session_id.to_string(),
                component_id: "toast".to_string(),
                kind: "callout".to_string(),
                props: serde_json::json!({ "variant": "info", "body": "Heads up" }),
                replace: false,
            },
        );
        assert_eq!(
            daemon.turns.get_untracked().len(),
            1,
            "render creates a host turn"
        );

        apply_test_event(
            &daemon,
            AgentEvent::ComponentUnmount {
                session_id: session_id.to_string(),
                component_id: "toast".to_string(),
            },
        );

        assert!(
            daemon.turns.get_untracked().is_empty(),
            "unmounting the only component must remove the empty synthetic turn"
        );
    }

    /// Session scoping is load-bearing for multiple web surfaces sharing the
    /// daemon. A component frame for another session must not mutate this
    /// transcript, even if it reuses an id currently mounted here.
    #[test]
    fn live_component_event_for_other_session_is_ignored() {
        let active_session = "session-component-active";
        let other_session = "session-component-other";
        let daemon = daemon_with_session(active_session);

        apply_test_event(
            &daemon,
            AgentEvent::ComponentRender {
                session_id: active_session.to_string(),
                component_id: "shared-id".to_string(),
                kind: "progress".to_string(),
                props: serde_json::json!({ "value": 0.1 }),
                replace: false,
            },
        );
        apply_test_event(
            &daemon,
            AgentEvent::ComponentRender {
                session_id: other_session.to_string(),
                component_id: "shared-id".to_string(),
                kind: "progress".to_string(),
                props: serde_json::json!({ "value": 1.0 }),
                replace: true,
            },
        );

        let turns = daemon.turns.get_untracked();
        let component = turns
            .iter()
            .flat_map(|turn| &turn.blocks)
            .find_map(|block| match block {
                Block::Component {
                    component_id,
                    props,
                    ..
                } if component_id == "shared-id" => Some(props.clone()),
                _ => None,
            })
            .expect("active session component should still exist");
        assert_eq!(
            component["value"], 0.1,
            "other session event must be ignored"
        );
    }

    #[test]
    fn component_placement_defaults_inline_and_reads_pinned() {
        assert_eq!(
            component_placement(&serde_json::json!({})),
            ComponentPlacement::Inline
        );
        assert_eq!(
            component_placement(&serde_json::json!({ "placement": "inline" })),
            ComponentPlacement::Inline
        );
        assert_eq!(
            component_placement(&serde_json::json!({ "placement": "pinned", "value": 0.5 })),
            ComponentPlacement::Pinned
        );
        // Unknown values and non-strings fall back to inline, never panic.
        assert_eq!(
            component_placement(&serde_json::json!({ "placement": "dock" })),
            ComponentPlacement::Inline
        );
        assert_eq!(
            component_placement(&serde_json::json!({ "placement": 7 })),
            ComponentPlacement::Inline
        );
    }

    #[test]
    fn pinned_component_render_docks_to_rail_not_transcript() {
        let session_id = "session-pinned-render";
        let daemon = daemon_with_session(session_id);

        apply_test_event(
            &daemon,
            AgentEvent::ComponentRender {
                session_id: session_id.to_string(),
                component_id: "map-1".to_string(),
                kind: "map".to_string(),
                props: serde_json::json!({ "placement": "pinned", "markers": [] }),
                replace: false,
            },
        );

        let pinned = daemon.pinned_widgets.get_untracked();
        assert_eq!(pinned.len(), 1, "pinned render docks into the rail");
        assert_eq!(pinned[0].component_id, "map-1");
        assert_eq!(pinned[0].kind, "map");
        assert!(
            daemon.turns.get_untracked().is_empty(),
            "pinned component must not also appear inline"
        );
    }

    #[test]
    fn pinned_component_replace_upserts_in_place() {
        let session_id = "session-pinned-replace";
        let daemon = daemon_with_session(session_id);

        for value in [0.2_f64, 0.9] {
            apply_test_event(
                &daemon,
                AgentEvent::ComponentRender {
                    session_id: session_id.to_string(),
                    component_id: "prog".to_string(),
                    kind: "progress".to_string(),
                    props: serde_json::json!({ "placement": "pinned", "value": value }),
                    replace: true,
                },
            );
        }

        let pinned = daemon.pinned_widgets.get_untracked();
        assert_eq!(pinned.len(), 1, "replace upserts, never duplicates");
        assert_eq!(pinned[0].props["value"], 0.9, "latest props win");
    }

    #[test]
    fn pinned_component_unmount_removes_from_rail() {
        let session_id = "session-pinned-unmount";
        let daemon = daemon_with_session(session_id);

        apply_test_event(
            &daemon,
            AgentEvent::ComponentRender {
                session_id: session_id.to_string(),
                component_id: "card".to_string(),
                kind: "stat".to_string(),
                props: serde_json::json!({ "placement": "pinned" }),
                replace: false,
            },
        );
        assert_eq!(daemon.pinned_widgets.get_untracked().len(), 1);

        apply_test_event(
            &daemon,
            AgentEvent::ComponentUnmount {
                session_id: session_id.to_string(),
                component_id: "card".to_string(),
            },
        );
        assert!(
            daemon.pinned_widgets.get_untracked().is_empty(),
            "unmount removes the pinned widget"
        );
    }

    #[test]
    fn unpin_widget_removes_from_rail() {
        let daemon = daemon_with_session("session-pinned-unpin");
        apply_test_event(
            &daemon,
            AgentEvent::ComponentRender {
                session_id: "session-pinned-unpin".to_string(),
                component_id: "m".to_string(),
                kind: "map".to_string(),
                props: serde_json::json!({ "placement": "pinned" }),
                replace: false,
            },
        );
        assert_eq!(daemon.pinned_widgets.get_untracked().len(), 1);
        daemon.unpin_widget("m");
        assert!(
            daemon.pinned_widgets.get_untracked().is_empty(),
            "unpin_widget removes the docked card"
        );
    }

    #[test]
    fn session_create_response_decodes_without_ok_field() {
        // OCEAN-124 regression. The daemon's POST /v1/agent/sessions returns
        // exactly `{session_id, cwd, client_type}` — no `ok` field. The surface
        // decoder used to require `ok: bool`, so serde failed with
        // `missing field 'ok'`, the surface never got a session id, the first
        // turn was never POSTed, and web chat was 100% dead. This is the exact
        // 91-byte body the live tester's daemon returned (verified via curl).
        let body = r#"{"session_id":"s1","cwd":"/","client_type":"surface-web"}"#;
        let resp: AgentSessionCreateResponse =
            serde_json::from_str(body).expect("daemon create response must decode");
        // A missing `ok` defaults to true: a 200 with a session id IS success.
        assert!(resp.ok, "missing `ok` must default to true (success)");
        assert_eq!(resp.session_id.as_deref(), Some("s1"));
        assert_eq!(resp.cwd.as_deref(), Some("/"));
    }

    #[test]
    fn session_create_response_honors_explicit_ok_false() {
        // A future/error shape that DOES send `ok:false` is still respected so
        // the failure arm (which reads `error`) keeps working.
        let body = r#"{"ok":false,"error":"workspace not found"}"#;
        let resp: AgentSessionCreateResponse =
            serde_json::from_str(body).expect("error shape must decode");
        assert!(!resp.ok);
        assert!(resp.session_id.is_none());
        assert_eq!(resp.error.as_deref(), Some("workspace not found"));
    }

    #[test]
    fn agent_event_names_map_to_real_variants() {
        // Drift guard for the per-name SSE subscription. gloo-net's EventSource
        // only delivers frames whose `event:` name was explicitly subscribed,
        // so a name the daemon emits but `AGENT_EVENT_NAMES` omits is dropped at
        // the transport layer before serde's `#[serde(other)]` catch-all can
        // help. This test deserializes a minimal frame for each subscribed name
        // and asserts it lands on a concrete variant, NOT `AgentEvent::Other`.
        // A dead/typo'd name in the list would deserialize to `Other` and fail
        // here; a daemon-emitted name missing from the list is caught by the
        // exact-set check below paired with `expected_daemon_names`.
        for name in AGENT_EVENT_NAMES {
            let frame = serde_json::json!({ "type": name }).to_string();
            // Per-variant required fields differ, but the `type` tag alone is
            // enough for serde's internally-tagged enum to pick the variant
            // (missing fields fail only if the variant has non-defaulted
            // fields). To stay field-agnostic we just confirm the tag is a known
            // variant by checking it is NOT routed to `Other`: deserialize a
            // value with only `type`, falling back to a permissive object.
            let parsed: Result<AgentEvent, _> = serde_json::from_str(&frame);
            // Variants with required fields will error on the bare frame — that
            // still proves the tag is recognized (serde matched the variant and
            // only then complained about a missing field). `Other` never errors
            // on a bare `type`. So: a clean parse to a non-Other variant OR a
            // field-level error both mean "recognized name". Only a clean parse
            // to `Other` means "unknown name".
            if let Ok(AgentEvent::Other) = parsed {
                panic!(
                    "AGENT_EVENT_NAMES contains \"{name}\" but it deserializes to \
                     AgentEvent::Other — it matches no variant. Remove it or fix \
                     the spelling so it mirrors the daemon's emitted name.",
                );
            }
        }

        // Exact-set lock against the daemon's authoritative emitted names (the
        // `agent_event_type_name` match in
        // ocean-os/crates/ocean-daemon/src/main.rs:3782). Update BOTH lists in
        // lockstep when the daemon gains/loses an event — that is the whole
        // contract this ticket (OCEAN-102) makes drift-proof.
        let expected_daemon_names = [
            "turn_started",
            "assistant_text_delta",
            "thinking_delta",
            "tool_call_started",
            "tool_call_chunk",
            "tool_call_finished",
            "turn_finished",
            "session_created",
            "extension",
            "component_render",
            "component_unmount",
            "browser_activity",
            "surface_patch",
        ];
        let mut expected = expected_daemon_names.to_vec();
        expected.sort_unstable();
        let mut subscribed = AGENT_EVENT_NAMES.to_vec();
        subscribed.sort_unstable();
        assert_eq!(
            subscribed, expected,
            "AGENT_EVENT_NAMES must exactly match the daemon's emitted SSE event \
             names. A name only the daemon has = a SILENT transport-layer drop; a \
             name only the surface has = a dead subscription.",
        );
    }

    #[test]
    fn surface_patch_event_deserializes_into_variant_not_other() {
        // OCEAN-178 regression. Golden fixture of the daemon's EXACT wire shape
        // for `AgentTurnEvent::SurfacePatch` (ocean-agent-sdk, internally tagged
        // on `"type" = "surface_patch"`, `snake_case`; envelopes nest an
        // `op`-tagged patch). Captured verbatim from the daemon's serde output.
        // Before this fix the web `AgentEvent` had no `SurfacePatch` variant, so
        // this JSON routed into `AgentEvent::Other` and the agent's canvas patches
        // were dropped. (The transport-layer drop — `surface_patch` missing from
        // `AGENT_EVENT_NAMES` — is the other half, guarded by the allow-list
        // test above; this test guards the serde half.)
        let raw = r#"{
  "type": "surface_patch",
  "session_id": "11111111-1111-4111-8111-111111111111",
  "turn_id": "22222222-2222-4222-8222-222222222222",
  "canvas_id": "canvas:main",
  "patches": [
    {
      "patch_id": "patch-1",
      "session_id": "11111111-1111-4111-8111-111111111111",
      "surface_id": "gpui:local",
      "canvas_id": "canvas:main",
      "actor": { "kind": "agent", "id": "sage" },
      "created_at_ms": 1725000000000,
      "patch": {
        "op": "upsert_component",
        "component": {
          "id": "brief-1",
          "kind": "brief_card",
          "rect": { "x": 420.0, "y": 120.0, "w": 320.0, "h": 220.0 },
          "content": { "body": "Draft", "title": "Sales Brief" },
          "metadata": { "source": "longhouse.sales" }
        }
      }
    }
  ]
}"#;

        let event: AgentEvent =
            serde_json::from_str(raw).expect("daemon surface_patch JSON must deserialize");

        // Must NOT fall through to the `Other` catch-all — that was the bug.
        let AgentEvent::SurfacePatch {
            session_id,
            turn_id,
            canvas_id,
            patches,
        } = event
        else {
            panic!("expected SurfacePatch, got a different / Other variant — wire shape drifted");
        };

        assert_eq!(session_id, "11111111-1111-4111-8111-111111111111");
        assert_eq!(turn_id, "22222222-2222-4222-8222-222222222222");
        assert_eq!(canvas_id, CanvasId::new("canvas:main"));
        assert_eq!(patches.len(), 1);

        let envelope = &patches[0];
        assert_eq!(envelope.patch_id, PatchId::new("patch-1"));
        assert_eq!(envelope.canvas_id, CanvasId::new("canvas:main"));
        assert_eq!(envelope.surface_id, SurfaceId::new("gpui:local"));
        assert_eq!(envelope.actor.kind, "agent");
        assert_eq!(envelope.actor.id.as_deref(), Some("sage"));
        assert_eq!(envelope.created_at_ms, 1_725_000_000_000);

        let SurfacePatch::UpsertComponent { component } = &envelope.patch else {
            panic!("expected UpsertComponent op");
        };
        assert_eq!(component.id, ComponentId::new("brief-1"));
        assert_eq!(component.kind, "brief_card");
        let rect = component.rect.expect("rect present");
        assert_eq!(
            (rect.x, rect.y, rect.w, rect.h),
            (420.0, 120.0, 320.0, 220.0)
        );
        assert_eq!(component.content["title"], "Sales Brief");

        // The one-line summary the web panel renders is derived correctly.
        assert_eq!(
            summarize_surface_patch(&envelope.patch),
            "upsert_component brief-1 (brief_card)"
        );
    }

    #[test]
    fn control_event_parses_permission_request_from_full_envelope() {
        // The /v1/events stream serializes the FULL envelope: the flattened
        // OceanEvent fields PLUS the envelope's permission_id / session_id.
        let data = serde_json::json!({
            "id": "evt-1",
            "at": "2026-06-05T00:00:00Z",
            "session_id": "sess-abc",
            "request_id": "req-1",
            "permission_id": "perm-xyz",
            "type": "permission_request",
            "tool": "bash",
            "reason": "permission required for bash",
            "args": { "cmd": "rm -rf build" }
        })
        .to_string();
        let evt: ControlEvent = serde_json::from_str(&data).unwrap();
        match evt {
            ControlEvent::PermissionRequest {
                permission_id,
                session_id,
                request_id,
                tool,
                ..
            } => {
                assert_eq!(permission_id.as_deref(), Some("perm-xyz"));
                assert_eq!(session_id.as_deref(), Some("sess-abc"));
                // TASK-44: the envelope's request_id is decoded so the card can
                // resolve its decision authority.
                assert_eq!(request_id.as_deref(), Some("req-1"));
                assert_eq!(tool, "bash");
            }
            other => panic!("expected PermissionRequest, got {other:?}"),
        }
    }

    #[test]
    fn control_event_parses_permission_decision() {
        let data = serde_json::json!({
            "id": "evt-2",
            "at": "2026-06-05T00:00:00Z",
            "session_id": "sess-abc",
            "permission_id": "perm-xyz",
            "type": "permission_decision",
            "allowed": true
        })
        .to_string();
        let evt: ControlEvent = serde_json::from_str(&data).unwrap();
        assert!(matches!(evt, ControlEvent::PermissionDecision { .. }));
    }

    #[test]
    fn control_event_unrelated_type_is_other() {
        let data = r#"{"type":"assistant_delta","text":"hi"}"#;
        let evt: ControlEvent = serde_json::from_str(data).unwrap();
        assert!(matches!(evt, ControlEvent::Other));
    }

    #[test]
    fn summarize_args_renders_object_as_key_value_lines() {
        let args = serde_json::json!({ "path": "/tmp/x", "contents": "hi" });
        let summary = summarize_args(&args);
        assert!(summary.contains("path: /tmp/x"));
        assert!(summary.contains("contents: hi"));
    }

    #[test]
    fn summarize_args_null_is_empty() {
        assert_eq!(summarize_args(&Value::Null), "");
    }

    #[test]
    fn thinking_level_values_match_daemon_serialization() {
        // These are the exact lowercase strings the daemon's `ThinkingLevel`
        // serde enum deserializes (off/minimal/low/medium/high/xhigh). The
        // composer's selector emits these and they flow straight onto
        // `AgentTurnRequest::thinking_level`; this same list also gates which
        // persisted value survives a reload (see load_persisted_thinking_level).
        assert_eq!(
            THINKING_LEVELS,
            &["off", "minimal", "low", "medium", "high", "xhigh"],
        );
    }

    #[test]
    fn thinking_level_restore_filter_accepts_all_offered_levels() {
        // Every level the composer dropdown offers must pass the restore filter,
        // or selecting it then reloading silently drops it back to the daemon
        // default. Regression guard for the OCEAN-202 minimal/xhigh additions:
        // the filter is `THINKING_LEVELS.contains(&v)`, so assert each offered
        // value is contained. (The dropdown's empty "" = no override is not a
        // stored level and is intentionally absent.)
        for level in ["off", "minimal", "low", "medium", "high", "xhigh"] {
            assert!(
                THINKING_LEVELS.contains(&level),
                "thinking level `{level}` is offered by the composer but would be \
                 filtered out of localStorage on reload",
            );
        }
        // And a garbage value is still rejected by the same filter.
        assert!(!THINKING_LEVELS.contains(&"turbo"));
    }

    /// TASK-76: the surface must NEVER put page-controlled browser data in
    /// `guidance`. Tab titles are authored by whatever site the operator has
    /// open; the daemon HONORS guidance and renders it under "Operator
    /// guidance for this turn:", so anything here arrives wearing operator
    /// authority at a runtime whose tools are ungated.
    ///
    /// This pins the SOURCE, not just a value: the browser-context helpers
    /// that used to build those strings are gone, so a future change that
    /// reintroduces prose from `__ocean_active_tab` / `__ocean_open_tabs` has
    /// to re-add a guidance producer — and this test names the invariant it
    /// would be violating. Browser state travels only via the structured
    /// `client_context`, which the daemon sanitizes.
    /// TASK-77: session ids must be percent-encoded before they land in a
    /// daemon URL. Not a live traversal today (ids come from the daemon or
    /// extension-origin localStorage), but raw interpolation into daemon paths
    /// is the exact pattern that produced the CONFIRMED proxy traversal in
    /// TASK-71 — this keeps the surface consistent with rooms.rs instead of
    /// leaving one module disciplined and its neighbour not.
    #[test]
    fn session_ids_are_percent_encoded_in_daemon_urls() {
        // The characters that would break out of a path segment.
        assert_eq!(encode_path_segment("abc-123_x.y~z"), "abc-123_x.y~z");
        assert_eq!(encode_path_segment("a/b"), "a%2Fb");
        assert_eq!(encode_path_segment(".."), "..");
        assert_eq!(encode_path_segment("a?b#c"), "a%3Fb%23c");
        assert_eq!(encode_path_segment("a b"), "a%20b");

        // And no production call site may interpolate an id straight into a
        // daemon URL. TASK-84: the original version of this check matched a
        // single literal binding (`{id}`) and therefore missed three sites
        // that bound `{session_id}` — two paths and one QUERY STRING. It now
        // checks every binding name we use for a session id, in both the path
        // and query positions.
        //
        // Needles are assembled at runtime: written as literals they would
        // match this test's own source and the assertion would pass on itself.
        // Comments are stripped first: a doc comment that DESCRIBES a URL
        // shape (`POST /v1/agent/sessions/{id}/messages`) is prose, not a call
        // site, and flagging it is the same self-reference trap that has bitten
        // these source-assertion tests repeatedly. Only executable lines count.
        let src_raw = include_str!("daemon.rs");
        let src: String = src_raw
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let src = src.as_str();
        let (open, close) = ("{", "}");
        let mut raw_forms = Vec::new();
        for binding in ["id", "session_id"] {
            // Path position, both URL shapes in use.
            raw_forms.push(format!("/v1/sessions/{open}{binding}{close}"));
            raw_forms.push(format!("/v1/agent/sessions/{open}{binding}{close}"));
            // Query position — an `&` or `#` in an id splits the query.
            raw_forms.push(format!("session_id={open}{binding}{close}"));
        }
        let offenders: Vec<&String> = raw_forms
            .iter()
            .filter(|form| src.contains(form.as_str()))
            .collect();
        assert!(
            offenders.is_empty(),
            "session ids must go through encode_path_segment, not raw \
             interpolation — found {offenders:?}",
        );
    }

    #[test]
    fn guidance_never_carries_browser_data() {
        let src = include_str!("daemon.rs");
        // Needles are assembled at runtime: a literal here would match THIS
        // test's own source and the assertion would fail on itself.
        let suffix = "_guidance";
        for stem in ["browser_context", "active_tab", "open_tabs"] {
            let dead = format!("fn {stem}{suffix}");
            let dead = dead.as_str();
            assert!(
                !src.contains(dead),
                "{dead} rebuilds the unsanitized guidance channel TASK-76 removed; \
                 ship browser state through client_context (daemon-sanitized) instead",
            );
        }
        // The real behavioral pin: the production turn construction calls
        // `turn_guidance()`, and that function returns None. This REPLACES a
        // source assertion (`src.contains("guidance: None,")`) that matched
        // its own text and therefore could not fail — it would have passed
        // even with `guidance: Some(page_controlled_text)` at the call site,
        // while appearing to guard a prompt-injection boundary.
        assert_eq!(
            turn_guidance(),
            None,
            "guidance is rendered under \"Operator guidance for this turn:\" — \
             it must never carry page-controlled text",
        );
        let call = format!("guidance: turn_{}()", "guidance");
        assert!(
            src.contains(call.as_str()),
            "the turn body must route guidance through turn_guidance()",
        );
        let wiring = format!("guidance: active_tab{suffix}");
        assert!(
            !src.contains(wiring.as_str()),
            "the browser-guidance wiring must stay removed",
        );
    }

    #[test]
    fn turn_request_omits_overrides_when_none() {
        // Defaults preserved: with no per-turn overrides, the override fields are
        // skipped from the wire shape entirely (daemon applies global defaults).
        let body = AgentTurnRequest {
            prompt: "hi",
            cwd: "/",
            session_id: Some("s1"),
            project_id: None,
            client_type: None,
            guidance: None,
            room_id: None,
            thinking_level: None,
            model_id: None,
            images: None,
            decision_token: None,
            client_context: None,
            agent: None,
            canvas: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(!json.contains("thinking_level"));
        assert!(!json.contains("model_id"));
        // OCEAN-378: no agent selected → the field is skipped entirely, so the
        // daemon's `agent: Option<String>` stays `None` and the surface profile
        // is unchanged (back-compat: wire shape byte-for-byte the same).
        assert!(!json.contains("agent"));
    }

    #[test]
    fn turn_request_emits_agent_when_set() {
        // OCEAN-378: a selected folder-as-agent serializes as the bare `agent`
        // key the daemon's `AgentTurnRequest::agent` deserializes, so the turn
        // runs under `<agents_root>/<agent>/instructions.md`.
        let body = AgentTurnRequest {
            prompt: "hi",
            cwd: "/",
            session_id: Some("s1"),
            project_id: None,
            client_type: None,
            guidance: None,
            room_id: None,
            thinking_level: None,
            model_id: None,
            images: None,
            decision_token: None,
            client_context: None,
            agent: Some("foo"),
            canvas: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains(r#""agent":"foo""#));
    }

    #[test]
    fn turn_request_omits_agent_when_none() {
        // Back-compat guard: with `agent: None` the key is absent, so an older
        // daemon (which never saw the field) deserializes the payload unchanged.
        let body = AgentTurnRequest {
            prompt: "hi",
            cwd: "/",
            session_id: Some("s1"),
            project_id: None,
            client_type: None,
            guidance: None,
            room_id: None,
            thinking_level: None,
            model_id: None,
            images: None,
            decision_token: None,
            client_context: None,
            agent: None,
            canvas: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(!json.contains("agent"));
    }

    #[test]
    fn turn_request_emits_selected_overrides() {
        let body = AgentTurnRequest {
            prompt: "hi",
            cwd: "/",
            session_id: Some("s1"),
            project_id: None,
            client_type: None,
            guidance: None,
            room_id: None,
            thinking_level: Some("high"),
            model_id: Some("claude-opus-4-8"),
            images: None,
            decision_token: None,
            client_context: None,
            agent: None,
            canvas: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains(r#""thinking_level":"high""#));
        assert!(json.contains(r#""model_id":"claude-opus-4-8""#));
    }

    #[test]
    fn turn_request_omits_images_when_none() {
        // OCEAN-138: with nothing captured/picked, `images` is skipped entirely
        // so the daemon's `images: Option<Vec<TurnImage>>` stays `None` and
        // existing text-only turns are byte-for-byte unchanged.
        let body = AgentTurnRequest {
            prompt: "hi",
            cwd: "/",
            session_id: Some("s1"),
            project_id: None,
            client_type: None,
            guidance: None,
            room_id: None,
            thinking_level: None,
            model_id: None,
            images: None,
            decision_token: None,
            client_context: None,
            agent: None,
            canvas: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(!json.contains("images"));
    }

    #[test]
    fn turn_request_emits_images_in_daemon_wire_shape() {
        // OCEAN-138: a staged image serializes to the EXACT shape the daemon's
        // `TurnImage` (ocean-agent-sdk) deserializes — `{mime_type, data}` —
        // under the `images` array, so it reaches the model as a Content::Image.
        let body = AgentTurnRequest {
            prompt: "what is on screen?",
            cwd: "/",
            session_id: Some("s1"),
            project_id: None,
            client_type: None,
            guidance: None,
            room_id: None,
            thinking_level: None,
            model_id: None,
            images: Some(vec![TurnImage {
                mime_type: "image/png".into(),
                data: "data:image/png;base64,AAAA".into(),
            }]),
            decision_token: None,
            client_context: None,
            agent: None,
            canvas: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        // Round-trip through serde_json::Value to assert structure exactly.
        let v: Value = serde_json::from_str(&json).unwrap();
        let img = &v["images"][0];
        assert_eq!(img["mime_type"], "image/png");
        assert_eq!(img["data"], "data:image/png;base64,AAAA");
        // No stray fields leak onto the wire image object.
        assert_eq!(img.as_object().unwrap().len(), 2);
    }

    #[test]
    fn parse_data_url_extracts_mime_and_keeps_full_url() {
        let img =
            parse_data_url("data:image/png;base64,iVBORw0KGgo=").expect("png data url must parse");
        assert_eq!(img.mime_type, "image/png");
        assert_eq!(img.data, "data:image/png;base64,iVBORw0KGgo=");
    }

    #[test]
    fn parse_data_url_rejects_non_base64_and_garbage() {
        // A plain URL, a non-base64 data URL, and an empty mime all fail closed
        // so we never stage an attachment the daemon can't turn into an image.
        assert!(parse_data_url("https://example.com/x.png").is_none());
        assert!(parse_data_url("data:image/png,notbase64").is_none());
        assert!(parse_data_url("data:;base64,AAAA").is_none());
        assert!(parse_data_url("garbage").is_none());
    }

    #[test]
    fn voice_client_type_matches_daemon_routing_string() {
        // OCEAN-181: the daemon routes its concise/speakable voice system prompt
        // (`voice_surface_prompt`) on `client_type == "leo-voice"` exactly
        // (ocean-os crates/ocean-agent/src/lib.rs). If this constant drifts, the
        // voice prompt silently goes unreachable again — the original bug. Lock
        // it to the literal the daemon checks.
        assert_eq!(VOICE_CLIENT_TYPE, "leo-voice");
    }

    #[test]
    fn voice_client_type_is_distinct_from_surface_identities() {
        // Guard that voice tagging did NOT collapse onto the typed/web/extension
        // identities. `surface_client_type()` itself can't run off-wasm (it
        // touches `web_sys`), so we assert against its two possible literals
        // directly: voice must be neither.
        assert_ne!(VOICE_CLIENT_TYPE, "surface-web");
        assert_ne!(VOICE_CLIENT_TYPE, "surface-extension");
    }

    #[test]
    fn surface_tauri_is_distinct_from_voice_and_other_surfaces() {
        // Guard that the Tauri surface identity does NOT collide with the voice
        // routing tag or the other surface identities. `running_as_tauri()` and
        // `surface_client_type()` can't run off-wasm (they touch `web_sys`), so
        // we assert against the literal "surface-tauri" directly.
        assert_ne!("surface-tauri", "leo-voice");
        assert_ne!("surface-tauri", "surface-web");
        assert_ne!("surface-tauri", "surface-extension");
    }

    #[test]
    fn voice_send_tags_turn_leo_voice_on_the_wire() {
        // OCEAN-181 core assertion. `send_voice_prompt` dispatches with
        // `VOICE_CLIENT_TYPE`, which lands on `AgentTurnRequest::client_type`.
        // Build that exact wire body the way dispatch does for the voice path
        // and assert it serializes `client_type=leo-voice`.
        let body = AgentTurnRequest {
            prompt: "hey",
            cwd: "/",
            session_id: Some("s1"),
            project_id: None,
            client_type: Some(VOICE_CLIENT_TYPE),
            guidance: None,
            room_id: None,
            thinking_level: None,
            model_id: None,
            images: None,
            decision_token: None,
            client_context: None,
            agent: None,
            canvas: None,
        };
        let v: Value = serde_json::from_str(&serde_json::to_string(&body).unwrap()).unwrap();
        assert_eq!(v["client_type"], "leo-voice");
    }

    #[test]
    fn typed_send_tags_turn_surface_web_on_the_wire() {
        // The companion to the voice case: a normal (typed) send dispatches with
        // `surface_client_type()`, which for the web PWA is `surface-web`. The
        // wire body must carry that unchanged — proving the voice change did not
        // touch the typed path. (`surface_client_type()` can't be called off-wasm
        // because it reads `web_sys`, so we pin the web literal it returns.)
        let body = AgentTurnRequest {
            prompt: "hey",
            cwd: "/",
            session_id: Some("s1"),
            project_id: None,
            client_type: Some("surface-web"),
            guidance: None,
            room_id: None,
            thinking_level: None,
            model_id: None,
            images: None,
            decision_token: None,
            client_context: None,
            agent: None,
            canvas: None,
        };
        let v: Value = serde_json::from_str(&serde_json::to_string(&body).unwrap()).unwrap();
        assert_eq!(v["client_type"], "surface-web");
    }

    #[test]
    fn voice_first_session_create_uses_surface_identity_not_leo_voice() {
        // Codex P2 regression on #45 (OCEAN-181). When the FIRST interaction on a
        // fresh surface is a voice transcript, the per-turn tag threaded through
        // dispatch is "leo-voice" — but the SESSION-CREATE body must still carry
        // the stable surface MEDIUM (surface-web / surface-extension), per the
        // AGENTS.md session contract. A session's client_type is a surface
        // identity, not a per-turn routing tag; voice is a mode OF the web /
        // extension surface, not its own surface. dispatch_prompt builds this
        // body with surface_client_type() (web literal off-wasm), NOT the
        // threaded client_type. Lock that: even on a voice-first session create,
        // the wire client_type must NOT be leo-voice.
        let body = AgentSessionCreateRequest {
            title: Some("hey there"),
            workspace_root: "/",
            project_id: None,
            // Mirrors the session-create call site: surface_client_type() — which
            // off-wasm is surface-web — regardless of the (voice) turn tag.
            client_type: Some("surface-web"),
        };
        let v: Value = serde_json::from_str(&serde_json::to_string(&body).unwrap()).unwrap();
        assert_eq!(v["client_type"], "surface-web");
        assert_ne!(
            v["client_type"], "leo-voice",
            "session client_type must be the surface medium, never the per-turn voice tag",
        );
    }

    /// OCEAN-314: `decision_token` serializes onto the wire when set, and is
    /// omitted when `None` so legacy daemons (pre-OCEAN-185) are unaffected.
    #[test]
    fn turn_request_emits_decision_token_when_set() {
        let token = "deadbeef".repeat(8); // 64 hex chars, like mint_decision_token
        let body = AgentTurnRequest {
            prompt: "hi",
            cwd: "/",
            session_id: Some("s1"),
            project_id: None,
            client_type: None,
            guidance: None,
            room_id: None,
            thinking_level: None,
            model_id: None,
            images: None,
            decision_token: Some(&token),
            client_context: None,
            agent: None,
            canvas: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(
            json.contains(r#""decision_token":"deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef""#),
            "decision_token must appear on the wire when set"
        );
    }

    #[test]
    fn turn_request_omits_decision_token_when_none() {
        let body = AgentTurnRequest {
            prompt: "hi",
            cwd: "/",
            session_id: Some("s1"),
            project_id: None,
            client_type: None,
            guidance: None,
            room_id: None,
            thinking_level: None,
            model_id: None,
            images: None,
            decision_token: None,
            client_context: None,
            agent: None,
            canvas: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(
            !json.contains("decision_token"),
            "decision_token must be absent from the wire when None (legacy compat)"
        );
        // OCEAN-377: with no browser state, client_context is omitted entirely
        // so the wire shape is byte-for-byte unchanged for non-extension turns.
        assert!(
            !json.contains("client_context"),
            "client_context must be absent from the wire when None"
        );
    }

    #[test]
    fn turn_request_emits_client_context_in_daemon_wire_shape() {
        // OCEAN-377: a populated client_context serializes to the EXACT shape the
        // daemon's `ocean_agent_sdk::ClientContext` deserializes —
        // `{client_type, browser: {active_tab_url, active_tab_title, tabs:
        // [{url, title, active}]}}` — so the agent sees the structured browser
        // state, not just the freeform guidance lines.
        let body = AgentTurnRequest {
            prompt: "what's on this page?",
            cwd: "/",
            session_id: Some("s1"),
            project_id: None,
            client_type: Some("surface-extension"),
            guidance: None,
            room_id: None,
            thinking_level: None,
            model_id: None,
            images: None,
            decision_token: None,
            client_context: Some(ClientContext {
                client_type: "surface-extension".into(),
                browser: Some(BrowserContext {
                    active_tab_url: Some("https://example.com/a".into()),
                    active_tab_title: Some("Page A".into()),
                    tabs: vec![
                        BrowserTab {
                            url: "https://example.com/a".into(),
                            title: "Page A".into(),
                            active: true,
                        },
                        BrowserTab {
                            url: "https://example.com/b".into(),
                            title: "Page B".into(),
                            active: false,
                        },
                    ],
                }),
            }),
            agent: None,
            canvas: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        let ctx = &v["client_context"];
        assert_eq!(ctx["client_type"], "surface-extension");
        let browser = &ctx["browser"];
        assert_eq!(browser["active_tab_url"], "https://example.com/a");
        assert_eq!(browser["active_tab_title"], "Page A");
        let tabs = browser["tabs"].as_array().unwrap();
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0]["url"], "https://example.com/a");
        assert_eq!(tabs[0]["title"], "Page A");
        assert_eq!(tabs[0]["active"], true);
        assert_eq!(tabs[1]["active"], false);
        // No stray fields leak onto the wire context objects.
        assert_eq!(ctx.as_object().unwrap().len(), 2); // client_type + browser
        assert_eq!(tabs[0].as_object().unwrap().len(), 3); // url + title + active
    }

    #[test]
    fn duplicate_url_tabs_keep_only_the_truly_active_one_flagged() {
        // OCEAN-377 P2: when the loader carries Chrome's own per-tab `active`
        // flag (snapshot_has_active_flag = true), we trust it verbatim. Two tabs
        // share a URL but only the genuinely-active one is flagged — a URL match
        // would (incorrectly) light up both.
        let mut tabs = vec![
            BrowserTab {
                url: "https://example.com/dup".into(),
                title: "First copy".into(),
                active: false,
            },
            BrowserTab {
                url: "https://example.com/dup".into(),
                title: "Second copy (focused)".into(),
                active: true,
            },
        ];
        flag_active_tab_fallback(&mut tabs, true, Some("https://example.com/dup"));
        assert!(
            !tabs[0].active,
            "the non-focused duplicate must stay inactive"
        );
        assert!(
            tabs[1].active,
            "only the truly-active duplicate stays flagged"
        );
        assert_eq!(
            tabs.iter().filter(|t| t.active).count(),
            1,
            "exactly one tab is active even when URLs collide"
        );
    }

    #[test]
    fn flagless_snapshot_falls_back_to_a_single_url_match() {
        // OCEAN-377 P2 fallback: an older loader publishes no per-tab `active`
        // flag (snapshot_has_active_flag = false). We flag exactly ONE tab — the
        // first URL match against the active-tab URL — not every duplicate.
        let mut tabs = vec![
            BrowserTab {
                url: "https://example.com/dup".into(),
                title: "First copy".into(),
                active: false,
            },
            BrowserTab {
                url: "https://example.com/dup".into(),
                title: "Second copy".into(),
                active: false,
            },
            BrowserTab {
                url: "https://example.com/other".into(),
                title: "Other".into(),
                active: false,
            },
        ];
        flag_active_tab_fallback(&mut tabs, false, Some("https://example.com/dup"));
        assert!(tabs[0].active, "first URL match is flagged");
        assert!(!tabs[1].active, "the second duplicate is NOT flagged");
        assert!(!tabs[2].active);
        assert_eq!(tabs.iter().filter(|t| t.active).count(), 1);
    }

    #[test]
    fn voice_turn_browser_context_carries_surface_identity_not_routing_tag() {
        // OCEAN-377 P2: a voice turn threads the routing tag `leo-voice` as the
        // FLAT turn client_type, but the browser context the daemon keys on must
        // carry the SURFACE identity (surface-extension), or
        // apply_browser_context() won't recognize it as a browser surface and
        // drops the snapshot. The call site passes surface_client_type() (not the
        // flat routing tag) into the assembler, which is what this asserts: the
        // context's client_type is whatever surface identity we hand it,
        // regardless of any voice routing.
        let ctx = assemble_client_context(
            "surface-extension",
            Some("https://example.com/a".into()),
            Some("Page A".into()),
            vec![BrowserTab {
                url: "https://example.com/a".into(),
                title: "Page A".into(),
                active: true,
            }],
        )
        .expect("browser state present → context built");
        assert_eq!(
            ctx.client_type, "surface-extension",
            "client_context.client_type must be the surface identity, never the \
             voice routing tag `leo-voice`"
        );
        assert_ne!(ctx.client_type, VOICE_CLIENT_TYPE);

        // And the assembler omits the context entirely when there's no browser
        // state, so non-extension/voice-only turns leave the wire shape unchanged.
        assert!(assemble_client_context(VOICE_CLIENT_TYPE, None, None, vec![]).is_none());
    }

    #[test]
    fn https_origin_falls_back_to_same_origin_not_mixed_content() {
        // The deployed page is https — the fallback MUST be same-origin (empty
        // → relative /v1/... via the proxy), never http://host:4780 which the
        // browser blocks as mixed content (empty model/project pickers).
        assert_eq!(daemon_url_fallback("https:", "ocean.agentsworld.org"), "");
        assert_eq!(
            daemon_url_fallback("https:", "ocean.agentsworld.org:8790"),
            ""
        );
    }

    #[test]
    fn http_lan_dev_uses_host_on_4780() {
        assert_eq!(
            daemon_url_fallback("http:", "192.168.1.50:8790"),
            "http://192.168.1.50:4780"
        );
        // `localhost` is pinned to the IPv4 loopback: macOS resolves it
        // ::1-first while the daemon listens on 127.0.0.1 only, and
        // WKWebView's v6->v4 fallback is unreliable (EventSource especially).
        assert_eq!(
            daemon_url_fallback("http:", "localhost:8790"),
            DEFAULT_DAEMON_URL
        );
        // The Tauri shell (tauri://localhost) takes the same pin.
        assert_eq!(
            daemon_url_fallback("tauri:", "localhost"),
            DEFAULT_DAEMON_URL
        );
        // `localhost` is pinned to the IPv4 loopback: macOS resolves it
        // ::1-first while the daemon listens on 127.0.0.1 only, and
        // WKWebView's v6->v4 fallback is unreliable (EventSource especially).
        assert_eq!(
            daemon_url_fallback("http:", "localhost:8790"),
            DEFAULT_DAEMON_URL
        );
        // The Tauri shell (tauri://localhost) takes the same pin.
        assert_eq!(
            daemon_url_fallback("tauri:", "localhost"),
            DEFAULT_DAEMON_URL
        );
    }

    #[test]
    fn no_host_falls_back_to_loopback_default() {
        assert_eq!(daemon_url_fallback("http:", ""), DEFAULT_DAEMON_URL);
    }

    #[test]
    fn newest_global_session_fetch_ticket_alone_can_replace_the_list() {
        let daemon = Daemon::dummy();
        let sentinel = SessionSummary {
            id: "sentinel".into(),
            title: "Sentinel".into(),
            cwd: "/sentinel".into(),
            workspace_root: Some("/sentinel".into()),
            owning_project: None,
            git_branch: None,
            turn_count: 1,
            active_turn: None,
            active_state: None,
            updated_at: "2026-07-17T00:00:00Z".into(),
        };
        daemon.session_list.set(vec![sentinel.clone()]);

        let thin_spawner_ticket = daemon.claim_session_fetch_ticket();
        let panel = daemon.clone();
        let panel_ticket = panel.claim_session_fetch_ticket();

        assert!(!daemon.store_sessions_if_current(thin_spawner_ticket, Vec::new()));
        assert_eq!(daemon.session_list.get_untracked()[0].id, "sentinel");
        assert!(panel.store_sessions_if_current(panel_ticket, Vec::new()));
        assert!(daemon.session_list.get_untracked().is_empty());

        let panel_ticket = panel.claim_session_fetch_ticket();
        let thin_spawner_ticket = daemon.claim_session_fetch_ticket();
        assert!(!panel.store_sessions_if_current(panel_ticket, vec![sentinel.clone()]));
        assert!(panel.session_list.get_untracked().is_empty());
        assert!(daemon.store_sessions_if_current(thin_spawner_ticket, vec![sentinel]));
        assert_eq!(panel.session_list.get_untracked()[0].id, "sentinel");
    }

    #[test]
    fn session_page_url_percent_encodes_reserved_cursor_bytes() {
        assert_eq!(
            sessions_page_url("https://surface.test/", Some("a&b=c+d/e?f#g %z"),),
            "https://surface.test/v1/agent/sessions?limit=1000&cursor=a%26b%3Dc%2Bd%2Fe%3Ff%23g%20%25z",
        );
        assert_eq!(
            sessions_page_url("https://surface.test/", None),
            "https://surface.test/v1/agent/sessions?limit=1000",
        );
    }

    #[test]
    fn explicit_project_session_cwd_never_inherits_ambient_state() {
        assert_eq!(
            project_session_cwd(" /srv/project ").as_deref(),
            Some("/srv/project")
        );
        assert_eq!(project_session_cwd("  "), None);
    }

    #[test]
    fn project_create_request_serializes_name_and_workspace_root_exactly() {
        // The daemon's `CreateProjectRequest` deserializes `name` +
        // `workspace_root` verbatim (plain snake_case Rust field names, no
        // rename). The surface's mirror MUST serialize exactly those two keys
        // with those exact values — anything else and the daemon 400s the
        // create (or worse, silently binds the project to the wrong root). We
        // send no `config` (the daemon defaults it), so the body must carry
        // only `name` + `workspace_root`.
        let req = ProjectCreateRequest {
            name: "neo".into(),
            workspace_root: "/srv/neo".into(),
        };
        let v = serde_json::to_value(&req).expect("request serializes");
        let map = v.as_object().expect("request is a JSON object");
        assert_eq!(map.len(), 2, "only name + workspace_root, no leaked field");
        assert_eq!(
            map.get("name").and_then(|x| x.as_str()),
            Some("neo"),
            "name round-trips verbatim",
        );
        assert_eq!(
            map.get("workspace_root").and_then(|x| x.as_str()),
            Some("/srv/neo"),
            "workspace_root round-trips verbatim",
        );
    }

    #[test]
    fn chat_workspace_root_is_tmp_and_distinct_from_project_sessions() {
        // Chat is projectless and explicitly pins `/tmp`; a project session
        // explicitly pins its resolved section root. Neither path may inherit
        // the cwd of the previously active session.
        assert_eq!(CHAT_WORKSPACE_ROOT, "/tmp");

        let chat = Daemon::dummy();
        chat.cwd.set(CHAT_WORKSPACE_ROOT.to_string());
        chat.new_session();
        assert_eq!(chat.project.get_untracked(), None);
        assert_eq!(chat.cwd.get_untracked(), "/tmp");

        let project = Daemon::dummy();
        project.project.set(Some("p1".into()));
        project
            .cwd
            .set(project_session_cwd("/srv/p1").expect("nonempty root"));
        project.new_session();
        assert_eq!(project.project.get_untracked(), Some("p1".to_string()));
        assert_eq!(project.cwd.get_untracked(), "/srv/p1");
    }

    #[test]
    fn old_project_json_decodes_with_defaults_for_new_fields() {
        // Old daemon JSON (no git_branch, git_dirty, worktrees) must decode.
        let json = r#"{"id":"p1","name":"Alpha","workspace_root":"/alpha"}"#;
        let p: ProjectInfo = serde_json::from_str(json).unwrap();
        assert_eq!(p.id, "p1");
        assert_eq!(p.name, "Alpha");
        assert_eq!(p.workspace_root, "/alpha");
        assert_eq!(p.git_branch, None);
        assert_eq!(p.git_dirty, None);
        assert!(p.worktrees.is_empty());
    }

    #[test]
    fn old_fs_dir_json_decodes_with_defaults_for_new_fields() {
        let json = r#"{"name":"src","path":"/src","git":true}"#;
        let d: FsDirEntry = serde_json::from_str(json).unwrap();
        assert_eq!(d.name, "src");
        assert_eq!(d.path, "/src");
        assert!(d.git);
        assert!(!d.is_repo);
        assert_eq!(d.git_branch, None);
    }

    #[test]
    fn realtime_secret_workspace_root_is_additive_and_backward_compatible() {
        let old: RealtimeSecret = serde_json::from_value(json!({
            "client_secret": "ephemeral",
            "model": "gpt-realtime-2.1"
        }))
        .unwrap();
        assert_eq!(old.workspace_root, None);

        let scoped: RealtimeSecret = serde_json::from_value(json!({
            "client_secret": "ephemeral",
            "model": "gpt-realtime-2.1",
            "workspace_root": "/work/tree"
        }))
        .unwrap();
        assert_eq!(scoped.workspace_root.as_deref(), Some("/work/tree"));
    }

    #[test]
    fn planner_wire_requests_are_narrow_and_exact() {
        let secret = PlannerRealtimeSecretRequest {
            purpose: "planner",
            planner_context: PlannerRealtimeContextRequest {
                project_id: "p1",
                workspace_root: "/work/tree",
            },
        };
        assert_eq!(
            serde_json::to_value(secret).unwrap(),
            json!({
                "purpose": "planner",
                "planner_context": {
                    "project_id": "p1",
                    "workspace_root": "/work/tree"
                }
            })
        );

        let create = AgentSessionCreateRequest {
            title: None,
            workspace_root: "/work/tree",
            project_id: Some("p1"),
            client_type: Some("surface-web"),
        };
        assert_eq!(
            serde_json::to_value(create).unwrap(),
            json!({
                "workspace_root": "/work/tree",
                "project_id": "p1",
                "client_type": "surface-web"
            })
        );

        let append = PlannerHandoffRequest {
            role: "user",
            kind: "planner_handoff",
            content: "# Plan",
        };
        assert_eq!(
            serde_json::to_value(append).unwrap(),
            json!({"role": "user", "kind": "planner_handoff", "content": "# Plan"})
        );
    }

    #[test]
    fn planner_start_turn_wire_has_one_prompt_token_and_no_power_fields() {
        let token = "ab".repeat(32);
        let body = AgentTurnRequest {
            prompt: "# Plan",
            cwd: "/work/tree",
            session_id: Some("s1"),
            project_id: Some("p1"),
            client_type: Some("surface-extension"),
            guidance: None,
            room_id: None,
            thinking_level: None,
            model_id: None,
            images: None,
            decision_token: Some(&token),
            client_context: None,
            agent: None,
            canvas: None,
        };
        let value = serde_json::to_value(body).unwrap();
        assert_eq!(value["prompt"], "# Plan");
        assert_eq!(value["decision_token"].as_str().unwrap().len(), 64);
        assert_eq!(value["client_type"], "surface-extension");
        for forbidden in [
            "yolo",
            "tools",
            "tool_allowlist",
            "capabilities",
            "permissions",
            "purpose",
            "kind",
        ] {
            assert!(value.get(forbidden).is_none(), "leaked {forbidden}");
        }
    }

    #[test]
    fn production_stream_lifecycle_invalidates_readiness_on_drop() {
        let mut lifecycle = PlannerStreamLifecycle::default();
        lifecycle.transition(PlannerStreamKind::Agent, EventSourceState::Open);
        assert!(!lifecycle.ready());
        lifecycle.transition(PlannerStreamKind::Permission, EventSourceState::Open);
        assert!(lifecycle.ready());
        lifecycle.transition(PlannerStreamKind::Agent, EventSourceState::Connecting);
        assert!(!lifecycle.ready());
        lifecycle.transition(PlannerStreamKind::Agent, EventSourceState::Open);
        assert!(lifecycle.ready());
        lifecycle.transition(PlannerStreamKind::Permission, EventSourceState::Closed);
        assert!(!lifecycle.ready());
    }

    #[test]
    fn planner_stream_readiness_requires_both_exact_session_generation_markers() {
        let agent = ("s1".to_string(), 7);
        let permission = ("s1".to_string(), 7);
        assert!(planner_streams_ready(
            "s1",
            7,
            Some(&agent),
            Some(&permission)
        ));
        assert!(!planner_streams_ready("s1", 7, Some(&agent), None));
        assert!(!planner_streams_ready(
            "s1",
            8,
            Some(&agent),
            Some(&permission)
        ));
        let other = ("s2".to_string(), 7);
        assert!(!planner_streams_ready("s1", 7, Some(&agent), Some(&other)));
    }

    #[derive(Default)]
    struct FakePermissionSnapshotSource {
        revision: u64,
        fetches: usize,
        applied: Vec<Vec<String>>,
        degraded: Vec<String>,
    }

    impl PermissionSnapshotSource for FakePermissionSnapshotSource {
        fn revision(&self) -> u64 {
            self.revision
        }

        fn fetch<'a>(&'a mut self) -> LocalBoxFuture<'a, Result<Vec<PendingPermission>, String>> {
            async move {
                self.fetches += 1;
                let id = if self.fetches == 1 {
                    self.revision += 1;
                    "stale-decided"
                } else {
                    "fresh-pending"
                };
                Ok(vec![PendingPermission {
                    permission_id: id.into(),
                    session_id: "s1".into(),
                    tool: "bash".into(),
                    reason: "test".into(),
                    args_summary: String::new(),
                    deciding: false,
                    request_id: None,
                    actionable: false,
                }])
            }
            .boxed_local()
        }

        fn apply(&mut self, snapshot: Vec<PendingPermission>) {
            self.applied.push(
                snapshot
                    .into_iter()
                    .map(|permission| permission.permission_id)
                    .collect(),
            );
        }

        fn degrade(&mut self, reason: String) {
            self.degraded.push(reason);
        }
    }

    /// A snapshot source that always errors from `fetch` — for seam 3.
    #[derive(Default)]
    struct ErroringPermissionSnapshotSource {
        fetches: usize,
        applied: usize,
        degraded: Vec<String>,
    }

    impl PermissionSnapshotSource for ErroringPermissionSnapshotSource {
        fn revision(&self) -> u64 {
            0
        }

        fn fetch<'a>(&'a mut self) -> LocalBoxFuture<'a, Result<Vec<PendingPermission>, String>> {
            async move {
                self.fetches += 1;
                Err("route missing".into())
            }
            .boxed_local()
        }

        fn apply(&mut self, _snapshot: Vec<PendingPermission>) {
            self.applied += 1;
        }

        fn degrade(&mut self, reason: String) {
            self.degraded.push(reason);
        }
    }

    #[test]
    fn permission_reconciler_refetches_after_revision_change_and_applies_once() {
        let mut source = FakePermissionSnapshotSource::default();
        reconcile_permission_snapshot(&mut source)
            .now_or_never()
            .expect("fake snapshots are immediately ready")
            .unwrap();
        assert_eq!(source.fetches, 2);
        assert_eq!(source.applied, vec![vec!["fresh-pending".to_string()]]);
    }

    // ---------------------------------------------------------------------
    // TASK-69: two-state permission view. Five seams, each with a failure-
    // watched test (watched RED against a deliberately-broken build before it
    // counted). Needles are assembled at runtime, not matched against source.
    // ---------------------------------------------------------------------

    fn ids_of(view: &PermissionView) -> Vec<String> {
        view.cards()
            .iter()
            .map(|card| card.permission_id.clone())
            .collect()
    }

    // Seam 1: a degraded session-load projection publishes a VISIBLE Unavailable
    // that retains the ids the operator was already shown locally — not a blank
    // Fresh that makes a pre-load gate vanish. Prior view carries known ids with
    // no live card (a prior degrade), and a fresh degrade preserves that warning.
    // (Broken build: Degraded arm returns `PermissionView::fresh(vec![])` →
    // `is_unavailable()` goes false → RED.)
    #[test]
    fn seam1_projection_degrade_publishes_unavailable_retaining_prior_ids() {
        let prior = PermissionView::unavailable(
            "earlier".into(),
            vec![format!("gate-{}", "alpha"), format!("gate-{}", "beta")],
        );
        let expected = prior.unconfirmed_ids();
        let projection = build_session_projection(
            None,
            &[],
            SnapshotSettle::Degraded("route missing on host".into()),
            &[],
            &prior,
            &[],
            &HashMap::new(),
        );
        assert!(
            projection.permission.is_unavailable(),
            "degrade must publish Unavailable, not a silent empty Fresh"
        );
        assert_eq!(projection.permission.unconfirmed_ids(), expected);
        assert!(projection.permission.cards().is_empty());
    }

    // Seam 1 contrast: a settled fetch is authoritative Fresh with actionable
    // cards, and NO degradation warning.
    #[test]
    fn seam1_projection_fresh_publishes_authoritative_cards() {
        let projection = build_session_projection(
            None,
            &[],
            SnapshotSettle::Fresh(vec![task44_card("gate-only", None)]),
            &[],
            &PermissionView::default(),
            &[],
            &HashMap::new(),
        );
        assert!(!projection.permission.is_unavailable());
        assert_eq!(
            ids_of(&projection.permission),
            vec!["gate-only".to_string()]
        );
        assert!(projection.permission.unconfirmed_ids().is_empty());
    }

    // -- codex HOLD regressions, driven through the REAL production wrapper --
    //
    // `Daemon::dummy()` constructs the full receiver natively (only `Daemon::new`
    // touches `js-sys`). So these drive the actual post-fetch commit wrapper
    // `apply_session_projection` — final admission recheck, transcript rebuild,
    // atomic signal commit incl. the two-state permission publish — over a real
    // dummy Daemon with INJECTED fetch results (`detail`, `settle`), and the
    // tombstones populated by the real `apply_control_event` plumbing, NOT
    // precomputed (codex HOLD on 8b41fee/4cfb5c2/c2a1810). Reconciler regressions
    // drive the real `DaemonPermissionSnapshotSource::apply`/`degrade`.

    /// A dummy Daemon whose session/generation/activity make snapshot admission
    /// pass for id "s1" at generation 5.
    fn wrapper_daemon() -> Daemon {
        let daemon = Daemon::dummy();
        daemon.session_id.set(Some("s1".into()));
        daemon.sse_generation.set(5);
        daemon.activity_revision.set(0);
        daemon
    }

    fn user_entry(text: &str) -> SessionTranscriptEntry {
        SessionTranscriptEntry {
            role: "user".into(),
            text: text.into(),
            tool_call_id: None,
            tool_name: None,
            is_error: None,
        }
    }

    fn assistant_entry(text: &str) -> SessionTranscriptEntry {
        SessionTranscriptEntry {
            role: "assistant".into(),
            text: text.into(),
            tool_call_id: None,
            tool_name: None,
            is_error: None,
        }
    }

    fn wrapper_detail(
        pending: Vec<String>,
        transcript: Vec<SessionTranscriptEntry>,
    ) -> SessionDetail {
        SessionDetail {
            id: "s1".into(),
            title: "session".into(),
            model: String::new(),
            workspace_root: None,
            cwd: None,
            transcript,
            tool_context: vec![],
            pending_permissions: pending,
            state: None, // terminal — no live turn, so the wrapper commits
            active_requests: vec![],
        }
    }

    fn decision_frame(id: &str) -> ControlEvent {
        ControlEvent::PermissionDecision {
            permission_id: Some(id.to_string()),
            session_id: Some("s1".to_string()),
        }
    }

    fn request_frame(id: &str) -> ControlEvent {
        ControlEvent::PermissionRequest {
            permission_id: Some(id.to_string()),
            session_id: Some("s1".to_string()),
            request_id: Some(format!("{id}-req")),
            tool: "bash".to_string(),
            reason: "run".to_string(),
            args: serde_json::Value::Null,
        }
    }

    /// Push a control frame through the real `apply_control_event` into the
    /// daemon's live pending/tombstone signals (as the control stream would),
    /// stamping card actionability from the daemon's own decision authority.
    fn drive_frame(daemon: &Daemon, frame: &ControlEvent) {
        apply_control_event(
            frame,
            "s1",
            daemon.permission_view,
            daemon.permission_tombstones,
            &daemon.decision_authority.get_untracked(),
        );
    }

    /// Run the real post-fetch commit wrapper with injected fetch results,
    /// claiming a fresh permission epoch (the ordinary single-publisher case).
    fn run_wrapper(
        daemon: &Daemon,
        detail: SessionDetail,
        settle: SnapshotSettle,
    ) -> ProjectionOutcome {
        let epoch = daemon.claim_permission_epoch();
        apply_session_projection(&daemon.commit_signals(), "s1", 5, 0, epoch, detail, settle)
            .expect("admission passes for the focused session")
    }

    #[test]
    fn live_session_projection_commits_full_transcript_without_sync_quarantine() {
        let daemon = wrapper_daemon();
        let mut detail = wrapper_detail(
            vec![],
            vec![
                user_entry("start the long task"),
                assistant_entry("partial persisted response"),
            ],
        );
        detail.state = Some(SessionRunState::Running);
        detail.active_requests = vec!["turn-live".into()];

        let outcome = run_wrapper(&daemon, detail, SnapshotSettle::Fresh(Vec::new()));

        assert_eq!(outcome, ProjectionOutcome::Terminal);
        assert_eq!(daemon.turns.get_untracked().len(), 2);
        assert!(matches!(
            &daemon.turns.get_untracked()[1].blocks[..],
            [Block::Text(text)] if text == "partial persisted response"
        ));
        assert!(daemon.streaming.get_untracked());
        assert_eq!(
            daemon.active_turn_id.get_untracked().as_deref(),
            Some("turn-live")
        );
        assert!(
            !daemon.sync_pending.get_untracked(),
            "session switching must never require client-side detail polling"
        );
    }

    // Regression 1: FIRST load, degraded snapshot — the TRANSCRIPT still commits
    // (63bc9ea) AND a pre-existing gate surfaces from the daemon's authoritative
    // SessionDetail.pending_permissions. Drives the real wrapper. (Broken: Degraded
    // ignores detail ids → empty warning → RED.)
    #[test]
    fn reg1_first_load_degrade_commits_transcript_and_surfaces_daemon_gate() {
        let daemon = wrapper_daemon();
        // Seed sentinels so we can prove the wrapper OVERWROTE transcript + pinned
        // state — i.e. it committed them despite the permission-snapshot degrade.
        daemon.turns.set(vec![Turn::user("STALE")]);
        daemon.pinned_widgets.set(vec![PinnedWidget {
            component_id: "stale-widget".into(),
            kind: "map".into(),
            props: serde_json::Value::Null,
        }]);
        let detail = wrapper_detail(
            vec![format!("gate-{}", "predates-load")],
            vec![user_entry("hello")],
        );
        run_wrapper(
            &daemon,
            detail,
            SnapshotSettle::Degraded("route 500".into()),
        );
        // Transcript committed from the snapshot (stale sentinel replaced) despite
        // the permission-snapshot failure (63bc9ea invariant).
        let turns = daemon.turns.get_untracked();
        assert_eq!(turns.len(), 1);
        assert!(matches!(&turns[0].blocks[..], [Block::Text(t)] if t == "hello"));
        // Pinned state committed too — the stale sentinel widget is gone.
        assert!(daemon.pinned_widgets.get_untracked().is_empty());
        // And the pre-load gate is a visible warning, not silence.
        let view = daemon.permission_view.get_untracked();
        assert!(view.is_unavailable());
        assert_eq!(
            view.unconfirmed_ids(),
            vec![format!("gate-{}", "predates-load")]
        );
    }

    // Regression 2: a live permission_request admitted during the await (real
    // apply_control_event into the daemon signal) SURVIVES a degraded wrapper
    // commit as a real card while an un-materialized daemon id still warns.
    // (Broken: degrade drops live cards → RED.)
    #[test]
    fn reg2_degrade_preserves_inflight_live_card_and_warns_uncovered() {
        let daemon = wrapper_daemon();
        drive_frame(&daemon, &request_frame("live-A"));
        let detail = wrapper_detail(
            vec!["live-A".to_string(), format!("gate-{}", "other")],
            vec![user_entry("hi")],
        );
        run_wrapper(&daemon, detail, SnapshotSettle::Degraded("churn".into()));
        let view = daemon.permission_view.get_untracked();
        assert_eq!(
            ids_of(&view),
            vec!["live-A".to_string()],
            "an admitted live card must survive the degraded commit"
        );
        assert!(view.is_unavailable());
        assert_eq!(view.unconfirmed_ids(), vec![format!("gate-{}", "other")]);
    }

    // Regression 3: a permission_decision for A interleaves the await (real
    // apply_control_event → removes A from the view AND tombstones it). On a
    // degraded wrapper whose stale detail ids still list A, the fold must NOT
    // resurrect A (seam 5). (Broken: fold ignores tombstones → RED.)
    #[test]
    fn reg3_decision_during_degrade_does_not_resurrect_from_stale_detail_ids() {
        let daemon = wrapper_daemon();
        drive_frame(&daemon, &request_frame("gate-A"));
        drive_frame(&daemon, &decision_frame("gate-A"));
        assert_eq!(
            daemon.permission_tombstones.get_untracked(),
            vec!["gate-A".to_string()]
        );
        let detail = wrapper_detail(vec![format!("gate-{}", "A")], vec![user_entry("hi")]);
        run_wrapper(
            &daemon,
            detail,
            SnapshotSettle::Degraded("confirm failed".into()),
        );
        let view = daemon.permission_view.get_untracked();
        assert!(
            !view.unconfirmed_ids().contains(&"gate-A".to_string()),
            "a gate decided during the load must not resurrect from stale detail ids"
        );
        assert!(view.unconfirmed_ids().is_empty());
    }

    // Regression 4: the case a scalar revision gate could NOT handle. A
    // permission_REQUEST B interleaves (resolves nothing); a still-open A lives
    // only in the older detail. On degrade, B is preserved AND A stays visible.
    // (Broken: drop all detail ids on churn → A vanishes → RED.)
    #[test]
    fn reg4_request_during_degrade_keeps_still_open_detail_id_visible() {
        let daemon = wrapper_daemon();
        drive_frame(&daemon, &request_frame("req-B"));
        assert!(daemon.permission_tombstones.get_untracked().is_empty());
        let detail = wrapper_detail(
            vec![format!("gate-{}", "still-open-A")],
            vec![user_entry("hi")],
        );
        run_wrapper(
            &daemon,
            detail,
            SnapshotSettle::Degraded("route 500".into()),
        );
        let view = daemon.permission_view.get_untracked();
        assert_eq!(ids_of(&view), vec!["req-B".to_string()]);
        assert!(view.is_unavailable());
        assert_eq!(
            view.unconfirmed_ids(),
            vec![format!("gate-{}", "still-open-A")],
            "an unrelated interleaved request must not hide a genuinely-open detail gate"
        );
    }

    // Regression 5: a reused id after a decision rematerializes. A is decided
    // (tombstoned), then a fresh request for A clears the tombstone so a later
    // degraded wrapper keeps A as a card. (Broken: request doesn't clear the
    // tombstone → RED.)
    #[test]
    fn reg5_reused_id_after_decision_rematerializes() {
        let daemon = wrapper_daemon();
        drive_frame(&daemon, &request_frame("gate-A"));
        drive_frame(&daemon, &decision_frame("gate-A"));
        assert_eq!(
            daemon.permission_tombstones.get_untracked(),
            vec!["gate-A".to_string()]
        );
        drive_frame(&daemon, &request_frame("gate-A"));
        assert!(
            daemon.permission_tombstones.get_untracked().is_empty(),
            "a reused permission_request must clear the decision tombstone"
        );
        let detail = wrapper_detail(vec![format!("gate-{}", "A")], vec![user_entry("hi")]);
        run_wrapper(
            &daemon,
            detail,
            SnapshotSettle::Degraded("route 500".into()),
        );
        assert_eq!(
            ids_of(&daemon.permission_view.get_untracked()),
            vec!["gate-A".to_string()]
        );
    }

    // Regression 6: an authoritative Fresh wrapper commit clears BOTH degradation
    // and tombstones. A is decided (tombstoned); a Fresh(empty) settle establishes
    // an authoritative empty view AND wipes the tombstones. (Broken: Fresh doesn't
    // clear tombstones → RED.)
    #[test]
    fn reg6_fresh_snapshot_clears_degradation_and_tombstones() {
        let daemon = wrapper_daemon();
        drive_frame(&daemon, &request_frame("gate-A"));
        drive_frame(&daemon, &decision_frame("gate-A"));
        assert_eq!(
            daemon.permission_tombstones.get_untracked(),
            vec!["gate-A".to_string()]
        );
        let detail = wrapper_detail(vec![format!("gate-{}", "A")], vec![user_entry("hi")]);
        run_wrapper(&daemon, detail, SnapshotSettle::Fresh(Vec::new()));
        let view = daemon.permission_view.get_untracked();
        assert!(
            !view.is_unavailable(),
            "a Fresh snapshot clears degradation"
        );
        assert!(view.cards().is_empty());
        assert!(
            daemon.permission_tombstones.get_untracked().is_empty(),
            "an authoritative Fresh snapshot absorbs and clears the tombstones"
        );
    }

    /// A `PermissionSnapshotSource` that wraps the REAL
    /// `DaemonPermissionSnapshotSource` — delegating `revision`/`apply`/`degrade`
    /// to it so the reconcile routing is exercised end-to-end — but returns a
    /// canned `fetch` result (no native HTTP). Lets the standalone regressions run
    /// through `reconcile_permission_snapshot` and hit the real apply/degrade,
    /// which cannot then diverge (codex HOLD on c2a1810).
    struct CannedFetchSource<'a> {
        inner: DaemonPermissionSnapshotSource<'a>,
        result: Result<Vec<PendingPermission>, String>,
    }

    impl PermissionSnapshotSource for CannedFetchSource<'_> {
        fn revision(&self) -> u64 {
            self.inner.revision()
        }
        fn fetch<'b>(&'b mut self) -> LocalBoxFuture<'b, Result<Vec<PendingPermission>, String>> {
            let result = self.result.clone();
            async move { result }.boxed_local()
        }
        fn apply(&mut self, snapshot: Vec<PendingPermission>) {
            self.inner.apply(snapshot);
        }
        fn degrade(&mut self, reason: String) {
            self.inner.degrade(reason);
        }
    }

    fn canned_source(
        daemon: &Daemon,
        result: Result<Vec<PendingPermission>, String>,
    ) -> CannedFetchSource<'_> {
        CannedFetchSource {
            inner: DaemonPermissionSnapshotSource {
                daemon,
                session_id: "s1",
                epoch_claim: daemon.claim_permission_epoch(),
            },
            result,
        }
    }

    // Regression 7 (codex HOLD on c2a1810): the STANDALONE reconciler path
    // (reconnect/planner reconcile) must ALSO preserve an admitted live card on a
    // failed reconcile — not wipe it to an id-only warning. Routes through the real
    // `reconcile_permission_snapshot` → `DaemonPermissionSnapshotSource::degrade`.
    // (Broken: degrade via card-wiping `unavailable` → the live card is lost → RED.)
    #[test]
    fn reg7_standalone_reconcile_degrade_preserves_live_card() {
        let daemon = wrapper_daemon();
        drive_frame(&daemon, &request_frame("live-A"));
        assert_eq!(
            ids_of(&daemon.permission_view.get_untracked()),
            vec!["live-A".to_string()]
        );
        let mut source = canned_source(&daemon, Err("permission refresh failed".into()));
        reconcile_permission_snapshot(&mut source)
            .now_or_never()
            .expect("canned fetch resolves immediately")
            .unwrap();
        let view = daemon.permission_view.get_untracked();
        assert!(
            view.is_unavailable(),
            "a failed reconcile publishes Unavailable"
        );
        assert_eq!(
            ids_of(&view),
            vec!["live-A".to_string()],
            "a failed standalone reconcile must preserve the admitted live card"
        );
    }

    // Regression 8 (codex HOLD on c2a1810): a stable authoritative Fresh on the
    // STANDALONE reconciler path must ALSO clear the decision tombstones, exactly
    // like the session-load commit. Routes through the real
    // `reconcile_permission_snapshot` → `DaemonPermissionSnapshotSource::apply`.
    // (Broken: apply publishes Fresh but leaves the tombstone → a later reused id
    // stays suppressed → RED.)
    #[test]
    fn reg8_standalone_reconcile_fresh_clears_tombstones() {
        let daemon = wrapper_daemon();
        drive_frame(&daemon, &request_frame("gate-A"));
        drive_frame(&daemon, &decision_frame("gate-A"));
        assert_eq!(
            daemon.permission_tombstones.get_untracked(),
            vec!["gate-A".to_string()]
        );
        let mut source = canned_source(&daemon, Ok(Vec::new())); // authoritative Fresh(empty)
        reconcile_permission_snapshot(&mut source)
            .now_or_never()
            .expect("canned fetch resolves immediately")
            .unwrap();
        assert!(!daemon.permission_view.get_untracked().is_unavailable());
        assert!(
            daemon.permission_tombstones.get_untracked().is_empty(),
            "a stable standalone reconcile Fresh must clear the tombstones"
        );
    }

    // Regression 9 (codex HOLD on c2a1810): the NEGATIVE admission branch. A
    // snapshot for a retired session/generation must write NOTHING — not the
    // transcript, not the permission view, not the tombstones. Drives the real
    // wrapper with a stale generation so the final admission recheck discards.
    #[test]
    fn reg9_retired_admission_writes_no_transcript_or_permission_state() {
        let daemon = wrapper_daemon();
        // Seed distinguishable prior state.
        daemon.turns.set(vec![Turn::user("KEEP")]);
        drive_frame(&daemon, &request_frame("gate-A"));
        drive_frame(&daemon, &decision_frame("gate-A"));
        let turns_before = serde_json::to_value(daemon.turns.get_untracked()).unwrap();
        let view_before = daemon.permission_view.get_untracked();
        let tombstones_before = daemon.permission_tombstones.get_untracked();
        // Retire the snapshot: call the wrapper with a generation the daemon has
        // moved past (sse_generation is 5; this snapshot captured generation 4).
        let detail = wrapper_detail(vec![format!("gate-{}", "A")], vec![user_entry("new")]);
        let epoch = daemon.claim_permission_epoch();
        let outcome = apply_session_projection(
            &daemon.commit_signals(),
            "s1",
            4,
            0,
            epoch,
            detail,
            SnapshotSettle::Fresh(Vec::new()),
        );
        assert!(outcome.is_err(), "a retired snapshot must not commit");
        // Nothing was written.
        assert_eq!(
            serde_json::to_value(daemon.turns.get_untracked()).unwrap(),
            turns_before,
            "a retired snapshot must not overwrite the transcript"
        );
        assert_eq!(daemon.permission_view.get_untracked(), view_before);
        assert_eq!(
            daemon.permission_tombstones.get_untracked(),
            tombstones_before
        );
    }

    // Regression 10 (codex HOLD on 8b41fee1): the cross-publisher epoch race. The
    // agent load claims its epoch first (detail lists A); decision A lands
    // (tombstone); a NEWER standalone reconcile then claims a later epoch and
    // publishes authoritative Fresh(empty), clearing the tombstone; finally the
    // OLDER agent rich fetch degrades. Its permission publish must be SKIPPED (its
    // epoch is superseded) so it cannot resurrect A over the newer Fresh — while
    // its transcript still commits. (Broken: no epoch gate → stale Degraded
    // overwrites Fresh(empty) with Unavailable(A) → RED.)
    #[test]
    fn reg10_stale_agent_degrade_does_not_overwrite_newer_standalone_fresh() {
        let daemon = wrapper_daemon();
        // A is pending and shown; the agent load starts (claims its epoch) with A
        // in its detail.
        drive_frame(&daemon, &request_frame("gate-A"));
        let agent_epoch = daemon.claim_permission_epoch();
        // Decision A lands during the agent's fetches: tombstone + remove.
        drive_frame(&daemon, &decision_frame("gate-A"));
        // A newer standalone reconcile publishes authoritative Fresh(empty),
        // claiming a later epoch and clearing the tombstone.
        let mut source = canned_source(&daemon, Ok(Vec::new()));
        reconcile_permission_snapshot(&mut source)
            .now_or_never()
            .expect("canned fetch resolves immediately")
            .unwrap();
        assert!(!daemon.permission_view.get_untracked().is_unavailable());
        assert!(daemon.permission_tombstones.get_untracked().is_empty());
        // Now the OLDER agent rich fetch degrades and tries to commit with its
        // stale epoch. Detail still lists A (captured before the decision).
        let detail = wrapper_detail(vec![format!("gate-{}", "A")], vec![user_entry("hi")]);
        apply_session_projection(
            &daemon.commit_signals(),
            "s1",
            5,
            0,
            agent_epoch,
            detail,
            SnapshotSettle::Degraded("older rich fetch failed".into()),
        )
        .expect("admission still passes; only the permission publish is gated");
        // Transcript committed...
        assert!(!daemon.turns.get_untracked().is_empty());
        // ...but the stale agent did NOT overwrite the newer authoritative Fresh:
        // A is not resurrected and the tombstone stays clear.
        let view = daemon.permission_view.get_untracked();
        assert!(
            !view.is_unavailable(),
            "a stale agent degrade must not overwrite the newer standalone Fresh"
        );
        assert!(view.cards().is_empty());
        assert!(daemon.permission_tombstones.get_untracked().is_empty());
    }

    // Regression 11 (codex HOLD on 8b41fee1): the inverse — the LATEST claimant
    // wins. An older standalone reconcile publishes first; the agent then claims a
    // later epoch and its publish supersedes the older one. (Broken: if the gate
    // rejected the latest claimant, the agent's authoritative result would be lost
    // → RED.)
    #[test]
    fn reg11_latest_claimant_publish_wins() {
        let daemon = wrapper_daemon();
        // An older standalone reconcile publishes Fresh([old-B]) first.
        let mut source = canned_source(&daemon, Ok(vec![task44_card("old-B", Some("old-B-req"))]));
        reconcile_permission_snapshot(&mut source)
            .now_or_never()
            .expect("canned fetch resolves immediately")
            .unwrap();
        assert_eq!(
            ids_of(&daemon.permission_view.get_untracked()),
            vec!["old-B".to_string()]
        );
        // The agent claims a LATER epoch and commits Fresh([new-C]); as the latest
        // claimant it wins.
        let agent_epoch = daemon.claim_permission_epoch();
        let detail = wrapper_detail(vec![], vec![user_entry("hi")]);
        apply_session_projection(
            &daemon.commit_signals(),
            "s1",
            5,
            0,
            agent_epoch,
            detail,
            SnapshotSettle::Fresh(vec![task44_card("new-C", Some("new-C-req"))]),
        )
        .expect("admission passes");
        assert_eq!(
            ids_of(&daemon.permission_view.get_untracked()),
            vec!["new-C".to_string()],
            "the latest claimant's authoritative publish must win"
        );
    }

    // Regression 12 (codex HOLD on 8b41fee1, pin 3): when an older permission
    // commit is SKIPPED, its downstream (live_requests → decision_authority
    // pruning) must derive from the WINNER (current permission_view), not the
    // rejected candidate. A newer card B published by a reconcile must keep its
    // actionability AND its request-bound decision authority when an older agent
    // commit that lacks B is skipped. (Broken: prune from the rejected candidate →
    // B's authority is pruned though B is still visibly published → RED.)
    #[test]
    fn reg12_skipped_older_commit_does_not_prune_newer_card_authority() {
        let daemon = wrapper_daemon();
        // Local authority so card B (in the daemon's session s1) is actionable.
        daemon
            .decision_authority
            .set(task44_authority(&[("B-req", "token-B", "s1")]));
        // The agent load starts first (claims the earlier epoch).
        let agent_epoch = daemon.claim_permission_epoch();
        // A newer standalone reconcile publishes Fresh([B]); B is actionable.
        let mut source = canned_source(&daemon, Ok(vec![session_card("B", Some("B-req"), "s1")]));
        reconcile_permission_snapshot(&mut source)
            .now_or_never()
            .expect("canned fetch resolves immediately")
            .unwrap();
        assert!(daemon.permission_view.get_untracked().cards()[0].actionable);
        // The older agent commit (which lacks B) is skipped; its authority pruning
        // must use the current view (with B), not its own empty candidate.
        let detail = wrapper_detail(vec![], vec![user_entry("hi")]);
        apply_session_projection(
            &daemon.commit_signals(),
            "s1",
            5,
            0,
            agent_epoch,
            detail,
            SnapshotSettle::Fresh(Vec::new()),
        )
        .expect("admission passes");
        let view = daemon.permission_view.get_untracked();
        assert_eq!(ids_of(&view), vec!["B".to_string()]);
        assert!(
            view.cards()[0].actionable,
            "the newer card must stay actionable through an older skipped commit"
        );
        assert!(
            daemon
                .decision_authority
                .get_untracked()
                .contains_key("B-req"),
            "an older skipped commit must not prune the newer card's decision authority"
        );
    }

    // Regression 13 (codex HOLD on 8b41fee1, pin 2): a session reset/switch must
    // ADVANCE the epoch so an in-flight OLD-session publisher cannot remain latest
    // and publish over the freshly-reset state. A reconcile claims its epoch, then
    // the user switches sessions (reset + epoch advance); when the stale reconcile
    // finally publishes, it is skipped. (Broken: switch doesn't advance the epoch →
    // the stale reconcile's card overwrites the reset state → RED.)
    #[test]
    fn reg13_session_switch_supersedes_in_flight_reconcile_publish() {
        let daemon = wrapper_daemon();
        // An in-flight reconcile has claimed its epoch (canned_source claims on
        // construction) but has not published yet.
        let mut source = canned_source(&daemon, Ok(vec![task44_card("stale", Some("stale-req"))]));
        // The user switches sessions: resets permission state AND advances the epoch.
        daemon.select_session_state("s2", "other session".into());
        // The stale reconcile now completes and tries to publish.
        reconcile_permission_snapshot(&mut source)
            .now_or_never()
            .expect("canned fetch resolves immediately")
            .unwrap();
        assert!(
            daemon.permission_view.get_untracked().cards().is_empty(),
            "a session switch must supersede an in-flight reconcile's stale publish"
        );
    }

    // Regression 14 (codex HOLD on af82ee8): the THIRD session-identity transition
    // — strict-resume retirement of an EXPIRED session — must clear old-session
    // permission state and invalidate in-flight publishers, exactly like
    // select/new. It previously bypassed both, leaving old actionable cards (and
    // their request-bound authority, which decide_permission resolves by
    // card/request not active session) visible in the replacement session. All
    // three paths now route through `retire_permission_state`. This drives that
    // shared retirement — the exact action the strict-resume branch runs — and
    // asserts old-session cards/tombstones are gone, the epoch advanced, and an
    // in-flight old-session publisher is rejected. (Broken: retirement doesn't
    // clear the view / advance the epoch → old card carries in → RED.)
    #[test]
    fn reg14_expired_session_retirement_clears_permission_and_supersedes_publisher() {
        let daemon = wrapper_daemon();
        // Old (s1) session state: an actionable card A (authority present), plus a
        // decided card B tombstoned.
        daemon
            .decision_authority
            .set(task44_authority(&[("card-A-req", "token-A", "s1")]));
        drive_frame(&daemon, &request_frame("card-A"));
        drive_frame(&daemon, &request_frame("card-B"));
        drive_frame(&daemon, &decision_frame("card-B"));
        assert!(daemon.permission_view.get_untracked().cards()[0].actionable);
        assert_eq!(
            daemon.permission_tombstones.get_untracked(),
            vec!["card-B".to_string()]
        );
        // An old-session publisher is in flight (claimed before the retirement).
        let stale_claim = daemon.claim_permission_epoch();
        // The strict-resume path retires the expired session (the exact action it
        // runs before recursively adopting a fresh one), then s2 is installed.
        daemon.retire_permission_state();
        daemon.session_id.set(Some("s2".into()));
        // Old-session card + tombstone are gone from the replacement session.
        assert!(
            daemon.permission_view.get_untracked().cards().is_empty(),
            "old-session cards must not carry into the strict-resume replacement session"
        );
        assert!(daemon.permission_tombstones.get_untracked().is_empty());
        // The in-flight old-session publisher is superseded: its publish is rejected
        // and the (empty) replacement view survives.
        let published = publish_permission_view(
            daemon.permission_view,
            daemon.permission_tombstones,
            daemon.permission_epoch,
            stale_claim,
            PermissionView::fresh(vec![task44_card("card-A", Some("card-A-req"))]),
        );
        assert!(
            !published,
            "an in-flight old-session publisher must be superseded by the retirement"
        );
        assert!(
            daemon.permission_view.get_untracked().cards().is_empty(),
            "a superseded stale publish must not resurrect an old-session card"
        );
        // Codex pin: the leftover s1 grant must NOT authorize a NEW-session (s2)
        // card that reuses/collides on the request id — authority is session-bound
        // (resolve_decision_authority requires card.session_id == grant.session_id).
        let mut s2_card = session_card("card-A", Some("card-A-req"), "s2");
        mark_cards_actionable(
            std::slice::from_mut(&mut s2_card),
            &daemon.decision_authority.get_untracked(),
        );
        assert!(
            !s2_card.actionable,
            "a retired s1 grant must not authorize an s2 card with a colliding request id"
        );
    }

    // Regression 15 (codex HOLD on 9c584d0 + 06:23 pin): decide_permission's
    // post-await completion must target the EXACT decided card instance —
    // permission_id + session_id + request_id — or write nothing. This covers the
    // cross-session race (an old-session decision landing after a switch) AND the
    // same-session id-REUSE race (the id reused for a new request while the POST is
    // in flight). Drives the extracted completion helper directly.
    #[test]
    fn reg15_permission_decision_completion_targets_exact_card_instance() {
        let daemon = wrapper_daemon();
        let complete = |session: &str, request: Option<&str>, completion: DecisionCompletion| {
            apply_permission_decision_completion(
                daemon.permission_view,
                daemon.permission_tombstones,
                daemon.permission_revision,
                daemon.status,
                "A",
                session,
                request,
                completion,
            )
        };

        // Branch 1 — cross-session: we decided s1 card A (request R1), but the
        // session switched to s2, whose view holds a DIFFERENT card A. The stale s1
        // completion must write nothing (the s2 card survives, untombstoned).
        daemon
            .permission_view
            .set(PermissionView::fresh(vec![session_card(
                "A",
                Some("R1"),
                "s2",
            )]));
        assert!(
            !complete(
                "s1",
                Some("R1"),
                DecisionCompletion::Succeeded { allow: true }
            ),
            "a cross-session stale completion must write nothing"
        );
        assert_eq!(
            ids_of(&daemon.permission_view.get_untracked()),
            vec!["A".to_string()]
        );
        assert!(daemon.permission_tombstones.get_untracked().is_empty());

        // Branch 2 — same-session id REUSE: s1 card A decided for R1, but the id was
        // reused for a NEW request R2 in s1 before the response landed. The old R1
        // completion must not remove/tombstone the valid R2 card.
        daemon
            .permission_view
            .set(PermissionView::fresh(vec![session_card(
                "A",
                Some("R2"),
                "s1",
            )]));
        assert!(
            !complete(
                "s1",
                Some("R1"),
                DecisionCompletion::Succeeded { allow: true }
            ),
            "a same-session reused-id completion must not remove the R2 card"
        );
        assert_eq!(
            ids_of(&daemon.permission_view.get_untracked()),
            vec!["A".to_string()]
        );
        assert!(daemon.permission_tombstones.get_untracked().is_empty());

        // Branch 3 — exact match: same session + request. Success removes and
        // tombstones the card.
        daemon
            .permission_view
            .set(PermissionView::fresh(vec![session_card(
                "A",
                Some("R1"),
                "s1",
            )]));
        assert!(complete(
            "s1",
            Some("R1"),
            DecisionCompletion::Succeeded { allow: true }
        ));
        assert!(daemon.permission_view.get_untracked().cards().is_empty());
        assert_eq!(
            daemon.permission_tombstones.get_untracked(),
            vec!["A".to_string()]
        );

        // Branch 3b — exact match, ERROR: only re-enables the card (clears
        // deciding), does not remove it.
        let mut card = session_card("A", Some("R1"), "s1");
        card.deciding = true;
        daemon
            .permission_view
            .set(PermissionView::fresh(vec![card]));
        daemon.permission_tombstones.set(Vec::new());
        assert!(complete(
            "s1",
            Some("R1"),
            DecisionCompletion::Failed {
                message: "boom".into()
            }
        ));
        let view = daemon.permission_view.get_untracked();
        assert_eq!(ids_of(&view), vec!["A".to_string()]);
        assert!(
            !view.cards()[0].deciding,
            "error completion re-enables the exact card"
        );
        assert!(daemon.permission_tombstones.get_untracked().is_empty());
    }

    // Seam 2: a failed cross-session permission poll classifies as Unavailable
    // (the caller keeps the last list but flags it stale) — it does NOT silently
    // pass as authoritative. (Broken build: Err arm returns `Fresh(vec![])` →
    // the `Unavailable` match fails → RED.)
    #[test]
    fn seam2_permission_poll_error_is_unavailable_not_authoritative() {
        let reason = format!("network {}", "down");
        let errored = classify_permission_poll(Err(reason.clone()));
        match errored {
            PermissionPollOutcome::Unavailable(got) => assert_eq!(got, reason),
            PermissionPollOutcome::Fresh(_) => {
                panic!("a failed poll must not classify as authoritative Fresh")
            }
        }
        let settled = classify_permission_poll(Ok(Vec::new()));
        assert!(matches!(settled, PermissionPollOutcome::Fresh(_)));
    }

    // Seam 3: reconciliation that cannot settle DEGRADES (visible Unavailable)
    // and returns Ok — it does not bubble a hard Err that a caller swallows to
    // empty (a silent fail-open). (Broken build: Err arm `return Err(...)` →
    // `result.is_ok()` false and `degraded` empty → RED.)
    #[test]
    fn seam3_reconcile_error_degrades_instead_of_failing_open() {
        let mut source = ErroringPermissionSnapshotSource::default();
        let result = reconcile_permission_snapshot(&mut source)
            .now_or_never()
            .expect("erroring source resolves immediately");
        assert!(
            result.is_ok(),
            "exhaustion/err must degrade, not bubble an Err a caller swallows"
        );
        assert_eq!(source.applied, 0, "a failed fetch must never apply cards");
        assert_eq!(source.degraded.len(), 1, "must publish exactly one degrade");
    }

    // Seam 4: a live permission_request frame while Unavailable re-materializes a
    // real card (dropping its id from the warning set) but does NOT clear the
    // degradation — only a settled snapshot may. So {A,B} unavailable + frame C
    // yields cards=[C], retained warning {A,B}, still Unavailable. (Broken build:
    // `ingest_request` sets availability = Fresh → `is_unavailable()` false → RED.)
    #[test]
    fn seam4_live_frame_rematerializes_card_without_clearing_degradation() {
        let known = vec!["A".to_string(), "B".to_string()];
        let mut view = PermissionView::unavailable("stale".into(), known.clone());
        view.ingest_request(task44_card("C", None));
        assert_eq!(ids_of(&view), vec!["C".to_string()]);
        assert!(
            view.is_unavailable(),
            "a live frame must not clear degradation — only a settled snapshot may"
        );
        assert_eq!(view.unconfirmed_ids(), known);

        // A frame whose id WAS in the known set materializes it out of the
        // warning without clearing the rest.
        let mut view2 = PermissionView::unavailable("stale".into(), known.clone());
        view2.ingest_request(task44_card("A", None));
        assert_eq!(view2.unconfirmed_ids(), vec!["B".to_string()]);
        assert!(view2.is_unavailable());
    }

    // Seam 5: a decided gate must not resurrect through the degrade warning — the
    // decision drops the id from BOTH live cards and known_pending_ids. (Broken
    // build: `resolve` retains cards only, skipping known_pending_ids → the
    // decided id stays in `unconfirmed_ids()` → RED.)
    #[test]
    fn seam5_decided_gate_does_not_resurrect_via_known_pending_ids() {
        let mut degraded =
            PermissionView::unavailable("stale".into(), vec!["A".to_string(), "B".to_string()]);
        degraded.resolve("A");
        assert_eq!(
            degraded.unconfirmed_ids(),
            vec!["B".to_string()],
            "a decided id must not survive in the degrade warning"
        );

        let mut fresh = PermissionView::fresh(vec![task44_card("X", None), task44_card("Y", None)]);
        fresh.resolve("X");
        assert_eq!(ids_of(&fresh), vec!["Y".to_string()]);
    }

    #[test]
    fn planner_retry_echoes_confirmed_prompt_only_once() {
        let mut turns = Vec::new();
        assert!(!planner_prompt_already_echoed(&turns, "# Plan"));
        turns.push(Turn::user("# Plan"));
        assert!(planner_prompt_already_echoed(&turns, "# Plan"));
        assert!(!planner_prompt_already_echoed(&turns, "# Other"));
    }

    // TASK-98: browser tool args are kept WHOLE so the cockpit's
    // summary_from_args can parse them as JSON — a real browser_navigate URL
    // exceeds 60 chars, and truncating mid-JSON made the summary "?". Non-browser
    // tools stay truncated for a compact raw-args glance.
    #[test]
    fn browser_tool_args_preview_is_kept_whole_for_summary_parsing() {
        let long_url = "https://example.com/some/very/long/path?query=one&more=two&yet=three";
        let preview = tool_args_preview("browser_navigate", &json!({ "url": long_url }));
        // Kept whole → still valid JSON, so the cockpit summary parses the url.
        let parsed: Value =
            serde_json::from_str(&preview).expect("browser args must stay parseable JSON");
        assert_eq!(parsed.get("url").and_then(|v| v.as_str()), Some(long_url));
        // Non-browser tools remain truncated (a bash/write call can be huge).
        let bash = tool_args_preview("bash", &json!({ "command": "x".repeat(200) }));
        assert!(bash.chars().count() <= 60);
    }

    // TASK-99: a RELOADED session must recover browser-action summaries too — the
    // transcript rebuild now populates a tool block's args_preview from the
    // matching tool CALL's persisted arguments (by tool_call_id), instead of
    // dropping it to empty and leaving the cockpit summary as "?".
    #[test]
    fn transcript_rebuild_recovers_browser_tool_args_for_summary() {
        let long_url = "https://example.com/very/long/path?a=1&b=2&c=3&d=4&e=5&f=6";
        let entries = vec![SessionTranscriptEntry {
            role: "tool".into(),
            text: "navigated".into(),
            tool_call_id: Some("call-1".into()),
            tool_name: Some("browser_navigate".into()),
            is_error: Some(false),
        }];
        let tool_context = vec![SessionToolContext {
            kind: "call".into(),
            tool_call_id: "call-1".into(),
            tool_name: "browser_navigate".into(),
            arguments: Some(json!({ "url": long_url })),
        }];
        let (turns, _) = turns_from_session_transcript(entries, &tool_context);
        let preview = turns
            .iter()
            .flat_map(|t| &t.blocks)
            .find_map(|b| match b {
                Block::ToolCall {
                    name, args_preview, ..
                } if name == "browser_navigate" => Some(args_preview.clone()),
                _ => None,
            })
            .expect("a browser_navigate tool block was rebuilt");
        // The rebuilt args are whole + parseable, so the summary recovers the url.
        let parsed: Value =
            serde_json::from_str(&preview).expect("rebuilt browser args must parse as JSON");
        assert_eq!(parsed.get("url").and_then(|v| v.as_str()), Some(long_url));
    }

    // -- session persistence (should_restore_session pure helper) --

    #[test]
    fn should_restore_session_returns_id_when_persisted_and_no_active() {
        assert_eq!(
            should_restore_session(Some("sess-abc"), None),
            Some("sess-abc")
        );
    }

    #[test]
    fn should_restore_session_returns_none_when_already_active() {
        assert_eq!(
            should_restore_session(Some("sess-abc"), Some("other")),
            None
        );
        // Same session already active — still skip (no-op restore).
        assert_eq!(
            should_restore_session(Some("sess-abc"), Some("sess-abc")),
            None
        );
    }

    #[test]
    fn should_restore_session_returns_none_when_no_persisted() {
        assert_eq!(should_restore_session(None, None), None);
        assert_eq!(should_restore_session(None, Some("active")), None);
    }

    #[test]
    fn should_restore_session_filters_empty_string() {
        assert_eq!(should_restore_session(Some(""), None), None);
    }

    // ── Gh DTO decode tests (TASK-32 review §4) ──────────────────────
    // Lane C wire shapes for /v1/repo/github/{project_id}/* read-only
    // projections. Every path returns the same 4 envelope families.

    #[test]
    fn gh_pull_list_envelope_decodes_open_prs() {
        let json = serde_json::json!({
            "ok": true,
            "pulls": [{
                "number": 42,
                "title": "Fix race in watcher admission",
                "title_truncated": false,
                "author": "risingtides",
                "author_truncated": false,
                "branch": "feat/watcher-fix",
                "base_branch": "main",
                "head_sha": "abc123def456abc123def456abc123def456abc1",
                "draft": false,
                "labels": [
                    {"name": "bug", "color": "ff0000"},
                    {"name": "native", "color": "0088ff"}
                ],
                "labels_truncated": false,
                "state": "open",
                "created_at": "2026-07-18T10:00:00Z",
                "updated_at": "2026-07-19T09:00:00Z"
            }],
            "pulls_truncated": false,
            "page": 1,
            "per_page": 10,
            "has_more": true,
            "rate": { "remaining": 4999, "reset": 1753000000 }
        });
        let env: GhPullListEnvelope =
            serde_json::from_value(json).expect("pull list envelope must decode");
        assert!(env.ok);
        assert_eq!(env.pulls.len(), 1);
        assert_eq!(env.pulls[0].number, 42);
        assert_eq!(env.pulls[0].title, "Fix race in watcher admission");
        assert_eq!(env.pulls[0].author, "risingtides");
        assert_eq!(env.pulls[0].branch, "feat/watcher-fix");
        assert_eq!(
            env.pulls[0].head_sha,
            "abc123def456abc123def456abc123def456abc1"
        );
        assert!(!env.pulls[0].draft);
        assert_eq!(env.pulls[0].labels.len(), 2);
        assert_eq!(env.pulls[0].labels[0].name, "bug");
        assert_eq!(env.pulls[0].labels[0].color, "ff0000");
        assert_eq!(env.pulls[0].state, "open");
        assert!(env.has_more);
        assert_eq!(env.page, 1);
        assert_eq!(env.rate.remaining, 4999);
    }

    #[test]
    fn gh_pull_list_envelope_decodes_closed_empty() {
        let json = serde_json::json!({
            "ok": true,
            "pulls": [],
            "pulls_truncated": false,
            "page": 1,
            "per_page": 10,
            "has_more": false,
            "rate": { "remaining": 4980, "reset": 1753000000 }
        });
        let env: GhPullListEnvelope =
            serde_json::from_value(json).expect("empty pull list must decode");
        assert!(env.ok);
        assert!(env.pulls.is_empty());
        assert!(!env.has_more);
    }

    #[test]
    fn gh_pull_detail_envelope_decodes_full_body() {
        let json = serde_json::json!({
            "ok": true,
            "pull": {
                "number": 42,
                "title": "Fix race in watcher admission",
                "title_truncated": false,
                "author": "risingtides",
                "author_truncated": false,
                "branch": "feat/watcher-fix",
                "base_branch": "main",
                "head_sha": "abc123def456abc123def456abc123def456abc1",
                "body": "## Summary\\n\\nFixes a race where the watcher...",
                "body_truncated": false,
                "draft": true,
                "labels": [],
                "labels_truncated": false,
                "state": "open",
                "mergeable": true,
                "requested_reviewers": ["alice", "bob"],
                "requested_reviewers_truncated": false,
                "created_at": "2026-07-18T10:00:00Z",
                "updated_at": "2026-07-19T09:00:00Z"
            },
            "rate": { "remaining": 4998, "reset": 1753000000 }
        });
        let env: GhPullDetailEnvelope =
            serde_json::from_value(json).expect("pull detail must decode");
        assert!(env.ok);
        assert_eq!(env.pull.number, 42);
        assert!(env.pull.body.contains("Summary"));
        assert!(env.pull.draft);
        assert_eq!(env.pull.mergeable, Some(true));
        assert_eq!(env.pull.requested_reviewers, vec!["alice", "bob"]);
        assert_eq!(env.rate.remaining, 4998);
    }

    #[test]
    fn gh_pull_detail_envelope_omits_optional_mergeable() {
        let json = serde_json::json!({
            "ok": true,
            "pull": {
                "number": 7,
                "title": "nix",
                "author": "bot",
                "branch": "bot/nix",
                "base_branch": "main",
                "head_sha": "def4567890123456789012345678901234567890",
                "state": "open",
                "created_at": "2026-07-19T00:00:00Z",
                "updated_at": "2026-07-19T00:00:00Z"
            },
            "rate": { "remaining": 4997, "reset": 1753000000 }
        });
        let env: GhPullDetailEnvelope =
            serde_json::from_value(json).expect("minimal pull detail must decode");
        assert_eq!(env.pull.mergeable, None);
        assert!(env.pull.requested_reviewers.is_empty());
        assert!(env.pull.labels.is_empty());
    }

    #[test]
    fn gh_checks_envelope_decodes_mixed_conclusions() {
        let json = serde_json::json!({
            "ok": true,
            "checks": [
                {
                    "id": 1,
                    "name": "lint",
                    "status": "completed",
                    "conclusion": "success"
                },
                {
                    "id": 2,
                    "name": "test",
                    "status": "completed",
                    "conclusion": "failure",
                    "details_url": "https://ci.example.com/run/2"
                },
                {
                    "id": 3,
                    "name": "deploy",
                    "status": "in_progress",
                    "conclusion": null
                }
            ],
            "checks_truncated": false,
            "rate": { "remaining": 4996, "reset": 1753000000 }
        });
        let env: GhChecksEnvelope =
            serde_json::from_value(json).expect("checks envelope must decode");
        assert!(env.ok);
        assert_eq!(env.checks.len(), 3);
        assert_eq!(env.checks[0].name, "lint");
        assert_eq!(env.checks[0].status, "completed");
        assert_eq!(env.checks[0].conclusion.as_deref(), Some("success"));
        assert_eq!(env.checks[1].conclusion.as_deref(), Some("failure"));
        assert_eq!(
            env.checks[1].details_url.as_deref(),
            Some("https://ci.example.com/run/2")
        );
        assert_eq!(env.checks[2].status, "in_progress");
        assert_eq!(env.checks[2].conclusion, None);
        assert_eq!(env.checks[2].started_at, None);
        assert_eq!(env.checks[2].completed_at, None);
    }

    #[test]
    fn gh_checks_envelope_decodes_empty() {
        let json = serde_json::json!({
            "ok": true,
            "checks": [],
            "checks_truncated": false,
            "rate": { "remaining": 4995, "reset": 1753000000 }
        });
        let env: GhChecksEnvelope = serde_json::from_value(json).expect("empty checks must decode");
        assert!(env.ok);
        assert!(env.checks.is_empty());
    }

    #[test]
    fn gh_reviews_envelope_decodes_across_states() {
        let json = serde_json::json!({
            "ok": true,
            "reviews": [
                {
                    "reviewer": "alice",
                    "reviewer_truncated": false,
                    "state": "approved",
                    "body": "LGTM, ship it",
                    "body_truncated": false,
                    "submitted_at": "2026-07-18T14:00:00Z"
                },
                {
                    "reviewer": "bob",
                    "reviewer_truncated": false,
                    "state": "changes_requested",
                    "body": "",
                    "body_truncated": false,
                    "submitted_at": null
                },
                {
                    "reviewer": "carol",
                    "reviewer_truncated": false,
                    "state": "commented",
                    "body": "nit: naming",
                    "body_truncated": false,
                    "submitted_at": "2026-07-19T01:00:00Z"
                }
            ],
            "reviews_truncated": false,
            "page": 1,
            "per_page": 10,
            "has_more": false,
            "rate": { "remaining": 4994, "reset": 1753000000 }
        });
        let env: GhReviewsEnvelope =
            serde_json::from_value(json).expect("reviews envelope must decode");
        assert!(env.ok);
        assert_eq!(env.reviews.len(), 3);
        assert_eq!(env.reviews[0].reviewer, "alice");
        assert_eq!(env.reviews[0].state, "approved");
        assert_eq!(env.reviews[0].body, "LGTM, ship it");
        assert_eq!(
            env.reviews[0].submitted_at.as_deref(),
            Some("2026-07-18T14:00:00Z")
        );
        assert_eq!(env.reviews[1].state, "changes_requested");
        assert_eq!(env.reviews[1].submitted_at, None);
        assert_eq!(env.reviews[2].state, "commented");
        assert!(!env.has_more);
        assert_eq!(env.page, 1);
        assert_eq!(env.rate.remaining, 4994);
    }

    #[test]
    fn gh_reviews_envelope_decodes_forward_compat_unknown_fields() {
        // The daemon may add fields; the surface must ignore them silently.
        let json = serde_json::json!({
            "ok": true,
            "reviews": [
                {
                    "reviewer": "alice",
                    "state": "approved",
                    "body": "",
                    "future_field": "ignore me",
                    "nested": { "also": "ignore" }
                }
            ],
            "page": 1,
            "per_page": 10,
            "has_more": false,
            "rate": { "remaining": 4993, "reset": 1753000000 },
            "future_envelope_field": 42
        });
        let env: GhReviewsEnvelope =
            serde_json::from_value(json).expect("forward-compat must not break decode");
        assert!(env.ok);
        assert_eq!(env.reviews.len(), 1);
        assert_eq!(env.reviews[0].reviewer, "alice");
    }

    // ── TASK-46 stage D: sync_pending B-v3 regression matrix ────────────────────

    fn daemon_with_sync(sync_pending: bool, deadline: Option<f64>) -> Daemon {
        let daemon = Daemon::dummy();
        daemon.sync_pending.set(sync_pending);
        daemon.sync_deadline.set(deadline);
        // Seed sync_cycle so we can distinguish a bump from zero.
        daemon.sync_cycle.set(1);
        daemon
    }

    // ── 1. default state ───────────────────────────────────────────────────────

    #[test]
    fn sync_pending_fields_default_false_and_none() {
        let daemon = Daemon::dummy();
        assert!(!daemon.sync_pending.get_untracked());
        assert!(daemon.sync_deadline.get_untracked().is_none());
        assert!(!daemon.sync_in_flight.get_untracked());
    }

    // ── 2. stop_sync_poll ──────────────────────────────────────────────────────

    #[test]
    fn stop_sync_poll_clears_all_state_and_bumps_cycle() {
        let daemon = daemon_with_sync(true, Some(now_millis() + 100_000.0));
        daemon.sync_in_flight.set(true);
        let prev_cycle = daemon.sync_cycle.get_untracked();

        daemon.stop_sync_poll();

        assert!(!daemon.sync_pending.get_untracked());
        assert!(daemon.sync_deadline.get_untracked().is_none());
        assert!(!daemon.sync_in_flight.get_untracked());
        assert_eq!(
            daemon.sync_cycle.get_untracked(),
            prev_cycle.wrapping_add(1),
            "cycle must bump on stop"
        );
    }

    // ── 3. suppression gate: content events ────────────────────────────────────

    #[test]
    fn apply_event_suppresses_tool_call_started_when_sync_pending() {
        let daemon = daemon_with_sync(true, Some(now_millis() + 100_000.0));
        daemon.session_id.set(Some("s1".into()));
        // Seed one assistant turn so the tool-call reducer has a target.
        daemon.turns.set(vec![Turn::assistant("t1".into())]);
        let before = daemon.turns.with_untracked(|t| t[0].blocks.len());

        apply_test_event(
            &daemon,
            AgentEvent::ToolCallStarted {
                session_id: "s1".into(),
                turn_id: "t1".into(),
                call: ToolCallSummary {
                    id: "c1".into(),
                    name: "bash".into(),
                    args_json: serde_json::json!({}),
                },
            },
        );

        let after = daemon.turns.with_untracked(|t| t[0].blocks.len());
        assert_eq!(
            after, before,
            "ToolCallStarted suppressed during sync_pending"
        );
    }

    #[test]
    fn apply_event_suppresses_assistant_text_delta_when_sync_pending() {
        let daemon = daemon_with_sync(true, Some(now_millis() + 100_000.0));
        daemon.session_id.set(Some("s1".into()));
        daemon.turns.set(vec![Turn::assistant("t1".into())]);
        let before = daemon.turns.with_untracked(|t| t[0].blocks.len());

        apply_test_event(
            &daemon,
            AgentEvent::AssistantTextDelta {
                session_id: "s1".into(),
                turn_id: "t1".into(),
                delta: "hello".into(),
            },
        );

        let after = daemon.turns.with_untracked(|t| t[0].blocks.len());
        assert_eq!(
            after, before,
            "AssistantTextDelta suppressed during sync_pending"
        );
    }

    #[test]
    fn apply_event_suppresses_thinking_delta_when_sync_pending() {
        let daemon = daemon_with_sync(true, Some(now_millis() + 100_000.0));
        daemon.session_id.set(Some("s1".into()));
        daemon.turns.set(vec![Turn::assistant("t1".into())]);
        let before = daemon.turns.with_untracked(|t| t[0].blocks.len());

        apply_test_event(
            &daemon,
            AgentEvent::ThinkingDelta {
                session_id: "s1".into(),
                turn_id: "t1".into(),
                delta: "hmm".into(),
            },
        );

        let after = daemon.turns.with_untracked(|t| t[0].blocks.len());
        assert_eq!(
            after, before,
            "ThinkingDelta suppressed during sync_pending"
        );
    }

    #[test]
    fn apply_event_suppresses_component_render_when_sync_pending() {
        let daemon = daemon_with_sync(true, Some(now_millis() + 100_000.0));
        daemon.session_id.set(Some("s1".into()));
        daemon.turns.set(vec![Turn::assistant("t1".into())]);
        let before = daemon.turns.with_untracked(|t| t[0].blocks.len());

        apply_test_event(
            &daemon,
            AgentEvent::ComponentRender {
                session_id: "s1".into(),
                component_id: "comp1".into(),
                kind: "card".into(),
                props: serde_json::Value::Null,
                replace: false,
            },
        );

        let after = daemon.turns.with_untracked(|t| t[0].blocks.len());
        assert_eq!(
            after, before,
            "ComponentRender suppressed during sync_pending"
        );
    }

    #[test]
    fn apply_event_suppresses_component_unmount_when_sync_pending() {
        let daemon = daemon_with_sync(true, Some(now_millis() + 100_000.0));
        daemon.session_id.set(Some("s1".into()));
        let before = daemon.pinned_widgets.with_untracked(|w| w.len());

        apply_test_event(
            &daemon,
            AgentEvent::ComponentUnmount {
                session_id: "s1".into(),
                component_id: "comp1".into(),
            },
        );

        let after = daemon.pinned_widgets.with_untracked(|w| w.len());
        assert_eq!(
            after, before,
            "ComponentUnmount suppressed during sync_pending"
        );
    }

    #[test]
    fn apply_event_suppresses_tool_call_finished_when_sync_pending() {
        let daemon = daemon_with_sync(true, Some(now_millis() + 100_000.0));
        daemon.session_id.set(Some("s1".into()));
        // Seed a running tool call block so the reducer has a target.
        let mut turn = Turn::assistant("t1".into());
        turn.blocks.push(Block::ToolCall {
            call_id: "c1".into(),
            name: "bash".into(),
            args_preview: "{}".into(),
            output: String::new(),
            expanded: false,
            status: ToolStatus::Running,
        });
        daemon.turns.set(vec![turn]);

        apply_test_event(
            &daemon,
            AgentEvent::ToolCallFinished {
                session_id: "s1".into(),
                turn_id: "t1".into(),
                call_id: "c1".into(),
                result: ToolResult {
                    ok: true,
                    output: "ok".into(),
                },
            },
        );

        // The block must NOT have transitioned to Ok — suppressed.
        let state = tool_state(&daemon, "t1", "c1");
        assert_eq!(
            state,
            (ToolStatus::Running, false),
            "ToolCallFinished suppressed during sync_pending — tool still Running"
        );
    }

    // ── 5. suppression gate: state/Stop flow through ───────────────────────────

    #[test]
    fn apply_event_passes_turn_started_when_sync_pending() {
        let daemon = daemon_with_sync(true, Some(now_millis() + 100_000.0));
        daemon.session_id.set(Some("s1".into()));

        apply_test_event(
            &daemon,
            AgentEvent::TurnStarted {
                turn_id: "t1".into(),
                session_id: "s1".into(),
                model: None,
            },
        );

        // TurnStarted bumps activity_revision even during sync_pending.
        assert!(daemon.activity_revision.get_untracked() > 0);
    }

    #[test]
    fn apply_event_passes_turn_finished_when_sync_pending() {
        let daemon = daemon_with_sync(true, Some(now_millis() + 100_000.0));
        daemon.session_id.set(Some("s1".into()));
        daemon.active_turn_id.set(Some("t1".into()));
        let prev_rev = daemon.activity_revision.get_untracked();

        apply_test_event(
            &daemon,
            AgentEvent::TurnFinished {
                session_id: "s1".into(),
                turn_id: "t1".into(),
                status: "completed".into(),
                error: None,
                wall_ms: Some(100),
                output_tokens: None,
                input_tokens: None,
                cache_read_tokens: None,
                tokens_per_second: None,
            },
        );

        // Activity revision bumps (terminal track).
        assert!(daemon.activity_revision.get_untracked() > prev_rev);
    }

    #[test]
    fn apply_event_passes_session_created_when_sync_pending() {
        let daemon = daemon_with_sync(true, Some(now_millis() + 100_000.0));
        daemon.session_id.set(Some("s1".into()));

        apply_test_event(
            &daemon,
            AgentEvent::SessionCreated {
                session_id: "s1".into(),
                title: "test session".into(),
                cwd: "/work".into(),
            },
        );

        assert_eq!(daemon.session_title.get_untracked(), "test session");
        assert_eq!(daemon.cwd.get_untracked(), "/work");
        assert!(!daemon.awaiting_session_adoption.get_untracked());
    }

    // ── 6. TurnFinished kick integration ───────────────────────────────────────

    #[test]
    fn turn_finished_kicks_sync_poll_when_sync_pending_true() {
        let daemon = daemon_with_sync(true, Some(now_millis() + 100_000.0));
        daemon.session_id.set(Some("s1".into()));
        let prev_kick = daemon.sync_poll_kick.get_untracked();

        apply_test_event(
            &daemon,
            AgentEvent::TurnFinished {
                session_id: "s1".into(),
                turn_id: "t1".into(),
                status: "completed".into(),
                error: None,
                wall_ms: Some(100),
                output_tokens: None,
                input_tokens: None,
                cache_read_tokens: None,
                tokens_per_second: None,
            },
        );

        assert_eq!(
            daemon.sync_poll_kick.get_untracked(),
            prev_kick.wrapping_add(1),
            "TurnFinished must bump sync_poll_kick when sync_pending"
        );
    }

    #[test]
    fn turn_finished_does_not_kick_when_sync_pending_false() {
        let daemon = daemon_with_sync(false, None);
        daemon.session_id.set(Some("s1".into()));
        let prev_kick = daemon.sync_poll_kick.get_untracked();

        apply_test_event(
            &daemon,
            AgentEvent::TurnFinished {
                session_id: "s1".into(),
                turn_id: "t1".into(),
                status: "completed".into(),
                error: None,
                wall_ms: Some(100),
                output_tokens: None,
                input_tokens: None,
                cache_read_tokens: None,
                tokens_per_second: None,
            },
        );

        assert_eq!(
            daemon.sync_poll_kick.get_untracked(),
            prev_kick,
            "kick must not change when sync_pending is false"
        );
    }

    #[test]
    fn turn_finished_bumps_activity_revision_when_sync_pending() {
        let daemon = daemon_with_sync(true, Some(now_millis() + 100_000.0));
        daemon.session_id.set(Some("s1".into()));
        let prev_rev = daemon.activity_revision.get_untracked();

        apply_test_event(
            &daemon,
            AgentEvent::TurnFinished {
                session_id: "s1".into(),
                turn_id: "t2".into(),
                status: "completed".into(),
                error: None,
                wall_ms: Some(100),
                output_tokens: None,
                input_tokens: None,
                cache_read_tokens: None,
                tokens_per_second: None,
            },
        );

        assert!(
            daemon.activity_revision.get_untracked() > prev_rev,
            "activity_revision must bump on TurnFinished"
        );
    }

    // ── 7. suppression gate: non-matching session ──────────────────────────────

    #[test]
    fn apply_event_suppression_does_not_apply_to_wrong_session() {
        // sync_pending is true for session "s1", but event targets "s2" — gate
        // is keyed on event.session_id(), so suppression should not trigger.
        let daemon = daemon_with_sync(true, Some(now_millis() + 100_000.0));
        daemon.session_id.set(Some("s1".into()));
        daemon.turns.set(vec![Turn::assistant("t1".into())]);
        let before = daemon.turns.with_untracked(|t| t[0].blocks.len());

        // Event for a different session — the session_id guard fires first.
        apply_test_event(
            &daemon,
            AgentEvent::ToolCallStarted {
                session_id: "s2".into(),
                turn_id: "t2".into(),
                call: ToolCallSummary {
                    id: "c2".into(),
                    name: "read".into(),
                    args_json: serde_json::json!({}),
                },
            },
        );

        let after = daemon.turns.with_untracked(|t| t[0].blocks.len());
        assert_eq!(
            after, before,
            "event for wrong session dropped before suppression gate"
        );
    }

    // ── 8. activity_revision tracking ──────────────────────────────────────────

    #[test]
    fn turn_started_bumps_activity_revision() {
        let daemon = Daemon::dummy();
        daemon.session_id.set(Some("s1".into()));
        let prev = daemon.activity_revision.get_untracked();

        apply_test_event(
            &daemon,
            AgentEvent::TurnStarted {
                turn_id: "t1".into(),
                session_id: "s1".into(),
                model: None,
            },
        );

        assert!(daemon.activity_revision.get_untracked() > prev);
    }

    // ── 9. activity_revision race: admission deciders ──────────────────────────

    #[test]
    fn admit_activity_snapshot_discards_on_revision_mismatch() {
        // Same session + gen, but activity_revision bumped (TurnStarted/TurnFinished).
        assert_eq!(
            admit_activity_snapshot("s1", 1, 5, Some("s1"), 1, 7),
            SnapshotAdmission::Discard,
            "bumped activity_revision must discard"
        );
    }

    #[test]
    fn admit_activity_snapshot_discards_wrong_session() {
        assert_eq!(
            admit_activity_snapshot("s1", 1, 3, Some("s2"), 1, 3),
            SnapshotAdmission::Discard,
        );
    }

    #[test]
    fn admit_activity_snapshot_discards_wrong_generation() {
        assert_eq!(
            admit_activity_snapshot("s1", 1, 3, Some("s1"), 2, 3),
            SnapshotAdmission::Discard,
        );
    }

    #[test]
    fn admit_activity_snapshot_commits_when_all_match() {
        assert_eq!(
            admit_activity_snapshot("s1", 1, 3, Some("s1"), 1, 3),
            SnapshotAdmission::Commit,
        );
    }

    #[test]
    fn admit_sync_snapshot_discards_on_cycle_mismatch() {
        // Same session, gen, activity_revision — but sync_cycle bumped (stop+restart).
        assert_eq!(
            admit_sync_snapshot("s1", 1, 5, 3, Some("s1"), 1, 7, 3),
            SnapshotAdmission::Discard,
            "stale cycle disqualified even when activity_revision matches"
        );
    }

    #[test]
    fn admit_sync_snapshot_discards_on_activity_revision_bump() {
        // Same session, gen, cycle — but activity_revision bumped mid-fetch.
        assert_eq!(
            admit_sync_snapshot("s1", 1, 3, 5, Some("s1"), 1, 3, 7),
            SnapshotAdmission::Discard,
            "activity_revision bump during fetch must discard"
        );
    }

    #[test]
    fn admit_sync_snapshot_commits_when_all_three_match() {
        assert_eq!(
            admit_sync_snapshot("s1", 1, 3, 5, Some("s1"), 1, 3, 5),
            SnapshotAdmission::Commit,
        );
    }

    #[test]
    fn activity_revision_bump_via_turn_started_during_sync_pending() {
        // Simulated race: snapshot captured at rev 0, then TurnStarted bumps
        // activity_revision. The admission decider must discard.
        let daemon = daemon_with_sync(true, Some(now_millis() + 100_000.0));
        daemon.session_id.set(Some("s1".into()));

        // Phase 1: capture snapshot values before any event.
        let snap_rev = daemon.activity_revision.get_untracked();
        let snap_gen = daemon.sse_generation.get_untracked();
        let snap_cycle = daemon.sync_cycle.get_untracked();

        // Phase 2: TurnStarted bumps activity_revision (simulating concurrent event).
        apply_test_event(
            &daemon,
            AgentEvent::TurnStarted {
                turn_id: "t1".into(),
                session_id: "s1".into(),
                model: None,
            },
        );
        let current_rev = daemon.activity_revision.get_untracked();
        assert!(
            current_rev > snap_rev,
            "TurnStarted must bump activity_revision"
        );

        // Phase 3: the stale snapshot (snap_rev) must be discarded by admission.
        assert_eq!(
            admit_sync_snapshot(
                "s1",
                snap_gen,
                snap_cycle,
                snap_rev,
                daemon.session_id.get_untracked().as_deref(),
                daemon.sse_generation.get_untracked(),
                daemon.sync_cycle.get_untracked(),
                current_rev,
            ),
            SnapshotAdmission::Discard,
            "stale snapshot with old activity_revision must be discarded"
        );
    }

    // ── 10. production-seam: try_commit_sync_snapshot race ─────────────────────

    /// Minimal terminal session detail with no transcript — enough to exercise
    /// the admission gate and signal-write path without depending on the daemon.
    fn minimal_detail() -> SessionDetail {
        SessionDetail {
            id: "s1".into(),
            title: "test session".into(),
            model: String::new(),
            workspace_root: None,
            cwd: None,
            transcript: vec![],
            tool_context: vec![],
            pending_permissions: vec![],
            state: None, // terminal (no live turn)
            active_requests: vec![],
        }
    }

    /// Sentinel Daemon for production-seam tests: sync_pending active, known
    /// session, generation, cycle, and a seeded turn to detect writes.
    fn sentinel_daemon() -> Daemon {
        let daemon = daemon_with_sync(true, Some(now_millis() + 100_000.0));
        daemon.session_id.set(Some("s1".into()));
        daemon.sse_generation.set(1);
        daemon.sync_cycle.set(5);
        // Seed a user turn so we can detect overwrites.
        daemon.turns.set(vec![Turn::user("old turn")]);
        daemon.streaming.set(false);
        daemon.session_title.set("old title".into());
        daemon
    }

    #[test]
    fn try_commit_sync_snapshot_discards_on_revision_bump() {
        let daemon = sentinel_daemon();
        daemon.activity_revision.set(0);

        // Simulate mid-fetch bump: snapshot captured at rev 0, but a concurrent
        // TurnStarted bumped activity_revision to 1 before commit.
        daemon.activity_revision.set(1);

        let detail = minimal_detail();
        let old_turns = serde_json::to_value(daemon.turns.get_untracked()).unwrap();
        let old_title = daemon.session_title.get_untracked();

        let outcome = daemon.try_commit_sync_snapshot(
            &detail, "s1", /* sse_generation */ 1, /* sync_cycle */ 5,
            /* snapshot_activity_revision (stale) */ 0,
        );

        assert_eq!(
            outcome,
            ProjectionOutcome::Live,
            "stale activity_revision must discard, not commit"
        );

        // Zero writes: turns, title, streaming untouched.
        assert_eq!(
            serde_json::to_value(daemon.turns.get_untracked()).unwrap(),
            old_turns,
            "turns must not change on discard"
        );
        assert_eq!(
            daemon.session_title.get_untracked(),
            old_title,
            "title must not change on discard"
        );
        assert!(
            !daemon.streaming.get_untracked(),
            "streaming must stay false on discard"
        );
        // sync_pending must remain true — caller will retry next tick.
        assert!(
            daemon.sync_pending.get_untracked(),
            "sync_pending must remain true after discard (retry next tick)"
        );
    }

    #[test]
    fn try_commit_sync_snapshot_commits_when_guards_match() {
        let daemon = sentinel_daemon();
        daemon.activity_revision.set(3);

        let detail = minimal_detail();
        // Same revision → admission must Commit, writes must apply.
        let outcome = daemon.try_commit_sync_snapshot(
            &detail, "s1", /* sse_generation */ 1, /* sync_cycle */ 5,
            /* snapshot_activity_revision */ 3,
        );

        assert_eq!(
            outcome,
            ProjectionOutcome::Terminal,
            "matching guards must commit terminal snapshot"
        );

        // Signal writes applied: turns replaced, title updated.
        assert_eq!(
            daemon.session_title.get_untracked(),
            "test session",
            "title must be committed"
        );
        let committed_turns = daemon.turns.get_untracked();
        assert!(
            committed_turns.is_empty(),
            "old sentinel turn must not survive commit"
        );
    }

    #[test]
    fn try_commit_sync_snapshot_discards_on_session_mismatch() {
        let daemon = sentinel_daemon();
        daemon.activity_revision.set(0);
        // Switch to different session after snapshot captured.
        daemon.session_id.set(Some("s2".into()));

        let detail = minimal_detail();
        let old_turns = serde_json::to_value(daemon.turns.get_untracked()).unwrap();

        let outcome = daemon.try_commit_sync_snapshot(
            &detail, "s1", // snapshot captured for s1, but current is s2
            1, 5, 0,
        );

        assert_eq!(outcome, ProjectionOutcome::Live);
        assert_eq!(
            serde_json::to_value(daemon.turns.get_untracked()).unwrap(),
            old_turns,
            "zero writes on session switch"
        );
        assert!(
            daemon.sync_pending.get_untracked(),
            "sync_pending stays true"
        );
    }

    #[test]
    fn try_commit_sync_snapshot_discards_on_generation_mismatch() {
        let daemon = sentinel_daemon();
        daemon.activity_revision.set(0);
        // New SSE generation after snapshot captured.
        daemon.sse_generation.set(2);

        let detail = minimal_detail();
        let old_turns = serde_json::to_value(daemon.turns.get_untracked()).unwrap();

        let outcome = daemon.try_commit_sync_snapshot(
            &detail, "s1", 1, // snapshot_gen=1, current=2
            5, 0,
        );

        assert_eq!(outcome, ProjectionOutcome::Live);
        assert_eq!(
            serde_json::to_value(daemon.turns.get_untracked()).unwrap(),
            old_turns
        );
        assert!(daemon.sync_pending.get_untracked());
    }

    #[test]
    fn try_commit_sync_snapshot_discards_on_cycle_mismatch() {
        let daemon = sentinel_daemon();
        daemon.activity_revision.set(0);
        // stop_sync_poll + restart bumped the cycle.
        daemon.sync_cycle.set(7);

        let detail = minimal_detail();
        let old_turns = serde_json::to_value(daemon.turns.get_untracked()).unwrap();

        let outcome = daemon.try_commit_sync_snapshot(
            &detail, "s1", 1, 5, // snapshot_cycle=5, current=7
            0,
        );

        assert_eq!(outcome, ProjectionOutcome::Live);
        assert_eq!(
            serde_json::to_value(daemon.turns.get_untracked()).unwrap(),
            old_turns
        );
        assert!(daemon.sync_pending.get_untracked());
    }

    // ── 11. stage G: release_sync_fetch — keyed cleanup helper ─────

    /// Matching completion: `release_sync_fetch` clears both the abort
    /// handle and in_flight, and returns `true`.
    #[test]
    fn release_sync_fetch_matching_clears_both_signals() {
        let daemon = sentinel_daemon();
        daemon.session_id.set(Some("s1".into()));
        daemon.sse_generation.set(1);
        daemon.sync_cycle.set(5);
        daemon.sync_in_flight.set(true);

        let (_fut, handle) = abortable(async { ProjectionOutcome::Live });
        daemon.sync_fetch_abort.set(Some(handle));

        let matched = daemon.release_sync_fetch("s1", 1, 5);

        assert!(matched, "matching must return true");
        assert!(
            daemon.sync_fetch_abort.get_untracked().is_none(),
            "matching must clear abort handle"
        );
        assert!(
            !daemon.sync_in_flight.get_untracked(),
            "matching must clear in_flight"
        );
    }

    /// Mismatched (cycle bumped): `release_sync_fetch` returns `false`
    /// and leaves BOTH signals untouched.
    #[test]
    fn release_sync_fetch_mismatched_leaves_both_signals_intact() {
        let daemon = sentinel_daemon();
        daemon.session_id.set(Some("s1".into()));
        daemon.sse_generation.set(1);
        daemon.sync_cycle.set(5);
        daemon.sync_in_flight.set(true);

        let (_fut, handle) = abortable(async { ProjectionOutcome::Live });
        daemon.sync_fetch_abort.set(Some(handle));

        // Bump the cycle — simulates stop_sync_poll + restart.
        daemon.sync_cycle.set(7);

        let in_flight_before = daemon.sync_in_flight.get_untracked();
        let handle_before = daemon.sync_fetch_abort.get_untracked().is_some();

        let matched = daemon.release_sync_fetch("s1", 1, 5);

        assert!(!matched, "mismatched must return false");
        assert_eq!(
            daemon.sync_in_flight.get_untracked(),
            in_flight_before,
            "mismatched must not clear in_flight"
        );
        assert_eq!(
            daemon.sync_fetch_abort.get_untracked().is_some(),
            handle_before,
            "mismatched must not clear abort handle"
        );
    }

    /// Retired Ok: stop_sync_poll cleared in_flight before the completion
    /// runs. `release_sync_fetch` sees a cycle mismatch and leaves both
    /// signals alone — the handle stays with the newer cycle.
    #[test]
    fn release_sync_fetch_retired_ok_leaves_both_signals_intact() {
        let daemon = sentinel_daemon();
        daemon.session_id.set(Some("s1".into()));
        daemon.sse_generation.set(1);
        daemon.sync_cycle.set(5);
        // stop_sync_poll already cleared in_flight
        daemon.sync_in_flight.set(false);

        let (_fut, handle) = abortable(async { ProjectionOutcome::Live });
        daemon.sync_fetch_abort.set(Some(handle));

        // Cycle bumped by restart.
        daemon.sync_cycle.set(7);

        let in_flight_before = daemon.sync_in_flight.get_untracked();
        let handle_before = daemon.sync_fetch_abort.get_untracked().is_some();

        let matched = daemon.release_sync_fetch("s1", 1, 5);

        assert!(!matched, "retired Ok must return false");
        assert_eq!(
            daemon.sync_in_flight.get_untracked(),
            in_flight_before,
            "retired Ok must not toggle in_flight"
        );
        assert_eq!(
            daemon.sync_fetch_abort.get_untracked().is_some(),
            handle_before,
            "retired Ok must not clear abort handle"
        );
    }

    /// Retired Aborted: like retired Ok but from the Err(aborted) arm.
    /// The cycle bump makes completion stale — both signals stay intact.
    #[test]
    fn release_sync_fetch_retired_aborted_leaves_both_signals_intact() {
        let daemon = sentinel_daemon();
        daemon.session_id.set(Some("s1".into()));
        daemon.sse_generation.set(1);
        daemon.sync_cycle.set(5);
        // stop_sync_poll already cleared in_flight
        daemon.sync_in_flight.set(false);

        let (_fut, handle) = abortable(async { ProjectionOutcome::Live });
        daemon.sync_fetch_abort.set(Some(handle));

        // Cycle bumped.
        daemon.sync_cycle.set(7);

        let in_flight_before = daemon.sync_in_flight.get_untracked();
        let handle_before = daemon.sync_fetch_abort.get_untracked().is_some();

        daemon.release_sync_fetch("s1", 1, 5);

        assert_eq!(
            daemon.sync_in_flight.get_untracked(),
            in_flight_before,
            "retired Aborted must not change in_flight"
        );
        assert_eq!(
            daemon.sync_fetch_abort.get_untracked().is_some(),
            handle_before,
            "retired Aborted must not clear abort handle"
        );
    }
}
