//! Who owns which agent has to reach the rail, not just the decoder.
//!
//! `ocean-os#437` serves `agent_owners` on room detail AND on `/snapshot`, the
//! route this surface hydrates through: one row per owned Agent participant,
//! naming the agent, the WORKER who owns it, and whether that worker is still
//! on the roster, ordered by roster position. Before this slice
//! `git grep agent_owners crates/ocean-surface-ui/src` returned nothing — the
//! field was served, documented in the ecosystem contract, and decoded nowhere,
//! so a member could not see who owned an agent or that an agent was unclaimed,
//! including in the audit view of a closed room where nobody is left to ask.
//!
//! Every rule in the feature is a pure function two unit-test modules already
//! own: `rooms.rs` proves the wire shape decodes with and without the key, and
//! `rooms_workspace.rs` proves `agent_ownership` maps an agent to its owner or
//! to unclaimed. What no unit test in a BINARY crate can say is whether
//! anything still CALLS them. `crates/ocean-surface-ui` is `src/main.rs` with
//! no `[lib]` (see `tests/common/mod.rs`), so a test here cannot mount the
//! workspace or read a rendered rail; scanning the source is the lever that is
//! left, under the two rules `unheld_room_controls.rs` paid for: name the CALL
//! SITE, not the literal, and scan `common::view_source`, which truncates at
//! `#[cfg(test)]` so a module's own unit tests cannot satisfy a needle by
//! quoting it.
//!
//! ## What this file deliberately does NOT assert
//!
//! That the FEDERATED members branch renders ownership. It must not, and the
//! reason is in the daemon: `SqliteRoomStore::agent_owners` joins the ownership
//! row to `participants` and orders by `p.position`, so both ids in every row
//! are LOCAL participant ids. A federated row's `member_id` is a bedrock-minted
//! binding id from `room_agent_bindings` — a different table in a different
//! namespace. Looking one up in the other would match nothing and badge every
//! federated agent "unclaimed", which is a confident lie rather than a gap. The
//! rail says nothing there instead, and `docs/OCEAN_ROOMS_PRODUCT.md` says why.
//!
//! ## Measured, not assumed
//!
//! Six mutations applied for real against this tree, each ALONE, with this file
//! and `cargo test -p ocean-surface-ui` both executed and the tree restored
//! verbatim in between. The right-hand column is the one that matters.
//!
//! | mutation                                                          | result |
//! |--------------------------------------------------------------------|--------|
//! | `#[serde(default)]` dropped from `agent_owners`                     | RED here, and SIX `rooms.rs` unit tests — every fixture in that module that builds a `/snapshot` body without the key |
//! | the decode arm's `r.agent_owners` replaced with `Vec::new()`         | RED here, alone — the field decodes and dies inside the response |
//! | `me.agent_owners.set(..)` moved inside `open_room`'s `if !closed`    | RED here, alone — the audit view is exactly the room that needs it |
//! | `reset_room_state` stops clearing it                                 | RED here, alone — the next room opened badges its agents with the previous room's owners |
//! | the rail's whole ownership block deleted                             | RED here — but see below |
//! | the `Unclaimed` arm's `view!` replaced with `().into_any()`          | RED here, alone, and both clippy lanes green — an unclaimed agent renders as a bare row again, which is the state before this slice |
//!
//! The fifth row is the one that came back other than expected, and it is
//! written down rather than quietly dropped. Deleting the rail's ownership
//! block was assumed to leave every other lane green, on the reasoning that
//! `agent_ownership` keeps its unit-test callers. It does not: those callers
//! are inside `#[cfg(test)]`, so a release build sees none, and
//! `cargo clippy --target wasm32-unknown-unknown -- -D warnings` fails with
//! three errors — `unused variable: owner_row_id`, `enum AgentOwnership is
//! never used`, `function agent_ownership is never used`. `cargo test` stays
//! green apart from this file (run). So the rail needles below are
//! belt-and-braces against a wholesale deletion; what they uniquely catch is a
//! block that still calls the predicate and renders the wrong thing — which is
//! what the sixth row measures.

mod common;

use common::{read, view_source, without_whitespace};

/// The wire half: decoded additively, carried out of the decode arm, published,
/// and cleared on the way out of a room.
#[test]
fn the_snapshots_agent_owners_reach_the_open_room_call_site() {
    let rooms = without_whitespace(&view_source("rooms.rs"));

    assert!(
        rooms.contains("#[serde(default)]agent_owners:Vec<RoomAgentOwner>,"),
        "`agent_owners` must decode with `#[serde(default)]`: the ecosystem \
         contract rules the field additive, so a daemon predating ocean-os#437 \
         sends no key at all. Without the default that body is a decode error, \
         and `open_room` reports decode errors as a failed open — every room on \
         such a daemon refusing to load over a field the open path never needs",
    );

    let hydration_arm = rooms
        .split_once("Ok(r)ifr.ok=>Ok((")
        .and_then(|(_, rest)| rest.split_once("))"))
        .map(|(arm, _)| arm)
        .expect("`open_room`'s hydration decode arm builds a tuple");
    assert!(
        hydration_arm.contains("r.agent_owners"),
        "the hydration decode arm must carry the owners out with the record \
         they describe; a field decoded and left inside the response is a field \
         nothing can render",
    );

    assert!(
        rooms.contains("me.agent_owners.set(agent_owners);"),
        "`open_room` must PUBLISH ownership — the rail reads the signal, and a \
         decode that stops at the local binding leaves `git grep agent_owners \
         src` answering exactly what it answered before this slice",
    );
    assert!(
        rooms.contains("self.agent_owners.set(Vec::new());"),
        "`reset_room_state` must clear it. Both `open_room` and `close_room` \
         call that, so this is what stops one room's ownership from outliving \
         it into the next room opened — where a same-named agent would wear \
         the previous room's owner and the rail has no way to tell",
    );
}

