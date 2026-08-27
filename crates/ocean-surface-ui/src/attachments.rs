//! Room context files — the browser half of the daemon's attachment routes.
//!
//! A room's attachments are the doc, the spec, the screenshot everybody in the
//! room needs to be looking at. The daemon has owned them since the
//! `room_attachments` module landed; until now no browser could reach a single
//! one of them.
//!
//!   GET    /v1/rooms/persistent/{key}/attachments        → the list
//!   POST   /v1/rooms/persistent/{key}/attachments        → put one in
//!   GET    /v1/rooms/persistent/{key}/attachments/{id}   → the bytes
//!
//! Three properties of that wire contract shape everything below:
//!
//! 1. **The upload body is RAW BYTES and every scrap of metadata rides the
//!    query string** — not multipart, not JSON, and emphatically not a custom
//!    header. The daemon's `cors.rs` allows exactly `content-type` and
//!    `authorization` on a cross-origin request, so an `X-Ocean-Attachment-*`
//!    header would pass under curl and die at the browser preflight.
//! 2. **A download is always `application/octet-stream` + `nosniff` +
//!    `Content-Disposition: attachment`**, whatever the uploader declared.
//!    Echoing a declared `text/html` back to this origin is stored XSS, so the
//!    declared type is good for exactly one thing here: choosing a glyph. The
//!    bytes are reached by linking at that octet-stream route, never by
//!    rendering them.
//! 3. **`uploader_id` is caller-asserted and gated.** An id that resolves to an
//!    Agent or System participant comes back 403 `forged_attachment_author`:
//!    an agent's file is written by the daemon's own convene path, never by a
//!    client claiming its identity. So the control is gated on the same two
//!    conditions the composer is — a resolved identity, and an access
//!    projection that permits writes.
//!
//! Everything decided before a request goes out — the size ceiling, the empty
//! body, an unusable filename, how a typed refusal should read — is a free
//! function below, unit-testable natively without a browser or a daemon. The
//! daemon stays the authority on every one of those rules; these copies exist
//! so the operator learns the rule instantly instead of watching 8 MiB crawl
//! upstream to a 413.

use gloo_net::http::Request;
use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};

use crate::rooms::{encode, Rooms};

/// Hard ceiling on one attachment, mirroring the daemon's
/// `MAX_ATTACHMENT_BYTES`. Enforced there; repeated here only so the refusal
/// arrives before the upload does.
pub const MAX_ATTACHMENT_BYTES: u64 = 8 * 1024 * 1024;

/// Longest declared content type the daemon will record
/// (`MAX_CONTENT_TYPE_LEN`).
const MAX_CONTENT_TYPE_LEN: usize = 128;

/// Longest filename the daemon will record (`MAX_FILENAME_LEN`).
const MAX_FILENAME_LEN: usize = 128;

/// What a file with no declared type is called on the wire.
const OPAQUE_CONTENT_TYPE: &str = "application/octet-stream";

// ---- Wire types -------------------------------------------------------------

/// One row of `GET .../attachments`. Mirrors `ocean_core::RoomAttachment`.
///
/// `sha256` and `on_behalf_of` are carried rather than dropped because this
/// struct is the wire shape, not the view model: silently discarding fields
/// the daemon publishes is how a client starts disagreeing with the server
/// about what a record is.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RoomAttachment {
    pub id: String,
    pub filename: String,
    pub content_type: String,
    pub byte_len: u64,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub uploaded_by: String,
    #[serde(default)]
    pub uploaded_at: String,
    #[serde(default)]
    pub on_behalf_of: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AttachmentsListBody {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    attachments: Vec<RoomAttachment>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UploadResultBody {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

// ---- Pure helpers -----------------------------------------------------------

fn list_url(base: &str, key: &str) -> String {
    format!("{base}/v1/rooms/persistent/{}/attachments", encode(key))
}

/// The upload URL, with filename / content type / uploader in the QUERY
/// STRING. See the module header for why they cannot be headers.
fn upload_url(base: &str, key: &str, filename: &str, content_type: &str, uploader: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/attachments\
         ?filename={}&content_type={}&uploader_id={}",
        encode(key),
        encode(filename),
        encode(content_type),
        encode(uploader),
    )
}

/// The bytes. Linked, never fetched-and-rendered: the daemon serves this
/// `application/octet-stream` with `nosniff` and an attachment disposition
/// specifically so it lands in the operator's downloads instead of executing
/// on this origin.
fn download_url(base: &str, key: &str, id: &str) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/attachments/{}",
        encode(key),
        encode(id),
    )
}

