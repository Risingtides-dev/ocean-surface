use std::collections::BTreeMap;

use super::domain::{
    parse_cursor, ActivityState, AttentionItem, AttentionPriority, EventEnvelope, EventPayload,
    ExecutionPhase, ExecutionState, IntegrityState, ObservatorySnapshot, ObservatoryState,
    SnapshotEdge,
};

const MAX_TRACKED_EXECUTIONS: usize = 2_000;
const MAX_SEEN_EVENTS: usize = 8_192;
const MAX_ATTENTION_ITEMS: usize = 128;
const MAX_FLOOR_SLOT_ENTRIES: usize = 4_096;

pub fn from_snapshot_preserving_slots(
    snapshot: ObservatorySnapshot,
    previous: &ObservatoryState,
) -> ObservatoryState {
    let same_authority =
        previous.observatory_id.is_empty() || previous.observatory_id == snapshot.observatory_id;
    let mut floor_slots = if same_authority {
        previous.floor_slots.clone()
    } else {
        BTreeMap::new()
    };
    if same_authority {
        for node in previous.nodes.values() {
            floor_slots
                .entry(node.execution_id.clone())
                .or_insert(node.floor_slot);
        }
    }
    let mut next_floor_slot = floor_slots
        .values()
        .copied()
        .max()
        .map_or(0, |slot| slot.saturating_add(1))
        .max(if same_authority {
            previous.next_floor_slot
        } else {
            0
        });
    let watermark = parse_cursor(&snapshot.watermark_cursor);
    let mut nodes = BTreeMap::new();
    for node in snapshot.nodes {
        let floor_slot = match floor_slots.get(&node.execution_id) {
            Some(slot) => *slot,
            None => {
                let slot = next_floor_slot;
                next_floor_slot = next_floor_slot.saturating_add(1);
                floor_slots.insert(node.execution_id.clone(), slot);
                slot
            }
        };
        nodes.insert(
            node.execution_id.clone(),
            ExecutionState {
                execution_id: node.execution_id,
                root_execution_id: node.root_execution_id,
                parent_execution_id: node.parent_execution_id,
                session_id: node.session_id,
                turn_id: node.turn_id,
                phase: node.phase,
                truth: node.truth,
                labels: node.labels,
                tools: BTreeMap::new(),
                permission_waiting: false,
                permission_reason: None,
                model_alias: None,
                floor_slot,
                started_at: node.started_at,
                last_cursor: watermark,
                duration_millis: node.duration_millis,
            },
        );
    }
    let edges = snapshot
        .edges
        .into_iter()
        .map(|edge| (edge.edge_id.clone(), edge))
        .collect();
    let mut attention = snapshot.attention;
    sort_attention(&mut attention);
    ObservatoryState {
        observatory_id: snapshot.observatory_id,
        daemon_instance_id: snapshot.daemon_instance_id,
        cursor: watermark,
        earliest_cursor: parse_cursor(&snapshot.earliest_available_cursor),
        nodes,
        floor_slots,
        next_floor_slot,
        edges,
        attention,
        seen_event_ids: Default::default(),
        integrity: IntegrityState::Live,
        last_error: None,
    }
}

