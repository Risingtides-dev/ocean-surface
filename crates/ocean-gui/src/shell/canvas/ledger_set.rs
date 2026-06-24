//! A set of [`CanvasLedger`]s keyed by `canvas_id` (OCEAN-257).
//!
//! The native surface used to hold exactly one [`CanvasLedger`]. But every
//! [`SurfacePatchEnvelope`](super::patch::SurfacePatchEnvelope) carries the
//! `canvas_id` it targets, and an agent can legitimately maintain several canvases
//! at once — a storyboard *and* a workflow board, say. With a single ledger, a
//! patch for a second canvas would discard the first (the apply path keyed on a
//! lone `canvas_id` match), collapsing every canvas into whichever was patched
//! last.
//!
//! [`CanvasLedgerSet`] fixes that: it stores one ledger per `canvas_id` and tracks
//! which one is *active* (the canvas the operator is currently viewing). Patches
//! route to the ledger named in their envelope; switching the active canvas just
//! changes which ledger the renderer is pointed at — no canvas is lost.
//!
//! The set itself owns no placement / apply logic; it is a keyed container plus an
//! active pointer. Callers pull a ledger out with [`take`](CanvasLedgerSet::take),
//! run the existing persistence-aware apply over it, and put the result back with
//! [`put`](CanvasLedgerSet::put). This keeps the single-canvas
//! [`CanvasLedger`] (and every consumer of it — renderer, hit-test, persistence,
//! compact-context) completely unchanged; multi-canvas is layered strictly on top.
//!
//! # Bounded growth (OCEAN-278)
//!
//! OCEAN-257 left the set unbounded: a long session in which the agent keeps
//! naming fresh canvases would accrete a ledger per id forever, and those ledgers
//! lingered across session switches (the operator loading a different session
//! still saw the prior session's canvases). This module bounds the set two ways:
//!
//! - **Cap + LRU.** The set keeps at most [`MAX_CANVASES`] canvases. When a `put`
//!   would push it past the cap, the **least-recently-touched** canvas is evicted.
//!   "Touched" = inserted, replaced, or made active; the ledger's `IndexMap` order
//!   is maintained as an MRU list (most-recently-touched at the back), so eviction
//!   pops from the front. The **active** canvas is never evicted — it's the one the
//!   operator is looking at — so the cap is a soft floor of "active + up to N−1
//!   others".
//! - **Per-session clear.** [`clear`](Self::clear) drops every canvas and the
//!   active pointer; the shell calls it on a session switch so canvases from a
//!   prior session don't bleed into the next (the exact gap OCEAN-257 flagged).
//!
//! # The operator-selected pin (OCEAN-380)
//!
//! "Active" alone is not enough to protect the operator's choice. Every incoming
//! agent patch routes through the shell as `put` + [`set_active`](Self::set_active)
//! on the *patched* canvas, so the canvas the agent just drew to becomes active —
//! that's deliberate (the agent's work comes to the foreground). But it means the
//! canvas the **operator** explicitly selected (a tab-strip click) loses `active`
//! the instant the agent touches a different canvas. If the operator then idles
//! while the agent keeps minting/patching other canvases, the operator's selection
//! is just another LRU candidate and can be evicted out from under them; on
//! re-selection it reloads from disk and may diverge from server-queued patches.
//!
//! So the set tracks a second pointer — the **operator-selected** canvas
//! ([`select`](Self::select)) — and exempts it from eviction alongside `active`.
//! The shell sets it only on an explicit operator selection, never on an agent
//! patch, so the operator's chosen canvas survives any amount of background agent
//! churn. With both pinned, the cap is a soft floor of "active + selected + up to
//! N−2 others".

use indexmap::IndexMap;

use super::ledger::CanvasLedger;
use super::patch::CanvasId;

/// Maximum number of canvases the set retains. A real session juggles a handful of
/// canvases at once (a storyboard, a workflow board, a scratch board…); past this
/// the least-recently-touched ones are evicted (LRU) so a session that keeps
/// minting new canvas ids can't grow the set without bound. Sized generously so
/// normal multi-canvas work never trips it — eviction is a backstop, not a budget
/// the operator should feel.
pub const MAX_CANVASES: usize = 16;

