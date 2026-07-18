# Code Context

## Files Retrieved

1. `AGENTS.md` (lines 79-149) — upstream session-label and Rooms contracts; candidate must preserve both while adding `island` stylesheet order.
2. `crates/ocean-surface-ui/src/app.rs` (lines 1125-1140) — reviewed `Rooms::new(&daemon, show_rooms)` API that the Tauri candidate must retain.
3. `crates/ocean-surface-ui/src/sessions.rs` (lines 443-460, 536-558, 785-805, 1834-1864) — project-label refresh/render-key fix from `590ff25`.
4. `crates/ocean-surface-ui/src/rooms.rs` (lines 257-317) — `25009e9` removed LiveKit state from `Rooms` and reduced its constructor.
5. `crates/ocean-surface-proxy/src/main.rs` (lines 203-237, 947-1050) — `/v1/agents` forwarding and unbuffered room SSE handling.
6. Candidate `crates/ocean-surface-ui/src/island.rs` (lines 310-338) — Island session labels prefer `owning_project`, then project-root lookup.
7. Candidate `crates/ocean-surface-ui/src/island_dynamic.rs` (lines 465-490, 532-548) — refreshes both sessions and projects initially and whenever the Island opens.
8. Candidate `crates/ocean-surface-ui/src/app.rs` — Tauri Dynamic Island, shortcuts, overlay coordination, IME-safe composer, stop-state behavior.
9. Candidate `crates/ocean-surface-ui/src/daemon.rs` — request cancellation, attention/history/session data required by the Island.
10. Candidate `crates/ocean-tauri/src/lib.rs` — native menus and opt-in diagnostic bridge.

## Key Code

Current `origin/main` is exactly:

```text
590ff25 Fix stale session project labels
25009e9 TASK-11 transplant onto origin/main...
b09db43 ...
```

The candidate worktree is based at `b09db43` with uncommitted changes. Therefore this is a **working-tree transplant**, not a normal commit rebase.

Load-bearing upstream API:

```rust
// app.rs:1134
let rooms = Rooms::new(&daemon, show_rooms);
```

Project rename behavior that must survive:

```rust
// sessions.rs:451-459
pub(crate) fn section_render_key(sec: &ProjectSection) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        sec.key,
        sec.label,
        section_total_sessions(sec),
        section_newest_ts(sec),
        section_layout_signature(sec),
    )
}
```

Opening the sessions panel must continue calling both:

```rust
daemon.get_value().fetch_sessions();
daemon.get_value().fetch_projects();
```

## Architecture

- `25009e9` establishes daemon-native Rooms:
  - room-scoped SSE with resume;
  - `/v1/agents` proxy and picker;
  - no Rooms-owned LiveKit lifecycle;
  - `Rooms::new` takes only daemon and panel-open signal.
- LiveKit remains a separate app-level optional panel. Do not restore candidate-base calls to `disconnect_livekit_bridge` or LiveKit fields on `Rooms`.
- The Tauri candidate adds a host-conditional Dynamic Island, backed by new daemon session/history/attention state.
- The Island already follows the newer project-label semantics:
  - prefers `owning_project`;
  - falls back to exact catalog/root lookup;
  - calls `fetch_projects()` initially and on each opening.
- `590ff25` still must be retained independently for the shared Sessions panel’s keyed rendering. The Island does not replace that behavior.

## Safe Port / Merge Guidance

1. Start from a genuinely clean `590ff25`/`origin/main` worktree.
2. Export the candidate tracked diff from `b09db43`, and separately copy its intended untracked source/style/doc files. Exclude `.pi-subagents/`.
3. Apply candidate changes by file/hunk, not by replacing whole files.
4. Resolve overlaps as follows:
   - **`AGENTS.md`:** combine changes. Keep the session project-label paragraph and Rooms Contract; add `island` only to the stylesheet cascade.
   - **`app.rs`:** retain all candidate Island/composer/menu coordination, but preserve `Rooms::new(&daemon, show_rooms)`. Do not reintroduce the old five-argument constructor.
   - **`sessions.rs`:** retain all of `590ff25`: `section_render_key`, label in the key, `fetch_projects()` on open, and rename regression test. Candidate’s substantive addition is `ev.stop_propagation()` for modal Escape; its comment/refactor-only changes are optional.
   - **`rooms.rs`, `livekit.rs`, proxy:** take `25009e9` unchanged. The candidate has no intentional replacement for these reviewed semantics.
5. Ensure candidate `main.rs` module declarations land together with `island.rs`, `island_dynamic.rs`, `search.rs`, and component files.
6. Ensure `styles/island.css` is included consistently in `index.html`, `extension/sidepanel.html`, and `scripts/build-extension.sh`.
7. Do not include `.pi-subagents/` artifacts.

The inspected v2 worktree appears to be an in-progress correct port: its differences from the old candidate are principally the retained `25009e9`/`590ff25` behavior.

## Conflicts and Risks

- **Semantic conflict, `app.rs`:** old candidate source still shows the pre-`25009e9` five-argument `Rooms::new`; replacing the file wholesale would regress reviewed Rooms isolation.
- **Semantic conflict, `sessions.rs`:** replacing wholesale would drop project catalog refresh, label-sensitive render identity, and its regression test.
- **Documentation conflict, `AGENTS.md`:** candidate and both reviewed commits edit adjacent contract sections; all three intents must be combined.
- Candidate `daemon.rs` references `rooms::livekit_token_path_for_room`; `25009e9` intentionally retains that pure utility, so this dependency remains valid.
- Untracked candidate files are required for compilation but are invisible to ordinary `git diff`; copying only the tracked patch is incomplete.
- Tauri validation requires a built `dist/`; direct `cargo check` currently fails at `generate_context!` when `../../dist` is absent.
- WASM validation is blocked because `wasm32-unknown-unknown` is not installed in this environment.

## Recommended Tests

After assembling the clean port:

```sh
cargo fmt --all -- --check
cargo test -p ocean-surface-ui
cargo clippy -p ocean-surface-ui --target wasm32-unknown-unknown -- -D warnings
cargo check -p ocean-surface-ui --target wasm32-unknown-unknown
cargo test -p ocean-surface-proxy
cargo clippy -p ocean-surface-proxy -- -D warnings
cargo check -p ocean-surface-proxy
./run-surface.sh   # or build dist via Trunk
cd crates/ocean-tauri && cargo test && cargo check
```

Also verify:

- project rename updates both Sessions panel and Island label;
- opening either surface refreshes the project catalog;
- Rooms use only room-scoped SSE and preserve `Last-Event-ID`;
- `/v1/agents` picker works through the browser proxy;
- room join/leave does not connect or disconnect LiveKit;
- Cmd/Ctrl+P and Cmd/Ctrl+Shift+F remain Tauri-only and IME-safe;
- Escape closes one overlay only.

## Start Here

Open `crates/ocean-surface-ui/src/app.rs` first. It is the main textual and semantic convergence point between the candidate Island and `25009e9`’s Rooms constructor change.