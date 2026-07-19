//! TASK-52 — open document flow: de-bubbled assistant turns + two-measure layout.
//!
//! Source-assertion tests (same style as mobile_composer_regressions.rs /
//! dead_selector_removal.rs) that pin the de-bubbling contract against the
//! shipped styles/transcript.css so it can't silently regress:
//!   1. The ASSISTANT turn body carries NO bubble chrome — no background,
//!      border, radius, or shadow — and spans the full transcript rail.
//!   2. PROSE keeps a readable measure: `.block--text` inside the assistant
//!      body is capped at 74ch (the two-measure prose column).
//!   3. RENDERED COMPONENTS break out to the full rail: the component-host
//!      breakout rule sets width:100% on the `.component-*` roots.
//!   4. USER messages keep their compact right-aligned bubble (background +
//!      radius + shadow) — the deliberate open/bubbled voice contrast.
//!   5. The specular streaming-breath (card chrome) is gone.
//!   6. The TASK-47 md-block flush rules survive the restructure.

fn transcript_css() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../styles/transcript.css");
    std::fs::read_to_string(path).expect("read styles/transcript.css")
}

/// Strip `/* … */` comments so property assertions never match commentary.
fn strip_css_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(open) = rest.find("/*") {
        out.push_str(&rest[..open]);
        if let Some(close) = rest[open + 2..].find("*/") {
            rest = &rest[open + 2 + close + 2..];
        } else {
            break;
        }
    }
    out.push_str(rest);
    out
}

/// The declaration block for the rule whose selector is EXACTLY `selector`
/// (the next non-whitespace char after the selector is `{`). Distinguishes
/// `.turn--assistant .turn__body` from `.turn--assistant .turn__body .block--text`.
fn rule_body_exact(css: &str, selector: &str) -> String {
    let css = strip_css_comments(css);
    let bytes = css.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = css[from..].find(selector) {
        let at = from + rel;
        let after = &css[at + selector.len()..];
        let trimmed = after.trim_start();
        if trimmed.starts_with('{') {
            let open = at + selector.len() + (after.len() - trimmed.len());
            return brace_body(bytes, open);
        }
        from = at + selector.len();
    }
    panic!("no rule with exact selector `{selector}` in transcript.css");
}

/// The declaration block that `needle` participates in — finds `needle`, then
/// the next `{` and its matching `}`. Works for grouped (comma-separated)
/// selectors where the brace does not immediately follow the needle.
fn decl_block_containing(css: &str, needle: &str) -> String {
    let css = strip_css_comments(css);
    let bytes = css.as_bytes();
    let at = css
        .find(needle)
        .unwrap_or_else(|| panic!("selector `{needle}` not found in transcript.css"));
    let open = at
        + css[at..]
            .find('{')
            .expect("selector has no declaration block");
    brace_body(bytes, open)
}

/// Inner text of the brace-matched block starting at `open` (index of `{`).
fn brace_body(bytes: &[u8], open: usize) -> String {
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return String::from_utf8_lossy(&bytes[open + 1..i]).into_owned();
                }
            }
            _ => {}
        }
        i += 1;
    }
    panic!("unbalanced braces from offset {open}");
}

// ---- 1. Assistant turn is de-bubbled --------------------------------------

#[test]
fn assistant_turn_body_carries_no_bubble_chrome() {
    let css = transcript_css();
    let body = rule_body_exact(&css, ".turn--assistant .turn__body");
    for decl in ["background: none", "border: none", "box-shadow: none"] {
        assert!(
            body.contains(decl),
            "assistant turn body must drop bubble chrome (`{decl}`) — TASK-52 \
             de-bubble; rule body was:\n{body}",
        );
    }
    assert!(
        body.contains("border-radius: 0"),
        "assistant turn body must have border-radius:0 (no rounded card) — TASK-52",
    );
    // Must NOT re-introduce the old card fills/edges.
    assert!(
        !body.contains("var(--bg-raised)") && !body.contains("var(--elev-1)"),
        "assistant turn body must not carry the old --bg-raised fill / --elev-1 \
         shadow — the card is gone (TASK-52)",
    );
}

#[test]
fn assistant_turn_body_spans_full_rail() {
    let css = transcript_css();
    let body = rule_body_exact(&css, ".turn--assistant .turn__body");
    assert!(
        body.contains("width: 100%") && body.contains("max-width: 100%"),
        "assistant turn body must span the full transcript rail (width:100%; \
         max-width:100%) so components can break out to full measure — TASK-52",
    );
    assert!(
        !body.contains("min(74ch"),
        "the 74ch cap must move OFF the turn body onto the prose block — TASK-52",
    );
}