/// The daemon's `sanitize_filename`, mirrored: last path component, no control
/// characters, no `"` (it would break out of the quoted `Content-Disposition`
/// parameter), bounded, and never `.` or `..`.
///
/// The point of running it here is not to sanitize — the daemon does that
/// regardless — but to know BEFORE dispatching whether the name it is about to
/// receive is one it can accept, and to show the operator the name the room
/// will actually record.
fn sanitized_filename(raw: &str) -> Option<String> {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let cleaned: String = last
        .chars()
        .filter(|c| !c.is_control() && *c != '"')
        .take(MAX_FILENAME_LEN)
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        return None;
    }
    Some(cleaned)
}

/// The `content_type` this upload will declare.
///
/// `File.type` is empty whenever the browser cannot guess from the extension,
/// and the daemon refuses an empty declaration outright
/// (`is_storable_content_type`) — so an unlabelled file would 400 with nothing
/// the operator could act on. Fall back to the honest answer, opaque bytes,
/// and apply the daemon's bounded visible-ASCII rule so a browser handing us
/// something stranger is corrected here rather than refused there.
fn declared_content_type(raw: &str) -> String {
    let trimmed = raw.trim();
    let storable = !trimmed.is_empty()
        && trimmed.len() <= MAX_CONTENT_TYPE_LEN
        && trimmed.bytes().all(|b| (0x20..0x7f).contains(&b));
    if storable {
        trimmed.to_string()
    } else {
        OPAQUE_CONTENT_TYPE.to_string()
    }
}

/// Why this file is not going to be uploaded, decided before the request.
///
/// All three rules are the daemon's; refusing here only makes the answer
/// immediate. Nothing is ever ADMITTED on this side — a `None` still faces
/// every server-side check.
fn upload_refusal(raw_filename: &str, byte_len: u64) -> Option<String> {
    let Some(name) = sanitized_filename(raw_filename) else {
        return Some("That file has no usable name.".to_string());
    };
    if byte_len == 0 {
        return Some(format!("\u{201c}{name}\u{201d} is empty."));
    }
    if byte_len > MAX_ATTACHMENT_BYTES {
        return Some(format!(
            "\u{201c}{name}\u{201d} is {} \u{2014} the limit is {}.",
            human_bytes(byte_len),
            human_bytes(MAX_ATTACHMENT_BYTES),
        ));
    }
    None
}

/// A size a person can read at a glance. One decimal past KB because the
/// difference between 1.2 MB and 1.9 MB is the whole reason the label exists.
fn human_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.1} KB", n as f64 / KB as f64)
    } else {
        format!("{n} B")
    }
}

/// A glyph for the DECLARED content type.
///
/// This is the only use the declaration gets on this side, and it is
/// deliberately cosmetic: a lying `image/png` buys an attacker a different
/// emoji and nothing else, because the bytes are still fetched from the
/// octet-stream route and never rendered.
fn kind_glyph(content_type: &str) -> &'static str {
    let lowered = content_type.to_ascii_lowercase();
    if lowered.starts_with("image/") {
        "\u{1f5bc}"
    } else if lowered.starts_with("video/") {
        "\u{1f3ac}"
    } else if lowered.starts_with("audio/") {
        "\u{1f50a}"
    } else if lowered.contains("pdf") {
        "\u{1f4d5}"
    } else if lowered.starts_with("text/") || lowered.contains("json") || lowered.contains("xml") {
        "\u{1f4c4}"
    } else if lowered.contains("zip") || lowered.contains("tar") || lowered.contains("gzip") {
        "\u{1f5dc}"
    } else {
        "\u{1f4ce}"
    }
}

/// Turn a refusal into something an operator can act on.
///
/// The daemon's typed `code` is the input, not its prose: the codes are stable
/// and the prose is written for a log reader. `forged_attachment_author` in
/// particular has to say what actually happened — the control was live because
/// this identity may write, but the id it carried belongs to an agent — or the
/// operator reads a permission bug into a working gate.
fn upload_failure_message(status: u16, code: Option<&str>, error: Option<&str>) -> String {
    match code {
        Some("attachment_too_large") => format!(
            "The room refused that file: the limit is {}.",
            human_bytes(MAX_ATTACHMENT_BYTES)
        ),
        Some("forged_attachment_author") => {
            "An agent's file is written by the daemon, not uploaded on its behalf.".to_string()
        }
        Some("unknown_room") => "That room is no longer open.".to_string(),
        Some("invalid_request") => "The room refused that file's name or type.".to_string(),
        _ => match error {
            Some(text) if !text.is_empty() => format!("Upload failed: {text}"),
            _ => format!("Upload failed ({status})."),
        },
    }
}

