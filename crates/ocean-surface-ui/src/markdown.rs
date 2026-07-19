//! Markdown → safe HTML for assistant text and Markdown components.
//!
//! pulldown-cmark follows CommonMark and passes both raw HTML and link/image
//! destinations through verbatim — there is no option to disable raw HTML, and
//! it never validates URL schemes. Because this output is written via
//! `inner_html` (transcript, MarkdownView, callout, float widget), untouched it
//! is a live injection surface: a `<script>` / `<img onerror=…>` or a
//! `[x](javascript:…)` link would execute. We neutralize both:
//!   * every raw-HTML event (block + inline) becomes a Text event, so the
//!     writer escapes `<`, `>`, `&` and the markup renders as inert text;
//!   * link/image destinations are scheme-checked against an allowlist
//!     (http/https/mailto/tel, plus data: for images), with ASCII whitespace
//!     and control chars stripped first so `java\tscript:` splits can't slip
//!     past; anything else is emptied.

use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag};

/// Parser options shared by [`render`] and [`split_blocks`] so the two always
/// agree on document structure (tables, task lists, strikethrough).
fn options() -> Options {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_TABLES);
    opts
}

/// Split accumulated markdown into its ordered top-level block source segments.
///
/// This is the boundary primitive behind streaming render: each returned slice
/// is exactly one top-level block (paragraph, list, table, fenced code, …), so
/// callers can render finalized blocks once and only re-parse the last,
/// in-progress block as a stream grows. Because appending text to a CommonMark
/// document never rewrites an earlier top-level block, every segment except the
/// last is stable once it appears.
///
/// Boundaries are anchored at top-level block *starts* reported by
/// pulldown-cmark's source-offset iterator — never on blank lines — so a fenced
/// code block, table, or list that spans blank lines is kept whole. Rendering
/// each segment through [`render`] and concatenating the results is
/// byte-identical to a single [`render`] of the whole input (the HTML writer
/// carries no state across a top-level boundary: block elements always close on
/// a newline, exactly as at the start of a fresh parse).
pub fn split_blocks(src: &str) -> Vec<String> {
    let mut starts: Vec<usize> = Vec::new();
    let mut depth = 0i32;
    for (event, range) in Parser::new_ext(src, options()).into_offset_iter() {
        match event {
            // A block opening at the top level begins a new segment; nested
            // opens (list items, table cells, inline spans) stay inside it.
            Event::Start(_) => {
                if depth == 0 {
                    starts.push(range.start);
                }
                depth += 1;
            }
            Event::End(_) => depth -= 1,
            // Standalone top-level block events (a thematic break) are their own
            // segment; anything standalone at depth > 0 is inline and ignored.
            _ => {
                if depth == 0 {
                    starts.push(range.start);
                }
            }
        }
    }

    if starts.is_empty() {
        // No parsed block (empty or whitespace-only input): one empty tail.
        return vec![src.to_string()];
    }

    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = starts.get(i + 1).copied().unwrap_or(src.len());
            src[start..end].to_string()
        })
        .collect()
}

pub fn render(src: &str) -> String {
    let opts = options();

    let parser = Parser::new_ext(src, opts).map(|ev| match ev {
        // Raw HTML must never reach inner_html as live markup. Textify it so
        // the writer escapes it; real markdown (code fences, tables, …) arrives
        // as its own events and is unaffected.
        Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
        // Scheme-check link/image destinations — javascript:/data:text/html and
        // friends are otherwise passed straight into the rendered href/src.
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Link {
            link_type,
            dest_url: safe_url(dest_url, false),
            title,
            id,
        }),
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Image {
            link_type,
            dest_url: safe_url(dest_url, true),
            title,
            id,
        }),
        other => other,
    });
    let mut out = String::with_capacity(src.len() * 3 / 2);
    html::push_html(&mut out, parser);
    // Externalise every real link to a new browser tab so clicking a URL in
    // the transcript never navigates the SPA away from the conversation.
    // pulldown-cmark's HTML writer emits plain `<a href>` with no attributes,
    // so inject target/rel after the fact, then revert sanitised (empty-href)
    // links so dead links stay inert.
    out = out.replace(
        "<a href=\"",
        "<a target=\"_blank\" rel=\"noopener noreferrer\" href=\"",
    );
    out = out.replace(
        "<a target=\"_blank\" rel=\"noopener noreferrer\" href=\"\">",
        "<a href=\"\">",
    );
    out
}

