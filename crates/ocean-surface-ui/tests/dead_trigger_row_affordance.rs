//! A permanently-dead wake-trigger row must not read as clickable.
//!
//! Source-assertion tests (the same style as mobile_composer_regressions.rs)
//! over styles/rooms-workspace.css. `.rooms-workspace__trigger` is a `<label>`,
//! so `input:disabled { cursor: not-allowed }` reaches the checkbox and nothing
//! else — the label text and its `__trigger-note` keep whatever the label's own
//! rules give them. That was harmless while the only disabled state was one
//! policy round-trip long, but `trigger_row_dead_here` now holds a row disabled
//! forever in rooms where its flag can never fire, and a permanent control that
//! brightens under a pointer cursor invites a click it will never honour.
//!
//! Only the affordance is pinned here. The `opacity: 0.45` fade is the signal
//! that the row is disabled at all and must survive.
//!
//! **The grip is a PROPERTY, not a rule body.** The live-hover assertion used to
//! read `.rooms-workspace__trigger:hover:not(:has(input:disabled)){color:var(--fg);}`
//! off the whitespace-stripped sheet — closing brace included, so it pinned that
//! rule as being EXACTLY one declaration long. Any second declaration added to it,
//! however unrelated to hover colour, took the guard down with it: a designer
//! adding `text-decoration` to a live row's hover would have been told the dead
//! row's affordance had regressed. It now goes through `rule_body` and asks for the
//! one declaration it actually needs, which is what the header above says it is
//! pinning. The other test in this file already worked this way; this one did not.
//!
//! Measured against this tree, one mutation per run of `cargo test -p
//! ocean-surface-ui --test dead_trigger_row_affordance`, the sheet restored between
//! each. Only the hover test's verdict is listed; its sibling stayed green
//! throughout, which is what makes each row attributable.
//!
//! | Mutation to `styles/rooms-workspace.css` | Old assert | New assert |
//! |---|---|---|
//! | none — the tree as it stands | green | green |
//! | `text-decoration: underline` added to the guarded rule | **RED** (the bug) | green |
//! | `color: var(--fg)` removed from it | RED | **RED** `got \`text-decoration:underline;\`` |
//! | `color: var(--fg)` changed to `var(--fg-2)` | RED | **RED** `got \`color:var(--fg-2);\`` |
//! | the guarded rule deleted outright | RED | **RED** `no rule for \`…:not(:has(input:disabled))\`` |
//! | `:not(:has(input:disabled))` dropped from the prelude | RED | **RED** at the unguarded-`:hover` assert |
//! | `color` → `background-color`, same value | RED | **RED** `got \`background-color:var(--fg);\`` |
//!
//! So the loosened grip costs nothing: every way of removing or changing the
//! protected declaration still reds, and only the false positive is gone.
//!
//! That last row is why the assert splits on `;` instead of asking `contains`
//! for `"color:var(--fg);"`. A substring test is satisfied by
//! `background-color:var(--fg);` and `border-color:var(--fg);` — both of which
//! leave the row's TEXT exactly as dim as it was, so the guard would have gone
//! green on the one change it exists to catch. Measured both ways on the same
//! mutated sheet: the substring form passed, the split form reds with the body
//! in the message. Caught in review on this PR, not by the mutation set above,
//! which is the honest note to leave — a row this table did not think to try.

fn rooms_workspace_css() -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../styles/rooms-workspace.css"
    );
    std::fs::read_to_string(path).expect("read styles/rooms-workspace.css")
}

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

fn css_without_whitespace(css: &str) -> String {
    css.chars().filter(|char| !char.is_whitespace()).collect()
}

/// Declarations of the first rule whose normalized prelude ends with `selector`.
/// Braces are single-byte ASCII, so byte scanning is safe over multi-byte text.
fn rule_body(normalized: &str, selector: &str) -> String {
    let needle = format!("{selector}{{");
    let at = normalized
        .find(&needle)
        .unwrap_or_else(|| panic!("no rule for `{selector}`"));
    let open = at + needle.len();
    let end = normalized[open..]
        .find('}')
        .unwrap_or_else(|| panic!("unterminated rule for `{selector}`"));
    normalized[open..open + end].to_string()
}

#[test]
fn a_dead_trigger_row_does_not_brighten_on_hover() {
    let normalized = css_without_whitespace(&strip_css_comments(&rooms_workspace_css()));
    assert!(
        !normalized.contains(".rooms-workspace__trigger:hover{"),
        "the trigger row's hover lift must stay guarded — an unguarded \
         `:hover` brightens rows that are permanently disabled",
    );
    let body = rule_body(
        &normalized,
        ".rooms-workspace__trigger:hover:not(:has(input:disabled))",
    );
    // Split on `;` rather than `contains`: the body is whitespace-stripped, so
    // each piece IS one declaration and `==` gets the boundary for free.
    // `contains("color:var(--fg);")` would have been satisfied by
    // `background-color:var(--fg);` or `border-color:var(--fg);` — either of
    // which leaves the row's TEXT exactly as dim as it was, which is the thing
    // this test is about.
    assert!(
        body.split(';').any(|decl| decl == "color:var(--fg)"),
        "a live trigger row still has to brighten on hover, got `{body}`",
    );
}

#[test]
fn a_dead_trigger_row_drops_the_pointer_cursor_but_keeps_the_fade() {
    let normalized = css_without_whitespace(&strip_css_comments(&rooms_workspace_css()));
    let body = rule_body(&normalized, ".rooms-workspace__trigger:has(input:disabled)");
    assert!(
        body.contains("cursor:default;"),
        "a disabled row must withdraw the label's pointer cursor, got `{body}`",
    );
    assert!(
        body.contains("opacity:0.45;"),
        "the disabled fade is the row's only remaining signal, got `{body}`",
    );
    assert!(
        !body.contains("pointer-events:none"),
        "`pointer-events: none` would suppress the disabled checkbox's own \
         `not-allowed` cursor along with the label's, got `{body}`",
    );
}