/// Latest-wins admission for an overlapping list fetch. Identical in shape to
/// `rooms.rs`'s list ticket, and here for the same reason: an older completion
/// publishing over a newer one is what put a premature \u{201c}no files\u{201d} on screen in
/// three previous features (TASK-104/106/107).
fn list_request_is_current(ticket: u64, current: u64) -> bool {
    ticket == current
}

// ---- State ------------------------------------------------------------------

/// Reactive handle for one room's context files.
///
/// Constructed at `RoomsWorkspace` component scope, never inside a rail
/// closure: those closures re-run on every `rooms.access` SSE update, and an
/// upload's in-flight flag rebuilt mid-request would leave the control enabled
/// during its own upload.
#[derive(Clone, Copy)]
pub struct RoomAttachmentsState {
    /// Daemon base URL, shared with `Daemon::url` through `Rooms::url` — read
    /// live at request time because bootstrap resolves the origin
    /// asynchronously (a phone via the tunnel resolves it late).
    pub url: RwSignal<String>,
    /// The open room's attachments, newest-first as the daemon ordered them.
    pub items: RwSignal<Vec<RoomAttachment>>,
    /// Whether a list request has SUCCEEDED for the room now open. Starts
    /// false and returns to false on every room change, so the empty state can
    /// never assert \u{201c}no files\u{201d} about a room that has not answered yet.
    pub loaded: RwSignal<bool>,
    /// Whether a list request is in flight.
    pub loading: RwSignal<bool>,
    /// The most recent failure, list or upload.
    pub error: RwSignal<Option<String>>,
    /// An upload is in flight — blocks re-submit and drives the button label.
    pub uploading: RwSignal<bool>,
    /// Monotonic ticket; only the latest overlapping list request may publish.
    ticket: RwSignal<u64>,
}

impl RoomAttachmentsState {
    pub fn new(rooms: &Rooms) -> Self {
        Self {
            url: rooms.url,
            items: RwSignal::new(Vec::new()),
            loaded: RwSignal::new(false),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            uploading: RwSignal::new(false),
            ticket: RwSignal::new(0),
        }
    }

    fn base(&self) -> String {
        self.url.get_untracked().trim_end_matches('/').to_string()
    }

    /// Retire whatever is on screen and whatever is in flight.
    ///
    /// The ticket bump is the load-bearing half: without it the previous
    /// room's list could still land and be rendered under this room's name.
    fn reset(&self) {
        self.ticket
            .update(|ticket| *ticket = ticket.wrapping_add(1));
        self.items.set(Vec::new());
        self.loaded.set(false);
        self.loading.set(false);
        self.error.set(None);
    }

