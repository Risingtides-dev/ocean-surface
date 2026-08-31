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

fn has_ascii_control_or_space(c: u8) -> bool {
    c <= 0x20 || c == 0x7f
}

fn has_literal_ascii_control(href: &str) -> bool {
    href.as_bytes()
        .iter()
        .copied()
        .any(|b| b < 0x20 || b == 0x7f)
}

fn has_percent_encoded_ascii_control_or_space(href: &str) -> bool {
    let bytes = href.as_bytes();
    bytes.windows(3).any(|window| {
        if window[0] != b'%' {
            return false;
        }
        let hi = (window[1] as char).to_digit(16);
        let lo = (window[2] as char).to_digit(16);
        match (hi, lo) {
            (Some(hi), Some(lo)) => has_ascii_control_or_space(((hi << 4) | lo) as u8),
            _ => false,
        }
    })
}

fn host_is_valid(host: &str) -> bool {
    if host.is_empty() {
        return false;
    }
    if host.starts_with('[') && host.ends_with(']') {
        return host.len() > 2;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

fn authority_is_valid(authority: &str) -> bool {
    if authority.is_empty() || authority.contains('@') {
        return false;
    }

    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let Some(close) = rest.find(']') else {
            return false;
        };
        let host_end = close + 2;
        let host = &authority[..host_end];
        let remainder = &authority[host_end..];
        if let Some(port) = remainder.strip_prefix(':') {
            (host, Some(port))
        } else if remainder.is_empty() {
            (host, None)
        } else {
            return false;
        }
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        if port.bytes().all(|b| b.is_ascii_digit()) {
            (host, Some(port))
        } else {
            (authority, None)
        }
    } else {
        (authority, None)
    };

    if !host_is_valid(host) {
        return false;
    }
    if let Some(port) = port {
        if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
    }
    true
}

/// Crate-visible because this is the room-content link posture, not just this
/// renderer's: any URL that reached the client through room data (the repo
/// panel's recorded CI rows, say) passes the same allowlist before it may
/// become an anchor.
pub(crate) fn scheme_allowed(href: &str) -> bool {
    if href.is_empty()
        || href.trim() != href
        || href.contains('\\')
        || has_literal_ascii_control(href)
        || has_percent_encoded_ascii_control_or_space(href)
    {
        return false;
    }

    let Some((scheme, rest)) = href.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }

    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if !authority_is_valid(authority) {
        return false;
    }

    true
}

pub(crate) fn is_mention_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
}

/// True when a token may start at this position (start of text or after a
/// non-word character) — keeps `user@host` and `3*4*5` from tokenizing.
pub(crate) fn mention_start_boundary(prev: Option<char>) -> bool {
    prev.is_none_or(|c| !c.is_alphanumeric())
}

fn at_boundary(prev: Option<char>) -> bool {
    mention_start_boundary(prev)
}

