# ocean-surface-proxy — local contract

The root `AGENTS.md` (Rooms Contract, Workspace Map) is the parent contract.
This file holds the proxy's boundary rules: what it trusts, what it refuses,
and why. The daemon behind it has no auth of its own for most routes and
runs tools, so the proxy's route table and these checks are the real
allow-list of browser-reachable authority.

## Paths: what the client sends is what the daemon gets, or nothing

- Every forward builds its upstream URL with `upstream_url(daemon, path,
  query)`, which parses the URL and refuses (`400
  {"ok":false,"error":"upstream_path_rewritten"}`) unless the parsed path is
  byte-for-byte the path the proxy decided to forward. The `url` crate
  normalises on parse (dot segments collapse, `\` becomes `/`, tab/newline
  vanish), and each of those once let an approved path arrive upstream as a
  different route. Callers hand the returned `reqwest::Url` to the client, so
  the value checked is the value sent. Never add a forwarder that formats a
  `String` URL and passes `&url` to reqwest.
- `has_dot_segment` refuses, per `/`-segment and after one percent-decode,
  `.`, `..`, and any backslash (raw `\`, `%5C`, `%5c`). Answer: `400` plain
  text `invalid path`. hyper accepts a raw `\` in the request target, and the
  allowlist's segment split did not see it as a separator while the URL parser
  did: `DELETE .../agents/x\..\..\close` reached `DELETE .../close` with the
  operator key. It runs on raw wildcard paths (rooms-persistent, longhouse) and
  on every captured `{id}`.
- A captured `{id}` is percent-DECODED by axum, so it is rebuilt with
  `daemon_path_with_segment(prefix, id, suffix)` — dot-checked, then
  re-encoded into exactly one segment. Formatting a capture raw lets `%2F`
  become a second segment and `%3F` start a query.
- Wildcard families forward the RAW request path (`req.uri().path()`), never
  the decoded `{*rest}` capture.

Both halves are needed and each is mutation-tested alone
(`src/tests/path_actor_csrf.rs`): `%5C` is not rewritten by the parser (only
the dot guard stops it), and a future normalisation rule is stopped only by
the parse-and-compare.

## Operator key

Injected only on the six room-agent authority shapes
(`room_agent_authority_mutation`), and read only AFTER every refusal —
`upstream_url`, the actor binding, the duplicate-key check — so a request the
proxy refuses never touches the credential. Browser `X-Ocean-Operator`,
Cookie, Origin and Referer never cross.

Holding the key does not make the caller the room's owner: in multi-user mode
the authority bodies' `owner_member_id` (bootstrap, authorize) is bound to the
session user like every member-lane identity below. Before that binding, any
signed-in roster user could bootstrap a local room naming someone else as
owner, and the first bootstrap WRITES the owner. Consequence for the surface:
only the room's owner can authorize an agent from their own session (the
authorize body carries the daemon-resolved owner from the preview).

## Member-lane actor binding (rooms-persistent)

Trust chain, top to bottom:

1. The proxy's session cookie (HttpOnly, `SameSite=Strict`) resolves to a
   roster user via `session_user`. That username is exactly what
   `/api/config` publishes as `user_id`, and the surface uses it as the room
   identity.
2. For requests under `/v1/rooms/persistent`, when (1) resolved a user,
   every identity the daemon reads must equal that username, or the proxy
   answers `403 {"ok":false,"code":"actor_mismatch","error":"actor_mismatch"}`
   before forwarding. Refused, never rewritten. The fields, from ocean-os
   `room_routes()` / `room_agent_authority.rs`:
   - query `actor_id` (close, attachment delete, and the workspace lane —
     `gate_workspace_call` gates READS on it too) and `uploader_id`
     (attachment upload), decoded as the daemon's `Query` decodes them, every
     occurrence, on EVERY method including GET. No other rooms GET reads an
     identity from its query (list/transcript/snapshot/events take cursors);
   - JSON body (non-GET/HEAD) `author_id` (post, artifact create/amend),
     `invoked_by` (agent invoke), `requested_by` (summarize),
     `owner_member_id` (authority bootstrap/authorize) and `owner_id` (an
     agent join — it writes `room_agent_owners`, which authority's
     `target_proof` trusts);
   - a join's (`POST {key}/participants`) body `id`, unless `kind` is
     `agent` (an agent id names a daemon-validated folder, not the caller —
     its `owner_id` is still bound).
   `agent_member_id` and the path ids of the authority routes are TARGETS and
   are not bound. A non-blank body that is not a JSON object is refused (fail
   closed); the daemon would refuse it anyway. A body that repeats any bound
   key, `id`, or `kind` (escaped spellings included) is refused `400
   {"ok":false,"code":"duplicate_identity_field",...}` — the check must not
   depend on which copy a parser keeps. The raw-bytes upload body is not
   parsed.
3. The daemon then roster-checks the (now session-bound) identity as before.

Not bound, on purpose:

- Single-operator mode (no users file) and auth-off: `/api/config` publishes
  an empty `user_id`, the surface uses the constant `surface-operator`, and
  there is no person to bind to. A roster deployment's legacy operator login
  (the `OCEAN_SURFACE_USER` credential, not a roster entry) is the same case.
- **Known gap (daemon work):** `DELETE {key}/participants/{id}` (leave AND
  the roster's remove-member) and `DELETE {key}/members/{id}` carry a TARGET,
  not an actor, and the daemon reads no caller identity on them. The surface
  uses the first to remove other participants, so the proxy cannot tell a
  spoofed leave from a legitimate remove: any signed-in user can remove
  anyone, INCLUDING the room's owner, which flips `owner_present` and with it
  what the authority ceremony will admit. Closing it needs the daemon to take
  a caller identity on these routes; the proxy can then bind it here.
- Routes with no client identity at all (create, PATCH room, invites,
  redeem, read-cursor, outbox retry, register agents) — nothing to bind.

## Cross-site requests and hosts

- Auth-off (loopback-only, `OCEAN_SURFACE_AUTH=off`): `auth_off_cross_site_gate`
  runs `auth_off_room_mutation_source_allowed` on EVERY non-GET/HEAD request
  under `/v1/` and `/api/`. Every supplied Origin/Referer must name the exact
  loopback Host that received the request; a request with neither is admitted
  (local scripts/CLIs). Refusal: `403
  {"ok":false,"error":"cross_site_mutation_refused"}`, or
  `cross_site_operator_mutation_refused` on the six authority shapes (the code
  their clients already decode). Why all of them: every proxied POST is
  reachable without a CORS preflight (a `<form>` or `no-cors` fetch), the
  forwarders stamp `application/json` on whatever body arrives, and close
  needs no body at all. `/csp-report`, `/login`, `/logout` are outside it.
- Auth-off, every request (reads and static files too): the request's
  authority (Host header, or the HTTP/2 URI authority) must be loopback —
  `localhost`, `127.0.0.0/8`, `::1` — or it is refused `403
  {"ok":false,"error":"non_loopback_host_refused"}`. Auth-off binds loopback
  only, so a non-loopback Host means a DNS-rebinding page (same-origin with
  the name it rebound, so no Origin check sees it) that could otherwise read
  every GET. A request naming no authority at all (HTTP/1.0, in-process tests)
  is admitted; a browser always names one. Auth-off behind a tunnel or
  `tailscale serve` now answers 403 — use auth-on for any non-local access,
  as the root contract already requires.
- Auth-on: no Origin check. The session cookie is `SameSite=Strict`, so a
  cross-site request arrives unauthenticated and the auth gate answers 401.
  Keep it Strict; relaxing it to Lax reopens every POST above. Two residual
  edges, recorded, not closed:
  - SameSite is about the SITE (registrable domain), not the origin: a page
    on another `*.agentsworld.org` host is same-site and its requests DO
    carry the cookie. Any host under that domain is inside this boundary.
  - A Basic credential a browser has cached for this origin (typed into a
    `user:pass@` URL — the proxy never challenges, so only by hand) is sent
    on cross-site requests regardless of SameSite.

## JSON lanes take JSON, not whatever arrived

Every forwarder to a JSON daemon route (turns, sessions, agents, model,
projects, component event, calls, realtime client-secret, session messages,
livekit token, permission decision, longhouse POST, and every rooms-persistent
non-GET except the attachment upload) admits a body only when it is empty or
declared `application/json` / `application/*+json` (case and parameters
ignored). Anything else — `text/plain`, a form or multipart type, or NO type
(a typeless Blob) — is refused `415
{"ok":false,"error":"json_content_type_required"}`. Those are exactly the
bodies a browser sends cross-origin without a CORS preflight; the forwarders
used to stamp `application/json` on them, which made JSON-only daemon routes
reachable by a plain `<form>` or `no-cors` fetch. This is defence in depth
behind the auth-off gate and, in auth-on, the answer to a same-site page.
Handlers take the `JsonForward` extractor; the two manual forwarders call
`json_body_acceptable` directly. Never add a JSON forward that takes a bare
`Bytes` body. The PWA sends every JSON write via gloo `.json()` or an explicit
`content-type: application/json`, and its bodiless POSTs are empty, so it is
unaffected. Raw-bytes lanes keep their own types: the attachment upload
forwards the declared type (the PWA sends a typeless ArrayBuffer), and
`/api/stt` forwards audio as octet-stream.

## Fail-closed side effects worth knowing

- A hand-written raw request target containing bytes the URL parser
  re-encodes (non-ASCII, spaces, etc.) is refused `upstream_path_rewritten`
  rather than forwarded re-encoded. Browsers already percent-encode these, so
  only hand-built clients notice.
- Room keys and ids containing a backslash (`\`, `%5C`) are unreachable
  through the proxy.