/// Allowlist URL schemes for a rendered link. `allow_data` also permits `data:`
/// (legitimate and inert for image sources). Relative URLs, fragments, and
/// protocol-relative URLs carry no scheme and pass. An unsafe destination is
/// emptied rather than dropped so the link/image text survives.
fn safe_url(dest: CowStr, allow_data: bool) -> CowStr {
    // Browsers ignore ASCII whitespace + control chars when resolving a scheme,
    // so strip them before detection (defeats `java\tscript:` / newline splits).
    let cleaned: String = dest
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && !c.is_control())
        .collect();
    let lower = cleaned.to_ascii_lowercase();
    let safe = match lower.find([':', '/', '?', '#']) {
        // A ':' before any path/query/fragment delimiter is an explicit scheme;
        // allow only known-inert ones.
        Some(i) if lower.as_bytes()[i] == b':' => {
            let scheme = &lower[..i];
            matches!(scheme, "http" | "https" | "mailto" | "tel")
                || (allow_data && scheme == "data")
        }
        // No scheme (relative path, fragment, protocol-relative) → safe.
        _ => true,
    };
    if safe {
        dest
    } else {
        CowStr::Borrowed("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_html_block_is_escaped() {
        let html = render("<script>alert(1)</script>");

        assert!(
            !html.contains("<script>"),
            "raw script tag must not survive as live markup: {html}"
        );
        assert!(
            html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
            "raw script tag should render as inert escaped text: {html}"
        );
    }

    #[test]
    fn inline_raw_html_is_escaped() {
        let html = render("text <img src=x onerror=alert(1)>");

        assert!(
            !html.contains("<img"),
            "raw img tag must not survive as live markup: {html}"
        );
        assert!(
            html.contains("&lt;img src=x onerror=alert(1)&gt;"),
            "raw img tag should render as inert escaped text: {html}"
        );
    }

    #[test]
    fn javascript_link_href_is_emptied() {
        let html = render("[click](javascript:alert(1))");

        assert!(
            !html.contains("href=\"javascript:"),
            "javascript: must not survive in a rendered href: {html}"
        );
        assert!(
            html.contains("<a href=\"\">click</a>"),
            "unsafe destination should be emptied while preserving link text: {html}"
        );
    }

    #[test]
    fn obfuscated_javascript_link_hrefs_are_emptied() {
        let tab_in_pointy_destination = render("[x](<java\tscript:alert(1)>)");
        assert!(
            !tab_in_pointy_destination
                .to_ascii_lowercase()
                .contains("javascript:"),
            "tab-obfuscated javascript: must not survive scheme detection: {tab_in_pointy_destination}"
        );
        assert!(
            tab_in_pointy_destination.contains("<a href=\"\">x</a>"),
            "unsafe destination should be emptied while preserving link text: {tab_in_pointy_destination}"
        );

        for source in ["[x](java\tscript:alert(1))", "[x](java\nscript:alert(1))"] {
            let html = render(source);

            assert!(
                !html.contains("href=\"java"),
                "whitespace-split javascript: must not produce a live href for {source:?}: {html}"
            );
            assert!(
                !html.to_ascii_lowercase().contains("javascript:"),
                "whitespace-split javascript: must not rejoin into an executable scheme for {source:?}: {html}"
            );
            assert!(
                html.contains('x'),
                "link text should remain visible for {source:?}: {html}"
            );
        }
    }

    #[test]
    fn data_text_html_link_href_is_emptied() {
        let html = render("[x](data:text/html,<script>alert(1)</script>)");

        assert!(
            !html.contains("href=\"data:text/html"),
            "data:text/html must not survive in a rendered link href: {html}"
        );
        assert!(
            html.contains("<a href=\"\">x</a>"),
            "unsafe data: link destination should be emptied while preserving link text: {html}"
        );
    }

    #[test]
    fn data_image_src_is_preserved() {
        let html = render("![a](data:image/png;base64,iVBORw0KGgo=)");

        assert!(
            html.contains("<img src=\"data:image/png;base64,iVBORw0KGgo=\" alt=\"a\" />"),
            "data:image sources should remain available for rendered images: {html}"
        );
    }

    #[test]
    fn split_blocks_concatenation_is_byte_identical() {
        // A full, received message rendered block-by-block must be byte-for-byte
        // identical to rendering the whole thing in one pass.
        let src = r#"# Heading

A paragraph with **bold**, *emphasis*, `inline code`, and a
[safe link](https://example.com/path?q=1).

- first
- second
- [x] done

| h1 | h2 |
|----|----|
| a  | b  |

```rust
let x = 1;
// blank line inside a fence:

let y = 2;
```

> a quote
> spanning lines

---

Trailing paragraph.
"#;
        let whole = render(src);
        let joined: String = split_blocks(src).iter().map(|s| render(s)).collect();
        assert_eq!(
            joined, whole,
            "block-split render must equal single-parse render"
        );
    }

    #[test]
    fn split_blocks_keeps_fence_whole_across_blank_lines() {
        // A fenced code block contains a blank line; naive blank-line splitting
        // would fragment it. It must remain a single segment.
        let src = "```\na\n\nb\n```\n";
        let segs = split_blocks(src);
        assert_eq!(
            segs.len(),
            1,
            "fence with blank line must be one block: {segs:?}"
        );
        assert_eq!(render(&segs[0]), render(src));
    }

    #[test]
    fn split_blocks_separates_paragraphs() {
        let segs = split_blocks("one\n\ntwo\n\nthree");
        assert_eq!(segs.len(), 3, "three paragraphs → three segments: {segs:?}");
    }

    #[test]
    fn safe_markdown_still_renders() {
        let html = render(
            r#"
**b** and *e*

[safe](https://example.com/path?q=1)

| h |
|---|
| c |

- [x] done

~~s~~

```
<script>alert(1)</script>
```
"#,
        );

        assert!(
            html.contains("<strong>b</strong>"),
            "bold should render: {html}"
        );
        assert!(
            html.contains("<em>e</em>"),
            "emphasis should render: {html}"
        );
        assert!(
            html.contains("<a target=\"_blank\" rel=\"noopener noreferrer\" href=\"https://example.com/path?q=1\">safe</a>"),
            "safe https links should keep their href: {html}"
        );
        assert!(html.contains("<table>"), "tables should render: {html}");
        assert!(
            html.contains("type=\"checkbox\"") && html.contains("checked"),
            "task lists should render checked checkbox inputs: {html}"
        );
        assert!(
            html.contains("<del>s</del>"),
            "strikethrough should render: {html}"
        );
        assert!(
            html.contains("<code>&lt;script&gt;alert(1)&lt;/script&gt;"),
            "fenced code should escape HTML tags inside code: {html}"
        );
        assert!(
            !html.contains("<script>"),
            "code fence content must not become executable script markup: {html}"
        );
    }
}