// ---- 2. Prose keeps a readable measure ------------------------------------

#[test]
fn assistant_prose_keeps_readable_measure() {
    let css = transcript_css();
    let body = rule_body_exact(&css, ".turn--assistant .turn__body .block--text");
    assert!(
        body.contains("max-width: 74ch"),
        "assistant prose (.block--text) must keep the 74ch readable measure — \
         the prose column of the two-measure layout (TASK-52); rule body:\n{body}",
    );
    // Prose stays flush on the open surface — no bubble chrome of its own.
    assert!(
        body.contains("padding: 0") && body.contains("background: none"),
        "assistant prose must sit flush on the de-bubbled surface (padding:0; \
         background:none) — TASK-52",
    );
}

// ---- 3. Components break out to full measure ------------------------------

#[test]
fn rendered_components_break_out_to_full_measure() {
    let css = transcript_css();
    // Dashboard is a representative member of the full-measure breakout group.
    let block = decl_block_containing(&css, ".turn--assistant .turn__body .component-dashboard");
    assert!(
        block.contains("width: 100%") && block.contains("max-width: 100%"),
        "the component-host breakout rule must set width:100%; max-width:100% so \
         rendered components span the full transcript rail — the heart of \
         TASK-52; declaration block:\n{block}",
    );
    // The breakout group must cover the major dashboards/tables/etc., not just
    // one host — assert several representative members share the rule.
    let stripped = strip_css_comments(&css);
    for host in [
        ".turn--assistant .turn__body .component-table",
        ".turn--assistant .turn__body .component-kanban",
        ".turn--assistant .turn__body .component-stats",
        ".turn--assistant .turn__body .component-form",
    ] {
        assert!(
            stripped.contains(host),
            "the full-measure breakout must include `{host}` — TASK-52",
        );
    }
    // The old flattening (chrome-stripping) of components is gone: components
    // regain their native card chrome now that the parent card is removed.
    assert!(
        !block.contains("box-shadow: none") && !block.contains("padding: 0"),
        "components must no longer be flattened (no box-shadow:none / padding:0 \
         in the breakout rule) — they regain native chrome and breathe (TASK-52)",
    );
}

// ---- 4. User bubble is unchanged ------------------------------------------

#[test]
fn user_turn_body_keeps_its_bubble() {
    let css = transcript_css();
    let body = rule_body_exact(&css, ".turn--user .turn__body");
    for decl in [
        "background: var(--sheen-soft), var(--bg-elevated)",
        "border-radius: var(--radius-lg)",
        "box-shadow: var(--elev-1)",
    ] {
        assert!(
            body.contains(decl),
            "user turns must KEEP their compact bubble (`{decl}`) — the deliberate \
             open-assistant / bubbled-user voice contrast (TASK-52); rule body:\n{body}",
        );
    }
    // Right-aligned column is what makes it read as a chat bubble.
    let col = rule_body_exact(&css, ".turn--user");
    assert!(
        col.contains("align-items: flex-end"),
        "user turns must stay right-aligned (align-items:flex-end) — TASK-52",
    );
}

// ---- 5. Specular streaming-breath (card chrome) is gone -------------------

#[test]
fn specular_streaming_breath_is_removed() {
    let css = transcript_css();
    assert!(
        !css.contains("ocean-specular-breath"),
        "the specular streaming-breath keyframe/animation is card chrome and must \
         be gone with the de-bubbled turn — the live-activity row owns the live \
         signal now (TASK-52)",
    );
    assert!(
        !css.contains(".turn--assistant.is-streaming .turn__body"),
        "no streaming-breath rule may target the de-bubbled turn body (TASK-52)",
    );
}

// ---- 6. TASK-47 md-block flush rules survive ------------------------------

#[test]
fn task47_md_block_flush_rules_survive() {
    let css = strip_css_comments(&transcript_css());
    assert!(
        css.contains(".md-block") && css.contains("display: contents"),
        "the TASK-47 display:contents .md-block host must survive the de-bubble \
         restructure (TASK-52 must not regress streaming markdown flushing)",
    );
    assert!(
        css.contains(".block--text > .md-block:first-child > :first-child")
            && css.contains(".block--text > .md-block:last-child > :last-child"),
        "the TASK-47 first/last md-block flush selectors must survive — they stay \
         anchored on .block--text > .md-block (unchanged structure) (TASK-52)",
    );
}