    /// Load one room's file list.
    pub fn fetch(&self, key: String) {
        let base = self.base();
        let me = *self;
        let ticket = self.ticket.get_untracked().wrapping_add(1);
        self.ticket.set(ticket);
        self.loading.set(true);
        self.error.set(None);
        spawn_local(async move {
            let url = list_url(&base, &key);
            let result = match Request::get(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    match resp.json::<AttachmentsListBody>().await {
                        Ok(body) if body.ok => Ok(body.attachments),
                        Ok(body) => Err(upload_failure_message(
                            status,
                            body.code.as_deref(),
                            body.error.as_deref(),
                        )),
                        Err(err) => Err(format!("Files decode error: {err}")),
                    }
                }
                Err(err) => Err(format!("Files request failed: {err}")),
            };
            if !list_request_is_current(ticket, me.ticket.get_untracked()) {
                return;
            }
            me.loading.set(false);
            match result {
                Ok(items) => {
                    me.items.set(items);
                    // Only a SUCCESS may declare the list known. A failed fetch
                    // that flipped this would replace an honest error with the
                    // false claim that the room has no files.
                    me.loaded.set(true);
                }
                Err(error) => me.error.set(Some(error)),
            }
        });
    }

    /// Read a picked file and POST its bytes, then re-list.
    ///
    /// The refusal check runs before `array_buffer()`, so an oversize file is
    /// never even read into the heap, let alone sent.
    fn upload(&self, key: String, uploader: String, file: web_sys::File) {
        let raw_name = file.name();
        let byte_len = file.size().max(0.0) as u64;
        if let Some(refusal) = upload_refusal(&raw_name, byte_len) {
            self.error.set(Some(refusal));
            return;
        }
        let Some(filename) = sanitized_filename(&raw_name) else {
            return;
        };
        let content_type = declared_content_type(&file.type_());
        let base = self.base();
        let me = *self;
        self.uploading.set(true);
        self.error.set(None);
        spawn_local(async move {
            let bytes = match JsFuture::from(file.array_buffer()).await {
                Ok(buffer) => js_sys::Uint8Array::new(&buffer).to_vec(),
                Err(_) => {
                    me.uploading.set(false);
                    me.error
                        .set(Some(format!("Could not read \u{201c}{filename}\u{201d}.")));
                    return;
                }
            };
            let url = upload_url(&base, &key, &filename, &content_type, &uploader);
            let outcome = match Request::post(&url).body(bytes) {
                Ok(request) => match request.send().await {
                    Ok(resp) => {
                        let status = resp.status();
                        match resp.json::<UploadResultBody>().await {
                            Ok(body) if body.ok => Ok(()),
                            Ok(body) => Err(upload_failure_message(
                                status,
                                body.code.as_deref(),
                                body.error.as_deref(),
                            )),
                            Err(err) => Err(format!("Upload decode error: {err}")),
                        }
                    }
                    Err(err) => Err(format!("Upload request failed: {err}")),
                },
                Err(err) => Err(format!("Upload encode error: {err}")),
            };
            me.uploading.set(false);
            match outcome {
                // The daemon appends a transcript marker for the upload, but
                // the room tail does not carry the attachment row itself, so
                // the list is re-read rather than patched locally.
                Ok(()) => me.fetch(key),
                Err(error) => me.error.set(Some(error)),
            }
        });
    }
}

// ---- Component --------------------------------------------------------------

