//! TASK-42 — mobile component reflow regressions.
//!
//! Source-assertion tests (same style as mobile_composer_regressions.rs /
//! voice_realtime_regressions.rs) that pin the reflow behavior of the agent
//! component system in styles/components.css so it can't silently regress:
//!   1. Fixed multi-column dashboards stack to a single column at phone widths
//!      (they must reflow, not horizontally scroll the page).
//!   2. Component tap targets clear the repo's 44px floor under a
//!      `@media (pointer: coarse)` scope — desktop density stays untouched.
//!   3. The already-correct table scroll, kanban scroll, and stat-card wrap
//!      behavior stays in place (verify, per the contract — do not reimplement).

fn read_components() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../styles/components.css");
    std::fs::read_to_string(path).expect("read styles/components.css")
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

/// Bodies of every `@media` block whose prelude starts with `needle`,
/// brace-matched so nested rules are captured whole. Braces are single-byte
/// ASCII, so byte scanning is safe even with multi-byte content elsewhere.
fn media_blocks(css: &str, needle: &str) -> Vec<String> {
    let css = strip_css_comments(css);
    let bytes = css.as_bytes();
    let mut blocks = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = css[from..].find(needle) {
        let at = from + rel;
        let Some(brace_rel) = css[at..].find('{') else {
            break;
        };
        let open = at + brace_rel;
        let mut depth = 0usize;
        let mut end = None;
        let mut i = open;
        while i < bytes.len() {
            match bytes[i] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        match end {
            Some(e) => {
                blocks.push(css[open + 1..e].to_string());
                from = e + 1;
            }
            None => break,
        }
    }
    blocks
}

fn coarse_blocks(css: &str) -> Vec<String> {
    media_blocks(css, "@media (pointer: coarse)")
}

/// Split a (media) block body into its `(selectors, body)` rules,
/// brace-matched so a rule's own body is never mistaken for a rule boundary.
fn rules_in(block: &str) -> Vec<(String, String)> {
    let bytes = block.as_bytes();
    let mut rules = Vec::new();
    let mut i = 0usize;
    let mut sel_start = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let selectors = block[sel_start..i].trim().to_string();
            let body_start = i + 1;
            let mut depth = 1usize;
            let mut j = body_start;
            while j < bytes.len() && depth > 0 {
                match bytes[j] {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            let body_end = j.saturating_sub(1);
            rules.push((selectors, block[body_start..body_end].to_string()));
            i = j;
            sel_start = i;
        } else {
            i += 1;
        }
    }
    rules
}

/// The px value of `prop` inside `body`, if present as `prop: Npx`.
fn px_value(body: &str, prop: &str) -> Option<u32> {
    let at = body.find(prop)?;
    let after = &body[at + prop.len()..];
    let colon = after.find(':')?;
    let value: String = after[colon + 1..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    value.parse::<u32>().ok()
}

/// The px value of `prop` applied to a rule whose selector list contains
/// `selector`, searched only inside `@media (pointer: coarse)` blocks.
fn coarse_prop_px(css: &str, selector: &str, prop: &str) -> Option<u32> {
    for block in coarse_blocks(css) {
        for (selectors, body) in rules_in(&block) {
            if selectors.contains(selector) {
                if let Some(px) = px_value(&body, prop) {
                    return Some(px);
                }
            }
        }
    }
    None
}

// ---- 1. Dashboard stacking ------------------------------------------------

#[test]
fn multi_column_dashboard_stacks_to_single_column_at_narrow_width() {
    // A fixed multi-column dashboard must collapse to one column at phone
    // width so it reflows instead of scrolling the page body horizontally.
    // The template is author-supplied inline, so the override needs !important.
    let css = read_components();
    let stacked = media_blocks(&css, "@media (max-width: 560px)")
        .iter()
        .flat_map(|block| rules_in(block))
        .any(|(selectors, body)| {
            selectors.contains(".component-dashboard")
                && body.contains("grid-template-columns")
                && body.replace(' ', "").contains("minmax(0,1fr)")
                && body.contains("!important")
        });
    assert!(
        stacked,
        "styles/components.css must force .component-dashboard to \
         `grid-template-columns: minmax(0, 1fr) !important` inside a narrow \
         `@media (max-width: 560px)` block so fixed multi-column dashboards \
         stack to one column instead of horizontally scrolling (TASK-42)",
    );
}

#[test]
fn dashboard_stacking_preserves_the_video_auto_fit_override() {
    // The stacking rule must exclude video dashboards so the existing
    // `:has(.block--video)` auto-fit override stays authoritative.
    let css = strip_css_comments(&read_components());
    // Video override still present, untouched.
    assert!(
        css.contains(".component-dashboard:has(.block--video)")
            && css
                .replace(' ', "")
                .contains("repeat(auto-fit,minmax(280px,1fr))"),
        "the .component-dashboard:has(.block--video) auto-fit override must \
         stay intact (TASK-42 must not disturb the video path)",
    );
    // Stacking rule scopes itself away from video dashboards.
    let excludes_video = media_blocks(&css, "@media (max-width: 560px)")
        .iter()
        .flat_map(|block| rules_in(block))
        .any(|(selectors, _)| {
            selectors.contains(".component-dashboard")
                && selectors.contains(":not(:has(.block--video))")
        });
    assert!(
        excludes_video,
        "the narrow-width dashboard-stacking rule must be scoped \
         `.component-dashboard:not(:has(.block--video))` so it does not \
         override the video auto-fit path",
    );
}

// ---- 2. Coarse-pointer tap-target floors ----------------------------------

#[test]
fn component_tap_targets_clear_44px_under_coarse_pointer() {
    // Every non-table interactive control a touch user hits must report a
    // >= 44px min-height under @media (pointer: coarse). Folds finding 5
    // (.filetree__row was ~25px effective height).
    let css = read_components();
    let targets = [
        ".filetree__row",
        ".form-field__input",
        "select.form-field__input",
        ".form-submit",
        ".kanban-card",
        ".confirm__btn",
    ];
    for selector in targets {
        let px = coarse_prop_px(&css, selector, "min-height").unwrap_or_else(|| {
            panic!(
                "styles/components.css must set {selector} min-height inside a \
                 @media (pointer: coarse) block (TASK-42 44px tap floor)"
            )
        });
        assert!(
            px >= 44,
            "{selector} must clear the 44px coarse-pointer tap floor; found {px}px",
        );
    }
}

#[test]
fn data_table_row_uses_height_not_min_height_for_its_44px_floor() {
    // .data-table is a real <table> with border-collapse; CSS table-cell layout
    // ignores min-height, so the row floor must use `height` (which acts as a
    // minimum for table cells — they grow past it, never shrink below).
    let css = read_components();
    assert!(
        coarse_prop_px(&css, ".data-table__row td", "min-height").is_none(),
        "the .data-table__row td floor must NOT use min-height — table-cell \
         layout ignores it, making the tap floor a silent no-op (TASK-42)",
    );
    let px = coarse_prop_px(&css, ".data-table__row td", "height").unwrap_or_else(|| {
        panic!(
            "styles/components.css must set .data-table__row td `height` inside \
             a @media (pointer: coarse) block so table rows clear the tap floor"
        )
    });
    assert!(
        px >= 44,
        ".data-table__row td must clear the 44px coarse-pointer tap floor; found {px}px",
    );
}

#[test]
fn coarse_tap_floor_does_not_change_desktop_filetree_density() {
    // Finding 5's correction: the floor must live ONLY under coarse pointer —
    // the base .filetree__row rule must not gain a desktop min-height, which
    // would break its intentionally compact desktop density.
    let css = strip_css_comments(&read_components());
    // Isolate the base (non-media) .filetree__row rule by cutting the file at
    // the first `@media` — base rules precede every media block in this file.
    let base = css.split("@media").next().unwrap_or(&css);
    let base_rule = rules_in(base)
        .into_iter()
        .find(|(selectors, _)| selectors == ".filetree__row");
    if let Some((_, body)) = base_rule {
        assert!(
            !body.contains("min-height"),
            "the base .filetree__row rule must not set min-height — the 44px \
             floor belongs only under @media (pointer: coarse) so desktop \
             density is unchanged (TASK-42 / finding 5 correction)",
        );
    }
}

// ---- 3. Already-correct behavior stays in place (verify, don't reimplement) -

#[test]
fn tables_scroll_within_their_own_container() {
    let css = strip_css_comments(&read_components());
    let table = rules_in(&css)
        .into_iter()
        .find(|(s, _)| s == ".component-table")
        .map(|(_, b)| b)
        .expect(".component-table rule must exist");
    assert!(
        table.contains("overflow-x") && table.contains("auto"),
        ".component-table must keep overflow-x: auto so wide tables scroll \
         inside their own container, not the page body",
    );
    let data = rules_in(&css)
        .into_iter()
        .find(|(s, _)| s == ".data-table")
        .map(|(_, b)| b)
        .expect(".data-table rule must exist");
    assert!(
        data.replace(' ', "").contains("min-width:max-content"),
        ".data-table must keep min-width: max-content so it overflows its \
         scroll container instead of squeezing the page",
    );
}

#[test]
fn kanban_columns_scroll_within_their_own_container() {
    let css = strip_css_comments(&read_components());
    let kanban = rules_in(&css)
        .into_iter()
        .find(|(s, _)| s == ".kanban-columns")
        .map(|(_, b)| b)
        .expect(".kanban-columns rule must exist");
    assert!(
        kanban.contains("overflow-x") && kanban.contains("auto"),
        ".kanban-columns must keep overflow-x: auto so the board scrolls \
         inside its own container at narrow width",
    );
}

#[test]
fn stat_cards_wrap_toward_a_single_column() {
    let css = strip_css_comments(&read_components());
    let stats = rules_in(&css)
        .into_iter()
        .find(|(s, _)| s == ".component-stats")
        .map(|(_, b)| b)
        .expect(".component-stats rule must exist");
    assert!(
        stats
            .replace(' ', "")
            .contains("repeat(auto-fit,minmax(120px,1fr))"),
        ".component-stats must keep its auto-fit minmax(120px, 1fr) grid so \
         stat cards wrap and collapse toward one column at narrow width",
    );
    let card = rules_in(&css)
        .into_iter()
        .find(|(s, _)| s == ".stat-card")
        .map(|(_, b)| b)
        .expect(".stat-card rule must exist");
    assert!(
        card.replace(' ', "").contains("min-width:0"),
        ".stat-card must keep min-width: 0 so it can shrink within its grid \
         track instead of forcing horizontal overflow",
    );
}