pub fn apply(mut state: ObservatoryState, event: EventEnvelope) -> ObservatoryState {
    let cursor = parse_cursor(&event.cursor);
    if state.seen_event_ids.contains(&event.event_id) || cursor <= state.cursor {
        return state;
    }
    if !state.observatory_id.is_empty() && state.observatory_id != event.observatory_id {
        state.integrity = IntegrityState::Stale;
        state.last_error =
            Some("observatory authority changed; requesting a fresh snapshot".into());
        return state;
    }
    if !state.daemon_instance_id.is_empty()
        && state.daemon_instance_id != event.daemon_instance_id
        && !matches!(event.payload, EventPayload::DaemonStarted { .. })
    {
        state.integrity = IntegrityState::Stale;
        state.last_error = Some("daemon restarted; requesting a fresh snapshot".into());
        return state;
    }
    if state.cursor > 0 && cursor > state.cursor.saturating_add(1) {
        state.integrity = IntegrityState::Gap;
        state.last_error = Some(format!(
            "event gap {}–{}; state may be incomplete",
            state.cursor.saturating_add(1),
            cursor.saturating_sub(1)
        ));
        for node in state.nodes.values_mut() {
            if node.is_active() {
                node.tools.clear();
                node.permission_waiting = false;
            }
        }
    }

    state.observatory_id = event.observatory_id.clone();
    state.daemon_instance_id = event.daemon_instance_id.clone();
    state.cursor = cursor;
    state.seen_event_ids.insert(event.event_id.clone());
    if state.seen_event_ids.len() > MAX_SEEN_EVENTS {
        let remove = state.seen_event_ids.len() - MAX_SEEN_EVENTS;
        let stale: Vec<_> = state.seen_event_ids.iter().take(remove).cloned().collect();
        for id in stale {
            state.seen_event_ids.remove(&id);
        }
    }

    let execution_id = event.topology.execution_id.clone();
    match event.payload {
        EventPayload::DaemonStarted { .. } => {
            state.integrity = IntegrityState::Live;
            state.last_error = None;
        }
        EventPayload::DaemonStopping { .. } => {
            state.integrity = IntegrityState::Disconnected;
        }
        EventPayload::ExecutionAdmitted { phase, labels } => {
            if !execution_id.is_empty() {
                let next_from_nodes = state
                    .nodes
                    .values()
                    .map(|node| node.floor_slot)
                    .max()
                    .map_or(0, |slot| slot.saturating_add(1));
                let floor_slot = state
                    .floor_slots
                    .get(&execution_id)
                    .copied()
                    .or_else(|| state.nodes.get(&execution_id).map(|node| node.floor_slot))
                    .unwrap_or_else(|| state.next_floor_slot.max(next_from_nodes));
                state.floor_slots.insert(execution_id.clone(), floor_slot);
                state.next_floor_slot = state
                    .next_floor_slot
                    .max(floor_slot.saturating_add(1))
                    .max(next_from_nodes);
                let node =
                    state
                        .nodes
                        .entry(execution_id.clone())
                        .or_insert_with(|| ExecutionState {
                            execution_id: execution_id.clone(),
                            root_execution_id: event.topology.root_execution_id.clone(),
                            parent_execution_id: event.topology.parent_execution_id.clone(),
                            session_id: event.topology.session_id.clone(),
                            turn_id: event.topology.turn_id.clone(),
                            phase,
                            truth: event.truth,
                            labels: labels.clone(),
                            tools: BTreeMap::new(),
                            permission_waiting: false,
                            permission_reason: None,
                            model_alias: None,
                            floor_slot,
                            started_at: event.occurred_at.clone(),
                            last_cursor: cursor,
                            duration_millis: None,
                        });
                node.phase = phase;
                node.last_cursor = cursor;
                if !labels.is_empty() {
                    node.labels = labels;
                }
                if let (Some(edge_id), Some(parent)) =
                    (event.topology.edge_id, event.topology.parent_execution_id)
                {
                    state.edges.insert(
                        edge_id.clone(),
                        SnapshotEdge {
                            edge_id,
                            parent_execution_id: parent,
                            child_execution_id: execution_id,
                            root_execution_id: event.topology.root_execution_id,
                            created_at: event.occurred_at,
                            truth: event.truth,
                        },
                    );
                }
            }
        }
        EventPayload::ExecutionPhaseChanged { to_phase, .. } => {
            if let Some(node) = state.nodes.get_mut(&execution_id) {
                node.phase = to_phase;
                node.last_cursor = cursor;
                if to_phase.is_terminal() {
                    node.tools.clear();
                    node.permission_waiting = false;
                }
            }
        }
        EventPayload::ExecutionHeartbeat {} => {
            if let Some(node) = state.nodes.get_mut(&execution_id) {
                node.last_cursor = cursor;
            }
        }
        EventPayload::ExecutionFinished {
            phase,
            duration_millis,
            error_classification,
        } => {
            if let Some(node) = state.nodes.get_mut(&execution_id) {
                node.phase = phase;
                node.duration_millis = Some(duration_millis);
                node.last_cursor = cursor;
                node.tools.clear();
                node.permission_waiting = false;
            }
            if matches!(phase, ExecutionPhase::Error | ExecutionPhase::TimedOut) {
                push_attention(
                    &mut state,
                    AttentionItem {
                        execution_id,
                        priority: AttentionPriority::Critical,
                        reason: error_classification.unwrap_or_else(|| "execution_error".into()),
                        occurred_at: event.occurred_at,
                        dismissed: false,
                        interrupted: false,
                    },
                );
            }
        }
        EventPayload::ToolStarted {
            tool_name,
            model_alias,
        } => {
            if let Some(node) = state.nodes.get_mut(&execution_id) {
                let call_id = event
                    .correlation
                    .tool_call_id
                    .unwrap_or_else(|| format!("tool:{cursor}"));
                node.tools.insert(
                    call_id,
                    ActivityState {
                        tool_name,
                        started_cursor: cursor,
                    },
                );
                if !model_alias.is_empty() {
                    node.model_alias = Some(model_alias);
                }
                node.last_cursor = cursor;
            }
        }
        EventPayload::ToolFinished { .. } => {
            if let Some(node) = state.nodes.get_mut(&execution_id) {
                if let Some(call_id) = event.correlation.tool_call_id {
                    node.tools.remove(&call_id);
                } else if let Some(call_id) = node.tools.keys().next().cloned() {
                    node.tools.remove(&call_id);
                }
                node.last_cursor = cursor;
            }
        }
        EventPayload::PermissionWaiting { reason_code } => {
            if let Some(node) = state.nodes.get_mut(&execution_id) {
                node.permission_waiting = true;
                node.permission_reason = Some(reason_code.clone());
                node.last_cursor = cursor;
            }
            push_attention(
                &mut state,
                AttentionItem {
                    execution_id,
                    priority: AttentionPriority::High,
                    reason: reason_code,
                    occurred_at: event.occurred_at,
                    dismissed: false,
                    interrupted: false,
                },
            );
        }
        EventPayload::PermissionResolved { .. } => {
            if let Some(node) = state.nodes.get_mut(&execution_id) {
                node.permission_waiting = false;
                node.permission_reason = None;
                node.last_cursor = cursor;
            }
            state
                .attention
                .retain(|item| item.execution_id != execution_id);
        }
        EventPayload::ModelRerouted { reason, .. } => push_attention(
            &mut state,
            AttentionItem {
                execution_id,
                priority: AttentionPriority::Medium,
                reason,
                occurred_at: event.occurred_at,
                dismissed: false,
                interrupted: false,
            },
        ),
        EventPayload::TopologyAttestationRejected { reason } => push_attention(
            &mut state,
            AttentionItem {
                execution_id,
                priority: AttentionPriority::Medium,
                reason,
                occurred_at: event.occurred_at,
                dismissed: false,
                interrupted: false,
            },
        ),
        EventPayload::StreamReset { reason } => {
            state.integrity = IntegrityState::Stale;
            state.last_error = Some(reason);
        }
        EventPayload::StreamGap { reason, .. } => {
            state.integrity = IntegrityState::Gap;
            state.last_error = Some(reason);
        }
    }

    enforce_caps(&mut state);
    state
}

