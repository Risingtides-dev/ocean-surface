//! Wire protocol for realtime native-canvas co-editing over LiveKit data
//! channels.
//!
//! The GPUI surface already joins one LiveKit room for voice/video/presence.
//! This module defines the **pure logic** that turns canvas deltas into
//! best-effort reliable data packets and back. It has *no* LiveKit import —
//! the feature-gated `surface_livekit_session` is the only place that touches
//! the transport; everything here is plain serde + base64 so it compiles and
//! tests without the `livekit` feature.
//!
//! Convergence does **not** live here. The native `CanvasLedger` already owns a
//! convergent merge (OCEAN-270: `LamportClock` + `ComponentVersion` +
//! `CanvasMergeState`, order-independent and duplicate-safe). The transport only
//! needs best-effort reliable broadcast: a dropped patch is recovered by a late
//! snapshot, and a duplicate patch is a merge no-op. See `CanvasLedger::merge_snapshot`
//! for the bulk-state path used by late joiners.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::shell::canvas::{CanvasId, SurfacePatch, SurfacePatchEnvelope};

/// LiveKit data-packet topic for canvas co-editing traffic. Only referenced
/// from the feature-gated session task, so it reads as dead code in the default
/// (no-WebRTC) build.
#[cfg_attr(not(feature = "livekit"), allow(dead_code))]
pub const CANVAS_SYNC_TOPIC: &str = "ocean.canvas.v1";
/// Max raw snapshot bytes per chunk (base64-inflated 4/3 -> ~13.3KB, under
/// LiveKit's ~15KB reliable packet budget).
pub const SNAPSHOT_CHUNK_RAW_BYTES: usize = 10_000;
/// How long a requester waits for a snapshot before retrying the next peer.
pub const SNAPSHOT_REQUEST_TIMEOUT_MS: u64 = 5_000;
/// Current wire-protocol version stamped on every message and checked on decode.
pub const PROTOCOL_VERSION: u32 = 1;

/// One canvas-co-editing message exchanged over [`CANVAS_SYNC_TOPIC`].
///
/// Internally tagged on `"kind"` with `snake_case` discriminants so the wire
/// stays human-readable and additive. `v` is always
/// [`PROTOCOL_VERSION`]; [`decode`] rejects any other value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
// The `Patch` variant carries a full envelope (~452 B), much larger than the
// others, but a `CanvasSyncMessage` is built and immediately serialized to
// bytes — never held in volume — so boxing the field isn't worth the churn.
#[allow(clippy::large_enum_variant)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CanvasSyncMessage {
    /// One applied patch, re-broadcast verbatim with its stamped version.
    Patch {
        v: u32,
        envelope: SurfacePatchEnvelope,
    },
    /// Late-joiner asks one specific peer for the full state of one canvas.
    SnapshotRequest {
        v: u32,
        canvas_id: CanvasId,
        request_id: String,
    },
    /// One chunk of a serialized `CanvasLedger` (`serde_json::to_vec`, base64
    /// chunks). Reassembled by [`SnapshotReassembly`] keyed on `request_id`.
    Snapshot {
        v: u32,
        canvas_id: CanvasId,
        request_id: String,
        chunk_index: u32,
        chunk_count: u32,
        data_b64: String,
    },
}

/// Serialize a message to compact JSON bytes. Infallible: every variant is
/// serde-clean, so a failure is a programmer bug (panic, never a dropped edit).
pub fn encode(msg: &CanvasSyncMessage) -> Vec<u8> {
    serde_json::to_vec(msg).expect("CanvasSyncMessage is serde-clean")
}