fn autolink_boundary(prev: Option<char>) -> bool {
    prev.is_none_or(|c| c.is_ascii_whitespace() || matches!(c, '(' | '[' | '<' | '{'))
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
                    if let Some(raw) = after.strip_prefix('(') {
                        let end = raw.find(')').unwrap_or(raw.len());
                        if end < raw.len() {
                            let href = &raw[..end];
                            let consumed = close + 1 + 1 + end + 1;
                            if !href.is_empty() && scheme_allowed(href) {
                                flush(&mut out, &mut text);
                                out.push(MdSpan::Link {
                                    href: href.to_string(),
                                    label: label.to_string(),
                                });
                                i += consumed;
                                prev = Some(')');
                                continue;
                            }
                            text.push_str(&rest[..consumed]);
                            i += consumed;
                            prev = Some(')');
                            continue;
                        }
                    }
                }
            }
        }

        // Bare http(s) autolink.
        if (c == 'h' || c == 'H')
            && autolink_boundary(prev)
            && (rest
                .get(..7)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
                || rest
                    .get(..8)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://")))
        {
            let end = rest
                .find(|ch: char| ch.is_whitespace() || matches!(ch, '<' | '>' | '"'))
                .unwrap_or(rest.len());
            let url = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')']);
            let scheme_len = if rest
                .get(..8)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
            {
                8
            } else {
                7
            };
            if url.len() > scheme_len && scheme_allowed(url) {
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
        let mixed = tokenize("[docs](HTTPS://ocean.dev)", &members(&[]));
        assert_eq!(
            mixed,
            vec![MdSpan::Link {
                href: "HTTPS://ocean.dev".into(),
                label: "docs".into()
            }]
        );
        let bad = tokenize("[x](javascript:alert(1))", &members(&[]));
        assert_eq!(bad, vec![MdSpan::Text("[x](javascript:alert(1))".into())]);
        let data = tokenize("[x](data:text/html,hi)", &members(&[]));
        assert_eq!(data, vec![MdSpan::Text("[x](data:text/html,hi)".into())]);
    }

    /// The only case in this file that can watch the http/https comparison in
    /// `scheme_allowed` be deleted. The `javascript:`/`data:` payloads above
    /// carry no `://`, so the split refuses them before the comparison is ever
    /// reached, and the autolink path pre-checks the `http(s)://` prefix
    /// itself — an explicit link whose href is a well-formed `scheme://host`
    /// is the one route that reaches the allowlist.
    #[test]
    fn labeled_link_rejects_non_http_scheme_with_authority() {
        for body in ["[x](ftp://ocean.dev)", "[x](FTP://ocean.dev)"] {
            assert_eq!(
                tokenize(body, &members(&[])),
                vec![MdSpan::Text(body.into())]
            );
        }
    }

    #[test]
    fn unsafe_labeled_links_render_literal_text() {
        let cases = [
            "[x](https://ocean.dev\t)",
            "[x](https://ocean.dev\n)",
            "[x](https://ocean.dev/%0a)",
            "[x](https://ocean.dev/%0D)",
            "[x](https://ocean.dev/%09)",
            "[x](https://ocean.dev/%20)",
            "[x](https://ocean.dev/%7f)",
            r"[x](https://ocean.dev\ )",
            "[x](https://exa\\mple.com)",
            "[x](https://user@example.com)",
            "[x](javascript:alert(1))",
            "[x](data:text/html,hi)",
        ];
        for body in cases {
            assert_eq!(
                tokenize(body, &members(&[])),
                vec![MdSpan::Text(body.into())]
            );
        }
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
    fn safe_bare_urls_preserve_queries_and_percent_escapes() {
        let spans = tokenize(
            "see HTTPS://ocean.dev/a?x=%2Fok&y=z and https://ocean.dev/a%2Fb",
            &members(&[]),
        );
        assert_eq!(
            spans,
            vec![
                MdSpan::Text("see ".into()),
                MdSpan::Link {
                    href: "HTTPS://ocean.dev/a?x=%2Fok&y=z".into(),
                    label: "HTTPS://ocean.dev/a?x=%2Fok&y=z".into()
                },
                MdSpan::Text(" and ".into()),
                MdSpan::Link {
                    href: "https://ocean.dev/a%2Fb".into(),
                    label: "https://ocean.dev/a%2Fb".into()
                },
            ]
        );
    }

    #[test]
    fn unsafe_bare_urls_stay_literal_text() {
        let cases = [
            "see https://ocean.dev/%0a",
            "see https://ocean.dev/%0D",
            "see https://ocean.dev/%09",
            "see https://ocean.dev/%20",
            "see https://ocean.dev/%7f",
            "see https://exa\\mple.com",
            "see https://user@example.com",
            "see javascript:alert(1)",
            "see data:text/plain,hi",
        ];
        for body in cases {
            assert_eq!(
                tokenize(body, &members(&[])),
                vec![MdSpan::Text(body.into())]
            );
        }
    }

    #[test]
    fn bare_url_detection_is_safe_across_utf8_boundaries() {
        for body in ["h12345💥", "h123456💥", "http://é", "https://日"] {
            assert_eq!(
                tokenize(body, &members(&[])),
                vec![MdSpan::Text(body.into())]
            );
        }
    }

    #[test]
    fn bare_scheme_alone_is_not_a_link() {
        let spans = tokenize("https:// is empty", &members(&[]));
        assert_eq!(spans, vec![MdSpan::Text("https:// is empty".into())]);
    }

    #[test]
    fn mention_boundary_is_unicode_aware() {
        assert!(mention_start_boundary(None));
        assert!(mention_start_boundary(Some(' ')));
        assert!(!mention_start_boundary(Some('a')));
        assert!(!mention_start_boundary(Some('é')));
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