/// All canvases for the active session, keyed by `canvas_id`, with a pointer to
/// the one currently shown.
///
/// Order is maintained as a **most-recently-touched** list: the canvas touched
/// last (inserted, replaced, or made active) sits at the back, the
/// least-recently-touched at the front. A tab strip built from
/// [`canvas_ids`](Self::canvas_ids) is still stable frame-to-frame for an
/// unchanging set of canvases — order only shifts when a canvas is actually
/// touched — and the front is exactly the LRU eviction candidate when the set is
/// over [`MAX_CANVASES`].
#[derive(Debug, Clone, Default)]
pub struct CanvasLedgerSet {
    canvases: IndexMap<CanvasId, CanvasLedger>,
    /// The canvas the renderer is pointed at. Stays `None` until the first canvas
    /// appears; thereafter it always names a present canvas (see [`set_active`]).
    ///
    /// [`set_active`]: Self::set_active
    active: Option<CanvasId>,
    /// The canvas the **operator** explicitly selected (OCEAN-380), distinct from
    /// `active`. Agent patches reassign `active` to whatever they touch, so this is
    /// the only pointer that reliably tracks the operator's own choice. It is
    /// exempted from eviction alongside `active` so background agent churn can't
    /// evict the canvas the operator is sitting on. `None` until the operator picks
    /// one; cleared if its canvas is removed by [`clear`](Self::clear).
    selected: Option<CanvasId>,
}

impl CanvasLedgerSet {
    /// An empty set (no canvases, no active pointer).
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether no canvas exists yet.
    pub fn is_empty(&self) -> bool {
        self.canvases.is_empty()
    }

    /// How many distinct canvases are present.
    pub fn len(&self) -> usize {
        self.canvases.len()
    }

    /// The canvas ids present, in stable first-seen tab order.
    pub fn canvas_ids(&self) -> Vec<CanvasId> {
        self.canvases.keys().cloned().collect()
    }

    /// The active canvas id, if any canvas exists.
    pub fn active_id(&self) -> Option<&CanvasId> {
        self.active.as_ref()
    }

    /// Borrow the active canvas's ledger, if one is active.
    pub fn active(&self) -> Option<&CanvasLedger> {
        self.active.as_ref().and_then(|id| self.canvases.get(id))
    }

    /// Borrow a specific canvas's ledger.
    pub fn get(&self, canvas_id: &CanvasId) -> Option<&CanvasLedger> {
        self.canvases.get(canvas_id)
    }

    /// Remove and return the ledger for `canvas_id`, if present. Callers run the
    /// persistence-aware apply over the returned (or freshly created) ledger and
    /// hand the result back via [`put`](Self::put). Removing rather than borrowing
    /// keeps the apply step working on an owned value (it consumes and returns the
    /// ledger), avoiding a clone of what can be a large component map.
    pub fn take(&mut self, canvas_id: &CanvasId) -> Option<CanvasLedger> {
        self.canvases.shift_remove(canvas_id)
    }

    /// Insert or replace a canvas's ledger, keyed on the ledger's own `canvas_id`.
    /// If this is the first canvas, it also becomes the active one.
    ///
    /// The touched canvas moves to the **most-recently-used** end, and the set is
    /// then trimmed to [`MAX_CANVASES`] by evicting least-recently-used canvases
    /// (never the active one) — so a session that keeps naming fresh canvases
    /// stays bounded (OCEAN-278).
    pub fn put(&mut self, ledger: CanvasLedger) {
        let id = ledger.canvas_id.clone();
        // `IndexMap::insert` keeps an existing key's position; we want the touched
        // canvas at the MRU end, so re-seat it explicitly after writing.
        self.canvases.insert(id.clone(), ledger);
        self.touch(&id);
        if self.active.is_none() {
            self.active = Some(id);
        }
        self.evict_over_cap();
    }

    /// Point the renderer at `canvas_id`. No-op if that canvas isn't present, so
    /// the active pointer never dangles. Making a canvas active also marks it
    /// most-recently-used, so the canvas the operator is looking at is never the
    /// LRU eviction candidate.
    pub fn set_active(&mut self, canvas_id: &CanvasId) {
        if self.canvases.contains_key(canvas_id) {
            self.active = Some(canvas_id.clone());
            self.touch(canvas_id);
        }
    }

