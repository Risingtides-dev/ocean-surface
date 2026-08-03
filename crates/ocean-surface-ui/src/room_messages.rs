//! Message-density derivation for the rooms timeline (daily-driver spec,
//! Slice 3 — @designer). Pure and presentation-agnostic: given adjacent
//! transcript entries, decide grouped rendering, conversation-gap headers,
//! compact system rows, and day separators.
//!
//! Deliberately NOT wired into markup yet: the timeline region of
//! `rooms_workspace.rs` is being restructured by the thread slice
//! (root-only timeline + reply rail), so the density model lands here as a
//! reviewed, test-locked contract that the post-merge timeline adopts
//! verbatim. Class names it maps to are already defined and waiting in the
//! stylesheets: `.rooms-workspace__msg--grouped`, `.rooms-workspace__day-separator`.
#![allow(dead_code)]

use crate::rooms::{RoomMessage, RoomMessageKind};

/// Same-author messages this close together (seconds) render grouped:
/// avatar/name once, tighter spacing. Slack/Discord convention: 5 minutes.
const GROUP_WINDOW_SECS: i64 = 300;

/// A silence longer than this (seconds) gets a timestamp header on the
/// next message: 15 minutes.
const GAP_HEADER_SECS: i64 = 900;

// ── Time parsing (no chrono; timestamps are daemon ISO-8601 UTC) ───────────

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parse `"YYYY-MM-DDTHH:MM:SS[.frac]Z"` to epoch seconds. `None` for
/// anything malformed — callers must degrade to un-grouped full rendering,
/// never guess.
fn parse_iso_epoch(ts: &str) -> Option<i64> {
    let b = ts.as_bytes();
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |s: &str| s.parse::<i64>().ok();
    let (y, mo, d) = (num(&ts[0..4])?, num(&ts[5..7])?, num(&ts[8..10])?);
    let (h, mi, s) = (num(&ts[11..13])?, num(&ts[14..16])?, num(&ts[17..19])?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3_600 + mi * 60 + s)
}

/// The `YYYY-MM-DD` day key of a message, `None` when unparseable.
fn day_key(msg: &RoomMessage) -> Option<&str> {
    let ts = msg.created_at.as_str();
    (parse_iso_epoch(ts).is_some()).then(|| &ts[0..10])
}

// ── Density decisions ──────────────────────────────────────────────────────

/// Whether `cur` renders as a compact single-line system row (join/leave/
/// system) instead of a full message card.
pub(crate) fn is_compact_system_row(msg: &RoomMessage) -> bool {
    !matches!(msg.kind, RoomMessageKind::Message)
}

/// Whether `cur` renders grouped under `prev` (avatar once, tight spacing):
/// both are real messages, same author identity, `cur` is 0..=5 minutes
/// after `prev`, and no day boundary sits between them. Unparseable or
/// out-of-order timestamps degrade to un-grouped — a wrong group is worse
/// than a missing one.
pub(crate) fn is_grouped(prev: &RoomMessage, cur: &RoomMessage) -> bool {
    if !matches!(prev.kind, RoomMessageKind::Message)
        || !matches!(cur.kind, RoomMessageKind::Message)
    {
        return false;
    }
    if prev.author_id != cur.author_id || prev.author_kind != cur.author_kind {
        return false;
    }
    match (
        parse_iso_epoch(&prev.created_at),
        parse_iso_epoch(&cur.created_at),
    ) {
        (Some(p), Some(c)) => {
            (0..=GROUP_WINDOW_SECS).contains(&(c - p)) && day_key(prev) == day_key(cur)
        }
        _ => false,
    }
}

/// Whether the silence before `cur` warrants a timestamp header
/// (> 15 minutes since `prev`). Unparseable timestamps: no header.
pub(crate) fn needs_gap_header(prev: &RoomMessage, cur: &RoomMessage) -> bool {
    match (
        parse_iso_epoch(&prev.created_at),
        parse_iso_epoch(&cur.created_at),
    ) {
        (Some(p), Some(c)) => c - p > GAP_HEADER_SECS,
        _ => false,
    }
}

