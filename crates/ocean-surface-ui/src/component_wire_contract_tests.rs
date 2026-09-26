//! Surface renders exactly the component kinds ocean-os publishes
//! (`docs/contracts/component-wire.json`, vendored at
//! `tests/fixtures/ocean-os-component-wire/`).
//!
//! `ComponentView` is a Leptos component a host test cannot mount, so its
//! dispatch is read off the source (non-test half only).

use crate::daemon::AGENT_EVENT_NAMES;
use crate::wire_contract_support::*;

const COMPONENTS_RS: &str = include_str!("components.rs");
const REALTIME_RS: &str = include_str!("voice/realtime.rs");

#[test]
fn component_wire_component_view_renders_exactly_the_published_kinds() {
    let source = Source::production(COMPONENTS_RS);
    let arms = source.match_arm_literals("pub fn ComponentView(");
    let kinds = strings(&component_wire(), "/kinds");
    // The runtime forwards an unknown kind with a warning, so the fallback arm
    // stays; the named arms are a closed set and must EQUAL the published one:
    // a published kind with no arm renders as "unknown component kind", and an
    // arm for an unpublished kind is dead or out of contract.
    let missing: Vec<_> = kinds.difference(&arms).collect();
    let extra: Vec<_> = arms.difference(&kinds).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "ComponentView has no arm for published kinds {missing:?} and arms for \
         unpublished kinds {extra:?}"
    );
    assert!(
        source
            .body_after("pub fn ComponentView(")
            .contains("other => view!"),
        "ComponentView lost its fallback arm for an unknown kind"
    );
}

#[test]
fn component_wire_render_events_are_subscribed_and_published() {
    let contract = component_wire();
    let agent_events = strings(&session_wire(), "/agent_event_types");
    for pointer in ["/render_event_type", "/unmount_event_type"] {
        let event = string(&contract, pointer);
        assert!(
            AGENT_EVENT_NAMES.contains(&event.as_str()),
            "the agent stream does not subscribe to `{event}`"
        );
        assert!(agent_events.contains(&event), "{event}");
    }
}

#[test]
fn component_wire_voice_render_default_kind_is_published() {
    // The realtime agent's `render_component` falls back to a kind when the
    // model sends none; that kind must be one the surface renders.
    let literals = Source::production(REALTIME_RS).literals_after("fn parse_render_args(");
    let kinds = strings(&component_wire(), "/kinds");
    assert!(literals.contains("markdown"), "{literals:?}");
    assert!(kinds.contains("markdown"));
}