    /// Record the canvas the **operator** explicitly selected (OCEAN-380), and make
    /// it active. Unlike [`set_active`](Self::set_active) — which the patch path
    /// also calls for the agent-patched canvas — this is invoked *only* on an
    /// operator action (a tab-strip click), so the `selected` pointer reliably
    /// tracks the operator's own choice even as agent patches keep reassigning
    /// `active`. The selected canvas is then exempt from eviction. No-op if the
    /// canvas isn't present, so the pointer never dangles.
    pub fn select(&mut self, canvas_id: &CanvasId) {
        if self.canvases.contains_key(canvas_id) {
            self.selected = Some(canvas_id.clone());
            self.set_active(canvas_id);
        }
    }

    /// The operator-selected canvas id, if the operator has selected one.
    pub fn selected_id(&self) -> Option<&CanvasId> {
        self.selected.as_ref()
    }

    /// Drop every canvas and both pointers (OCEAN-278). The shell calls this on a
    /// session switch so canvases from a prior session don't linger into the next —
    /// the gap OCEAN-257 flagged. After this the set is exactly as it was at
    /// construction.
    pub fn clear(&mut self) {
        self.canvases.clear();
        self.active = None;
        self.selected = None;
    }

    /// Move `canvas_id` to the most-recently-used end of the order. Caller ensures
    /// the key is present.
    fn touch(&mut self, canvas_id: &CanvasId) {
        if let Some(index) = self.canvases.get_index_of(canvas_id) {
            let last = self.canvases.len() - 1;
            if index != last {
                self.canvases.move_index(index, last);
            }
        }
    }

