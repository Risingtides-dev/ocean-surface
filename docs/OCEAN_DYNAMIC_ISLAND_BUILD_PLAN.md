# Ocean Dynamic Island — Build Plan

> Binding interaction contract:
> [`OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md`](OCEAN_DYNAMIC_ISLAND_IMPLEMENTATION.md).

## Goal

Ship a titlebar object that changes shape around one current intent. Agent
interaction, session switching, and transcript Recall share positioning and
state routing, but never render as one combined list/dashboard.

## Preserve from the first prototype

- Tauri-only mount and desktop shortcut gating;
- daemon session catalogue and `Daemon::switch_session`;
- deterministic fuzzy session metadata scorer;
- authoritative request/permission snapshots;
- submitter-token permission boundary;
- request cancellation transport state;
- focus capture/restore and overlay mutual exclusion;
- responsive titlebar geometry;
- native diagnostic launch/screenshot harness.

## Remove from the product path

- `Closed/Browse/Search` as the complete mode model;
- click-to-open session catalogue;
- search field plus Activity plus sessions in one surface;
- full-window dimming scrim;
- Activity section headers and accordion disclosures;
- duplicate rows for one session/work item;
- quiet `N recent` compact status;
- unbounded initial session list.

## Phase 1 — Correct the interaction model

### Changes

- add `Closed/Agent/Sessions/Recall` state;
- make compact click open Agent;
- route `Cmd/Ctrl+P` to Sessions;
- route `Cmd/Ctrl+Shift+F` to Recall;
- add command-registry entries;
- ensure at most one mode mounts;
- keep browser/PWA shortcuts untouched.

### Exit

- mode-transition unit tests pass;
- opening one mode never mounts content from another;
- `Cmd/Ctrl+K` and Escape precedence remain correct.

## Phase 2 — Direct Agent object

### Changes

- project requests/permissions into ordered work objects;
- show one selected object at a time;
- expose direct Approve/Deny/Stop/Open Session actions;
- retain focused-token permission gates;
- add idle focused-session `Ask Ocean…` steering field;
- collapse after successful prompt dispatch;
- use left/right stepping when several items exist.

### Exit

- no Activity list or disclosure drawer exists in the active UI;
- permission and stop tests prove authority boundaries;
- running, needs-human, failure, cancellation, and idle states are distinct.

## Phase 3 — Dedicated Sessions switcher

### Changes

- isolate session metadata query/selection state;
- render at most eight initial rows and twenty filtered rows;
- keep focused-first ordering and zero-turn discoverability;
- arrow/Enter/click use `Daemon::switch_session`;
- keep project management in the full Sessions panel.

### Exit

- Sessions contains no agent work or transcript history hits;
- local fuzzy ranking tests remain deterministic;
- browser/extension Sessions behavior is unchanged.

## Phase 4 — Daemon-owned Recall

### Daemon

- add `GET /v1/agent/history/search`;
- search persisted display transcript entries for user/assistant roles only;
- exact phrase, all-token lexical, then fuzzy subsequence ranking;
- stable deterministic tie breaking;
- Unicode-safe bounded excerpts;
- default 20, clamp 1..50;
- explicit `exact|lexical|fuzzy` provenance;
- no provider calls, embeddings, raw provider messages, or tool payloads.

### Surface

- add typed response DTOs and generation-guarded requests;
- debounce query dispatch;
- render excerpt/session/role/workspace/match provenance;
- arrow/Enter/click open the source session;
- loading, empty, and truthful error states;
- do not claim semantic ranking until daemon/Bedrock provides it.

### Exit

- a phrase present only inside a prior transcript is discoverable;
- stale responses cannot replace a newer query;
- Recall contains no session metadata-only results or agent actions.

## Phase 5 — Elastic native geometry

### Changes

- stage starts behind the compact capsule;
- remove visual scrim dimming;
- assign per-mode bounded geometry;
- derive nested radii from outer radius minus padding;
- preserve outside-click dismissal;
- preserve titlebar traffic-light/control clearance;
- reduced-motion fallback.

### Exit

- wide and 720px native screenshots read as one object changing shape;
- Agent remains shallow;
- Sessions and Recall scroll independently within bounds;
- no mode resembles a full-height dashboard.

## Phase 6 — Hardening

### Automated

```sh
cargo test -p ocean-surface-ui
cargo check -p ocean-surface-ui --target wasm32-unknown-unknown
cargo clippy -p ocean-surface-ui --all-targets -- -D warnings
cargo check -p ocean-surface-proxy
cd crates/ocean-tauri && cargo check
./scripts/build-extension.sh
cargo fmt --all -- --check
git diff --check
```

Daemon work additionally runs its owning crate tests, workspace checks required
by the daemon contract, docs checks, and fresh review.

### Native interaction matrix

| State | Wide | 720px | Keyboard | Pointer |
| --- | --- | --- | --- | --- |
| compact ready | screenshot | screenshot | focus/Enter | click Agent |
| Agent idle | screenshot | screenshot | type/Enter/Escape | routes/outside |
| Agent running | screenshot | screenshot | step/Stop | direct actions |
| Agent permission | screenshot | screenshot | Tab/actions | Approve/Deny |
| Sessions | screenshot | screenshot | query/arrows/Enter | row click |
| Recall | screenshot | screenshot | query/arrows/Enter | hit click |

## File map

| Path | Responsibility |
| --- | --- |
| `crates/ocean-surface-ui/src/island.rs` | pure session/attention projection and ranking |
| `crates/ocean-surface-ui/src/island_dynamic.rs` | active shell, modes, interaction rendering |
| `crates/ocean-surface-ui/src/search.rs` | shared deterministic fuzzy scorer |
| `crates/ocean-surface-ui/src/daemon.rs` | HTTP DTOs/signals/transport |
| `crates/ocean-surface-ui/src/app.rs` | global routing and overlay precedence |
| `styles/island.css` | compact object and per-mode elastic geometry |
| `../ocean-os/crates/ocean-agent/` | persisted transcript search logic |
| `../ocean-os/crates/ocean-daemon/` | Recall HTTP route/response authority |

The temporary two-module split keeps the heavily shared `app.rs` integration
small while the corrected interaction proves itself. After native acceptance,
rename `island.rs` to a neutral model module and make `island_dynamic.rs` the
canonical `island` module in one cleanup-only change.

## Stop conditions

Pause rather than invent behavior when:

- a requested action lacks daemon authority;
- background turns would share focused SSE/token state;
- semantic indexing ownership is unclear;
- a Recall hit lacks enough identity to open safely;
- a visual change would alter web/extension behavior;
- concurrent work modifies the same shared integration hunk.

## Definition of corrected V1

Corrected V1 is complete when:

- compact click opens direct Agent interaction;
- agent work is one morphing object, not an Activity feed;
- Sessions and Recall are separate tools with separate queries/results;
- transcript Recall is daemon-backed and truthfully labeled;
- permission/cancellation authority remains unchanged;
- wide/narrow native evidence passes;
- the canonical WASM, Tauri, and extension builds pass.
