//! Streaming-friendly markdown renderer shared by the transcript and the float
//! bubble.
//!
//! The naive path re-parses the entire accumulated message and swaps the whole
//! `inner_html` on every streaming delta — O(N²) over a stream, and it tears
//! down all child DOM each delta (flicker, lost selection, reset embeds). This
//! module splits the accumulated markdown into top-level blocks
//! ([`crate::markdown::split_blocks`]); finalized blocks render once as stable,
//! keyed elements whose HTML is never recomputed, and only the last
//! (in-progress) block re-parses as deltas arrive.
//!
//! [`reconcile_blocks`] is the pure core: given the previous block list and the
//! new accumulated text, it reuses the cached HTML of every leading block whose
//! source is unchanged and re-renders only what actually changed (the tail, plus
//! a block the moment it seals). The Leptos [`MarkdownStream`] component drives a
//! keyed `<For>` off that list, so a finalized block keeps its exact DOM node
//! across deltas. Concatenating the rendered blocks is byte-identical to a
//! single `render` of the whole message (see `markdown::split_blocks`).

use leptos::prelude::*;

use crate::markdown::render as render_md;

/// One rendered top-level markdown block. `src` is the block's markdown source
/// (the reuse key across deltas); `html` is its rendered output; `key` is a
/// stable, unique identity for keyed reconciliation (position + source), so a
/// finalized block keeps the same DOM node while the growing tail gets a fresh
/// one each delta.
#[derive(Clone, PartialEq, Debug)]
pub struct RenderedBlock {
    pub key: String,
    pub src: String,
    pub html: String,
}

/// Reconcile the previous block list against new accumulated `text`, rendering
/// through [`render_md`]. Leading blocks whose source is unchanged are reused
/// verbatim (no re-parse); everything from the first changed block onward — in
/// practice just the in-progress tail — is re-rendered.
pub fn reconcile_blocks(prev: &[RenderedBlock], text: &str) -> Vec<RenderedBlock> {
    reconcile_blocks_with(prev, text, render_md)
}

/// [`reconcile_blocks`] with an injectable renderer so tests can count exactly
/// which blocks are (re-)parsed.
fn reconcile_blocks_with(
    prev: &[RenderedBlock],
    text: &str,
    render: impl Fn(&str) -> String,
) -> Vec<RenderedBlock> {
    crate::markdown::split_blocks(text)
        .into_iter()
        .enumerate()
        .map(|(i, seg)| {
            // A leading block with identical source is finalized and stable:
            // reuse its cached HTML and identity, never re-parsing it.
            if let Some(p) = prev.get(i) {
                if p.src == seg {
                    return p.clone();
                }
            }
            let html = render(&seg);
            RenderedBlock {
                key: format!("{i}\u{1}{seg}"),
                src: seg,
                html,
            }
        })
        .collect()
}