    /// Evict least-recently-used canvases until at most [`MAX_CANVASES`] remain.
    /// The active canvas and the operator-selected canvas (OCEAN-380) are never
    /// evicted: candidates are scanned from the LRU (front) end and both pinned ids
    /// are skipped, so over-cap pressure falls on the stalest canvas that is neither
    /// shown nor operator-selected. With those always kept, the set can momentarily
    /// hold the cap exactly; it never exceeds it after a `put`.
    fn evict_over_cap(&mut self) {
        while self.canvases.len() > MAX_CANVASES {
            // Find the least-recently-used canvas that is neither active nor
            // operator-selected. Scanning from the front (LRU) yields the stalest
            // eligible victim first.
            let victim = self
                .canvases
                .keys()
                .find(|id| Some(*id) != self.active.as_ref() && Some(*id) != self.selected.as_ref())
                .cloned();
            match victim {
                Some(id) => {
                    self.canvases.shift_remove(&id);
                }
                // Every remaining canvas is pinned (active and/or selected), e.g. a
                // degenerate cap: nothing safe to evict, so stop rather than drop
                // what's on screen or what the operator chose.
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::canvas::ledger::CanvasMode;
    use crate::shell::canvas::patch::{
        ActorRef, CanvasComponentPatch, ComponentId, Rect, SurfacePatch,
    };
    use serde_json::{Value, json};

    fn ledger_with(canvas: &str, component: &str) -> CanvasLedger {
        let mut l = CanvasLedger::new(canvas, "sess-1", CanvasMode::Freeform);
        l.apply_patch(
            SurfacePatch::UpsertComponent {
                component: CanvasComponentPatch {
                    id: ComponentId::new(component),
                    kind: "card".to_string(),
                    rect: Some(Rect::new(0.0, 0.0, 10.0, 10.0)),
                    z_index: None,
                    content: json!({ "title": component }),
                    metadata: Value::Null,
                },
            },
            ActorRef::system(),
            0,
        );
        l
    }

    #[test]
    fn distinct_canvas_ids_coexist_as_separate_ledgers() {
        let mut set = CanvasLedgerSet::new();
        set.put(ledger_with("canvas:storyboard", "frame-1"));
        set.put(ledger_with("canvas:workflow", "node-1"));

        assert_eq!(set.len(), 2, "two canvases must coexist");

        let story = set.get(&CanvasId::new("canvas:storyboard")).unwrap();
        assert!(story.component(&ComponentId::new("frame-1")).is_some());
        assert!(
            story.component(&ComponentId::new("node-1")).is_none(),
            "the workflow node must not bleed into the storyboard canvas",
        );

        let flow = set.get(&CanvasId::new("canvas:workflow")).unwrap();
        assert!(flow.component(&ComponentId::new("node-1")).is_some());
        assert!(flow.component(&ComponentId::new("frame-1")).is_none());
    }

    #[test]
    fn first_canvas_becomes_active_and_set_active_switches() {
        let mut set = CanvasLedgerSet::new();
        assert!(set.active().is_none());

        set.put(ledger_with("canvas:main", "a"));
        assert_eq!(
            set.active_id(),
            Some(&CanvasId::new("canvas:main")),
            "first canvas is active",
        );

        set.put(ledger_with("canvas:secondary", "b"));
        // Adding a second canvas does NOT steal focus.
        assert_eq!(set.active_id(), Some(&CanvasId::new("canvas:main")));

        set.set_active(&CanvasId::new("canvas:secondary"));
        assert_eq!(set.active_id(), Some(&CanvasId::new("canvas:secondary")));

        // Switching to an absent canvas is a no-op (pointer stays valid).
        set.set_active(&CanvasId::new("canvas:ghost"));
        assert_eq!(set.active_id(), Some(&CanvasId::new("canvas:secondary")));
    }

    // ----- Bounded growth + eviction (OCEAN-278) ---------------------------

    /// A distinct canvas id for the Nth synthetic canvas.
    fn nth_canvas(n: usize) -> String {
        format!("canvas:{n}")
    }

    #[test]
    fn put_keeps_set_within_the_cap() {
        let mut set = CanvasLedgerSet::new();
        // Insert well past the cap; the set must never exceed MAX_CANVASES.
        for n in 0..(MAX_CANVASES + 8) {
            set.put(ledger_with(&nth_canvas(n), "c"));
            assert!(
                set.len() <= MAX_CANVASES,
                "set grew past the cap at insert {n}: {} > {MAX_CANVASES}",
                set.len(),
            );
        }
        assert_eq!(set.len(), MAX_CANVASES, "set settles exactly at the cap");
    }

    #[test]
    fn over_cap_evicts_the_least_recently_used_canvas() {
        let mut set = CanvasLedgerSet::new();
        // Fill to the cap: canvas:0 (active, first) .. canvas:{cap-1}.
        for n in 0..MAX_CANVASES {
            set.put(ledger_with(&nth_canvas(n), "c"));
        }
        // canvas:0 is active; canvas:1 is now the least-recently-used non-active.
        assert_eq!(set.active_id(), Some(&CanvasId::new(nth_canvas(0))));

        // One more canvas tips us over the cap → the LRU (canvas:1) is evicted.
        set.put(ledger_with(&nth_canvas(MAX_CANVASES), "c"));

        assert_eq!(set.len(), MAX_CANVASES);
        assert!(
            set.get(&CanvasId::new(nth_canvas(1))).is_none(),
            "the least-recently-used canvas (canvas:1) was evicted",
        );
        assert!(
            set.get(&CanvasId::new(nth_canvas(MAX_CANVASES))).is_some(),
            "the just-inserted canvas is retained",
        );
    }

    #[test]
    fn active_canvas_is_never_evicted_even_when_it_is_the_lru() {
        let mut set = CanvasLedgerSet::new();
        // canvas:0 is the first (active) canvas and we never touch it again, so by
        // recency it is the least-recently-used — yet it must survive eviction.
        for n in 0..MAX_CANVASES {
            set.put(ledger_with(&nth_canvas(n), "c"));
        }
        assert_eq!(set.active_id(), Some(&CanvasId::new(nth_canvas(0))));

        // Push many more canvases; the active canvas:0 must remain throughout.
        for n in MAX_CANVASES..(MAX_CANVASES * 2) {
            set.put(ledger_with(&nth_canvas(n), "c"));
            assert!(
                set.get(&CanvasId::new(nth_canvas(0))).is_some(),
                "active canvas:0 must never be evicted (insert {n})",
            );
        }
        assert_eq!(set.active_id(), Some(&CanvasId::new(nth_canvas(0))));
    }

    #[test]
    fn set_active_refreshes_recency_so_the_active_canvas_outlives_older_ones() {
        let mut set = CanvasLedgerSet::new();
        for n in 0..MAX_CANVASES {
            set.put(ledger_with(&nth_canvas(n), "c"));
        }
        // Make an old canvas active: canvas:2 was stale, now it's both active and
        // most-recently-used, so it must not be the eviction victim.
        set.set_active(&CanvasId::new(nth_canvas(2)));

        set.put(ledger_with(&nth_canvas(MAX_CANVASES), "c"));
        assert!(
            set.get(&CanvasId::new(nth_canvas(2))).is_some(),
            "the freshly-activated canvas:2 survives — recency was refreshed",
        );
        // The evicted one is the stalest non-active: canvas:0 (the original first,
        // never re-touched, and no longer active).
        assert!(
            set.get(&CanvasId::new(nth_canvas(0))).is_none(),
            "the stalest non-active canvas (canvas:0) was evicted instead",
        );
    }

    #[test]
    fn touching_a_canvas_via_put_spares_it_from_eviction() {
        let mut set = CanvasLedgerSet::new();
        for n in 0..MAX_CANVASES {
            set.put(ledger_with(&nth_canvas(n), "c"));
        }
        // canvas:0 is active (spared anyway). Re-put canvas:1 so it's no longer the
        // LRU; the next-stalest non-active (canvas:2) should be evicted instead.
        set.put(ledger_with(&nth_canvas(1), "c"));
        set.put(ledger_with(&nth_canvas(MAX_CANVASES), "c"));

        assert!(
            set.get(&CanvasId::new(nth_canvas(1))).is_some(),
            "canvas:1 was touched, so it is no longer the LRU victim",
        );
        assert!(
            set.get(&CanvasId::new(nth_canvas(2))).is_none(),
            "canvas:2 (now the stalest non-active) was evicted",
        );
    }

    // ----- Operator-selected pin survives agent churn (OCEAN-380) ----------

    #[test]
    fn operator_selected_canvas_survives_agent_patching_other_canvases() {
        // The exact OCEAN-380 scenario: the operator selects a canvas, then idles
        // while the agent keeps patching *other* canvases. Each agent patch routes
        // as put + set_active on the patched canvas (so `active` is reassigned away
        // from the operator's choice), and there are far more than the cap of them.
        // The operator-selected canvas must NOT be evicted.
        let mut set = CanvasLedgerSet::new();

        // Seed enough canvases that the selected one is genuinely the stalest by
        // recency once the agent moves on — and well past the cap (33+ total).
        for n in 0..(MAX_CANVASES + 1) {
            set.put(ledger_with(&nth_canvas(n), "c"));
        }

        // Operator explicitly selects a NON-default canvas (not canvas:0, the first
        // / originally-active one). This is the tab-strip-click path.
        let chosen = CanvasId::new(nth_canvas(3));
        set.select(&chosen);
        assert_eq!(set.selected_id(), Some(&chosen));
        assert_eq!(
            set.active_id(),
            Some(&chosen),
            "select also makes it active"
        );

        // Agent now patches many *other* canvases — far past the eviction window.
        // Each `set_active` hijacks `active` to the patched canvas, so the only
        // thing protecting the operator's choice is the `selected` pin.
        for n in (MAX_CANVASES + 1)..(MAX_CANVASES * 3) {
            let other = nth_canvas(n);
            set.put(ledger_with(&other, "c"));
            set.set_active(&CanvasId::new(other)); // mirror the shell's patch path
            assert!(
                set.get(&chosen).is_some(),
                "operator-selected canvas:3 must never be evicted (agent patch {n})",
            );
        }

        // Still present after all the churn, still the operator's selection, and
        // can be re-activated without a disk reload (it's in memory).
        assert!(set.get(&chosen).is_some());
        assert_eq!(set.selected_id(), Some(&chosen));
        assert_eq!(set.len(), MAX_CANVASES);
    }

    #[test]
    fn both_active_and_selected_are_exempt_when_they_differ() {
        // After an agent patch, `active` (agent's canvas) and `selected` (operator's
        // canvas) point at different canvases. Both must survive eviction.
        let mut set = CanvasLedgerSet::new();
        for n in 0..MAX_CANVASES {
            set.put(ledger_with(&nth_canvas(n), "c"));
        }
        let operator_pick = CanvasId::new(nth_canvas(2));
        set.select(&operator_pick);

        // Agent patches a fresh canvas → it becomes active; operator_pick keeps the
        // selected pin. Pushing this over the cap forces an eviction.
        let agent_canvas = CanvasId::new(nth_canvas(MAX_CANVASES));
        set.put(ledger_with(&nth_canvas(MAX_CANVASES), "c"));
        set.set_active(&agent_canvas);

        assert_eq!(set.active_id(), Some(&agent_canvas));
        assert_eq!(set.selected_id(), Some(&operator_pick));
        assert!(
            set.get(&agent_canvas).is_some(),
            "the active (agent-patched) canvas is kept",
        );
        assert!(
            set.get(&operator_pick).is_some(),
            "the operator-selected canvas is kept even though it isn't active",
        );
        assert_eq!(set.len(), MAX_CANVASES);
    }

    #[test]
    fn select_is_a_noop_for_an_absent_canvas() {
        let mut set = CanvasLedgerSet::new();
        set.put(ledger_with("canvas:a", "a1"));
        set.select(&CanvasId::new("canvas:ghost"));
        assert!(
            set.selected_id().is_none(),
            "selecting an absent canvas does not set the pin",
        );
        // A real selection records the pin.
        set.select(&CanvasId::new("canvas:a"));
        assert_eq!(set.selected_id(), Some(&CanvasId::new("canvas:a")));
    }

    #[test]
    fn clear_drops_every_canvas_and_the_active_pointer() {
        let mut set = CanvasLedgerSet::new();
        set.put(ledger_with("canvas:a", "a1"));
        set.put(ledger_with("canvas:b", "b1"));
        set.select(&CanvasId::new("canvas:b"));
        assert_eq!(set.len(), 2);

        set.clear();

        assert!(set.is_empty(), "clear drops every canvas");
        assert_eq!(set.len(), 0);
        assert!(set.active_id().is_none(), "clear drops the active pointer");
        assert!(set.active().is_none());
        assert!(
            set.selected_id().is_none(),
            "clear drops the operator-selected pointer too",
        );

        // The set is reusable after a clear: the next canvas becomes active again,
        // exactly as on a fresh set (models a new session starting).
        set.put(ledger_with("canvas:c", "c1"));
        assert_eq!(set.active_id(), Some(&CanvasId::new("canvas:c")));
    }

    #[test]
    fn take_then_put_roundtrips_and_preserves_other_canvases() {
        let mut set = CanvasLedgerSet::new();
        set.put(ledger_with("canvas:a", "a1"));
        set.put(ledger_with("canvas:b", "b1"));

        // Pull canvas:a out, mutate it, put it back — canvas:b is untouched.
        let mut a = set
            .take(&CanvasId::new("canvas:a"))
            .expect("canvas:a present");
        assert_eq!(set.len(), 1, "take removes the canvas from the set");
        a.apply_patch(
            SurfacePatch::UpsertComponent {
                component: CanvasComponentPatch {
                    id: ComponentId::new("a2"),
                    kind: "card".to_string(),
                    rect: Some(Rect::new(20.0, 0.0, 10.0, 10.0)),
                    z_index: None,
                    content: Value::Null,
                    metadata: Value::Null,
                },
            },
            ActorRef::system(),
            1,
        );
        set.put(a);

        assert_eq!(set.len(), 2, "put restores it");
        let a = set.get(&CanvasId::new("canvas:a")).unwrap();
        assert_eq!(a.components.len(), 2, "the mutation landed");
        let b = set.get(&CanvasId::new("canvas:b")).unwrap();
        assert_eq!(b.components.len(), 1, "canvas:b is unaffected");
    }
}