/// The audit view is not a special case, and the guard is POSITIONAL because
/// nothing else can be: `me.agent_owners.set(..)` inside `open_room`'s
/// `if !closed { … }` compiles, keeps every unit test green, and ships a frozen
/// room whose rail says every agent is unclaimed. A closed room retains its
/// roster and its ownership rows, the snapshot IS the audit view, and it is the
/// room whose reader has nobody left to ask — so the publish sits ahead of the
/// branch that decides whether to open a tail at all.
#[test]
fn a_closed_rooms_audit_view_publishes_the_same_ownership() {
    let rooms = without_whitespace(&view_source("rooms.rs"));

    let publish = rooms
        .find("me.agent_owners.set(agent_owners);")
        .expect("`open_room` publishes the owners");
    // The gate's OPENING BRACE, not the call inside it. Matching
    // `if!closed{me.start_live_tail(` would make the mutation this test exists
    // for — moving the publish into that branch — red on the `expect` rather
    // than on the assertion below, reporting a missing tail gate for a tree
    // whose tail gate is exactly where it always was.
    let tail_gate = rooms
        .find("if!closed{")
        .expect("`open_room` gates the live tail on closedness");

    assert!(
        publish < tail_gate,
        "the ownership publish must sit AHEAD of `if !closed` and outside it: \
         a soft-closed room reports agent_owners unchanged, and gating the \
         publish on liveness paints the audit view as a room where nobody \
         owned anything",
    );
}

/// The rail half. `agent_ownership` keeps unit-test callers no matter what the
/// view does, so deleting the call site below leaves `cargo test` and both
/// clippy lanes green with the feature gone — which is the whole reason this
/// test exists rather than trusting the compiler.
#[test]
fn the_members_rail_renders_the_owner_and_names_the_unclaimed() {
    let workspace = without_whitespace(&view_source("rooms_workspace.rs"));

    assert!(
        workspace.contains(
            "matchrooms.agent_owners.with(|owners|{agent_ownership(owners,&participants,&owner_row_id)}){"
        ),
        "the rail must ASK the predicate, at the row it is rendering — a rail \
         that decodes ownership and renders none is the state this slice \
         exists to leave behind",
    );
    assert!(
        workspace.contains("letparticipants=rooms.open_room.get().map(|r|r.participants)"),
        "and it must read the LIVE roster beside the owners: join, leave and \
         remove all replace `open_room` from routes that carry no agent_owners \
         at all, so the owner's display name and their presence both come from \
         the roster the reader is actually looking at",
    );
    assert!(
        workspace.contains("ifkind!=RoomParticipantKind::Agent{return().into_any();}"),
        "only agent rows carry ownership: a Human row badged `unclaimed` is \
         nonsense, and every non-agent kind would get one without this",
    );

    assert!(
        workspace.contains("{format!(\"ownedby{owner}\")}"),
        "the owned case must NAME the owner. A dot alone says a state changed \
         and not whose",
    );
    assert!(
        workspace.contains(
            "<spanclass=\"rooms-workspace__member-ownerrooms-workspace__member-owner--unclaimed\">\"unclaimed\"</span>"
        ),
        "an agent nobody owns must SAY so. Rendering nothing there is \
         indistinguishable from a rail that had nothing to say, which is \
         exactly what a member saw before this slice",
    );
    assert!(
        workspace.contains("class:rooms-workspace__member-presence--live=present")
            && workspace.contains("class:rooms-workspace__member-presence--unavailable=!present"),
        "owner presence rides the rail's OWN presence dot, in both states — \
         the binding outlives the worker, so `owned by Alice` with Alice gone \
         must not read the same as `owned by Alice` with Alice here",
    );
}

/// The stylesheet half. Every class the rail emits above must resolve, or the
/// ownership line renders as unstyled text mid-row: the rail is a 220px flex
/// column and the owner line only reaches its own second line through
/// `flex-basis: 100%`.
#[test]
fn the_ownership_line_has_the_rules_it_renders_against() {
    let css = read("styles/rooms-workspace.css");

    for selector in [
        ".rooms-workspace__member-owner {",
        ".rooms-workspace__member-owner--unclaimed {",
        ".rooms-workspace__member-owner .rooms-workspace__member-presence {",
    ] {
        assert!(
            css.contains(selector),
            "`{selector}` is emitted by the members rail and must exist in \
             styles/rooms-workspace.css",
        );
    }

    let owner_rule = css
        .split_once(".rooms-workspace__member-owner {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(body, _)| body)
        .expect("the owner-line rule has a body");
    assert!(
        owner_rule.contains("flex-basis: 100%"),
        "the ownership line takes its own row under the name, like the agent \
         descriptor does. Inline, it competes with the kind badge and the \
         remove control for a 220px rail and ellipses away to nothing",
    );

    let dot_override = css
        .split_once(".rooms-workspace__member-owner .rooms-workspace__member-presence {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(body, _)| body)
        .expect("the nested presence-dot rule has a body");
    assert!(
        dot_override.contains("margin-left: 0"),
        "`.rooms-workspace__member-presence` carries `margin-left: auto`, which \
         is what pins the ROW's dot to the right edge. Inherited here it would \
         fling the owner's dot away from the name it belongs to",
    );
}
