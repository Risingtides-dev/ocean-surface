//! Message-density derivation for the rooms timeline (daily-driver spec,
//! Slice 3 — @designer). Pure and presentation-agnostic: given adjacent
//! transcript entries, decide grouped rendering, conversation-gap headers,
//! compact system rows, and day separators.
//!
//! The timeline in `rooms_workspace.rs` consumes these helpers directly.
//! Class names they map to are defined in the stylesheets:
//! `.rooms-workspace__msg--grouped`, `.rooms-workspace__day-separator`.

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

/// Civil date for epoch seconds — the inverse of `days_from_civil`, same
/// algorithm. Used to name the day a local-shifted instant falls on, which is
/// not in general the day its UTC wire value spells.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ── The member's clock ─────────────────────────────────────────────────────
//
// The daemon writes `created_at` as RFC 3339 UTC — `to_rfc3339_opts(Nanos,
// true)` in ocean-os's `fmt_ts`, so always a `Z` — and the surface used to
// render bytes 11..16 of that string as the row's clock. That is the UTC
// hour, shown to the member as if it were theirs. West of Greenwich every
// timestamp read hours late; east, hours early; and a room busy across UTC
// midnight drew its day separator in the middle of the member's afternoon.
//
// Everything below takes the offset as an argument — minutes to ADD to UTC —
// so it stays pure and testable. The view supplies it per timestamp, which is
// also what makes a transcript spanning a DST change render each row against
// the offset that was actually in force.

/// Epoch seconds shifted into the viewer's zone. `None` for a wire value that
/// is not the canonical shape — callers degrade, never guess.
fn local_epoch(ts: &str, utc_offset_minutes: i64) -> Option<i64> {
    Some(parse_iso_epoch(ts)? + utc_offset_minutes * 60)
}

/// The member's wall clock for this instant, `HH:MM`, 24-hour. `None` when
/// the wire value is not canonical RFC 3339 — the caller shows the raw wire
/// string rather than inventing a time.
pub(crate) fn local_clock_time(ts: &str, utc_offset_minutes: i64) -> Option<String> {
    let secs = local_epoch(ts, utc_offset_minutes)?;
    let day_secs = secs.rem_euclid(86_400);
    Some(format!(
        "{:02}:{:02}",
        day_secs / 3_600,
        day_secs % 3_600 / 60
    ))
}