/// Deserialize a message and enforce the protocol version. Any malformed bytes
/// or `v != PROTOCOL_VERSION` yields an `Err`; the caller logs a status line and
/// drops the packet (a dropped patch is recovered by a later snapshot).
pub fn decode(bytes: &[u8]) -> Result<CanvasSyncMessage, String> {
    let msg: CanvasSyncMessage = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let v = match &msg {
        CanvasSyncMessage::Patch { v, .. }
        | CanvasSyncMessage::SnapshotRequest { v, .. }
        | CanvasSyncMessage::Snapshot { v, .. } => *v,
    };
    if v != PROTOCOL_VERSION {
        return Err(format!(
            "unsupported canvas sync protocol version {v} (expected {PROTOCOL_VERSION})"
        ));
    }
    Ok(msg)
}

/// Whether a patch is broadcast in sync v1. True ONLY for the six
/// versioned/component-or-edge ops: `UpsertComponent | MoveComponent |
/// ResizeComponent | DeleteComponent | Connect | Disconnect`. `Select` /
/// `SetViewport` / `Focus` are per-user view state; `Layout` / `Group` are
/// multi-component bulk ops with no single-component version stamp — both are
/// excluded from sync v1 (not broadcast, dropped on receive).
pub fn sync_eligible(patch: &SurfacePatch) -> bool {
    matches!(
        patch,
        SurfacePatch::UpsertComponent { .. }
            | SurfacePatch::MoveComponent { .. }
            | SurfacePatch::ResizeComponent { .. }
            | SurfacePatch::DeleteComponent { .. }
            | SurfacePatch::Connect { .. }
            | SurfacePatch::Disconnect { .. }
    )
}

/// Split a serialized `CanvasLedger` into chunked [`CanvasSyncMessage::Snapshot`]
/// packets. Empty input yields a single empty chunk (`chunk_count: 1`) so a
/// requester's pending state always resolves. Each chunk's `data_b64` is the
/// base64 of one [`SNAPSHOT_CHUNK_RAW_BYTES`] slice of `ledger_json`.
pub fn chunk_snapshot(
    canvas_id: &CanvasId,
    request_id: &str,
    ledger_json: &[u8],
) -> Vec<CanvasSyncMessage> {
    let engine = base64::engine::general_purpose::STANDARD;
    let total = ledger_json.len();
    let chunk_count = total.div_ceil(SNAPSHOT_CHUNK_RAW_BYTES).max(1) as u32;
    let mut out = Vec::with_capacity(chunk_count as usize);
    for chunk_index in 0..chunk_count {
        let start = chunk_index as usize * SNAPSHOT_CHUNK_RAW_BYTES;
        let end = (start + SNAPSHOT_CHUNK_RAW_BYTES).min(total);
        let data_b64 = engine.encode(&ledger_json[start..end]);
        out.push(CanvasSyncMessage::Snapshot {
            v: PROTOCOL_VERSION,
            canvas_id: canvas_id.clone(),
            request_id: request_id.to_string(),
            chunk_index,
            chunk_count,
            data_b64,
        });
    }
    out
}

/// Reassembles a chunked snapshot. One instance per `request_id`, keyed in the
/// shell. Insert chunks as they arrive (any order); [`SnapshotReassembly::try_assemble`]
/// stays `None` until every chunk is present.
#[derive(Debug, Clone)]
pub struct SnapshotReassembly {
    chunks: Vec<Option<String>>,
}

impl SnapshotReassembly {
    /// A fresh reassembly expecting `chunk_count` chunks.
    pub fn new(chunk_count: u32) -> Self {
        Self {
            chunks: vec![None; chunk_count as usize],
        }
    }

    /// Record one chunk. Out-of-range `chunk_index` (>= `chunk_count`) is
    /// rejected silently — a malformed/unsolicited chunk must not corrupt the
    /// in-flight snapshot.
    pub fn insert(&mut self, chunk_index: u32, data_b64: String) {
        let idx = chunk_index as usize;
        if idx < self.chunks.len() {
            self.chunks[idx] = Some(data_b64);
        }
    }

