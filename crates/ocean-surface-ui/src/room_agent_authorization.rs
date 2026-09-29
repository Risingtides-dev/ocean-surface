//! Rooms Phase 1 — local room-agent authorization ceremony.
//!
//! This module is deliberately separate from `agents.rs`: an agent package on
//! disk is code that *could* be authorized, while a room binding is durable,
//! revocable local execution authority. Package previews and binding reads are
//! credential-free. Browser-PWA mutations use the same-origin Surface proxy,
//! which injects the daemon's mode-0600 operator key server-side; browser code
//! never reads or stores that credential. The Tauri shell injects the same key
//! natively, beside the daemon it supervises. The Chrome extension has neither
//! and remains explicitly read-only.
//!
//! Every mutation on that privileged allowlist — bootstrap, authorize,
//! reauthorize, suspend, resume, revoke — leaves this module through ONE
//! transport seam, [`send_authority_mutation`], addressed by an
//! [`AuthorityRoute`] that nothing else constructs and nothing else consumes.
//! That is what keeps the host branch in one place instead of once per verb.
//! Credential-free calls (binding inspection, package preview, federated
//! membership registration) keep their ordinary same-origin `Request`.

use std::collections::HashSet;

use gloo_net::http::Request;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::rooms::{
    encode, Room, RoomAccessProjection, RoomAccessState, RoomParticipantKind, Rooms,
};

/// Kept in one function while ocean-os and Surface land concurrently. The
/// daemon owns the preview: the client must never reproduce its digest or infer
/// a room member identity from a package name.
fn package_preview_url(base: &str, room: &str, package: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/agents/preview/{}",
        encode(room),
        encode(package)
    )
}

fn bindings_path(room: &str) -> String {
    format!("/v1/rooms/persistent/{}/agents", encode(room))
}

/// Binding INSPECTION is credential-free and stays an ordinary same-origin
/// GET on every host, so it is addressed by a URL rather than a route.
fn bindings_url(base: &str, room: &str) -> String {
    format!("{base}{}", bindings_path(room))
}

/// One privileged daemon route on the Room-agent authority allowlist, held as
/// a PATH and never as a full URL.
///
/// The type IS the seam. It is constructed only by the four builders below and
/// consumed only by [`send_authority_mutation`], so a privileged mutation
/// cannot reach the network by a path the transport did not choose — the
/// compiler holds that, not a reviewer. Holding a path rather than a URL is
/// what lets the native shell supply the origin itself: the webview names the
/// route, the shell names the daemon.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AuthorityRoute(String);

impl AuthorityRoute {
    fn path(&self) -> &str {
        &self.0
    }
}

/// `POST .../agents` — authorize a first binding for a resolved member.
fn bindings_route(room: &str) -> AuthorityRoute {
    AuthorityRoute(bindings_path(room))
}

/// `POST .../agents/bootstrap` — establish durable local first-agent authority.
fn bootstrap_route(room: &str) -> AuthorityRoute {
    AuthorityRoute(format!(
        "/v1/rooms/persistent/{}/agents/bootstrap",
        encode(room)
    ))
}

/// `POST .../agents/<member>/<reauthorize|suspend|resume>`.
fn binding_action_route(room: &str, member: &str, action: &str) -> AuthorityRoute {
    AuthorityRoute(format!(
        "/v1/rooms/persistent/{}/agents/{}/{}",
        encode(room),
        encode(member),
        action
    ))
}

/// `DELETE .../agents/<member>` — revoke.
fn binding_route(room: &str, member: &str) -> AuthorityRoute {
    AuthorityRoute(format!(
        "/v1/rooms/persistent/{}/agents/{}",
        encode(room),
        encode(member)
    ))
}

/// The two verbs the authority allowlist admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthorityMethod {
    Post,
    Delete,
}

impl AuthorityMethod {
    fn as_str(self) -> &'static str {
        match self {
            Self::Post => "POST",
            Self::Delete => "DELETE",
        }
    }
}

/// Why a privileged mutation never produced a daemon answer. The two arms are
/// the two the call sites already worded differently: a request that could not
/// be constructed, and one that was constructed and not delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthorityFailure {
    Build,
    Send,
}

/// A daemon answer to a privileged mutation: status plus body text, undecoded.
/// Both transports produce exactly this, which is why one decode path serves
/// both hosts.
struct AuthorityReply {
    status: u16,
    body: String,
}

/// The ONE seam every privileged Room-agent authority mutation takes.
///
/// On the Tauri shell it hands method, route and body to
/// `daemon_operator_request`, which attaches the mode-0600 operator key and
/// supplies the daemon origin natively. Everywhere else it is the ordinary
/// same-origin `Request` the browser PWA has always sent, joined onto `base`,
/// where the Surface proxy injects the same key server-side. Neither path
/// exposes the credential to this module.
async fn send_authority_mutation(
    base: &str,
    method: AuthorityMethod,
    route: AuthorityRoute,
    body: String,
) -> Result<AuthorityReply, AuthorityFailure> {
    if crate::host::room_authority_native_transport() {
        return match crate::host::daemon_operator_request(method.as_str(), route.path(), &body)
            .await
        {
            Some(reply) => Ok(AuthorityReply {
                status: reply.status,
                body: reply.body,
            }),
            None => Err(AuthorityFailure::Send),
        };
    }
    let url = format!("{base}{}", route.path());
    let request = match method {
        AuthorityMethod::Post => Request::post(&url),
        AuthorityMethod::Delete => Request::delete(&url),
    }
    .header("content-type", "application/json")
    .body(body)
    .map_err(|_| AuthorityFailure::Build)?;
    let response = request.send().await.map_err(|_| AuthorityFailure::Send)?;
    let status = response.status();
    let body = response.text().await.map_err(|_| AuthorityFailure::Send)?;
    Ok(AuthorityReply { status, body })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BindingStatus {
    Active,
    Suspended,
    Stale,
    Revoked,
    #[serde(other)]
    Unavailable,
}

impl BindingStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Stale => "stale",
            Self::Revoked => "revoked",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct RoomAgentBinding {
    pub room_id: String,
    pub agent_member_id: String,
    pub agent_package_id: String,
    pub agent_definition_digest: String,
    #[serde(default)]
    pub agent_definition_revision: Option<String>,
    pub display_name: String,
    pub owner_member_id: String,
    pub activation_policy: String,
    pub context_policy: String,
    pub memory_scope: String,
    #[serde(default)]
    pub requested_capabilities: Vec<String>,
    #[serde(default)]
    pub room_capability_grants: Vec<String>,
    pub status: BindingStatus,
    #[serde(default)]
    pub owner_eligible: bool,
    #[serde(default)]
    pub generation: serde_json::Value,
}

impl RoomAgentBinding {
    fn generation_label(&self) -> String {
        self.generation
            .as_str()
            .map(str::to_owned)
            .or_else(|| self.generation.as_u64().map(|value| value.to_string()))
            .unwrap_or_else(|| "?".to_owned())
    }
}

#[derive(Debug, Deserialize)]
struct BindingsResponse {
    ok: bool,
    #[serde(default)]
    owner_eligible: bool,
    #[serde(default)]
    bindings: Vec<RoomAgentBinding>,
    #[serde(default)]
    error: Option<String>,
}

/// Server-derived preview. Required identity fields intentionally have no
/// serde defaults: a daemon that cannot prove them is unavailable, never a
/// prompt for the browser to fill the blanks.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct AgentPackagePreview {
    pub agent_package_id: String,
    pub agent_member_id: Option<String>,
    pub owner_member_id: Option<String>,
    pub display_name: String,
    pub agent_definition_digest: String,
    #[serde(default)]
    pub requested_capabilities: Vec<String>,
    #[serde(default)]
    pub grantable_capabilities: Vec<String>,
    #[serde(default)]
    pub unavailable_capabilities: Vec<UnavailableCapability>,
    #[serde(default)]
    pub binding: Option<RoomAgentBinding>,
    /// False means the daemon could resolve the package for display but could
    /// not prove this operator owns a stable room member for it.
    pub owner_eligible: bool,
}

impl AgentPackagePreview {
    fn valid_for(&self, package: &str) -> bool {
        self.agent_package_id == package && valid_sha256_digest(&self.agent_definition_digest)
    }

    fn authorization_ready(&self) -> bool {
        self.owner_eligible
            && self
                .agent_member_id
                .as_deref()
                .is_some_and(|member| !member.trim().is_empty())
            && self
                .owner_member_id
                .as_deref()
                .is_some_and(|owner| !owner.trim().is_empty())
    }

