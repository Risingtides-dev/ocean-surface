//! Devices — the machines one login can attach to.
//!
//! The product this serves: sign in once at the public surface and reach
//! whichever of your OWN machines you mean. The proxy owns the roster and the
//! selection; this module is the surface half — it asks which machines exist,
//! shows them with their health, and posts the one you picked.
//!
//! Three rules shape everything here.
//!
//! 1. **A machine is a NAME.** `GET /api/devices` publishes names, health and
//!    which one you are on — never a `daemon_url`. So nothing in this file
//!    accepts, stores, or renders an address, and no URL or credential is ever
//!    typed in the browser. The proxy routes by the selection it recorded
//!    server-side; the surface never re-points its own traffic.
//! 2. **The picker is offered once, by the SERVER's account of the facts.**
//!    `selection_explicit` is false until somebody actually picks, so a fresh
//!    login with two machines is asked, and a browser reopened tomorrow is not.
//!    No client-side "have I asked yet" flag, which would be wrong in the
//!    second browser and lost by a cleared cache.
//! 3. **A switch re-attaches everything, and a session that is not on the new
//!    machine is cleared rather than errored.** A session id belongs to the
//!    daemon that minted it; carrying one to another machine and reporting its
//!    absence as a failure would make switching feel broken every time.
//!
//! Off-proxy hosts (the Chrome side panel, Tauri) have no `/api/devices` to
//! ask, so the load fails quietly and the chrome stays absent — absence, not
//! errors, per the platform contract.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use wasm_bindgen::JsCast;

use crate::daemon::Daemon;
use crate::rooms::Rooms;

/// Everything a device switch has to re-attach.
///
/// The daemon owns the transcript, the streams and the catalogues; `Rooms`
/// owns the open room, which is a SEPARATE state with its own generation and
/// its own tail. A G1 room is daemon-native and local to the machine that
/// holds it, so after a switch the open room is either gone or — worse — a
/// different room that happens to share the key, and its transcript, roster,
/// access projection and drafts all describe the machine we left.
#[derive(Clone)]
pub struct Attachments {
    pub daemon: Daemon,
    pub rooms: Rooms,
}

/// One machine, as the proxy describes it. Deliberately no `daemon_url`: see
/// rule 1 above.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeviceRow {
    #[serde(default)]
    pub name: String,
    /// Where a fresh session lands when nobody has chosen.
    #[serde(default, rename = "default")]
    pub is_default: bool,
    #[serde(default)]
    pub selected: bool,
    #[serde(default)]
    pub health: DeviceHealth,
}

/// What the proxy's live probe of that machine's daemon found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Deserialize)]
pub struct DeviceHealth {
    /// `ok`, `unhealthy`, `unreachable` — or empty on a daemon/proxy that
    /// answers a shape we do not know, which reads as unknown, not as down.
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub rev: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct DeviceListResponse {
    #[serde(default)]
    devices: Vec<DeviceRow>,
    #[serde(default)]
    selected: String,
    #[serde(default)]
    selection_explicit: bool,
}

/// The typed refusal every proxied route answers when the machine a session is
/// attached to cannot serve it.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceUnavailable {
    #[serde(default)]
    pub error: String,
    /// `unreachable` (the machine did not answer) or `unknown_device` (the
    /// roster no longer has it).
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub device: String,
}

/// What the picker is doing right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerPhase {
    /// Nothing asked yet — the state an off-proxy host stays in.
    Idle,
    Loading,
    Ready,
    /// A selection is in flight for this device; its row shows the wait and no
    /// second switch may start.
    Switching(String),
    /// The last load or switch failed, with something a person can read.
    Failed(String),
}

// ── pure decisions ────────────────────────────────────────────────
//
// Every branch worth arguing about is a free function with a test. The proxy
// stays the authority on all of them; these exist so the surface behaves the
// same way twice, not to replace a round trip.

/// Offer the picker unprompted?
///
/// Only when there is a real choice to make (more than one machine), the
/// person has not already made it (`selection_explicit`), and we have not
/// already opened it in this page's life. The third guard is about not
/// re-opening the panel under somebody who just closed it — the durable "asked
/// and answered" fact is the server's, not ours.
pub fn should_offer_picker(devices: usize, selection_explicit: bool, offered: bool) -> bool {
    devices > 1 && !selection_explicit && !offered
}

