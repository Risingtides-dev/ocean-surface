//! Safe markdown-lite rendering for room message bodies.
//!
//! SECURITY MODEL — structural, not sanitized: the tokenizer never parses
//! HTML and the renderer only ever emits Leptos text nodes inside a fixed
//! set of elements (`<strong>`, `<em>`, `<code>`, `<a>`, `<span>`). There is
//! no `innerHTML` path, so no body content can become markup. Links are
//! additionally allowlisted to `http://` / `https://` schemes; anything
//! else (`javascript:`, `data:`, …) renders as literal text.
//!
//! MENTION TRUTH: `@id` highlights only when `id` resolves against the
//! open room's daemon-provided participant roster. Unresolved tokens stay
//! plain text — no fabricated identity affordances.
//!
//! Grammar (deliberately lite, single pass, no nesting):
//! `**bold**`, `*italic*`, `` `code` ``, `[label](https://…)`,
//! bare `https://…` autolinks, `@member-id`.

use leptos::prelude::*;
use std::collections::HashSet;

/// One rendered run of a message body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MdSpan {
    Text(String),
    Bold(String),
    Italic(String),
    Code(String),
    Link { href: String, label: String },
    Mention(String),
}

fn scheme_allowed(href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn is_mention_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
}

/// True when a token may start at this position (start of text or after a
/// non-word character) — keeps `user@host` and `3*4*5` from tokenizing.
fn at_boundary(prev: Option<char>) -> bool {
    prev.is_none_or(|c| !c.is_alphanumeric())
}

fn flush(out: &mut Vec<MdSpan>, text: &mut String) {
    if !text.is_empty() {
        out.push(MdSpan::Text(std::mem::take(text)));
    }
}

/// Tokenize a message body against the room's participant ids.
pub fn tokenize(body: &str, members: &HashSet<String>) -> Vec<MdSpan> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut i = 0usize;
    let mut prev: Option<char> = None;
    while i < body.len() {
        let rest = &body[i..];
        let c = rest.chars().next().expect("in-bounds char");

        // `code` — verbatim, nothing tokenizes inside.
        if c == '`' {
            if let Some(rel) = rest[1..].find('`') {
                if rel > 0 {
                    flush(&mut out, &mut text);
                    out.push(MdSpan::Code(rest[1..1 + rel].to_string()));
                    i += 1 + rel + 1;
                    prev = Some('`');
                    continue;
                }
            }
        }

        // **bold** then *italic* (double checked first).
        if c == '*' {
            if let Some(inner) = rest
                .strip_prefix("**")
                .and_then(|r| r.find("**").filter(|&n| n > 0).map(|n| r[..n].to_string()))
            {
                flush(&mut out, &mut text);
                i += 2 + inner.len() + 2;
                out.push(MdSpan::Bold(inner));
                prev = Some('*');
                continue;
            }
            if at_boundary(prev) {
                if let Some(inner) = rest.strip_prefix('*').and_then(|r| {
                    r.find('*')
                        .filter(|&n| n > 0 && !r[..n].contains('\n'))
                        .map(|n| r[..n].to_string())
                }) {
                    flush(&mut out, &mut text);
                    i += 1 + inner.len() + 1;
                    out.push(MdSpan::Italic(inner));
                    prev = Some('*');
                    continue;
                }
            }
        }

        // [label](href) — href must pass the scheme allowlist or the whole
        // thing stays literal text.
        if c == '[' {
            if let Some(close) = rest.find(']') {
                let label = &rest[1..close];
                let after = &rest[close + 1..];
                if !label.is_empty() && after.starts_with('(') {
                    if let Some(end) = after.find(')') {
                        let href = &after[1..end];
                        if scheme_allowed(href) {
                            flush(&mut out, &mut text);
                            out.push(MdSpan::Link {
                                href: href.to_string(),
                                label: label.to_string(),
                            });
                            i += close + 1 + end + 1;
                            prev = Some(')');
                            continue;
                        }
                    }
                }
            }
        }

        // Bare http(s) autolink.
        if (c == 'h' || c == 'H')
            && at_boundary(prev)
            && (rest[..7.min(rest.len())].eq_ignore_ascii_case("http://")
                || rest[..8.min(rest.len())].eq_ignore_ascii_case("https://"))
        {
            let end = rest
                .find(|ch: char| ch.is_whitespace() || matches!(ch, '<' | '>' | '"'))
                .unwrap_or(rest.len());
            let url = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')']);
            let scheme_len = if rest[..8.min(rest.len())].eq_ignore_ascii_case("https://") {
                8
            } else {
                7
            };
            if url.len() > scheme_len {
                flush(&mut out, &mut text);
                out.push(MdSpan::Link {
                    href: url.to_string(),
                    label: url.to_string(),
                });
                i += url.len();
                prev = url.chars().last();
                continue;
            }
        }

        // @mention — highlighted ONLY when the id resolves in the roster.
        if c == '@' && at_boundary(prev) {
            let raw: String = rest[1..]
                .chars()
                .take_while(|&c| is_mention_char(c))
                .collect();
            // Trailing sentence punctuation like "@bob." — retry trimmed.
            let mut id = raw.as_str();
            while !id.is_empty() && !members.contains(id) {
                match id.chars().last() {
                    Some('.' | '-' | '_') => id = &id[..id.len() - 1],
                    _ => break,
                }
            }
            if !id.is_empty() && members.contains(id) {
                flush(&mut out, &mut text);
                out.push(MdSpan::Mention(id.to_string()));
                i += 1 + id.len();
                prev = id.chars().last();
                continue;
            }
        }

        text.push(c);
        i += c.len_utf8();
        prev = Some(c);
    }
    flush(&mut out, &mut text);
    out
}

