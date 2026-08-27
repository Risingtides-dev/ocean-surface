//! Agent builder — author a daemon agent without leaving a room.
//!
//! Folder-as-agent (`ocean-agent::agentdir`) is a folder on disk: `agent.toml`
//! (model, description, tools, capabilities, yolo) plus a required
//! `instructions.md` system prompt. The daemon exposes it over
//!
//!   GET  /v1/agents          → the roster the room's `+ agent` picker reads
//!   POST /v1/agents          → create one
//!
//! …but until now the surface only ever performed the GET, so the only way to
//! AUTHOR an agent was to hand-write the folder or curl the JSON. This module
//! is the write half: the small form that turns "I need a new agent in this
//! room right now" into one round-trip, mounted directly under the roster's
//! `+ agent` strip in `rooms_workspace.rs`.
//!
//! It lives beside `rooms.rs` rather than inside it for the same reason
//! `rooms.rs` carries its own request layer: this is a distinct daemon
//! resource with its own failure modes, and `rooms_workspace.rs` is already
//! ~4.9k lines.
//!
//! Everything the form decides *before* dispatching (name legality, tool
//! splitting, which models to offer, how a refusal should read) is a free
//! function below, so it is unit-testable natively without a browser or a
//! daemon. The daemon remains the authority on every one of those rules — the
//! client-side copies exist so the operator sees the rule *before* a
//! round-trip, never instead of one.

use gloo_net::http::Request;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen_futures::spawn_local;

use crate::daemon::ModelInfo;

/// What the daemon says when `instructions` is empty
/// (`agentdir::WriteError::MissingInstructions`). Mirrored verbatim so the
/// pre-flight message and the server's refusal read identically — an operator
/// should never have to learn two vocabularies for one rule.
const MISSING_INSTRUCTIONS: &str = "instructions are required; an agent needs a system prompt";

/// Pre-flight wording for an illegal folder name, mirroring
/// `agentdir::WriteError::InvalidName`'s guidance half.
const INVALID_NAME: &str = "use 1-64 chars of a-z, 0-9, '-' or '_' (no leading '.' or '-')";

/// Shown when the write verb never reached a daemon that implements it. The
/// agent write API landed after the read API, so a surface can easily be
/// newer than the daemon it is pointed at (and on web the proxy allowlist is
/// a second place the route can be missing). Without this the operator sees a
/// JSON decode error from an empty 404 body and has no idea what to fix.
const NO_WRITE_API: &str = "this daemon has no agent write API — update ocean-os";

// ---- Wire types (mirror the daemon's AgentWriteBody) ----

/// Create body. FLAT on the wire: the daemon's `AgentWriteBody` carries `name`
/// beside a `#[serde(flatten)]` `AgentSpec`, so every field sits at the top
/// level rather than nested under a `spec` key.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentWriteBody {
    /// Only meaningful on create; `PUT /v1/agents/{name}` takes it from the path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `None` = inherit the daemon default, which is what an empty selection in
    /// the model picker means. Sending `Some("")` would pin the agent to a
    /// model id that does not exist.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub tools: Vec<String>,
    pub capabilities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yolo: Option<bool>,
    pub instructions: String,
}

/// `{ ok, agent?, error? }` — the shape the write verbs answer with. Only the
/// two fields the form branches on are decoded; the returned `agent` is the
/// daemon's business, and the picker re-reads it from `GET /v1/agents`.
#[derive(Debug, Clone, Deserialize)]
struct AgentWriteResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    error: Option<String>,
}

// ---- Pure helpers (unit-tested; no signals, no network) ----

/// Client-side mirror of `agentdir::valid_agent_name`.
///
/// The daemon stays the authority — this exists only so a bad name surfaces as
/// an inline hint the moment the operator hits Create, instead of after a
/// round-trip that returns 400. Kept deliberately identical (including the
/// leading-`.`/`-` refusals, which stop an agent being written as a hidden
/// folder that `discover` would skip) so the two never disagree.
pub fn agent_name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Split the free-text tools field into daemon tool names.
///
/// Free text rather than a picker on purpose: there is no tool-name catalogue
/// to drive one — the daemon exposes no `/v1/tools`, and hardcoding a list in
/// the surface would rot the moment a tool is added. Accepts commas and/or
/// whitespace so "bash, web_fetch" and "bash web_fetch" both work, drops
/// empties, and de-dups while preserving the order the operator typed (the
/// list is an allowlist, so a duplicate is noise, not meaning).
pub fn parse_tools(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in raw.split([',', ' ', '\t', '\n', '\r']) {
        let token = token.trim();
        if token.is_empty() || out.iter().any(|seen| seen == token) {
            continue;
        }
        out.push(token.to_string());
    }
    out
}