/// Render a reactive markdown string as a stream of stable, keyed blocks.
///
/// `class` is applied to the block-level container (which also owns the
/// external-link click wiring); each block is hosted in a `display: contents`
/// `.md-block` element so the block-split structure has no effect on layout or
/// margin collapsing.
#[component]
pub fn MarkdownStream(
    /// Reactive accumulated markdown source.
    #[prop(into)]
    text: Signal<String>,
    /// Class for the block-level container element.
    #[prop(into)]
    class: String,
) -> impl IntoView {
    // Dedupe on the string itself: an unrelated `turns` mutation that leaves the
    // text unchanged must not even re-scan for boundaries.
    let src = Memo::new(move |_| text.get());
    let blocks = Memo::new(move |prev: Option<&Vec<RenderedBlock>>| {
        reconcile_blocks(prev.map(Vec::as_slice).unwrap_or(&[]), &src.get())
    });

    view! {
        <div class=class on:click=crate::host::open_external_link_click>
            <For
                each=move || blocks.get()
                key=|b| b.key.clone()
                children=|b| view! { <div class="md-block" inner_html=b.html></div> }
            />
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn html_of(blocks: &[RenderedBlock]) -> String {
        blocks.iter().map(|b| b.html.as_str()).collect()
    }

    // (a) A fully-received message rendered as blocks is byte-identical to the
    // single-parse render, across every construct.
    #[test]
    fn full_message_parity_across_constructs() {
        let src = "# Title\n\nText with **bold**, `code`, and [a link](https://example.com).\n\n\
                   - one\n- two\n- [x] done\n\n\
                   | a | b |\n|---|---|\n| 1 | 2 |\n\n\
                   ```rust\nlet x = 1;\n\nlet y = 2;\n```\n\n\
                   > quote\n\nlast\n";
        assert_eq!(
            html_of(&reconcile_blocks(&[], src)),
            crate::markdown::render(src)
        );
    }

    // (b) Once the tail is the only growing block, each delta re-parses exactly
    // one block; finalized blocks are not re-parsed.
    #[test]
    fn tail_only_reparse_per_delta() {
        let prev = reconcile_blocks(&[], "para one\n\npara two is streaming her");
        assert_eq!(prev.len(), 2);

        let count = Cell::new(0);
        let next = reconcile_blocks_with(&prev, "para one\n\npara two is streaming here", |s| {
            count.set(count.get() + 1);
            crate::markdown::render(s)
        });

        assert_eq!(count.get(), 1, "only the tail block should re-parse");
        assert_eq!(next.len(), 2);
        assert_eq!(
            next[0].html, prev[0].html,
            "finalized block HTML must be reused"
        );
        assert_eq!(
            next[0].key, prev[0].key,
            "finalized block identity must be stable"
        );
    }

    // (c) A fenced code block streamed character-by-character never renders a
    // broken/partial fence as literal text.
    #[test]
    fn streamed_fence_never_renders_broken() {
        let src = "```\nlet x = 1;\n\nlet y = 2;\n```"; // ASCII → byte slicing is safe
        for i in 1..=src.len() {
            let prefix = &src[..i];
            let html = html_of(&reconcile_blocks(&[], prefix));
            // Once the opening fence line exists, content is a code block.
            if prefix.starts_with("```\n") {
                assert!(
                    html.contains("<pre") || html.contains("<code"),
                    "open fence must render as code at {prefix:?}: {html}"
                );
                assert!(
                    !html.contains("```"),
                    "raw fence markers must never leak as text at {prefix:?}: {html}"
                );
            }
        }
    }

    // (d) A table split across deltas renders as a paragraph until complete,
    // then renders once as a table.
    #[test]
    fn streamed_table_renders_once_complete() {
        let header_only = html_of(&reconcile_blocks(&[], "| h1 | h2 |\n"));
        assert!(
            !header_only.contains("<table"),
            "header row alone is not a table: {header_only}"
        );

        let complete = html_of(&reconcile_blocks(
            &[],
            "| h1 | h2 |\n|----|----|\n| a | b |\n",
        ));
        assert!(
            complete.contains("<table"),
            "completed table must render: {complete}"
        );
    }

    // (e) An unrelated mutation (same text) re-parses nothing.
    #[test]
    fn unrelated_mutation_does_not_reparse() {
        let prev = reconcile_blocks(&[], "alpha\n\nbeta");
        let count = Cell::new(0);
        let next = reconcile_blocks_with(&prev, "alpha\n\nbeta", |s| {
            count.set(count.get() + 1);
            crate::markdown::render(s)
        });
        assert_eq!(count.get(), 0, "identical text must not re-parse any block");
        assert_eq!(next, prev);
    }

    // (f) A finalized block's identity survives a subsequent tail delta, so the
    // keyed <For> preserves its DOM node.
    #[test]
    fn finalized_block_identity_survives_delta() {
        let prev = reconcile_blocks(&[], "alpha\n\nbe");
        let next = reconcile_blocks(&prev, "alpha\n\nbet");
        assert_eq!(prev.len(), 2);
        assert_eq!(next.len(), 2);
        assert_eq!(
            prev[0].key, next[0].key,
            "finalized block key must be identical across a delta"
        );
    }
}
