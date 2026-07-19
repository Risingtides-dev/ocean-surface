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
    let css = read_style("composer.css");
    let px = coarse_font_size_px(&css, ".ocean-composer__input").expect(
        "composer.css must set .ocean-composer__input font-size inside a \
         @media (pointer: coarse) block — the iOS anti-zoom floor (TASK-41)",
    );
    assert!(
        px >= 16,
        "the composer textarea must compute to >= 16px on touch so iOS Safari \
         does not force-zoom on focus; found {px}px",
    );
}

#[test]
fn touch_focused_fields_carry_the_16px_anti_zoom_floor() {
    // Every field a phone user actually focuses, mapped to the stylesheet that
    // co-locates its coarse-pointer floor. Each must reach >= 16px so focusing
    // it never triggers the iOS zoom-and-shift (TASK-41).
    let fields: &[(&str, &str)] = &[
        ("composer.css", ".ocean-composer__input"),
        ("island.css", ".island-search__input"),
        ("island.css", ".island-recall__input"),
        ("panels.css", ".sessions-create__input"),
        ("panels.css", ".rooms-panel__create-input"),
        ("panels.css", ".rooms-composer__input"),
        ("panels.css", ".rooms-addagent__input"),
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
