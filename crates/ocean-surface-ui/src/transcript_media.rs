//! Inline media for attachment markers in the rooms transcript.
//!
//! When someone drops an image into a room, the transcript gets a system
//! marker ("john attached 'spec.png' (52413 bytes)") carrying the
//! `attachment_id` of the file it describes. This module turns that marker
//! into an `<img>` pointed at the daemon's download route — and nothing more.
//!
//! The safety story is the daemon's, not ours. That route sniffs the BYTES
//! and serves a real image type for exactly PNG/JPEG/GIF/WebP; everything
//! else stays `application/octet-stream`, and `X-Content-Type-Options:
//! nosniff` is unconditional. So an `<img>` at that URL is self-truthing: a
//! real image renders, and a non-image, deleted, or unsniffable attachment
//! errors the element, which swaps in the named-file link the files panel
//! already renders. No byte is fetched-and-inspected here, no
//! uploader-declared content type is consulted, and nothing the uploader
//! wrote is ever rendered as markup — SVG is deliberately outside the
//! daemon's sniff allowlist precisely so it can never execute on this
//! origin, and this module must not work around that.
//!
//! REMOVAL markers ("john removed attachment 'spec.png'") carry
//! `attachment_id` too — the field exists partly so a client can retire a
//! rendered image — and must never mount an image attempt. As the daemon
//! words its markers today that cannot happen anyway: an upload marker ends
//! in ` bytes)` and a removal in `'`, so the upload parse alone rejects
//! every removal body. Removal is still checked first, explicitly, as
//! future-proofing: if the daemon rewords a marker or the upload parse is
//! ever loosened, the failure mode stays a lost preview, never an image
//! under a removal line. An unrecognized marker body mounts nothing at all.

use leptos::prelude::*;

use crate::attachments::download_url;
use crate::rooms::{RoomMessage, RoomMessageKind, Rooms};

// ---- Pure decisions ---------------------------------------------------------

/// Filename inside the store's upload marker,
/// `"{uploader} attached '{filename}' ({byte_len} bytes)"`.
///
/// The filename may itself contain `'` or `' (` (the daemon strips only
/// control characters and `"`), so the split anchors on the FIRST
/// `" attached '"` and the LAST `"' ("` with a digits-then-`bytes)` tail —
/// the one decomposition the server-built suffix guarantees.
fn upload_marker_filename(body: &str) -> Option<&str> {
    let after = body.split_once(" attached '")?.1;
    let (name, tail) = after.rsplit_once("' (")?;
    let count = tail.strip_suffix(" bytes)")?;
    (!name.is_empty() && !count.is_empty() && count.bytes().all(|b| b.is_ascii_digit()))
        .then_some(name)
}

/// Whether a marker body is the store's removal marker,
/// `"{remover} removed attachment '{filename}'"`.
fn is_removal_marker(body: &str) -> bool {
    body.contains(" removed attachment '") && body.ends_with('\'')
}

/// The one decision the transcript makes per row: mount an image attempt —
/// `Some((attachment_id, filename))` — or mount nothing.
///
/// The removal check is not load-bearing today: no body satisfies both
/// shapes, because an upload marker must end in ` bytes)` and a removal in
/// `'`, so the upload parse already returns `None` for every removal. It
/// still runs first, deliberately, so the two rejections stay independent:
/// should the daemon reword a marker or the parse above grow laxer, the
/// cost is a lost preview — never an image mounted under a "removed
/// attachment" line, exactly the state the field exists to retire.
pub(crate) fn marker_image(msg: &RoomMessage) -> Option<(String, String)> {
    // Markers ride the system lane. A Message-kind row carrying the field is
    // not one, whatever it claims.
    if matches!(msg.kind, RoomMessageKind::Message) {
        return None;
    }
    let id = msg.attachment_id.as_deref()?;
    if is_removal_marker(&msg.body) {
        return None;
    }
    upload_marker_filename(&msg.body).map(|name| (id.to_string(), name.to_string()))
}

// ---- View -------------------------------------------------------------------