fn push_attention(state: &mut ObservatoryState, item: AttentionItem) {
    state.attention.retain(|existing| {
        existing.execution_id != item.execution_id || existing.reason != item.reason
    });
    state.attention.push(item);
    sort_attention(&mut state.attention);
    state.attention.truncate(MAX_ATTENTION_ITEMS);
}

fn sort_attention(attention: &mut [AttentionItem]) {
    attention.sort_by(|left, right| {
        priority_rank(left.priority)
            .cmp(&priority_rank(right.priority))
            .then_with(|| right.occurred_at.cmp(&left.occurred_at))
            .then_with(|| left.execution_id.cmp(&right.execution_id))
    });
}

fn priority_rank(priority: AttentionPriority) -> u8 {
    match priority {
        AttentionPriority::Critical => 0,
        AttentionPriority::High => 1,
        AttentionPriority::Medium => 2,
        AttentionPriority::Low => 3,
        AttentionPriority::Info => 4,
    }
}

fn enforce_caps(state: &mut ObservatoryState) {
    if state.nodes.len() > MAX_TRACKED_EXECUTIONS {
        let mut candidates: Vec<_> = state
            .nodes
            .values()
            .map(|node| {
                (
                    if node.phase.is_terminal() { 0_u8 } else { 1_u8 },
                    node.last_cursor,
                    node.execution_id.clone(),
                )
            })
            .collect();
        candidates.sort();
        let remove = state.nodes.len() - MAX_TRACKED_EXECUTIONS;
        for (_, _, execution_id) in candidates.into_iter().take(remove) {
            state.nodes.remove(&execution_id);
            state.edges.retain(|_, edge| {
                edge.parent_execution_id != execution_id && edge.child_execution_id != execution_id
            });
            state
                .attention
                .retain(|item| item.execution_id != execution_id);
        }
    }

    // The slot registry outlives evicted nodes on purpose (a re-observed
    // execution returns to its module, and later slots never shift), but it
    // must not grow without bound. Drop only entries whose execution is no
    // longer tracked, lowest slot first; next_floor_slot never decreases, so
    // pruned slots are honest permanent gaps rather than reusable positions.
    if state.floor_slots.len() > MAX_FLOOR_SLOT_ENTRIES {
        let mut absent: Vec<_> = state
            .floor_slots
            .iter()
            .filter(|(execution_id, _)| !state.nodes.contains_key(*execution_id))
            .map(|(execution_id, slot)| (*slot, execution_id.clone()))
            .collect();
        absent.sort();
        let remove = state.floor_slots.len() - MAX_FLOOR_SLOT_ENTRIES;
        for (_, execution_id) in absent.into_iter().take(remove) {
            state.floor_slots.remove(&execution_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observatory::{
        domain::{Correlation, EventKind, Producer, SnapshotNode, Topology, TruthProvenance},
        layout::build_layout,
    };

    fn admitted(cursor: u64, id: &str) -> EventEnvelope {
        EventEnvelope {
            schema_version: 1,
            cursor: cursor.to_string(),
            event_id: format!("event-{cursor}"),
            observatory_id: "obs".into(),
            daemon_instance_id: "boot".into(),
            occurred_at: cursor.to_string(),
            recorded_at: cursor.to_string(),
            kind: EventKind::ExecutionAdmitted,
            truth: TruthProvenance::HostObserved,
            producer: Producer::default(),
            topology: Topology {
                execution_id: id.into(),
                root_execution_id: id.into(),
                ..Default::default()
            },
            correlation: Correlation::default(),
            payload: EventPayload::ExecutionAdmitted {
                phase: ExecutionPhase::Admitted,
                labels: vec![format!("agent {id}")],
            },
        }
    }

    #[test]
    fn duplicate_event_is_idempotent() {
        let event = admitted(1, "a");
        let once = apply(ObservatoryState::default(), event.clone());
        let twice = apply(once.clone(), event);
        assert_eq!(once.nodes.len(), twice.nodes.len());
        assert_eq!(once.cursor, twice.cursor);
    }

    #[test]
    fn cursor_jump_marks_state_incomplete() {
        let first = apply(ObservatoryState::default(), admitted(1, "a"));
        let jumped = apply(first, admitted(3, "b"));
        assert_eq!(jumped.integrity, IntegrityState::Gap);
    }

    #[test]
    fn live_admission_spawns_the_next_cubicle_without_moving_the_first() {
        let first = apply(ObservatoryState::default(), admitted(1, "a"));
        let before = build_layout(&first);
        let first_slot = before.stations[0].clone();

        let second = apply(first, admitted(2, "b"));
        let after = build_layout(&second);
        let retained = after
            .stations
            .iter()
            .find(|station| station.execution_id == "a")
            .expect("first cubicle remains present");
        let spawned = after
            .stations
            .iter()
            .find(|station| station.execution_id == "b")
            .expect("live admission creates a cubicle");

        assert_eq!(after.cubicles.len(), 2);
        assert_eq!(retained, &first_slot);
        assert_eq!(spawned.slot, 1);
        assert_eq!(second.nodes["b"].floor_slot, 1);
        assert_eq!(second.nodes["b"].started_at, "2");
    }

    #[test]
    fn snapshot_order_becomes_stable_floor_slots_without_timestamp_sorting() {
        let snapshot = ObservatorySnapshot {
            watermark_cursor: "4".into(),
            earliest_available_cursor: "1".into(),
            observatory_id: "obs".into(),
            daemon_instance_id: "boot".into(),
            nodes: vec![
                SnapshotNode {
                    execution_id: "z-first".into(),
                    root_execution_id: "z-first".into(),
                    started_at: "2026-07-17T00:00:02+00:00".into(),
                    ..Default::default()
                },
                SnapshotNode {
                    execution_id: "a-second".into(),
                    root_execution_id: "a-second".into(),
                    started_at: "2026-07-17T00:00:01Z".into(),
                    ..Default::default()
                },
            ],
            edges: vec![],
            attention: vec![],
        };

        let state = from_snapshot_preserving_slots(snapshot, &ObservatoryState::default());

        assert_eq!(state.nodes["z-first"].floor_slot, 0);
        assert_eq!(state.nodes["a-second"].floor_slot, 1);
    }

    fn snapshot_with_nodes(observatory_id: &str, ids: &[&str]) -> ObservatorySnapshot {
        ObservatorySnapshot {
            watermark_cursor: "9".into(),
            earliest_available_cursor: "1".into(),
            observatory_id: observatory_id.into(),
            daemon_instance_id: "boot".into(),
            nodes: ids
                .iter()
                .map(|id| SnapshotNode {
                    execution_id: (*id).into(),
                    root_execution_id: (*id).into(),
                    ..Default::default()
                })
                .collect(),
            edges: vec![],
            attention: vec![],
        }
    }

    #[test]
    fn snapshot_refresh_preserves_slots_for_retained_and_evicted_executions() {
        let mut state = apply(ObservatoryState::default(), admitted(1, "a"));
        state = apply(state, admitted(2, "b"));
        assert_eq!(state.nodes["a"].floor_slot, 0);
        assert_eq!(state.nodes["b"].floor_slot, 1);

        // Refresh returns the same executions in a different response order,
        // drops "a" (e.g. replay to an earlier window), and adds "c".
        let refreshed =
            from_snapshot_preserving_slots(snapshot_with_nodes("obs", &["c", "b"]), &state);
        assert_eq!(refreshed.nodes["b"].floor_slot, 1);
        assert_eq!(refreshed.nodes["c"].floor_slot, 2);

        // "a" reappears on the next refresh and returns to its original module.
        let returned = from_snapshot_preserving_slots(
            snapshot_with_nodes("obs", &["a", "b", "c"]),
            &refreshed,
        );
        assert_eq!(returned.nodes["a"].floor_slot, 0);
        assert_eq!(returned.nodes["b"].floor_slot, 1);
        assert_eq!(returned.nodes["c"].floor_slot, 2);
    }

    #[test]
    fn observatory_authority_change_resets_the_slot_registry() {
        let state = apply(ObservatoryState::default(), admitted(1, "a"));
        let replaced = from_snapshot_preserving_slots(
            snapshot_with_nodes("different-obs", &["fresh"]),
            &state,
        );
        assert_eq!(replaced.nodes["fresh"].floor_slot, 0);
        assert_eq!(replaced.next_floor_slot, 1);
    }

    #[test]
    fn snapshot_and_live_event_share_one_state_model() {
        let snapshot = ObservatorySnapshot {
            watermark_cursor: "4".into(),
            earliest_available_cursor: "1".into(),
            observatory_id: "obs".into(),
            daemon_instance_id: "boot".into(),
            nodes: vec![],
            edges: vec![],
            attention: vec![],
        };
        let state = apply(
            from_snapshot_preserving_slots(snapshot, &ObservatoryState::default()),
            admitted(5, "next"),
        );
        assert!(state.nodes.contains_key("next"));
        assert_eq!(state.cursor, 5);
    }
}
