use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub type Cursor = u64;

pub fn parse_cursor(raw: &str) -> Cursor {
    raw.parse().unwrap_or(0)
}

fn deserialize_cursor<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum WireCursor {
        String(String),
        Number(u64),
    }
    Ok(match WireCursor::deserialize(deserializer)? {
        WireCursor::String(value) => value,
        WireCursor::Number(value) => value.to_string(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPhase {
    Admitted,
    #[default]
    Running,
    Finished,
    Error,
    Canceled,
    TimedOut,
}

impl ExecutionPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Running => "running",
            Self::Finished => "complete",
            Self::Error => "error",
            Self::Canceled => "interrupted",
            Self::TimedOut => "timed out",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Finished | Self::Error | Self::Canceled | Self::TimedOut
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TruthProvenance {
    #[default]
    HostObserved,
    ExtensionAttested,
    Derived,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Producer {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SnapshotNode {
    pub execution_id: String,
    pub root_execution_id: String,
    pub parent_execution_id: Option<String>,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub turn_id: String,
    #[serde(default)]
    pub request_id: String,
    pub phase: ExecutionPhase,
    #[serde(default)]
    pub producer: Producer,
    #[serde(default)]
    pub truth: TruthProvenance,
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub last_activity_at: String,
    #[serde(default)]
    pub labels: Vec<String>,
    pub duration_millis: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SnapshotEdge {
    pub edge_id: String,
    pub parent_execution_id: String,
    pub child_execution_id: String,
    #[serde(default)]
    pub root_execution_id: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub truth: TruthProvenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionPriority {
    Critical,
    High,
    Medium,
    Low,
    Info,
}

impl AttentionPriority {
    pub fn label(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Info => "info",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionItem {
    pub execution_id: String,
    pub priority: AttentionPriority,
    pub reason: String,
    #[serde(default)]
    pub occurred_at: String,
    #[serde(default)]
    pub dismissed: bool,
    #[serde(default)]
    pub interrupted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ObservatorySnapshot {
    #[serde(deserialize_with = "deserialize_cursor")]
    pub watermark_cursor: String,
    #[serde(deserialize_with = "deserialize_cursor")]
    pub earliest_available_cursor: String,
    pub observatory_id: String,
    pub daemon_instance_id: String,
    #[serde(default)]
    pub nodes: Vec<SnapshotNode>,
    #[serde(default)]
    pub edges: Vec<SnapshotEdge>,
    #[serde(default)]
    pub attention: Vec<AttentionItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Topology {
    #[serde(default)]
    pub execution_id: String,
    #[serde(default)]
    pub root_execution_id: String,
    pub parent_execution_id: Option<String>,
    pub edge_id: Option<String>,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub turn_id: String,
    #[serde(default)]
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Correlation {
    pub tool_call_id: Option<String>,
    pub permission_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub schema_version: u32,
    #[serde(deserialize_with = "deserialize_cursor")]
    pub cursor: String,
    pub event_id: String,
    pub observatory_id: String,
    pub daemon_instance_id: String,
    #[serde(default)]
    pub occurred_at: String,
    #[serde(default)]
    pub recorded_at: String,
    pub kind: EventKind,
    #[serde(default)]
    pub truth: TruthProvenance,
    #[serde(default)]
    pub producer: Producer,
    #[serde(default)]
    pub topology: Topology,
    #[serde(default)]
    pub correlation: Correlation,
    pub payload: EventPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    DaemonStarted,
    DaemonStopping,
    ExecutionAdmitted,
    ExecutionPhaseChanged,
    ExecutionHeartbeat,
    ExecutionFinished,
    ToolStarted,
    ToolFinished,
    PermissionWaiting,
    PermissionResolved,
    ModelRerouted,
    TopologyAttestationRejected,
    StreamReset,
    StreamGap,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum EventPayload {
    DaemonStarted {
        version: String,
    },
    DaemonStopping {
        reason: Option<String>,
    },
    ExecutionAdmitted {
        phase: ExecutionPhase,
        #[serde(default)]
        labels: Vec<String>,
    },
    ExecutionPhaseChanged {
        from_phase: ExecutionPhase,
        to_phase: ExecutionPhase,
    },
    ExecutionHeartbeat {},
    ExecutionFinished {
        phase: ExecutionPhase,
        duration_millis: u64,
        error_classification: Option<String>,
    },
    ToolStarted {
        tool_name: String,
        #[serde(default)]
        model_alias: String,
    },
    ToolFinished {
        tool_name: String,
        duration_millis: u64,
        outcome: ToolOutcome,
        byte_count: u64,
    },
    PermissionWaiting {
        reason_code: String,
    },
    PermissionResolved {
        reason_code: String,
        outcome: PermissionOutcome,
        duration_millis: u64,
    },
    ModelRerouted {
        from_model: String,
        to_model: String,
        reason: String,
    },
    TopologyAttestationRejected {
        reason: String,
    },
    StreamReset {
        reason: String,
    },
    StreamGap {
        #[serde(deserialize_with = "deserialize_cursor")]
        from_cursor: String,
        #[serde(deserialize_with = "deserialize_cursor")]
        to_cursor: String,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutcome {
    Success,
    Error,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOutcome {
    Approved,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityState {
    pub tool_name: String,
    pub started_cursor: Cursor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionState {
    pub execution_id: String,
    pub root_execution_id: String,
    pub parent_execution_id: Option<String>,
    pub session_id: String,
    pub turn_id: String,
    pub phase: ExecutionPhase,
    pub truth: TruthProvenance,
    pub labels: Vec<String>,
    pub tools: BTreeMap<String, ActivityState>,
    pub permission_waiting: bool,
    pub permission_reason: Option<String>,
    pub model_alias: Option<String>,
    pub last_cursor: Cursor,
    pub duration_millis: Option<u64>,
}

impl ExecutionState {
    pub fn label(&self) -> String {
        self.labels
            .first()
            .filter(|label| !label.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| short_id(&self.execution_id))
    }

    pub fn is_active(&self) -> bool {
        !self.phase.is_terminal()
    }
}

pub fn short_id(value: &str) -> String {
    value.chars().take(10).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IntegrityState {
    #[default]
    Live,
    Gap,
    Stale,
    Disconnected,
}

impl IntegrityState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Gap => "incomplete",
            Self::Stale => "resyncing",
            Self::Disconnected => "offline",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ObservatoryState {
    pub observatory_id: String,
    pub daemon_instance_id: String,
    pub cursor: Cursor,
    pub earliest_cursor: Cursor,
    pub nodes: BTreeMap<String, ExecutionState>,
    pub edges: BTreeMap<String, SnapshotEdge>,
    pub attention: Vec<AttentionItem>,
    pub seen_event_ids: BTreeSet<String>,
    pub integrity: IntegrityState,
    pub last_error: Option<String>,
}

impl ObservatoryState {
    pub fn active_count(&self) -> usize {
        self.nodes.values().filter(|node| node.is_active()).count()
    }

    pub fn running_count(&self) -> usize {
        self.nodes
            .values()
            .filter(|node| node.phase == ExecutionPhase::Running)
            .count()
    }

    pub fn needs_animation(&self) -> bool {
        self.integrity == IntegrityState::Live
            && self.nodes.values().any(|node| {
                node.phase == ExecutionPhase::Running
                    || node.permission_waiting
                    || !node.tools.is_empty()
            })
    }
}