/// The `YYYY-MM-DD` the member would call this instant. `None` when
/// unparseable.
pub(crate) fn local_day_key(ts: &str, utc_offset_minutes: i64) -> Option<String> {
    let secs = local_epoch(ts, utc_offset_minutes)?;
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// The local day key of a message, `None` when unparseable.
///
/// `offset_for` is asked about THIS message's instant, never about a
/// neighbour's. Two rows minutes apart can sit on opposite sides of a DST
/// change: in New York `2026-11-01T04:30Z` is 00:30 EDT and
/// `2026-11-01T07:30Z` is 02:30 EST — the same local day under two different
/// offsets. Resolving the pair with either row's offset alone maps the other
/// to the wrong day and invents a separator between them.
fn day_key(msg: &RoomMessage, offset_for: &impl Fn(&str) -> i64) -> Option<String> {
    local_day_key(&msg.created_at, offset_for(&msg.created_at))
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
///
/// The elapsed-seconds half needs no offset; the day boundary does, and it is
/// the MEMBER'S midnight, not Greenwich's. Two messages a minute apart that
/// straddle UTC midnight are one conversation to a reader in New York and
/// must not be split into two headed rows.
///
/// `offset_for` is resolved per message, not once for the pair — see
/// `day_key`.
pub(crate) fn is_grouped(
    prev: &RoomMessage,
    cur: &RoomMessage,
    offset_for: impl Fn(&str) -> i64,
) -> bool {
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
            (0..=GROUP_WINDOW_SECS).contains(&(c - p))
                && day_key(prev, &offset_for) == day_key(cur, &offset_for)
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
///
/// The day is the MEMBER'S. A separator drawn on UTC days lands mid-afternoon
/// for a reader in New York and splits one evening's conversation in two,
/// while the real local midnight it should have marked passes unmarked.
///
/// `offset_for` is resolved per message, not once for the pair — see
/// `day_key`.
pub(crate) fn day_separator_label(
    prev: Option<&RoomMessage>,
    cur: &RoomMessage,
    offset_for: impl Fn(&str) -> i64,
) -> Option<String> {
    let cur_day = day_key(cur, &offset_for)?;
    match prev {
        None => Some(cur_day),
        Some(p) => (day_key(p, &offset_for).as_ref() != Some(&cur_day)).then_some(cur_day),
    }
}

/// Humanize a `YYYY-MM-DD` separator label against the client's current
/// day key: "Today", "Yesterday", or the date itself. Pure — the caller
/// supplies `today` (client clock is a view concern), so this stays
/// deterministic under test.
pub(crate) fn humanize_day_label(day: &str, today: &str) -> String {
    if day == today {
        return "Today".to_string();
    }
    match (
        parse_iso_epoch(&format!("{day}T00:00:00Z")),
        parse_iso_epoch(&format!("{today}T00:00:00Z")),
    ) {
        (Some(d), Some(t)) if t - d == 86_400 => "Yesterday".to_string(),
        _ => day.to_string(),
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
            thread_parent_seq: None,
            federated: None,
            attachment_id: None,
        }
    }

    fn m(seq: u64, author: &str, ts: &str) -> RoomMessage {
        msg(seq, author, RoomMessageKind::Message, ts)
    }

    /// America/New_York in summer: UTC-04:00. The offset every local test
    /// below is written against, because it is where the operator reads this
    /// transcript and because it is the sign that used to be wrong.
    const NEW_YORK_EDT: i64 = -240;
    /// Asia/Kolkata: UTC+05:30 — the half-hour offset that catches an
    /// implementation carrying whole hours.
    const KOLKATA: i64 = 330;
    /// Greenwich, where the old rendering was accidentally right.
    const UTC: i64 = 0;

    /// America/New_York across the 2026 autumn transition, as the browser
    /// would answer it: EDT (-240) up to 2026-11-01T06:00Z, EST (-300) after.
    /// The one offset a transcript cannot have a single value for.
    fn new_york_2026(ts: &str) -> i64 {
        match parse_iso_epoch(ts) {
            Some(secs) if secs >= parse_iso_epoch("2026-11-01T06:00:00Z").unwrap() => -300,
            _ => NEW_YORK_EDT,
        }
    }

    /// A zone that springs forward at LOCAL MIDNIGHT — Santiago's convention,
    /// and the only shape where a DST change can land inside the five-minute
    /// grouping window. New York's 02:00 transition cannot: five minutes
    /// either side of it is nowhere near a local midnight, so both offsets
    /// name the same day and grouping looks correct even when it is computed
    /// wrongly. Here -240 becomes -180 at 2026-09-06T04:00Z, which is 00:00
    /// local jumping to 01:00 local.
    fn springs_forward_at_midnight(ts: &str) -> i64 {
        match parse_iso_epoch(ts) {
            Some(secs) if secs >= parse_iso_epoch("2026-09-06T04:00:00Z").unwrap() => -180,
            _ => -240,
        }
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
        assert!(is_grouped(&a, &b, |_: &str| UTC));
        assert!(!is_grouped(&a, &c, |_: &str| UTC));
        // Elapsed time is offset-invariant: the window says the same thing
        // wherever it is read.
        assert!(is_grouped(&a, &b, |_: &str| NEW_YORK_EDT));
        assert!(!is_grouped(&a, &c, |_: &str| KOLKATA));
    }

    #[test]
    fn never_groups_across_author_kind_or_disorder() {
        let a = m(1, "u1", "2026-07-29T12:00:00Z");
        assert!(!is_grouped(
            &a,
            &m(2, "u2", "2026-07-29T12:00:30Z"),
            |_: &str| UTC
        ));
        assert!(!is_grouped(
            &a,
            &m(2, "u1", "2026-07-29T11:59:59Z"),
            |_: &str| UTC
        )); // out of order
        assert!(!is_grouped(
            &a,
            &msg(
                2,
                "u1",
                RoomMessageKind::ParticipantJoined,
                "2026-07-29T12:00:30Z"
            ),
            |_: &str| UTC,
        ));
        assert!(!is_grouped(&a, &m(2, "u1", "not-a-time"), |_: &str| UTC));
    }

    #[test]
    fn never_groups_across_midnight_even_within_window() {
        let a = m(1, "u1", "2026-07-29T23:58:00Z");
        let b = m(2, "u1", "2026-07-30T00:01:00Z"); // 180s but new day
        assert!(!is_grouped(&a, &b, |_: &str| UTC));
    }

    #[test]
    fn the_midnight_that_splits_a_group_is_the_member_s_own() {
        // 23:58Z and 00:01Z: two UTC days, one New York evening (19:58 and
        // 20:01 the same afternoon). One conversation, so one headed row.
        let a = m(1, "u1", "2026-07-29T23:58:00Z");
        let b = m(2, "u1", "2026-07-30T00:01:00Z");
        assert!(!is_grouped(&a, &b, |_: &str| UTC));
        assert!(is_grouped(&a, &b, |_: &str| NEW_YORK_EDT));

        // And the reverse: one UTC day, two New York days. 03:58Z is 23:58
        // the previous evening; 04:01Z is 00:01 the next morning.
        let c = m(3, "u1", "2026-07-29T03:58:00Z");
        let d = m(4, "u1", "2026-07-29T04:01:00Z");
        assert!(is_grouped(&c, &d, |_: &str| UTC));
        assert!(!is_grouped(&c, &d, |_: &str| NEW_YORK_EDT));
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
            day_separator_label(None, &a, |_: &str| UTC),
            Some("2026-07-29".to_string())
        );
        assert_eq!(
            day_separator_label(Some(&a), &b, |_: &str| UTC),
            Some("2026-07-30".to_string())
        );
        assert_eq!(
            day_separator_label(Some(&a), &m(3, "x", "2026-07-29T23:59:00Z"), |_: &str| UTC),
            None
        );
        assert_eq!(
            day_separator_label(Some(&a), &m(3, "x", "bad"), |_: &str| UTC),
            None
        );
    }

    #[test]
    fn the_day_a_separator_marks_is_the_member_s_day() {
        // The pair that used to draw a separator in the middle of a New York
        // evening: 23:00Z and 01:00Z are two UTC days and one local day.
        let a = m(1, "u1", "2026-07-29T23:00:00Z"); // 19:00 local, 07-29
        let b = m(2, "u1", "2026-07-30T01:00:00Z"); // 21:00 local, still 07-29
        assert_eq!(
            day_separator_label(Some(&a), &b, |_: &str| UTC),
            Some("2026-07-30".to_string()),
        );
        assert_eq!(
            day_separator_label(Some(&a), &b, |_: &str| NEW_YORK_EDT),
            None
        );

        // And the local midnight the UTC reading walked straight past.
        let c = m(3, "u1", "2026-07-29T03:00:00Z"); // 23:00 local, 07-28
        let d = m(4, "u1", "2026-07-29T05:00:00Z"); // 01:00 local, 07-29
        assert_eq!(day_separator_label(Some(&c), &d, |_: &str| UTC), None);
        assert_eq!(
            day_separator_label(Some(&c), &d, |_: &str| NEW_YORK_EDT),
            Some("2026-07-29".to_string()),
        );

        // The label a member sees names their day, not the wire's: the row
        // that opens the transcript is dated where they are.
        assert_eq!(
            day_separator_label(None, &c, |_: &str| NEW_YORK_EDT),
            Some("2026-07-28".to_string()),
        );
        // East of Greenwich the shift runs the other way, and by a half hour.
        assert_eq!(
            day_separator_label(None, &m(5, "u1", "2026-07-29T19:45:00Z"), |_: &str| KOLKATA),
            Some("2026-07-30".to_string()),
        );
    }

    /// A pair that straddles a DST change has TWO offsets, and resolving it
    /// with either row's alone maps the other row to the wrong day.
    ///
    /// Codex caught this on #201: the view derived one offset from the CURRENT
    /// row and both `day_separator_label` and `is_grouped` applied it to the
    /// predecessor too. In New York on 2026-11-01, `04:30Z` is 00:30 EDT and
    /// `07:30Z` is 02:30 EST — the same local day, three UTC hours apart,
    /// under two different offsets. Under the current row's `-300`, the
    /// predecessor maps to Oct 31 and a day separator appears between two rows
    /// of the same local morning.
    #[test]
    fn a_dst_change_between_two_rows_does_not_invent_a_day() {
        let before = m(1, "u1", "2026-11-01T04:30:00Z"); // 00:30 EDT, Nov 1
        let after = m(2, "u1", "2026-11-01T07:30:00Z"); // 02:30 EST, Nov 1

        // Each row against its own offset: one local day, so no separator.
        assert_eq!(new_york_2026(&before.created_at), -240);
        assert_eq!(new_york_2026(&after.created_at), -300);
        assert_eq!(
            day_separator_label(Some(&before), &after, new_york_2026),
            None
        );

        // The defect, stated as the thing that must not happen: the current
        // row's offset applied to both invents the boundary.
        assert_eq!(
            day_separator_label(Some(&before), &after, |_: &str| -300),
            Some("2026-11-01".to_string()),
            "this is the WRONG answer the per-pair offset produced — kept so \
             the test fails loudly if the resolver stops being per message",
        );

        // A real boundary is still found when the rows genuinely span one.
        let evening = m(3, "u1", "2026-11-01T03:30:00Z"); // 23:30 EDT, Oct 31
        assert_eq!(
            day_separator_label(Some(&evening), &before, new_york_2026),
            Some("2026-11-01".to_string()),
        );

        // New York's fall-back sits at 02:00 local, so a five-minute window
        // around it never reaches a local midnight and BOTH offsets name the
        // same day. Grouping there is right either way — which is exactly why
        // it is not evidence.
        let last_edt = m(4, "u1", "2026-11-01T05:58:00Z"); // 01:58 EDT
        let first_est = m(5, "u1", "2026-11-01T06:01:00Z"); // 01:01 EST
        assert!(is_grouped(&last_edt, &first_est, new_york_2026));

        // A zone that springs forward AT local midnight is where the same
        // defect reaches `is_grouped`. Three minutes apart, genuinely two
        // local days, so they must not group — but under the current row's
        // -180 the predecessor moves to 00:58 of the SAME day and they do.
        let before_jump = m(6, "u1", "2026-09-06T03:58:00Z"); // 23:58, Sep 5
        let after_jump = m(7, "u1", "2026-09-06T04:01:00Z"); // 01:01, Sep 6
        assert!(!is_grouped(
            &before_jump,
            &after_jump,
            springs_forward_at_midnight
        ));
        assert!(
            is_grouped(&before_jump, &after_jump, |_: &str| -180),
            "the WRONG answer a per-pair offset produces — kept so this test \
             fails loudly if the resolver stops being per message",
        );
        assert_eq!(
            day_separator_label(Some(&before_jump), &after_jump, springs_forward_at_midnight),
            Some("2026-09-06".to_string()),
        );
    }

    #[test]
    fn local_clock_reads_the_member_s_wall_time() {
        let ts = "2026-07-29T03:43:12Z";
        assert_eq!(local_clock_time(ts, UTC).as_deref(), Some("03:43"));
        // The whole defect in one line: 03:43Z is 23:43 the previous evening
        // in New York, and the surface used to print "03:43".
        assert_eq!(local_clock_time(ts, NEW_YORK_EDT).as_deref(), Some("23:43"));
        // Half-hour zones are not a rounding of whole ones.
        assert_eq!(local_clock_time(ts, KOLKATA).as_deref(), Some("09:13"));
        // Fractional seconds are what the daemon actually sends
        // (`to_rfc3339_opts(Nanos, true)`), and must not shift the clock.
        assert_eq!(
            local_clock_time("2026-07-29T03:43:12.987654321Z", NEW_YORK_EDT).as_deref(),
            Some("23:43"),
        );
        // Midnight and the last minute of the day, both directions.
        assert_eq!(
            local_clock_time("2026-07-29T04:00:00Z", NEW_YORK_EDT).as_deref(),
            Some("00:00"),
        );
        assert_eq!(
            local_clock_time("2026-07-29T03:59:00Z", NEW_YORK_EDT).as_deref(),
            Some("23:59"),
        );
        // Nothing is invented for a wire value the parser does not accept.
        assert_eq!(local_clock_time("bad", NEW_YORK_EDT), None);
        assert_eq!(local_clock_time("", NEW_YORK_EDT), None);
        assert_eq!(local_clock_time("2026-06-05T12", NEW_YORK_EDT), None);
        assert_eq!(local_clock_time("2026-06-05 12:34:56Z", NEW_YORK_EDT), None);
        // Multi-byte input must not panic or index into a char boundary.
        assert_eq!(
            local_clock_time("\u{ff12}\u{ff10}\u{ff12}\u{ff16}-07-25T03:43:12Z", UTC),
            None,
        );
    }

    #[test]
    fn local_day_key_crosses_the_boundary_in_both_directions() {
        // Crossing back over midnight, including over a month end.
        assert_eq!(
            local_day_key("2026-08-01T02:00:00Z", NEW_YORK_EDT).as_deref(),
            Some("2026-07-31"),
        );
        // And forward, including over a year end.
        assert_eq!(
            local_day_key("2026-12-31T19:00:00Z", KOLKATA).as_deref(),
            Some("2027-01-01"),
        );
        // A leap day is a day like any other, in either direction.
        assert_eq!(
            local_day_key("2028-03-01T03:00:00Z", NEW_YORK_EDT).as_deref(),
            Some("2028-02-29"),
        );
        assert_eq!(local_day_key("bad", NEW_YORK_EDT), None);
    }

    #[test]
    fn civil_dates_round_trip_through_the_epoch() {
        // `civil_from_days` is the inverse of the `days_from_civil` the day
        // keys were already built on; a disagreement between them would
        // mis-date rows only near the boundaries these keys are for.
        for (y, m, d) in [
            (1970, 1, 1),
            (1999, 12, 31),
            (2000, 3, 1),
            (2026, 7, 29),
            (2028, 2, 29),
            (2100, 3, 1),
        ] {
            assert_eq!(civil_from_days(days_from_civil(y, m, d)), (y, m, d));
        }
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

    #[test]
    fn humanized_day_labels_today_yesterday_else_date() {
        assert_eq!(humanize_day_label("2026-07-29", "2026-07-29"), "Today");
        assert_eq!(humanize_day_label("2026-07-28", "2026-07-29"), "Yesterday");
        // month boundary yesterday
        assert_eq!(humanize_day_label("2026-06-30", "2026-07-01"), "Yesterday");
        assert_eq!(humanize_day_label("2026-07-20", "2026-07-29"), "2026-07-20");
        // future or garbage: plain date, never a wrong "Yesterday"
        assert_eq!(humanize_day_label("2026-07-30", "2026-07-29"), "2026-07-30");
        assert_eq!(humanize_day_label("bad", "2026-07-29"), "bad");
    }
}