/// Is a switch to `target` worth doing?
///
/// Re-selecting the machine you are already on would tear down a live SSE
/// stream, drop the transcript, and re-fetch three catalogues to arrive
/// exactly where it started. An empty target is never a switch.
pub fn switch_needed(current: &str, target: &str) -> bool {
    !target.trim().is_empty() && current != target
}

/// What a device switch does with the session id this browser remembers.
///
/// A session belongs to the daemon that minted it. On the machine we just
/// moved to it either exists — in which case reopen it, which is the whole
/// point of "pick up where you left off" — or it does not, in which case the
/// browser's memory of it is stale and gets cleared. Neither outcome is an
/// error, and the second one especially must not be reported as one: it is the
/// ordinary case of two machines with different histories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRestore {
    /// The machine has it: reopen that transcript.
    Restore,
    /// The machine does not have it (or could not say): forget it and start
    /// clean. Never an error surfaced to the person.
    Clear,
}

/// Classify a `GET /v1/sessions/{id}` reply from the machine we just attached
/// to. `ok` is the daemon's own envelope flag; `has_session` is whether it
/// actually named a session.
pub fn classify_session_restore(ok: bool, has_session: bool) -> SessionRestore {
    if ok && has_session {
        SessionRestore::Restore
    } else {
        SessionRestore::Clear
    }
}

/// Did this browser move to another machine while this tab was not looking?
///
/// Only a change between two NAMED machines counts. Learning a name for the
/// first time (`was` empty) is this tab's own boot, not a switch, and an empty
/// answer from the proxy is an unreadable reply rather than a machine — either
/// one would tear down a healthy transcript to arrive back where it started.
pub fn switched_underneath(was: &str, now: &str) -> bool {
    !was.trim().is_empty() && !now.trim().is_empty() && was != now
}

/// Should the surface show device chrome at all?
///
/// A single-device deployment — every single-operator install, and every
/// roster entry that never listed devices — gets a name back from
/// `/api/devices` too, so "we learned a name" is not the same question as
/// "there is a choice". Chrome whose only action is to reselect the machine
/// you are already on is control density for its own sake.
pub fn device_chrome_visible(selected: &str, devices: usize) -> bool {
    !selected.trim().is_empty() && devices > 1
}

/// May a reply that has just come back still write what it was sent to write?
///
/// Used twice over, for the same reason in two shapes: a device listing that
/// started before a switch, and a session restore that started before somebody
/// began a different session. Both are answers to a question nobody is asking
/// any more, and both would otherwise win the last write.
pub fn claim_is_current(claimed: u64, current: u64) -> bool {
    claimed == current
}

/// One line under a device's name. Health is the only reason to hesitate over
/// a machine, so it is stated plainly rather than as a coloured dot alone.
pub fn health_label(health: &DeviceHealth) -> String {
    match health.state.as_str() {
        "ok" if !health.version.is_empty() => format!("online · {}", health.version),
        "ok" => "online".to_string(),
        "unhealthy" => "answering, but not healthy".to_string(),
        "unreachable" => "not answering".to_string(),
        _ => "unknown".to_string(),
    }
}

/// The `data-health` value a row carries, for the stylesheet to colour. Kept
/// to a closed set so an unknown state from a newer proxy renders as neutral
/// rather than as a missing rule.
pub fn health_state(health: &DeviceHealth) -> &'static str {
    match health.state.as_str() {
        "ok" => "ok",
        "unhealthy" => "degraded",
        "unreachable" => "down",
        _ => "unknown",
    }
}

/// A sentence about a machine that cannot serve this session.
///
/// Two callers, one wording: the typed 503 a proxied route answers, and the
/// picker's own reading of a device's health probe. `reason` is the proxy's
/// vocabulary (`unknown_device`, `unreachable`); anything else reads as not
/// answering, which is the safe thing to say about a machine we cannot
/// characterise.
pub fn unavailable_message(device: &str, reason: &str) -> String {
    let device = if device.trim().is_empty() {
        "That device"
    } else {
        device
    };
    match reason {
        "unknown_device" => format!("{device} is no longer on your roster. Pick another machine."),
        _ => format!("{device} isn't answering. Pick another machine, or wake it up."),
    }
}