    fn allows_decision(&self, reauthorize_member_id: Option<&str>) -> bool {
        if !self.authorization_ready() {
            return false;
        }
        match (&self.binding, reauthorize_member_id) {
            (None, None) => true,
            (Some(binding), Some(member_id)) => {
                binding.status == BindingStatus::Stale
                    && binding.agent_member_id == member_id
                    && self.agent_member_id.as_deref() == Some(member_id)
            }
            _ => false,
        }
    }
}

fn valid_sha256_digest(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct UnavailableCapability {
    pub capability: String,
    pub reason: String,
}

#[derive(Debug, Deserialize)]
struct PreviewResponse {
    ok: bool,
    #[serde(default)]
    package_id: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    definition_digest: Option<String>,
    #[serde(default)]
    requested_capabilities: Vec<String>,
    #[serde(default)]
    grantable_capabilities: Vec<String>,
    #[serde(default)]
    unavailable_capabilities: Vec<UnavailableCapability>,
    #[serde(default)]
    binding: Option<RoomAgentBinding>,
    #[serde(default)]
    agent_member_id: Option<String>,
    #[serde(default)]
    owner_member_id: Option<String>,
    #[serde(default)]
    owner_eligible: bool,
    #[serde(default)]
    error: Option<String>,
}

impl PreviewResponse {
    fn into_preview(self) -> Option<AgentPackagePreview> {
        Some(AgentPackagePreview {
            agent_package_id: self.package_id?,
            agent_member_id: self.agent_member_id,
            owner_member_id: self.owner_member_id,
            display_name: self.display_name?,
            agent_definition_digest: self.definition_digest?,
            requested_capabilities: self.requested_capabilities,
            grantable_capabilities: self.grantable_capabilities,
            unavailable_capabilities: self.unavailable_capabilities,
            binding: self.binding,
            owner_eligible: self.owner_eligible,
        })
    }
}

#[derive(Debug, Deserialize)]
struct BindingMutationResponse {
    ok: bool,
    #[serde(default)]
    binding: Option<RoomAgentBinding>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct AuthorizeBody<'a> {
    agent_member_id: &'a str,
    agent_package_id: &'a str,
    owner_member_id: &'a str,
    decision_id: &'a str,
    activation_policy: &'a str,
    context_policy: &'a str,
    memory_scope: &'a str,
    room_capability_grants: &'a [String],
}

#[derive(Debug, Serialize)]
struct ReauthorizeBody<'a> {
    decision_id: &'a str,
    activation_policy: &'a str,
    context_policy: &'a str,
    memory_scope: &'a str,
    room_capability_grants: &'a [String],
}

#[derive(Debug, Serialize)]
struct StatusBody<'a> {
    decision_id: &'a str,
}

#[derive(Debug, Serialize)]
struct BootstrapBody<'a> {
    owner_member_id: &'a str,
    agent_package_id: &'a str,
}

#[derive(Debug, Serialize)]
struct FederatedRegistrationBody<'a> {
    agent_names: &'a [String],
}

#[derive(Debug, Deserialize)]
struct BootstrapResponse {
    ok: bool,
    #[serde(default)]
    created: Option<bool>,
    #[serde(default)]
    room_id: Option<String>,
    #[serde(default)]
    owner_member_id: Option<String>,
    #[serde(default)]
    agent_member_id: Option<String>,
    #[serde(default)]
    agent_package_id: Option<String>,
    #[serde(default)]
    owner_eligible: bool,
    #[serde(default)]
    room: Option<Room>,
    #[serde(default)]
    package_preview: Option<PreviewResponse>,
    #[serde(default)]
    error: Option<String>,
}