/// Day-separator label to render before `cur`: its `YYYY-MM-DD` when it
/// opens the transcript or lands on a different day than `prev`.
/// (Humanizing "Today"/"Yesterday" is presentational and needs a client
/// clock — the adopting view layers that on top.)
pub(crate) fn day_separator_label(prev: Option<&RoomMessage>, cur: &RoomMessage) -> Option<String> {
    let cur_day = day_key(cur)?;
    match prev {
        None => Some(cur_day.to_string()),
        Some(p) => (day_key(p) != Some(cur_day)).then(|| cur_day.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::RoomParticipantKind;

    fn msg(seq: u64, author: &str, kind: RoomMessageKind, ts: &str) -> RoomMessage {
        RoomMessage {
            seq,
            author_id: author.to_string(),
            author_kind: RoomParticipantKind::Human,
            kind,
            body: "b".to_string(),
            created_at: ts.to_string(),
            federated: None,
        }
    }

    fn m(seq: u64, author: &str, ts: &str) -> RoomMessage {
        msg(seq, author, RoomMessageKind::Message, ts)
    }

    #[test]
    fn parse_iso_epoch_handles_day_and_bad_input() {
        assert_eq!(parse_iso_epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso_epoch("1970-01-02T00:00:01Z"), Some(86_401));
        assert_eq!(
            parse_iso_epoch("2026-07-29T12:00:00.123Z"),
            parse_iso_epoch("2026-07-29T12:00:00Z")
        );
        assert_eq!(parse_iso_epoch("garbage"), None);
        assert_eq!(parse_iso_epoch("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_iso_epoch(""), None);
    }

    #[test]
    fn groups_same_author_within_five_minutes() {
        let a = m(1, "u1", "2026-07-29T12:00:00Z");
        let b = m(2, "u1", "2026-07-29T12:05:00Z"); // exactly 300s: inclusive
        let c = m(3, "u1", "2026-07-29T12:05:01Z"); // 301s: full row
        assert!(is_grouped(&a, &b));
        assert!(!is_grouped(&a, &c));
    }

    #[test]
    fn never_groups_across_author_kind_or_disorder() {
        let a = m(1, "u1", "2026-07-29T12:00:00Z");
        assert!(!is_grouped(&a, &m(2, "u2", "2026-07-29T12:00:30Z")));
        assert!(!is_grouped(&a, &m(2, "u1", "2026-07-29T11:59:59Z"))); // out of order
        assert!(!is_grouped(
            &a,
            &msg(
                2,
                "u1",
                RoomMessageKind::ParticipantJoined,
                "2026-07-29T12:00:30Z"
            ),
        ));
        assert!(!is_grouped(&a, &m(2, "u1", "not-a-time")));
    }

    #[test]
    fn never_groups_across_midnight_even_within_window() {
        let a = m(1, "u1", "2026-07-29T23:58:00Z");
        let b = m(2, "u1", "2026-07-30T00:01:00Z"); // 180s but new day
        assert!(!is_grouped(&a, &b));
    }

    #[test]
    fn gap_header_after_fifteen_minutes_of_silence() {
        let a = m(1, "u1", "2026-07-29T12:00:00Z");
        assert!(!needs_gap_header(&a, &m(2, "u2", "2026-07-29T12:15:00Z"))); // exactly 900s: no
        assert!(needs_gap_header(&a, &m(2, "u2", "2026-07-29T12:15:01Z")));
        assert!(!needs_gap_header(&a, &m(2, "u2", "bad")));
    }

    #[test]
    fn day_separator_opens_transcript_and_marks_day_changes() {
        let a = m(1, "u1", "2026-07-29T23:00:00Z");
        let b = m(2, "u1", "2026-07-30T01:00:00Z");
        assert_eq!(
            day_separator_label(None, &a),
            Some("2026-07-29".to_string())
        );
        assert_eq!(
            day_separator_label(Some(&a), &b),
            Some("2026-07-30".to_string())
        );
        assert_eq!(
            day_separator_label(Some(&a), &m(3, "x", "2026-07-29T23:59:00Z")),
            None
        );
        assert_eq!(day_separator_label(Some(&a), &m(3, "x", "bad")), None);
    }

    #[test]
    fn system_rows_are_compact() {
        assert!(is_compact_system_row(&msg(
            1,
            "u",
            RoomMessageKind::ParticipantJoined,
            "t"
        )));
        assert!(is_compact_system_row(&msg(
            1,
            "u",
            RoomMessageKind::System,
            "t"
        )));
        assert!(!is_compact_system_row(&m(1, "u", "t")));
    }
}