/// Read a proxied route's typed 503 into that sentence.
///
/// Without this a `device_unavailable` body reaches the surface as a JSON
/// decode error against whatever shape the caller expected — the exact failure
/// mode the typed body exists to end.
pub async fn device_unavailable_error(response: gloo_net::http::Response) -> String {
    match response.json::<DeviceUnavailable>().await {
        Ok(body) if body.error == "device_unavailable" => {
            unavailable_message(&body.device, &body.reason)
        }
        _ => "The machine this session is on isn't answering.".to_string(),
    }
}

/// The banner the picker shows about the machine you are currently on, or
/// `None` while it is healthy. Health is the proxy's live probe, so this is a
/// reading of fact rather than a guess.
pub fn attached_warning(devices: &[DeviceRow], selected: &str) -> Option<String> {
    let row = devices.iter().find(|row| row.name == selected)?;
    match row.health.state.as_str() {
        "ok" | "" => None,
        reason => Some(unavailable_message(&row.name, reason)),
    }
}

// ── reactive handle ───────────────────────────────────────────────

/// The picker's state, created once at app scope.
#[derive(Clone, Copy)]
pub struct DeviceState {
    pub devices: RwSignal<Vec<DeviceRow>>,
    /// The machine this session is attached to, by name. Empty until
    /// `/api/devices` answers — and on hosts where it never will, which is what
    /// keeps the header chip and the menu entry absent off-proxy.
    pub selected: RwSignal<String>,
    pub phase: RwSignal<PickerPhase>,
    pub open: RwSignal<bool>,
    /// Whether this page has already offered the choice unprompted.
    offered: RwSignal<bool>,
    /// Monotonic ticket for everything that writes `selected`.
    ///
    /// `GET /api/devices` snapshots the selection before it probes each
    /// machine's health, so a listing that started before a switch can land
    /// after it carrying the OLD name — overwriting the choice that just
    /// succeeded, showing the wrong machine in the header indefinitely, and
    /// (through the focus re-check) firing a re-attach that clears the
    /// transcript the switch just restored. Only the newest claimant writes.
    generation: RwSignal<u64>,
}

impl Default for DeviceState {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceState {
    pub fn new() -> Self {
        Self {
            devices: RwSignal::new(Vec::new()),
            selected: RwSignal::new(String::new()),
            phase: RwSignal::new(PickerPhase::Idle),
            open: RwSignal::new(false),
            offered: RwSignal::new(false),
            generation: RwSignal::new(0),
        }
    }

    /// Is there a choice to show chrome for?
    ///
    /// A single-device deployment — which is every single-operator install and
    /// every roster entry that never listed devices — gets a name back from
    /// `/api/devices` too. Rendering the chip and the menu row off `known()`
    /// therefore gave those installs permanent chrome whose only action is to
    /// reselect the machine they are already on. Control density is a design
    /// defect; the header shows a device when there is another one to go to.
    pub fn has_choice(&self) -> bool {
        device_chrome_visible(&self.selected.get(), self.devices.get().len())
    }

    /// Ask the proxy which machines this login has, and offer the choice once
    /// if there is one to make.
    ///
    /// Quiet on every failure. The Chrome side panel and Tauri have no proxy in
    /// front of them, so this route does not exist there and its absence must
    /// leave the surface exactly as it was.
    pub fn load(self, auto_offer: bool) {
        self.refresh(None, auto_offer);
    }

