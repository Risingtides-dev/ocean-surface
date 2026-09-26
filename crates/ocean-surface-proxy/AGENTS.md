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
(`room_agent_authority_mutation`), and only AFTER `upstream_url` has
accepted the path — a path the parser would rewrite never reaches the key
read. Browser `X-Ocean-Operator`, Cookie, Origin and Referer never cross.

## Member-lane actor binding (rooms-persistent)

Trust chain, top to bottom:

1. The proxy's session cookie (HttpOnly, `SameSite=Strict`) resolves to a
   roster user via `session_user`. That username is exactly what
   `/api/config` publishes as `user_id`, and the surface uses it as the room
   identity.
2. For every non-GET/HEAD request under `/v1/rooms/persistent`, when (1)
   resolved a user, every identity the daemon's member lane reads must equal
   that username, or the proxy answers `403
   {"ok":false,"code":"actor_mismatch","error":"actor_mismatch"}` before
   forwarding. Refused, never rewritten. The fields, from ocean-os
   `room_routes()`:
   - query `actor_id` (close, attachment delete, workspace commands) and
     `uploader_id` (attachment upload) — decoded as the daemon's `Query`
     decodes them, every occurrence;
   - JSON body `author_id` (post, artifact create/amend), `invoked_by`
     (agent invoke), `requested_by` (summarize);
   - a join's (`POST {key}/participants`) body `id`, unless `kind` is
     `agent` (an agent id names a daemon-validated folder, not the caller).
   A non-blank body that is not a JSON object is refused (fail closed); the
   daemon would refuse it anyway. The raw-bytes upload body is not parsed.
3. The daemon then roster-checks the (now session-bound) identity as before.

Not bound, on purpose:

- Single-operator mode (no users file) and auth-off: `/api/config` publishes
  an empty `user_id`, the surface uses the constant `surface-operator`, and
  there is no person to bind to. A roster deployment's legacy operator login
  (the `OCEAN_SURFACE_USER` credential, not a roster entry) is the same case.
- `DELETE {key}/participants/{id}` (leave AND the roster's remove-member)
  and `DELETE {key}/members/{id}` carry a TARGET, not an actor; the surface
  uses the first to remove other participants, so the proxy cannot tell a
  spoofed leave from a legitimate remove. The daemon has no caller identity
  for these routes; closing that is daemon work.
- Routes with no client identity at all (create, PATCH room, invites,
  redeem, read-cursor, outbox retry, register agents) — nothing to bind.

## Cross-site requests

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
- Auth-on: no Origin check. The session cookie is `SameSite=Strict`, so a
  cross-site request arrives unauthenticated and the auth gate answers 401.
  Keep it Strict; relaxing it to Lax reopens every POST above.