/// Turn a failed write into a sentence the operator can act on.
///
/// The daemon's own error text is always preferred — it already says the
/// useful thing ("agent \"x\" already exists", "instructions are required…").
/// The fallbacks matter because the two most likely failures produce NO body
/// at all: a daemon predating the write API answers 405 with axum's plain-text
/// method-not-allowed, and a proxy missing the route falls through to ServeDir
/// with an empty 404. Both used to reach the UI as "EOF while parsing a value".
pub fn write_error_message(http_status: u16, daemon_error: Option<&str>) -> String {
    if let Some(err) = daemon_error.map(str::trim).filter(|e| !e.is_empty()) {
        return err.to_string();
    }
    match http_status {
        404 | 405 => NO_WRITE_API.to_string(),
        status => format!("HTTP {status}"),
    }
}

/// `(value, label)` pairs for the model `<select>`.
///
/// Two invariants, both about not silently changing an agent the operator did
/// not touch:
///
///  * The empty value is ALWAYS first and means "inherit daemon default" —
///    that is what `AgentConfig::model = None` is, and it must be selectable.
///  * `current` is ALWAYS present, even when the catalogue does not list it or
///    has not loaded yet. `/v1/models` resolves asynchronously at bootstrap, so
///    a form opened early would otherwise offer no option matching a pinned
///    model and quietly rewrite it to "inherit" on save.
pub fn model_options(catalogue: &[ModelInfo], current: &str) -> Vec<(String, String)> {
    let mut out = vec![(String::new(), "inherit daemon default".to_string())];
    for model in catalogue {
        if model.id.is_empty() || out.iter().any(|(value, _)| value == &model.id) {
            continue;
        }
        let label = if model.label.trim().is_empty() {
            model.id.clone()
        } else {
            model.label.clone()
        };
        out.push((model.id.clone(), label));
    }
    let current = current.trim();
    if !current.is_empty() && !out.iter().any(|(value, _)| value == current) {
        out.push((current.to_string(), current.to_string()));
    }
    out
}

/// Why a create attempt was refused before it left the browser, if it was.
///
/// Both rules are the daemon's (`agentdir::write` checks the same two, in the
/// same order); we check them here purely to save a round-trip.
pub fn create_blocked_reason(name: &str, instructions: &str) -> Option<&'static str> {
    if !agent_name_is_valid(name) {
        return Some(INVALID_NAME);
    }
    if instructions.trim().is_empty() {
        return Some(MISSING_INSTRUCTIONS);
    }
    None
}

