//! Historical Observatory views, rebuilt on the client.
//!
//! Since Gate 1 repair G1 the daemon answers `GET /v1/observatory/snapshot`
//! only at the current watermark; an earlier `at` is refused with `409
//! snapshot_not_historical` because the projection cannot reconstruct past
//! state. The replay rail therefore never asks for a past snapshot. It starts
//! from an empty state (keeping the session-local slot registry so cubicles
//! do not move), pages `GET /v1/observatory/replay` from the earliest retained
//! cursor through the target, and folds every event with the same reducer the
//! live stream uses.
//!
//! Everything here is pure so it can be unit-tested off the browser.

use serde::Deserialize;

use super::domain::{
    parse_cursor, Correlation, Cursor, EventEnvelope, EventKind, EventPayload, IntegrityState,
    ObservatoryState, Producer, Topology, TruthProvenance,
};
use super::reducer::apply;

/// Events requested per replay page. The daemon clamps to 1..=10000.
pub const REPLAY_PAGE_LIMIT: usize = 1_000;
/// Upper bound on pages folded for one scrub, so a runaway range cannot pin
/// the tab. 200 pages × 1000 events is far beyond any retained window today.
pub const MAX_REPLAY_PAGES: usize = 200;

/// Wire shape of `GET /v1/observatory/replay` (manifest §7.3).
#[derive(Debug, Clone, Deserialize)]
pub struct ReplayPage {
    /// Kept as raw JSON so one unknown event kind cannot sink a whole page.
    #[serde(default)]
    pub events: Vec<serde_json::Value>,
    #[serde(default, deserialize_with = "deserialize_optional_cursor")]
    pub next_after: Option<Cursor>,
    #[serde(default)]
    pub has_more: bool,
    #[serde(default)]
    pub complete: bool,
    pub meta: ReplayMeta,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReplayMeta {
    pub daemon_instance_id: String,
    pub observatory_id: String,
}

/// A replay event. It is the full envelope minus the authority ids, which the
/// page carries once in `meta`.
#[derive(Debug, Clone, Deserialize)]
struct ReplayEvent {
    schema_version: u32,
    #[serde(deserialize_with = "deserialize_cursor_value")]
    cursor: Cursor,
    event_id: String,
    #[serde(default)]
    occurred_at: String,
    #[serde(default)]
    recorded_at: String,
    kind: EventKind,
    #[serde(default)]
    truth: TruthProvenance,
    #[serde(default)]
    producer: Producer,
    #[serde(default)]
    topology: Topology,
    #[serde(default)]
    correlation: Correlation,
    payload: EventPayload,
}

impl ReplayEvent {
    fn into_envelope(self, meta: &ReplayMeta) -> EventEnvelope {
        EventEnvelope {
            schema_version: self.schema_version,
            cursor: self.cursor.to_string(),
            event_id: self.event_id,
            observatory_id: meta.observatory_id.clone(),
            daemon_instance_id: meta.daemon_instance_id.clone(),
            occurred_at: self.occurred_at,
            recorded_at: self.recorded_at,
            kind: self.kind,
            truth: self.truth,
            producer: self.producer,
            topology: self.topology,
            correlation: self.correlation,
            payload: self.payload,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WireCursor {
    String(String),
    Number(u64),
}

impl WireCursor {
    fn value(self) -> Cursor {
        match self {
            Self::String(value) => parse_cursor(&value),
            Self::Number(value) => value,
        }
    }
}

fn deserialize_cursor_value<'de, D>(deserializer: D) -> Result<Cursor, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(WireCursor::deserialize(deserializer)?.value())
}

fn deserialize_optional_cursor<'de, D>(deserializer: D) -> Result<Option<Cursor>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<WireCursor>::deserialize(deserializer)?.map(WireCursor::value))
}

/// Where the first replay page starts: just before the earliest retained
/// cursor, so the daemon never answers `410 retention_boundary_crossed`.
pub fn replay_start(earliest_cursor: Cursor) -> Cursor {
    earliest_cursor.saturating_sub(1)
}

/// The URL of one replay page.
pub fn replay_path(after: Cursor, through: Cursor) -> String {
    format!("/v1/observatory/replay?after={after}&through={through}&limit={REPLAY_PAGE_LIMIT}")
}

/// An empty state for the same authority that keeps the slot registry, so a
/// replayed execution lands in the cubicle it already owns.
pub fn replay_base(previous: &ObservatoryState) -> ObservatoryState {
    let mut floor_slots = previous.floor_slots.clone();
    for node in previous.nodes.values() {
        floor_slots
            .entry(node.execution_id.clone())
            .or_insert(node.floor_slot);
    }
    let next_floor_slot = floor_slots
        .values()
        .copied()
        .max()
        .map_or(0, |slot| slot.saturating_add(1))
        .max(previous.next_floor_slot);
    ObservatoryState {
        observatory_id: previous.observatory_id.clone(),
        daemon_instance_id: String::new(),
        earliest_cursor: previous.earliest_cursor,
        floor_slots,
        next_floor_slot,
        ..ObservatoryState::default()
    }
}

/// Fold one replay page into `state` with the live reducer. Events past
/// `target` are ignored. An event this client cannot parse is skipped, which
/// leaves a cursor gap the reducer reports as incomplete instead of hiding it.
pub fn fold_replay_page(
    mut state: ObservatoryState,
    page: &ReplayPage,
    target: Cursor,
) -> ObservatoryState {
    for raw in &page.events {
        match serde_json::from_value::<ReplayEvent>(raw.clone()) {
            Ok(event) if event.cursor <= target => {
                state = apply(state, event.into_envelope(&page.meta));
            }
            Ok(_) => {}
            Err(error) => log::warn!("unparseable Observatory replay event: {error}"),
        }
    }
    state
}

/// Label the folded state as the historical view at `target`.
pub fn finish_replay(mut state: ObservatoryState, target: Cursor) -> ObservatoryState {
    state.cursor = state.cursor.max(target);
    let pruned = state.earliest_cursor > 1;
    match state.integrity {
        IntegrityState::Gap => {
            let detail = state
                .last_error
                .take()
                .unwrap_or_else(|| "event gap".into());
            state.last_error = Some(format!(
                "Replay at cursor {target} is incomplete: {detail}."
            ));
        }
        _ if pruned => {
            state.integrity = IntegrityState::Gap;
            state.last_error = Some(format!(
                "Replay at cursor {target}: history before cursor {} was pruned, so executions admitted earlier are not shown.",
                state.earliest_cursor
            ));
        }
        IntegrityState::Disconnected => {
            state.integrity = IntegrityState::Historical;
            state.last_error = Some(format!(
                "Replay at cursor {target}: the daemon was stopping at this point."
            ));
        }
        _ => {
            state.integrity = IntegrityState::Historical;
            state.last_error = Some(format!(
                "Rebuilt from recorded events through cursor {target}. Press Live to resume."
            ));
        }
    }
    state
}

/// Why an Observatory request did not produce state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The request never reached the daemon (or the response was cut off).
    Network(String),
    /// `409 snapshot_not_historical`: the daemon only snapshots the current
    /// watermark. A protocol answer, not a disconnect.
    NotHistorical(String),
    /// The replay route cannot serve this range (pruned, unsupported, bad
    /// range). The daemon is reachable; history is not.
    ReplayUnavailable(String),
    /// Any other non-2xx answer.
    Http { status: u16, message: String },
    /// A 2xx body this client could not decode.
    Invalid(String),
}

impl FetchError {
    pub fn message(&self) -> String {
        match self {
            Self::Network(message) | Self::Invalid(message) => message.clone(),
            Self::NotHistorical(message) | Self::ReplayUnavailable(message) => {
                format!("Historical replay unavailable: {message}")
            }
            Self::Http { status, message } => format!("daemon answered {status}: {message}"),
        }
    }

