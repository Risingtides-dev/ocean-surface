use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
struct CssRule {
    selectors: BTreeSet<String>,
    declarations: BTreeMap<String, String>,
}

fn realtime_composer_css() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../styles/composer.css");
    let css = std::fs::read_to_string(path).expect("read root styles/composer.css");
    let start = css
        .find("/* ── Realtime voice chat")
        .expect("composer.css has realtime voice section comment");
    let end = css[start..]
        .find(".voice-chat-active .voice-wrap")
        .map(|idx| start + idx)
        .expect("composer.css has realtime voice-wrap section");
    css[start..end].to_string()
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

fn parse_flat_css_rules(css: &str) -> Vec<CssRule> {
    let css = strip_css_comments(css);
    let mut rules = Vec::new();
    let mut rest = css.as_str();

    while let Some(open) = rest.find('{') {
        let selector_text = rest[..open].trim();
        let after_open = &rest[open + 1..];
        let close = after_open
            .find('}')
            .expect("every CSS rule opened in realtime section closes");
        let declaration_text = &after_open[..close];

        if !selector_text.is_empty() {
            let selectors = selector_text
                .split(',')
                .map(|selector| selector.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|selector| !selector.is_empty())
                .collect::<BTreeSet<_>>();
            let declarations = declaration_text
                .split(';')
                .filter_map(|declaration| {
                    let (name, value) = declaration.split_once(':')?;
                    Some((name.trim().to_string(), value.trim().to_string()))
                })
                .collect::<BTreeMap<_, _>>();
            rules.push(CssRule {
                selectors,
                declarations,
            });
        }

        rest = &after_open[close + 1..];
    }

    rules
}

#[test]
fn realtime_active_css_hides_all_composer_controls_in_one_completed_rule() {
    let rules = parse_flat_css_rules(&realtime_composer_css());
    let expected_selectors = BTreeSet::from([
        ".voice-chat-active .ocean-composer__input".to_string(),
        ".voice-chat-active .ocean-composer__send".to_string(),
        ".voice-chat-active .ocean-composer__halt".to_string(),
        ".voice-chat-active .ocean-turn-controls".to_string(),
        ".voice-chat-active .ocean-slash-menu".to_string(),
    ]);

    let matching_rule = rules
        .iter()
        .find(|rule| rule.selectors == expected_selectors);

    assert!(
        matching_rule
            .and_then(|rule| rule.declarations.get("display"))
            .is_some_and(|value| value == "none"),
        "expected one completed .voice-chat-active rule with selectors {expected_selectors:?} and display:none so the composer input, Send/Stop action slot, turn controls, and slash menu all hide together; parsed realtime rules were {rules:#?}"
    );
}