/// The media block for one transcript row, or `None` when the row mounts
/// nothing (`Option<AnyView>` renders as nothing in the timeline).
///
/// The URL is rebuilt from live signals rather than snapshotted: the daemon
/// origin can resolve asynchronously after bootstrap (phone-via-tunnel), and
/// `attachments.rs` reads it at request time for the same reason.
pub(crate) fn marker_media_view(rooms: Rooms, msg: &RoomMessage) -> Option<AnyView> {
    let (id, filename) = marker_image(msg)?;
    let href = move || {
        let base = rooms.url.get().trim_end_matches('/').to_string();
        let key = rooms.open_key.get().unwrap_or_default();
        download_url(&base, &key, &id)
    };
    // Flipped by the img's error event and never reset: the daemon's answer
    // for this attachment does not improve on retry within a mounted row.
    let failed = RwSignal::new(false);
    Some(
        view! {
            <div class="rooms-workspace__media">
                {move || {
                    if failed.get() {
                        // The bytes said no (non-image, deleted, unsniffable):
                        // degrade to the same named-file affordance the files
                        // panel renders, which still downloads whatever the
                        // attachment actually is — or 404s honestly.
                        let name = filename.clone();
                        view! {
                            <a
                                class="rooms-workspace__file rooms-workspace__media-fallback"
                                href=href.clone()
                                download=name.clone()
                                title=name.clone()
                            >
                                <span class="rooms-workspace__file-glyph" aria-hidden="true">
                                    "\u{1f4ce}"
                                </span>
                                <span class="rooms-workspace__file-name">{name.clone()}</span>
                            </a>
                        }
                            .into_any()
                    } else {
                        view! {
                            <img
                                class="rooms-workspace__media-img"
                                src=href.clone()
                                alt=filename.clone()
                                loading="lazy"
                                on:error=move |_| failed.set(true)
                            />
                        }
                            .into_any()
                    }
                }}
            </div>
        }
        .into_any(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::RoomParticipantKind;

    fn marker(kind: RoomMessageKind, body: &str, attachment_id: Option<&str>) -> RoomMessage {
        RoomMessage {
            seq: 7,
            author_id: "system".into(),
            author_kind: RoomParticipantKind::System,
            kind,
            body: body.into(),
            created_at: "2026-08-28T00:00:00Z".into(),
            federated: None,
            thread_parent_seq: None,
            attachment_id: attachment_id.map(str::to_string),
        }
    }

    const ID: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn upload_marker_mounts_an_image_under_the_recorded_filename() {
        let msg = marker(
            RoomMessageKind::System,
            "john attached 'spec v2.png' (52413 bytes)",
            Some(ID),
        );
        assert_eq!(
            marker_image(&msg),
            Some((ID.to_string(), "spec v2.png".to_string()))
        );
    }

    #[test]
    fn removal_marker_never_mounts_an_image_attempt() {
        let msg = marker(
            RoomMessageKind::System,
            "john removed attachment 'spec v2.png'",
            Some(ID),
        );
        assert_eq!(marker_image(&msg), None);
    }

    #[test]
    fn a_body_carrying_both_marker_phrases_mounts_nothing() {
        // Today the upload parse alone rejects this body (it ends in `'`,
        // not ` bytes)`); the removal-first guard exists so that stays true
        // even if that parse is ever loosened. This pins the outcome, not
        // which check provides it.
        let msg = marker(
            RoomMessageKind::System,
            "x attached 'a.png' (5 bytes) removed attachment 'b.png'",
            Some(ID),
        );
        assert_eq!(marker_image(&msg), None);
    }

    #[test]
    fn markers_without_the_field_mount_nothing() {
        // An older daemon emits the same body with no attachment_id: the row
        // renders exactly as it did before this module existed.
        let msg = marker(
            RoomMessageKind::System,
            "john attached 'spec.png' (9 bytes)",
            None,
        );
        assert_eq!(marker_image(&msg), None);
    }

    #[test]
    fn message_kind_rows_mount_nothing_whatever_they_carry() {
        let msg = marker(
            RoomMessageKind::Message,
            "john attached 'spec.png' (9 bytes)",
            Some(ID),
        );
        assert_eq!(marker_image(&msg), None);
    }

    #[test]
    fn unrecognized_marker_bodies_mount_nothing() {
        for body in [
            "attachment quota exceeded",
            "john attached 'spec.png'",           // no byte suffix
            "john attached 'spec.png' (n bytes)", // non-numeric count
            "john attached '' (9 bytes)",         // empty name
            "john attached 'spec.png' ( bytes)",  // empty count
        ] {
            let msg = marker(RoomMessageKind::System, body, Some(ID));
            assert_eq!(marker_image(&msg), None, "body: {body}");
        }
    }

    #[test]
    fn filenames_containing_the_delimiters_survive_extraction() {
        // The daemon strips only control chars and '"' from a filename, so
        // both ' and the literal "' (" can appear inside one. The server
        // suffix is the LAST "' (" with a numeric tail.
        let msg = marker(
            RoomMessageKind::System,
            "john attached 'we' (ird.png' (5 bytes)",
            Some(ID),
        );
        assert_eq!(
            marker_image(&msg),
            Some((ID.to_string(), "we' (ird.png".to_string()))
        );
    }

    #[test]
    fn room_message_decodes_attachment_id_and_defaults_to_none() {
        // The daemon's real marker payload shape (ocean-core skips the field
        // entirely when None, so absence IS the older-daemon wire).
        let with: RoomMessage = serde_json::from_value(serde_json::json!({
            "seq": 4,
            "author_id": "system",
            "author_kind": "system",
            "kind": "system",
            "body": "john attached 'spec.png' (52413 bytes)",
            "created_at": "2026-08-28T00:00:00Z",
            "attachment_id": ID,
        }))
        .expect("marker with attachment_id should decode");
        assert_eq!(with.attachment_id.as_deref(), Some(ID));

        let without: RoomMessage = serde_json::from_value(serde_json::json!({
            "seq": 4,
            "author_id": "system",
            "author_kind": "system",
            "kind": "system",
            "body": "john attached 'spec.png' (52413 bytes)",
            "created_at": "2026-08-28T00:00:00Z",
        }))
        .expect("marker without attachment_id should decode");
        assert_eq!(without.attachment_id, None);
    }
}