/// Render a message body reactively against the room's member-id set.
/// Every span becomes text nodes inside fixed elements — no HTML path.
pub fn body_view(body: String, members: Memo<HashSet<String>>) -> impl IntoView {
    move || {
        tokenize(&body, &members.get())
            .into_iter()
            .map(|span| match span {
                MdSpan::Text(s) => s.into_any(),
                MdSpan::Bold(s) => view! { <strong class="rooms-md__b">{s}</strong> }.into_any(),
                MdSpan::Italic(s) => view! { <em class="rooms-md__i">{s}</em> }.into_any(),
                MdSpan::Code(s) => view! { <code class="rooms-md__code">{s}</code> }.into_any(),
                MdSpan::Link { href, label } => view! {
                    <a
                        class="rooms-md__link"
                        href=href
                        target="_blank"
                        rel="noopener noreferrer nofollow"
                    >
                        {label}
                    </a>
                }
                .into_any(),
                MdSpan::Mention(id) => view! {
                    <span class="rooms-md__mention">{format!("@{id}")}</span>
                }
                .into_any(),
            })
            .collect::<Vec<_>>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn plain_text_passes_through_untokenized() {
        let spans = tokenize("hello world", &members(&[]));
        assert_eq!(spans, vec![MdSpan::Text("hello world".into())]);
    }

    #[test]
    fn html_is_never_markup_only_literal_text() {
        // Structural safety: markup arrives as a Text span; the renderer
        // emits it as a text node, so it can never become elements.
        let spans = tokenize("<script>alert(1)</script>", &members(&[]));
        assert_eq!(
            spans,
            vec![MdSpan::Text("<script>alert(1)</script>".into())]
        );
    }

    #[test]
    fn bold_italic_code_tokenize() {
        let spans = tokenize("**b** *i* `c`", &members(&[]));
        assert_eq!(
            spans,
            vec![
                MdSpan::Bold("b".into()),
                MdSpan::Text(" ".into()),
                MdSpan::Italic("i".into()),
                MdSpan::Text(" ".into()),
                MdSpan::Code("c".into()),
            ]
        );
    }

    #[test]
    fn code_is_verbatim_no_inner_tokens() {
        let spans = tokenize("`**not bold** @bob`", &members(&["bob"]));
        assert_eq!(spans, vec![MdSpan::Code("**not bold** @bob".into())]);
    }

    #[test]
    fn unclosed_markers_stay_literal() {
        let spans = tokenize("2*3 and a_b and `tick", &members(&[]));
        assert_eq!(spans, vec![MdSpan::Text("2*3 and a_b and `tick".into())]);
    }

    #[test]
    fn labeled_link_requires_allowed_scheme() {
        let ok = tokenize("[docs](https://ocean.dev)", &members(&[]));
        assert_eq!(
            ok,
            vec![MdSpan::Link {
                href: "https://ocean.dev".into(),
                label: "docs".into()
            }]
        );
        let bad = tokenize("[x](javascript:alert(1))", &members(&[]));
        assert_eq!(bad, vec![MdSpan::Text("[x](javascript:alert(1))".into())]);
        let data = tokenize("[x](data:text/html,hi)", &members(&[]));
        assert_eq!(data, vec![MdSpan::Text("[x](data:text/html,hi)".into())]);
    }

    #[test]
    fn bare_url_autolinks_and_trims_trailing_punctuation() {
        let spans = tokenize("see https://ocean.dev/a, ok", &members(&[]));
        assert_eq!(
            spans,
            vec![
                MdSpan::Text("see ".into()),
                MdSpan::Link {
                    href: "https://ocean.dev/a".into(),
                    label: "https://ocean.dev/a".into()
                },
                MdSpan::Text(", ok".into()),
            ]
        );
    }

    #[test]
    fn bare_scheme_alone_is_not_a_link() {
        let spans = tokenize("https:// is empty", &members(&[]));
        assert_eq!(spans, vec![MdSpan::Text("https:// is empty".into())]);
    }

    #[test]
    fn mention_resolves_only_against_roster() {
        let m = members(&["ada"]);
        assert_eq!(
            tokenize("hi @ada", &m),
            vec![MdSpan::Text("hi ".into()), MdSpan::Mention("ada".into())]
        );
        // Unknown id: NO mention affordance — plain text.
        assert_eq!(
            tokenize("hi @ghost", &m),
            vec![MdSpan::Text("hi @ghost".into())]
        );
    }

    #[test]
    fn mention_trailing_punctuation_retries_trimmed() {
        let m = members(&["ada"]);
        assert_eq!(
            tokenize("ping @ada.", &m),
            vec![
                MdSpan::Text("ping ".into()),
                MdSpan::Mention("ada".into()),
                MdSpan::Text(".".into()),
            ]
        );
    }

    #[test]
    fn email_local_part_is_not_a_mention() {
        let m = members(&["host"]);
        assert_eq!(
            tokenize("mail user@host now", &m),
            vec![MdSpan::Text("mail user@host now".into())]
        );
    }

    #[test]
    fn mid_word_asterisks_do_not_italicize() {
        let spans = tokenize("3*4*5", &members(&[]));
        assert_eq!(spans, vec![MdSpan::Text("3*4*5".into())]);
    }

    #[test]
    fn multiline_bodies_keep_newlines_in_text() {
        let spans = tokenize("a\nb", &members(&[]));
        assert_eq!(spans, vec![MdSpan::Text("a\nb".into())]);
    }

    #[test]
    fn mixed_body_end_to_end() {
        let m = members(&["ada"]);
        let spans = tokenize("**hi** @ada see [d](https://o.dev) `x`", &m);
        assert_eq!(
            spans,
            vec![
                MdSpan::Bold("hi".into()),
                MdSpan::Text(" ".into()),
                MdSpan::Mention("ada".into()),
                MdSpan::Text(" see ".into()),
                MdSpan::Link {
                    href: "https://o.dev".into(),
                    label: "d".into()
                },
                MdSpan::Text(" ".into()),
                MdSpan::Code("x".into()),
            ]
        );
    }
}