/// The open room's context files: a real loading state, the list, and one file
/// input behind a button.
///
/// `writes_allowed` is supplied by the workspace rather than recomputed here so
/// this control and the composer can never disagree about the same room's
/// access projection.
#[component]
pub fn RoomAttachments(
    rooms: Rooms,
    state: RoomAttachmentsState,
    writes_allowed: Signal<bool>,
) -> impl IntoView {
    let file_input: NodeRef<leptos::html::Input> = NodeRef::new();

    // Follow the open room. Clearing FIRST is what stops the previous room's
    // files from being shown, however briefly, under this room's name.
    Effect::new(move |_| match rooms.open_key.get() {
        Some(key) if !key.is_empty() => {
            state.reset();
            state.fetch(key);
        }
        _ => state.reset(),
    });

    // Identity is deliberately NOT part of this predicate. `identity_resolved`
    // reads its signals untracked, so a control disabled on it would never
    // re-enable when bootstrap answers; the composer gates on access here and
    // refuses on identity at the action, and the file picker does the same.
    let can_upload = move || {
        writes_allowed.get()
            && !state.uploading.get()
            && rooms.open_key.get().is_some_and(|key| !key.is_empty())
    };

    view! {
        <div class="rooms-workspace__files">
            <div class="rooms-workspace__files-head">
                <span class="rooms-workspace__files-title">"Files"</span>
                <button
                    class="rooms-workspace__files-add"
                    type="button"
                    title="Attach a context file to this room"
                    disabled=move || !can_upload()
                    on:click=move |_| {
                        if let Some(input) = file_input.get_untracked() {
                            input.click();
                        }
                    }
                >
                    {move || if state.uploading.get() { "uploading\u{2026}" } else { "+ file" }}
                </button>
            </div>

            // The button above only forwards the user gesture; this is the real
            // control. One file per upload: the route takes one raw body.
            <input
                class="rooms-workspace__files-input"
                type="file"
                aria-label="Choose a room context file"
                node_ref=file_input
                on:change=move |ev| {
                    let Some(target) = ev.target() else { return };
                    let Ok(input) = target.dyn_into::<web_sys::HtmlInputElement>() else {
                        return;
                    };
                    let picked = input.files().and_then(|files| files.get(0));
                    // Let the operator pick the same file again after a failure;
                    // browsers suppress `change` for an unchanged value.
                    input.set_value("");
                    let Some(file) = picked else { return };
                    let Some(key) = rooms
                        .open_key
                        .get_untracked()
                        .filter(|key| !key.is_empty())
                    else {
                        return;
                    };
                    if !rooms.identity_resolved() {
                        state.error.set(Some(
                            "Still signing in \u{2014} try again in a moment.".to_string(),
                        ));
                        return;
                    }
                    state.upload(key, rooms.identity_id.get_untracked(), file);
                }
            />

            {move || {
                state.error.get().map(|error| view! {
                    <div class="rooms-workspace__files-error" role="alert">{error}</div>
                })
            }}

            {move || {
                // Order matters: in-flight and never-answered both outrank the
                // empty state, which may only speak for a room that replied.
                if state.loading.get() {
                    return view! {
                        <div class="rooms-workspace__files-note">"Loading files\u{2026}"</div>
                    }.into_any();
                }
                if !state.loaded.get() {
                    return ().into_any();
                }
                let items = state.items.get();
                if items.is_empty() {
                    return view! {
                        <div class="rooms-workspace__files-note">"No files yet."</div>
                    }.into_any();
                }
                let base = state.url.get().trim_end_matches('/').to_string();
                let key = rooms.open_key.get().unwrap_or_default();
                let rows = items
                    .into_iter()
                    .map(|item| {
                        // The href is the daemon's octet-stream route. `download`
                        // is belt to its braces: same-origin the browser honors
                        // it, cross-origin (Tauri, extension) the daemon's own
                        // Content-Disposition still lands the file.
                        let href = download_url(&base, &key, &item.id);
                        let meta =
                            format!("{} \u{b7} {}", human_bytes(item.byte_len), item.uploaded_by);
                        view! {
                            <a
                                class="rooms-workspace__file"
                                role="listitem"
                                href=href
                                download=item.filename.clone()
                                title=format!("{} ({})", item.filename, item.content_type)
                            >
                                <span class="rooms-workspace__file-glyph" aria-hidden="true">
                                    {kind_glyph(&item.content_type)}
                                </span>
                                <span class="rooms-workspace__file-name">
                                    {item.filename.clone()}
                                </span>
                                <span class="rooms-workspace__file-meta">{meta}</span>
                            </a>
                        }
                    })
                    .collect::<Vec<_>>();
                view! {
                    <div class="rooms-workspace__files-list" role="list" aria-label="Room files">
                        {rows}
                    </div>
                }.into_any()
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_url_carries_every_field_in_the_query_string() {
        // Not headers: cors.rs allows only content-type and authorization, so a
        // custom header passes under curl and dies at the browser preflight.
        let url = upload_url(
            "http://d",
            "call:xyz",
            "spec v2.md",
            "text/markdown",
            "smaths",
        );
        assert_eq!(
            url,
            "http://d/v1/rooms/persistent/call%3Axyz/attachments\
             ?filename=spec%20v2.md&content_type=text%2Fmarkdown&uploader_id=smaths"
        );
    }

    #[test]
    fn room_key_is_one_encoded_path_segment_on_every_route() {
        // A key with a slash must never become two segments and reach a
        // different daemon route.
        assert_eq!(
            list_url("http://d", "a/b"),
            "http://d/v1/rooms/persistent/a%2Fb/attachments"
        );
        assert_eq!(
            download_url("http://d", "a/b", "0123456789abcdef0123456789abcdef"),
            "http://d/v1/rooms/persistent/a%2Fb/attachments/0123456789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn oversize_and_empty_files_are_refused_before_the_request() {
        let refusal = upload_refusal("huge.bin", MAX_ATTACHMENT_BYTES + 1).expect("refused");
        assert!(refusal.contains("8.0 MB"), "names the limit: {refusal}");
        assert!(refusal.contains("huge.bin"), "names the file: {refusal}");
        assert!(upload_refusal("empty.txt", 0).is_some());
        // Exactly at the cap is legal — the daemon admits it, so we must too.
        assert!(upload_refusal("exact.bin", MAX_ATTACHMENT_BYTES).is_none());
        assert!(upload_refusal("notes.md", 12).is_none());
    }

    #[test]
    fn a_name_the_daemon_would_reject_is_refused_here_instead() {
        assert!(upload_refusal("..", 10).is_some());
        assert!(upload_refusal(".", 10).is_some());
        assert!(upload_refusal("   ", 10).is_some());
        assert!(upload_refusal("/", 10).is_some());
    }

    #[test]
    fn filenames_are_sanitized_the_way_the_daemon_sanitizes_them() {
        assert_eq!(sanitized_filename("/etc/passwd").as_deref(), Some("passwd"));
        assert_eq!(
            sanitized_filename("C:\\docs\\spec.md").as_deref(),
            Some("spec.md")
        );
        // A quote would break out of the Content-Disposition parameter.
        assert_eq!(sanitized_filename("a\"b.txt").as_deref(), Some("ab.txt"));
        assert_eq!(sanitized_filename("a\nb.txt").as_deref(), Some("ab.txt"));
        assert_eq!(
            sanitized_filename(&"x".repeat(200)).map(|name| name.len()),
            Some(MAX_FILENAME_LEN)
        );
        assert_eq!(sanitized_filename(""), None);
    }

    #[test]
    fn an_undeclared_type_becomes_opaque_bytes_not_an_empty_string() {
        // The daemon refuses an empty content_type outright, so sending one
        // through would be a 400 the operator could do nothing about.
        assert_eq!(declared_content_type(""), OPAQUE_CONTENT_TYPE);
        assert_eq!(declared_content_type("   "), OPAQUE_CONTENT_TYPE);
        assert_eq!(declared_content_type("text/markdown"), "text/markdown");
        // Bounded and visible-ASCII, matching is_storable_content_type.
        assert_eq!(declared_content_type("text/\u{e9}"), OPAQUE_CONTENT_TYPE);
        assert_eq!(declared_content_type("text/x\nhtml"), OPAQUE_CONTENT_TYPE);
        assert_eq!(
            declared_content_type(&"a".repeat(MAX_CONTENT_TYPE_LEN + 1)),
            OPAQUE_CONTENT_TYPE
        );
    }

    #[test]
    fn typed_refusals_read_as_the_rule_they_are() {
        assert!(upload_failure_message(413, Some("attachment_too_large"), None).contains("8.0 MB"));
        let forged = upload_failure_message(403, Some("forged_attachment_author"), None);
        assert!(forged.contains("daemon"), "{forged}");
        // An untyped failure still says something, and never swallows the
        // server's own words when it has them.
        assert_eq!(
            upload_failure_message(500, None, Some("disk on fire")),
            "Upload failed: disk on fire"
        );
        assert_eq!(
            upload_failure_message(500, None, None),
            "Upload failed (500)."
        );
        assert_eq!(
            upload_failure_message(500, None, Some("")),
            "Upload failed (500)."
        );
    }

    #[test]
    fn stale_list_responses_cannot_publish() {
        assert!(list_request_is_current(7, 7));
        assert!(!list_request_is_current(6, 7));
    }

    #[test]
    fn sizes_read_like_sizes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2.0 KB");
        assert_eq!(human_bytes(MAX_ATTACHMENT_BYTES), "8.0 MB");
    }

    #[test]
    fn the_glyph_is_the_only_thing_a_declared_type_decides() {
        assert_eq!(kind_glyph("image/png"), kind_glyph("IMAGE/PNG"));
        assert_ne!(kind_glyph("image/png"), kind_glyph("application/pdf"));
        // A lie about the type buys a different emoji and nothing more: the
        // bytes are still fetched from the octet-stream route.
        assert_eq!(kind_glyph("text/html"), kind_glyph("text/plain"));
    }

    #[test]
    fn a_listed_attachment_decodes_from_the_daemons_own_shape() {
        let body: AttachmentsListBody = serde_json::from_str(
            r#"{"ok":true,"attachments":[{
                "id":"0123456789abcdef0123456789abcdef",
                "filename":"spec.md",
                "content_type":"text/markdown",
                "byte_len":1234,
                "sha256":"deadbeef",
                "uploaded_by":"smaths",
                "uploaded_at":"2026-08-25T10:00:00Z"
            }]}"#,
        )
        .expect("decode");
        assert!(body.ok);
        assert_eq!(body.attachments.len(), 1);
        assert_eq!(body.attachments[0].filename, "spec.md");
        // on_behalf_of is absent over HTTP today (the forged-author gate means
        // only a human ever uploads) and must not be required to decode.
        assert_eq!(body.attachments[0].on_behalf_of, None);
    }

    #[test]
    fn a_typed_refusal_body_decodes_without_the_success_fields() {
        let body: UploadResultBody = serde_json::from_str(
            r#"{"ok":false,"code":"attachment_too_large","error":"too big","max_bytes":8388608}"#,
        )
        .expect("decode");
        assert!(!body.ok);
        assert_eq!(body.code.as_deref(), Some("attachment_too_large"));
        assert_eq!(body.error.as_deref(), Some("too big"));
    }
}