    /// True when the daemon is reachable and only history is missing.
    pub fn is_replay_unavailable(&self) -> bool {
        matches!(self, Self::NotHistorical(_) | Self::ReplayUnavailable(_))
    }
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error: String,
    #[serde(default)]
    message: String,
}

/// Classify a non-2xx Observatory answer. `replay` marks the replay route,
/// where range problems mean "history unavailable" rather than a fault.
pub fn classify_http_error(status: u16, body: &str, replay: bool) -> FetchError {
    let parsed = serde_json::from_str::<ErrorBody>(body).ok();
    let code = parsed
        .as_ref()
        .map(|body| body.error.as_str())
        .unwrap_or_default();
    let message = parsed
        .as_ref()
        .map(|body| body.message.clone())
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| {
            if body.trim().is_empty() {
                code.to_owned()
            } else {
                body.trim().to_owned()
            }
        });
    if status == 409 && code == "snapshot_not_historical" {
        let message = if message.is_empty() {
            "the daemon only snapshots the current watermark".to_owned()
        } else {
            message
        };
        return FetchError::NotHistorical(message);
    }
    if replay && matches!(status, 400 | 404 | 405 | 409 | 410 | 501) {
        let message = if message.is_empty() {
            format!("replay route answered {status}")
        } else {
            message
        };
        return FetchError::ReplayUnavailable(message);
    }
    FetchError::Http { status, message }
}