impl BootstrapResponse {
    fn into_server_projection(
        self,
        room_key: &str,
        owner_member_id: &str,
        agent_package_id: &str,
    ) -> Option<(Room, AgentPackagePreview)> {
        if self.created.is_none()
            || self.room_id.as_deref() != Some(room_key)
            || self.owner_member_id.as_deref() != Some(owner_member_id)
            || self.agent_member_id.as_deref() != Some(agent_package_id)
            || self.agent_package_id.as_deref() != Some(agent_package_id)
            || !self.owner_eligible
        {
            return None;
        }
        let room = self.room?;
        if room.id != room_key
            || !room.participants.iter().any(|participant| {
                participant.id == owner_member_id && participant.kind == RoomParticipantKind::Human
            })
            || !room.participants.iter().any(|participant| {
                participant.id == agent_package_id && participant.kind == RoomParticipantKind::Agent
            })
        {
            return None;
        }
        let preview = self.package_preview?.into_preview()?;
        if !preview.valid_for(agent_package_id)
            || !preview.allows_decision(None)
            || preview.owner_member_id.as_deref() != Some(owner_member_id)
            || preview.agent_member_id.as_deref() != Some(agent_package_id)
        {
            return None;
        }
        Some((room, preview))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoticeState {
    Requested,
    Active,
    Revoked,
    Denied,
    Unavailable,
}

impl NoticeState {
    fn label(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Active => "active",
            Self::Revoked => "revoked",
            Self::Denied => "denied",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Notice {
    state: NoticeState,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingStatusDecision {
    room: String,
    agent_member_id: String,
    action: String,
    decision_id: String,
}

fn pending_decision_id<'a>(
    pending: Option<&'a PendingStatusDecision>,
    room: &str,
    agent_member_id: &str,
    action: &str,
) -> Option<&'a str> {
    pending
        .filter(|pending| {
            pending.room == room
                && pending.agent_member_id == agent_member_id
                && pending.action == action
        })
        .map(|pending| pending.decision_id.as_str())
}

fn classify_error(status: u16, error: Option<String>) -> Notice {
    let message = error.unwrap_or_else(|| format!("authorization request rejected ({status})"));
    let state = match status {
        401 | 403 | 409 => NoticeState::Denied,
        503 => NoticeState::Unavailable,
        _ => NoticeState::Unavailable,
    };
    Notice { state, message }
}

fn authority_mutations_supported_on_this_host() -> bool {
    crate::host::room_authority_mutations_supported()
}

/// This is an affordance check, never ownership proof. It only exposes the
/// atomic bootstrap attempt for a resolved Human who is already present in a
/// local room with no binding. The operator-authenticated daemon route alone
/// decides whether that Human may establish or replay the durable owner role.
fn local_owner_bootstrap_candidate(
    access: Option<&RoomAccessProjection>,
    room: Option<&Room>,
    identity_resolved: bool,
    identity_id: &str,
    bindings: &[RoomAgentBinding],
) -> bool {
    identity_resolved
        && bindings.is_empty()
        && access.is_some_and(|access| access.state == RoomAccessState::Local)
        && room.is_some_and(|room| {
            room.participants.iter().any(|participant| {
                participant.id == identity_id && participant.kind == RoomParticipantKind::Human
            })
        })
}

fn rooms_local_owner_bootstrap_candidate(rooms: Rooms, bindings: &[RoomAgentBinding]) -> bool {
    local_owner_bootstrap_candidate(
        rooms.access.get_untracked().as_ref(),
        rooms.open_room.get_untracked().as_ref(),
        rooms.identity_resolved(),
        &rooms.identity_id.get_untracked(),
        bindings,
    )
}

fn caller_can_authorize(rooms: Rooms, owner_eligible: bool) -> bool {
    authority_mutations_supported_on_this_host() && rooms.identity_resolved() && owner_eligible
}

fn caller_can_configure(rooms: Rooms, owner_eligible: bool, bootstrap_candidate: bool) -> bool {
    authority_mutations_supported_on_this_host()
        && (caller_can_authorize(rooms, owner_eligible) || bootstrap_candidate)
}

fn grant_is_allowed(preview: &AgentPackagePreview, grant: &str) -> bool {
    preview
        .grantable_capabilities
        .iter()
        .any(|grantable| grantable == grant)
        && preview
            .requested_capabilities
            .iter()
            .any(|requested| requested == grant)
}

fn canonical_grants(preview: &AgentPackagePreview, grants: &[String]) -> Vec<String> {
    let mut narrowed: Vec<String> = grants
        .iter()
        .filter(|grant| grant_is_allowed(preview, grant))
        .cloned()
        .collect();
    narrowed.sort();
    narrowed.dedup();
    narrowed
}

fn uuid_v4_from_bytes(mut bytes: [u8; 16]) -> String {
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

fn mint_decision_id() -> Result<String, &'static str> {
    let crypto = web_sys::window()
        .and_then(|window| window.crypto().ok())
        .ok_or("secure randomness is unavailable")?;
    let buffer = js_sys::Uint8Array::new_with_length(16);
    crypto
        .get_random_values_with_array_buffer_view(&buffer)
        .map_err(|_| "secure randomness failed")?;
    let bytes: [u8; 16] = buffer
        .to_vec()
        .try_into()
        .map_err(|_| "secure randomness failed")?;
    Ok(uuid_v4_from_bytes(bytes))
}

#[derive(Clone, Copy)]
pub(crate) struct RoomAgentAuthorizationState {
    bindings: RwSignal<Vec<RoomAgentBinding>>,
    owner_eligible: RwSignal<bool>,
    loaded: RwSignal<bool>,
    busy: RwSignal<bool>,
    notice: RwSignal<Option<Notice>>,
    selected_package: RwSignal<String>,
    preview: RwSignal<Option<AgentPackagePreview>>,
    activation_policy: RwSignal<String>,
    context_policy: RwSignal<String>,
    memory_scope: RwSignal<String>,
    grants: RwSignal<Vec<String>>,
    decision_id: RwSignal<Option<String>>,
    reauthorize_member_id: RwSignal<Option<String>>,
    previous_digest: RwSignal<Option<String>>,
    pending_status_decision: RwSignal<Option<PendingStatusDecision>>,
    op: RwSignal<u64>,
}

impl RoomAgentAuthorizationState {
    pub(crate) fn new() -> Self {
        Self {
            bindings: RwSignal::new(Vec::new()),
            owner_eligible: RwSignal::new(false),
            loaded: RwSignal::new(false),
            busy: RwSignal::new(false),
            notice: RwSignal::new(None),
            selected_package: RwSignal::new(String::new()),
            preview: RwSignal::new(None),
            activation_policy: RwSignal::new("explicit_only".to_owned()),
            context_policy: RwSignal::new("invocation_only".to_owned()),
            memory_scope: RwSignal::new("none".to_owned()),
            grants: RwSignal::new(Vec::new()),
            decision_id: RwSignal::new(None),
            reauthorize_member_id: RwSignal::new(None),
            previous_digest: RwSignal::new(None),
            pending_status_decision: RwSignal::new(None),
            op: RwSignal::new(0),
        }
    }

    fn next_op(&self) -> u64 {
        self.op.update(|op| *op = op.wrapping_add(1));
        self.op.get_untracked()
    }

    fn current(&self, op: u64, rooms: Rooms, generation: u64, room: &str) -> bool {
        self.op.get_untracked() == op && rooms.room_is_current(generation, room)
    }

    pub(crate) fn active_agent_member_ids(&self) -> HashSet<String> {
        self.bindings
            .get()
            .into_iter()
            .filter(|binding| binding.status == BindingStatus::Active)
            .map(|binding| binding.agent_member_id)
            .collect()
    }

    pub(crate) fn active_agent_member_ids_untracked(&self) -> HashSet<String> {
        self.bindings
            .get_untracked()
            .into_iter()
            .filter(|binding| binding.status == BindingStatus::Active)
            .map(|binding| binding.agent_member_id)
            .collect()
    }

    fn reset_form(&self) {
        self.selected_package.set(String::new());
        self.preview.set(None);
        self.activation_policy.set("explicit_only".to_owned());
        self.context_policy.set("invocation_only".to_owned());
        self.memory_scope.set("none".to_owned());
        self.grants.set(Vec::new());
        self.decision_id.set(None);
        self.reauthorize_member_id.set(None);
        self.previous_digest.set(None);
    }

    fn load_bindings(&self, rooms: Rooms, room: String, generation: u64) {
        let op = self.next_op();
        if self
            .pending_status_decision
            .get_untracked()
            .as_ref()
            .is_some_and(|pending| pending.room != room)
        {
            self.pending_status_decision.set(None);
        }
        self.bindings.set(Vec::new());
        self.owner_eligible.set(false);
        self.loaded.set(false);
        self.busy.set(true);
        self.notice.set(None);
        self.reset_form();
        let me = *self;
        let url = bindings_url(&rooms.url.get_untracked(), &room);
        wasm_bindgen_futures::spawn_local(async move {
            let outcome = match Request::get(&url).send().await {
                Ok(response) => {
                    let status = response.status();
                    match response.json::<BindingsResponse>().await {
                        Ok(body) if response_ok(status, body.ok) => {
                            Ok((body.bindings, body.owner_eligible))
                        }
                        Ok(body) => Err(classify_error(status, body.error)),
                        Err(_) => Err(Notice {
                            state: NoticeState::Unavailable,
                            message: "binding inspection returned invalid data".to_owned(),
                        }),
                    }
                }
                Err(_) => Err(Notice {
                    state: NoticeState::Unavailable,
                    message: "binding inspection is unavailable".to_owned(),
                }),
            };
            if !me.current(op, rooms, generation, &room) {
                return;
            }
            me.busy.set(false);
            me.loaded.set(true);
            match outcome {
                Ok((bindings, owner_eligible)) => {
                    me.bindings.set(bindings);
                    me.owner_eligible.set(owner_eligible);
                }
                Err(notice) => me.notice.set(Some(notice)),
            }
        });
    }

    fn select_package(&self, rooms: Rooms, package: String) {
        self.decision_id.set(None);
        self.reauthorize_member_id.set(None);
        self.previous_digest.set(None);
        self.grants.set(Vec::new());
        self.preview.set(None);
        self.selected_package.set(package.clone());
        if package.is_empty() {
            self.notice.set(None);
            return;
        }
        if rooms_local_owner_bootstrap_candidate(rooms, &self.bindings.get_untracked()) {
            self.bootstrap_local_package(rooms, package);
        } else {
            self.load_preview(rooms, package);
        }
    }

    fn bootstrap_local_package(&self, rooms: Rooms, package: String) {
        let Some(room) = rooms.open_key.get_untracked() else {
            return;
        };
        let owner = rooms.identity_id.get_untracked();
        if !rooms_local_owner_bootstrap_candidate(rooms, &self.bindings.get_untracked()) {
            self.notice.set(Some(Notice {
                state: NoticeState::Denied,
                message: "the local room is not eligible for first-agent bootstrap".to_owned(),
            }));
            return;
        }
        let body = match serde_json::to_string(&BootstrapBody {
            owner_member_id: &owner,
            agent_package_id: &package,
        }) {
            Ok(body) => body,
            Err(_) => return,
        };
        let generation = rooms.generation_snapshot();
        let op = self.next_op();
        self.busy.set(true);
        self.notice.set(Some(Notice {
            state: NoticeState::Requested,
            message: "Establishing first-agent authority".to_owned(),
        }));
        let me = *self;
        let base = rooms.url.get_untracked();
        let route = bootstrap_route(&room);
        wasm_bindgen_futures::spawn_local(async move {
            let outcome = match send_authority_mutation(&base, AuthorityMethod::Post, route, body)
                .await
            {
                Ok(reply) => match serde_json::from_str::<BootstrapResponse>(&reply.body) {
                    Ok(body) if response_ok(reply.status, body.ok) => body
                        .into_server_projection(&room, &owner, &package)
                        .ok_or(Notice {
                            state: NoticeState::Unavailable,
                            message: "bootstrap response omitted server authority proof".to_owned(),
                        }),
                    Ok(body) => Err(classify_error(reply.status, body.error)),
                    Err(_) => Err(classify_error(reply.status, None)),
                },
                Err(AuthorityFailure::Send) => Err(Notice {
                    state: NoticeState::Unavailable,
                    message: "first-agent bootstrap is unavailable".to_owned(),
                }),
                Err(AuthorityFailure::Build) => Err(Notice {
                    state: NoticeState::Unavailable,
                    message: "bootstrap request could not be built".to_owned(),
                }),
            };
            if !me.current(op, rooms, generation, &room)
                || me.selected_package.get_untracked() != package
            {
                return;
            }
            me.busy.set(false);
            match outcome {
                Ok((record, preview)) => {
                    rooms.open_room.set(Some(record));
                    rooms.fetch_rooms_silent();
                    // The daemon's store inserts a `room_agent_owners` row as
                    // part of creating this agent participant, so the room's
                    // ownership just changed and the rail's copy of it is from
                    // hydration. Without this the agent that was just
                    // bootstrapped renders `unclaimed` until the room is closed
                    // and reopened.
                    rooms.refresh_agent_owners();
                    me.owner_eligible.set(true);
                    me.preview.set(Some(preview));
                    me.notice.set(Some(Notice {
                        state: NoticeState::Requested,
                        message: "Package resolved; no agent binding has been authorized"
                            .to_owned(),
                    }));
                }
                Err(notice) => {
                    me.selected_package.set(String::new());
                    me.notice.set(Some(notice));
                }
            }
        });
    }

    fn register_federated_membership(&self, rooms: Rooms) {
        let Some(room) = rooms.open_key.get_untracked() else {
            return;
        };
        let Some(preview) = self.preview.get_untracked() else {
            return;
        };
        if !preview.owner_eligible {
            self.notice.set(Some(Notice {
                state: NoticeState::Denied,
                message: "the daemon did not confirm room-owner eligibility".to_owned(),
            }));
            return;
        }
        let package = preview.agent_package_id.clone();
        let Some(access) = rooms.access.get_untracked() else {
            return;
        };
        if access.state != RoomAccessState::Live {
            self.notice.set(Some(Notice {
                state: NoticeState::Unavailable,
                message: "federated room membership is not live".to_owned(),
            }));
            return;
        }
        let url = format!(
            "{}/v1/rooms/persistent/{}/members/agents",
            rooms.url.get_untracked(),
            encode(&room)
        );
        let packages = vec![package.clone()];
        let body = serde_json::to_string(&FederatedRegistrationBody {
            agent_names: &packages,
        });
        let body = match body {
            Ok(body) => body,
            Err(_) => return,
        };
        let generation = rooms.generation_snapshot();
        let op = self.next_op();
        self.busy.set(true);
        self.notice.set(Some(Notice {
            state: NoticeState::Requested,
            message: "Registering non-authorizing room membership".to_owned(),
        }));
        let me = *self;
        wasm_bindgen_futures::spawn_local(async move {
            let outcome = match Request::post(&url)
                .header("content-type", "application/json")
                .body(body)
            {
                Ok(request) => match request.send().await {
                    Ok(response) => {
                        let status = response.status();
                        match response.json::<RoomAccessProjection>().await {
                            Ok(access) if (200..300).contains(&status) => Ok(access),
                            _ => Err(classify_error(status, None)),
                        }
                    }
                    Err(_) => Err(Notice {
                        state: NoticeState::Unavailable,
                        message: "membership registration is unavailable".to_owned(),
                    }),
                },
                Err(_) => Err(Notice {
                    state: NoticeState::Unavailable,
                    message: "membership request could not be built".to_owned(),
                }),
            };
            if !me.current(op, rooms, generation, &room) {
                return;
            }
            me.busy.set(false);
            match outcome {
                Ok(access) => rooms.access.set(Some(access)),
                Err(notice) => {
                    me.notice.set(Some(notice));
                    return;
                }
            }
            me.load_preview(rooms, package);
        });
    }

    fn load_preview(&self, rooms: Rooms, package: String) {
        let Some(room) = rooms.open_key.get_untracked() else {
            return;
        };
        let generation = rooms.generation_snapshot();
        let op = self.next_op();
        self.busy.set(true);
        self.notice.set(Some(Notice {
            state: NoticeState::Requested,
            message: "Resolving package authority".to_owned(),
        }));
        let me = *self;
        let url = package_preview_url(&rooms.url.get_untracked(), &room, &package);
        wasm_bindgen_futures::spawn_local(async move {
            let outcome = match Request::get(&url).send().await {
                Ok(response) => {
                    let status = response.status();
                    match response.json::<PreviewResponse>().await {
                        Ok(body) if response_ok(status, body.ok) => match body.into_preview() {
                            Some(preview) if preview.valid_for(&package) => Ok(preview),
                            Some(_) => Err(Notice {
                                state: NoticeState::Unavailable,
                                message: "the daemon returned a mismatched package preview"
                                    .to_owned(),
                            }),
                            None => Err(Notice {
                                state: NoticeState::Unavailable,
                                message: "package preview omitted authority identity".to_owned(),
                            }),
                        },
                        Ok(body) => Err(classify_error(status, body.error)),
                        Err(_) => Err(Notice {
                            state: NoticeState::Unavailable,
                            message: "package preview returned invalid data".to_owned(),
                        }),
                    }
                }
                Err(_) => Err(Notice {
                    state: NoticeState::Unavailable,
                    message: "package preview is unavailable".to_owned(),
                }),
            };
            if !me.current(op, rooms, generation, &room)
                || me.selected_package.get_untracked() != package
            {
                return;
            }
            me.busy.set(false);
            match outcome {
                Ok(preview) => {
                    let reauthorize_member = me.reauthorize_member_id.get_untracked();
                    let ready = preview.allows_decision(reauthorize_member.as_deref());
                    let notice = if ready {
                        Notice {
                            state: NoticeState::Requested,
                            message: "Package resolved; no authority has been granted".to_owned(),
                        }
                    } else if preview
                        .binding
                        .as_ref()
                        .is_some_and(|binding| binding.status == BindingStatus::Revoked)
                    {
                        Notice {
                            state: NoticeState::Unavailable,
                            message: "The resolved member identity is terminally revoked"
                                .to_owned(),
                        }
                    } else if preview.binding.is_some() {
                        Notice {
                            state: NoticeState::Unavailable,
                            message: "This member already has a binding".to_owned(),
                        }
                    } else if preview.owner_eligible {
                        Notice {
                            state: NoticeState::Unavailable,
                            message: "Register display membership before authorization".to_owned(),
                        }
                    } else {
                        Notice {
                            state: NoticeState::Denied,
                            message: "The daemon did not confirm room-owner eligibility".to_owned(),
                        }
                    };
                    me.preview.set(Some(preview));
                    me.notice.set(Some(notice));
                }
                Err(notice) => me.notice.set(Some(notice)),
            }
        });
    }

    fn review(&self) {
        let Some(preview) = self.preview.get_untracked() else {
            return;
        };
        let reauthorize_member = self.reauthorize_member_id.get_untracked();
        if !preview.allows_decision(reauthorize_member.as_deref()) {
            self.notice.set(Some(Notice {
                state: NoticeState::Unavailable,
                message: "The resolved member is not eligible for this authorization decision"
                    .to_owned(),
            }));
            return;
        }
        self.grants
            .set(canonical_grants(&preview, &self.grants.get_untracked()));
        match mint_decision_id() {
            Ok(decision_id) => {
                self.decision_id.set(Some(decision_id));
                self.notice.set(Some(Notice {
                    state: NoticeState::Requested,
                    message: "Review this exact local authorization".to_owned(),
                }));
            }
            Err(message) => self.notice.set(Some(Notice {
                state: NoticeState::Unavailable,
                message: message.to_owned(),
            })),
        }
    }

    fn authorize(&self, rooms: Rooms) {
        let Some(room) = rooms.open_key.get_untracked() else {
            return;
        };
        let Some(preview) = self.preview.get_untracked() else {
            return;
        };
        let reauthorize_member = self.reauthorize_member_id.get_untracked();
        if !preview.allows_decision(reauthorize_member.as_deref()) {
            self.notice.set(Some(Notice {
                state: NoticeState::Unavailable,
                message: "the resolved member is not eligible for this authorization decision"
                    .to_owned(),
            }));
            return;
        }
        let (Some(agent_member_id), Some(owner_member_id)) = (
            preview.agent_member_id.as_deref(),
            preview.owner_member_id.as_deref(),
        ) else {
            self.notice.set(Some(Notice {
                state: NoticeState::Unavailable,
                message: "the daemon has not resolved room membership".to_owned(),
            }));
            return;
        };
        let Some(decision_id) = self.decision_id.get_untracked() else {
            return;
        };
        let generation = rooms.generation_snapshot();
        let op = self.next_op();
        let activation = self.activation_policy.get_untracked();
        let context = self.context_policy.get_untracked();
        let memory = self.memory_scope.get_untracked();
        let grants = canonical_grants(&preview, &self.grants.get_untracked());
        let base = rooms.url.get_untracked();
        let (route, body) = if let Some(member) = reauthorize_member.as_deref() {
            let route = binding_action_route(&room, member, "reauthorize");
            let body = serde_json::to_string(&ReauthorizeBody {
                decision_id: &decision_id,
                activation_policy: &activation,
                context_policy: &context,
                memory_scope: &memory,
                room_capability_grants: &grants,
            });
            (route, body)
        } else {
            let route = bindings_route(&room);
            let body = serde_json::to_string(&AuthorizeBody {
                agent_member_id,
                agent_package_id: &preview.agent_package_id,
                owner_member_id,
                decision_id: &decision_id,
                activation_policy: &activation,
                context_policy: &context,
                memory_scope: &memory,
                room_capability_grants: &grants,
            });
            (route, body)
        };
        let body = match body {
            Ok(body) => body,
            Err(_) => {
                self.notice.set(Some(Notice {
                    state: NoticeState::Unavailable,
                    message: "authorization request could not be encoded".to_owned(),
                }));
                return;
            }
        };
        self.busy.set(true);
        self.notice.set(Some(Notice {
            state: NoticeState::Requested,
            message: "Submitting operator decision".to_owned(),
        }));
        let me = *self;
        wasm_bindgen_futures::spawn_local(async move {
            let outcome = match send_authority_mutation(&base, AuthorityMethod::Post, route, body)
                .await
            {
                Ok(reply) => match serde_json::from_str::<BindingMutationResponse>(&reply.body) {
                    Ok(body) if response_ok(reply.status, body.ok) => body.binding.ok_or(Notice {
                        state: NoticeState::Unavailable,
                        message: "authorization response omitted the binding".to_owned(),
                    }),
                    Ok(body) => Err(classify_error(reply.status, body.error)),
                    Err(_) => Err(Notice {
                        state: NoticeState::Unavailable,
                        message: "authorization response returned invalid data".to_owned(),
                    }),
                },
                Err(AuthorityFailure::Send) => Err(Notice {
                    state: NoticeState::Unavailable,
                    message: "authorization request is unavailable".to_owned(),
                }),
                Err(AuthorityFailure::Build) => Err(Notice {
                    state: NoticeState::Unavailable,
                    message: "authorization request could not be built".to_owned(),
                }),
            };
            if !me.current(op, rooms, generation, &room) {
                return;
            }
            me.busy.set(false);
            match outcome {
                Ok(binding) => {
                    upsert_binding(&me.bindings, binding);
                    me.reset_form();
                    // Same reason as the bootstrap above: an authorization can
                    // establish the agent participant, and the ownership row
                    // lands with it. `upsert_binding` updates this component's
                    // own list and nothing the members rail reads.
                    rooms.refresh_agent_owners();
                    me.notice.set(Some(Notice {
                        state: NoticeState::Active,
                        message: "Room agent authorization is active".to_owned(),
                    }));
                }
                Err(notice) => me.notice.set(Some(notice)),
            }
        });
    }

    fn begin_reauthorize(&self, rooms: Rooms, binding: RoomAgentBinding) {
        self.activation_policy
            .set(binding.activation_policy.clone());
        self.context_policy.set(binding.context_policy.clone());
        self.memory_scope.set(binding.memory_scope.clone());
        self.grants.set(binding.room_capability_grants.clone());
        self.reauthorize_member_id
            .set(Some(binding.agent_member_id.clone()));
        self.previous_digest
            .set(Some(binding.agent_definition_digest.clone()));
        self.decision_id.set(None);
        self.preview.set(None);
        self.selected_package.set(binding.agent_package_id.clone());
        self.load_preview(rooms, binding.agent_package_id);
    }

    fn status_mutation(&self, rooms: Rooms, binding: RoomAgentBinding, action: &'static str) {
        let Some(room) = rooms.open_key.get_untracked() else {
            return;
        };
        let agent_member_id = binding.agent_member_id.clone();
        let existing = self.pending_status_decision.get_untracked();
        let decision_id =
            match pending_decision_id(existing.as_ref(), &room, &agent_member_id, action) {
                Some(decision_id) => decision_id.to_owned(),
                None => match mint_decision_id() {
                    Ok(decision_id) => {
                        self.pending_status_decision
                            .set(Some(PendingStatusDecision {
                                room: room.clone(),
                                agent_member_id: agent_member_id.clone(),
                                action: action.to_owned(),
                                decision_id: decision_id.clone(),
                            }));
                        decision_id
                    }
                    Err(message) => {
                        self.notice.set(Some(Notice {
                            state: NoticeState::Unavailable,
                            message: message.to_owned(),
                        }));
                        return;
                    }
                },
            };
        let generation = rooms.generation_snapshot();
        let op = self.next_op();
        let base = rooms.url.get_untracked();
        // Revoke is the allowlist's only DELETE; the other three are POSTs to
        // the member's action route.
        let (method, route) = if action == "revoke" {
            (
                AuthorityMethod::Delete,
                binding_route(&room, &binding.agent_member_id),
            )
        } else {
            (
                AuthorityMethod::Post,
                binding_action_route(&room, &binding.agent_member_id, action),
            )
        };
        let body = match serde_json::to_string(&StatusBody {
            decision_id: &decision_id,
        }) {
            Ok(body) => body,
            Err(_) => return,
        };
        self.busy.set(true);
        self.notice.set(Some(Notice {
            state: NoticeState::Requested,
            message: format!("{action} requested"),
        }));
        let me = *self;
        wasm_bindgen_futures::spawn_local(async move {
            let outcome = match send_authority_mutation(&base, method, route, body).await {
                Ok(reply) => match serde_json::from_str::<BindingMutationResponse>(&reply.body) {
                    Ok(body) if response_ok(reply.status, body.ok) => body.binding.ok_or(Notice {
                        state: NoticeState::Unavailable,
                        message: "authority response omitted the binding".to_owned(),
                    }),
                    Ok(body) => Err(classify_error(reply.status, body.error)),
                    Err(_) => Err(Notice {
                        state: NoticeState::Unavailable,
                        message: "authority response returned invalid data".to_owned(),
                    }),
                },
                Err(AuthorityFailure::Send) => Err(Notice {
                    state: NoticeState::Unavailable,
                    message: "authority request is unavailable".to_owned(),
                }),
                Err(AuthorityFailure::Build) => Err(Notice {
                    state: NoticeState::Unavailable,
                    message: "authority request could not be built".to_owned(),
                }),
            };
            if !me.current(op, rooms, generation, &room) {
                return;
            }
            me.busy.set(false);
            match outcome {
                Ok(binding) => {
                    if pending_decision_id(
                        me.pending_status_decision.get_untracked().as_ref(),
                        &room,
                        &agent_member_id,
                        action,
                    ) == Some(decision_id.as_str())
                    {
                        me.pending_status_decision.set(None);
                    }
                    let state = if binding.status == BindingStatus::Revoked {
                        NoticeState::Revoked
                    } else {
                        NoticeState::Active
                    };
                    let message = format!("Binding is {}", binding.status.label());
                    upsert_binding(&me.bindings, binding);
                    me.notice.set(Some(Notice { state, message }));
                }
                Err(notice) => me.notice.set(Some(notice)),
            }
        });
    }
}

fn response_ok(status: u16, ok: bool) -> bool {
    (200..300).contains(&status) && ok
}

fn upsert_binding(signal: &RwSignal<Vec<RoomAgentBinding>>, binding: RoomAgentBinding) {
    signal.update(|bindings| {
        if let Some(current) = bindings.iter_mut().find(|current| {
            current.room_id == binding.room_id && current.agent_member_id == binding.agent_member_id
        }) {
            *current = binding;
        } else {
            bindings.push(binding);
        }
    });
}

fn package_is_authorizable(bindings: &[RoomAgentBinding], package: &str) -> bool {
    !bindings.iter().any(|binding| {
        binding.agent_package_id == package
            && matches!(
                binding.status,
                BindingStatus::Active | BindingStatus::Suspended | BindingStatus::Stale
            )
    })
}

fn unavailable_capability_explanation(reason: &str) -> &'static str {
    match reason {
        "phase1_resource_confinement_unavailable" => {
            "Room resource confinement is not available on this node."
        }
        _ => "This capability is unavailable on this node.",
    }
}

#[component]
fn PackageAuthorizationPreview(
    rooms: Rooms,
    state: RoomAgentAuthorizationState,
    preview: AgentPackagePreview,
) -> impl IntoView {
    let digest = preview.agent_definition_digest.clone();
    let display_name = preview.display_name.clone();
    let grantable_capabilities = preview.grantable_capabilities.clone();
    let unavailable_capabilities = preview.unavailable_capabilities.clone();
    let requested_capabilities = preview.requested_capabilities.clone();
    let existing_binding = preview.binding.clone();
    let reauthorize_member = state.reauthorize_member_id.get_untracked();
    let decision_ready = preview.allows_decision(reauthorize_member.as_deref());
    let owner_eligible = preview.owner_eligible;

    let capability_request = if requested_capabilities.is_empty() {
        view! {
            <div class="rooms-workspace__authority-empty">
                "This package requests no capabilities."
            </div>
        }
        .into_any()
    } else {
        view! {
            <div class="rooms-workspace__authority-requested">
                <span>"Requested"</span>
                <code>{requested_capabilities.join(", ")}</code>
            </div>
        }
        .into_any()
    };

    let unavailable_request = if unavailable_capabilities.is_empty() {
        ().into_any()
    } else {
        view! {
            <div class="rooms-workspace__authority-unavailable" data-state="unavailable">
                <span>"Unavailable requests"</span>
                <ul>
                    <For
                        each=move || unavailable_capabilities.clone()
                        key=|item| item.capability.clone()
                        children=move |item: UnavailableCapability| {
                            view! {
                                <li>
                                    <code>{item.capability}</code>
                                    <span>{unavailable_capability_explanation(&item.reason)}</span>
                                </li>
                            }
                        }
                    />
                </ul>
            </div>
        }
        .into_any()
    };

    let ceremony = if decision_ready {
        let grant_controls = if grantable_capabilities.is_empty() {
            view! {
                <div class="rooms-workspace__authority-empty" data-state="unavailable">
                    "No Phase 1 capabilities are grantable on this node."
                </div>
            }
            .into_any()
        } else {
            view! {
                <fieldset class="rooms-workspace__authority-capabilities">
                    <legend>"Grantable capabilities"</legend>
                    <For
                        each=move || grantable_capabilities.clone()
                        key=|capability| capability.clone()
                        children=move |capability: String| {
                            let checked_capability = capability.clone();
                            let changed_capability = capability.clone();
                            view! {
                                <label>
                                    <input
                                        type="checkbox"
                                        prop:checked=move || state.grants.get().contains(&checked_capability)
                                        on:change=move |event| {
                                            let checked = event_target_checked(&event);
                                            state.grants.update(|grants| {
                                                grants.retain(|grant| grant != &changed_capability);
                                                if checked {
                                                    grants.push(changed_capability.clone());
                                                }
                                            });
                                            state.decision_id.set(None);
                                        }
                                    />
                                    <code>{capability}</code>
                                </label>
                            }
                        }
                    />
                </fieldset>
            }
            .into_any()
        };

        view! {
            <div class="rooms-workspace__authority-ready">
                {grant_controls}
                <div class="rooms-workspace__authority-policies">
                    <label>
                        <span>"Activation"</span>
                        <select on:change=move |event| {
                            state.activation_policy.set(event_target_value(&event));
                            state.decision_id.set(None);
                        }>
                            <option value="explicit_only" selected=move || state.activation_policy.get() == "explicit_only">"Explicit only"</option>
                            <option value="mention" selected=move || state.activation_policy.get() == "mention">"Mention"</option>
                            <option value="task_and_thread" selected=move || state.activation_policy.get() == "task_and_thread">"Task and thread"</option>
                        </select>
                    </label>
                    <label>
                        <span>"Context"</span>
                        <select on:change=move |event| {
                            state.context_policy.set(event_target_value(&event));
                            state.decision_id.set(None);
                        }>
                            <option value="invocation_only" selected=move || state.context_policy.get() == "invocation_only">"Invocation only"</option>
                            <option value="room_recent" selected=move || state.context_policy.get() == "room_recent">"Recent room"</option>
                            <option value="room_history" selected=move || state.context_policy.get() == "room_history">"Room history"</option>
                        </select>
                    </label>
                    <label>
                        <span>"Memory"</span>
                        <select on:change=move |event| {
                            state.memory_scope.set(event_target_value(&event));
                            state.decision_id.set(None);
                        }>
                            <option value="none" selected=move || state.memory_scope.get() == "none">"None"</option>
                            <option value="room" selected=move || state.memory_scope.get() == "room">"This room"</option>
                        </select>
                    </label>
                </div>
                {move || match (state.decision_id.get(), state.preview.get()) {
                    (Some(decision_id), Some(preview)) => view! {
                        <div class="rooms-workspace__authority-confirm" data-state="requested">
                            <dl>
                                <div><dt>"Package"</dt><dd>{preview.agent_package_id}</dd></div>
                                <div><dt>"Member"</dt><dd>{preview.agent_member_id.unwrap_or_default()}</dd></div>
                                <div><dt>"Digest"</dt><dd><code>{preview.agent_definition_digest}</code></dd></div>
                                <div><dt>"Activation"</dt><dd>{state.activation_policy.get()}</dd></div>
                                <div><dt>"Context"</dt><dd>{state.context_policy.get()}</dd></div>
                                <div><dt>"Memory"</dt><dd>{state.memory_scope.get()}</dd></div>
                                <div><dt>"Grants"</dt><dd>{state.grants.get().join(", ")}</dd></div>
                                <div><dt>"Decision"</dt><dd><code>{decision_id}</code></dd></div>
                            </dl>
                            <div class="rooms-workspace__authority-actions">
                                <button type="button" on:click=move |_| state.decision_id.set(None)>
                                    "Back"
                                </button>
                                <button type="button" class="rooms-workspace__authority-approve"
                                    disabled=move || state.busy.get()
                                    on:click=move |_| state.authorize(rooms)>
                                    "Authorize"
                                </button>
                            </div>
                        </div>
                    }.into_any(),
                    _ => view! {
                        <button type="button" class="rooms-workspace__authority-review"
                            disabled=move || state.busy.get()
                            on:click=move |_| state.review()>
                            "Review authorization"
                        </button>
                    }.into_any(),
                }}
            </div>
        }
        .into_any()
    } else if let Some(binding) = existing_binding {
        let message = if binding.status == BindingStatus::Revoked {
            "This member identity is revoked and terminal. A distinct server-resolved member is required for any new authorization."
                .to_owned()
        } else {
            format!(
                "This member already has a {} binding. Use its binding controls above.",
                binding.status.label()
            )
        };
        view! {
            <div class="rooms-workspace__authority-membership" data-state="unavailable">
                {message}
            </div>
        }
        .into_any()
    } else if owner_eligible
        && rooms
            .access
            .get_untracked()
            .is_some_and(|access| access.state == RoomAccessState::Live)
    {
        view! {
            <div class="rooms-workspace__authority-membership" data-state="unavailable">
                <span>"Federated membership is required before authority can be granted."</span>
                <button type="button"
                    disabled=move || state.busy.get()
                    on:click=move |_| state.register_federated_membership(rooms)>
                    "Register membership"
                </button>
            </div>
        }
        .into_any()
    } else {
        view! {
            <div class="rooms-workspace__authority-membership" data-state="denied">
                "The daemon did not confirm room-owner eligibility for this package."
            </div>
        }
        .into_any()
    };

    view! {
        <div class="rooms-workspace__authority-preview" data-state="requested">
            <div class="rooms-workspace__authority-preview-name">{display_name}</div>
            <code class="rooms-workspace__authority-digest">{digest.clone()}</code>
            {move || state.previous_digest.get().map(|previous| view! {
                <div class="rooms-workspace__authority-diff">
                    <code>{previous}</code>
                    <span aria-hidden="true">" → "</span>
                    <code>{digest.clone()}</code>
                </div>
            })}
            <div class="rooms-workspace__authority-narrow">
                "Grants can only narrow server-confirmed package requests."
            </div>
            {capability_request}
            {unavailable_request}
            {ceremony}
        </div>
    }
}

#[component]
pub(crate) fn RoomAgentAuthorizationPanel(
    rooms: Rooms,
    state: RoomAgentAuthorizationState,
    agent_builder: crate::agents::AgentBuilderState,
) -> impl IntoView {
    Effect::new(move |_| {
        let generation = rooms.generation_snapshot_reactive();
        if let Some(room) = rooms.open_key.get() {
            state.load_bindings(rooms, room, generation);
        } else {
            state.next_op();
            state.bindings.set(Vec::new());
            state.owner_eligible.set(false);
            state.loaded.set(false);
            state.busy.set(false);
            state.notice.set(None);
            state.reset_form();
        }
    });

    let can_authorize = Memo::new(move |_| caller_can_authorize(rooms, state.owner_eligible.get()));
    let can_configure = Memo::new(move |_| {
        let bindings = state.bindings.get();
        let identity_id = rooms.identity_id.get();
        let bootstrap_candidate = local_owner_bootstrap_candidate(
            rooms.access.get().as_ref(),
            rooms.open_room.get().as_ref(),
            rooms.identity_authoritative.get() && !identity_id.is_empty(),
            &identity_id,
            &bindings,
        );
        caller_can_configure(rooms, state.owner_eligible.get(), bootstrap_candidate)
    });
    let current_preview = move || state.preview.get();
    let authorizable_packages = move || {
        let bindings = state.bindings.get();
        rooms
            .available_agents
            .get()
            .into_iter()
            .filter(|package| package_is_authorizable(&bindings, package))
            .collect::<Vec<_>>()
    };

    view! {
        <section class="rooms-workspace__authority" aria-labelledby="rooms-agent-authority-title">
            <div class="rooms-workspace__authority-head">
                <h4 id="rooms-agent-authority-title" class="rooms-workspace__authority-title">
                    "Agents"
                </h4>
                <span class="rooms-workspace__authority-count">
                    {move || state.bindings.get().len()}
                </span>
            </div>

            {move || {
                if !state.loaded.get() && rooms.open_key.get().is_some() {
                    view! { <div class="rooms-workspace__authority-empty">"Reading authority…"</div> }
                        .into_any()
                } else if state.bindings.get().is_empty() {
                    view! {
                        <div class="rooms-workspace__authority-empty">
                            "No authorized agents in this room."
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="rooms-workspace__authority-list" role="list">
                            <For
                                each=move || state.bindings.get()
                                key=|binding| format!("{}:{}", binding.agent_member_id, binding.generation_label())
                                children=move |binding: RoomAgentBinding| {
                                    let status = binding.status;
                                    let suspend_binding = binding.clone();
                                    let resume_binding = binding.clone();
                                    let stale_binding = binding.clone();
                                    let revoke_binding = binding.clone();
                                    let actions = if can_authorize.get() && binding.owner_eligible {
                                        match status {
                                            BindingStatus::Active => view! {
                                                <div class="rooms-workspace__authority-actions">
                                                    <button type="button" disabled=move || state.busy.get()
                                                        on:click=move |_| state.status_mutation(rooms, suspend_binding.clone(), "suspend")>
                                                        "Suspend"
                                                    </button>
                                                    <button type="button" class="rooms-workspace__authority-danger"
                                                        disabled=move || state.busy.get()
                                                        on:click=move |_| state.status_mutation(rooms, revoke_binding.clone(), "revoke")>
                                                        "Revoke"
                                                    </button>
                                                </div>
                                            }.into_any(),
                                            BindingStatus::Suspended => view! {
                                                <div class="rooms-workspace__authority-actions">
                                                    <button type="button" disabled=move || state.busy.get()
                                                        on:click=move |_| state.status_mutation(rooms, resume_binding.clone(), "resume")>
                                                        "Resume"
                                                    </button>
                                                    <button type="button" class="rooms-workspace__authority-danger"
                                                        disabled=move || state.busy.get()
                                                        on:click=move |_| state.status_mutation(rooms, revoke_binding.clone(), "revoke")>
                                                        "Revoke"
                                                    </button>
                                                </div>
                                            }.into_any(),
                                            BindingStatus::Stale => view! {
                                                <div class="rooms-workspace__authority-actions">
                                                    <button type="button" disabled=move || state.busy.get()
                                                        on:click=move |_| state.begin_reauthorize(rooms, stale_binding.clone())>
                                                        "Review update"
                                                    </button>
                                                    <button type="button" class="rooms-workspace__authority-danger"
                                                        disabled=move || state.busy.get()
                                                        on:click=move |_| state.status_mutation(rooms, revoke_binding.clone(), "revoke")>
                                                        "Revoke"
                                                    </button>
                                                </div>
                                            }.into_any(),
                                            BindingStatus::Revoked | BindingStatus::Unavailable => ().into_any(),
                                        }
                                    } else {
                                        ().into_any()
                                    };
                                    view! {
                                        <article
                                            class="rooms-workspace__authority-binding"
                                            data-state=status.label()
                                            role="listitem"
                                        >
                                            <div class="rooms-workspace__authority-binding-main">
                                                <span class="rooms-workspace__authority-binding-name">
                                                    {binding.display_name.clone()}
                                                </span>
                                                <span class="rooms-workspace__authority-binding-state">
                                                    {status.label()}
                                                </span>
                                            </div>
                                            <code class="rooms-workspace__authority-digest">
                                                {binding.agent_definition_digest.clone()}
                                            </code>
                                            <div class="rooms-workspace__authority-meta">
                                                {format!(
                                                    "{} · {} · memory {} · generation {}",
                                                    binding.activation_policy,
                                                    binding.context_policy,
                                                    binding.memory_scope,
                                                    binding.generation_label(),
                                                )}
                                            </div>
                                            {actions}
                                        </article>
                                    }
                                }
                            />
                        </div>
                    }.into_any()
                }
            }}

            {move || if can_configure.get() {
                view! {
                    <div class="rooms-workspace__authority-ceremony">
                        <label class="rooms-workspace__authority-label" for="room-agent-package">
                            "Authorize package"
                        </label>
                        <select
                            id="room-agent-package"
                            class="rooms-workspace__authority-select"
                            disabled=move || state.busy.get()
                            on:change=move |event| {
                                state.select_package(rooms, event_target_value(&event));
                            }
                        >
                            <option value="" selected=move || state.selected_package.get().is_empty()>
                                "Choose a package"
                            </option>
                            <For
                                each=authorizable_packages
                                key=|package| package.clone()
                                children=move |package: String| {
                                    let value = package.clone();
                                    view! { <option value=value>{package}</option> }
                                }
                            />
                        </select>

                        {move || current_preview().map(|preview| view! {
                            <PackageAuthorizationPreview rooms state preview />
                        })}

                        <crate::agents::AgentBuilder
                            state=agent_builder
                            agents=rooms.available_agents
                            on_saved=Callback::new(move |_name: String| rooms.fetch_agents())
                            on_deleted=Callback::new(move |_name: String| rooms.fetch_agents())
                        />
                    </div>
                }.into_any()
            } else if rooms.open_key.get().is_some() {
                view! {
                    <div class="rooms-workspace__authority-readonly" data-state="unavailable">
                        {if authority_mutations_supported_on_this_host() {
                            "Only the room owner can authorize local agents."
                        } else {
                            "Room authorization is read-only in this host. Use the desktop app or the authenticated Surface proxy to change authority."
                        }}
                    </div>
                }.into_any()
            } else {
                ().into_any()
            }}

            {move || state.notice.get().map(|notice| view! {
                <div
                    class="rooms-workspace__authority-notice"
                    data-state=notice.state.label()
                    role=if matches!(notice.state, NoticeState::Denied | NoticeState::Unavailable) {
                        "alert"
                    } else {
                        "status"
                    }
                >
                    <span>{notice.state.label()}</span>
                    {notice.message}
                </div>
            })}
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview() -> AgentPackagePreview {
        AgentPackagePreview {
            agent_package_id: "builder".into(),
            agent_member_id: Some("member-builder".into()),
            owner_member_id: Some("owner".into()),
            display_name: "Builder".into(),
            agent_definition_digest: format!("sha256:{}", "a".repeat(64)),
            requested_capabilities: vec!["read".into(), "net.fetch".into()],
            grantable_capabilities: vec!["read".into()],
            unavailable_capabilities: vec![UnavailableCapability {
                capability: "net.fetch".into(),
                reason: "phase1_resource_confinement_unavailable".into(),
            }],
            binding: None,
            owner_eligible: true,
        }
    }

    #[test]
    fn preview_route_is_isolated_and_encodes_both_identities() {
        assert_eq!(
            package_preview_url("http://d", "team/blue", "review agent"),
            "http://d/v1/rooms/persistent/team%2Fblue/agents/preview/review%20agent"
        );
        assert_eq!(
            bootstrap_route("team/blue").path(),
            "/v1/rooms/persistent/team%2Fblue/agents/bootstrap"
        );
    }

    /// Every privileged route is a PATH, encodes each untrusted identity into
    /// exactly one segment, and lands on the same six shapes both the Surface
    /// proxy and the Tauri shell allowlist. A route that grew an origin, or an
    /// identity that split into two segments, would be a credential pointed
    /// somewhere nobody chose.
    #[test]
    fn every_privileged_route_is_an_encoded_single_segment_path() {
        for route in [
            bindings_route("team/blue"),
            bootstrap_route("team/blue"),
            binding_action_route("team/blue", "agent/one", "suspend"),
            binding_route("team/blue", "agent/one"),
        ] {
            let path = route.path();
            assert!(
                path.starts_with("/v1/rooms/persistent/team%2Fblue/agents"),
                "{path} must be an absolute daemon path under the room's agents route",
            );
            assert!(
                !path.contains("://") && !path.contains("team/blue"),
                "{path} must carry no origin and no unencoded room key",
            );
        }
        assert_eq!(
            bindings_route("team/blue").path(),
            "/v1/rooms/persistent/team%2Fblue/agents"
        );
        assert_eq!(
            binding_action_route("team/blue", "agent/one", "reauthorize").path(),
            "/v1/rooms/persistent/team%2Fblue/agents/agent%2Fone/reauthorize"
        );
        assert_eq!(
            binding_route("team/blue", "agent/one").path(),
            "/v1/rooms/persistent/team%2Fblue/agents/agent%2Fone"
        );
    }

    /// The browser transport joins the route onto the daemon base, so the
    /// URL the PWA sends is byte-for-byte the one it sent before the seam.
    #[test]
    fn the_browser_transport_joins_the_route_onto_the_daemon_base() {
        assert_eq!(
            format!("http://d{}", bindings_route("team/blue").path()),
            bindings_url("http://d", "team/blue"),
        );
    }

    #[test]
    fn the_allowlist_verbs_are_the_two_the_daemon_admits() {
        assert_eq!(AuthorityMethod::Post.as_str(), "POST");
        assert_eq!(AuthorityMethod::Delete.as_str(), "DELETE");
    }

    #[test]
    fn first_agent_bootstrap_requires_the_complete_server_projection() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let response = serde_json::json!({
            "ok": true,
            "created": true,
            "room_id": "team",
            "owner_member_id": "human-1",
            "agent_member_id": "researcher",
            "agent_package_id": "researcher",
            "owner_eligible": true,
            "room": {
                "id": "team",
                "name": "Team",
                "participants": [
                    {"id": "human-1", "kind": "human", "display_name": "Human"},
                    {"id": "researcher", "kind": "agent", "display_name": "Researcher"}
                ]
            },
            "package_preview": {
                "ok": true,
                "package_id": "researcher",
                "display_name": "Researcher",
                "definition_digest": digest,
                "requested_capabilities": [],
                "grantable_capabilities": [],
                "unavailable_capabilities": [],
                "binding": null,
                "agent_member_id": "researcher",
                "owner_member_id": "human-1",
                "owner_eligible": true
            }
        });
        let valid: BootstrapResponse = serde_json::from_value(response.clone()).unwrap();
        let (room, preview) = valid
            .into_server_projection("team", "human-1", "researcher")
            .expect("server projection");
        assert_eq!(room.participants.len(), 2);
        assert!(preview.allows_decision(None));

        let mut replay = response.clone();
        replay["created"] = serde_json::json!(false);
        let replay: BootstrapResponse = serde_json::from_value(replay).unwrap();
        assert!(replay
            .into_server_projection("team", "human-1", "researcher")
            .is_some());

        let mut forged = response;
        forged["owner_member_id"] = serde_json::json!("different-human");
        let forged: BootstrapResponse = serde_json::from_value(forged).unwrap();
        assert!(forged
            .into_server_projection("team", "human-1", "researcher")
            .is_none());
    }

    #[test]
    fn bootstrap_affordance_requires_a_live_local_human_and_no_binding() {
        let access = RoomAccessProjection {
            state: RoomAccessState::Local,
            last_confirmed_global_sequence: None,
            members: Vec::new(),
            self_member_id: None,
            outbox: Vec::new(),
        };
        let room = Room {
            id: "team".into(),
            name: "Team".into(),
            participants: vec![crate::rooms::RoomParticipant {
                id: "human-1".into(),
                kind: RoomParticipantKind::Human,
                display_name: "Human".into(),
            }],
            created_at: String::new(),
            updated_at: String::new(),
            trigger_policy: None,
            workspace_root: None,
        };
        assert!(local_owner_bootstrap_candidate(
            Some(&access),
            Some(&room),
            true,
            "human-1",
            &[],
        ));
        assert!(!local_owner_bootstrap_candidate(
            Some(&access),
            Some(&room),
            true,
            "different-human",
            &[],
        ));
        assert!(!local_owner_bootstrap_candidate(
            Some(&access),
            Some(&room),
            false,
            "human-1",
            &[],
        ));
    }

    #[test]
    fn preview_identity_is_fail_closed() {
        let valid = preview();
        assert!(valid.valid_for("builder"));

        let mut mismatched = valid.clone();
        mismatched.agent_package_id = "other".into();
        assert!(!mismatched.valid_for("builder"));

        let mut malformed_digest = valid.clone();
        malformed_digest.agent_definition_digest = "sha256:not-a-digest".into();
        assert!(!malformed_digest.valid_for("builder"));

        let mut no_member = valid.clone();
        no_member.agent_member_id = None;
        assert!(no_member.valid_for("builder"));
        assert!(!no_member.authorization_ready());

        let mut ineligible = valid;
        ineligible.owner_eligible = false;
        assert!(ineligible.valid_for("builder"));
        assert!(!ineligible.authorization_ready());
    }

    #[test]
    fn grants_are_always_an_intersection_and_start_empty() {
        let preview = preview();
        assert_eq!(canonical_grants(&preview, &[]), Vec::<String>::new());
        assert_eq!(
            canonical_grants(
                &preview,
                &["unrequested".into(), "read".into(), "read".into()]
            ),
            vec!["read"]
        );
        assert_eq!(
            canonical_grants(&preview, &["net.fetch".into()]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn uuid_formatter_sets_v4_and_rfc4122_variant_bits() {
        let uuid = uuid_v4_from_bytes([0; 16]);
        assert_eq!(uuid, "00000000-0000-4000-8000-000000000000");
    }

    #[test]
    fn inactive_bindings_are_never_presented_as_available() {
        let binding = |status| RoomAgentBinding {
            room_id: "room".into(),
            agent_member_id: "member".into(),
            agent_package_id: "builder".into(),
            agent_definition_digest: "sha256:a".into(),
            agent_definition_revision: None,
            display_name: "Builder".into(),
            owner_member_id: "owner".into(),
            activation_policy: "explicit_only".into(),
            context_policy: "invocation_only".into(),
            memory_scope: "none".into(),
            requested_capabilities: vec![],
            room_capability_grants: vec![],
            status,
            owner_eligible: true,
            generation: serde_json::json!(1),
        };
        for status in [
            BindingStatus::Active,
            BindingStatus::Suspended,
            BindingStatus::Stale,
        ] {
            assert!(!package_is_authorizable(&[binding(status)], "builder"));
        }
        assert!(package_is_authorizable(
            &[binding(BindingStatus::Revoked)],
            "builder"
        ));

        let mut terminal = preview();
        terminal.binding = Some(binding(BindingStatus::Revoked));
        assert!(!terminal.allows_decision(None));
    }

    #[test]
    fn ui_error_states_keep_denial_separate_from_unavailability() {
        assert_eq!(
            classify_error(403, Some("room_owner_required".into())).state,
            NoticeState::Denied
        );
        assert_eq!(classify_error(503, None).state, NoticeState::Unavailable);
    }

    #[test]
    fn lost_status_response_reuses_the_exact_decision_id() {
        let pending = PendingStatusDecision {
            room: "room-a".into(),
            agent_member_id: "agent-1".into(),
            action: "suspend".into(),
            decision_id: "018f0000-0000-4000-8000-000000000001".into(),
        };
        assert_eq!(
            pending_decision_id(Some(&pending), "room-a", "agent-1", "suspend"),
            Some("018f0000-0000-4000-8000-000000000001")
        );
        assert_eq!(
            pending_decision_id(Some(&pending), "room-a", "agent-1", "resume"),
            None
        );
        assert_eq!(
            pending_decision_id(Some(&pending), "room-b", "agent-1", "suspend"),
            None
        );
    }
}