/// Build the create body from raw form text.
///
/// Empty description/model collapse to `None` rather than `Some("")`: the
/// daemon writes `agent.toml` from this spec, and an empty string would be
/// persisted as a real (meaningless) value instead of the absent key that
/// means "inherit".
pub fn create_body(
    name: &str,
    description: &str,
    model: &str,
    tools_raw: &str,
    instructions: &str,
) -> AgentWriteBody {
    AgentWriteBody {
        name: Some(name.trim().to_string()),
        description: non_empty(description),
        model: non_empty(model),
        tools: parse_tools(tools_raw),
        capabilities: Vec::new(),
        yolo: None,
        instructions: instructions.to_string(),
    }
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

// ---- Reactive state ----

/// Signal handles for the agent-builder form.
///
/// Constructed ONCE in `RoomsWorkspace` and captured by the members-rail
/// closure, never created inside it. The members rail re-renders whenever the
/// room access projection changes (every SSE roster update), so form state
/// owned by that closure would be wiped mid-sentence — losing a half-written
/// system prompt to an unrelated participant joining.
///
/// All fields are `Copy` signal handles, like [`crate::rooms::Rooms`] and
/// [`crate::daemon::Daemon`], so the whole struct is passed by value.
#[derive(Clone, Copy)]
pub struct AgentBuilderState {
    /// Daemon base URL, shared with `Daemon::url` via `Rooms::url` — read live
    /// at request time because bootstrap resolves the origin asynchronously.
    pub url: RwSignal<String>,
    /// The `/v1/models` catalogue, shared with `Daemon::models`. Reused rather
    /// than re-fetched: bootstrap already populates it, and the model list is
    /// the daemon's to publish, never the surface's to hardcode.
    pub models: RwSignal<Vec<ModelInfo>>,
    /// Whether the form is disclosed.
    pub open: RwSignal<bool>,
    pub name: RwSignal<String>,
    pub description: RwSignal<String>,
    /// Empty = inherit the daemon default.
    pub model: RwSignal<String>,
    /// Raw comma/space separated tool names; parsed by [`parse_tools`] on save.
    pub tools: RwSignal<String>,
    /// The `instructions.md` system prompt. The one required slot.
    pub instructions: RwSignal<String>,
    /// A write is in flight — drives the button label and blocks re-submit.
    pub pending: RwSignal<bool>,
    /// Inline form error. The form stays open with its input intact so the
    /// operator can fix and retry, following `Daemon::create_project`.
    pub error: RwSignal<Option<String>>,
}

impl AgentBuilderState {
    /// Share the rooms handle's live `url` and `models` signals, so the builder
    /// targets exactly the origin and catalogue the rest of the surface uses.
    pub fn new(rooms: &crate::rooms::Rooms) -> Self {
        Self {
            url: rooms.url,
            models: rooms.models,
            open: RwSignal::new(false),
            name: RwSignal::new(String::new()),
            description: RwSignal::new(String::new()),
            model: RwSignal::new(String::new()),
            tools: RwSignal::new(String::new()),
            instructions: RwSignal::new(String::new()),
            pending: RwSignal::new(false),
            error: RwSignal::new(None),
        }
    }

    fn base(&self) -> String {
        self.url.get_untracked().trim_end_matches('/').to_string()
    }

    /// Clear the form back to a blank create. Called only after a confirmed
    /// success — a failed write must keep the operator's typing.
    fn reset_fields(&self) {
        self.name.set(String::new());
        self.description.set(String::new());
        self.model.set(String::new());
        self.tools.set(String::new());
        self.instructions.set(String::new());
        self.error.set(None);
    }

    /// `POST /v1/agents` — create the agent, then hand its name to `on_saved`
    /// so the caller can refresh the picker one line above.
    ///
    /// Create never overwrites: the daemon answers 409 if the name is taken,
    /// which lands inline rather than silently clobbering an agent the
    /// operator forgot about.
    pub fn create(&self, on_saved: Callback<String>) {
        if self.pending.get_untracked() {
            return;
        }
        let name = self.name.get_untracked().trim().to_string();
        let instructions = self.instructions.get_untracked();
        if let Some(reason) = create_blocked_reason(&name, &instructions) {
            self.error.set(Some(reason.to_string()));
            return;
        }
        let body = create_body(
            &name,
            &self.description.get_untracked(),
            &self.model.get_untracked(),
            &self.tools.get_untracked(),
            &instructions,
        );
        self.error.set(None);
        self.pending.set(true);

        let base = self.base();
        let me = *self;
        spawn_local(async move {
            let post_url = format!("{base}/v1/agents");
            let request = match Request::post(&post_url)
                .header("content-type", "application/json")
                .json(&body)
            {
                Ok(request) => request,
                Err(err) => {
                    log::error!("agent create encode error: {err}");
                    me.error
                        .set(Some(format!("could not encode request: {err}")));
                    me.pending.set(false);
                    return;
                }
            };
            match request.send().await {
                Ok(resp) => {
                    // Read the body as TEXT, not `json()`. A missing route
                    // answers with an empty body, and `json()` would surface
                    // that as "EOF while parsing a value" instead of the
                    // actionable status-derived message.
                    let status = resp.status();
                    let raw = resp.text().await.unwrap_or_default();
                    let decoded = serde_json::from_str::<AgentWriteResponse>(&raw).ok();
                    if decoded.as_ref().is_some_and(|r| r.ok) {
                        me.pending.set(false);
                        me.reset_fields();
                        me.open.set(false);
                        on_saved.run(name);
                    } else {
                        let daemon_error = decoded.and_then(|r| r.error);
                        let message = write_error_message(status, daemon_error.as_deref());
                        log::error!("agent create rejected ({status}): {message}");
                        me.error.set(Some(message));
                        me.pending.set(false);
                    }
                }
                Err(err) => {
                    log::error!("agent create post error: {err}");
                    me.error
                        .set(Some(format!("could not reach the daemon: {err}")));
                    me.pending.set(false);
                }
            }
        });
    }
}

// ---- Component ----

/// The create-an-agent form, mounted under the roster's `+ agent` picker.
///
/// Deliberately does NOT auto-join the new agent to the open room: `on_saved`
/// refreshes the picker directly above, so the operator's next action is the
/// one they already know. Putting the agent in the room stays the picker's job.
#[component]
pub fn AgentBuilder(state: AgentBuilderState, on_saved: Callback<String>) -> impl IntoView {
    let submit = move || state.create(on_saved);

    view! {
        <button
            class="rooms-workspace__agentbuilder-toggle"
            type="button"
            title="Author a new agent on the daemon"
            aria-controls="rooms-workspace-agent-builder"
            aria-expanded=move || state.open.get().to_string()
            on:click=move |_| state.open.update(|open: &mut bool| *open = !*open)
        >
            "+ new agent"
        </button>

        <Show when=move || state.open.get()>
            <div id="rooms-workspace-agent-builder" class="rooms-workspace__agentbuilder">
                <input
                    class="rooms-workspace__agentbuilder-input"
                    type="text"
                    aria-label="Agent name"
                    placeholder="name — a-z, 0-9, - or _"
                    prop:value=move || state.name.get()
                    on:input=move |ev| state.name.set(event_target_value(&ev))
                    disabled=move || state.pending.get()
                />
                <input
                    class="rooms-workspace__agentbuilder-input"
                    type="text"
                    aria-label="Agent description"
                    placeholder="one-line description (optional)"
                    prop:value=move || state.description.get()
                    on:input=move |ev| state.description.set(event_target_value(&ev))
                    disabled=move || state.pending.get()
                />
                // Uncontrolled `<select>`, like the agent picker above it:
                // Leptos does not re-assert `value` on re-render, so the
                // chosen option is marked per-option from the signal.
                <select
                    class="rooms-workspace__agentbuilder-select"
                    aria-label="Agent model"
                    on:change=move |ev| state.model.set(event_target_value(&ev))
                    disabled=move || state.pending.get()
                >
                    <For
                        each=move || model_options(&state.models.get(), &state.model.get())
                        key=|option: &(String, String)| option.0.clone()
                        children=move |(value, label): (String, String)| {
                            let selected = state.model.get_untracked() == value;
                            view! { <option value=value selected=selected>{label}</option> }
                        }
                    />
                </select>
                <input
                    class="rooms-workspace__agentbuilder-input"
                    type="text"
                    aria-label="Agent tools"
                    // Free text, not a picker: the daemon publishes no tool
                    // catalogue to populate one from (see the module docs).
                    placeholder="tools, comma separated (optional)"
                    prop:value=move || state.tools.get()
                    on:input=move |ev| state.tools.set(event_target_value(&ev))
                    disabled=move || state.pending.get()
                />
                <textarea
                    class="rooms-workspace__agentbuilder-prompt"
                    aria-label="Agent instructions"
                    rows="6"
                    placeholder="instructions.md — the system prompt this agent runs with"
                    prop:value=move || state.instructions.get()
                    on:input=move |ev| state.instructions.set(event_target_value(&ev))
                    disabled=move || state.pending.get()
                ></textarea>

                <Show when=move || state.error.get().is_some()>
                    <div class="rooms-workspace__agentbuilder-error" role="alert">
                        {move || state.error.get().unwrap_or_default()}
                    </div>
                </Show>

                <button
                    class="rooms-workspace__agentbuilder-save"
                    type="button"
                    aria-busy=move || state.pending.get().to_string()
                    disabled=move || state.pending.get()
                    on:click=move |_| submit()
                >
                    {move || if state.pending.get() { "Saving…" } else { "Create agent" }}
                </button>
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, label: &str) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            provider: "test".to_string(),
            label: label.to_string(),
        }
    }

    #[test]
    fn agent_name_mirrors_the_daemons_charset_rule() {
        assert!(agent_name_is_valid("researcher"));
        assert!(agent_name_is_valid("code-review_2"));
        assert!(agent_name_is_valid("a"));
        assert!(agent_name_is_valid(&"a".repeat(64)));

        assert!(!agent_name_is_valid(""));
        assert!(!agent_name_is_valid(&"a".repeat(65)));
        assert!(!agent_name_is_valid("-lead"), "no leading dash");
        assert!(!agent_name_is_valid(".hidden"), "no hidden folders");
        assert!(!agent_name_is_valid("Flux"), "lowercase only");
        assert!(!agent_name_is_valid("a/b"), "no path separators");
        assert!(!agent_name_is_valid(".."), "no traversal");
        assert!(!agent_name_is_valid("web fetch"), "no spaces");
    }

    #[test]
    fn parse_tools_splits_trims_and_dedupes() {
        assert_eq!(
            parse_tools("bash, web_fetch,,  edit "),
            vec!["bash", "web_fetch", "edit"]
        );
        // Whitespace alone is a valid separator too.
        assert_eq!(parse_tools("bash web_fetch"), vec!["bash", "web_fetch"]);
        // A duplicate is noise in an allowlist; first mention keeps its place.
        assert_eq!(parse_tools("bash, edit, bash"), vec!["bash", "edit"]);
        assert!(parse_tools("").is_empty());
        assert!(parse_tools("  ,  , ").is_empty());
    }

    #[test]
    fn write_errors_explain_a_missing_route_instead_of_a_decode_failure() {
        // A daemon predating the write API: 405, plain text, nothing to decode.
        assert_eq!(write_error_message(405, None), NO_WRITE_API);
        // A proxy missing the route: empty 404 body from ServeDir.
        assert_eq!(write_error_message(404, None), NO_WRITE_API);
        assert_eq!(write_error_message(404, Some("   ")), NO_WRITE_API);
        // The daemon's own text always wins when it sent one.
        assert_eq!(
            write_error_message(409, Some("agent \"flux\" already exists")),
            "agent \"flux\" already exists"
        );
        assert_eq!(
            write_error_message(404, Some("no agent named \"flux\"")),
            "no agent named \"flux\""
        );
        assert_eq!(write_error_message(500, None), "HTTP 500");
    }

    #[test]
    fn model_options_always_offer_inherit_and_never_drop_the_current_model() {
        let catalogue = vec![model("claude-opus-5", "Opus 5"), model("grok-4", "")];

        let options = model_options(&catalogue, "");
        assert_eq!(options[0], (String::new(), "inherit daemon default".into()));
        assert_eq!(options[1], ("claude-opus-5".into(), "Opus 5".into()));
        // A model with no label falls back to its id rather than rendering blank.
        assert_eq!(options[2], ("grok-4".into(), "grok-4".into()));

        // R8: /v1/models resolves asynchronously. A model pinned in agent.toml
        // but absent from a not-yet-loaded catalogue must still be offered, or
        // saving the form silently rewrites it to "inherit".
        let options = model_options(&[], "claude-opus-5");
        assert_eq!(options.len(), 2);
        assert!(options.iter().any(|(v, _)| v == "claude-opus-5"));

        // Present in the catalogue → listed once, not twice.
        let options = model_options(&catalogue, "grok-4");
        assert_eq!(
            options.iter().filter(|(v, _)| v == "grok-4").count(),
            1,
            "the current model must not be duplicated"
        );
    }

    #[test]
    fn create_is_blocked_on_exactly_the_daemons_two_write_rules() {
        assert_eq!(create_blocked_reason("researcher", "be useful"), None);
        assert_eq!(
            create_blocked_reason("Flux", "be useful"),
            Some(INVALID_NAME)
        );
        // Name is checked first, matching agentdir::write's order.
        assert_eq!(create_blocked_reason("Flux", ""), Some(INVALID_NAME));
        assert_eq!(
            create_blocked_reason("researcher", "   \n "),
            Some(MISSING_INSTRUCTIONS)
        );
    }

    #[test]
    fn create_body_sends_absent_keys_rather_than_empty_strings() {
        let body = create_body("  researcher  ", "  ", "", "bash, edit", "be useful");
        assert_eq!(body.name.as_deref(), Some("researcher"));
        // Empty description/model must be absent so agent.toml inherits
        // instead of persisting a meaningless "".
        assert_eq!(body.description, None);
        assert_eq!(body.model, None);
        assert_eq!(body.tools, vec!["bash", "edit"]);
        assert_eq!(body.instructions, "be useful");

        let json = serde_json::to_value(&body).expect("body serializes");
        assert!(json.get("description").is_none());
        assert!(json.get("model").is_none());
        assert!(json.get("yolo").is_none());
        // FLAT on the wire — the daemon flattens AgentSpec beside `name`.
        assert_eq!(
            json.get("name").and_then(|v| v.as_str()),
            Some("researcher")
        );
        assert!(json.get("spec").is_none());
    }

    #[test]
    fn create_body_keeps_a_chosen_description_and_model() {
        let body = create_body("researcher", "reads the web", "claude-opus-5", "", "hi");
        assert_eq!(body.description.as_deref(), Some("reads the web"));
        assert_eq!(body.model.as_deref(), Some("claude-opus-5"));
        assert!(body.tools.is_empty());
    }
}