/// The client-side effect of a failed fetch. Returns whether the connection
/// is actually lost; a replay-unavailable answer never reads as a disconnect.
pub fn apply_fetch_error(state: &mut ObservatoryState, error: &FetchError) -> bool {
    state.last_error = Some(error.message());
    if error.is_replay_unavailable() {
        state.integrity = IntegrityState::ReplayUnavailable;
        false
    } else {
        state.integrity = IntegrityState::Disconnected;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observatory::domain::ExecutionPhase;
    use serde_json::json;

    fn event(
        cursor: u64,
        kind: &str,
        execution: &str,
        data: serde_json::Value,
    ) -> serde_json::Value {
        json!({
            "cursor": cursor.to_string(),
            "event_id": format!("event-{cursor}"),
            "schema_version": 1,
            "occurred_at": format!("2026-07-17T00:00:{cursor:02}Z"),
            "recorded_at": format!("2026-07-17T00:00:{cursor:02}Z"),
            "kind": kind,
            "truth": "host_observed",
            "producer": { "kind": "daemon", "id": "ocean-daemon" },
            "topology": { "execution_id": execution, "root_execution_id": execution },
            "correlation": {},
            "visibility": "metadata",
            "payload": { "kind": kind_payload_tag(kind), "data": data },
        })
    }

    fn kind_payload_tag(kind: &str) -> String {
        kind.split('_')
            .map(|part| {
                let mut chars = part.chars();
                chars
                    .next()
                    .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
                    .unwrap_or_default()
            })
            .collect()
    }

    fn page(
        events: Vec<serde_json::Value>,
        next_after: Option<&str>,
        has_more: bool,
    ) -> ReplayPage {
        serde_json::from_value(json!({
            "events": events,
            "next_after": next_after,
            "has_more": has_more,
            "complete": !has_more,
            "continuation_url": null,
            "meta": {
                "daemon_instance_id": "boot",
                "observatory_id": "obs",
                "after": "0",
                "through": null,
                "generated_at": "2026-07-17T00:00:00Z"
            }
        }))
        .expect("replay page decodes")
    }

    #[test]
    fn replay_page_decodes_the_daemon_wire_shape() {
        let decoded = page(
            vec![event(
                1,
                "execution_admitted",
                "a",
                json!({"phase": "running", "labels": []}),
            )],
            Some("1"),
            true,
        );
        assert_eq!(decoded.next_after, Some(1));
        assert!(decoded.has_more);
        assert_eq!(decoded.events.len(), 1);
    }

    #[test]
    fn folding_pages_rebuilds_the_past_state_at_the_target_cursor() {
        let first = page(
            vec![
                event(
                    1,
                    "execution_admitted",
                    "a",
                    json!({"phase": "running", "labels": ["agent a"]}),
                ),
                event(
                    2,
                    "execution_admitted",
                    "b",
                    json!({"phase": "running", "labels": []}),
                ),
            ],
            Some("2"),
            true,
        );
        let second = page(
            vec![
                event(
                    3,
                    "execution_finished",
                    "a",
                    json!({"phase": "finished", "duration_millis": 40, "error_classification": null}),
                ),
                // Past the target: must not leak into the historical view.
                event(
                    4,
                    "execution_admitted",
                    "c",
                    json!({"phase": "running", "labels": []}),
                ),
            ],
            None,
            false,
        );
        let base = replay_base(&ObservatoryState {
            earliest_cursor: 1,
            ..ObservatoryState::default()
        });
        let state = fold_replay_page(base, &first, 3);
        let state = fold_replay_page(state, &second, 3);
        let state = finish_replay(state, 3);

        assert_eq!(state.cursor, 3);
        assert_eq!(state.nodes["a"].phase, ExecutionPhase::Finished);
        assert_eq!(state.nodes["b"].phase, ExecutionPhase::Running);
        assert!(!state.nodes.contains_key("c"));
        assert_eq!(state.integrity, IntegrityState::Historical);
        assert_eq!(state.observatory_id, "obs");
    }

    #[test]
    fn replay_keeps_existing_cubicle_slots() {
        let mut previous = ObservatoryState {
            observatory_id: "obs".into(),
            earliest_cursor: 1,
            next_floor_slot: 2,
            ..ObservatoryState::default()
        };
        previous.floor_slots.insert("a".into(), 0);
        previous.floor_slots.insert("b".into(), 1);
        let replayed = fold_replay_page(
            replay_base(&previous),
            &page(
                vec![event(
                    1,
                    "execution_admitted",
                    "b",
                    json!({"phase": "running", "labels": []}),
                )],
                None,
                false,
            ),
            1,
        );
        assert_eq!(replayed.nodes["b"].floor_slot, 1);
        assert!(!replayed.nodes.contains_key("a"));
        assert_eq!(replayed.next_floor_slot, 2);
    }

    #[test]
    fn unparseable_event_surfaces_as_a_gap_not_silence() {
        let state = fold_replay_page(
            replay_base(&ObservatoryState {
                earliest_cursor: 1,
                ..ObservatoryState::default()
            }),
            &page(
                vec![
                    event(
                        1,
                        "execution_admitted",
                        "a",
                        json!({"phase": "running", "labels": []}),
                    ),
                    json!({"cursor": "2", "kind": "from_the_future"}),
                    event(
                        3,
                        "execution_admitted",
                        "b",
                        json!({"phase": "running", "labels": []}),
                    ),
                ],
                None,
                false,
            ),
            3,
        );
        let state = finish_replay(state, 3);
        assert_eq!(state.integrity, IntegrityState::Gap);
        assert!(state.last_error.unwrap().contains("incomplete"));
    }

    #[test]
    fn pruned_history_is_labelled_incomplete() {
        let state = finish_replay(
            replay_base(&ObservatoryState {
                earliest_cursor: 50,
                ..ObservatoryState::default()
            }),
            60,
        );
        assert_eq!(state.integrity, IntegrityState::Gap);
        assert!(state.last_error.unwrap().contains("pruned"));
        assert_eq!(replay_start(50), 49);
        assert_eq!(replay_start(0), 0);
    }

    #[test]
    fn replay_path_is_bounded_by_the_target() {
        assert_eq!(
            replay_path(0, 42),
            "/v1/observatory/replay?after=0&through=42&limit=1000"
        );
    }

    #[test]
    fn snapshot_not_historical_409_is_not_a_disconnect() {
        let body = r#"{"error":"snapshot_not_historical","message":"Only the current watermark can be snapshotted; omit `at` and tail from the returned watermark","http_status":409}"#;
        let error = classify_http_error(409, body, false);
        assert!(matches!(error, FetchError::NotHistorical(_)));
        assert!(error.is_replay_unavailable());

        let mut state = ObservatoryState::default();
        let disconnected = apply_fetch_error(&mut state, &error);
        assert!(!disconnected);
        assert_eq!(state.integrity, IntegrityState::ReplayUnavailable);
        assert_ne!(state.integrity, IntegrityState::Disconnected);
        assert!(state
            .last_error
            .unwrap()
            .starts_with("Historical replay unavailable"));
    }

    #[test]
    fn other_conflicts_and_server_errors_stay_distinct() {
        let conflict = classify_http_error(409, r#"{"error":"something_else"}"#, false);
        assert!(matches!(conflict, FetchError::Http { status: 409, .. }));

        let pruned = classify_http_error(
            410,
            r#"{"error":"retention_boundary_crossed","message":"Events from cursor 1 to 9 are not available"}"#,
            true,
        );
        assert_eq!(
            pruned,
            FetchError::ReplayUnavailable("Events from cursor 1 to 9 are not available".into())
        );

        let missing_route = classify_http_error(404, "", true);
        assert!(missing_route.is_replay_unavailable());

        let store_down = classify_http_error(503, r#"{"error":"store_unavailable"}"#, true);
        assert!(matches!(store_down, FetchError::Http { status: 503, .. }));
        let mut state = ObservatoryState::default();
        assert!(apply_fetch_error(&mut state, &store_down));
        assert_eq!(state.integrity, IntegrityState::Disconnected);

        let mut state = ObservatoryState::default();
        assert!(apply_fetch_error(
            &mut state,
            &FetchError::Network("snapshot unavailable".into())
        ));
        assert_eq!(state.integrity, IntegrityState::Disconnected);
    }
}