    /// Re-read the list, and — when `attach` is given — re-attach if the proxy
    /// says this browser is now on a different machine than this tab thinks.
    ///
    /// The case this exists for is two tabs. A selection is per browser, so
    /// switching in one tab moves the other tab's traffic too: the proxy ends
    /// its open stream (it must, or the tab would keep tailing the machine it
    /// left) and the tab reconnects onto the new one — while still showing the
    /// transcript it had already rendered from the old machine. That is the
    /// same blend, one layer up. Nothing pushes the change here, so the check
    /// runs when the tab is looked at again, which is the moment a stale
    /// transcript would otherwise be read as current.
    pub fn refresh(self, attach: Option<Attachments>, auto_offer: bool) {
        if crate::daemon::running_as_extension() {
            return;
        }
        let claimed = self.claim();
        self.phase.set(PickerPhase::Loading);
        spawn_local(async move {
            match fetch_devices().await {
                Ok(list) => {
                    // The proxy snapshots `selected` before probing health, so
                    // this answer can be older than a switch that has already
                    // happened. Only the newest claimant writes.
                    if !self.still_current(claimed) {
                        log::debug!("device listing superseded before it landed");
                        return;
                    }
                    let was = self.selected.get_untracked();
                    self.selected.set(list.selected.clone());
                    let count = list.devices.len();
                    self.devices.set(list.devices);
                    self.phase.set(PickerPhase::Ready);
                    if let Some(attach) = attach {
                        if switched_underneath(&was, &list.selected) {
                            log::info!("this browser is now on '{}'; re-attaching", list.selected);
                            reattach(&attach);
                        }
                    }
                    if auto_offer
                        && should_offer_picker(
                            count,
                            list.selection_explicit,
                            self.offered.get_untracked(),
                        )
                    {
                        self.offered.set(true);
                        self.open.set(true);
                    }
                }
                Err(error) => {
                    // Never chrome, never a toast: a host with no proxy is not
                    // a broken surface.
                    log::debug!("device list unavailable: {error}");
                    if self.still_current(claimed) {
                        self.phase.set(PickerPhase::Idle);
                    }
                }
            }
        });
    }

    /// Take the next ticket. Everything that writes `selected` holds one.
    fn claim(self) -> u64 {
        let next = self.generation.get_untracked().wrapping_add(1);
        self.generation.set(next);
        next
    }

    fn still_current(self, claimed: u64) -> bool {
        claim_is_current(claimed, self.generation.get_untracked())
    }

    /// Re-check which machine this browser is on whenever the tab is looked at
    /// again. Registered once, at app scope.
    pub fn recheck_on_focus(self, attach: Attachments) {
        if crate::daemon::running_as_extension() {
            return;
        }
        let Some(window) = web_sys::window() else {
            return;
        };
        let handler = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
            self.refresh(Some(attach.clone()), false);
        });
        if window
            .add_event_listener_with_callback("focus", handler.as_ref().unchecked_ref())
            .is_ok()
        {
            // Lives as long as the page does, which is the listener's life too.
            handler.forget();
        }
    }

    /// Attach this session to `name`, then re-attach everything to the machine
    /// it now points at.
    pub fn select(self, attach: Attachments, name: String) {
        if !switch_needed(&self.selected.get_untracked(), &name) {
            self.open.set(false);
            return;
        }
        if matches!(self.phase.get_untracked(), PickerPhase::Switching(_)) {
            return;
        }
        self.phase.set(PickerPhase::Switching(name.clone()));
        let claimed = self.claim();
        spawn_local(async move {
            match post_selection(&name).await {
                Ok(()) => {
                    // A listing that started before this POST is stale now, and
                    // this claim is what retires it.
                    if !self.still_current(claimed) {
                        log::debug!("a newer device intent superseded this switch");
                        return;
                    }
                    self.selected.set(name.clone());
                    self.open.set(false);
                    self.phase.set(PickerPhase::Ready);
                    reattach(&attach);
                    // Re-read health and `selected` from the proxy, which is
                    // the authority on both.
                    self.refresh(None, false);
                }
                Err(error) => {
                    log::warn!("device selection failed: {error}");
                    if self.still_current(claimed) {
                        self.phase.set(PickerPhase::Failed(error));
                    }
                }
            }
        });
    }
}

/// Everything that belonged to the machine being left.
///
/// The daemon's half is its own: streams, transcript, catalogues. The room is
/// separate state with a separate generation and a separate tail, and closing
/// it is synchronous on purpose — the rooms contract says opening and closing
/// share one reset path so transcript, access and tail state cannot leak
/// across room identity, and a machine switch is the widest identity change
/// there is. Re-listing afterwards shows the new machine's rooms, which are
/// the only rooms now reachable.
fn reattach(attach: &Attachments) {
    attach.rooms.close_room();
    attach.rooms.fetch_rooms();
    attach.daemon.reattach_to_selected_device();
}

async fn fetch_devices() -> Result<DeviceListResponse, String> {
    let response = gloo_net::http::Request::get("/api/devices")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    response
        .json::<DeviceListResponse>()
        .await
        .map_err(|error| error.to_string())
}

