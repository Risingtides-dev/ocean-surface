//! TASK-41 — mobile composer stability regressions.
//!
//! These assert against the shipped CSS/HTML source (the same source-assertion
//! style as voice_realtime_regressions.rs) so the iOS-Safari anti-zoom fix
//! can't silently regress:
//!   1. Every touch-focused field a phone user hits computes to >= 16px under a
//!      `@media (pointer: coarse)` scope — below 16px, iOS force-zooms the page
//!      on focus and shifts/rescales the layout.
//!   2. The viewport meta carries `interactive-widget=resizes-content` so the
//!      soft keyboard resizes the content box instead of overlaying it.

fn read_style(name: &str) -> String {
    let path = format!("{}/../../styles/{}", env!("CARGO_MANIFEST_DIR"), name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read styles/{name}: {e}"))
}

fn index_html() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../index.html");
    std::fs::read_to_string(path).expect("read root index.html")
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

/// Bodies of every `@media (pointer: coarse)` block, brace-matched so nested
/// rules are captured whole. Braces are single-byte ASCII, so byte scanning is
/// safe even though comments/content carry multi-byte characters.
fn coarse_pointer_blocks(css: &str) -> Vec<String> {
    let css = strip_css_comments(css);
    let needle = "@media (pointer: coarse)";
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

/// The `font-size` px value applied to `selector` inside any coarse-pointer
/// block, if present. Reads the first `font-size: Npx` that follows the
/// selector within its rule body (up to the closing `}`).
fn coarse_font_size_px(css: &str, selector: &str) -> Option<u32> {
    for block in coarse_pointer_blocks(css) {
        let Some(sel_at) = block.find(selector) else {
            continue;
        };
        // Restrict to this rule's body: from the selector to the next `}`.
        let tail = &block[sel_at..];
        let rule_end = tail.find('}').unwrap_or(tail.len());
        let rule = &tail[..rule_end];
        let Some(fs_at) = rule.find("font-size") else {
            continue;
        };
        let after = &rule[fs_at..];
        let Some(colon) = after.find(':') else {
            continue;
        };
        let value: String = after[colon + 1..]
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(px) = value.parse::<u32>() {
            return Some(px);
        }
    }
    None
}

#[test]
fn composer_input_has_16px_floor_under_coarse_pointer() {
    // The composer floor landed on main in styles/compact.css (owner lane,
    // commit 98c8a59) — it shipped untested, so this covers it. The rule is
    // scoped `.ocean-surface .ocean-composer__input`; coarse_font_size_px finds
    // `.ocean-composer__input` as a substring of that descendant selector.
    let css = read_style("compact.css");
    let px = coarse_font_size_px(&css, ".ocean-composer__input").expect(
        "compact.css must set .ocean-composer__input font-size inside a \
         @media (pointer: coarse) block — the iOS anti-zoom floor (TASK-41)",
    );
    assert!(
        px >= 16,
        "the composer textarea must compute to >= 16px on touch so iOS Safari \
         does not force-zoom on focus; found {px}px",
    );
}

#[test]
fn landed_composer_floor_is_scoped_to_the_app_shell() {
    // Guard the exact selector the owner lane shipped: `.ocean-surface
    // .ocean-composer__input`. Scoping under the .ocean-surface shell (not the
    // extension-only root) is what makes the floor reach the iPhone PWA, and
    // the extra class keeps it above the composer's own base rule. If a refactor
    // narrows it to .ocean-surface--extension the PWA regresses silently.
    let css = strip_css_comments(&read_style("compact.css"));
    let has_shell_scoped_floor = coarse_pointer_blocks(&css).iter().any(|block| {
        block
            .split('}')
            .any(|rule| rule.contains(".ocean-surface .ocean-composer__input"))
    });
    assert!(
        has_shell_scoped_floor,
        "the composer anti-zoom floor must stay scoped to \
         `.ocean-surface .ocean-composer__input` under @media (pointer: coarse) \
         so it applies to the PWA shell, not just the extension side panel",
    );
}

#[test]
fn touch_focused_fields_carry_the_16px_anti_zoom_floor() {
    // Every field a phone user actually focuses, mapped to the stylesheet that
    // co-locates its coarse-pointer floor. Each must reach >= 16px so focusing
    // it never triggers the iOS zoom-and-shift (TASK-41).
    //
    // The three Rooms entries here used to name `.rooms-panel__create-input`,
    // `.rooms-composer__input` and `.rooms-addagent__input` in panels.css.
    // Those classes are emitted by nothing: the shipped Rooms surface is
    // `rooms_workspace.rs`, which renders `.rooms-workspace__*`. So the guard
    // was pinning a floor under fields no phone user can focus, while the
    // fields they DO focus — the room-name/redeem input, the composer, the
    // add-agent select and the three agent-builder controls — carried their
    // floor in styles/rooms-workspace.css with nothing asserting it. Deleting
    // that live rule passed every gate.
    //
    // The Rooms half of this table is NOT hand-maintained — see
    // `every_rooms_text_entry_control_is_in_the_anti_zoom_floor`, which derives
    // it from the view source. A hand-written list is how the second half of
    // the same miss survived: the six composer/room controls were floored while
    // the right rail's eight sat at 11-13px.
    let fields: &[(&str, &str)] = &[
        ("compact.css", ".ocean-composer__input"),
        ("island.css", ".island-search__input"),
        ("island.css", ".island-recall__input"),
        ("panels.css", ".sessions-create__input"),
        ("deck.css", ".palette-input"),
    ];
    for (file, selector) in fields {
        let css = read_style(file);
        let px = coarse_font_size_px(&css, selector).unwrap_or_else(|| {
            panic!(
                "{file} must set {selector} font-size inside a \
                 @media (pointer: coarse) block (TASK-41 anti-zoom floor)"
            )
        });
        assert!(
            px >= 16,
            "{selector} in {file} must compute to >= 16px on touch to avoid \
             iOS auto-zoom on focus; found {px}px",
        );
    }
}

#[test]
fn viewport_meta_declares_interactive_widget_resizes_content() {
    let html = index_html();
    let meta = html
        .lines()
        .find(|line| line.contains("name=\"viewport\""))
        .expect("index.html must declare a viewport meta tag");
    assert!(
        meta.contains("interactive-widget=resizes-content"),
        "viewport meta must set interactive-widget=resizes-content so the soft \
         keyboard resizes the content box instead of overlaying the composer \
         (TASK-41); found: {meta}",
    );
}

/// Every `rooms-workspace__*` class on a text-entry element in the shipped
/// view source, paired with whether it opens a keyboard.
///
/// The attribute region is bounded by tracking `(){}[]` depth, because a
/// Leptos attribute value is a Rust expression and can hold `>` — `->` in a
/// closure signature, a turbofish, a comparison. Scanning to the first bare
/// `>` would cut an element short and silently drop its class.
fn rooms_text_entry_classes() -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src"
    ))];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read_dir src") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap_or_default();
            // Production half only: a fixture quoting markup is not a control.
            let view = source
                .split_once("#[cfg(test)]")
                .map_or(&source[..], |h| h.0);
            for tag in ["<input", "<select", "<textarea"] {
                let mut from = 0usize;
                while let Some(rel) = view[from..].find(tag) {
                    let at = from + rel;
                    let body = &view[at + tag.len()..];
                    let mut depth = 0i32;
                    let mut end = body.len();
                    for (i, c) in body.char_indices() {
                        match c {
                            '(' | '{' | '[' => depth += 1,
                            ')' | '}' | ']' => depth -= 1,
                            '>' if depth == 0 => {
                                end = i;
                                break;
                            }
                            _ => {}
                        }
                    }
                    let attrs = &body[..end];
                    // A file picker opens a chooser, never the keyboard, so it
                    // cannot trigger the focus zoom this floor exists for.
                    if !attrs.contains("type=\"file\"") {
                        if let Some(c) = attrs.find("rooms-workspace__") {
                            let class: String = attrs[c..]
                                .chars()
                                .take_while(|ch| {
                                    ch.is_ascii_lowercase() || *ch == '-' || *ch == '_'
                                })
                                .collect();
                            found.push(class);
                        }
                    }
                    from = at + tag.len();
                }
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// The anti-zoom floor must cover EVERY text-entry control the rooms surface
/// renders, derived from the source rather than from a list someone remembered
/// to update.
///
/// Codex caught the gap this closes on #198: the repointed guard named six
/// controls and claimed to cover "every field a phone user focuses", while
/// `__invite-input`, `__repo-input`, the three artifact editor fields,
/// `__authority-select` and the two compute-panel inputs rendered in the
/// mobile right rail at 11-13px. Focusing any of them still force-zoomed the
/// page, with this file asserting that none could.
#[test]
fn every_rooms_text_entry_control_is_in_the_anti_zoom_floor() {
    let classes = rooms_text_entry_classes();
    assert!(
        classes.len() >= 10,
        "the scanner found only {} rooms text-entry controls, which means it \
         stopped matching the markup rather than that the surface shrank: {classes:?}",
        classes.len(),
    );
    let css = read_style("rooms-workspace.css");
    let missing: Vec<_> = classes
        .iter()
        .filter(|class| coarse_font_size_px(&css, &format!(".{class}")).is_none_or(|px| px < 16))
        .collect();
    assert!(
        missing.is_empty(),
        "these rooms controls open a keyboard on a phone but carry no 16px \
         coarse-pointer floor in styles/rooms-workspace.css, so focusing one \
         force-zooms the page: {missing:?}",
    );
}