    /// `None` until every chunk has arrived. Once complete, decodes each chunk's
    /// base64 and concatenates them in index order; a decode failure yields
    /// `Some(Err(..))` (the requester clears pending and the timeout path
    /// retries the next peer).
    pub fn try_assemble(&self) -> Option<Result<Vec<u8>, String>> {
        if self.chunks.iter().any(|c| c.is_none()) {
            return None;
        }
        let engine = base64::engine::general_purpose::STANDARD;
        let mut out = Vec::new();
        for slot in &self.chunks {
            let data_b64 = slot.as_ref().expect("all chunks present");
            match engine.decode(data_b64) {
                Ok(bytes) => out.extend_from_slice(&bytes),
                Err(e) => return Some(Err(e.to_string())),
            }
        }
        Some(Ok(out))
    }
}

/// Pick the next peer to ask for a snapshot: the lexicographically smallest
/// remote identity not already tried. Deterministic across replicas and avoids
/// election chatter — every requester converges on the same peer ordering.
pub fn choose_snapshot_peer<'a>(
    remote_identities: impl Iterator<Item = &'a str>,
    already_tried: &[String],
) -> Option<String> {
    let mut candidates: Vec<&str> = remote_identities.collect();
    candidates.sort_unstable();
    candidates
        .into_iter()
        .find(|id| !already_tried.iter().any(|t| t == *id))
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::canvas::{
        ActorId, ActorRef, CanvasComponentPatch, CanvasEdgePatch, ComponentId, ComponentVersion,
        EdgeId, Endpoint, FocusTarget, LayoutStrategy, LayoutTarget, PatchId, SurfaceId,
        Viewport,
    };
    use serde_json::json;

    fn versioned_delete_env() -> SurfacePatchEnvelope {
        SurfacePatchEnvelope {
            patch_id: PatchId::new("p:1"),
            session_id: "s".to_string(),
            surface_id: SurfaceId::new("gpui:local"),
            canvas_id: CanvasId::new("canvas:main"),
            actor: ActorRef::human(Some("alice".to_string())),
            created_at_ms: 10,
            patch: SurfacePatch::DeleteComponent {
                component_id: ComponentId::new("c1"),
            },
            version: Some(ComponentVersion::new(2, ActorId::new("human:alice"))),
        }
    }

    #[test]
    fn patch_message_pinned_json() {
        // Drift guard: pins the exact compact JSON of a Patch message carrying a
        // versioned envelope. Any serde attribute change (tag rename, field
        // reorder on ComponentVersion, ActorId transparency) breaks this loudly.
        let msg = CanvasSyncMessage::Patch {
            v: 1,
            envelope: versioned_delete_env(),
        };
        let json = String::from_utf8(encode(&msg)).unwrap();
        let expected = concat!(
            r#"{"kind":"patch","v":1,"envelope":{"patch_id":"p:1","#,
            r#""session_id":"s","surface_id":"gpui:local","canvas_id":"canvas:main","#,
            r#""actor":{"kind":"human","id":"alice"},"created_at_ms":10,"#,
            r#""patch":{"op":"delete_component","component_id":"c1"},"#,
            r#""version":{"rev":2,"actor":"human:alice"}}}"#,
        );
        assert_eq!(json, expected);
    }

    #[test]
    fn patch_roundtrip_preserves_envelope_and_version() {
        let msg = CanvasSyncMessage::Patch {
            v: 1,
            envelope: versioned_delete_env(),
        };
        let back = decode(&encode(&msg)).expect("roundtrip decodes");
        assert_eq!(back, msg);
    }

    #[test]
    fn snapshot_request_roundtrip() {
        let msg = CanvasSyncMessage::SnapshotRequest {
            v: 1,
            canvas_id: CanvasId::new("canvas:main"),
            request_id: "req-42".to_string(),
        };
        let back = decode(&encode(&msg)).expect("roundtrip decodes");
        assert_eq!(back, msg);
    }

    #[test]
    fn snapshot_chunk_roundtrip_decodes_to_message() {
        let ledger_json = br#"{"canvas_id":"canvas:main"}"#;
        let chunks = chunk_snapshot(&CanvasId::new("canvas:main"), "req-1", ledger_json);
        assert_eq!(chunks.len(), 1, "small snapshot is a single chunk");
        let back = decode(&encode(&chunks[0])).expect("roundtrip decodes");
        assert_eq!(back, chunks[0]);
    }

    #[test]
    fn decode_rejects_wrong_protocol_version() {
        // Hand-build a v=2 message so decode's version gate is exercised without
        // a real producer.
        let raw = r#"{"kind":"snapshot_request","v":2,"canvas_id":"c","request_id":"r"}"#;
        let err = decode(raw.as_bytes()).unwrap_err();
        assert!(err.contains("version 2"), "unexpected error: {err}");
    }

    #[test]
    fn decode_rejects_malformed_bytes() {
        let err = decode(b"not json").unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn sync_eligible_truth_table() {
        use SurfacePatch::*;
        let eligible_upsert = UpsertComponent {
            component: CanvasComponentPatch {
                id: ComponentId::new("c"),
                kind: "card".to_string(),
                rect: None,
                z_index: None,
                content: json!({}),
                metadata: json!({}),
            },
        };
        let eligible_connect = Connect {
            edge: CanvasEdgePatch {
                id: EdgeId::new("e"),
                from: Endpoint {
                    component_id: ComponentId::new("a"),
                    port: None,
                },
                to: Endpoint {
                    component_id: ComponentId::new("b"),
                    port: None,
                },
                kind: None,
                label: None,
                metadata: json!({}),
            },
        };
        // The six eligible ops.
        assert!(sync_eligible(&eligible_upsert));
        assert!(sync_eligible(&eligible_connect));
        assert!(sync_eligible(&MoveComponent {
            component_id: ComponentId::new("c"),
            x: 1.0,
            y: 2.0,
        }));
        assert!(sync_eligible(&ResizeComponent {
            component_id: ComponentId::new("c"),
            width: 3.0,
            height: 4.0,
        }));
        assert!(sync_eligible(&DeleteComponent {
            component_id: ComponentId::new("c"),
        }));
        assert!(sync_eligible(&Disconnect {
            edge_id: EdgeId::new("e"),
        }));
        // The five excluded ops.
        assert!(!sync_eligible(&Select {
            ids: vec![ComponentId::new("c")],
        }));
        assert!(!sync_eligible(&SetViewport {
            viewport: Viewport::default(),
        }));
        assert!(!sync_eligible(&Focus {
            target: FocusTarget::Canvas,
        }));
        assert!(!sync_eligible(&Layout {
            target: LayoutTarget::Canvas,
            strategy: LayoutStrategy::Grid,
        }));
        assert!(!sync_eligible(&Group {
            frame_id: ComponentId::new("f"),
            children: vec![ComponentId::new("c")],
        }));
    }

    #[test]
    fn chunk_and_reassemble_roundtrip_at_boundaries() {
        for &size in &[0usize, 1, 2, SNAPSHOT_CHUNK_RAW_BYTES - 1, SNAPSHOT_CHUNK_RAW_BYTES, SNAPSHOT_CHUNK_RAW_BYTES + 1, SNAPSHOT_CHUNK_RAW_BYTES * 2, SNAPSHOT_CHUNK_RAW_BYTES * 2 + 1] {
            let payload: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            let canvas_id = CanvasId::new("canvas:main");
            let chunks = chunk_snapshot(&canvas_id, "req", &payload);
            let chunk_count = chunks.len() as u32;
            assert_eq!(chunk_count, chunk_count_expected(size));
            let mut reassembly = SnapshotReassembly::new(chunk_count);
            for msg in &chunks {
                let CanvasSyncMessage::Snapshot {
                    chunk_index,
                    chunk_count: cc,
                    data_b64,
                    ..
                } = msg
                else {
                    panic!("expected Snapshot variant");
                };
                assert_eq!(*cc, chunk_count, "chunk_count consistent across chunks");
                reassembly.insert(*chunk_index, data_b64.clone());
            }
            let assembled = reassembly
                .try_assemble()
                .expect("all chunks present")
                .expect("base64 decodes");
            assert_eq!(assembled, payload, "size {size} reassembles identically");
        }
    }

    fn chunk_count_expected(size: usize) -> u32 {
        size.div_ceil(SNAPSHOT_CHUNK_RAW_BYTES).max(1) as u32
    }

    #[test]
    fn reassemble_accepts_out_of_order_inserts() {
        let payload: Vec<u8> = (0..(SNAPSHOT_CHUNK_RAW_BYTES * 2 + 17))
            .map(|i| (i % 251) as u8)
            .collect();
        let chunks = chunk_snapshot(&CanvasId::new("c"), "r", &payload);
        let count = chunks.len() as u32;
        let mut reassembly = SnapshotReassembly::new(count);
        // Insert in reverse order.
        for msg in chunks.iter().rev() {
            let CanvasSyncMessage::Snapshot {
                chunk_index,
                data_b64,
                ..
            } = msg
            else {
                unreachable!();
            };
            reassembly.insert(*chunk_index, data_b64.clone());
        }
        let assembled = reassembly.try_assemble().unwrap().unwrap();
        assert_eq!(assembled, payload);
    }

    #[test]
    fn reassembly_is_partial_until_all_chunks_present() {
        let mut reassembly = SnapshotReassembly::new(3);
        assert!(reassembly.try_assemble().is_none());
        reassembly.insert(0, "YQ==".to_string()); // "a"
        assert!(reassembly.try_assemble().is_none(), "still missing chunks");
        reassembly.insert(2, "Yg==".to_string()); // "b"
        assert!(reassembly.try_assemble().is_none(), "missing middle chunk");
        reassembly.insert(1, "Yw==".to_string()); // "c"
        let assembled = reassembly.try_assemble().expect("all chunks present").expect("base64 decodes");
        // Assembled in index order: chunk 0 ("a") + chunk 1 ("c") + chunk 2 ("b").
        assert_eq!(assembled, b"acb");
    }
    #[test]
    fn reassembly_reports_base64_failure() {
        let mut reassembly = SnapshotReassembly::new(1);
        reassembly.insert(0, "!!!not base64!!!".to_string());
        let res = reassembly.try_assemble().expect("all chunks present");
        assert!(res.is_err(), "invalid base64 yields Err");
    }

    #[test]
    fn reassembly_rejects_out_of_range_chunk_index() {
        let mut reassembly = SnapshotReassembly::new(2);
        reassembly.insert(5, "YQ==".to_string()); // ignored
        assert!(reassembly.try_assemble().is_none(), "rogue chunk ignored");
        reassembly.insert(0, "YQ==".to_string());
        reassembly.insert(1, "Yg==".to_string());
        assert_eq!(reassembly.try_assemble().unwrap().unwrap(), b"ab");
    }

    #[test]
    fn choose_peer_picks_lexicographically_smallest_untried() {
        let peers = ["zoe", "alice", "mike"];
        assert_eq!(
            choose_snapshot_peer(peers.iter().copied(), &[]),
            Some("alice".to_string())
        );
        let mut tried = vec!["alice".to_string()];
        assert_eq!(
            choose_snapshot_peer(peers.iter().copied(), &tried),
            Some("mike".to_string())
        );
        tried.push("mike".to_string());
        assert_eq!(
            choose_snapshot_peer(peers.iter().copied(), &tried),
            Some("zoe".to_string())
        );
        tried.push("zoe".to_string());
        assert_eq!(
            choose_snapshot_peer(peers.iter().copied(), &tried),
            None,
            "all peers exhausted"
        );
    }

    #[test]
    fn choose_peer_empty_when_no_remotes() {
        assert_eq!(choose_snapshot_peer(std::iter::empty(), &[]), None);
    }
}