async fn post_selection(name: &str) -> Result<(), String> {
    let request = gloo_net::http::Request::post("/api/devices/select")
        .header("content-type", "application/json")
        .json(&serde_json::json!({ "name": name }))
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if response.ok() {
        Ok(())
    } else if response.status() == 404 {
        Err(format!("{name} is no longer on your roster"))
    } else {
        Err(format!("HTTP {}", response.status()))
    }
}

// ── view ──────────────────────────────────────────────────────────

/// The current machine, in the header, next to the connection status.
///
/// Absent entirely until the proxy names a machine, so a single-device
/// deployment and every off-proxy host keep the header they have.
#[component]
pub fn DeviceChip(state: DeviceState) -> impl IntoView {
    view! {
        <Show when=move || state.has_choice()>
            <button
                class="ocean-device-chip"
                type="button"
                title="Switch device"
                aria-label=move || format!("device: {} — switch", state.selected.get())
                on:click=move |_| state.open.set(true)
            >
                <span class="ocean-device-chip__dot"></span>
                <span class="ocean-device-chip__name">{move || state.selected.get()}</span>
            </button>
        </Show>
    }
}

/// The picker itself.
///
/// A self-contained overlay that consumes its own Escape — like the palette
/// and the slash popover, and unlike the reveal rail, whose z-ordered
/// close-exactly-one chain this deliberately stays out of.
#[component]
pub fn DevicePicker(state: DeviceState, attach: Attachments) -> impl IntoView {
    let attach_for_rows = attach.clone();
    let panel: NodeRef<leptos::html::Div> = NodeRef::new();
    // Focus the dialog when it opens, which is both the a11y contract for a
    // modal and what makes the Escape handler below reachable: a `keydown` on
    // this element only sees events from it and its descendants, and opening
    // the picker otherwise leaves focus on the header chip or the menu row.
    // Without this the first Escape sails past to the window rail and closes a
    // reveal UNDERNEATH the open picker.
    Effect::new(move |_| {
        if state.open.get() {
            if let Some(element) = panel.get() {
                let _ = element.focus();
            }
        }
    });
    view! {
        <div
            class="devices-overlay"
            hidden=move || !state.open.get()
            on:click=move |_| state.open.set(false)
        >
            <div
                class="devices-panel"
                node_ref=panel
                role="dialog"
                aria-modal="true"
                aria-label="Choose a device"
                tabindex="-1"
                on:click=|event| event.stop_propagation()
                on:keydown=move |event| {
                    if event.key() == "Escape" {
                        // Consumed here so the window-level rail never sees it
                        // and closes a reveal underneath this panel.
                        event.stop_propagation();
                        state.open.set(false);
                    }
                }
            >
                <header class="devices-panel__head">
                    <h2 class="devices-panel__title">"Your devices"</h2>
                    <button
                        class="sessions-panel__close"
                        type="button"
                        aria-label="Close"
                        on:click=move |_| state.open.set(false)
                    >
                        "✕"
                    </button>
                </header>
                <p class="devices-panel__note">
                    "Pick the machine to work on. Your sessions come from it."
                </p>
                <Show when=move || {
                    attached_warning(&state.devices.get(), &state.selected.get()).is_some()
                }>
                    <p class="devices-panel__warning" role="status">
                        {move || {
                            attached_warning(&state.devices.get(), &state.selected.get())
                                .unwrap_or_default()
                        }}
                    </p>
                </Show>
                <ul class="devices-panel__list">
                    <For
                        each=move || state.devices.get()
                        key=|row| (row.name.clone(), row.selected, row.health.clone())
                        let:row
                    >
                        {
                            let attach = attach_for_rows.clone();
                            let name = row.name.clone();
                            let switching = {
                                let name = name.clone();
                                move || state.phase.get() == PickerPhase::Switching(name.clone())
                            };
                            view! {
                                <li class="devices-panel__row">
                                    <button
                                        class="devices-panel__pick"
                                        type="button"
                                        data-health=health_state(&row.health)
                                        aria-current=move || row.selected.to_string()
                                        disabled=switching
                                        on:click=move |_| state.select(attach.clone(), name.clone())
                                    >
                                        <span class="devices-panel__dot"></span>
                                        <span class="devices-panel__name">
                                            {row.name.clone()}
                                        </span>
                                        <span class="devices-panel__health">
                                            {if switching() {
                                                "switching\u{2026}".to_string()
                                            } else {
                                                health_label(&row.health)
                                            }}
                                        </span>
                                        <Show when=move || row.selected>
                                            <span class="devices-panel__current">"current"</span>
                                        </Show>
                                    </button>
                                </li>
                            }
                        }
                    </For>
                </ul>
                <Show when=move || matches!(state.phase.get(), PickerPhase::Failed(_))>
                    <p class="devices-panel__error" role="alert">
                        {move || match state.phase.get() {
                            PickerPhase::Failed(message) => message,
                            _ => String::new(),
                        }}
                    </p>
                </Show>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn health(state: &str, version: &str) -> DeviceHealth {
        DeviceHealth {
            state: state.to_string(),
            version: version.to_string(),
            rev: String::new(),
        }
    }

    #[test]
    fn the_picker_is_offered_once_and_only_with_a_choice_to_make() {
        // One machine is not a choice; offering it would be chrome for its
        // own sake.
        assert!(!should_offer_picker(1, false, false));
        assert!(!should_offer_picker(0, false, false));
        // Two machines and nobody has picked: ask, once.
        assert!(should_offer_picker(2, false, false));
        // Already asked in this page's life — closing it must not reopen it.
        assert!(!should_offer_picker(2, false, true));
        // The person picked at some point in the past: the server remembers,
        // so a browser opened tomorrow is not asked again.
        assert!(!should_offer_picker(2, true, false));
    }

    #[test]
    fn re_picking_the_current_machine_is_not_a_switch() {
        assert!(!switch_needed("mini", "mini"));
        assert!(switch_needed("mini", "studio"));
        // Nothing is not a device. An empty target would tear the stream down
        // and reconnect to the same place.
        assert!(!switch_needed("mini", ""));
        assert!(!switch_needed("mini", "   "));
        // No current device yet (first answer from the proxy) still switches.
        assert!(switch_needed("", "studio"));
    }

    #[test]
    fn a_session_the_new_machine_does_not_have_is_cleared_not_errored() {
        // It is there: reopen it. This is "pick up where you left off".
        assert_eq!(
            classify_session_restore(true, true),
            SessionRestore::Restore
        );
        // The daemon answered, and does not have it. Two machines with
        // different histories is the ordinary case, not a failure.
        assert_eq!(classify_session_restore(true, false), SessionRestore::Clear);
        // The daemon refused the envelope, or named nothing at all.
        assert_eq!(classify_session_restore(false, true), SessionRestore::Clear);
        assert_eq!(
            classify_session_restore(false, false),
            SessionRestore::Clear
        );
    }

    #[test]
    fn a_tab_re_attaches_only_when_the_machine_under_it_really_changed() {
        // Another tab switched while this one was in the background: the proxy
        // ended its stream, it reconnected onto the new machine, and the
        // transcript on screen is the old machine's. That is a re-attach.
        assert!(switched_underneath("mini", "studio"));
        // This tab's own boot — it had no name yet. Re-attaching here would
        // clear a transcript that was just restored.
        assert!(!switched_underneath("", "studio"));
        // The proxy answered without a name (an older build, a stale
        // selection): not a machine, so not a switch.
        assert!(!switched_underneath("mini", ""));
        assert!(!switched_underneath("mini", "mini"));
    }

    #[test]
    fn device_chrome_appears_only_when_there_is_somewhere_to_go() {
        // The shape that made this wrong: a single-operator install answers
        // /api/devices with one named machine, so "we know a name" was true
        // and every such deployment grew a header chip and a menu row whose
        // only action was to reselect the machine it was already on.
        assert!(!device_chrome_visible("mini", 1));
        assert!(device_chrome_visible("mini", 2));
        // Nothing learned yet, and the off-proxy hosts that never will.
        assert!(!device_chrome_visible("", 0));
        assert!(!device_chrome_visible("", 2));
        assert!(!device_chrome_visible("   ", 2));
    }

    #[test]
    fn an_answer_to_a_question_nobody_is_asking_any_more_does_not_write() {
        assert!(claim_is_current(7, 7));
        assert!(!claim_is_current(6, 7));
        // A listing that started two switches ago is not "close enough".
        assert!(!claim_is_current(5, 7));
        // The ticket wraps rather than saturating, so the comparison has to
        // hold across the boundary too.
        assert!(claim_is_current(u64::MAX.wrapping_add(1), 0));
    }

    #[test]
    fn health_reads_as_a_sentence_and_an_unknown_state_is_neutral() {
        assert_eq!(health_label(&health("ok", "0.9.2")), "online · 0.9.2");
        assert_eq!(health_label(&health("ok", "")), "online");
        assert_eq!(health_label(&health("unreachable", "")), "not answering");
        assert_eq!(
            health_label(&health("unhealthy", "")),
            "answering, but not healthy"
        );
        // A state this bundle has not heard of must not render as "down".
        assert_eq!(health_label(&health("draining", "")), "unknown");
        assert_eq!(health_state(&health("draining", "")), "unknown");
        assert_eq!(health_state(&health("ok", "")), "ok");
        assert_eq!(health_state(&health("unreachable", "")), "down");
        assert_eq!(health_state(&health("unhealthy", "")), "degraded");
    }

    #[test]
    fn the_typed_503_becomes_something_a_person_can_act_on() {
        let unreachable = DeviceUnavailable {
            error: "device_unavailable".into(),
            reason: "unreachable".into(),
            device: "studio".into(),
        };
        assert_eq!(
            unavailable_message(&unreachable.device, &unreachable.reason),
            "studio isn't answering. Pick another machine, or wake it up."
        );
        assert_eq!(
            unavailable_message("studio", "unknown_device"),
            "studio is no longer on your roster. Pick another machine."
        );
        // A body without a device name still reads as a sentence.
        assert!(unavailable_message("", "unreachable").starts_with("That device isn't answering"));
        // A reason from a newer proxy is not narrated as something specific.
        assert_eq!(
            unavailable_message("studio", "draining"),
            "studio isn't answering. Pick another machine, or wake it up."
        );
    }

    #[test]
    fn the_picker_says_when_the_machine_you_are_on_is_not_answering() {
        let rows = vec![
            DeviceRow {
                name: "mini".into(),
                is_default: true,
                selected: false,
                health: health("ok", "0.9.2"),
            },
            DeviceRow {
                name: "studio".into(),
                is_default: false,
                selected: true,
                health: health("unreachable", ""),
            },
        ];
        assert_eq!(
            attached_warning(&rows, "studio").as_deref(),
            Some("studio isn't answering. Pick another machine, or wake it up.")
        );
        // The machine you are on is fine: no banner, no chrome.
        assert_eq!(attached_warning(&rows, "mini"), None);
        // A name that is not in the list says nothing rather than guessing.
        assert_eq!(attached_warning(&rows, "someone-elses-mac"), None);
        // An unknown health state is not narrated as down.
        let unknown = vec![DeviceRow {
            name: "mini".into(),
            is_default: true,
            selected: true,
            health: DeviceHealth::default(),
        }];
        assert_eq!(attached_warning(&unknown, "mini"), None);
    }

    #[test]
    fn the_device_list_decodes_the_proxys_shape_and_tolerates_an_older_one() {
        let list: DeviceListResponse = serde_json::from_str(
            r#"{"ok":true,"selected":"studio","selection_explicit":true,"devices":[
                {"name":"mini","default":true,"selected":false,
                 "health":{"state":"ok","version":"0.9.2","rev":"abc1234"}},
                {"name":"studio","default":false,"selected":true,
                 "health":{"state":"unreachable","version":"","rev":""}}
            ]}"#,
        )
        .expect("device list");
        assert_eq!(list.selected, "studio");
        assert!(list.selection_explicit);
        assert!(list.devices[0].is_default);
        assert!(list.devices[1].selected);
        assert_eq!(list.devices[0].health.version, "0.9.2");

        // A proxy that predates health (or a shape we do not know) must not
        // make the picker undecodable — it renders as unknown.
        let sparse: DeviceListResponse =
            serde_json::from_str(r#"{"devices":[{"name":"mini"}],"selected":"mini"}"#)
                .expect("sparse list");
        assert_eq!(sparse.devices[0].name, "mini");
        assert_eq!(health_state(&sparse.devices[0].health), "unknown");
        assert!(!sparse.selection_explicit);
    }
}
