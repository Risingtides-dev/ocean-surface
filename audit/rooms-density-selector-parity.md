# Rooms density selector parity audit
- `.rooms-workspace__msg--grouped` present in `styles/rooms-workspace.css`
- `.rooms-workspace__day-separator` present in `styles/rooms-workspace.css`
- Both selectors are exercised by `room_messages.rs` / `rooms_workspace.rs` adoption paths
- Full audit: all 86 `rooms-workspace__*` classes emitted by
  `rooms_workspace.rs` resolve against loaded stylesheets; the two
  orphans found (`__left-options`, `__member-list` a11y wrappers from
  the ARIA slice) are now explicitly `display: block` (layout-neutral;
  parents are plain block-flow scroll containers, no child combinators).
- Single definition: density rules live only at the head of
  `styles/rooms-workspace.css`; `rooms-interaction.css` carries a
  do-not-re-add pointer; the grouped-time hover reveal gained its
  missing base `opacity: 0` state.
